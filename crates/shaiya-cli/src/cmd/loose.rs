//! `shaiya loose ...`: list, add, replace, remove, apply a patch, history, rollback, prune, health and recover for a loose data folder (a
//! client with no `data.sah`; see docs/formats/loose-client.md). Every write goes through `shaiya_archive::loose_edit` (journal in the
//! confirmed backup folder, atomic checked writes, undo), with the file checks built from the readers (`shaiya_inspect::loose`).

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use shaiya_archive::journal::{Rollback, State};
use shaiya_archive::loose::{self, LooseLayout};
use shaiya_archive::loose_edit::{self, Busy, LooseChange, LooseOptions, LoosePlan, LoosePut, MAX_PUT_BYTES, UNKNOWN_BUSY_TEXT};
use shaiya_clientdata::LooseTree;
use shaiya_core::config::{self, ClientKind};
use shaiya_core::glob::Pattern;
use shaiya_core::{Error, Job, Result};
use shaiya_inspect::loose as checks;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(clap::Args, Clone)]
pub struct Common {
    /// The backup folder (outside the data folder) for a folder given by path, or for a saved client that has none yet
    #[arg(long, value_name = "DIR")]
    pub backups: Option<PathBuf>,
    /// Say that the game is stopped, when this system cannot tell (else the question is asked on a terminal, and without one the change is refused)
    #[arg(long)]
    pub yes_game_stopped: bool,
}

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// List the files of a loose client
    ///
    /// Example: shaiya loose list "@gx" "Character/*/ANI/*.ANI"
    List(ListArgs),
    /// Add new files to a loose client (a file that is already there is refused: use replace)
    ///
    /// The file goes to --at DIR (default: the top of the data folder) under its own name, or to the path --as PATH. The change is
    /// checked first (names, links, converted Shaiya-GX media, the file readers); the journal and the originals go to the backup folder.
    ///
    /// Example: shaiya loose add "@gx" new.ANI --at Character/Human/ANI
    Add(PutArgs),
    /// Replace files of a loose client (a file that is not there is refused: use add)
    ///
    /// Example: shaiya loose replace "@gx" huwm_117.ANI --as Character/Human/ANI/huwm_117.ANI
    Replace(PutArgs),
    /// Remove files from a loose client (a copy is kept in the journal; rollback brings them back)
    ///
    /// Example: shaiya loose remove "@gx" Item/old.3DO
    Remove(RemoveArgs),
    /// Apply a patch to a loose client: its files and removals become one change with its own journal
    ///
    /// Example: shaiya loose apply "@gx" ps0109.patch
    Apply(ApplyArgs),
    /// List the changes the toolkit made to a loose client and whether each can be rolled back
    History(ClientArgs),
    /// Undo the newest change (only while the files are as the change left them)
    #[command(visible_alias = "undo")]
    Rollback(RollbackArgs),
    /// Delete the oldest journals, keeping the newest KEEP
    Prune(PruneArgs),
    /// Read-only look at a loose client: unreadable files, files that fail their reader, model textures found nowhere, leftovers
    Health(ClientArgs),
    /// Put right an interrupted change or rollback of a loose client
    Recover(ClientArgs),
}

#[derive(clap::Args)]
pub struct ClientArgs {
    /// `@name` of a saved loose client, or a loose data folder
    pub client: String,
    #[command(flatten)]
    pub common: Common,
}

#[derive(clap::Args)]
pub struct ListArgs {
    pub client: String,
    /// Only paths matching one of these patterns (case-insensitive, `*` and `?`)
    pub patterns: Vec<String>,
}

#[derive(clap::Args)]
pub struct PutArgs {
    pub client: String,
    /// The files to put into the data folder
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    /// The folder below the data folder they go to (default: the top)
    #[arg(long, value_name = "DIR")]
    pub at: Option<String>,
    /// The whole path below the data folder (one file only)
    #[arg(long = "as", value_name = "PATH")]
    pub as_path: Option<String>,
    /// Show the checked change and stop
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub common: Common,
}

