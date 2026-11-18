//! Client files the editor may write: the staging
//! folder and the verified save of a `.wld`.
//!
//! Nothing here writes into the client folder, `data.sah` or `data.saf`. Client output goes to the **staging folder**
//! (setting `client_out`) as `<out>/data/world/<id>.wld`. When the editor reads a client file it looks at that path first, so a
//! staged file overrides the client's own copy.
//!
//! A save writes `<file>.tmp`, reads it again, parses it and compares it with the model, and only then replaces the file:
//! the replaced file goes to a timestamped copy in the data folder, and to `<file>.bak` beside it once. On any mismatch the
//! file is not replaced and the model stays modified.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use shaiya_clientdata::wld_raw::WldRaw;

use crate::client_rules::ClientRules;

/// `<out>/data/<rel>`: where a staged copy of the client file `rel` (a `world/2.wld` style path) lives.
pub fn staged_path(out: &Path, rel: &str) -> PathBuf {
    let mut p = out.join("data");
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        p.push(part);
    }
    p
}

/// The path as comparable lower-case components. The path is made absolute (Windows rules: `..`, trailing dots and spaces),
/// then its deepest folder that exists is resolved (junctions, symbolic links, `subst` and mapped drives, short names) and the
/// parts that do not exist yet are put back on. So a staging folder that does not exist yet under a link into the client is
/// still seen as inside the client. The `\\?\` and `\\?\UNC\` prefixes of resolved paths are dropped so both forms compare.
fn components(p: &Path) -> Vec<String> {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let resolved = loop {
        if let Ok(r) = std::fs::canonicalize(base) {
            let mut r = r;
            for part in rest.iter().rev() {
                r.push(part);
            }
            break r;
        }
        match (base.parent(), base.file_name()) {
            (Some(up), Some(name)) => {
                rest.push(name.to_os_string());
                base = up;
            }
            _ => break abs.clone(),
        }
    };
    let text = resolved.display().to_string();
    let text = match text.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_string(),
    };
    text.split(['/', '\\']).filter(|s| !s.is_empty()).map(|s| s.to_lowercase()).collect()
}

/// Whether the folder `inner` is `outer` or lies inside it: by the resolved path, and by the identity of the folders that
/// exist, so another name for the same folder (a share such as `\\localhost\D$\...` for `D:\...`) is caught too.
fn is_inside(inner: &Path, outer: &Path) -> bool {
    let (a, b) = (components(inner), components(outer));
    if a.len() >= b.len() && a[..b.len()] == b[..] {
        return true;
    }
    let Some(target) = dir_stamp(outer) else { return false };
    let abs = std::path::absolute(inner).unwrap_or_else(|_| inner.to_path_buf());
    abs.ancestors().any(|d| dir_stamp(d) == Some(target) && dir_names(d) == dir_names(outer))
}

/// Whether the folder `inner` is `outer` or lies inside it (the same test the staging folder check uses, so another name for one folder is caught).
pub fn lies_in(inner: &Path, outer: &Path) -> bool {
    is_inside(inner, outer)
}

/// The identity of a folder: what tells two names of one folder apart from two folders. Volume serial number and file index on
/// Windows, device and inode on unix. `None` when it cannot be read (the folders are then not treated as the same).
fn dir_stamp(p: &Path) -> Option<(u64, u64, u32)> {
    let m = std::fs::metadata(p).ok().filter(|m| m.is_dir())?;
    #[cfg(windows)]
    {
        let _ = m;
        win_identity(p).map(|(v, i)| (v, i, 0))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((m.dev(), m.ino(), 0))
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = m;
        None
    }
}

