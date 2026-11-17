//! The archive, patch and SData batch capabilities of the matrix (archive reading, in-place edits and rollback on a stub SAF, patch apply and
//! rollback on a stub SAF, and SData batch work. Everything that writes does so in a temporary copy: the client's `data.sah` is copied
//! next to an empty `data.saf`, so no client file is ever changed and no SAF content is needed.

use crate::{Cap, Cell, Observed};
use shaiya_archive::check::{check_index, Severity};
use shaiya_archive::diff::{diff, Change, DiffOptions, Side};
use shaiya_archive::edit::{apply_in_place, EditPlan, InPlaceOptions, Put, PutMode, Source};
use shaiya_archive::journal::{self, JournalMode};
use shaiya_archive::{encode_name_strict, Archive, ArchiveSource, Index};
use shaiya_core::fs::Existing;
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::source::{ByteSource, FolderSource};
use shaiya_core::Job;
use shaiya_patch::{apply_with, build_with, ApplyOptions, BuildInput, BuildOptions};
use shaiya_seed::batch::{self, BatchOptions, Op, Output, Status};
use shaiya_testkit::TempDir;
use std::path::Path;

use Cell::{Fails as F, Ok as O};

/// A plain ASCII archive path that is safe to name in a plan and in a folder tree: letters, digits and `_ - . /` only, no component
/// ending in a dot.
fn safe_path(p: &str) -> bool {
    !p.is_empty() && p.split('/').all(|s| !s.is_empty() && !s.ends_with('.') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
}

/// Two distinct safe visible paths: one to replace, one to remove.
fn pick_two(idx: &Index) -> Option<(String, String)> {
    let mut it = idx.visible().into_iter().map(|f| f.path).filter(|p| safe_path(p) && encode_name_strict(p).is_ok());
    Some((it.next()?, it.next()?))
}

/// A folder holding a copy of `sah` and an empty `data.saf`.
fn stub_client(root: &Path, sah_bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    let dir = root.join("client");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("data.sah"), sah_bytes).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("data.saf"), b"").map_err(|e| e.to_string())?;
    Ok(dir)
}

fn saf_len(dir: &Path) -> u64 {
    std::fs::metadata(dir.join("data.saf")).map(|m| m.len()).unwrap_or(u64::MAX)
}

/// What a rollback must restore: `data.sah` byte-identical, `data.saf` empty again, no pending marker.
fn restored(dir: &Path, original: &[u8]) -> Result<(), String> {
    let now = std::fs::read(dir.join("data.sah")).map_err(|e| e.to_string())?;
    if now != original {
        return Err("data.sah is not byte-identical after rollback".into());
    }
    if saf_len(dir) != 0 {
        return Err(format!("data.saf is {} bytes after rollback, expected 0", saf_len(dir)));
    }
    if dir.join("data.toolkit-pending").exists() {
        return Err("pending marker left behind".into());
    }
    Ok(())
}