#[derive(clap::Args)]
pub struct RemoveArgs {
    pub client: String,
    /// Paths below the data folder
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<String>,
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub common: Common,
}

#[derive(clap::Args)]
pub struct ApplyArgs {
    pub client: String,
    pub patch: PathBuf,
    #[arg(long)]
    pub dry_run: bool,
    /// Require a valid signature from this public key file on the patch (checked before anything is read, and again right before the write)
    #[arg(long, value_name = "PUBLIC.key", conflicts_with = "public_hex")]
    pub key: Option<PathBuf>,
    #[arg(long, value_name = "HEX")]
    pub public_hex: Option<String>,
    #[arg(long, value_name = "N")]
    pub check_version: Option<u32>,
    #[command(flatten)]
    pub common: Common,
}

#[derive(clap::Args)]
pub struct RollbackArgs {
    pub client: String,
    /// The change to undo (default: the newest applied one)
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,
    /// Restore files even if they changed since (each changed file is saved first, inside the journal folder)
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[command(flatten)]
    pub common: Common,
}

#[derive(clap::Args)]
pub struct PruneArgs {
    pub client: String,
    /// Keep the newest KEEP journals
    #[arg(long, value_name = "KEEP")]
    pub keep: usize,
    #[command(flatten)]
    pub common: Common,
}

pub fn run(ctx: &Ctx, cmd: &Cmd, job: &Job) -> i32 {
    let r = match cmd {
        Cmd::List(a) => list(ctx, a),
        Cmd::Add(a) => put_cmd(ctx, a, false, job),
        Cmd::Replace(a) => put_cmd(ctx, a, true, job),
        Cmd::Remove(a) => remove_cmd(ctx, a, job),
        Cmd::Apply(a) => apply_cmd(ctx, a, job),
        Cmd::History(a) => history(ctx, a),
        Cmd::Rollback(a) => rollback(ctx, a, job),
        Cmd::Prune(a) => prune(ctx, a),
        Cmd::Health(a) => health(ctx, a, job),
        Cmd::Recover(a) => recover(ctx, a),
    };
    match r {
        Ok(code) => code,
        Err(e) => fail(ctx, &e),
    }
}

// ------------------------------------------------------------------------------------------------------ the client

/// The layout of the CLIENT argument: a saved loose client (its saved folders and backup folder) or a folder that the detection accepts
/// (its backup folder is `--backups`). An archive client or any other folder is refused with exit 2.
fn client_layout(ctx: &Ctx, arg: &str, backups: Option<&Path>) -> Result<LooseLayout> {
    let given = backups.map(|b| std::path::absolute(b).unwrap_or_else(|_| b.to_path_buf()));
    let mut layout = if let Some(name) = arg.strip_prefix('@') {
        let clients = crate::config::load_clients(ctx.data_dir.as_deref())?;
        let c = config::find_client(&clients, name).ok_or_else(|| {
            Error::io(PathBuf::from(arg), std::io::Error::new(std::io::ErrorKind::NotFound, "no saved client folder with this name (hint: shaiya clients list)"))
        })?;
        if !c.kind.is_supported() {
            return Err(Error::Refused(crate::config::unsupported_text(c)));
        }
        if c.kind != ClientKind::Loose {
            return Err(not_loose(arg));
        }
        let root = c.path.clone();
        let data = c.data_folder().to_path_buf();
        let gx = loose::probe_gx(&data);
        LooseLayout { root, data, gx, backups: c.backups.clone() }
    } else {
        let path = PathBuf::from(arg);
        match loose_layout(&path)? {
            Some(l) => l,
            None => return Err(not_loose(arg)),
        }
    };
    match (&layout.backups, given) {
        (None, Some(g)) => layout.backups = Some(g),
        (Some(saved), Some(g)) if !shaiya_core::fs::same_file(saved, &g) => {
            return Err(Error::Refused(format!("--backups {} differs from the backup folder saved for {arg} ({}); change it in the Toolkit's client list", g.display(), saved.display())))
        }
        _ => {}
    }
    Ok(layout)
}

