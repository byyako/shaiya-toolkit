//! Every write in the toolkit goes through here: atomic replace, backups, and the one function that applies the output rules.

use crate::error::{Error, Result};
use crate::job::Job;
use crate::time::utc_stamp;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const TEMP_SUFFIX: &str = ".shaiya-tmp";

/// What to do when an output file already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    Refuse,
    Skip,
    Overwrite,
}

/// Whether a write is followed by fsync.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sync {
    Yes,
    No,
}

/// A file or folder name every OS accepts: characters Windows forbids (`<>:"|?*`, `/`, `\` and controls), trailing dots or spaces and
/// reserved device names (CON, PRN, AUX, NUL, COM1..9, LPT1..9, any case, with or without extension) are replaced by `%XX` (UTF-8
/// bytes); `None` when the name is fine as it is.
pub fn portable_name(name: &str) -> Option<String> {
    fn pct(out: &mut String, c: char) {
        let mut b = [0u8; 4];
        for x in c.encode_utf8(&mut b).bytes() {
            out.push_str(&format!("%{x:02X}"));
        }
    }
    if name.is_empty() {
        return Some("%5F".to_string());
    }
    let mut chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    let mut changed = false;
    let last = chars.len() - 1;
    let reserved = {
        let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit() && stem.as_bytes()[3] != b'0')
    };
    for (i, c) in chars.drain(..).enumerate() {
        let bad = (c as u32) < 0x20 || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\') || (i == last && (c == '.' || c == ' '));
        if bad || (reserved && i == 0) {
            pct(&mut out, c);
            changed = true;
        } else {
            out.push(c);
        }
    }
    changed.then_some(out)
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "out".into());
    let tmp = format!(".{name}.{}{TEMP_SUFFIX}", std::process::id());
    path.with_file_name(tmp)
}

/// Write to `.<name>.<pid>.shaiya-tmp` in the same folder, fsync, rename over `path`.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = temp_path(path);
    let result = (|| -> std::io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(path, e));
    }
    Ok(())
}

/// Streams `src` into a temp file next to `path` (1 MiB buffer, `job.check()` between chunks, byte progress), then renames it over
/// `path`. A cancelled or failed run removes the temp file and leaves `path` as it was. Returns the bytes written.
pub fn write_atomic_from(path: &Path, src: &mut dyn Read, job: &Job) -> Result<u64> {
    write_atomic_from_with(path, src, Sync::Yes, job)
}

/// [`write_atomic_from`] with a choice of fsync. Archive pairs, indexes, patches, journals, markers and keys use `Sync::Yes`; bulk
/// outputs of many files (extract, SData batch) use `Sync::No`: the temp + rename still means no half-written target.
pub fn write_atomic_from_with(path: &Path, src: &mut dyn Read, sync: Sync, job: &Job) -> Result<u64> {
    let tmp = temp_path(path);
    let result = (|| -> Result<u64> {
        let mut f = File::create(&tmp).map_err(|e| Error::io(&tmp, e))?;
        let mut buf = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            job.check()?;
            let n = src.read(&mut buf).map_err(|e| Error::io(path, e))?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n]).map_err(|e| Error::io(&tmp, e))?;
            total += n as u64;
            job.advance(n as u64);
        }
        if sync == Sync::Yes {
            f.sync_all().map_err(|e| Error::io(&tmp, e))?;
        }
        drop(f);
        std::fs::rename(&tmp, path).map_err(|e| Error::io(path, e))?;
        Ok(total)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn backup_candidate(path: &Path, n: u32) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let tail = match n {
        0 => ".bak".to_string(),
        1 => format!(".{}.bak", utc_stamp(SystemTime::now())),
        n => format!(".{}-{}.bak", utc_stamp(SystemTime::now()), n - 1),
    };
    path.with_file_name(format!("{name}{tail}"))
}

/// The first backup path that does not exist yet (`<file>.bak`, then `<file>.<UTC stamp>.bak`, then with a counter).
fn free_backup_path(path: &Path) -> PathBuf {
    let mut n = 0;
    loop {
        let c = backup_candidate(path, n);
        if !c.exists() {
            return c;
        }
        n += 1;
    }
}

