//! Sharing the staged client files. Four things, none of them a save of client data:
//!
//! - **Export patch**: the staged files (`<out>/data/...`) into `psNNNN.patch` with `shaiya_archive::patch::build`. The patch is
//!   built **unsigned** (a signing step is not built); [`sign_hook`] is the one place a signing step would go.
//! - **Export folder**: a plain copy of the staged tree.
//! - **Install / uninstall** on a test client folder the user picks (never the client the editor reads from): the editor makes its
//!   own backup of `data.sah` first, because `patch::apply` overwrites `data.sah.bak` on every call.
//! - **Copy loose**: staged files whose archive path the target does not have, copied to `<target>/data/...`.
//!
//! Everything here is plain file work so the tests can run it on a client made in a temp dir.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Value};
use shaiya_archive::Index;
use shaiya_patch as patch;

use crate::client_files::{check_out_dir, lies_in};

/// The index copy made before the first install into a folder. Never overwritten while the install record exists.
pub const FIRST_BACKUP: &str = "data.sah.svmap-editor";
/// The install record, in the folder that was installed into.
pub const RECORD: &str = "svmap-editor-install.json";
/// Shown in the export dialog and written to the log.
pub const UNSIGNED_NOTE: &str = "This patch is unsigned. The original updater does not check signatures. The replacement updater needs one: sign the file with `shaiya patch sign`.";

const EDGE: u64 = 1 << 20;

/// SIGNING HOOK. A signing step would run here on the finished patch and return the path of the signature file
/// (`psNNNN.patch.sig`). No signing step is built yet, so this returns `None` and the patch stays unsigned.
pub fn sign_hook(_patch: &Path) -> Result<Option<PathBuf>, String> {
    Ok(None)
}

/// One staged file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staged {
    /// Archive path, `/` separated: `world/2.wld`.
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
}

/// The files under `<out>/data`, sorted by path. Editor leftovers (`.bak`, `.tmp`) are not staged files and are skipped.
pub fn staged_files(out: &Path) -> Result<Vec<Staged>, String> {
    let root = out.join("data");
    let mut found = Vec::new();
    if root.is_dir() {
        walk(&root, "", &mut found).map_err(|e| format!("cannot read {}: {e}", root.display()))?;
    }
    found.sort_by_key(|s: &Staged| s.rel.to_lowercase());
    Ok(found)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<Staged>) -> std::io::Result<()> {
    for e in fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let meta = e.metadata()?;
        if meta.is_dir() {
            walk(&e.path(), &r, out)?;
        } else if meta.is_file() {
            let lower = name.to_lowercase();
            if !(lower.ends_with(".bak") || lower.ends_with(".tmp")) {
                out.push(Staged { rel: r, path: e.path(), size: meta.len() });
            }
        }
    }
    Ok(())
}

fn fnv(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    h
}
const FNV_START: u64 = 0xcbf29ce484222325;

/// What identifies `data.sah`: its length and a hash of all bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SahFp {
    pub len: u64,
    pub hash: u64,
}

/// What identifies the first `len` bytes of `data.saf` without reading a gigabyte: the length and a hash of the first and the last mebibyte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SafFp {
    pub len: u64,
    pub head: u64,
    pub tail: u64,
}

fn sah_fp(bytes: &[u8]) -> SahFp {
    SahFp { len: bytes.len() as u64, hash: fnv(bytes, FNV_START) }
}

/// The fingerprint of the first `len` bytes of `data.saf`.
pub fn saf_fp(path: &Path, len: u64) -> Result<SafFp, String> {
    let err = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
    let mut f = File::open(path).map_err(err)?;
    let have = f.metadata().map_err(err)?.len();
    if have < len {
        return Err(format!("{} is shorter ({have} bytes) than the {len} bytes it held", path.display()));
    }
    let mut buf = vec![0u8; EDGE.min(len) as usize];
    f.read_exact(&mut buf).map_err(err)?;
    let head = fnv(&buf, FNV_START);
    f.seek(SeekFrom::Start(len - buf.len() as u64)).map_err(err)?;
    f.read_exact(&mut buf).map_err(err)?;
    Ok(SafFp { len, head, tail: fnv(&buf, FNV_START) })
}

fn read_index(dir: &Path) -> Result<(Vec<u8>, Index), String> {
    let path = dir.join("data.sah");
    let bytes = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let idx = Index::parse(&bytes).map_err(|e| format!("{} is not an archive index the editor can read ({e}). A headerless data.sah is not supported.", path.display()))?;
    Ok((bytes, idx))
}

// -------------------------------------------------------------------------------------------------------------------- export

/// A patch number as the file name: `ps0109.patch`.
pub fn patch_name(n: u32) -> Result<String, String> {
    if (1..=9999).contains(&n) {
        Ok(format!("ps{n:04}.patch"))
    } else {
        Err("The patch number must be from 1 to 9999.".to_string())
    }
}

/// A temp folder that is removed again.
struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Result<Temp, String> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let mut last = String::new();
        for _ in 0..8 {
            let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("svmap-editor-{tag}-{}-{nanos}-{n}", std::process::id()));
            // create_dir fails when the folder exists, so two callers never share one
            match fs::create_dir(&p) {
                Ok(()) => return Ok(Temp(p)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = e.to_string(),
                Err(e) => return Err(format!("cannot make a temporary folder: {e}")),
            }
        }
        Err(format!("cannot make a temporary folder: {last}"))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Builds a patch holding exactly `staged` at `patch_file`, and reads it back to check the list. The patch format takes a folder,