/// In-place add 3 (one with a cp949 name), replace 1, remove 1 on the stub; then rollback. Also reports the free-list entries before and
/// after the edit.
fn edit_rollback(sah_bytes: &[u8], idx: &Index) -> Result<(usize, usize), String> {
    let (replace, remove) = pick_two(idx).ok_or("no two plain visible paths to replace and remove")?;
    let tmp = TempDir::new("matrix-edit");
    let dir = stub_client(tmp.path(), sah_bytes)?;
    let free_before = idx.free.len();
    let put = |path: &str, bytes: &[u8], mode| Put { path: path.to_string(), source: Source::Bytes(bytes.to_vec()), version: 1_700_000_000, mode };
    let plan = EditPlan {
        puts: vec![
            put("interface/t1-matrix-a.txt", b"alpha", PutMode::AddOnly),
            put("interface/t1-\u{b9e4}\u{d2b8}\u{b9ad}\u{c2a4}.txt", b"cp949 name", PutMode::AddOnly),
            put("t1-matrix/deep/c.bin", &[7u8; 5000], PutMode::AddOnly),
            put(&replace, b"replaced content", PutMode::ReplaceOnly),
        ],
        removes: vec![remove.clone()],
        ignore_missing: false,
        loose: Vec::new(),
        label: "matrix-stub".into(),
    };
    let journals = dir.join("shaiya-toolkit-backups");
    let opts = InPlaceOptions { journal: JournalMode::In(journals), legacy_bak: false, check_in_use: true };
    let rep = apply_in_place(&dir, &plan, &opts, &Job::silent()).map_err(|e| format!("apply_in_place: {e}"))?;
    if rep.added.len() != 3 || rep.replaced.len() != 1 || rep.removed.len() != 1 {
        return Err(format!("report: added {}, replaced {}, removed {}", rep.added.len(), rep.replaced.len(), rep.removed.len()));
    }
    let after = Index::parse(&std::fs::read(dir.join("data.sah")).map_err(|e| e.to_string())?).map_err(|e| format!("edited index: {e}"))?;
    if after.version != idx.version {
        return Err(format!("SAH version changed from {} to {}", idx.version, after.version));
    }
    if after.get("interface/t1-matrix-a.txt").is_none() || after.get("t1-matrix/deep/c.bin").is_none() {
        return Err("an added file is missing from the edited index".into());
    }
    let cp = after.get("interface/t1-\u{b9e4}\u{d2b8}\u{b9ad}\u{c2a4}.txt").ok_or("the cp949-named file is missing from the edited index")?;
    if cp.length != 10 {
        return Err("the cp949-named file has the wrong length".into());
    }
    if after.get(&replace).map(|e| e.length) != Some(16) || after.get(&remove).is_some() {
        return Err("replace or remove did not take effect".into());
    }
    if saf_len(&dir) != 5 + 10 + 5000 + 16 {
        return Err(format!("data.saf is {} bytes after the edit", saf_len(&dir)));
    }
    let free_after = after.free.len();
    let journal = rep.journal.ok_or("no journal folder in the report")?;
    journal::rollback(&dir, &journal, false, &Job::silent()).map_err(|e| format!("rollback: {e}"))?;
    restored(&dir, sah_bytes)?;
    Ok((free_before, free_after))
}

/// The same through a synthetic patch with a loose file: apply with a journal, then rollback.
fn patch_rollback(sah_bytes: &[u8], idx: &Index) -> Result<(), String> {
    let (replace, remove) = pick_two(idx).ok_or("no two plain visible paths to replace and remove")?;
    let tmp = TempDir::new("matrix-patch");
    let dir = stub_client(tmp.path(), sah_bytes)?;
    let loose_src = tmp.path().join("loose-src.txt");
    std::fs::write(&loose_src, b"loose file").map_err(|e| e.to_string())?;
    let input = BuildInput {
        files: vec![
            ("interface/t1-matrix-patch.txt".to_string(), Source::Bytes(b"added by patch".to_vec()), 1_700_000_000),
            ("interface/t1-\u{d328}\u{ce58}.txt".to_string(), Source::Bytes(b"cp949".to_vec()), 1_700_000_000),
            (replace.clone(), Source::Bytes(b"patched".to_vec()), 1_700_000_000),
        ],
        loose: vec![("t1-matrix-loose.txt".to_string(), loose_src)],
        removals: vec![remove.clone()],
    };
    let patch = tmp.path().join("ps9999.patch");
    build_with(&input, &patch, &BuildOptions { existing: Existing::Refuse, sign: None }, &Job::silent()).map_err(|e| format!("build: {e}"))?;
    let opts = ApplyOptions { protect: Vec::new(), journal: JournalMode::In(dir.join("shaiya-toolkit-backups")), legacy_bak: false, require_signature: None, check_in_use: true };
    let rep = apply_with(&patch, &dir, &opts, &Job::silent()).map_err(|e| format!("apply: {e}"))?;
    let after = Index::parse(&std::fs::read(dir.join("data.sah")).map_err(|e| e.to_string())?).map_err(|e| format!("patched index: {e}"))?;
    if after.version != idx.version || after.get("interface/t1-matrix-patch.txt").is_none() || after.get(&remove).is_some() || after.get(&replace).map(|e| e.length) != Some(7) {
        return Err("the patched index is not as the patch says".into());
    }
    if std::fs::read(dir.join("t1-matrix-loose.txt")).ok().as_deref() != Some(b"loose file".as_slice()) {
        return Err("the loose file was not written".into());
    }
    let journal = rep.journal.ok_or("no journal folder in the report")?;
    journal::rollback(&dir, &journal, false, &Job::silent()).map_err(|e| format!("rollback: {e}"))?;
    restored(&dir, sah_bytes)?;
    if dir.join("t1-matrix-loose.txt").exists() {
        return Err("the loose file is still there after rollback".into());
    }
    Ok(())
}

