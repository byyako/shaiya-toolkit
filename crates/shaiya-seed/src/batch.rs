//! SData work over many files with one shape (the command line's `shaiya sdata` and Shaiya Toolkit's SData tab): info, check, decrypt and
//! encrypt over any [`ByteSource`] (a folder, an archive, memory).

use super::{decrypt_body, info, is_encrypted, wrap, Wrapper};
use shaiya_core::fs::{make_backup, write_atomic_from_with, Existing, Sync};
use shaiya_core::source::ByteSource;
use shaiya_core::{Error, Job, Result, Unit};
use std::path::{Path, PathBuf};

pub enum Op<'a> {
    Info,
    Check,
    Decrypt { unchecked: bool },
    Encrypt { like: Option<&'a dyn ByteSource> },
}

pub enum Output {
    None,
    File(PathBuf),
    Dir(PathBuf),
    InPlace,
}

pub struct BatchOptions<'a> {
    pub op: Op<'a>,
    pub output: Output,
    pub existing: Existing,
    /// Files that need no change (plain input to decrypt, encrypted input to encrypt) are copied unchanged; otherwise they are skipped.
    pub pass_through: bool,
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Status {
    Encrypted,
    Plain,
    CrcOk,
    CrcBad,
    Decrypted,
    DecryptedCrcBad,
    EncryptedNew,
    PassedThrough,
    SkippedUnchanged,
    SkippedExisting,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct ItemResult {
    pub name: String,
    pub status: Status,
    pub size: u64,
    pub plain_size: Option<u64>,
    pub crc: Option<u32>,
    pub crc_ok: Option<bool>,
    pub padding_zero: Option<bool>,
    pub tail_zero: Option<bool>,
    pub out: Option<PathBuf>,
    pub message: Option<String>,
}

impl ItemResult {
    fn new(name: &str, status: Status, size: u64) -> ItemResult {
        ItemResult { name: name.to_string(), status, size, plain_size: None, crc: None, crc_ok: None, padding_zero: None, tail_zero: None, out: None, message: None }
    }
    fn failed(name: &str, size: u64, why: impl ToString) -> ItemResult {
        let mut r = ItemResult::new(name, Status::Failed, size);
        r.message = Some(why.to_string());
        r
    }
}

/// What the file looks like inside, without failing on a bad checksum: (plaintext, tail, crc_ok).
fn open_wrapper(data: &[u8], name: &str) -> Result<(Vec<u8>, Wrapper, bool)> {
    let h = info(data, name)?.ok_or_else(|| Error::format(name, Some(0), "not a SEED-encrypted file"))?;
    let mut plain = decrypt_body(data);
    let tail = plain.split_off(h.size as usize);
    let ok = super::crc32(&plain) == h.crc;
    Ok((plain, Wrapper::Seed { crc: h.crc, size: h.size, padding: h.padding, tail }, ok))
}

fn find_original(like: &dyn ByteSource, name: &str) -> Option<String> {
    let names = like.names();
    if let Some(n) = names.iter().find(|n| n.as_str() == name) {
        return Some(n.clone());
    }
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    if let Some(n) = names.iter().find(|n| norm(n) == norm(name)) {
        return Some(n.clone());
    }
    let file = norm(name).rsplit('/').next().unwrap_or("").to_string();
    let hits: Vec<&String> = names.iter().filter(|n| norm(n).rsplit('/').next() == Some(file.as_str())).collect();
    (hits.len() == 1).then(|| hits[0].clone())
}

fn target_of(output: &Output, name: &str) -> Option<PathBuf> {
    match output {
        Output::File(p) => Some(p.clone()),
        Output::Dir(d) => Some(name.replace('\\', "/").split('/').filter(|s| !s.is_empty()).fold(d.clone(), |p, s| p.join(s))),
        Output::None | Output::InPlace => None,
    }
}

fn write_out(target: &Path, bytes: &[u8], job: &Job) -> Result<()> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    }
    write_atomic_from_with(target, &mut &bytes[..], Sync::No, job).map(|_| ())
}

/// Every name of `src` in order. `Info`/`Check` never write. `Decrypt`: unwrap (strict CRC unless `unchecked`); plain input ->
/// `PassedThrough` (copied) or `SkippedUnchanged`. `Encrypt`: `like == None` -> `encrypt()`; `like == Some(s)` -> the original with the
/// same name (exact name, else a unique case-insensitive file-name match) gives the wrapper (plain stays plain); encrypted input ->
/// `PassedThrough`/`SkippedUnchanged`. `Output::File` only with exactly one name; `InPlace` only for names with `file_path` (a backup
/// is made first). `Existing::Refuse` and any target exists -> `Error::Refused` before writing. A failing item is recorded (`Failed`
/// plus a message) and the batch continues.
pub fn run(src: &dyn ByteSource, opts: &BatchOptions, job: &Job) -> Result<Vec<ItemResult>> {
    let names = src.names();
    if matches!(opts.output, Output::File(_)) && names.len() != 1 {
        return Err(Error::Refused(format!("-o names one file, but {} files were given; name a folder instead", names.len())));
    }
    let writes = !matches!(opts.op, Op::Info | Op::Check) && !matches!(opts.output, Output::None);
    if writes && opts.existing == Existing::Refuse && !matches!(opts.output, Output::InPlace) {
        let clash: Vec<String> = names.iter().filter_map(|n| target_of(&opts.output, n)).filter(|t| t.exists()).map(|t| t.display().to_string()).collect();
        if !clash.is_empty() {
            let shown: Vec<&str> = clash.iter().take(5).map(String::as_str).collect();
            return Err(Error::Refused(format!("{} output files already exist (for example {}); use --force to overwrite or --skip-existing to leave them", clash.len(), shown.join(", "))));
        }
    }
    job.stage(match opts.op {
        Op::Info | Op::Check => "checking",
        Op::Decrypt { .. } => "decrypting",
        Op::Encrypt { .. } => "encrypting",
    });
    job.total(Some(names.len() as u64), Unit::Items);
    let mut results = Vec::with_capacity(names.len());
    for name in &names {
        job.check()?;
        let r = one(src, name, opts, writes, job);
        results.push(r);
        job.advance(1);
    }
    Ok(results)
}