/// Before a write: the backup folder is not the backup folder of another saved loose client and not inside its data folder (their journals
/// and their marker would be mixed). The library filters every journal by its data folder as well; this stops the mix at the door.
fn own_backups(ctx: &Ctx, layout: &LooseLayout) -> Result<()> {
    let Some(b) = &layout.backups else { return Ok(()) };
    let clients = crate::config::load_clients(ctx.data_dir.as_deref())?;
    let others: Vec<loose::OtherClient> = clients
        .iter()
        .filter(|c| c.kind == ClientKind::Loose && !shaiya_core::fs::same_file(c.data_folder(), &layout.data))
        .map(|c| loose::OtherClient { name: c.name.clone(), data: c.data_folder().to_path_buf(), backups: c.backups.clone() })
        .collect();
    match loose::backup_folder_shared_problem(b, &others) {
        Some(why) => Err(Error::Refused(why)),
        None => Ok(()),
    }
}

fn not_loose(arg: &str) -> Error {
    usage_refusal(format!("{arg} is not a loose data folder (a saved loose client, or a folder with no data.sah that holds Character, Item, Monster, World or Effect folders): the shaiya loose commands do not work on it; use shaiya archive or shaiya patch for an archive client"))
}

fn tree_of(layout: &LooseLayout, job: &Job) -> Result<LooseTree> {
    LooseTree::open(layout, job)
}

fn check_fn(rel: &str, bytes: &[u8]) -> Result<()> {
    checks::format_check(rel, bytes)
}

fn options(label: &str, patch: bool, confirmed: bool) -> LooseOptions {
    LooseOptions { label: label.to_string(), patch, confirmed_unknown_busy: confirmed, crash_after_puts: None }
}

/// The game question: `Busy` is left to `apply` to refuse; `Unknown` needs `--yes-game-stopped` or an answer on a terminal.
fn game_stopped(ctx: &Ctx, layout: &LooseLayout, common: &Common) -> Result<bool> {
    match loose_edit::busy_state(layout) {
        Busy::Unknown => {
            if common.yes_game_stopped {
                return Ok(true);
            }
            if !std::io::stdin().is_terminal() || ctx.json {
                return Err(Error::Refused(format!("{UNKNOWN_BUSY_TEXT} (say that it is stopped with --yes-game-stopped)")));
            }
            eprint!("{UNKNOWN_BUSY_TEXT}. Is the game stopped? [y/N] ");
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line).map_err(Error::from)?;
            if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                Ok(true)
            } else {
                Err(Error::Refused(format!("{UNKNOWN_BUSY_TEXT}; nothing was changed")))
            }
        }
        _ => Ok(false),
    }
}

/// The confirmation lines (Changes, Writes, Undo, Game) of a plan, on stderr.
fn show_plan(ctx: &Ctx, layout: &LooseLayout, plan: &LoosePlan, client_arg: &str) {
    let (added, replaced, removed) = plan.counts();
    note(ctx, &format!("Changes: {} added, {} replaced, {} removed", added, replaced, removed));
    note(ctx, &format!("Writes:  {} written in the data folder {}; originals and the journal go to {}", size(plan.bytes_written()), layout.data.display(), layout.backups.as_ref().map(|b| b.display().to_string()).unwrap_or_default()));
    note(ctx, &format!("Undo:    shaiya loose rollback {} (while the files are as this change leaves them)", shell_arg(client_arg)));
    note(ctx, "Game:    restart the game to see the change");
    if plan.gx {
        note(ctx, "Layout:  Shaiya-GX (converted media is never changed)");
    }
    for p in plan.new_dirs.iter().take(5) {
        note(ctx, &format!("new folder: {p}"));
    }
}