/// Volume serial number and file index of a folder, from the opened directory handle (`GetFileInformationByHandle`; the std
/// accessors for them are not stable). Two names of one folder (a junction, a share, a short name) give the same pair.
#[cfg(windows)]
#[allow(unsafe_code)]
fn win_identity(p: &Path) -> Option<(u64, u64)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;

    /// `BY_HANDLE_FILE_INFORMATION`: 13 32-bit fields.
    #[repr(C)]
    #[derive(Default)]
    struct ByHandleFileInformation {
        attributes: u32,
        times: [u32; 6],
        volume_serial: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    extern "system" {
        fn GetFileInformationByHandle(file: *mut core::ffi::c_void, info: *mut ByHandleFileInformation) -> i32;
    }
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000; // needed to open a directory
    let f = std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(p).ok()?;
    let mut info = ByHandleFileInformation::default();
    // SAFETY: the handle is open for the whole call (owned by `f`) and `info` is a valid, correctly laid out out-parameter.
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle().cast(), &mut info) };
    (ok != 0).then(|| (u64::from(info.volume_serial), u64::from(info.index_high) << 32 | u64::from(info.index_low)))
}

/// The folder's entry names, lower case and sorted (read only when the stamps agree).
fn dir_names(p: &Path) -> Option<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(p).ok()?.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect();
    names.sort();
    Some(names)
}

/// A staging folder is refused when the staged files could land in the client: the folder is the client folder or inside it,
/// or the client folder is inside `<out>/data`.
pub fn check_out_dir(out: &Path, client: &Path) -> Result<(), String> {
    if is_inside(out, client) {
        return Err(format!("{} is the client folder or inside it. The editor never writes into the client; pick another folder.", out.display()));
    }
    if is_inside(client, &out.join("data")) {
        return Err(format!("The client folder is inside {}. Pick a folder that does not hold the client.", out.join("data").display()));
    }
    if out.exists() && !out.is_dir() {
        return Err(format!("{} is a file, not a folder.", out.display()));
    }
    Ok(())
}

/// What a verified save did.
#[derive(Debug)]
pub struct Saved {
    pub path: PathBuf,
    /// The timestamped copy of the file that was replaced (in the data folder), if there was one.
    pub backup: Option<PathBuf>,
    /// The `<file>.bak` made beside the file by this save.
    pub bak_made: bool,
}

/// Where timestamped copies of replaced client files go, and how many stay per file.
#[derive(Clone, Copy)]
pub struct BackupPlan<'a> {
    /// `<data>\backups\client\<key>`.
    pub dir: &'a Path,
    /// `world-2` for `world/2.wld`.
    pub stem: &'a str,
    pub ext: &'a str,
    pub keep: u32,
}

/// The backup folder of a staging folder: `<data>\backups\client\<key>`.
pub fn backup_dir(data: &Path, out: &Path) -> PathBuf {
    data.join("backups").join("client").join(crate::keys::path_key(&out.display().to_string()))
}

/// `world/2.wld` as the pair (`world-2`, `wld`) of the backup names.
pub fn backup_stem(rel: &str) -> (String, String) {
    let (base, ext) = rel.rsplit_once('.').unwrap_or((rel, ""));
    (base.replace(['/', '\\'], "-"), ext.to_string())
}

fn entry_key(stem: &str, ext: &str, name: &str) -> Option<(String, u32)> {
    let rest = name.strip_prefix(&format!("{stem}-"))?.strip_suffix(&format!(".{ext}"))?;
    match rest.split_once('_') {
        Some((s, n)) => Some((s.to_string(), n.parse().ok()?)),
        None => Some((rest.to_string(), 0)),
    }
}

