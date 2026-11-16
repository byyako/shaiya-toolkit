//! `shaiya sdata ...`: info, check, decrypt, encrypt over files, folders or an archive.

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};
use std::io::Write;
use shaiya_archive::{Archive, ArchiveSource};
use shaiya_core::fs::{same_file, Existing};
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::source::{ByteSource, FolderSource};
use shaiya_core::{Error, Job, Result};
use shaiya_seed::batch::{run as batch_run, BatchOptions, ItemResult, Op, Output, Status};
use std::path::PathBuf;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Describe SData files: encrypted or plain, plaintext size, checksum
    ///
    /// Example: shaiya sdata info Item.SData GuildHouse.SData
    Info(InfoArgs),
    /// Check the checksums of SData files (exit 1 when one fails)
    ///
    /// Example: shaiya sdata check --from @us
    Check(InfoArgs),
    /// Decrypt SData files to a folder (or in place with a backup)
    ///
    /// Plain files (some clients do not encrypt) are copied unchanged unless --skip-plain. Names are kept: decrypt to plain/, edit,
    /// then `encrypt plain/ -o enc/ --like @us` gives the original names back.
    ///
    /// Example: shaiya sdata decrypt --from @us -o plain/
    Decrypt(DecryptArgs),
    /// Encrypt SData files to a folder (or in place with a backup)
    ///
    /// --like takes a file, a folder of originals or an archive: each output reuses its original's header padding and tail, so an
    /// unchanged file comes out byte-identical, and a plain original stays plain.
    ///
    /// Example: shaiya sdata encrypt plain/ -o enc/ --like @us
    Encrypt(EncryptArgs),
}

#[derive(clap::Args)]
pub struct InfoArgs {
    /// Files or folders (folders are searched for *.SData); with --from, patterns over the archive's files (default *.sdata)
    #[arg(value_name = "INPUT", required_unless_present = "from")]
    pub inputs: Vec<String>,
    /// Read from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// Take every file, not only those named *.SData
    #[arg(long)]
    pub all_files: bool,
}