fn plan_json(plan: &LoosePlan) -> Value {
    json!({
        "puts": plan.puts.iter().map(|p| json!({"path": p.rel, "replaces": p.existed, "size": p.len})).collect::<Vec<_>>(),
        "removes": plan.removes.iter().map(|r| json!({"path": r.rel, "size": r.len})).collect::<Vec<_>>(),
        "new_folders": plan.new_dirs, "gx": plan.gx
    })
}

// ------------------------------------------------------------------------------------------------------ list

fn list(ctx: &Ctx, a: &ListArgs) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, None)?;
    let tree = tree_of(&layout, &Job::silent())?;
    for n in tree.notes() {
        note(ctx, &format!("note: {n}"));
    }
    let pats: Vec<Pattern> = a.patterns.iter().map(|p| Pattern::new(p)).collect::<Result<_>>()?;
    let files: Vec<_> = tree.files().iter().filter(|f| pats.is_empty() || pats.iter().any(|p| p.matches(&f.rel))).collect();
    if ctx.json {
        print_json(&json!({
            "data": layout.data.display().to_string(), "gx_layout": layout.gx,
            "files": files.iter().map(|f| json!({"path": f.rel, "size": f.len})).collect::<Vec<_>>(), "shown": files.len(), "total": tree.files().len()
        }));
        return Ok(0);
    }
    let mut w = out();
    for f in &files {
        outln!(w, "{:>12}  {}", f.len, f.rel);
    }
    outln!(w, "{} of {}", count(files.len(), "file"), count(tree.files().len(), "file"));
    Ok(0)
}

// ------------------------------------------------------------------------------------------------------ add, replace, remove, apply

/// `Ok` change of a put command: each file read after its size was checked, named by `--as`, or `--at` + its own name.
fn change_of_files(a: &PutArgs, replace: bool, layout: &LooseLayout) -> Result<LooseChange> {
    if a.as_path.is_some() && a.files.len() != 1 {
        return Err(Error::Refused("--as names one file: give exactly one FILE with it".to_string()));
    }
    if a.as_path.is_some() && a.at.is_some() {
        return Err(Error::Refused("use either --as PATH or --at DIR, not both".to_string()));
    }
    let mut puts = Vec::new();
    for f in &a.files {
        let rel = match (&a.as_path, &a.at) {
            (Some(p), _) => p.clone(),
            (None, Some(dir)) => format!("{}/{}", dir.trim_matches(['/', '\\']), file_name(f)),
            (None, None) => file_name(f),
        };
        puts.push(LoosePut { rel, bytes: loose_edit::read_capped(f, MAX_PUT_BYTES)? });
    }
    // add refuses what is there, replace refuses what is not: both before anything is read further
    let mut problems = Vec::new();
    for p in &puts {
        let there = loose_edit::file_exists(layout, &p.rel);
        if !replace && there {
            problems.push(format!("add {}: already in the data folder (use shaiya loose replace)", p.rel));
        }
        if replace && !there {
            problems.push(format!("replace {}: not in the data folder (use shaiya loose add)", p.rel));
        }
    }
    if !problems.is_empty() {
        return Err(Error::Refused(if problems.len() == 1 { problems.remove(0) } else { format!("{} problems:\n  {}", problems.len(), problems.join("\n  ")) }));
    }
    Ok(LooseChange { puts, removes: Vec::new() })
}

/// What a write is called, whether it is a patch, and whether it is only a dry run.
struct What<'a> {
    label: &'a str,
    patch: bool,
    dry_run: bool,
    /// The patch file and the key its signature must verify with, checked again right before the write.
    signed: Option<(&'a Path, shaiya_patch::VerifyKey)>,
}