fn sdata_filter() -> Filter {
    Filter { include: vec![Pattern::new("*.sdata").expect("pattern")], ..Filter::default() }
}

/// `Check` over the SData files, then decrypt and encrypt `--like` the originals: byte-identical.
fn sdata_caps(o: &mut Observed, src: &dyn ByteSource) {
    let names = src.names();
    if names.is_empty() {
        return;
    }
    let job = Job::silent();
    let check = BatchOptions { op: Op::Check, output: Output::None, existing: Existing::Refuse, pass_through: false, dry_run: false };
    match batch::run(src, &check, &job) {
        Ok(items) => {
            let bad: Vec<String> = items.iter().filter(|i| matches!(i.status, Status::Failed | Status::CrcBad)).map(|i| format!("{}: {:?} {}", i.name, i.status, i.message.clone().unwrap_or_default())).collect();
            let encrypted = items.iter().filter(|i| i.status == Status::CrcOk).count();
            let plain = items.iter().filter(|i| i.status == Status::Plain).count();
            o.set(Cap::SdataBatchCheck, if !bad.is_empty() { F } else if encrypted == 0 && plain > 0 { Cell::Plain } else { O });
            for b in bad.iter().take(10) {
                o.notes.push(format!("sdata batch check: {b}"));
            }
            o.facts.push(format!("sdata_batch_check: {} files, {encrypted} CRC OK, {plain} plain, {} bad", items.len(), bad.len()));
        }
        Err(e) => {
            o.set(Cap::SdataBatchCheck, F);
            o.notes.push(format!("sdata batch check: {e}"));
        }
    }
    match like_roundtrip(src) {
        Ok(n) => {
            o.set(Cap::SdataLikeRoundtrip, O);
            o.facts.push(format!("sdata_like_roundtrip: {n} files byte-identical"));
        }
        Err(e) => {
            o.set(Cap::SdataLikeRoundtrip, F);
            o.notes.push(format!("sdata like round trip: {e}"));
        }
    }
}

fn like_roundtrip(src: &dyn ByteSource) -> Result<usize, String> {
    let tmp = TempDir::new("matrix-sdata");
    let (plain_dir, enc_dir) = (tmp.path().join("plain"), tmp.path().join("enc"));
    let job = Job::silent();
    let dec = BatchOptions { op: Op::Decrypt { unchecked: false }, output: Output::Dir(plain_dir.clone()), existing: Existing::Refuse, pass_through: true, dry_run: false };
    let items = batch::run(src, &dec, &job).map_err(|e| format!("decrypt: {e}"))?;
    if let Some(f) = items.iter().find(|i| i.status == Status::Failed) {
        return Err(format!("decrypt {}: {}", f.name, f.message.clone().unwrap_or_default()));
    }
    let plains = FolderSource::new(&[plain_dir], &Filter::default()).map_err(|e| e.to_string())?;
    let enc = BatchOptions { op: Op::Encrypt { like: Some(src) }, output: Output::Dir(enc_dir.clone()), existing: Existing::Refuse, pass_through: true, dry_run: false };
    let items = batch::run(&plains, &enc, &job).map_err(|e| format!("encrypt: {e}"))?;
    if let Some(f) = items.iter().find(|i| i.status == Status::Failed) {
        return Err(format!("encrypt {}: {}", f.name, f.message.clone().unwrap_or_default()));
    }
    let names = src.names();
    for name in &names {
        let want = src.read(name).map_err(|e| e.to_string())?;
        let got = std::fs::read(name.split('/').fold(enc_dir.clone(), |p, s| p.join(s))).map_err(|e| format!("{name}: {e}"))?;
        if want != got {
            return Err(format!("{name}: re-encrypted bytes differ from the original ({} vs {} bytes)", got.len(), want.len()));
        }
    }
    Ok(names.len())
}