#[derive(clap::Args)]
pub struct DecryptArgs {
    #[arg(value_name = "INPUT", required_unless_present = "from")]
    pub inputs: Vec<String>,
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// A file (one input) or a folder to write into
    #[arg(short, long, value_name = "PATH", conflicts_with = "in_place")]
    pub out: Option<PathBuf>,
    /// Replace the files with their decrypted form (a backup is made first)
    #[arg(long)]
    pub in_place: bool,
    /// Write files whose checksum fails too (exit 1)
    #[arg(long)]
    pub unchecked: bool,
    /// Leave plain files out instead of copying them
    #[arg(long)]
    pub skip_plain: bool,
    #[arg(long, conflicts_with = "skip_existing")]
    pub force: bool,
    #[arg(long)]
    pub skip_existing: bool,
    #[arg(long)]
    pub all_files: bool,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct EncryptArgs {
    #[arg(value_name = "INPUT")]
    pub inputs: Vec<String>,
    #[arg(short, long, value_name = "PATH", conflicts_with = "in_place")]
    pub out: Option<PathBuf>,
    #[arg(long)]
    pub in_place: bool,
    /// Reuse the wrapper of the originals: a file, a folder of originals, or an archive
    #[arg(long, value_name = "PATH|ARCHIVE")]
    pub like: Option<String>,
    /// Leave encrypted files out instead of copying them
    #[arg(long)]
    pub skip_encrypted: bool,
    #[arg(long, conflicts_with = "skip_existing")]
    pub force: bool,
    #[arg(long)]
    pub skip_existing: bool,
    #[arg(long)]
    pub all_files: bool,
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(ctx: &Ctx, cmd: &Cmd, job: &Job) -> i32 {
    let r = match cmd {
        Cmd::Info(a) => info(ctx, a, false, job),
        Cmd::Check(a) => info(ctx, a, true, job),
        Cmd::Decrypt(a) => decrypt(ctx, a, job),
        Cmd::Encrypt(a) => encrypt(ctx, a, job),
    };
    match r {
        Ok(code) => code,
        Err(e) => fail(ctx, &e),
    }
}

/// Two sources read as one (explicit files are never filtered by name; folders are).
struct Chain(Vec<FolderSource>);

impl ByteSource for Chain {
    fn label(&self) -> String {
        self.0.iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")
    }
    fn names(&self) -> Vec<String> {
        self.0.iter().flat_map(|s| s.names()).collect()
    }
    fn size(&self, name: &str) -> Option<u64> {
        self.0.iter().find_map(|s| s.size(name))
    }
    fn read(&self, name: &str) -> Result<Vec<u8>> {
        match self.0.iter().find(|s| s.size(name).is_some()) {
            Some(s) => s.read(name),
            None => Err(Error::io(name, std::io::Error::new(std::io::ErrorKind::NotFound, "not in these inputs"))),
        }
    }
    fn file_path(&self, name: &str) -> Option<PathBuf> {
        self.0.iter().find_map(|s| s.file_path(name))
    }
}

fn sdata_filter(all_files: bool) -> Result<Filter> {
    Ok(if all_files { Filter::default() } else { Filter { include: vec![Pattern::new("*.sdata")?], ..Filter::default() } })
}

fn folder_chain(ctx: &Ctx, inputs: &[String], all_files: bool) -> Result<(Chain, Vec<PathBuf>)> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for i in inputs {
        let p = resolve(ctx, i)?;
        if p.is_dir() {
            dirs.push(p);
        } else {
            files.push(p);
        }
    }
    let mut chain = Vec::new();
    if !files.is_empty() {
        chain.push(FolderSource::new(&files, &Filter::default())?);
    }
    if !dirs.is_empty() {
        chain.push(FolderSource::new(&dirs, &sdata_filter(all_files)?)?);
    }
    for s in chain.iter().flat_map(|c| c.skipped()) {
        warn(ctx, &format!("skipped {s}"));
    }
    Ok((Chain(chain), dirs))
}

/// What `--like` names: an archive, or files on disk.
enum Like {
    Archive(Archive),
    Folder(FolderSource),
}

fn open_like(ctx: &Ctx, s: &str) -> Result<Like> {
    let p = resolve(ctx, s)?;
    let is_sah = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sah"));
    if (p.is_dir() && shaiya_archive::client::ClientFiles::locate(&p).is_ok()) || (p.is_file() && is_sah) {
        return Ok(Like::Archive(Archive::open(&p)?));
    }
    Ok(Like::Folder(FolderSource::new(&[p], &Filter::default())?))
}

fn status_text(s: Status) -> &'static str {
    match s {
        Status::Encrypted => "encrypted",
        Status::Plain => "plain",
        Status::CrcOk => "crc_ok",
        Status::CrcBad => "crc_bad",
        Status::Decrypted => "decrypted",
        Status::DecryptedCrcBad => "decrypted_crc_bad",
        Status::EncryptedNew => "encrypted_new",
        Status::PassedThrough => "passed_through",
        Status::SkippedUnchanged => "skipped_unchanged",
        Status::SkippedExisting => "skipped_existing",
        Status::Failed => "failed",
    }
}

fn summary(items: &[ItemResult]) -> Value {
    let mut m = serde_json::Map::new();
    for i in items {
        let e = m.entry(status_text(i.status).to_string()).or_insert(json!(0));
        *e = json!(e.as_u64().unwrap_or(0) + 1);
    }
    Value::Object(m)
}

/// Runs `f` over the source the arguments describe.
fn with_source(ctx: &Ctx, inputs: &[String], from: Option<&str>, all_files: bool, f: &mut dyn FnMut(&dyn ByteSource, &[PathBuf]) -> Result<i32>) -> Result<i32> {
    match from {
        Some(a) => {
            let archive = open_full(ctx, a)?;
            let filter = if inputs.is_empty() { sdata_filter(all_files)? } else { Filter::from_args(inputs, &[], None, None)? };
            let src = ArchiveSource { archive: &archive, filter, include_shadowed: false };
            f(&src, &[])
        }
        None => {
            let (chain, dirs) = folder_chain(ctx, inputs, all_files)?;
            f(&chain, &dirs)
        }
    }
}