fn finish(ctx: &Ctx, layout: &LooseLayout, plan: LoosePlan, common: &Common, what: What<'_>, client_arg: &str, job: &Job) -> Result<i32> {
    let (label, patch, dry_run) = (what.label, what.patch, what.dry_run);
    show_plan(ctx, layout, &plan, client_arg);
    if dry_run {
        if ctx.json {
            print_json(&json!({"dry_run": true, "plan": plan_json(&plan)}));
        } else {
            println!("dry run: nothing was written");
        }
        return Ok(0);
    }
    let confirmed = game_stopped(ctx, layout, common)?;
    if let Some((file, k)) = &what.signed {
        verify_signature(file, k, job)?;
    }
    let r = loose_edit::apply(layout, plan, &options(label, patch, confirmed), &check_fn, job)?;
    for n in &r.recovered {
        note(ctx, &format!("note: {n}"));
    }
    if ctx.json {
        print_json(&serde_json::to_value(&r).unwrap_or(Value::Null));
    } else {
        println!("{}: {} added, {} replaced, {} removed; journal {}", label, r.added.len(), r.replaced.len(), r.removed.len(), r.journal.display());
        println!("undo with: shaiya loose rollback {}", shell_arg(client_arg));
    }
    Ok(0)
}

/// The patch's `.sig` must verify with `key` (the refusal says why, nothing was read from the folder).
fn verify_signature(patch: &Path, key: &shaiya_patch::VerifyKey, job: &Job) -> Result<()> {
    shaiya_patch::sign::verify_file(&key.public_hex, key.check_version, patch, None, job).map_err(|e| match e {
        Error::Cancelled => Error::Cancelled,
        Error::Refused(m) => Error::Refused(m),
        other => Error::Refused(format!("signature check failed for {}: {other}", patch.display())),
    })
}

fn put_cmd(ctx: &Ctx, a: &PutArgs, replace: bool, job: &Job) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    own_backups(ctx, &layout)?;
    let change = change_of_files(a, replace, &layout)?;
    let plan = loose_edit::plan(&layout, change, &check_fn)?;
    finish(ctx, &layout, plan, &a.common, What { label: if replace { "loose replace" } else { "loose add" }, patch: false, dry_run: a.dry_run, signed: None }, &a.client, job)
}

fn remove_cmd(ctx: &Ctx, a: &RemoveArgs, job: &Job) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    own_backups(ctx, &layout)?;
    let plan = loose_edit::plan(&layout, LooseChange { puts: Vec::new(), removes: a.paths.clone() }, &check_fn)?;
    finish(ctx, &layout, plan, &a.common, What { label: "loose remove", patch: false, dry_run: a.dry_run, signed: None }, &a.client, job)
}

fn apply_cmd(ctx: &Ctx, a: &ApplyArgs, job: &Job) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    own_backups(ctx, &layout)?;
    let key = super::patch::key_of(&a.key, &a.public_hex, a.check_version)?;
    if let Some(k) = &key {
        verify_signature(&a.patch, k, job)?;
    }
    let (plan, missing) = shaiya_patch::loose::plan_loose(&a.patch, &layout, &check_fn, job)?;
    for m in &missing {
        note(ctx, &format!("note: remove {m}: not in the data folder, left out"));
    }
    let label = file_name(&a.patch);
    finish(ctx, &layout, plan, &a.common, What { label: &label, patch: true, dry_run: a.dry_run, signed: key.map(|k| (a.patch.as_path(), k)) }, &a.client, job)
}

// ------------------------------------------------------------------------------------------------------ history, rollback, prune

fn recovered(ctx: &Ctx, layout: &LooseLayout, common: &Common) -> Result<()> {
    if loose_edit::unfinished(layout).is_some() {
        let confirmed = game_stopped(ctx, layout, common)?;
        for n in loose_edit::recover(layout, &options("", false, confirmed))? {
            note(ctx, &format!("note: {n}"));
        }
    }
    Ok(())
}

