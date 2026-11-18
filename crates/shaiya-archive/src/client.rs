//! A client folder as the toolkit changes it: locating its files, the "is the game running" check, recovery of interrupted work, and
//! `Version.ini`.

use crate::{compact, find_in_dir, journal, swap};
use shaiya_core::{Error, Result};
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

/// The files of a client folder. [`ClientFiles::locate`] finds `data.sah`/`data.saf` in any case, and `game.exe` and `Version.ini` when
/// present.
#[derive(Clone, Debug)]
pub struct ClientFiles {
    pub dir: PathBuf,
    pub sah: PathBuf,
    pub saf: PathBuf,
    pub game_exe: Option<PathBuf>,
    pub version_ini: Option<PathBuf>,
}

fn missing(dir: &Path, what: &str) -> Error {
    Error::io(dir.join(what), std::io::Error::new(std::io::ErrorKind::NotFound, format!("no {what} in this folder")))
}

impl ClientFiles {
    /// `Error::Io` naming the missing file.
    pub fn locate(dir: &Path) -> Result<ClientFiles> {
        let sah = find_in_dir(dir, "data.sah").ok_or_else(|| missing(dir, "data.sah"))?;
        let saf = find_in_dir(dir, "data.saf").ok_or_else(|| missing(dir, "data.saf"))?;
        Ok(ClientFiles { dir: dir.to_path_buf(), sah, saf, game_exe: find_in_dir(dir, "game.exe"), version_ini: find_in_dir(dir, "version.ini") })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InUse {
    Free,
    Busy(String),
    /// This OS cannot tell (Linux, macOS).
    Unknown,
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Windows: opens sah and saf with read + write and `share_mode(0)`; raw OS error 32 -> `Busy("data.saf is open in another program")`;
/// opens `game.exe` for write (error 32 -> `Busy("game.exe from this folder is running")`); error 5 on the data files ->
/// `Busy("... is read-only")`. Other systems: `Unknown`.
#[cfg(windows)]
pub fn in_use(files: &ClientFiles) -> InUse {
    use std::os::windows::fs::OpenOptionsExt;
    for p in [&files.sah, &files.saf] {
        match OpenOptions::new().read(true).write(true).share_mode(0).open(p) {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(32) => return InUse::Busy(format!("{} is open in another program", name_of(p))),
            Err(e) if e.raw_os_error() == Some(5) => return InUse::Busy(format!("{} is read-only or not writable", name_of(p))),
            Err(e) => return InUse::Busy(format!("{} cannot be opened for writing ({e})", name_of(p))),
        }
    }
    if let Some(exe) = &files.game_exe {
        match OpenOptions::new().write(true).open(exe) {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(32) => return InUse::Busy("game.exe from this folder is running".to_string()),
            Err(_) => {}
        }
    }
    InUse::Free
}

#[cfg(not(windows))]
pub fn in_use(files: &ClientFiles) -> InUse {
    // Writability can still be told; running programs cannot.
    for p in [&files.sah, &files.saf] {
        if OpenOptions::new().read(true).write(true).open(p).is_err() {
            return InUse::Busy(format!("{} is read-only or not writable", name_of(p)));
        }
    }
    InUse::Unknown
}

/// `Busy` -> `Error::Refused("<reason>; close the game (and any other program using this client, Shaiya Toolkit included) and try again")`.
pub fn ensure_not_in_use(files: &ClientFiles) -> Result<InUse> {
    match in_use(files) {
        InUse::Busy(why) => Err(Error::Refused(format!("{why}; close the game (and any other program using this client, Shaiya Toolkit included) and try again"))),
        other => Ok(other),
    }
}

/// Run before every write and when Shaiya Toolkit opens a client: `compact::finish_compact`, `swap::finish_swap`, then the pending marker
/// (see docs/guides/undo-and-recovery.md). Returns human notes ("finished an interrupted compaction", ...).
pub fn recover(dir: &Path) -> Result<Vec<String>> {
    let mut notes = Vec::new();
    let had_compact = ["data.sah.compact", "data.saf.compact", "data.compact-commit"].iter().any(|n| dir.join(n).exists());
    if had_compact {
        let committed = dir.join("data.compact-commit").exists();
        compact::finish_compact(dir)?;
        notes.push(if committed { "finished an interrupted compaction".to_string() } else { "removed the leftovers of an interrupted compaction (the archive is as it was)".to_string() });
    }
    if let Some(n) = swap::finish_swap(dir)? {
        notes.push(n);
    }
    if let Some(n) = journal::recover_marker(dir)? {
        notes.push(n);
    }
    Ok(notes)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct VersionIni {
    pub check_version: Option<u32>,
    pub current_version: Option<u32>,
}

/// `[Version]` keys, case-insensitive, ASCII/cp949 text.
pub fn read_version_ini(files: &ClientFiles) -> Option<VersionIni> {
    let raw = std::fs::read(files.version_ini.as_ref()?).ok()?;
    let (text, _) = shaiya_core::text::Encoding::Cp949.decode(&raw);
    let mut out = VersionIni::default();
    let mut in_section = false;
    for line in text.lines() {
        let l = line.trim().trim_start_matches('\u{FEFF}');
        if let Some(sec) = l.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_section = sec.trim().eq_ignore_ascii_case("version");
            continue;
        }
        if !in_section {
            continue;
        }
        let Some((k, v)) = l.split_once('=') else { continue };
        let v = v.trim().parse::<u32>().ok();
        match k.trim().to_ascii_lowercase().as_str() {
            "checkversion" => out.check_version = v,
            "currentversion" => out.current_version = v,
            _ => {}
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::{SahBuilder, TempDir};

    #[test]
    fn locate_finds_files_and_names_the_missing_one() {
        let d = TempDir::new("locate");
        let e = ClientFiles::locate(d.path()).unwrap_err();
        assert!(e.to_string().contains("data.sah"), "{e}");
        SahBuilder::new(0).file(b"a", b"1").write_to(d.path());
        std::fs::remove_file(d.path().join("data.saf")).unwrap();
        assert!(ClientFiles::locate(d.path()).unwrap_err().to_string().contains("data.saf"));
        std::fs::write(d.path().join("data.saf"), b"1").unwrap();
        std::fs::write(d.path().join("Version.ini"), b"[Version]\r\nCheckVersion=3\r\nCurrentVersion=108\r\n").unwrap();
        let f = ClientFiles::locate(d.path()).unwrap();
        assert!(f.game_exe.is_none());
        assert!(f.version_ini.is_some());
        let v = read_version_ini(&f).unwrap();
        assert_eq!((v.check_version, v.current_version), (Some(3), Some(108)));
    }

    #[test]
    fn version_ini_is_case_and_section_aware() {
        let d = TempDir::new("ini");
        SahBuilder::new(0).file(b"a", b"1").write_to(d.path());
        let p = d.path().join("VERSION.INI");
        std::fs::write(&p, b"\xEF\xBB\xBF[Other]\ncheckversion=9\n[version]\n CHECKVERSION = 7 \nStartUpdate=UPDATE_END\n").unwrap();
        let f = ClientFiles::locate(d.path()).unwrap();
        let v = read_version_ini(&f).unwrap();
        assert_eq!((v.check_version, v.current_version), (Some(7), None));
        std::fs::remove_file(&p).unwrap();
        assert!(read_version_ini(&ClientFiles::locate(d.path()).unwrap()).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn an_open_data_saf_is_busy_and_closed_is_free() {
        let d = TempDir::new("inuse");
        SahBuilder::new(0).file(b"a", b"1").write_to(d.path());
        let f = ClientFiles::locate(d.path()).unwrap();
        assert_eq!(in_use(&f), InUse::Free);
        let held = std::fs::File::open(&f.saf).unwrap();
        match in_use(&f) {
            InUse::Busy(why) => assert!(why.contains("data.saf"), "{why}"),
            other => panic!("{other:?}"),
        }
        let r = ensure_not_in_use(&f).unwrap_err();
        assert!(matches!(r, Error::Refused(_)) && r.to_string().contains("try again"), "{r}");
        drop(held);
        assert_eq!(ensure_not_in_use(&f).unwrap(), InUse::Free);
    }

    #[cfg(not(windows))]
    #[test]
    fn other_systems_cannot_tell() {
        let d = TempDir::new("inuse");
        SahBuilder::new(0).file(b"a", b"1").write_to(d.path());
        assert_eq!(in_use(&ClientFiles::locate(d.path()).unwrap()), InUse::Unknown);
    }

    #[test]
    fn recover_on_a_clean_folder_says_nothing() {
        let d = TempDir::new("recover-clean");
        SahBuilder::new(0).file(b"a", b"1").write_to(d.path());
        assert!(recover(d.path()).unwrap().is_empty());
    }
}