/// so the files are copied (with their times, which the format keeps as the entry version) into a temporary tree first.
pub fn build_patch_file(staged: &[Staged], patch_file: &Path) -> Result<(), String> {
    if staged.is_empty() {
        return Err("Nothing is staged. Save an edited client map to the staging folder first.".to_string());
    }
    let tmp = Temp::new("patch")?;
    let tree = tmp.0.join("files");
    for s in staged {
        let dest = rel_join(&tree, &s.rel);
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p).map_err(|e| format!("cannot copy {}: {e}", s.rel))?;
        }
        fs::copy(&s.path, &dest).map_err(|e| format!("cannot copy {}: {e}", s.rel))?;
        if let Ok(t) = fs::metadata(&s.path).and_then(|m| m.modified()) {
            if let Ok(f) = OpenOptions::new().write(true).open(&dest) {
                let _ = f.set_modified(t);
            }
        }
    }
    patch::build(Some(&tree), None, None, patch_file).map_err(|e| format!("cannot build the patch: {e}"))?;
    let (files, loose) = patch::list(patch_file).map_err(|e| format!("the new patch cannot be read back: {e}"))?;
    let mut want: Vec<(String, u64)> = staged.iter().map(|s| (s.rel.to_lowercase(), s.size)).collect();
    let mut got: Vec<(String, u64)> = files.iter().map(|(p, n)| (p.to_lowercase(), *n as u64)).collect();
    want.sort();
    got.sort();
    if want != got || !loose.is_empty() {
        let _ = fs::remove_file(patch_file);
        return Err("The patch read back does not list the staged files. Nothing was exported.".to_string());
    }
    Ok(())
}

fn rel_join(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

/// What an export wrote.
#[derive(Debug)]
pub struct Exported {
    pub path: PathBuf,
    pub files: Vec<Staged>,
    /// Always true for now: see [`sign_hook`].
    pub unsigned: bool,
}

fn check_export_dest(dest: &Path, source: &Path, out: &Path) -> Result<(), String> {
    if lies_in(dest, source) {
        return Err(format!("{} is the client folder or inside it. The editor never writes into the client; pick another folder.", dest.display()));
    }
    if lies_in(dest, &out.join("data")) {
        return Err(format!("{} is inside the staged files. Pick a folder beside the staging folder.", dest.display()));
    }
    Ok(())
}

/// Export patch: `<dest>/psNNNN.patch`. An existing file of that name is not replaced.
pub fn export_patch(out: &Path, source: &Path, dest: &Path, number: u32) -> Result<Exported, String> {
    let name = patch_name(number)?;
    check_export_dest(dest, source, out)?;
    let staged = staged_files(out)?;
    if staged.is_empty() {
        return Err("Nothing is staged. Save an edited client map to the staging folder first.".to_string());
    }
    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;
    let path = dest.join(&name);
    if path.exists() {
        return Err(format!("{name} is already in that folder. Pick another folder or another number."));
    }
    let tmp = dest.join(format!("{name}.tmp"));
    if let Err(e) = build_patch_file(&staged, &tmp) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })?;
    sign_hook(&path)?;
    Ok(Exported { path, files: staged, unsigned: true })
}

/// Export folder: a copy of the staged tree at `<dest>/data/...`. A file that is already there stops the export before any write.
pub fn export_folder(out: &Path, source: &Path, dest: &Path) -> Result<Exported, String> {
    check_export_dest(dest, source, out)?;
    let staged = staged_files(out)?;
    if staged.is_empty() {
        return Err("Nothing is staged. Save an edited client map to the staging folder first.".to_string());
    }
    let root = dest.join("data");
    if let Some(s) = staged.iter().find(|s| rel_join(&root, &s.rel).exists()) {
        return Err(format!("{} is already in that folder. Pick an empty folder.", s.rel));
    }
    for s in &staged {
        let target = rel_join(&root, &s.rel);
        if let Some(p) = target.parent() {
            fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        }
        fs::copy(&s.path, &target).map_err(|e| format!("cannot copy {}: {e}", s.rel))?;
        let same = fs::read(&s.path).ok() == fs::read(&target).ok();
        if !same {
            return Err(format!("{} was copied wrong (the copy differs). Delete the folder {} and try again.", s.rel, root.display()));
        }
    }
    Ok(Exported { path: root, files: staged, unsigned: false })
}

// -------------------------------------------------------------------------------------------------------------------- target checks

/// What a target folder must pass before the editor touches its archive.
pub struct Guard<'a> {
    /// The client folder the editor reads from.
    pub source: &'a Path,
    /// More folders that are never a target (the main client, from the `SHAIYA_CLIENT` variable).
    pub also: &'a [PathBuf],
    /// The staging folder, when the check needs it.
    pub out: Option<&'a Path>,
}

/// Opens `data.saf` for writing with no sharing: fails while the game (or anything else) has it open. Windows only; elsewhere an
/// ordinary write open is the best test there is.
pub fn open_exclusive(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    o.open(path)
}

