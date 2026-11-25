//! `shaiya clients ...`: the saved "my client folders" list the toolkit programs share.

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use std::io::Write;
use shaiya_archive::cleanup;
use shaiya_archive::client::{self, read_version_ini, ClientFiles};
use shaiya_archive::health::{self, Kind};
use shaiya_archive::journal;
use shaiya_archive::Index;
use shaiya_core::config::{self, ClientFolder};
use shaiya_core::time::utc_iso;
use shaiya_core::{Error, Job, Result};
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// List the saved client folders
    ///
    /// Any ARCHIVE or CLIENT argument may be @name for one of these. Shaiya Toolkit shows the same list.
    ///
    /// Example: shaiya clients list
    List,
    /// Save a client folder under a name
    ///
    /// Example: shaiya clients add us "C:\Games\Shaiya" --note "US 108"
    Add(AddArgs),
    /// Forget a saved name (the files are not touched)
    ///
    /// Example: shaiya clients remove us
    Remove(NameArgs),
    /// Show a saved client folder: archive, Version.ini, game.exe, changes made by the toolkit
    ///
    /// Example: shaiya clients show us
    Show(NameArgs),
    /// Look at a client folder: index check, dead space, journals, leftovers (read only)
    ///
    /// Exit 0 when nothing is found, 1 when something is, 3 when the index cannot be read. Writes nothing.
    ///
    /// Example: shaiya clients health us
    Health(HealthArgs),
    /// Move leftovers into shaiya-toolkit-backups/cleanup-UTC (none of them is deleted), or put them back
    ///
    /// Kinds: sah-bak, stray-temp, swap-pair, editor-backup, editor-first-copy.
    ///
    /// Example: shaiya clients clean us --move stray-temp --dry-run
    Clean(CleanArgs),
}

