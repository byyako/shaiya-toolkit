//! `shaiya patch ...`: build, list, verify, apply, rollback, history, keygen, sign, sign-updater.

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use shaiya_archive::client::ClientFiles;
use shaiya_archive::journal::{self, JournalMode, Rollback};
use shaiya_archive::Index;
use shaiya_core::fs::Existing;
use shaiya_core::glob::Filter;
use shaiya_core::time::parse_utc;
use shaiya_core::{Error, Job, Result};
use shaiya_patch::sign;
use shaiya_patch::{apply_with, build_with, contents, patch_number, plan_seq, verify, ApplyOptions, BuildInput, BuildOptions, Contents, SigStatus, VerifyKey};
use std::io::Write;
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Build a patch file from folders, from the difference of two archives, or from a list of removals
    ///
    /// --files DIR mirrors archive paths (DIR/Item/Item.SData becomes Item/Item.SData), --loose DIR holds files for the client folder
    /// itself (game.exe), --remove LIST names archive paths to delete. --diff OLD NEW builds the patch that turns OLD into NEW.
    /// With --files, --only-changed CLIENT leaves out the files that are already identical in that client (listed on stderr).
    /// The same inputs always give the same bytes.
    ///
    /// Example: shaiya patch build -o ps0109.patch --files changed/ --sign secret.key --check-version 3
    Build(BuildArgs),
    /// Show what is inside a patch
    ///
    /// Example: shaiya patch list ps0109.patch
    List(ListArgs),
    /// Check patches: structure, signature, and (with --client) whether they are applied
    ///
    /// Exit 0 when every check passes, 1 when one fails.
    ///
    /// Example: shaiya patch verify ps0109.patch --key public.key --check-version 3
    Verify(VerifyArgs),
    /// Apply patches to a client folder (with a backup you can roll back)
    ///
    /// Patches are applied in the order given, each with its own journal. --dry-run shows what each patch would do.
    ///
    /// Example: shaiya patch apply "@us" ps0109.patch --key public.key --check-version 3
    Apply(ApplyArgs),
    /// Publish patches for the regular updater: copy them into a server folder and raise PatchFileVersion in UpdateVersion.ini
    ///
    /// The folder is DIR/Shaiya/UpdateVersion.ini with the patches in DIR/Shaiya/patch/ (--layout shaiya), or all in DIR (--layout flat).
    /// Nothing is published when a name is not psNNNN.patch, a number is missing, a different file with the same name is already
    /// there, CheckVersion differs from the existing ini's, or DIR (or a folder it would write into) is a client folder. --dry-run shows the plan and writes nothing.
    /// Exit 0 done (or nothing to do), 3 bad input, 4 refused.
    ///
    /// Example: shaiya patch publish ps0110.patch --to server-files --check-version 3
    Publish(PublishArgs),
    /// Undo the newest change made by the toolkit to a client folder
    ///
    /// Example: shaiya patch rollback "@us"
    Rollback(RollbackArgs),
    /// List the changes the toolkit made to a client folder and whether each can be rolled back
    ///
    /// Example: shaiya patch history "@us"
    History(HistoryArgs),
    /// Make a signing key pair (Ed25519)
    ///
    /// Example: shaiya patch keygen --secret secret.key --public public.key
    Keygen(KeygenArgs),
    /// Sign patch files (writes FILE.sig)
    ///
    /// Example: shaiya patch sign ps0109.patch --key secret.key --check-version 3
    Sign(SignArgs),
    /// Sign an updater executable under its updater version
    ///
    /// Example: shaiya patch sign-updater new_updater.exe --key secret.key --updater-version 2 --check-version 3
    SignUpdater(SignUpdaterArgs),
}