/// The checks every action on a target folder shares. `exclusive`: also refuse when `data.saf` is open elsewhere.
pub fn check_target(target: &Path, g: &Guard, exclusive: bool) -> Result<(), String> {
    if !target.is_dir() {
        return Err(format!("{} is not a folder.", target.display()));
    }
    for (what, other) in std::iter::once(("the client folder the editor reads", g.source)).chain(g.also.iter().map(|p| ("the main client folder", p.as_path()))) {
        if lies_in(target, other) || lies_in(other, target) {
            return Err(format!("{} is {what}, or lies in it or around it. Install only into a separate test copy of the client.", target.display()));
        }
    }
    if let Some(out) = g.out {
        check_out_dir(out, target)?;
    }
    for f in ["data.sah", "data.saf"] {
        if !target.join(f).is_file() {
            return Err(format!("{} has no {f}. It is not a client folder.", target.display()));
        }
    }
    read_index(target)?;
    if exclusive {
        open_exclusive(&target.join("data.saf")).map_err(|e| format!("data.saf cannot be opened for writing ({e}). Close the game and anything else that uses this client."))?;
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------------------------------- record

fn hex(n: u64) -> String {
    format!("{n:016x}")
}
fn unhex(v: &Value) -> Option<u64> {
    u64::from_str_radix(v.as_str()?, 16).ok()
}
fn sah_json(f: SahFp) -> Value {
    json!({ "len": f.len, "hash": hex(f.hash) })
}
fn saf_json(f: SafFp) -> Value {
    json!({ "len": f.len, "head": hex(f.head), "tail": hex(f.tail) })
}
fn sah_from(v: &Value) -> Option<SahFp> {
    Some(SahFp { len: v["len"].as_u64()?, hash: unhex(&v["hash"])? })
}
fn saf_from(v: &Value) -> Option<SafFp> {
    Some(SafFp { len: v["len"].as_u64()?, head: unhex(&v["head"])?, tail: unhex(&v["tail"])? })
}

/// The install record of a folder.
#[derive(Clone, Debug)]
pub struct Record {
    /// `data.sah` and the first part of `data.saf` before the first install: what uninstall gives back.
    pub sah_before: SahFp,
    pub saf_before: SafFp,
    /// The state after the latest install: what must still be true for uninstall to be safe.
    pub sah_after: SahFp,
    pub saf_after: SafFp,
    /// One entry per install: UTC stamp, the timestamped index copy and the archive paths.
    pub installs: Vec<(String, String, Vec<String>)>,
}

impl Record {
    fn to_json(&self) -> Value {
        json!({
            "tool": "svmap-editor",
            "first_backup": FIRST_BACKUP,
            "sah_before": sah_json(self.sah_before),
            "saf_before": saf_json(self.saf_before),
            "sah_after": sah_json(self.sah_after),
            "saf_after": saf_json(self.saf_after),
            "installs": self.installs.iter().map(|(t, b, f)| json!({ "time": t, "backup": b, "files": f })).collect::<Vec<_>>(),
        })
    }
    fn from_json(v: &Value) -> Option<Record> {
        let installs = v["installs"]
            .as_array()?
            .iter()
            .map(|i| Some((i["time"].as_str()?.to_string(), i["backup"].as_str()?.to_string(), i["files"].as_array()?.iter().filter_map(|f| f.as_str().map(String::from)).collect())))
            .collect::<Option<Vec<_>>>()?;
        Some(Record { sah_before: sah_from(&v["sah_before"])?, saf_before: saf_from(&v["saf_before"])?, sah_after: sah_from(&v["sah_after"])?, saf_after: saf_from(&v["saf_after"])?, installs })
    }
}

/// The record of a folder: `Ok(None)` when there is none, an error when the file is there and unreadable.
pub fn read_record(target: &Path) -> Result<Option<Record>, String> {
    let path = target.join(RECORD);
    let Ok(bytes) = fs::read(&path) else { return Ok(None) };
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{} cannot be read ({e}). Uninstall by hand from {FIRST_BACKUP} or delete the file if the client is fine.", path.display()))?;
    Record::from_json(&v).map(Some).ok_or_else(|| format!("{} is not an install record this editor wrote.", path.display()))
}

fn write_record(target: &Path, r: &Record) -> Result<(), String> {
    let path = target.join(RECORD);
    let tmp = target.join(format!("{RECORD}.tmp"));
    let text = serde_json::to_string_pretty(&r.to_json()).map_err(|e| e.to_string())?;
    let mut f = File::create(&tmp).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    drop(f);
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot write {}: {e}", path.display())
    })
}

/// Copies `from` over `to` through a `.restore` file and a rename.
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    let tmp = PathBuf::from(format!("{}.restore", to.display()));
    fs::copy(from, &tmp).map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), tmp.display()))?;
    fs::rename(&tmp, to).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", to.display())
    })
}

// -------------------------------------------------------------------------------------------------------------------- install

/// What an install did.
#[derive(Debug)]
pub struct Installed {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub bytes: u64,
    /// The first backup was made by this install.
    pub first: bool,
    pub timestamped: PathBuf,
}

fn timestamped_copy(target: &Path, now: SystemTime, expect: &[u8]) -> Result<PathBuf, String> {
    let stamp = crate::stamp::utc_stamp(now);
    for n in 0..1000 {
        let name = if n == 0 { format!("data.sah.svmap-editor-{stamp}") } else { format!("data.sah.svmap-editor-{stamp}_{n}") };
        let p = target.join(name);
        if !p.exists() {
            fs::copy(target.join("data.sah"), &p).map_err(|e| format!("cannot make the backup {}: {e}", p.display()))?;
            if fs::read(&p).ok().as_deref() != Some(expect) {
                let _ = fs::remove_file(&p);
                return Err(format!("the backup {} was not copied right", p.display()));
            }
            return Ok(p);
        }
    }
    Err("too many backups with the same time stamp".to_string())
}

/// Puts the archive back as it was before a failed install: the index from `backup` and `data.saf` cut back to `saf_len`.
fn roll_back(target: &Path, backup: &Path, sah: SahFp, saf_len: u64) -> (String, bool) {
    let _ = fs::remove_file(target.join("update.saf.tmp"));
    let mut said = Vec::new();
    let sah_now = || fs::read(target.join("data.sah")).map(|b| sah_fp(&b)).ok();
    if sah_now() != Some(sah) {
        match replace_file(backup, &target.join("data.sah")) {
            Ok(()) if sah_now() == Some(sah) => said.push("data.sah was put back".to_string()),
            Ok(()) => said.push(format!("data.sah could NOT be put back (the copy differs); copy {} over it", backup.display())),
            Err(e) => said.push(format!("data.sah could NOT be put back ({e}); copy {} over it", backup.display())),
        }
    }
    let sah_back = sah_now() == Some(sah);
    match open_exclusive(&target.join("data.saf")).and_then(|f| if f.metadata()?.len() > saf_len { f.set_len(saf_len) } else { Ok(()) }) {
        Ok(()) => said.push("data.saf was cut back".to_string()),
        Err(e) => said.push(format!("data.saf could not be cut back ({e}); the extra bytes at its end are harmless")),
    }
    (said.join(", "), sah_back)
}