fn info(ctx: &Ctx, a: &InfoArgs, check: bool, job: &Job) -> Result<i32> {
    with_source(ctx, &a.inputs, a.from.as_deref(), a.all_files, &mut |src, _| {
        let items = batch_run(src, &BatchOptions { op: if check { Op::Check } else { Op::Info }, output: Output::None, existing: Existing::Refuse, pass_through: false, dry_run: false }, job)?;
        let bad = items.iter().any(|i| matches!(i.status, Status::CrcBad) || i.crc_ok == Some(false));
        let failed = items.iter().any(|i| i.status == Status::Failed);
        if ctx.json {
            print_json(&json!({"items": items.iter().map(|i| serde_json::to_value(i).unwrap_or(Value::Null)).collect::<Vec<_>>(), "summary": summary(&items)}));
        } else {
            let width = items.iter().map(|i| i.name.chars().count()).max().unwrap_or(0);
            let mut w = out();
            for i in &items {
                let pad = " ".repeat(width - i.name.chars().count());
                match i.status {
                    Status::Failed => outln!(w, "{}{pad}  FAILED: {}", i.name, i.message.clone().unwrap_or_default()),
                    Status::Plain => outln!(w, "{}{pad}  not encrypted, {} bytes", i.name, num(i.size)),
                    _ => outln!(
                        w,
                        "{}{pad}  SEED, plaintext {} bytes, CRC 0x{:08x} {}, header padding {}, tail {}",
                        i.name,
                        num(i.plain_size.unwrap_or(0)),
                        i.crc.unwrap_or(0),
                        if i.crc_ok == Some(true) { "OK" } else { "FAILED" },
                        if i.padding_zero == Some(true) { "zero" } else { "not zero" },
                        if i.tail_zero == Some(true) { "zero" } else { "not zero" }
                    ),
                }
            }
        }
        Ok(if failed { 3 } else if bad && check { 1 } else { 0 })
    })
}

fn output_for(out: &Option<PathBuf>, in_place: bool, names: usize, input_dirs: &[PathBuf], from: bool) -> Result<Output> {
    if in_place {
        if from {
            return Err(Error::Refused("--in-place needs files on disk; with --from use -o".to_string()));
        }
        return Ok(Output::InPlace);
    }
    let Some(o) = out else {
        return Err(Error::Refused("this would write files: give -o PATH, or --in-place to change the inputs (a backup is made)".to_string()));
    };
    if input_dirs.iter().any(|d| same_file(d, o)) {
        return Err(Error::Refused(format!("{} is also an input folder; write to another folder or use --in-place", o.display())));
    }
    let s = o.to_string_lossy();
    let as_file = names == 1 && !o.is_dir() && !s.ends_with('/') && !s.ends_with('\\');
    Ok(if as_file { Output::File(o.clone()) } else { Output::Dir(o.clone()) })
}