/// Copies `target` into the backup folder under a UTC stamp and keeps the newest `keep`. `Ok(None)` when `keep` is 0.
fn timestamped_backup(plan: &BackupPlan, target: &Path, now: SystemTime) -> Result<Option<PathBuf>, String> {
    if plan.keep == 0 {
        return Ok(None);
    }
    std::fs::create_dir_all(plan.dir).map_err(|e| format!("cannot create {}: {e}", plan.dir.display()))?;
    let stamp = crate::stamp::utc_stamp(now);
    let mut have: Vec<((String, u32), PathBuf)> = std::fs::read_dir(plan.dir)
        .map(|rd| rd.flatten().filter_map(|e| Some((entry_key(plan.stem, plan.ext, &e.file_name().to_string_lossy())?, e.path()))).collect())
        .unwrap_or_default();
    let n = have.iter().filter(|((s, _), _)| *s == stamp).map(|((_, n), _)| n + 1).max().unwrap_or(0);
    let name = if n == 0 { format!("{}-{stamp}.{}", plan.stem, plan.ext) } else { format!("{}-{stamp}_{n}.{}", plan.stem, plan.ext) };
    let dest = plan.dir.join(name);
    std::fs::copy(target, &dest).map_err(|e| format!("cannot write the backup {}: {e}", dest.display()))?;
    have.push(((stamp, n), dest.clone()));
    have.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, old) in have.into_iter().skip(plan.keep as usize) {
        let _ = std::fs::remove_file(old);
    }
    Ok(Some(dest))
}

#[cfg(test)]
type WriteHook = Option<Box<dyn Fn(&Path)>>;
#[cfg(test)]
thread_local! {
    /// Test hook: runs after the temporary file was written and before it is read back (the test damages it).
    static WRITE_HOOK: std::cell::RefCell<WriteHook> = const { std::cell::RefCell::new(None) };
}

/// The safe write: `.tmp`, read back and compared (`verify` gets the bytes read back), the replaced file copied to a
/// timestamped backup and to `.bak` once, then an atomic rename. Any failure leaves `target` as it was and removes the `.tmp`.
/// `forbid` is a folder the file must never land in (the client folder): checked again once the folders exist, so a link
/// planted inside the staging folder cannot lead the write into the client. A read-only target is refused before anything is
/// written.
pub fn write_verified(target: &Path, bytes: &[u8], verify: &dyn Fn(&[u8]) -> Result<(), String>, backup: Option<BackupPlan>, forbid: Option<&Path>) -> Result<Saved, String> {
    let parent = target.parent().ok_or("no folder for the file")?;
    if let Ok(m) = std::fs::metadata(target) {
        if !m.is_file() {
            return Err(format!("{} is not a file; nothing was written.", target.display()));
        }
        if m.permissions().readonly() {
            return Err(format!("{} is read-only, so it was not replaced. Clear its read-only flag and save again.", target.display()));
        }
    }
    if let Some(client) = forbid {
        if is_inside(parent, client) {
            return Err(format!("{} lies inside the client folder; nothing was written.", parent.display()));
        }
    }
    std::fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    if let Some(client) = forbid {
        if is_inside(parent, client) {
            return Err(format!("{} leads into the client folder; nothing was written.", parent.display()));
        }
    }
    let mut tmp_name = target.file_name().ok_or("no file name")?.to_os_string();
    tmp_name.push(".tmp");
    let tmp = target.with_file_name(tmp_name);
    let fail = |msg: String| {
        let _ = std::fs::remove_file(&tmp);
        Err(msg)
    };
    // a stale `.tmp` (or a link of that name) goes first, so the write below never writes through it into another file
    let _ = std::fs::remove_file(&tmp);
    if let Err(e) = std::fs::write(&tmp, bytes) {
        return fail(format!("cannot write {}: {e}", tmp.display()));
    }
    #[cfg(test)]
    WRITE_HOOK.with(|h| {
        if let Some(f) = h.borrow().as_ref() {
            f(&tmp);
        }
    });
    let back = match std::fs::read(&tmp) {
        Ok(b) => b,
        Err(e) => return fail(format!("cannot read {} again: {e}", tmp.display())),
    };
    if back != bytes {
        return fail(format!("{} does not read back as written; {} was not replaced.", tmp.display(), target.display()));
    }
    if let Err(why) = verify(&back) {
        return fail(format!("{} does not read back as the model ({why}); {} was not replaced.", tmp.display(), target.display()));
    }
    let existed = target.is_file();
    let mut backup_path = None;
    if existed {
        if let Some(plan) = backup {
            match timestamped_backup(&plan, target, SystemTime::now()) {
                Ok(p) => backup_path = p,
                Err(e) => return fail(e),
            }
        }
    }
    let mut bak_made = false;
    if existed {
        let mut bak_name = target.file_name().ok_or("no file name")?.to_os_string();
        bak_name.push(".bak");
        let bak = target.with_file_name(bak_name);
        // symlink_metadata: a dangling link named `.bak` counts as there, so the copy never writes through it
        if std::fs::symlink_metadata(&bak).is_err() {
            if let Err(e) = std::fs::copy(target, &bak) {
                return fail(format!("cannot write the backup {}: {e}", bak.display()));
            }
            bak_made = true;
        }
    }
    if let Err(e) = std::fs::rename(&tmp, target) {
        return fail(format!("cannot replace {}: {e}", target.display()));
    }
    Ok(Saved { path: target.to_path_buf(), backup: backup_path, bak_made })
}