#[derive(clap::Args)]
pub struct BuildArgs {
    /// The patch file to write (the updater only fetches psNNNN.patch)
    #[arg(short, long, value_name = "OUT.patch")]
    pub out: PathBuf,
    /// A folder mirroring archive paths
    #[arg(long, value_name = "DIR", conflicts_with = "diff")]
    pub files: Option<PathBuf>,
    /// A folder of files for the client folder itself
    #[arg(long, value_name = "DIR")]
    pub loose: Option<PathBuf>,
    /// A list of archive paths to remove (one per line)
    #[arg(long, value_name = "LIST", conflicts_with = "diff")]
    pub remove: Option<PathBuf>,
    /// Build from the difference of two archives (old, new)
    #[arg(long, num_args = 2, value_names = ["OLD", "NEW"])]
    pub diff: Vec<String>,
    /// Leave out the --files files that are already the same (length and SHA-256) in this client's archive; the client is only read
    #[arg(long, value_name = "CLIENT", requires = "files", conflicts_with = "diff")]
    pub only_changed: Option<String>,
    /// Entry time for every file: Unix seconds, 2026-11-15 or 2026-11-15T12:00:00Z (default: file times)
    #[arg(long, value_name = "T")]
    pub timestamp: Option<String>,
    /// Sign the patch with this secret key file
    #[arg(long, value_name = "KEY", requires = "check_version")]
    pub sign: Option<PathBuf>,
    /// The client generation the signature is for (Version.ini CheckVersion)
    #[arg(long, value_name = "N")]
    pub check_version: Option<u32>,
    /// Replace the patch (and its signature) if it exists
    #[arg(long)]
    pub force: bool,
    /// Show what would be built and write nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct ListArgs {
    pub patch: PathBuf,
}

#[derive(clap::Args)]
pub struct VerifyArgs {
    #[arg(required = true)]
    pub patches: Vec<PathBuf>,
    /// A public key file
    #[arg(long, value_name = "PUBLIC.key", conflicts_with = "public_hex")]
    pub key: Option<PathBuf>,
    /// The public key as hex
    #[arg(long, value_name = "HEX")]
    pub public_hex: Option<String>,
    /// The client generation the signatures are for
    #[arg(long, value_name = "N")]
    pub check_version: Option<u32>,
    /// Also check whether the patches are applied to this client folder
    #[arg(long, value_name = "CLIENT")]
    pub client: Option<String>,
    /// A missing .sig counts as a failure
    #[arg(long)]
    pub require_signature: bool,
}