fn print_batch(ctx: &Ctx, items: &[ItemResult], dry: bool, plain_word: &str) -> i32 {
    let failed = items.iter().filter(|i| i.status == Status::Failed).count();
    let crc_bad = items.iter().filter(|i| i.status == Status::DecryptedCrcBad).count();
    if ctx.json {
        let mut v = json!({"items": items.iter().map(|i| serde_json::to_value(i).unwrap_or(Value::Null)).collect::<Vec<_>>(), "summary": summary(items)});
        if dry {
            v["dry_run"] = json!(true);
        }
        print_json(&v);
    } else {
        let mut w = out();
        let would = if dry { "would " } else { "" };
        for i in items {
            let to = i.out.as_ref().map(|p| format!(" -> {}", p.display())).unwrap_or_default();
            match i.status {
                Status::Decrypted => outln!(w, "{}  {would}decrypt{} {} bytes{to}", i.name, if dry { "" } else { "ed" }, num(i.plain_size.unwrap_or(0))),
                Status::DecryptedCrcBad => outln!(w, "{}  {would}decrypt (checksum mismatch, written because of --unchecked){to}", i.name),
                Status::EncryptedNew => outln!(w, "{}  {would}encrypt{} {} bytes{to}{}", i.name, if dry { "" } else { "ed" }, num(i.plain_size.unwrap_or(0)), i.message.as_ref().map(|m| format!("  ({m})")).unwrap_or_default()),
                Status::PassedThrough => outln!(w, "{}  {plain_word}, {}copied{to}{}", i.name, if dry { "would be " } else { "" }, i.message.as_ref().map(|m| format!("  ({m})")).unwrap_or_default()),
                Status::SkippedUnchanged => outln!(w, "{}  {plain_word}, skipped", i.name),
                Status::SkippedExisting => outln!(w, "{}  exists, skipped{to}", i.name),
                Status::Failed => outln!(w, "{}  FAILED: {}", i.name, i.message.clone().unwrap_or_default()),
                _ => outln!(w, "{}  {}", i.name, status_text(i.status)),
            }
        }
        let _ = w.flush();
        let count_of = |s: Status| items.iter().filter(|i| i.status == s).count();
        note(
            ctx,
            &format!(
                "{}: {} done, {} copied, {} skipped, {failed} failed",
                count(items.len(), "file"),
                count_of(Status::Decrypted) + count_of(Status::DecryptedCrcBad) + count_of(Status::EncryptedNew),
                count_of(Status::PassedThrough),
                count_of(Status::SkippedUnchanged) + count_of(Status::SkippedExisting)
            ),
        );
    }
    if failed > 0 {
        3
    } else if crc_bad > 0 {
        1
    } else {
        0
    }
}

fn existing_of(force: bool, skip: bool) -> Existing {
    if force {
        Existing::Overwrite
    } else if skip {
        Existing::Skip
    } else {
        Existing::Refuse
    }
}

fn decrypt(ctx: &Ctx, a: &DecryptArgs, job: &Job) -> Result<i32> {
    with_source(ctx, &a.inputs, a.from.as_deref(), a.all_files, &mut |src, dirs| {
        let output = output_for(&a.out, a.in_place, src.names().len(), dirs, a.from.is_some())?;
        if a.in_place && !a.dry_run {
            note(ctx, "note: the originals are kept as NAME.bak");
        }
        let items = batch_run(
            src,
            &BatchOptions { op: Op::Decrypt { unchecked: a.unchecked }, output, existing: existing_of(a.force, a.skip_existing), pass_through: !a.skip_plain, dry_run: a.dry_run },
            job,
        )?;
        Ok(print_batch(ctx, &items, a.dry_run, "plain"))
    })
}

fn encrypt(ctx: &Ctx, a: &EncryptArgs, job: &Job) -> Result<i32> {
    if a.inputs.is_empty() {
        return Err(Error::Refused("nothing to encrypt: give files or folders".to_string()));
    }
    let like = a.like.as_deref().map(|l| open_like(ctx, l)).transpose()?;
    with_source(ctx, &a.inputs, None, a.all_files, &mut |src, dirs| {
        let output = output_for(&a.out, a.in_place, src.names().len(), dirs, false)?;
        let run_with = |like_src: Option<&dyn ByteSource>| -> Result<Vec<ItemResult>> {
            batch_run(
                src,
                &BatchOptions { op: Op::Encrypt { like: like_src }, output, existing: existing_of(a.force, a.skip_existing), pass_through: !a.skip_encrypted, dry_run: a.dry_run },
                job,
            )
        };
        let items = match &like {
            None => run_with(None)?,
            Some(Like::Folder(f)) => run_with(Some(f))?,
            Some(Like::Archive(ar)) => {
                let s = ArchiveSource { archive: ar, filter: Filter::default(), include_shadowed: false };
                run_with(Some(&s))?
            }
        };
        Ok(print_batch(ctx, &items, a.dry_run, "encrypted"))
    })
}