/// A map's client `.wld` as the editor holds it: the lossless model, and the bytes it was read from or last saved as. Opening
/// one needs a file that writes back byte for byte; any other file stays read-only (`Err` says why).
pub struct ClientWld {
    pub id: u32,
    pub model: WldRaw,
    saved: Vec<u8>,
    /// The bytes came from the staging folder.
    pub staged: bool,
    /// The client folder the file was read from; a save never writes inside it.
    client_dir: PathBuf,
}

impl ClientWld {
    /// Reads `world/<id>.wld` (the staged copy first) into the model.
    pub fn open(rules: &ClientRules, id: u32) -> Result<ClientWld, String> {
        let path = format!("world/{id}.wld");
        let (bytes, staged) = rules.read_file(&path).map_err(|e| e.to_string())?;
        shaiya_clientdata::wld_raw::lossless_check(&bytes, &path).map_err(|why| format!("{path} stays read-only. The editor cannot write it back byte for byte: {why}."))?;
        let model = WldRaw::parse(&bytes, &path).map_err(|e| e.to_string())?;
        Ok(ClientWld { id, model, saved: bytes, staged, client_dir: rules.source.dir().to_path_buf() })
    }

    /// The client folder the file was read from (the one nothing may be written into).
    pub fn client_dir(&self) -> &Path {
        &self.client_dir
    }

    /// A map that does not exist in the client yet (the new map wizard): the model is saved as it is.
    pub fn new_map(id: u32, model: WldRaw, client_dir: &Path) -> ClientWld {
        ClientWld { id, model, saved: Vec::new(), staged: false, client_dir: client_dir.to_path_buf() }
    }

    /// A model that is already in memory, with no client folder behind it (tests that never save).
    #[cfg(any(test, feature = "testing"))]
    pub fn from_model(id: u32, model: WldRaw) -> ClientWld {
        ClientWld { id, saved: model.to_bytes(), model, staged: false, client_dir: PathBuf::from("no-client-folder") }
    }

    /// The model differs from what was read or last saved. It writes the whole model to compare: about 5 MB for a field map, so
    /// not for every frame.
    pub fn dirty(&self) -> bool {
        self.model.to_bytes() != self.saved
    }

    pub fn rel_path(&self) -> String {
        format!("world/{}.wld", self.id)
    }