/// Installs the staged files into the test client `target`.
///
/// Order: checks (read source, headerless index, game running), the first-install copy `data.sah.svmap-editor` (never replaced), a
/// timestamped copy of `data.sah`, `patch::apply`, then a check of the result: the new index parses, every staged file reads back
/// from `data.saf` byte for byte, no other entry moved, and the old part of `data.saf` is unchanged. A failure puts the old index
/// back. `data.sah.bak` is overwritten by `patch::apply` on each call, so it is not a backup of this editor.
pub fn install(out: &Path, target: &Path, g: &Guard, now: SystemTime) -> Result<Installed, String> {
    let guard = Guard { source: g.source, also: g.also, out: Some(out) };
    check_target(target, &guard, true)?;
    let staged = staged_files(out)?;
    if staged.is_empty() {
        return Err("Nothing is staged. Save an edited client map to the staging folder first.".to_string());
    }
    let record = read_record(target)?;
    let (sah_bytes, old_idx) = read_index(target)?;
    let sah_now = sah_fp(&sah_bytes);
    let saf_len = fs::metadata(target.join("data.saf")).map_err(|e| e.to_string())?.len();
    let saf_now = saf_fp(&target.join("data.saf"), saf_len)?;
    let first = record.is_none();
    if let Some(r) = &record {
        if r.sah_after != sah_now || r.saf_after != saf_now {
            return Err("data.sah or data.saf changed after the last install (a game update, or another tool). Uninstall refuses too. Restore the client from your own copy, or delete the install record and the backups by hand if you accept the new state.".to_string());
        }
    } else {
        if target.join(FIRST_BACKUP).exists() {
            return Err(format!("{FIRST_BACKUP} exists but there is no {RECORD}. Move the old backup away first so it cannot be mistaken for this install's."));
        }
        let first_path = target.join(FIRST_BACKUP);
        fs::copy(target.join("data.sah"), &first_path).map_err(|e| format!("cannot make {FIRST_BACKUP}: {e}"))?;
        let copy = fs::read(&first_path).map_err(|e| e.to_string())?;
        if copy != sah_bytes {
            let _ = fs::remove_file(&first_path);
            return Err(format!("{FIRST_BACKUP} was not copied right. Nothing was changed."));
        }
    }
    // Before the patch is applied nothing in the archive has changed: a failure only takes back the first backup made just now,
    // so the folder is as it was (no backup without a record left behind).
    let abort = |msg: String| -> String {
        if first {
            let _ = fs::remove_file(target.join(FIRST_BACKUP));
        }
        format!("{msg} Nothing was changed.")
    };
    let timestamped = timestamped_copy(target, now, &sah_bytes).map_err(abort)?;
    let (sah_before, saf_before) = match &record {
        Some(r) => (r.sah_before, r.saf_before),
        None => (sah_now, saf_now),
    };
    let fail = |msg: String, target: &Path| -> String {
        let (said, sah_back) = roll_back(target, &timestamped, sah_now, saf_len);
        if first && sah_back {
            let _ = fs::remove_file(target.join(FIRST_BACKUP));
        }
        format!("{msg} ({said}.)")
    };

    let tmp = Temp::new("install").map_err(abort)?;
    let patch_file = tmp.0.join("install.patch");
    build_patch_file(&staged, &patch_file).map_err(abort)?;
    let report = match patch::apply(&patch_file, target, &[]) {
        Ok(r) => r,
        Err(e) => return Err(fail(format!("The patch could not be applied: {e}."), target)),
    };
    if !report.loose.is_empty() {
        return Err(fail("The patch wrote loose files, which the editor never builds.".to_string(), target));
    }
    if let Err(e) = verify_install(target, &old_idx, &staged, saf_len, saf_now) {
        return Err(fail(format!("The result did not check out: {e}."), target));
    }
    let new_bytes = fs::read(target.join("data.sah")).map_err(|e| fail(format!("cannot read data.sah: {e}"), target))?;
    let sah_after = sah_fp(&new_bytes);
    let new_len = fs::metadata(target.join("data.saf")).map_err(|e| e.to_string())?.len();
    let saf_after = saf_fp(&target.join("data.saf"), new_len).map_err(|e| fail(e, target))?;
    let mut installs = record.as_ref().map(|r| r.installs.clone()).unwrap_or_default();
    installs.push((crate::stamp::utc_stamp(now), timestamped.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), staged.iter().map(|s| s.rel.clone()).collect()));
    let rec = Record { sah_before, saf_before, sah_after, saf_after, installs };
    if let Err(e) = write_record(target, &rec) {
        return Err(fail(e, target));
    }
    Ok(Installed { added: report.added, replaced: report.replaced, bytes: report.bytes, first, timestamped })
}

fn verify_install(target: &Path, old: &Index, staged: &[Staged], old_len: u64, old_fp: SafFp) -> Result<(), String> {
    let (_, idx) = read_index(target)?;
    let mut saf = File::open(target.join("data.saf")).map_err(|e| e.to_string())?;
    let total: u64 = staged.iter().map(|s| s.size).sum();
    let len = saf.metadata().map_err(|e| e.to_string())?.len();
    if len != old_len + total {
        return Err(format!("data.saf is {len} bytes, expected {}", old_len + total));
    }
    for s in staged {
        let e = idx.get(&s.rel).ok_or_else(|| format!("{} is not in the new index", s.rel))?;
        if e.length as u64 != s.size {
            return Err(format!("{} has length {} in the index, expected {}", s.rel, e.length, s.size));
        }
        let mut buf = vec![0u8; s.size as usize];
        saf.seek(SeekFrom::Start(e.offset)).and_then(|_| saf.read_exact(&mut buf)).map_err(|e| format!("{} cannot be read back: {e}", s.rel))?;
        if buf != fs::read(&s.path).map_err(|e| e.to_string())? {
            return Err(format!("{} reads back different from the staged file", s.rel));
        }
    }
    let staged_lower: Vec<String> = staged.iter().map(|s| s.rel.to_lowercase()).collect();
    for (p, e) in old.files() {
        if staged_lower.contains(&p.to_lowercase()) {
            continue;
        }
        match idx.get(&p) {
            Some(n) if n.offset == e.offset && n.length == e.length && n.version == e.version => {}
            _ => return Err(format!("the entry {p} changed")),
        }
    }
    if idx.file_count() < old.file_count() {
        return Err("the new index has fewer entries".to_string());
    }
    if saf_fp(&target.join("data.saf"), old_len)? != old_fp {
        return Err("the old part of data.saf changed".to_string());
    }
    Ok(())
}