/// `<file>.bak` when free, else `<file>.<UTC yyyymmddThhmmssZ>.bak`; never overwrites an existing backup. Returns the backup path.
pub fn make_backup(path: &Path) -> Result<PathBuf> {
    let mut src = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut n = 0;
    loop {
        let dest = backup_candidate(path, n);
        match OpenOptions::new().write(true).create_new(true).open(&dest) {
            Ok(mut out) => {
                let copied = std::io::copy(&mut src, &mut out).and_then(|_| out.sync_all());
                if let Err(e) = copied {
                    drop(out);
                    let _ = std::fs::remove_file(&dest);
                    return Err(Error::io(&dest, e));
                }
                return Ok(dest);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(Error::io(&dest, e)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    NewFile,
    Overwrite,
    InPlaceWithBackup,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPlan {
    pub mode: WriteMode,
    pub target: PathBuf,
    /// Where `make_backup` will put the input (only for `InPlaceWithBackup`).
    pub backup: Option<PathBuf>,
}

/// The one place that applies the write rules (spec section 8): `out` given -> NewFile (Overwrite with `force`); `out` None and
/// `in_place` -> InPlaceWithBackup; neither -> `Error::Refused`. `out` equal to `input` without `in_place` -> Refused. `out` naming a
/// different file together with `in_place` is ambiguous and refused.
pub fn plan_output(input: &Path, out: Option<&Path>, in_place: bool, force: bool) -> Result<OutputPlan> {
    match out {
        Some(o) => {
            let same = same_file(input, o);
            if same && !in_place {
                return Err(Error::Refused(format!("the output {} is the input file; use --in-place to change it (a backup is made)", o.display())));
            }
            if in_place && !same {
                return Err(Error::Refused("use either --out or --in-place, not both".into()));
            }
            if same {
                return Ok(OutputPlan { mode: WriteMode::InPlaceWithBackup, target: input.to_path_buf(), backup: Some(free_backup_path(input)) });
            }
            if o.exists() {
                if !force {
                    return Err(Error::Refused(format!("{} already exists; use --force to overwrite it", o.display())));
                }
                return Ok(OutputPlan { mode: WriteMode::Overwrite, target: o.to_path_buf(), backup: None });
            }
            Ok(OutputPlan { mode: WriteMode::NewFile, target: o.to_path_buf(), backup: None })
        }
        None if in_place => {
            Ok(OutputPlan { mode: WriteMode::InPlaceWithBackup, target: input.to_path_buf(), backup: Some(free_backup_path(input)) })
        }
        None => Err(Error::Refused("this would change a file: give an output path with --out, or --in-place to change the input (a backup is made)".into())),
    }
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// True when both paths name the same existing file (or, when one does not exist, the same absolute path).
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => absolute(a) == absolute(b),
    }
}

/// Removes this toolkit's `*.shaiya-tmp` files older than 24 h in `dir`.
pub fn cleanup_stale_temps(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !(name.starts_with('.') && name.ends_with(TEMP_SUFFIX)) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|d| d > Duration::from_secs(24 * 3600));
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{Cancel, Silent};

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shaiya-core-fs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let d = scratch("atomic");
        let p = d.join("a.bin");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two!").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two!");
        let names: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["a.bin"]);
        assert!(write_atomic(&d.join("no-such-dir").join("x"), b"x").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn atomic_from_streams_and_cancel_leaves_target() {
        let d = scratch("from");
        let p = d.join("b.bin");
        write_atomic(&p, b"old").unwrap();
        let data = vec![7u8; 2_500_000];
        let n = write_atomic_from(&p, &mut &data[..], &Job::silent()).unwrap();
        assert_eq!(n, 2_500_000);
        assert_eq!(std::fs::read(&p).unwrap(), data);
        let c = Cancel::new();
        c.cancel();
        let r = write_atomic_from(&p, &mut &b"new"[..], &Job::new(&Silent, c));
        assert!(matches!(r, Err(Error::Cancelled)));
        assert_eq!(std::fs::read(&p).unwrap(), data, "a cancelled write must leave the target alone");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "temp file removed");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn backup_names_never_overwrite() {
        let d = scratch("backup");
        let p = d.join("data.sah");
        std::fs::write(&p, b"v1").unwrap();
        let b1 = make_backup(&p).unwrap();
        assert_eq!(b1, d.join("data.sah.bak"));
        std::fs::write(&p, b"v2").unwrap();
        let b2 = make_backup(&p).unwrap();
        let n2 = b2.file_name().unwrap().to_string_lossy().into_owned();
        assert!(n2.starts_with("data.sah.") && n2.ends_with("Z.bak") && n2.len() == "data.sah.20261115T120000Z.bak".len(), "{n2}");
        let b3 = make_backup(&p).unwrap();
        assert_ne!(b3, b2);
        assert_eq!(std::fs::read(&b1).unwrap(), b"v1");
        assert_eq!(std::fs::read(&b2).unwrap(), b"v2");
        assert!(make_backup(&d.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn plan_output_rules() {
        let d = scratch("plan");
        let input = d.join("in.bin");
        std::fs::write(&input, b"x").unwrap();
        let existing = d.join("exists.bin");
        std::fs::write(&existing, b"y").unwrap();
        let fresh = d.join("fresh.bin");

        // out given
        let p = plan_output(&input, Some(&fresh), false, false).unwrap();
        assert_eq!((p.mode, p.target.clone(), p.backup.clone()), (WriteMode::NewFile, fresh.clone(), None));
        let r = plan_output(&input, Some(&existing), false, false);
        assert!(matches!(r, Err(Error::Refused(_))));
        let p = plan_output(&input, Some(&existing), false, true).unwrap();
        assert_eq!(p.mode, WriteMode::Overwrite);
        // out equals input
        assert!(matches!(plan_output(&input, Some(&input), false, true), Err(Error::Refused(_))));
        // in place
        let p = plan_output(&input, None, true, false).unwrap();
        assert_eq!(p.mode, WriteMode::InPlaceWithBackup);
        assert_eq!(p.target, input);
        assert_eq!(p.backup, Some(d.join("in.bin.bak")));
        let p = plan_output(&input, Some(&input), true, false).unwrap();
        assert_eq!(p.mode, WriteMode::InPlaceWithBackup);
        // neither
        assert!(matches!(plan_output(&input, None, false, false), Err(Error::Refused(_))));
        assert!(matches!(plan_output(&input, None, false, true), Err(Error::Refused(_))));
        // both naming different files
        assert!(matches!(plan_output(&input, Some(&fresh), true, false), Err(Error::Refused(_))));
        // the planned backup name is free, and becomes taken once made
        std::fs::write(d.join("in.bin.bak"), b"old").unwrap();
        let p = plan_output(&input, None, true, false).unwrap();
        assert_ne!(p.backup, Some(d.join("in.bin.bak")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn portable_names() {
        assert_eq!(portable_name("Item.SData"), None);
        assert_eq!(portable_name("10051\u{C6D0}.3do"), None);
        assert_eq!(portable_name("a:b"), Some("a%3Ab".into()));
        for (bad, want) in [("<", "%3C"), (">", "%3E"), ("\"", "%22"), ("|", "%7C"), ("?", "%3F"), ("*", "%2A"), ("\u{1}", "%01")] {
            assert_eq!(portable_name(&format!("x{bad}y")), Some(format!("x{want}y")), "{bad:?}");
        }
        assert_eq!(portable_name("CON.txt"), Some("%43ON.txt".into()));
        assert_eq!(portable_name("nul"), Some("%6Eul".into()));
        assert_eq!(portable_name("Com3"), Some("%43om3".into()));
        assert_eq!(portable_name("lpt9.x.y"), Some("%6Cpt9.x.y".into()));
        assert_eq!(portable_name("COM0"), None);
        assert_eq!(portable_name("CONSOLE"), None);
        assert_eq!(portable_name("name."), Some("name%2E".into()));
        assert_eq!(portable_name("name "), Some("name%20".into()));
        assert_eq!(portable_name(".."), Some(".%2E".into()));
        assert_eq!(portable_name("\u{BAAC}:"), Some("\u{BAAC}%3A".into()));
        assert!(portable_name("").is_some());
    }

    #[test]
    fn atomic_from_without_sync_still_renames() {
        let d = scratch("nosync");
        let p = d.join("c.bin");
        let n = write_atomic_from_with(&p, &mut &b"abc"[..], Sync::No, &Job::silent()).unwrap();
        assert_eq!((n, std::fs::read(&p).unwrap()), (3, b"abc".to_vec()));
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn same_file_sees_through_paths() {
        let d = scratch("same");
        let p = d.join("a.txt");
        std::fs::write(&p, b"x").unwrap();
        assert!(same_file(&p, &d.join(".").join("a.txt")));
        assert!(!same_file(&p, &d.join("b.txt")));
        assert!(same_file(&d.join("nope"), &d.join("nope")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cleanup_only_touches_old_temp_files() {
        let d = scratch("stale");
        let fresh_tmp = d.join(".a.1.shaiya-tmp");
        let old_tmp = d.join(".b.2.shaiya-tmp");
        let other = d.join("keep.shaiya-tmp.txt");
        for p in [&fresh_tmp, &old_tmp, &other] {
            std::fs::write(p, b"x").unwrap();
        }
        let old = SystemTime::now() - Duration::from_secs(48 * 3600);
        File::options().write(true).open(&old_tmp).unwrap().set_modified(old).unwrap();
        cleanup_stale_temps(&d);
        assert!(fresh_tmp.exists() && other.exists());
        assert!(!old_tmp.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
