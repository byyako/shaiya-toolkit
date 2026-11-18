//! `shaiya clients ...`: the saved "my client folders" list the toolkit programs share.

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use std::io::Write;
use shaiya_archive::client::{read_version_ini, ClientFiles};
use shaiya_archive::journal;
use shaiya_archive::Index;
use shaiya_core::config::{self, ClientFolder};
use shaiya_core::time::utc_iso;
use shaiya_core::{Error, Result};
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
}

#[derive(clap::Args)]
pub struct AddArgs {
    /// 1 to 40 characters, no / \ or @
    pub name: String,
    /// The client folder (it must hold data.sah)
    pub path: PathBuf,
    #[arg(long, default_value = "")]
    pub note: String,
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
                    outln!(w, "{:<width$}  {}  {}  {}", c.name, if c.path.is_dir() { "found  " } else { "missing" }, c.path.display(), c.note);
                }
            }
            Ok(0)
        }
        Cmd::Add(a) => {
            config::validate_client_name(&a.name)?;
            let files = ClientFiles::locate(&a.path)?;
            let path = std::path::absolute(&files.dir).unwrap_or(files.dir.clone());
            let root = root(ctx)?;
            let entry = ClientFolder { name: a.name.clone(), path, added_utc: utc_iso(std::time::SystemTime::now()), note: a.note.clone() };
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
        Cmd::Show(a) => {
            let name = strip(&a.name);
            let clients = crate::config::load_clients(ctx.data_dir.as_deref())?;
            let c = config::find_client(&clients, name)
                .ok_or_else(|| Error::io(PathBuf::from(format!("@{name}")), std::io::Error::new(std::io::ErrorKind::NotFound, "no saved client folder with this name (hint: shaiya clients list)")))?;
            let files = ClientFiles::locate(&c.path)?;
            let idx = Index::read(&files.sah)?;
            let ini = read_version_ini(&files);
            let journals = journal::list(&c.path, &c.path.join(journal::DEFAULT_DIR)).map(|l| l.len()).unwrap_or(0);
            if ctx.json {
                print_json(&json!({
                    "name": c.name, "path": c.path.display().to_string(), "note": c.note, "added_utc": c.added_utc, "sah_version": idx.version, "files": idx.file_count(),
                    "check_version": ini.and_then(|i| i.check_version), "current_version": ini.and_then(|i| i.current_version),
                    "game_exe": files.game_exe.is_some(), "journals": journals
                }));
            } else {
                let fmt = |v: Option<u32>| v.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string());
                println!("{}  {}", c.name, c.path.display());
                println!("  archive:   SAH version {}, {}", idx.version, count(idx.file_count(), "file"));
                println!("  Version.ini: CheckVersion {}, CurrentVersion {}", fmt(ini.and_then(|i| i.check_version)), fmt(ini.and_then(|i| i.current_version)));
                println!("  game.exe:  {}", if files.game_exe.is_some() { "present" } else { "not found" });
                println!("  changes made by the toolkit: {journals}");
            }
            Ok(0)
        }
    }
}