fn history(ctx: &Ctx, a: &ClientArgs) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    if let Some(u) = loose_edit::unfinished(&layout) {
        note(ctx, &format!("note: {u}"));
    }
    if loose_edit::change_running(&layout) {
        note(ctx, &format!("note: {}", loose_edit::RUNNING_TEXT));
    }
    let all = loose_edit::list(&layout)?;
    let damaged = loose_edit::damaged_journals(&layout);
    for d in &damaged {
        note(ctx, &format!("note: the journal folder {d} cannot be read (it is not listed; it blocks the rollback of older changes)"));
    }
    if ctx.json {
        let items: Vec<Value> = all
            .iter()
            .map(|(j, r)| {
                let mut v = serde_json::to_value(j).unwrap_or(Value::Null);
                v["rollback"] = match r {
                    Rollback::Possible => json!({"possible": true, "reason": Value::Null}),
                    Rollback::Blocked(why) => json!({"possible": false, "reason": why}),
                };
                v
            })
            .collect();
        print_json(&json!({"journals": items, "damaged": damaged, "change_running": loose_edit::change_running(&layout)}));
        return Ok(0);
    }
    let mut w = out();
    if all.is_empty() {
        outln!(w, "No changes made by the toolkit to this client yet.");
    }
    for (j, r) in &all {
        let state = match &j.state {
            State::Pending => "pending".to_string(),
            State::Applied => "applied".to_string(),
            State::RolledBack { at_utc } => format!("rolled back {at_utc}"),
        };
        let rb = match r {
            Rollback::Possible => "possible".to_string(),
            Rollback::Blocked(why) => format!("blocked: {why}"),
        };
        outln!(w, "{}  {:<14} {:<12} {}  {:<24} rollback {rb}", j.id, j.label, j.kind, j.created_utc, state);
    }
    Ok(0)
}

fn rollback(ctx: &Ctx, a: &RollbackArgs, job: &Job) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    if !a.dry_run {
        own_backups(ctx, &layout)?;
        recovered(ctx, &layout, &a.common)?;
    }
    let all = loose_edit::list(&layout)?;
    let target = match &a.id {
        Some(id) => {
            // the whole id, or an ending of it that names exactly one journal: an ending that fits two is never guessed
            let hits: Vec<&(loose_edit::LooseJournal, Rollback)> = all.iter().filter(|(j, _)| &j.id == id).collect();
            let hits = if hits.is_empty() { all.iter().filter(|(j, _)| j.id.ends_with(id.as_str())).collect() } else { hits };
            match hits.len() {
                0 => return Err(Error::io(id, std::io::Error::new(std::io::ErrorKind::NotFound, "no such journal (hint: shaiya loose history)"))),
                1 => hits[0],
                n => return Err(Error::Refused(format!("--id {id} fits {n} journals ({}); give more of the id", hits.iter().map(|(j, _)| j.id.as_str()).collect::<Vec<_>>().join(", ")))),
            }
        }
        None => all.iter().rev().find(|(j, _)| j.state == State::Applied).ok_or_else(|| Error::Refused("nothing to roll back: the toolkit has made no changes to this client (or all were rolled back)".to_string()))?,
    };
    let (j, status) = target;
    if let Rollback::Blocked(why) = status {
        // "changed since" is the one reason --force lifts; every other reason stays
        if !(a.force && why.contains("rollback with force")) {
            return Err(Error::Refused(format!("cannot roll back {}: {why}", j.id)));
        }
    }
    if a.dry_run {
        if ctx.json {
            print_json(&json!({"dry_run": true, "journal": serde_json::to_value(j).unwrap_or(Value::Null)}));
        } else {
            println!(
                "would roll back {} (applied {}): restore {} and delete {}",
                j.label,
                j.created_utc,
                count(j.records.iter().filter(|r| r.existed).count(), "file"),
                count(j.records.iter().filter(|r| !r.existed).count(), "created file")
            );
        }
        return Ok(0);
    }
    let confirmed = game_stopped(ctx, &layout, &a.common)?;
    let r = loose_edit::rollback(&layout, &j.dir, a.force, &options("", false, confirmed), job)?;
    if ctx.json {
        print_json(&serde_json::to_value(&r).unwrap_or(Value::Null));
    } else {
        println!("rolled back {} (applied {}): {} restored, {} deleted", j.label, j.created_utc, r.restored.len(), r.deleted.len());
        for s in &r.saved {
            println!("saved before it was put back: {s} (in the journal folder, under saved/)");
        }
    }
    Ok(0)
}