// -------------------------------------------------------------------------------------------------------------------- uninstall

/// What an uninstall did.
#[derive(Debug)]
pub struct Uninstalled {
    pub installs: usize,
    /// `data.saf` was cut back to its old length (false: its length had changed since, so the dead bytes stay).
    pub saf_cut: bool,
}

/// Gives back the archive as it was before the first install: the index from `data.sah.svmap-editor`, then `data.saf` cut back to
/// its old length when it has not changed since the last install. Refuses while the game runs, and when `data.sah` or `data.saf`
/// changed since the last install (then restoring would undo someone else's work). Removes the record and the first backup after
/// the restored index checked out. The timestamped copies stay.
pub fn uninstall(target: &Path, g: &Guard) -> Result<Uninstalled, String> {
    check_target(target, &Guard { source: g.source, also: g.also, out: None }, true)?;
    let rec = read_record(target)?.ok_or_else(|| format!("{} has no {RECORD}. This editor did not install into it.", target.display()))?;
    let first = target.join(FIRST_BACKUP);
    let first_bytes = fs::read(&first).map_err(|e| format!("{FIRST_BACKUP} cannot be read ({e}). Nothing was changed."))?;
    if sah_fp(&first_bytes) != rec.sah_before {
        return Err(format!("{FIRST_BACKUP} is not the file that was saved at the first install. Nothing was changed."));
    }
    let (now_bytes, _) = read_index(target)?;
    let saf_len = fs::metadata(target.join("data.saf")).map_err(|e| e.to_string())?.len();
    if saf_len < rec.saf_before.len || saf_fp(&target.join("data.saf"), rec.saf_before.len)? != rec.saf_before {
        return Err("data.saf no longer starts with the archive from before the first install (a game update, or another tool). The old index would not fit it, so nothing was changed.".to_string());
    }
    let saf_unchanged = saf_len == rec.saf_after.len && saf_fp(&target.join("data.saf"), saf_len)? == rec.saf_after;
    if sah_fp(&now_bytes) != rec.sah_after {
        return Err("data.sah changed after the last install (a game update, or another tool). Restoring the old index would undo that, so nothing was changed.".to_string());
    }
    replace_file(&first, &target.join("data.sah"))?;
    let back = fs::read(target.join("data.sah")).map_err(|e| e.to_string())?;
    if sah_fp(&back) != rec.sah_before {
        // put the installed index back
        let _ = fs::write(target.join("data.sah"), &now_bytes);
        return Err("The restored data.sah did not check out, so the installed one was put back.".to_string());
    }
    let mut cut = false;
    if saf_unchanged {
        let f = open_exclusive(&target.join("data.saf")).map_err(|e| format!("data.saf cannot be opened ({e}). The index is restored; the extra bytes at the end of data.saf are harmless."))?;
        f.set_len(rec.saf_before.len).map_err(|e| format!("data.saf could not be cut back ({e}). The index is restored; the extra bytes are harmless."))?;
        drop(f);
        if saf_fp(&target.join("data.saf"), rec.saf_before.len)? != rec.saf_before || fs::metadata(target.join("data.saf")).map(|m| m.len()).ok() != Some(rec.saf_before.len) {
            return Err("data.saf does not match its old state after the cut. The index is restored.".to_string());
        }
        cut = true;
    }
    fs::remove_file(target.join(RECORD)).map_err(|e| format!("The archive is restored but {RECORD} could not be removed: {e}"))?;
    let _ = fs::remove_file(&first);
    Ok(Uninstalled { installs: rec.installs.len(), saf_cut: cut })
}

// -------------------------------------------------------------------------------------------------------------------- copy loose

/// What copy loose decided for one staged file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LooseVerdict {
    Copy,
    /// The archive has this path: the game reads the archive first and ignores a loose copy.
    InArchive,
    /// The same bytes are already there.
    AlreadyThere,
    /// A different file is there; it is not replaced.
    Exists,
}

/// What would happen to each staged file.
pub fn loose_plan(out: &Path, target: &Path, g: &Guard) -> Result<Vec<(Staged, LooseVerdict)>, String> {
    check_target(target, &Guard { source: g.source, also: g.also, out: Some(out) }, false)?;
    let (_, idx) = read_index(target)?;
    let root = target.join("data");
    Ok(staged_files(out)?
        .into_iter()
        .map(|s| {
            let dest = rel_join(&root, &s.rel);
            let v = if idx.get(&s.rel).is_some() {
                LooseVerdict::InArchive
            } else if dest.exists() {
                if fs::read(&dest).ok() == fs::read(&s.path).ok() {
                    LooseVerdict::AlreadyThere
                } else {
                    LooseVerdict::Exists
                }
            } else {
                LooseVerdict::Copy
            };
            (s, v)
        })
        .collect())
}

/// Copies the staged files whose archive path the target lacks to `<target>/data/...`; refuses each of the others (never
/// replaces). Returns the plan with the files that were written marked as `Copy`.
pub fn copy_loose(out: &Path, target: &Path, g: &Guard) -> Result<Vec<(Staged, LooseVerdict)>, String> {
    let plan = loose_plan(out, target, g)?;
    let root = target.join("data");
    for (s, v) in &plan {
        if *v != LooseVerdict::Copy {
            continue;
        }
        let dest = rel_join(&root, &s.rel);
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p).map_err(|e| format!("cannot create {}: {e}", p.display()))?;
        }
        let tmp = PathBuf::from(format!("{}.tmp", dest.display()));
        fs::copy(&s.path, &tmp).map_err(|e| format!("cannot copy {}: {e}", s.rel))?;
        if fs::read(&tmp).ok() != fs::read(&s.path).ok() {
            let _ = fs::remove_file(&tmp);
            return Err(format!("{} was copied wrong. Nothing was left of it.", s.rel));
        }
        fs::rename(&tmp, &dest).map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("cannot write {}: {e}", dest.display())
        })?;
    }
    Ok(plan)
}