#[derive(clap::Args)]
pub struct AddArgs {
    /// 1 to 40 characters, no / \ or @
    pub name: String,
    /// The client folder: it holds data.sah, or it is a loose data folder (see docs/tools/shaiya.md)
    pub path: PathBuf,
    #[arg(long, default_value = "")]
    pub note: String,
    /// For a loose data folder: the backup folder (outside the data folder); without it the client is read-only until one is chosen
    #[arg(long, value_name = "DIR")]
    pub backups: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct NameArgs {
    pub name: String,
}

pub fn run(ctx: &Ctx, cmd: &Cmd) -> i32 {
    match go(ctx, cmd) {
        Ok(code) => code,
        Err(e) => fail(ctx, &e),
    }
}

fn root(ctx: &Ctx) -> Result<PathBuf> {
    crate::config::data_root(ctx.data_dir.as_deref()).ok_or_else(|| Error::Unsupported("no data folder can be determined on this system (set SHAIYA_TOOLKIT_HOME or use --data-dir)".to_string()))
}

fn strip(name: &str) -> &str {
    name.strip_prefix('@').unwrap_or(name)
}

fn go(ctx: &Ctx, cmd: &Cmd) -> Result<i32> {
    match cmd {
        Cmd::List => {
            let clients = crate::config::load_clients(ctx.data_dir.as_deref())?;
            if ctx.json {
                let v: Vec<Value> = clients
                    .iter()
                    .map(|c| {
                        let mut v = serde_json::to_value(c).unwrap_or(Value::Null);
                        v["exists"] = json!(c.path.is_dir());
                        v
                    })
                    .collect();
                print_json(&json!({"clients": v}));
            } else if clients.is_empty() {
                println!("No saved client folders. Add one with: shaiya clients add NAME PATH");
            } else {
                let width = clients.iter().map(|c| c.name.chars().count()).max().unwrap_or(0).max(4);
                let mut w = out();
                for c in &clients {
                    let kind = if c.kind == config::ClientKind::Loose { "  [loose]" } else { "" };
                    outln!(w, "{:<width$}  {}  {}  {}{kind}", c.name, if c.path.is_dir() { "found  " } else { "missing" }, c.path.display(), c.note);
                }
            }
            Ok(0)
        }
        Cmd::Add(a) => {
            config::validate_client_name(&a.name)?;
            let root = root(ctx)?;
            let abs = std::path::absolute(&a.path).unwrap_or_else(|_| a.path.clone());
            let entry = match loose_layout(&abs)? {
                Some(layout) => {
                    if let Some(b) = &a.backups {
                        if let Some(why) = shaiya_archive::loose::backup_folder_problem(&layout, b) {
                            return Err(Error::Refused(why));
                        }
                    }
                    ClientFolder {
                        name: a.name.clone(),
                        path: layout.root.clone(),
                        added_utc: utc_iso(std::time::SystemTime::now()),
                        note: a.note.clone(),
                        kind: config::ClientKind::Loose,
                        data: (layout.data != layout.root).then(|| layout.data.clone()),
                        backups: a.backups.as_ref().map(|b| std::path::absolute(b).unwrap_or_else(|_| b.clone())),
                    }
                }
                None => {
                    if a.backups.is_some() {
                        return Err(Error::Refused("--backups is for a loose data folder; this folder holds data.sah".to_string()));
                    }
                    let files = ClientFiles::locate(&a.path)?;
                    let path = std::path::absolute(&files.dir).unwrap_or(files.dir.clone());
                    ClientFolder::archive(a.name.clone(), path, utc_iso(std::time::SystemTime::now()), a.note.clone())
                }
            };
            config::update_clients(&root, &mut |list| {
                if config::find_client(list, &a.name).is_some() {
                    return Err(Error::Refused(format!("a client named {:?} is already saved (shaiya clients remove it first)", a.name)));
                }
                list.push(entry.clone());
                Ok(())
            })?;
            if ctx.json {
                print_json(&json!({"added": serde_json::to_value(&entry).unwrap_or(Value::Null)}));
            } else {
                println!("saved {} -> {}  (use it as \"@{}\")", a.name, entry.path.display(), a.name);
            }
            Ok(0)
        }
        Cmd::Remove(a) => {
            let name = strip(&a.name).to_string();
            let root = root(ctx)?;
            let mut found = false;
            config::update_clients(&root, &mut |list| {
                let before = list.len();
                list.retain(|c| !c.name.eq_ignore_ascii_case(&name));
                found = list.len() != before;
                Ok(())
            })?;
            if !found {
                return Err(Error::io(PathBuf::from(format!("@{name}")), std::io::Error::new(std::io::ErrorKind::NotFound, "no saved client folder with this name (hint: shaiya clients list)")));
            }
            if ctx.json {
                print_json(&json!({"removed": name}));
            } else {
                println!("forgot {name} (its files are untouched)");
            }
            Ok(0)
        }
        Cmd::Health(a) => health_cmd(ctx, a),
        Cmd::Clean(a) => clean_cmd(ctx, a),
        Cmd::Show(a) => {
            let name = strip(&a.name);
            let clients = crate::config::load_clients(ctx.data_dir.as_deref())?;
            let c = config::find_client(&clients, name)
                .ok_or_else(|| Error::io(PathBuf::from(format!("@{name}")), std::io::Error::new(std::io::ErrorKind::NotFound, "no saved client folder with this name (hint: shaiya clients list)")))?;
            if c.kind == config::ClientKind::Loose {
                return show_loose(ctx, c);
            }
            let files = ClientFiles::locate(&c.path)?;
            let idx = Index::read(&files.sah)?;
            let ini = read_version_ini(&files).map(|i| (i.check_version, i.current_version));
            let journals = journal::list(&c.path, &c.path.join(journal::DEFAULT_DIR)).map(|l| l.len()).unwrap_or(0);
            if ctx.json {
                print_json(&json!({
                    "name": c.name, "path": c.path.display().to_string(), "note": c.note, "added_utc": c.added_utc, "sah_version": idx.version, "files": idx.file_count(),
                    "check_version": ini.and_then(|i| i.0), "current_version": ini.and_then(|i| i.1),
                    "game_exe": files.game_exe.is_some(), "journals": journals
                }));
            } else {
                let fmt = |v: Option<u32>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
                println!("{}  {}", c.name, c.path.display());
                println!("  archive:   SAH version {}, {}", idx.version, count(idx.file_count(), "file"));
                println!("  Version.ini: CheckVersion {}, CurrentVersion {}", fmt(ini.and_then(|i| i.0)), fmt(ini.and_then(|i| i.1)));
                println!("  game.exe:  {}", if files.game_exe.is_some() { "present" } else { "not found" });
                println!("  changes made by the toolkit: {journals}");
            }
            Ok(0)
        }
    }
}

#[derive(clap::Args)]
pub struct HealthArgs {
    /// A saved client name (or @name), or a client folder
    pub client: String,
}

#[derive(clap::Args)]
pub struct CleanArgs {
    /// A saved client name (or @name), or a client folder
    #[arg(required_unless_present = "put_back", conflicts_with = "put_back")]
    pub client: Option<String>,
    /// The kinds to move (repeat the flag or separate with commas): sah-bak, stray-temp, swap-pair, editor-backup, editor-first-copy
    #[arg(long = "move", value_name = "KIND", value_delimiter = ',', value_parser = parse_kind, required_unless_present = "put_back", conflicts_with = "put_back")]
    pub kinds: Vec<Kind>,
    /// Show what would be moved; nothing is changed
    #[arg(long)]
    pub dry_run: bool,
    /// Move everything a cleanup folder lists back to where it was
    #[arg(long, value_name = "DIR")]
    pub put_back: Option<PathBuf>,
}

fn parse_kind(s: &str) -> std::result::Result<Kind, String> {
    Kind::parse(s).ok_or_else(|| format!("unknown kind {s:?}; the kinds are {}", Kind::ALL.map(|k| k.name()).join(", ")))
}

/// NAME (a saved client), @name, or a folder.
fn target(ctx: &Ctx, arg: &str) -> Result<PathBuf> {
    let clients = crate::config::load_clients(ctx.data_dir.as_deref())?;
    match config::find_client(&clients, strip(arg)) {
        Some(c) => {
            refuse_loose(arg, &c.path)?;
            Ok(c.path.clone())
        }
        None => archive_client_dir(ctx, arg),
    }
}

fn health_cmd(ctx: &Ctx, a: &HealthArgs) -> Result<i32> {
    let dir = target(ctx, &a.client)?;
    let h = health::report(&dir)?;
    if ctx.json {
        let mut v = serde_json::to_value(&h).unwrap_or(Value::Null);
        v["client"] = json!(dir.display().to_string());
        v["healthy"] = json!(h.healthy());
        v["findings"] = json!(h.findings());
        print_json(&v);
        return Ok(i32::from(!h.healthy()));
    }
    let mut w = out();
    outln!(w, "{}", dir.display());
    outln!(w, "  index:       SAH version {}, {}, {}, {}", h.index.sah_version, count(h.index.files, "file"), count(h.index.problems, "problem"), count(h.index.warnings, "warning"));
    match &h.dead_space {
        Some(d) => outln!(w, "  data.saf:    {}, {} in use, {} dead space{}", size(d.saf_bytes), size(d.live_bytes), size(d.dead_bytes), if d.dead_bytes > 0 { " (shaiya archive compact makes it smaller; it makes no journal)" } else { "" }),
        None => outln!(w, "  data.saf:    missing"),
    }
    outln!(w, "  journals:    {} ({})", h.journals.count, size(h.journals.bytes));
    match &h.pending_marker {
        Some(m) if m.readable => outln!(w, "  pending:     {} of {} did not finish; opening the client in Shaiya Toolkit or any write command finishes or removes it", m.op, m.label),
        Some(_) => outln!(w, "  pending:     data.toolkit-pending cannot be read (see docs/guides/undo-and-recovery.md)"),
        None => outln!(w, "  pending:     none"),
    }
    if let Some(s) = &h.start_update {
        outln!(w, "  Version.ini: StartUpdate={s}: an original-updater run did not finish; the toolkit never edits this");
    }
    if h.leftovers.is_empty() {
        outln!(w, "  leftovers:   none");
    }
    for l in &h.leftovers {
        outln!(w, "  leftover:    {}  {}  ({}){}", l.kind.name(), l.files.join(" + "), size(l.bytes), if l.pre_selected { "" } else { "  [not pre-selected in the toolkit]" });
        outln!(w, "               {}", l.note);
    }
    for n in &h.not_offered {
        outln!(w, "  not offered: {}  ({})", n.file, n.why);
    }
    for c in &h.cleanups {
        outln!(w, "  cleanup:     {}  {}, {} still there to put back ({})", c.name, c.state, c.to_put_back, size(c.bytes));
    }
    outln!(w, "{}", if h.healthy() { "healthy: nothing to report".to_string() } else { format!("{} to look at", count(h.findings(), "finding")) });
    Ok(i32::from(!h.healthy()))
}

fn clean_cmd(ctx: &Ctx, a: &CleanArgs) -> Result<i32> {
    if let Some(folder) = &a.put_back {
        let r = cleanup::put_back(folder, &Job::silent())?;
        if ctx.json {
            print_json(&json!({"put_back": serde_json::to_value(&r).unwrap_or(Value::Null)}));
        } else {
            println!("put back {} ({} already in place)", count(r.restored.len(), "file"), r.already_in_place.len());
        }
        if !r.missing.is_empty() {
            warn(ctx, &format!("not found in the cleanup folder or in place: {}", r.missing.join(", ")));
            return Ok(3);
        }
        return Ok(0);
    }
    let dir = target(ctx, a.client.as_deref().unwrap_or_default())?;
    let kinds = a.kinds.clone();
    let pick = move |h: &health::Health| -> Result<Vec<String>> {
        for k in &kinds {
            let held = matches!(k, Kind::EditorBackup | Kind::EditorFirstCopy) && h.not_offered.iter().any(|n| n.why.contains(health::EDITOR_RECORD));
            if held && !h.leftovers.iter().any(|l| l.kind == *k) {
                return Err(Error::Refused(format!("{} is not offered while {} exists in the client folder (the map editor's Uninstall uses those backups)", k.name(), health::EDITOR_RECORD)));
            }
        }
        Ok(cleanup::ids_of_kinds(h, &kinds))
    };
    if a.dry_run {
        let h = health::report(&dir)?;
        let plan = cleanup::plan(&h, &pick(&h)?)?;
        if ctx.json {
            let items: Vec<Value> = plan.items.iter().map(|l| json!({"kind": l.kind.name(), "id": l.id, "files": l.files, "bytes": l.bytes})).collect();
            print_json(&json!({"dry_run": true, "would_move": items, "files": plan.files, "bytes": plan.bytes}));
        } else if plan.items.is_empty() {
            println!("nothing to move");
        } else {
            for l in &plan.items {
                println!("would move {}  {}  ({})", l.kind.name(), l.files.join(" + "), size(l.bytes));
            }
            println!("{} ({}) would go to shaiya-toolkit-backups/cleanup-<UTC>/; nothing was changed", count(plan.files, "file"), size(plan.bytes));
        }
        return Ok(0);
    }
    if let Ok(files) = ClientFiles::locate(&dir) {
        if client::in_use(&files) == client::InUse::Unknown {
            warn(ctx, "cannot check whether the game is running on this system; make sure it is closed");
        }
    }
    let r = cleanup::clean_by(&dir, &pick, &Job::silent())?;
    for n in &r.notes {
        note(ctx, &format!("note: {n}"));
    }
    if ctx.json {
        print_json(&json!({"folder": r.folder, "path": r.path.display().to_string(), "moved": r.moved, "bytes": r.bytes, "notes": r.notes}));
    } else if r.moved.is_empty() {
        println!("nothing to move");
    } else {
        println!("moved {} ({}) to {}", count(r.moved.len(), "file"), size(r.bytes), r.path.display());
        println!("undo with: shaiya clients clean --put-back {}", shell_arg(&r.path.display().to_string()));
    }
    Ok(0)
}

/// `clients show` of a loose client: where its folders are, how many files it lists, whether it is a Shaiya-GX layout (read from Shaiya-GX's
/// data-format document), and whether a backup folder is confirmed.
fn show_loose(ctx: &Ctx, c: &ClientFolder) -> Result<i32> {
    let layout = loose_layout(&c.path)?.ok_or_else(|| Error::Refused(format!("{} is no longer a loose data folder ({})", c.path.display(), shaiya_archive::loose::detect_hint())))?;
    let tree = shaiya_clientdata::LooseTree::open(&layout, &Job::silent())?;
    let notes = tree.notes();
    let backups_problem = match &c.backups {
        None => Some("choose a backup folder outside the data folder".to_string()),
        Some(b) => shaiya_archive::loose::backup_folder_problem(&layout, b),
    };
    if ctx.json {
        print_json(&json!({
            "name": c.name, "path": c.path.display().to_string(), "kind": "loose", "note": c.note, "added_utc": c.added_utc,
            "data": layout.data.display().to_string(), "files": tree.files().len(), "gx_layout": layout.gx,
            "backups": c.backups.as_ref().map(|b| b.display().to_string()), "read_only_reason": backups_problem, "notes": notes
        }));
    } else {
        println!("{}  {}", c.name, c.path.display());
        println!("  kind:      loose folder, {}", layout.describe());
        println!("  files:     {}", count(tree.files().len(), "file"));
        match &backups_problem {
            None => println!("  backups:   {}", c.backups.as_ref().map(|b| b.display().to_string()).unwrap_or_default()),
            Some(why) => println!("  backups:   none confirmed ({why}); read-only"),
        }
        for n in &notes {
            println!("  note:      {n}");
        }
    }
    Ok(0)
}