fn prune(ctx: &Ctx, a: &PruneArgs) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    own_backups(ctx, &layout)?;
    let gone = loose_edit::prune(&layout, a.keep)?;
    if ctx.json {
        print_json(&json!({"pruned": gone}));
    } else {
        for id in &gone {
            println!("pruned {id}");
        }
        println!("{} deleted", count(gone.len(), "journal"));
    }
    Ok(0)
}

fn recover(ctx: &Ctx, a: &ClientArgs) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    own_backups(ctx, &layout)?;
    let confirmed = game_stopped(ctx, &layout, &a.common)?;
    let notes = loose_edit::recover(&layout, &options("", false, confirmed))?;
    if ctx.json {
        print_json(&json!({"recovered": notes}));
    } else if notes.is_empty() {
        println!("nothing to recover");
    } else {
        for n in &notes {
            println!("{n}");
        }
    }
    Ok(0)
}

// ------------------------------------------------------------------------------------------------------ health

fn health(ctx: &Ctx, a: &ClientArgs, job: &Job) -> Result<i32> {
    let layout = client_layout(ctx, &a.client, a.common.backups.as_deref())?;
    let tree = tree_of(&layout, job)?;
    let h = checks::health(&layout, &tree, job)?;
    if ctx.json {
        print_json(&json!({
            "files": h.files, "unreadable": h.unreadable, "unreadable_list": h.unreadable_list,
            "failing": h.failing, "failing_list": h.failing_list.iter().map(|(p, w)| json!({"path": p, "why": w})).collect::<Vec<_>>(),
            "unresolved_textures": h.unresolved, "unresolved_list": h.unresolved_list.iter().map(|(m, t)| json!({"mesh": m, "texture": t})).collect::<Vec<_>>(),
            "lists_refused": h.lists_refused, "temp_files": h.temps, "unfinished": h.unfinished, "change_running": h.running, "damaged_journals": h.damaged_journals, "notes": h.notes,
            "journals": h.journals.map(|(all, possible)| json!({"all": all, "rollback_possible": possible})), "clean": h.is_clean()
        }));
        return Ok(if h.is_clean() { 0 } else { 1 });
    }
    let mut w = out();
    outln!(w, "{} listed", count(h.files, "file"));
    outln!(w, "unreadable files:           {}", h.unreadable);
    for p in &h.unreadable_list {
        outln!(w, "  {p}");
    }
    outln!(w, "files that fail their reader: {}", h.failing);
    for (p, why) in &h.failing_list {
        outln!(w, "  {p}: {why}");
    }
    outln!(w, "model textures found nowhere: {}", h.unresolved);
    for (m, t) in &h.unresolved_list {
        outln!(w, "  {m}: {t}");
    }
    if h.lists_refused > 0 {
        outln!(w, "list files that did not read: {}", h.lists_refused);
    }
    outln!(w, "temporary files:            {}", h.temps);
    if let Some(u) = &h.unfinished {
        outln!(w, "unfinished change:          {u} (shaiya loose recover)");
    }
    if h.running {
        outln!(w, "change running:             {} (nothing is to be recovered)", loose_edit::RUNNING_TEXT);
    }
    for d in &h.damaged_journals {
        outln!(w, "damaged journal folder:     {d} (cannot be read; it blocks the rollback of older changes)");
    }
    if let Some((all, possible)) = h.journals {
        outln!(w, "journals:                   {all}, {possible} can be rolled back");
    }
    for n in &h.notes {
        outln!(w, "note: {n}");
    }
    outln!(w, "{}", if h.is_clean() { "nothing needs attention" } else { "something needs attention" });
    Ok(if h.is_clean() { 0 } else { 1 })
}