/// Helpers the window tests share with this module's tests.
#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;
    use shaiya_archive::{encode_name, Entry};

    /// A tiny client: `data.sah` / `data.saf` holding `world/1.wld`, `world/2.wld` and `item/a.bin`.
    pub(crate) fn tiny_client(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        let mut idx = Index::new();
        let mut saf: Vec<u8> = Vec::new();
        for (p, fill) in [("world/1.wld", 1u8), ("world/2.wld", 2), ("item/a.bin", 3)] {
            let data = vec![fill; 100 + fill as usize];
            let (_, name) = shaiya_archive::split_path(p).unwrap();
            idx.put(p, Entry { name: encode_name(name), offset: saf.len() as u64, length: data.len() as u32, version: 7 }).unwrap();
            saf.extend_from_slice(&data);
        }
        fs::write(dir.join("data.sah"), idx.to_bytes()).unwrap();
        fs::write(dir.join("data.saf"), saf).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::tiny_client;
    use super::*;
    use crate::testutil::TempDir;

    fn stage(out: &Path, files: &[(&str, &[u8])]) {
        for (rel, data) in files {
            let p = rel_join(&out.join("data"), rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, data).unwrap();
        }
    }

    fn sums(dir: &Path) -> (Vec<u8>, Vec<u8>) {
        (fs::read(dir.join("data.sah")).unwrap(), fs::read(dir.join("data.saf")).unwrap())
    }

    #[test]
    fn patch_export_lists_only_edited() {
        let root = TempDir::new("c6-export");
        let (source, out) = (root.join("source"), root.join("out"));
        tiny_client(&source);
        stage(&out, &[("world/2.wld", &[9u8; 50]), ("world/10.txt", b"abc")]);
        // editor leftovers and a file beside data are not staged files
        fs::write(out.join("data/world/2.wld.bak"), b"old").unwrap();
        fs::write(out.join("data/world/2.wld.tmp"), b"x").unwrap();
        fs::write(out.join("svmap-editor-new-map-10.txt"), b"note").unwrap();
        let (a, b) = sums(&source);
        let dest = root.join("share");
        let done = export_patch(&out, &source, &dest, 109).unwrap();
        assert!(done.unsigned && done.path.ends_with("ps0109.patch"));
        let rels: Vec<&str> = done.files.iter().map(|s| s.rel.as_str()).collect();
        assert_eq!(rels, ["world/10.txt", "world/2.wld"]);
        let (files, loose) = patch::list(&done.path).unwrap();
        let mut got: Vec<(String, u32)> = files;
        got.sort();
        assert_eq!(got, vec![("world/10.txt".to_string(), 3), ("world/2.wld".to_string(), 50)]);
        assert!(loose.is_empty());
        assert_eq!(sign_hook(&done.path).unwrap(), None);
        // the patch applies to a copy of the client and the staged bytes come out
        let copy = root.join("copy");
        tiny_client(&copy);
        let r = patch::apply(&done.path, &copy, &[]).unwrap();
        assert_eq!((r.added.len(), r.replaced.len()), (1, 1));
        // no overwrite, number range, and the client source is never a destination
        assert!(export_patch(&out, &source, &dest, 109).unwrap_err().contains("already"));
        assert!(export_patch(&out, &source, &dest, 0).is_err() && export_patch(&out, &source, &dest, 10000).is_err());
        assert!(export_patch(&out, &source, &source.join("sub"), 1).unwrap_err().contains("client folder"));
        assert!(!source.join("sub").exists() && !source.join("ps0001.patch").exists());
        // the folder copy
        let f = export_folder(&out, &source, &root.join("plain")).unwrap();
        assert_eq!(f.files.len(), 2);
        assert_eq!(fs::read(root.join("plain/data/world/2.wld")).unwrap(), vec![9u8; 50]);
        assert!(!root.join("plain/data/world/2.wld.bak").exists());
        assert!(export_folder(&out, &source, &root.join("plain")).unwrap_err().contains("already"));
        // nothing staged
        assert!(export_patch(&root.join("empty"), &source, &root.join("x"), 5).unwrap_err().contains("Nothing is staged"));
        assert_eq!(sums(&source), (a, b), "the client source is untouched");
    }

    #[test]
    fn install_refuses_read_source_and_running_game() {
        let root = TempDir::new("c6-refuse");
        let (source, out, test) = (root.join("source"), root.join("out"), root.join("test"));
        tiny_client(&source);
        tiny_client(&test);
        stage(&out, &[("world/2.wld", &[9u8; 50])]);
        let before = (sums(&source), sums(&test));
        let none: Vec<PathBuf> = Vec::new();
        let g = Guard { source: &source, also: &none, out: None };
        let now = SystemTime::now();
        // the read source, a folder inside it, and a folder around it
        assert!(install(&out, &source, &g, now).unwrap_err().contains("separate test copy"));
        fs::create_dir_all(source.join("inner")).unwrap();
        tiny_client(&source.join("inner"));
        assert!(install(&out, &source.join("inner"), &g, now).unwrap_err().contains("separate test copy"));
        assert!(uninstall(&source, &g).is_err());
        // the main client from the variable
        let main = vec![test.clone()];
        let g_main = Guard { source: &source, also: &main, out: None };
        assert!(install(&out, &test, &g_main, now).unwrap_err().contains("main client"));
        // not a client / headerless index / staging folder inside the target
        assert!(install(&out, &root.join("nothing"), &g, now).unwrap_err().contains("not a folder"));
        let bare = root.join("bare");
        fs::create_dir_all(&bare).unwrap();
        assert!(install(&out, &bare, &g, now).unwrap_err().contains("no data.sah"));
        let headerless = root.join("headerless");
        fs::create_dir_all(&headerless).unwrap();
        fs::write(headerless.join("data.sah"), b"\x01\x02\x03").unwrap();
        fs::write(headerless.join("data.saf"), b"x").unwrap();
        assert!(install(&out, &headerless, &g, now).unwrap_err().contains("not an archive index"));
        assert!(install(&test.join("staging"), &test, &g, now).is_err(), "nothing staged there, and a staging folder inside the target is refused");
        assert!(install(&root.join("no-stage"), &test, &g, now).unwrap_err().contains("Nothing is staged"));
        // the game running: data.saf is open with read sharing only
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let game = OpenOptions::new().read(true).share_mode(1).open(test.join("data.saf")).unwrap();
            let e = install(&out, &test, &g, now).unwrap_err();
            assert!(e.contains("Close the game"), "{e}");
            assert!(uninstall(&test, &g).is_err());
            drop(game);
        }
        assert_eq!(before, (sums(&source), sums(&test)), "every refusal left both clients as they were");
        assert!(!test.join(FIRST_BACKUP).exists() && !test.join(RECORD).exists());
        install(&out, &test, &g, now).unwrap();
    }

    #[test]
    fn install_keeps_first_index_backup() {
        let root = TempDir::new("c6-first");
        let (source, out, test) = (root.join("source"), root.join("out"), root.join("test"));
        tiny_client(&source);
        tiny_client(&test);
        let none: Vec<PathBuf> = Vec::new();
        let g = Guard { source: &source, also: &none, out: None };
        let (sah0, saf0) = sums(&test);
        stage(&out, &[("world/2.wld", &[9u8; 50]), ("world/10.wld", &[5u8; 20])]);
        let t0 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let a = install(&out, &test, &g, t0).unwrap();
        assert!(a.first);
        assert_eq!((a.added.len(), a.replaced.len(), a.bytes), (1, 1, 70));
        assert_eq!(fs::read(test.join(FIRST_BACKUP)).unwrap(), sah0, "the first backup is the original index");
        assert_eq!(fs::read(test.join(a.timestamped.file_name().unwrap())).unwrap(), sah0);
        assert!(a.timestamped.file_name().unwrap().to_string_lossy().starts_with("data.sah.svmap-editor-2023"));
        // the new archive holds the staged bytes, the others are untouched
        let (sah1, saf1) = sums(&test);
        assert_ne!(sah1, sah0);
        assert_eq!(&saf1[..saf0.len()], &saf0[..]);
        let idx = Index::parse(&sah1).unwrap();
        let e = idx.get("world/2.wld").unwrap();
        assert_eq!(&saf1[e.offset as usize..e.offset as usize + 50], &[9u8; 50]);
        assert_eq!(idx.file_count(), 4);
        assert!(test.join("data.sah.bak").exists(), "apply made its own .bak (overwritten each time: not our backup)");
        // a second install keeps the first backup and adds its own timestamped copy
        stage(&out, &[("world/2.wld", &[8u8; 60])]);
        let b = install(&out, &test, &g, t0 + std::time::Duration::from_secs(60)).unwrap();
        assert!(!b.first);
        assert_eq!(fs::read(test.join(FIRST_BACKUP)).unwrap(), sah0, "still the original index");
        assert_ne!(a.timestamped, b.timestamped);
        assert_eq!(fs::read(&b.timestamped).unwrap(), sah1, "the second copy is the index before the second install");
        let rec = read_record(&test).unwrap().unwrap();
        assert_eq!(rec.installs.len(), 2);
        assert_eq!(rec.sah_before, sah_fp(&sah0));
        // someone else changes data.sah: install and uninstall refuse
        let (sah2, _) = sums(&test);
        let mut broken = sah2.clone();
        broken.push(0);
        fs::write(test.join("data.sah"), &broken).unwrap();
        assert!(install(&out, &test, &g, t0).is_err());
        fs::write(test.join("data.sah"), &sah2).unwrap();
        // a leftover first backup with no record is not taken for ours
        fs::remove_file(test.join(RECORD)).unwrap();
        assert!(install(&out, &test, &g, t0).unwrap_err().contains(FIRST_BACKUP));
    }

    #[test]
    fn install_refuses_a_read_only_archive() {
        let root = TempDir::new("c6-rollback");
        let (source, out, test) = (root.join("source"), root.join("out"), root.join("test"));
        tiny_client(&source);
        tiny_client(&test);
        let none: Vec<PathBuf> = Vec::new();
        let g = Guard { source: &source, also: &none, out: None };
        stage(&out, &[("world/2.wld", &[9u8; 50])]);
        let before = sums(&test);
        // make data.saf read-only so apply cannot append
        let mut perm = fs::metadata(test.join("data.saf")).unwrap().permissions();
        perm.set_readonly(true);
        fs::set_permissions(test.join("data.saf"), perm.clone()).unwrap();
        let e = install(&out, &test, &g, SystemTime::now()).unwrap_err();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        fs::set_permissions(test.join("data.saf"), perm).unwrap();
        assert!(e.contains("cannot be opened") || e.contains("could not be applied"), "{e}");
        assert_eq!(sums(&test), before);
        assert!(!test.join("update.saf.tmp").exists());
        assert!(!test.join(FIRST_BACKUP).exists() && !test.join(RECORD).exists(), "a refused install leaves no backup or record");
    }

    #[test]
    fn uninstall_refuses_when_the_old_archive_changed() {
        let root = TempDir::new("c6-oldpart");
        let (source, out, test) = (root.join("source"), root.join("out"), root.join("test"));
        tiny_client(&source);
        tiny_client(&test);
        let none: Vec<PathBuf> = Vec::new();
        let g = Guard { source: &source, also: &none, out: None };
        stage(&out, &[("world/2.wld", &[9u8; 50])]);
        install(&out, &test, &g, SystemTime::now()).unwrap();
        let (sah1, mut saf1) = sums(&test);
        saf1[0] ^= 0xff;
        fs::write(test.join("data.saf"), &saf1).unwrap();
        let e = uninstall(&test, &g).unwrap_err();
        assert!(e.contains("nothing was changed"), "{e}");
        assert_eq!(sums(&test), (sah1, saf1), "a refused uninstall writes nothing");
        assert!(test.join(RECORD).exists() && test.join(FIRST_BACKUP).exists());
    }

    #[test]
    fn uninstall_restores_index() {
        let root = TempDir::new("c6-uninstall");
        let (source, out, test) = (root.join("source"), root.join("out"), root.join("test"));
        tiny_client(&source);
        tiny_client(&test);
        let none: Vec<PathBuf> = Vec::new();
        let g = Guard { source: &source, also: &none, out: None };
        let original = sums(&test);
        let source_before = sums(&source);
        assert!(uninstall(&test, &g).unwrap_err().contains("did not install"));
        stage(&out, &[("world/2.wld", &[9u8; 50]), ("world/10.wld", &[5u8; 20])]);
        install(&out, &test, &g, SystemTime::now()).unwrap();
        stage(&out, &[("world/2.wld", &[8u8; 60])]);
        install(&out, &test, &g, SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert_ne!(sums(&test), original);
        let u = uninstall(&test, &g).unwrap();
        assert_eq!(u.installs, 2);
        assert!(u.saf_cut);
        assert_eq!(sums(&test), original, "index and archive are byte for byte what they were");
        assert!(!test.join(RECORD).exists() && !test.join(FIRST_BACKUP).exists());
        assert!(fs::read_dir(&test).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("data.sah.svmap-editor-")), "the timestamped copies stay");
        // after an uninstall a new install starts a new first backup
        let again = install(&out, &test, &g, SystemTime::now() + std::time::Duration::from_secs(9)).unwrap();
        assert!(again.first);
        // a changed data.saf (another tool appended) is not cut: the index still restores
        let mut f = OpenOptions::new().append(true).open(test.join("data.saf")).unwrap();
        f.write_all(b"extra").unwrap();
        drop(f);
        let u = uninstall(&test, &g).unwrap();
        assert!(!u.saf_cut, "an archive that grew since is not cut");
        assert_eq!(fs::read(test.join("data.sah")).unwrap(), original.0, "the index is still restored");
        assert_eq!(sums(&source), source_before);
    }

    #[test]
    fn copy_loose_warns_archive_path() {
        let root = TempDir::new("c6-loose");
        let (source, out, test) = (root.join("source"), root.join("out"), root.join("test"));
        tiny_client(&source);
        tiny_client(&test);
        let none: Vec<PathBuf> = Vec::new();
        let g = Guard { source: &source, also: &none, out: None };
        stage(&out, &[("world/2.wld", &[9u8; 50]), ("world/10.wld", &[5u8; 20]), ("world/10.txt", b"txt")]);
        fs::create_dir_all(test.join("data/world")).unwrap();
        fs::write(test.join("data/world/10.txt"), b"other").unwrap();
        let before = sums(&test);
        let plan = loose_plan(&out, &test, &g).unwrap();
        let by = |rel: &str| plan.iter().find(|(s, _)| s.rel == rel).unwrap().1.clone();
        assert_eq!(by("world/2.wld"), LooseVerdict::InArchive, "the archive wins, so a loose copy would be ignored");
        assert_eq!(by("world/10.wld"), LooseVerdict::Copy);
        assert_eq!(by("world/10.txt"), LooseVerdict::Exists);
        copy_loose(&out, &test, &g).unwrap();
        assert_eq!(fs::read(test.join("data/world/10.wld")).unwrap(), vec![5u8; 20]);
        assert!(!test.join("data/world/2.wld").exists(), "a path the archive has is refused");
        assert_eq!(fs::read(test.join("data/world/10.txt")).unwrap(), b"other", "nothing is replaced");
        assert!(!test.join("data/world/10.wld.tmp").exists());
        assert_eq!(by_after(&out, &test, &g, "world/10.wld"), LooseVerdict::AlreadyThere);
        assert_eq!(sums(&test), before, "the archive is not touched");
        assert!(copy_loose(&out, &source, &g).is_err(), "never into the read source");
    }

    fn by_after(out: &Path, test: &Path, g: &Guard, rel: &str) -> LooseVerdict {
        loose_plan(out, test, g).unwrap().into_iter().find(|(s, _)| s.rel == rel).unwrap().1
    }

    /// The real client's archive, copied to a temp folder: installing the staged files and uninstalling gives the same bytes back.
    /// Needs `SHAIYA_CLIENT`; the original is only read (its hash is checked before and after).
    #[test]
    fn real_archive_copy_installs_and_uninstalls() {
        let Some(client) = std::env::var_os("SHAIYA_CLIENT").map(PathBuf::from) else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        if !client.join("data.sah").is_file() {
            eprintln!("skipped: SHAIYA_CLIENT has no data.sah");
            return;
        }
        let orig = fs::read(client.join("data.sah")).unwrap();
        let (len0, mtime0) = {
            let m = fs::metadata(client.join("data.saf")).unwrap();
            (m.len(), m.modified().unwrap())
        };
        let root = TempDir::new("c6-real");
        let test = root.join("test");
        fs::create_dir_all(&test).unwrap();
        fs::write(test.join("data.sah"), &orig).unwrap();
        // data.saf is large: use a short stand-in of the right length is not possible, so only the index is real and the archive
        // part is a zero-filled file that is long enough for every entry
        let idx = Index::parse(&orig).unwrap();
        let end = idx.files().iter().map(|(_, e)| e.offset + e.length as u64).max().unwrap_or(0);
        let f = File::create(test.join("data.saf")).unwrap();
        f.set_len(end).unwrap();
        drop(f);
        let out = root.join("out");
        stage(&out, &[("world/200.wld", &[1u8; 64])]);
        let none: Vec<PathBuf> = Vec::new();
        let g = Guard { source: &client, also: &none, out: None };
        let before = sums(&test);
        install(&out, &test, &g, SystemTime::now()).unwrap();
        uninstall(&test, &g).unwrap();
        assert_eq!(sums(&test), before);
        assert_eq!(fs::read(client.join("data.sah")).unwrap(), orig, "the client's index is untouched");
        let m = fs::metadata(client.join("data.saf")).unwrap();
        assert_eq!((m.len(), m.modified().unwrap()), (len0, mtime0), "the client's archive is untouched");
    }
}