    /// Writes the model to `<out>/data/world/<id>.wld`. A model that fails [`WldRaw::validate`] is refused. On any failure the file
    /// is not replaced and the model stays as it is (still modified when it was).
    pub fn save(&mut self, out: &Path, backups: Option<(&Path, u32)>) -> Result<Saved, String> {
        // the caller checked the folder already; checked again here so no path can write into the client
        check_out_dir(out, &self.client_dir)?;
        let bad = self.model.validate();
        if !bad.is_empty() {
            return Err(format!("{} would not read back: {}", self.rel_path(), bad.join("; ")));
        }
        let bytes = self.model.to_bytes();
        let rel = self.rel_path();
        let target = staged_path(out, &rel);
        let (stem, ext) = backup_stem(&rel);
        let plan = backups.map(|(dir, keep)| BackupPlan { dir, stem: &stem, ext: &ext, keep });
        let want = bytes.clone();
        let verify = move |back: &[u8]| -> Result<(), String> {
            let parsed = WldRaw::parse(back, "the written file").map_err(|e| e.to_string())?;
            if parsed.to_bytes() != want {
                return Err("the file parses but differs from the model".to_string());
            }
            Ok(())
        };
        let saved = write_verified(&target, &bytes, &verify, plan, Some(&self.client_dir))?;
        self.saved = bytes;
        self.staged = true;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn tiny_wld() -> Vec<u8> {
        // a hand-made field map through the model's own writer (the editor tests for the real client live in app_tests)
        let mut w = tiny_model();
        w.portals[0].dest_map = 5;
        w.to_bytes()
    }

    fn tiny_model() -> WldRaw {
        crate::testutil::tiny_wld(4)
    }

    /// A tiny client folder (extracted layout) with one map, for the tests that need `ClientRules`.
    fn client_with(dir: &Path, id: u32, bytes: &[u8]) {
        let p = dir.join("world");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join(format!("{id}.wld")), bytes).unwrap();
    }

    #[test]
    fn staging_inside_the_client_is_refused() {
        let t = TempDir::new("staging-check");
        let client = t.join("client");
        std::fs::create_dir_all(client.join("data")).unwrap();
        let out = t.join("out");
        assert!(check_out_dir(&out, &client).is_ok());
        assert!(check_out_dir(&client, &client).unwrap_err().contains("never writes into the client"), "the client folder itself");
        assert!(check_out_dir(&client.join("staged"), &client).is_err(), "a folder inside it");
        assert!(check_out_dir(&client.join("data"), &client).is_err());
        // the client inside the staging data folder
        let weird = t.join("w");
        let inner = weird.join("data").join("client");
        std::fs::create_dir_all(&inner).unwrap();
        assert!(check_out_dir(&weird, &inner).unwrap_err().contains("inside"));
        // a file is not a folder
        std::fs::write(t.join("f"), b"x").unwrap();
        assert!(check_out_dir(&t.join("f"), &client).is_err());
        assert_eq!(staged_path(Path::new("o"), "world/2.wld"), Path::new("o").join("data").join("world").join("2.wld"));
    }