/// Index-only capabilities and the stub tests, for a client whose `data.sah` parsed as `idx` from `raw`.
pub(crate) fn index_caps(o: &mut Observed, idx: &Index, raw: &[u8]) {
    // visible count (files minus shadowed)
    o.set(Cap::ArchiveVisibleCount, O);
    o.numbers.push((Cap::ArchiveVisibleCount, idx.visible().len() as u64));

    // index-only check: no problems; the shadowed warnings are the number
    let rep = check_index(idx, None);
    o.set(Cap::ArchiveCheckIndex, if rep.problems() == 0 { O } else { F });
    o.numbers.push((Cap::ArchiveCheckIndex, rep.count("shadowed") as u64));
    for sev in [Severity::Problem, Severity::Warning, Severity::Info] {
        let mut codes: Vec<(&str, usize)> = Vec::new();
        for f in rep.findings.iter().filter(|f| f.severity == sev) {
            match codes.iter_mut().find(|c| c.0 == f.code) {
                Some(c) => c.1 += 1,
                None => codes.push((f.code, 1)),
            }
        }
        if !codes.is_empty() {
            o.facts.push(format!("check_index {sev:?}: {}", codes.iter().map(|(c, n)| format!("{c} x{n}")).collect::<Vec<_>>().join(", ")));
        }
    }
    for f in rep.findings.iter().filter(|f| f.severity == Severity::Problem).take(5) {
        o.notes.push(format!("check_index problem {}: {}", f.code, f.message));
    }

    // data end in u64
    let end = idx.data_end();
    o.set(Cap::ArchiveDataEnd, if end > 0 && rep.layout.data_end == end { O } else { F });
    o.numbers.push((Cap::ArchiveDataEnd, end));

    // diff with itself
    let d = diff(&Side::Index(idx), &Side::Index(idx), &DiffOptions::default(), &Job::silent());
    match d {
        Ok(entries) if !entries.is_empty() && entries.iter().all(|e| e.change == Change::Same) => o.set(Cap::ArchiveDiffSelf, O),
        Ok(entries) => {
            o.set(Cap::ArchiveDiffSelf, F);
            o.notes.push(format!("diff with itself: {} entries, {} not Same", entries.len(), entries.iter().filter(|e| e.change != Change::Same).count()));
        }
        Err(e) => {
            o.set(Cap::ArchiveDiffSelf, F);
            o.notes.push(format!("diff with itself: {e}"));
        }
    }

    // in-place edit and rollback on a stub (also the free-list cell)
    o.numbers.push((Cap::ArchiveFreeList, idx.free.len() as u64));
    match edit_rollback(raw, idx) {
        Ok((before, after)) => {
            o.set(Cap::ArchiveEditRollbackStub, O);
            if before == after {
                o.set(Cap::ArchiveFreeList, O);
            } else {
                o.set(Cap::ArchiveFreeList, F);
                o.notes.push(format!("free list: {before} entries before the edit, {after} after"));
            }
        }
        Err(e) => {
            o.set(Cap::ArchiveEditRollbackStub, F);
            o.set(Cap::ArchiveFreeList, F);
            o.notes.push(format!("edit/rollback on a stub: {e}"));
        }
    }

    // a synthetic patch with a loose file, apply and rollback
    match patch_rollback(raw, idx) {
        Ok(()) => o.set(Cap::PatchApplyRollbackStub, O),
        Err(e) => {
            o.set(Cap::PatchApplyRollbackStub, F);
            o.notes.push(format!("patch apply/rollback on a stub: {e}"));
        }
    }
}

/// The SData capabilities for a client folder: from the archive when `data.saf` is there, else from `files/`.
pub(crate) fn sdata_caps_for(o: &mut Observed, dir: &Path, has_saf: bool) {
    let filter = sdata_filter();
    if has_saf {
        match Archive::open(dir) {
            Ok(a) => {
                let src = ArchiveSource { archive: &a, filter, include_shadowed: false };
                sdata_caps(o, &src);
            }
            Err(e) => o.notes.push(format!("SData batch: {e}")),
        }
    } else if dir.join("files").is_dir() {
        match FolderSource::new(&[dir.join("files")], &filter) {
            Ok(src) => sdata_caps(o, &src),
            Err(e) => o.notes.push(format!("SData batch: {e}")),
        }
    }
}