fn one(src: &dyn ByteSource, name: &str, opts: &BatchOptions, writes: bool, job: &Job) -> ItemResult {
    let data = match src.read(name) {
        Ok(d) => d,
        Err(e) => return ItemResult::failed(name, src.size(name).unwrap_or(0), e),
    };
    let size = data.len() as u64;
    let mut res = ItemResult::new(name, Status::Plain, size);
    // 1. what the item becomes (None = nothing to write)
    let mut output: Option<Vec<u8>> = None;
    match &opts.op {
        Op::Info | Op::Check => {
            if is_encrypted(&data) {
                match open_wrapper(&data, name) {
                    Ok((plain, w, ok)) => {
                        let Wrapper::Seed { crc, padding, tail, .. } = &w else { unreachable!("open_wrapper returns Seed") };
                        res.plain_size = Some(plain.len() as u64);
                        res.crc = Some(*crc);
                        res.crc_ok = Some(ok);
                        res.padding_zero = Some(padding.iter().all(|&b| b == 0));
                        res.tail_zero = Some(tail.iter().all(|&b| b == 0));
                        res.status = match (&opts.op, ok) {
                            (Op::Check, true) => Status::CrcOk,
                            (Op::Check, false) => Status::CrcBad,
                            _ => Status::Encrypted,
                        };
                    }
                    Err(e) => return ItemResult::failed(name, size, e),
                }
            }
        }
        Op::Decrypt { unchecked } => {
            if !is_encrypted(&data) {
                res.status = if opts.pass_through { Status::PassedThrough } else { Status::SkippedUnchanged };
                if opts.pass_through {
                    output = Some(data.clone());
                }
            } else {
                match open_wrapper(&data, name) {
                    Ok((plain, w, ok)) => {
                        let Wrapper::Seed { crc, .. } = &w else { unreachable!() };
                        res.crc = Some(*crc);
                        res.crc_ok = Some(ok);
                        res.plain_size = Some(plain.len() as u64);
                        if ok {
                            res.status = Status::Decrypted;
                            output = Some(plain);
                        } else if *unchecked {
                            res.status = Status::DecryptedCrcBad;
                            res.message = Some("checksum mismatch (damaged file or a different key); written because of --unchecked".to_string());
                            output = Some(plain);
                        } else {
                            return ItemResult { crc: res.crc, crc_ok: Some(false), ..ItemResult::failed(name, size, "SEED checksum failed (damaged file or a different key)") };
                        }
                    }
                    Err(e) => return ItemResult::failed(name, size, e),
                }
            }
        }
        Op::Encrypt { like } => {
            if is_encrypted(&data) {
                res.status = if opts.pass_through { Status::PassedThrough } else { Status::SkippedUnchanged };
                if opts.pass_through {
                    output = Some(data.clone());
                }
            } else {
                res.plain_size = Some(size);
                match like {
                    None => {
                        res.status = Status::EncryptedNew;
                        output = Some(super::encrypt(&data));
                    }
                    Some(orig) => match find_original(*orig, name) {
                        None => {
                            res.status = Status::EncryptedNew;
                            res.message = Some("no original with this name; used a zero header".to_string());
                            output = Some(super::encrypt(&data));
                        }
                        Some(on) => match orig.read(&on) {
                            Err(e) => return ItemResult::failed(name, size, format!("reading the original {on}: {e}")),
                            Ok(o) => {
                                if is_encrypted(&o) {
                                    match open_wrapper(&o, &on) {
                                        Ok((_, w, _)) => {
                                            res.status = Status::EncryptedNew;
                                            output = Some(wrap(&data, &w));
                                        }
                                        Err(e) => return ItemResult::failed(name, size, format!("the original {on}: {e}")),
                                    }
                                } else {
                                    // a plain original: this one stays plain
                                    res.status = Status::PassedThrough;
                                    res.message = Some("the original is not encrypted; kept plain".to_string());
                                    output = Some(data.clone());
                                }
                            }
                        },
                    },
                }
            }
        }
    }
    // 2. writing
    let Some(bytes) = output else { return res };
    if !writes {
        return res;
    }
    match &opts.output {
        Output::None => {}
        Output::InPlace => {
            let Some(path) = src.file_path(name) else {
                return ItemResult::failed(name, size, "not a file on disk; in-place changes need files (use -o for an archive)");
            };
            res.out = Some(path.clone());
            if opts.dry_run {
                return res;
            }
            if let Err(e) = make_backup(&path).and_then(|_| write_out(&path, &bytes, job)) {
                return ItemResult::failed(name, size, e);
            }
        }
        out => {
            let Some(target) = target_of(out, name) else { return res };
            res.out = Some(target.clone());
            if target.exists() && opts.existing == Existing::Skip {
                res.status = Status::SkippedExisting;
                return res;
            }
            if opts.dry_run {
                return res;
            }
            if let Err(e) = write_out(&target, &bytes, job) {
                return ItemResult::failed(name, size, e);
            }
        }
    }
    res
}