    /// Other spellings of the client folder are refused too: case, trailing separators and dots, `..`, relative paths, the
    /// `\\?\` form, and a junction (made with `mklink /J`) whose staging folder does not exist yet.
    #[test]
    fn staging_aliases_of_the_client_are_refused() {
        let t = TempDir::new("staging-alias");
        let client = t.join("Client");
        std::fs::create_dir_all(client.join("world")).unwrap();
        let s = client.display().to_string();
        let portable = [format!("{s}/"), format!("{s}/world/../new"), format!("{s}/../Client/new/deeper")];
        // spellings that only mean the same folder under Windows path rules
        #[cfg(windows)]
        let windows_only = [
            s.to_uppercase(),
            format!("{s}\\"),
            format!("{s}\\\\new"),
            format!("{s}."),
            format!("{s} "),
            format!("{s}\\world\\..\\new"),
            format!("{s}\\..\\Client\\new\\deeper"),
            format!(r"\\?\{s}\new"),
        ];
        #[cfg(not(windows))]
        let windows_only: [String; 0] = [];
        for spelled in portable.into_iter().chain(windows_only) {
            assert!(check_out_dir(Path::new(&spelled), &client).is_err(), "{spelled}");
        }
        // relative paths are resolved against the working folder (only read here: nothing is written)
        let cwd = std::env::current_dir().unwrap();
        for rel in [".", "src", "src/../new", "./x/y"] {
            assert!(check_out_dir(Path::new(rel), &cwd).is_err(), "{rel}");
        }
        assert!(check_out_dir(Path::new(".."), &cwd.join("src")).is_ok(), "a parent folder whose data folder does not hold it");
        assert!(check_out_dir(&t.join("Client2"), &client).is_ok(), "a sibling with a longer name is fine");
        std::fs::create_dir_all(t.join("Twin").join("world")).unwrap();
        assert!(check_out_dir(&t.join("Twin"), &client).is_ok(), "a folder with the same entries is another folder");
        // the same folder through the administrative share (\\localhost\D$\...), when the share is reachable
        let unc = s.split_once(":\\").map(|(drive, rest)| PathBuf::from(format!(r"\\localhost\{drive}$\{rest}")));
        match unc.filter(|u| u.is_dir()) {
            Some(u) => {
                assert!(check_out_dir(&u, &client).is_err(), "{}", u.display());
                assert!(check_out_dir(&u.join("new").join("deeper"), &client).is_err(), "a folder that does not exist yet under the share");
                let e = write_verified(&u.join("data").join("world").join("2.wld"), b"x", &|_| Ok(()), None, Some(&client)).unwrap_err();
                assert!(e.contains("client folder"), "{e}");
                assert!(!client.join("data").exists(), "nothing landed in the client");
            }
            None => eprintln!("no administrative share; the UNC case was skipped"),
        }
        // a junction into the client, with a staging folder below it that does not exist yet
        let link = t.join("link");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&client).output().is_ok_and(|o| o.status.success());
        if made {
            assert!(check_out_dir(&link, &client).is_err());
            assert!(check_out_dir(&link.join("not").join("yet"), &client).is_err(), "resolved through the junction");
            // a junction planted as `<out>/data/world` is caught at write time, after the folders exist
            let out = t.join("out");
            std::fs::create_dir_all(out.join("data")).unwrap();
            let planted = out.join("data").join("world");
            let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&planted).arg(&client).output().is_ok_and(|o| o.status.success());
            assert!(ok, "second junction");
            assert!(check_out_dir(&out, &client).is_ok(), "the folder itself looks fine");
            let e = write_verified(&planted.join("2.wld"), b"x", &|_| Ok(()), None, Some(&client)).unwrap_err();
            assert!(e.contains("client folder"), "{e}");
            assert!(!client.join("2.wld").exists() && !client.join("2.wld.tmp").exists(), "nothing landed in the client");
            let _ = std::fs::remove_dir(&planted);
            let _ = std::fs::remove_dir(&link);
        } else {
            eprintln!("mklink /J failed; the junction case was skipped");
        }
    }

    /// A read-only target is refused with the reason before anything is written; a stale `.tmp` that is a hard link to
    /// another file is removed, not written through.
    #[test]
    fn read_only_target_and_stale_tmp() {
        let t = TempDir::new("client-save-ro");
        let target = t.join("w").join("2.wld");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"old").unwrap();
        let mut perm = std::fs::metadata(&target).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&target, perm.clone()).unwrap();
        let e = write_verified(&target, b"new", &|_| Ok(()), None, None).unwrap_err();
        assert!(e.contains("read-only"), "{e}");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert!(!target.with_extension("wld.tmp").exists() && !target.with_extension("wld.bak").exists());
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(&target, perm).unwrap();
        // a stale .tmp hard-linked to a precious file
        let precious = t.join("precious.bin");
        std::fs::write(&precious, b"keep me").unwrap();
        if let Err(e) = std::fs::hard_link(&precious, target.with_extension("wld.tmp")) {
            // only a file system without hard links may skip the case; any other failure is a real one
            let no_links = e.kind() == std::io::ErrorKind::Unsupported || matches!(e.raw_os_error(), Some(1 | 38 | 50 | 95 | 124));
            assert!(no_links, "hard_link failed for a reason other than missing hard-link support: {e}");
            eprintln!("skipped: this file system has no hard links, the linked-tmp case is not run");
            return;
        }
        write_verified(&target, b"new", &|_| Ok(()), None, None).unwrap();
        assert_eq!(std::fs::read(&precious).unwrap(), b"keep me", "the linked file is untouched");
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(target.with_extension("wld.bak")).unwrap(), b"old");
        assert!(!target.with_extension("wld.tmp").exists());
    }

    /// A staged file overrides the client's own copy for the wld, its decoded form, its state and the area names; clearing
    /// the staging folder brings the client's copy back.
    #[test]
    fn staged_wld_overrides_source() {
        let t = TempDir::new("staged-override");
        let client = t.join("client");
        let out = t.join("out");
        client_with(&client, 7, &tiny_wld());
        let rules = ClientRules::open(&client).unwrap();
        rules.set_staging(Some(out.clone()));
        let st = rules.wld_state(7);
        assert!(!st.staged && st.read_only.is_none());
        assert_eq!(rules.wld(7).unwrap().portals[0].dest_map, 5);
        // stage a version with the portal's destination changed and a name file
        let mut m = tiny_model();
        m.portals[0].dest_map = 9;
        let p = staged_path(&out, "world/7.wld");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, m.to_bytes()).unwrap();
        std::fs::write(staged_path(&out, "world/7.txt"), [0xff, 0xfe, b'0', 0, 9, 0, b'X', 0]).unwrap();
        rules.set_staging(Some(out.clone()));
        let st = rules.wld_state(7);
        assert!(st.staged && st.read_only.is_none(), "{st:?}");
        assert_eq!(rules.wld(7).unwrap().portals[0].dest_map, 9, "the staged file wins");
        assert_eq!(rules.area_names(7).unwrap().get(0), Some("X"));
        // the client's own folder is untouched and comes back without the staging folder
        assert_eq!(std::fs::read(client.join("world").join("7.wld")).unwrap(), tiny_wld());
        rules.set_staging(None);
        assert_eq!(rules.wld(7).unwrap().portals[0].dest_map, 5);
        assert!(!rules.wld_state(7).staged);
        assert!(rules.area_names(7).unwrap().is_empty());
    }

    /// A file that does not write back stays read-only with the reason, and the editor says so in Problems.
    #[test]
    fn not_lossless_wld_is_read_only() {
        let t = TempDir::new("not-lossless");
        let client = t.join("client");
        client_with(&client, 3, &crate::testutil::not_lossless_wld(4));
        let rules = ClientRules::open(&client).unwrap();
        let st = rules.wld_state(3);
        assert!(st.read_only.as_deref().is_some_and(|r| r.contains("first difference")), "{st:?}");
        let e = ClientWld::open(&rules, 3).err().expect("read-only");
        assert!(e.contains("stays read-only") && e.contains("first difference"), "{e}");
        let f = rules.check(Some(3), &shaiya_svmap::Svmap::new(4));
        assert!(f.iter().any(|f| f.code == "wld-not-lossless" && f.severity == shaiya_svmap::Severity::Info), "{f:?}");
        // a good map has neither
        client_with(&client, 4, &tiny_wld());
        assert!(rules.wld_state(4).read_only.is_none());
        assert!(!rules.check(Some(4), &shaiya_svmap::Svmap::new(4)).iter().any(|f| f.code == "wld-not-lossless"));
    }

    /// The safe save: a good save replaces the file and keeps both backups; a read-back that differs leaves the file alone,
    /// removes the `.tmp` and keeps the model modified.
    #[test]
    fn save_verified_mismatch_keeps_dirty() {
        let t = TempDir::new("client-save");
        let client = t.join("client");
        let out = t.join("out");
        let data = t.join("data");
        client_with(&client, 7, &tiny_wld());
        let rules = ClientRules::open(&client).unwrap();
        let mut w = ClientWld::open(&rules, 7).unwrap();
        assert!(!w.dirty() && !w.staged);
        let target = staged_path(&out, "world/7.wld");
        let bdir = backup_dir(&data, &out);
        // first save: nothing to replace, so no backup yet
        w.model.portals[0].min[0] += 5.0;
        w.model.portals[0].max[0] += 5.0;
        assert!(w.dirty());
        let s = w.save(&out, Some((&bdir, 5))).unwrap();
        assert_eq!(s.path, target);
        assert!(s.backup.is_none() && !s.bak_made && !w.dirty());
        let first = std::fs::read(&target).unwrap();
        assert_eq!(shaiya_clientdata::wld::Wld::parse(&first, "x").unwrap().portals[0].min[0], 15.0);
        assert!(!target.with_extension("wld.tmp").exists());
        // a second save replaces it, with both backups of the first version
        w.model.portals[0].dest_map = 11;
        let s = w.save(&out, Some((&bdir, 5))).unwrap();
        assert!(s.bak_made && target.with_extension("wld.bak").is_file());
        assert_eq!(std::fs::read(s.backup.as_ref().unwrap()).unwrap(), first, "the timestamped copy is the replaced file");
        assert_eq!(std::fs::read(target.with_extension("wld.bak")).unwrap(), first);
        let good = std::fs::read(&target).unwrap();
        // a third save whose tmp file is damaged after the write
        w.model.portals[0].dest_map = 12;
        WRITE_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(|p: &Path| {
                let mut b = std::fs::read(p).unwrap();
                let n = b.len();
                b[n - 20] ^= 1;
                std::fs::write(p, b).unwrap();
            }));
        });
        let e = w.save(&out, Some((&bdir, 5))).unwrap_err();
        WRITE_HOOK.with(|h| *h.borrow_mut() = None);
        assert!(e.contains("does not read back") && e.contains("was not replaced"), "{e}");
        assert!(w.dirty(), "the model stays modified");
        assert_eq!(std::fs::read(&target).unwrap(), good, "the file is not replaced");
        assert!(!target.with_extension("wld.tmp").exists(), "no stray .tmp");
        // without the hook the next save works
        w.save(&out, Some((&bdir, 5))).unwrap();
        assert!(!w.dirty());
        assert_ne!(std::fs::read(&target).unwrap(), good);
        // the client's own file was never touched
        assert_eq!(std::fs::read(client.join("world").join("7.wld")).unwrap(), tiny_wld());
        assert!(!client.join("world").join("7.wld.bak").exists() && !client.join("world").join("7.wld.tmp").exists());
        // opening again reads the staged copy
        let again = ClientWld::open(&rules, 7);
        assert!(again.is_ok());
        rules.set_staging(Some(out.clone()));
        assert!(ClientWld::open(&rules, 7).unwrap().staged);
    }

    #[test]
    fn invalid_models_are_refused_before_any_write() {
        let t = TempDir::new("client-save-bad");
        let client = t.join("client");
        let out = t.join("out");
        client_with(&client, 7, &tiny_wld());
        let rules = ClientRules::open(&client).unwrap();
        let mut w = ClientWld::open(&rules, 7).unwrap();
        w.model.zone_groups.push(Default::default());
        let e = w.save(&out, None).unwrap_err();
        assert!(e.contains("bind group 0 has no box"), "{e}");
        assert!(!out.exists(), "nothing was written");
    }

    #[test]
    fn backups_keep_the_newest_and_count_up_within_a_second() {
        let t = TempDir::new("client-backups");
        let file = t.join("f.wld");
        let dir = t.join("b");
        let plan = BackupPlan { dir: &dir, stem: "world-1", ext: "wld", keep: 2 };
        let now = SystemTime::now();
        std::fs::write(&file, b"one").unwrap();
        let a = timestamped_backup(&plan, &file, now).unwrap().unwrap();
        let b = timestamped_backup(&plan, &file, now).unwrap().unwrap();
        assert_ne!(a, b, "a second copy in the same second gets a counter");
        let c = timestamped_backup(&plan, &file, now).unwrap().unwrap();
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(left.len(), 2, "only the newest two stay");
        assert!(c.is_file() && b.is_file() && !a.is_file());
        assert!(timestamped_backup(&BackupPlan { keep: 0, ..plan }, &file, now).unwrap().is_none());
        assert_eq!(backup_stem("world/2.wld"), ("world-2".to_string(), "wld".to_string()));
    }
}