#[derive(clap::Args)]
pub struct ApplyArgs {
    /// The client folder (or @name)
    pub client: String,
    #[arg(required = true)]
    pub patches: Vec<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
    /// Require a valid signature from this public key file on every patch
    #[arg(long, value_name = "PUBLIC.key", conflicts_with = "public_hex")]
    pub key: Option<PathBuf>,
    #[arg(long, value_name = "HEX")]
    pub public_hex: Option<String>,
    #[arg(long, value_name = "N")]
    pub check_version: Option<u32>,
    /// Loose file names that must not be overwritten (written as NAME.new)
    #[arg(long, value_name = "NAME")]
    pub protect: Vec<String>,
    /// Make no journal (the change cannot be rolled back)
    #[arg(long)]
    pub no_journal: bool,
    #[arg(long, value_name = "DIR")]
    pub journal_dir: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct PublishArgs {
    /// The patches to publish (psNNNN.patch; a .sig beside a patch goes with it)
    #[arg(required = true)]
    pub patches: Vec<PathBuf>,
    /// The server folder to publish into (never a client folder)
    #[arg(long, value_name = "DIR")]
    pub to: PathBuf,
    /// The client generation (CheckVersion in your players' Version.ini); required for a new UpdateVersion.ini
    #[arg(long, value_name = "N")]
    pub check_version: Option<u32>,
    /// UpdaterVersion for a new UpdateVersion.ini (default 1), or the new value for an existing one
    #[arg(long, value_name = "N")]
    pub updater_version: Option<u32>,
    /// Allow a CheckVersion that differs from the existing ini's (every player is then told to reinstall the game)
    #[arg(long)]
    pub new_check_version: bool,
    /// shaiya: DIR/Shaiya/UpdateVersion.ini and DIR/Shaiya/patch/psNNNN.patch; flat: both in DIR
    #[arg(long, value_name = "LAYOUT", default_value = "shaiya", value_parser = ["shaiya", "flat"])]
    pub layout: String,
    /// Show the plan and write nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct RollbackArgs {
    pub client: String,
    /// The change to undo (default: the newest)
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,
    /// Restore loose files even if they changed since (each changed file is saved first)
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub dry_run: bool,
    #[arg(long, value_name = "DIR")]
    pub journal_dir: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct HistoryArgs {
    pub client: String,
    /// Delete all but the newest KEEP journals
    #[arg(long, value_name = "KEEP")]
    pub prune: Option<usize>,
    #[arg(long, value_name = "DIR")]
    pub journal_dir: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct KeygenArgs {
    #[arg(long, value_name = "FILE")]
    pub secret: PathBuf,
    #[arg(long, value_name = "FILE")]
    pub public: PathBuf,
}

#[derive(clap::Args)]
pub struct SignArgs {
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    #[arg(long, value_name = "SECRET.key")]
    pub key: PathBuf,
    #[arg(long, value_name = "N")]
    pub check_version: u32,
    /// Replace an existing .sig
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct SignUpdaterArgs {
    pub exe: PathBuf,
    #[arg(long, value_name = "SECRET.key")]
    pub key: PathBuf,
    #[arg(long, value_name = "V")]
    pub updater_version: u32,
    #[arg(long, value_name = "N")]
    pub check_version: u32,
    #[arg(long)]
    pub force: bool,
}

pub fn run(ctx: &Ctx, cmd: &Cmd, job: &Job) -> i32 {
    let r = match cmd {
        Cmd::Build(a) => build(ctx, a, job),
        Cmd::List(a) => list(ctx, a),
        Cmd::Verify(a) => verify_cmd(ctx, a, job),
        Cmd::Apply(a) => apply(ctx, a, job),
        Cmd::Publish(a) => publish_cmd(ctx, a, job),
        Cmd::Rollback(a) => rollback(ctx, a, job),
        Cmd::History(a) => history(ctx, a),
        Cmd::Keygen(a) => keygen(ctx, a),
        Cmd::Sign(a) => sign_cmd(ctx, a, job),
        Cmd::SignUpdater(a) => sign_updater(ctx, a, job),
    };
    match r {
        Ok(code) => code,
        Err(e) => fail(ctx, &e),
    }
}

fn key_of(path: &Option<PathBuf>, hex: &Option<String>, check_version: Option<u32>) -> Result<Option<VerifyKey>> {
    let public = match (path, hex) {
        (Some(p), _) => sign::read_key(p)?,
        (None, Some(h)) => {
            let _: [u8; 32] = sign::from_hex(h)?;
            h.trim().to_ascii_lowercase()
        }
        (None, None) => return Ok(None),
    };
    let cv = check_version.ok_or_else(|| Error::Refused("--check-version is required with a key (a wrong default would make signatures other servers reject)".to_string()))?;
    Ok(Some(VerifyKey { public_hex: public, check_version: cv }))
}

fn contents_table(w: &mut dyn Write, c: &Contents) {
    let mut files: Vec<_> = c.files.iter().collect();
    files.sort_by_key(|f| f.path.to_lowercase());
    for f in files {
        outln!(w, "  archive {:>11}  {:<20}  {}", num(f.length as u64), iso_version(f.version).unwrap_or_default(), f.path);
    }
    let mut loose = c.loose.clone();
    loose.sort();
    for (n, s) in &loose {
        outln!(w, "  loose   {:>11}  {:<20}  {n}", num(*s), "");
    }
    for r in &c.removals {
        outln!(w, "  remove  {:>11}  {:<20}  {r}", "", "");
    }
    let bytes: u64 = c.files.iter().map(|f| f.length as u64).sum();
    outln!(w, "{} ({}), {}, {}", count(c.files.len(), "archive file"), size(bytes), count(c.loose.len(), "loose file"), count(c.removals.len(), "removal"));
}

fn list(ctx: &Ctx, a: &ListArgs) -> Result<i32> {
    let c = contents(&a.patch)?;
    if ctx.json {
        print_json(&serde_json::to_value(&c).unwrap_or(Value::Null));
        return Ok(0);
    }
    let mut w = out();
    outln!(w, "{}  ZIP {} ({} unpacked), update.sah version {}, signature file {}", file_name(&a.patch), size(c.zip_size), size(c.unpacked_size), c.sah_version, if c.signature_file { "present" } else { "not present" });
    contents_table(&mut w, &c);
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- build

fn build(ctx: &Ctx, a: &BuildArgs, job: &Job) -> Result<i32> {
    let ts = a.timestamp.as_deref().map(parse_utc).transpose()?.map(|t| t.clamp(0, i32::MAX as i64) as i32);
    let input = if a.diff.len() == 2 {
        let (old, new) = (open_full(ctx, &a.diff[0])?, open_full(ctx, &a.diff[1])?);
        let mut i = BuildInput::from_diff(&old, &new, &Filter::default(), job)?;
        if let Some(l) = &a.loose {
            i.loose = BuildInput::from_folders(None, Some(l), None, ts)?.loose;
        }
        i
    } else {
        if a.files.is_none() && a.loose.is_none() && a.remove.is_none() {
            return Err(Error::Refused("nothing to build: give --files DIR, --loose DIR, --remove LIST or --diff OLD NEW".to_string()));
        }
        BuildInput::from_folders(a.files.as_deref(), a.loose.as_deref(), a.remove.as_deref(), ts)?
    };
    let mut input = input;
    let mut unchanged: Vec<String> = Vec::new();
    if let Some(c) = &a.only_changed {
        unchanged = input.skip_same_as(&archive_client_dir(ctx, c)?, job)?;
        if !unchanged.is_empty() {
            note(ctx, &format!("left out, already the same in the client ({}):", count(unchanged.len(), "file")));
            for p in &unchanged {
                note(ctx, &format!("  {p}"));
            }
        }
    }
    let name = file_name(&a.out);
    if patch_number(&name).is_none() {
        warn(ctx, &format!("{name} is not named psNNNN.patch; the updater only fetches psNNNN.patch"));
    }
    if !input.removals.is_empty() {
        warn(ctx, "remove.txt is honoured by the toolkit and the replacement updater, not by the original Updater.exe");
    }
    if input.files.is_empty() && input.loose.is_empty() && input.removals.is_empty() {
        warn(ctx, "the patch is empty (no files, no loose files, no removals)");
    }
    let problems = shaiya_patch::input_problems(&input);
    if !problems.is_empty() {
        return Err(Error::Refused(format!("{}:\n  {}", count(problems.len(), "problem"), problems.join("\n  "))));
    }
    let findings = shaiya_inspect::validate::check_input(&input);
    warn_findings(ctx, &findings);
    if a.dry_run {
        let total: u64 = input.files.iter().map(|f| f.1.len().unwrap_or(0)).sum();
        let loose: u64 = input.loose.iter().map(|(_, p)| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)).sum();
        if ctx.json {
            print_json(&json!({"dry_run": true, "out": a.out.display().to_string(), "files": input.files.iter().map(|f| f.0.clone()).collect::<Vec<_>>(),
                "loose": input.loose.iter().map(|f| f.0.clone()).collect::<Vec<_>>(), "removals": input.removals, "archive_bytes": total, "loose_bytes": loose,
                "unchanged": unchanged, "warnings": findings_json(&findings)}));
        } else {
            let c = Contents {
                sah_version: 0,
                files: input.files.iter().map(|(p, s, v)| shaiya_patch::PatchFile { path: p.clone(), length: s.len().unwrap_or(0) as u32, version: *v }).collect(),
                loose: input.loose.iter().map(|(n, p)| (n.clone(), std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))).collect(),
                removals: input.removals.clone(),
                zip_size: 0,
                unpacked_size: total + loose,
                signature_file: false,
            };
            let mut w = out();
            outln!(w, "would build {} ({} before compression)", a.out.display(), size(total + loose));
            contents_table(&mut w, &c);
        }
        return Ok(0);
    }
    let sign = match &a.sign {
        Some(k) => Some((sign::read_key(k)?, a.check_version.expect("clap: --sign requires --check-version"))),
        None => None,
    };
    let signing = sign.is_some();
    let r = build_with(&input, &a.out, &BuildOptions { existing: if a.force { Existing::Overwrite } else { Existing::Refuse }, sign }, job)?;
    let sig = signing.then(|| sign::signature_path(&a.out).display().to_string());
    if ctx.json {
        let mut v = serde_json::to_value(&r).unwrap_or(Value::Null);
        v["out"] = json!(a.out.display().to_string());
        v["signature"] = json!(sig);
        v["unchanged"] = json!(unchanged);
        v["warnings"] = findings_json(&findings);
        print_json(&v);
    } else {
        let meta = std::fs::metadata(&a.out).map(|m| m.len()).unwrap_or(0);
        println!("built {} ({}): {} ({}), {}, {}", a.out.display(), size(meta), count(r.added.len(), "archive file"), size(r.bytes), count(r.loose.len(), "loose file"), count(r.removed.len(), "removal"));
        if let Some(s) = sig {
            println!("signed: {s}");
        }
    }
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- verify

fn verify_cmd(ctx: &Ctx, a: &VerifyArgs, job: &Job) -> Result<i32> {
    let key = key_of(&a.key, &a.public_hex, a.check_version)?;
    if a.require_signature && key.is_none() {
        return Err(Error::Refused("--require-signature needs a key (--key FILE or --public-hex HEX with --check-version)".to_string()));
    }
    let client = a.client.as_deref().map(|c| archive_client_dir(ctx, c)).transpose()?;
    let mut reports = Vec::new();
    let mut failed = false;
    for p in &a.patches {
        let r = verify(p, key.as_ref(), client.as_deref(), job)?;
        let bad_sig = matches!(r.signature, SigStatus::Invalid(_)) || (a.require_signature && r.signature == SigStatus::Missing);
        failed |= !r.problems.is_empty() || bad_sig || r.not_applied.as_ref().is_some_and(|n| !n.is_empty());
        reports.push(r);
    }
    if ctx.json {
        print_json(&json!({"patches": reports.iter().map(|r| serde_json::to_value(r).unwrap_or(Value::Null)).collect::<Vec<_>>()}));
        return Ok(i32::from(failed));
    }
    let mut w = out();
    for r in &reports {
        let name = file_name(&r.patch);
        let structure = if r.problems.is_empty() {
            match contents(&r.patch) {
                Ok(c) => format!("structure OK ({}, {}, {})", count(c.files.len(), "archive file"), count(c.loose.len(), "loose file"), count(c.removals.len(), "removal")),
                Err(_) => "structure OK".to_string(),
            }
        } else {
            "structure PROBLEMS".to_string()
        };
        let sig = match &r.signature {
            SigStatus::NotChecked => "signature not checked".to_string(),
            SigStatus::Missing => format!("signature MISSING ({name}.sig not found)"),
            SigStatus::Valid => "signature valid".to_string(),
            SigStatus::Invalid(why) => format!("signature INVALID ({why})"),
        };
        outln!(w, "{name}  {structure}; {sig}");
        for p in &r.problems {
            outln!(w, "  problem: {p}");
        }
        if let Some(na) = &r.not_applied {
            if na.is_empty() {
                outln!(w, "  applied to the client: yes");
            } else {
                outln!(w, "  not applied to the client:");
                for n in na {
                    outln!(w, "    {n}");
                }
            }
        }
    }
    Ok(i32::from(failed))
}

// ---------------------------------------------------------------------------------------------------------- apply

fn apply(ctx: &Ctx, a: &ApplyArgs, job: &Job) -> Result<i32> {
    let dir = archive_client_dir(ctx, &a.client)?;
    let key = key_of(&a.key, &a.public_hex, a.check_version)?;
    let numbers: Vec<Option<u32>> = a.patches.iter().map(|p| patch_number(&file_name(p))).collect();
    if numbers.windows(2).any(|w| matches!((w[0], w[1]), (Some(x), Some(y)) if y <= x)) {
        warn(ctx, "the patch numbers are not in ascending order; they are applied in the order given");
    }
    // signatures of every patch, before the first write
    if let Some(k) = &key {
        let mut bad = Vec::new();
        for p in &a.patches {
            if let Err(e) = sign::verify_file(&k.public_hex, k.check_version, p, None, job) {
                bad.push(format!("{}: {e}", p.display()));
            }
        }
        if !bad.is_empty() {
            return Err(Error::Refused(format!("signature check failed:\n  {}", bad.join("\n  "))));
        }
    }
    let files = ClientFiles::locate(&dir)?;
    let idx = Index::read(&files.sah)?;
    if a.dry_run {
        let plans = plan_seq(&a.patches, &dir, &a.protect)?;
        if ctx.json {
            print_json(&json!({"dry_run": true, "plans": plans.iter().map(|p| serde_json::to_value(p).unwrap_or(Value::Null)).collect::<Vec<_>>()}));
            return Ok(0);
        }
        let mut w = out();
        for p in &plans {
            outln!(w, "{} -> {} ({}, {})", file_name(&p.patch), dir.display(), file_name(&files.sah), count(idx.file_count(), "file"));
            let overwrites: Vec<&str> = p.loose.iter().filter(|l| l.overwrites).map(|l| l.target.as_str()).collect();
            outln!(
                w,
                "  would add {}, replace {}, remove {} ({} not present), write {}{}",
                p.added.len(),
                p.replaced.len(),
                p.removed.len(),
                p.missing.len(),
                count(p.loose.len(), "loose file"),
                if overwrites.is_empty() { String::new() } else { format!(" (overwrites {})", overwrites.join(", ")) }
            );
            outln!(w, "  would append {} to data.saf; needs about {} free on that drive while applying", size(p.bytes), size(p.bytes + p.temp_bytes));
            outln!(w, "  signature {}", if key.is_some() { "valid" } else { "not checked" });
        }
        return Ok(0);
    }
    let _ = idx;
    prepare_write(ctx, &dir, true)?;
    if a.no_journal {
        warn(ctx, "no journal: these changes cannot be rolled back with `shaiya patch rollback` (data.sah.bak keeps the old index)");
    }
    if key.is_none() {
        note(ctx, "signature not checked");
    }
    let root = journal_root(&dir, a.journal_dir.as_deref());
    let opts = ApplyOptions {
        protect: a.protect.clone(),
        journal: if a.no_journal { JournalMode::None } else { JournalMode::In(root) },
        legacy_bak: true,
        require_signature: key,
        check_in_use: false,
    };
    let mut done = Vec::new();
    for p in &a.patches {
        match apply_with(p, &dir, &opts, job) {
            Ok(r) => {
                if !ctx.json {
                    let mut w = out();
                    outln!(
                        w,
                        "applied {}: added {}, replaced {}, removed {} ({} not present), {} written ({} appended to data.saf)",
                        file_name(p),
                        r.added.len(),
                        r.replaced.len(),
                        r.removed.len(),
                        r.missing.len(),
                        count(r.loose.len(), "loose file"),
                        size(r.bytes)
                    );
                    if let Some(j) = &r.journal {
                        outln!(w, "backup: {}  (undo: {})", j.display(), undo_hint(&a.client, a.journal_dir.as_deref()));
                    }
                }
                done.push(r);
            }
            Err(e) => {
                if !done.is_empty() {
                    eprintln!("error: {} failed after {} applied; those stay applied (`{}` lists them)", file_name(p), count(done.len(), "patch"), history_hint(&a.client, a.journal_dir.as_deref()));
                }
                if ctx.json {
                    print_json(&json!({"client": dir.display().to_string(), "results": done.iter().map(|r| serde_json::to_value(r).unwrap_or(Value::Null)).collect::<Vec<_>>(),
                        "error": {"patch": p.display().to_string(), "message": e.to_string()}}));
                }
                return Err(e);
            }
        }
    }
    if ctx.json {
        print_json(&json!({"client": dir.display().to_string(), "results": done.iter().map(|r| serde_json::to_value(r).unwrap_or(Value::Null)).collect::<Vec<_>>()}));
    }
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- publish

fn publish_cmd(ctx: &Ctx, a: &PublishArgs, job: &Job) -> Result<i32> {
    use shaiya_patch::publish::{plan_publish, publish, FileState, Layout, PublishOptions};
    let client_dirs: Vec<PathBuf> = crate::config::load_clients(ctx.data_dir.as_deref())?.into_iter().map(|c| c.path).collect();
    let opts = PublishOptions {
        layout: Layout::parse(&a.layout).unwrap_or(Layout::Shaiya),
        check_version: a.check_version,
        updater_version: a.updater_version,
        new_check_version: a.new_check_version,
        client_dirs,
    };
    let plan = plan_publish(&a.patches, &a.to, &opts, job)?;
    let mut findings = Vec::new();
    for item in &plan.items {
        findings.extend(shaiya_inspect::validate::check_patch(&item.patch.source, job)?);
    }
    for w in &plan.warnings {
        warn(ctx, w);
    }
    warn_findings(ctx, &findings);
    let rel = |p: &std::path::Path| p.strip_prefix(&plan.dir).unwrap_or(p).display().to_string();
    let report = if a.dry_run { None } else { Some(publish(&plan, job)?) };
    if ctx.json {
        let mut v = json!({"dry_run": a.dry_run, "plan": serde_json::to_value(&plan).unwrap_or(Value::Null), "warnings": findings_json(&findings), "notes": plan.warnings,
            "nothing_to_do": plan.nothing_to_do()});
        if let Some(r) = &report {
            v["result"] = serde_json::to_value(r).unwrap_or(Value::Null);
        }
        print_json(&v);
        return Ok(0);
    }
    let mut w = out();
    outln!(w, "{} {} to {} (layout {})", if a.dry_run { "would publish" } else { "published" }, count(plan.items.len(), "patch"), plan.dir.display(), a.layout);
    for item in &plan.items {
        let state = match item.patch.state {
            FileState::New => "new",
            FileState::AlreadyPublished => "already published (same file)",
        };
        outln!(w, "  {}  {}  {}  -> {}", file_name(&item.patch.source), state, size(item.patch.size), rel(&item.patch.target));
        if let Some(s) = &item.signature {
            outln!(w, "    {}  {}  -> {}", file_name(&s.source), if s.state == FileState::New { "new" } else { "already published (same file)" }, rel(&s.target));
        }
    }
    let ini = rel(&plan.ini_path);
    if !plan.ini_changed {
        outln!(w, "  {ini}: unchanged (PatchFileVersion {}, CheckVersion {}, UpdaterVersion {})", plan.patch_file_version_after, plan.check_version, plan.updater_version);
    } else {
        let before = plan.patch_file_version_before.map_or("new file".to_string(), |v| v.to_string());
        outln!(w, "  {ini}: PatchFileVersion {before} -> {} (CheckVersion {}, UpdaterVersion {})", plan.patch_file_version_after, plan.check_version, plan.updater_version);
    }
    if report.is_some() {
        outln!(w, "every written file was read back and matches");
    }
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- rollback / history

fn rollback(ctx: &Ctx, a: &RollbackArgs, job: &Job) -> Result<i32> {
    let dir = archive_client_dir(ctx, &a.client)?;
    prepare_write(ctx, &dir, !a.dry_run)?;
    let root = journal_root(&dir, a.journal_dir.as_deref());
    let all = journal::list(&dir, &root)?;
    let target = match &a.id {
        Some(id) => all.iter().find(|(j, _)| &j.id == id || j.id.ends_with(id.as_str())).ok_or_else(|| Error::io(id, std::io::Error::new(std::io::ErrorKind::NotFound, format!("no such journal (hint: {})", history_hint(&a.client, a.journal_dir.as_deref())))))?,
        None => all
            .iter()
            .rev()
            .find(|(j, _)| j.state == journal::State::Applied)
            .ok_or_else(|| Error::Refused("nothing to roll back: the toolkit has made no changes to this client (or all were rolled back)".to_string()))?,
    };
    let (j, status) = target;
    if let Rollback::Blocked(why) = status {
        if !(a.force && why.contains("--force")) {
            return Err(Error::Refused(format!("cannot roll back {}: {why}", j.id)));
        }
    }
    if a.dry_run {
        if ctx.json {
            print_json(&json!({"dry_run": true, "journal": serde_json::to_value(j).unwrap_or(Value::Null)}));
        } else {
            println!(
                "would roll back {} (applied {}): restore the index, cut data.saf back to {}, restore {} and delete {}",
                j.label,
                j.created_utc,
                count(j.saf_len_before as usize, "byte"),
                j.loose.iter().filter(|l| l.existed).count(),
                count(j.loose.iter().filter(|l| !l.existed).count(), "loose file")
            );
        }
        return Ok(0);
    }
    let r = journal::rollback(&dir, &j.dir, a.force, job)?;
    if ctx.json {
        print_json(&serde_json::to_value(&r).unwrap_or(Value::Null));
    } else {
        println!(
            "rolled back {} (applied {}): index restored, data.saf truncated to {}, {} restored, {} deleted",
            j.label,
            j.created_utc,
            count(r.truncated_to as usize, "byte"),
            count(r.restored_loose.len(), "loose file"),
            r.deleted_loose.len()
        );
    }
    Ok(0)
}

fn history(ctx: &Ctx, a: &HistoryArgs) -> Result<i32> {
    let dir = archive_client_dir(ctx, &a.client)?;
    for n in shaiya_archive::client::recover(&dir)? {
        note(ctx, &format!("note: {n}"));
    }
    let root = journal_root(&dir, a.journal_dir.as_deref());
    let mut pruned = Vec::new();
    if let Some(keep) = a.prune {
        pruned = journal::prune(&dir, &root, keep)?;
    }
    let all = journal::list(&dir, &root)?;
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
        print_json(&json!({"journals": items, "pruned": pruned}));
        return Ok(0);
    }
    let mut w = out();
    if all.is_empty() {
        outln!(w, "No changes made by the toolkit to this client yet.");
    }
    for (j, r) in &all {
        let state = match &j.state {
            journal::State::Pending => "pending".to_string(),
            journal::State::Applied => "applied".to_string(),
            journal::State::RolledBack { at_utc } => format!("rolled back {at_utc}"),
        };
        let rb = match r {
            Rollback::Possible => "possible".to_string(),
            Rollback::Blocked(why) => format!("blocked: {why}"),
        };
        outln!(w, "{}  {:<14} {:<13} {}  {:<24} rollback {rb}", j.id, j.label, j.kind, j.created_utc, state);
    }
    for id in &pruned {
        outln!(w, "pruned {id}");
    }
    Ok(0)
}

// ---------------------------------------------------------------------------------------------------------- keys

fn keygen(ctx: &Ctx, a: &KeygenArgs) -> Result<i32> {
    let public = sign::write_key_pair(&a.secret, &a.public)?;
    if ctx.json {
        print_json(&json!({"public_hex": public, "secret_file": a.secret.display().to_string(), "public_file": a.public.display().to_string()}));
        return Ok(0);
    }
    println!("public key: {public}");
    println!("secret key written to {}; public key written to {}", a.secret.display(), a.public.display());
    note(ctx, "Give the public key above (or public key file) to whoever checks your patches; keep the secret key off the patch server and backed up.");
    if cfg!(windows) {
        note(ctx, "Windows does not restrict who can read this file here: store it in a folder only you can open.");
    }
    note(ctx, "Never sign a different file under a patch number you have already published; ship fixes as the next number.");
    Ok(0)
}

fn sign_cmd(ctx: &Ctx, a: &SignArgs, job: &Job) -> Result<i32> {
    let secret = sign::read_key(&a.key)?;
    let existing = if a.force { Existing::Overwrite } else { Existing::Refuse };
    let mut signed = Vec::new();
    for f in &a.files {
        let sig = sign::sign_file(&secret, a.check_version, f, None, existing, job)?;
        signed.push(json!({"file": f.display().to_string(), "sig": sig.display().to_string(), "name": file_name(f).to_ascii_lowercase(), "check_version": a.check_version}));
        if !ctx.json {
            println!("signed {} -> {}", f.display(), sig.display());
        }
    }
    if ctx.json {
        print_json(&json!({"signed": signed}));
    }
    Ok(0)
}

fn sign_updater(ctx: &Ctx, a: &SignUpdaterArgs, job: &Job) -> Result<i32> {
    let secret = sign::read_key(&a.key)?;
    let name = sign::updater_name(a.updater_version);
    let sig = sign::sign_file(&secret, a.check_version, &a.exe, Some(&name), if a.force { Existing::Overwrite } else { Existing::Refuse }, job)?;
    if ctx.json {
        print_json(&json!({"signed": [{"file": a.exe.display().to_string(), "sig": sig.display().to_string(), "name": name, "check_version": a.check_version}]}));
    } else {
        println!("signed {} as {name} -> {}", a.exe.display(), sig.display());
    }
    Ok(0)
}

