//! Putting another archive pair in place of a client's own, crash-safe through `data.swap-journal` (spec section 5.5).

use crate::client::ClientFiles;
use crate::{layout, saf_of, Index};
use shaiya_core::fs::{same_file, write_atomic};
use shaiya_core::time::utc_stamp;
use shaiya_core::{Error, Job, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const JOURNAL: &str = "data.swap-journal";

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SwapReport {
    pub installed_from: PathBuf,
    pub kept_as: (PathBuf, PathBuf),
}

/// The four planned renames, in order: `(from, to)`.
pub fn plan_renames(client: &Path, other: &Path) -> Result<Vec<(PathBuf, PathBuf)>> {
    let files = ClientFiles::locate(client)?;
    let (o_sah, o_saf) = resolve_other(other)?;
    let stamp = utc_stamp(SystemTime::now());
    let kept = |ext: &str, n: u32| {
        let name = if n == 0 { format!("data.{stamp}.{ext}") } else { format!("data.{stamp}-{n}.{ext}") };
        client.join(name)
    };
    let mut n = 0;
    while kept("sah", n).exists() || kept("saf", n).exists() {
        n += 1;
    }
    Ok(vec![(files.sah.clone(), kept("sah", n)), (files.saf.clone(), kept("saf", n)), (o_sah, files.sah), (o_saf, files.saf)])
}

fn resolve_other(other: &Path) -> Result<(PathBuf, PathBuf)> {
    if other.is_dir() {
        let f = ClientFiles::locate(other)?;
        Ok((f.sah, f.saf))
    } else {
        let saf = saf_of(other);
        if !saf.is_file() {
            return Err(Error::io(&saf, std::io::Error::new(std::io::ErrorKind::NotFound, "the matching .saf file is missing")));
        }
        Ok((other.to_path_buf(), saf))
    }
}

#[cfg(unix)]
fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_volume(a: &Path, b: &Path) -> bool {
    fn root(p: &Path) -> Option<String> {
        let c = std::fs::canonicalize(p).ok()?;
        c.components().next().map(|x| x.as_os_str().to_string_lossy().to_lowercase())
    }
    match (root(a), root(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Puts the pair at `other` (a folder with `data.sah`/`data.saf`, or a `.sah` with its sibling `.saf`) in place of `client/data.sah` +
/// `data.saf`; the client's pair is kept as `client/data.<UTC stamp>.sah`/`.saf`. Both pairs must be on the same volume (rename); else
/// `Error::Refused`. The new pair must parse and have no out-of-bounds entries.
pub fn swap(client: &Path, other: &Path, job: &Job) -> Result<SwapReport> {
    job.stage("swapping");
    if let Some(note) = finish_swap(client)? {
        job.info(&note);
    }
    let files = ClientFiles::locate(client)?;
    let (o_sah, o_saf) = resolve_other(other)?;
    if same_file(&o_sah, &files.sah) || same_file(&o_saf, &files.saf) {
        return Err(Error::Refused("that is the archive this client already uses".to_string()));
    }
    let idx = Index::read(&o_sah)?;
    let saf_len = std::fs::metadata(&o_saf).map_err(|e| Error::io(&o_saf, e))?.len();
    let l = layout::analyse(&idx, Some(saf_len));
    if l.out_of_bounds_count > 0 {
        return Err(Error::Refused(format!("{} has {} entries that lie outside {}; it would not work as a client archive", o_sah.display(), l.out_of_bounds_count, o_saf.display())));
    }
    if !same_volume(&o_sah, &files.sah) || !same_volume(&o_saf, &files.saf) {
        return Err(Error::Refused("the other archive is on another drive than the client folder; copy it into the client's drive first (a swap only renames files)".to_string()));
    }
    let plan = plan_renames(client, other)?;
    let text: String = plan.iter().map(|(f, t)| format!("{}\t{}\n", f.display(), t.display())).collect();
    write_atomic(&client.join(JOURNAL), text.as_bytes())?;
    job.check()?;
    for (from, to) in &plan {
        std::fs::rename(from, to).map_err(|e| Error::io(from, e))?;
    }
    let _ = std::fs::remove_file(client.join(JOURNAL));
    Ok(SwapReport { installed_from: o_sah, kept_as: (plan[0].1.clone(), plan[1].1.clone()) })
}

/// Repeats each planned rename whose source exists and target does not, then deletes the journal, so a crash at any point rolls forward.
pub fn finish_swap(client: &Path) -> Result<Option<String>> {
    let jp = client.join(JOURNAL);
    let text = match std::fs::read_to_string(&jp) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(&jp, e)),
    };
    let mut lines = Vec::new();
    for l in text.lines().filter(|l| !l.trim().is_empty()) {
        let Some((from, to)) = l.split_once('\t') else {
            return Err(Error::Refused("data.swap-journal is damaged; see docs/guides/undo-and-recovery.md".to_string()));
        };
        lines.push((PathBuf::from(from), PathBuf::from(to)));
    }
    for (from, to) in &lines {
        if from.exists() && !to.exists() {
            std::fs::rename(from, to).map_err(|e| Error::io(from, e))?;
        }
    }
    std::fs::remove_file(&jp).map_err(|e| Error::io(&jp, e))?;
    Ok(Some("finished an interrupted archive swap".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::{SahBuilder, TempDir};

    fn client(d: &Path, body: &[u8]) -> PathBuf {
        std::fs::create_dir_all(d).unwrap();
        SahBuilder::new(0).file(b"a/f.txt", body).write_to(d);
        d.to_path_buf()
    }

    fn read(d: &Path) -> Vec<u8> {
        crate::Archive::open(d).unwrap().read("a/f.txt").unwrap()
    }

    #[test]
    fn swap_puts_the_other_pair_in_place_and_keeps_the_old_one() {
        let t = TempDir::new("swap1");
        let c = client(&t.path().join("c"), b"mine");
        let o = client(&t.path().join("o"), b"theirs");
        let r = swap(&c, &o, &Job::silent()).unwrap();
        assert_eq!(read(&c), b"theirs");
        let kept_sah = r.kept_as.0.clone();
        assert!(kept_sah.file_name().unwrap().to_string_lossy().starts_with("data.2") && kept_sah.exists() && r.kept_as.1.exists());
        assert!(!c.join(JOURNAL).exists());
        assert!(!o.join("data.sah").exists(), "the other pair moved");
        // swapping back through the kept .sah
        let back = swap(&c, &kept_sah, &Job::silent()).unwrap();
        assert_eq!(read(&c), b"mine");
        assert!(back.kept_as.0.exists());
    }

    #[test]
    fn a_pair_with_entries_outside_its_saf_or_the_same_pair_is_refused() {
        let t = TempDir::new("swap2");
        let c = client(&t.path().join("c"), b"mine");
        let o = client(&t.path().join("o"), b"theirs");
        std::fs::write(o.join("data.saf"), b"x").unwrap();
        assert!(matches!(swap(&c, &o, &Job::silent()), Err(Error::Refused(_))));
        assert!(matches!(swap(&c, &c, &Job::silent()), Err(Error::Refused(_))));
        assert_eq!(read(&c), b"mine");
        assert!(swap(&c, &t.path().join("missing"), &Job::silent()).is_err());
    }

    #[test]
    fn a_crash_after_any_rename_rolls_forward() {
        for done in 0..=4 {
            let t = TempDir::new("swap3");
            let c = client(&t.path().join("c"), b"mine");
            let o = client(&t.path().join("o"), b"theirs");
            let plan = plan_renames(&c, &o).unwrap();
            let text: String = plan.iter().map(|(f, t)| format!("{}\t{}\n", f.display(), t.display())).collect();
            std::fs::write(c.join(JOURNAL), text).unwrap();
            for (from, to) in plan.iter().take(done) {
                std::fs::rename(from, to).unwrap();
            }
            // the client is in whatever half state the crash left; finishing completes it
            assert!(finish_swap(&c).unwrap().is_some());
            assert_eq!(read(&c), b"theirs", "after {done} renames");
            assert!(!c.join(JOURNAL).exists());
            assert!(plan[0].1.exists() && plan[1].1.exists());
            assert!(finish_swap(&c).unwrap().is_none());
        }
    }
}
