//! The toolkit data folder rule and the shared "my client folders" list (`shared.json`, key `clients`). Pure: the binaries read the
//! environment and pass it in ([`DataDirInputs`]), so no library reads environment variables.

use crate::error::{Error, Result};
use crate::fs::write_atomic;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct DataDirInputs {
    /// `SHAIYA_TOOLKIT_HOME`.
    pub toolkit_home: Option<PathBuf>,
    /// The folder of the executable (for `portable.txt`).
    pub exe_dir: Option<PathBuf>,
    /// Windows `%APPDATA%`.
    pub appdata: Option<PathBuf>,
    /// `HOME` (or the user profile folder).
    pub home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Mac,
    Linux,
}

impl Platform {
    pub const CURRENT: Platform = if cfg!(windows) {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Mac
    } else {
        Platform::Linux
    };
}

/// The rule with the OS and the `portable.txt` check given: `toolkit_home`; else portable -> `exe_dir/shaiya-toolkit-data`; else the
/// per-OS folder. `None` when no folder can be determined (then nothing is stored).
pub fn data_root_with(i: &DataDirInputs, platform: Platform, portable: bool) -> Option<PathBuf> {
    if let Some(h) = i.toolkit_home.as_ref().filter(|h| !h.as_os_str().is_empty()) {
        return Some(h.clone());
    }
    if portable {
        if let Some(d) = &i.exe_dir {
            return Some(d.join("shaiya-toolkit-data"));
        }
    }
    match platform {
        Platform::Windows => i.appdata.as_ref().map(|a| a.join("shaiya-toolkit")),
        Platform::Mac => i.home.as_ref().map(|h| h.join("Library").join("Application Support").join("shaiya-toolkit")),
        Platform::Linux => match i.xdg_config_home.as_ref().filter(|x| !x.as_os_str().is_empty()) {
            Some(x) => Some(x.join("shaiya-toolkit")),
            None => i.home.as_ref().map(|h| h.join(".config").join("shaiya-toolkit")),
        },
    }
}

/// The rule: `toolkit_home`; else `exe_dir/portable.txt` present -> `exe_dir/shaiya-toolkit-data`; else the per-OS folder.
pub fn data_root(i: &DataDirInputs) -> Option<PathBuf> {
    let portable = i.exe_dir.as_ref().is_some_and(|d| d.join("portable.txt").is_file());
    data_root_with(i, Platform::CURRENT, portable)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientFolder {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub added_utc: String,
    #[serde(default)]
    pub note: String,
    /// `archive` (`data.sah` + `data.saf`, the default and never written) or `loose` (a folder of loose files).
    #[serde(default, skip_serializing_if = "ClientKind::is_archive")]
    pub kind: ClientKind,
    /// A loose client's `data` folder when it differs from `path` (which is the install `root`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<PathBuf>,
    /// A loose client's confirmed backup folder (outside `data`); none until one is chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backups: Option<PathBuf>,
    /// Keys of the entry that this version does not know (a newer version wrote them): kept and written back as they were.
    #[serde(flatten, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

/// How a saved client keeps its data. A `kind` this version does not know (a newer version wrote it, or the file was edited) is read as
/// `Other` with the text as written and is written back as it was: the entry is listed, never opened or changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ClientKind {
    #[default]
    Archive,
    Loose,
    /// The `kind` text of an entry this version does not know (it is kept exactly).
    Other(String),
}

impl ClientKind {
    pub fn is_archive(&self) -> bool {
        *self == ClientKind::Archive
    }

    /// The text of the `kind` key.
    pub fn as_str(&self) -> &str {
        match self {
            ClientKind::Archive => "archive",
            ClientKind::Loose => "loose",
            ClientKind::Other(s) => s,
        }
    }

    /// Whether this version knows what to do with the entry (archive and loose are the only kinds it opens).
    pub fn is_supported(&self) -> bool {
        !matches!(self, ClientKind::Other(_))
    }
}

impl Serialize for ClientKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ClientKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<ClientKind, D::Error> {
        let text = String::deserialize(d)?;
        Ok(match text.as_str() {
            "archive" => ClientKind::Archive,
            "loose" => ClientKind::Loose,
            _ => ClientKind::Other(text),
        })
    }
}

impl ClientFolder {
    /// A saved archive client (`data.sah` + `data.saf` in `path`).
    pub fn archive(name: impl Into<String>, path: PathBuf, added_utc: impl Into<String>, note: impl Into<String>) -> ClientFolder {
        ClientFolder { name: name.into(), path, added_utc: added_utc.into(), note: note.into(), kind: ClientKind::Archive, data: None, backups: None, extra: BTreeMap::new() }
    }

    /// The folder that holds the files: `data` for a loose client that has one, else `path`.
    pub fn data_folder(&self) -> &Path {
        self.data.as_deref().unwrap_or(&self.path)
    }
}

fn shared_path(root: &Path) -> PathBuf {
    root.join("shared.json")
}

fn read_shared(root: &Path) -> Result<Option<Map<String, Value>>> {
    let p = shared_path(root);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::io(&p, e)),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(Some(m)),
        Ok(_) => Err(Error::format(p.display().to_string(), None, "shared.json is not a JSON object")),
        Err(e) => Err(Error::format(p.display().to_string(), Some(e.column() as u64), format!("shared.json is not valid JSON: {e}"))),
    }
}

/// The entries that parse as clients, and the entries that do not (or have no path) exactly as they were written: a write-back keeps those.
fn split_clients(map: &Map<String, Value>) -> (Vec<ClientFolder>, Vec<Value>) {
    let Some(Value::Array(a)) = map.get("clients") else { return (Vec::new(), Vec::new()) };
    let (mut list, mut raw) = (Vec::new(), Vec::new());
    for v in a {
        match serde_json::from_value::<ClientFolder>(v.clone()) {
            Ok(c) if !c.path.as_os_str().is_empty() => list.push(c),
            _ => raw.push(v.clone()),
        }
    }
    (list, raw)
}

fn clients_of(map: &Map<String, Value>) -> Vec<ClientFolder> {
    split_clients(map).0
}

/// Reads `<root>/shared.json` "clients"; a missing file or key is an empty list.
pub fn load_clients(root: &Path) -> Result<Vec<ClientFolder>> {
    Ok(read_shared(root)?.map(|m| clients_of(&m)).unwrap_or_default())
}

/// Re-reads shared.json, lets `f` change only the clients list, writes back atomically keeping every other key as it was. An entry that does
/// not parse as a client (a newer version's, or a damaged one) is written back exactly as it was, after the list; an entry of an unknown
/// `kind` is in the list (as [`ClientKind::Other`]) and keeps its `kind` text.
pub fn update_clients(root: &Path, f: &mut dyn FnMut(&mut Vec<ClientFolder>) -> Result<()>) -> Result<Vec<ClientFolder>> {
    let existing = read_shared(root)?;
    let fresh = existing.is_none();
    let mut map = existing.unwrap_or_default();
    let (mut list, raw) = split_clients(&map);
    f(&mut list)?;
    if fresh {
        map.insert("format".into(), Value::from(1));
    }
    let mut written = match serde_json::to_value(&list).map_err(|e| Error::format("shared.json", None, e.to_string()))? {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    written.extend(raw);
    map.insert("clients".into(), Value::Array(written));
    std::fs::create_dir_all(root).map_err(|e| Error::io(root, e))?;
    let text = serde_json::to_string_pretty(&Value::Object(map)).map_err(|e| Error::format("shared.json", None, e.to_string()))?;
    write_atomic(&shared_path(root), text.as_bytes())?;
    Ok(list)
}

/// Case-insensitive.
pub fn find_client<'a>(clients: &'a [ClientFolder], name: &str) -> Option<&'a ClientFolder> {
    clients.iter().find(|c| c.name.eq_ignore_ascii_case(name) || c.name.to_lowercase() == name.to_lowercase())
}

/// 1 to 40 characters, no `/`, `\`, `@`, control characters, not only spaces; `Error::Refused` with the reason.
pub fn validate_client_name(name: &str) -> Result<()> {
    let bad = |why: &str| Err(Error::Refused(format!("client name {name:?}: {why}")));
    let n = name.chars().count();
    if n == 0 || n > 40 {
        return bad("use 1 to 40 characters");
    }
    if name.trim().is_empty() {
        return bad("a name cannot be only spaces");
    }
    if let Some(c) = name.chars().find(|c| matches!(c, '/' | '\\' | '@') || c.is_control()) {
        return bad(&format!("the character {c:?} is not allowed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shaiya-core-config-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn client(name: &str, path: &str) -> ClientFolder {
        ClientFolder::archive(name, PathBuf::from(path), "2026-11-15T12:00:00Z", "")
    }

    #[test]
    fn data_root_follows_the_documented_order() {
        let i = DataDirInputs {
            toolkit_home: None,
            exe_dir: Some(PathBuf::from("/opt/tk")),
            appdata: Some(PathBuf::from("C:/Users/u/AppData/Roaming")),
            home: Some(PathBuf::from("/home/u")),
            xdg_config_home: None,
        };
        assert_eq!(data_root_with(&i, Platform::Windows, false), Some(PathBuf::from("C:/Users/u/AppData/Roaming").join("shaiya-toolkit")));
        assert_eq!(data_root_with(&i, Platform::Mac, false), Some(PathBuf::from("/home/u/Library/Application Support/shaiya-toolkit")));
        assert_eq!(data_root_with(&i, Platform::Linux, false), Some(PathBuf::from("/home/u/.config/shaiya-toolkit")));
        let xdg = DataDirInputs { xdg_config_home: Some(PathBuf::from("/x/cfg")), ..i.clone() };
        assert_eq!(data_root_with(&xdg, Platform::Linux, false), Some(PathBuf::from("/x/cfg/shaiya-toolkit")));
        assert_eq!(data_root_with(&i, Platform::Linux, true), Some(PathBuf::from("/opt/tk/shaiya-toolkit-data")));
        let home = DataDirInputs { toolkit_home: Some(PathBuf::from("/h")), ..i.clone() };
        assert_eq!(data_root_with(&home, Platform::Windows, true), Some(PathBuf::from("/h")));
        assert_eq!(data_root_with(&DataDirInputs::default(), Platform::Windows, false), None);
        assert_eq!(data_root_with(&DataDirInputs { exe_dir: Some(PathBuf::from("/o")), ..Default::default() }, Platform::Linux, true), Some(PathBuf::from("/o/shaiya-toolkit-data")));
    }

    #[test]
    fn portable_txt_next_to_the_exe_switches_the_root() {
        let d = tmp("portable");
        let i = DataDirInputs { exe_dir: Some(d.clone()), toolkit_home: None, appdata: Some(d.join("ad")), home: Some(d.join("home")), xdg_config_home: None };
        assert_ne!(data_root(&i), Some(d.join("shaiya-toolkit-data")));
        std::fs::write(d.join("portable.txt"), b"").unwrap();
        assert_eq!(data_root(&i), Some(d.join("shaiya-toolkit-data")));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_file_or_key_is_an_empty_list() {
        let d = tmp("missing");
        assert!(load_clients(&d).unwrap().is_empty());
        std::fs::write(d.join("shared.json"), r#"{"theme": "dark"}"#).unwrap();
        assert!(load_clients(&d).unwrap().is_empty());
        std::fs::write(d.join("shared.json"), "garbage").unwrap();
        assert!(matches!(load_clients(&d), Err(Error::Format { .. })));
        assert!(update_clients(&d, &mut |_| Ok(())).is_err(), "a damaged file is never overwritten");
        assert_eq!(std::fs::read_to_string(d.join("shared.json")).unwrap(), "garbage");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn update_keeps_every_other_key_and_both_additions() {
        let d = tmp("update");
        std::fs::write(d.join("shared.json"), r#"{"format": 1, "theme": "light", "ui_scale": 1.25, "future_key": {"a": [1, 2]}, "clients": [{"name": "us", "path": "Z:/us"}]}"#).unwrap();
        let before: Value = serde_json::from_str(&std::fs::read_to_string(d.join("shared.json")).unwrap()).unwrap();
        // two read-modify-writes in sequence (two programs): both additions survive
        update_clients(&d, &mut |l| {
            l.push(client("ru", "Z:/ru"));
            Ok(())
        })
        .unwrap();
        let list = update_clients(&d, &mut |l| {
            l.push(client("fr", "Z:/fr"));
            Ok(())
        })
        .unwrap();
        assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["us", "ru", "fr"]);
        assert_eq!(load_clients(&d).unwrap(), list);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(d.join("shared.json")).unwrap()).unwrap();
        for k in ["format", "theme", "ui_scale", "future_key"] {
            assert_eq!(before[k], after[k], "{k}");
        }
        // an error from the closure writes nothing
        let r = update_clients(&d, &mut |l| {
            l.clear();
            Err(Error::Refused("no".into()))
        });
        assert!(r.is_err());
        assert_eq!(load_clients(&d).unwrap().len(), 3);
        // a fresh root is created
        let fresh = d.join("new-root");
        update_clients(&fresh, &mut |l| {
            l.push(client("a", "p"));
            Ok(())
        })
        .unwrap();
        assert_eq!(load_clients(&fresh).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }


    #[test]
    fn an_old_client_list_without_kind_loads_as_archive() {
        let d = tmp("old-kind");
        std::fs::write(d.join("shared.json"), r#"{"format": 1, "clients": [{"name": "us", "path": "/games/us", "note": "n"}]}"#).unwrap();
        let list = load_clients(&d).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].kind.clone(), list[0].data.clone(), list[0].backups.clone()), (ClientKind::Archive, None, None));
        assert!(list[0].kind.is_archive());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn kind_data_and_backups_round_trip_and_are_omitted_for_archive() {
        let d = tmp("kind-round");
        let loose = ClientFolder {
            kind: ClientKind::Loose,
            data: Some(PathBuf::from("/games/gx/Client/data")),
            backups: Some(PathBuf::from("/games/gx/backups")),
            ..client("gx", "/games/gx/Client")
        };
        update_clients(&d, &mut |l| {
            l.push(client("us", "/games/us"));
            l.push(loose.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(load_clients(&d).unwrap()[1], loose);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("shared.json")).unwrap()).unwrap();
        let (us, gx) = (&v["clients"][0], &v["clients"][1]);
        for k in ["kind", "data", "backups"] {
            assert!(us.get(k).is_none(), "an archive client has no {k} key");
        }
        assert_eq!(gx["kind"], "loose");
        assert_eq!(load_clients(&d).unwrap()[1].data_folder(), Path::new("/games/gx/Client/data"));
        assert_eq!(load_clients(&d).unwrap()[0].data_folder(), Path::new("/games/us"));
        // an unrelated change of the list keeps the loose entry whole
        update_clients(&d, &mut |l| {
            l[0].note = "x".into();
            Ok(())
        })
        .unwrap();
        assert_eq!(load_clients(&d).unwrap()[1], loose);
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn an_entry_of_an_unknown_kind_or_shape_survives_an_unrelated_change_exactly() {
        let d = tmp("unknown-kind");
        let future = r#"{"name": "srv", "path": "/games/srv", "kind": "server", "extra": {"a": [1, 2]}}"#;
        let cased = r#"{"name": "gx", "path": "/games/gx", "kind": "Loose"}"#;
        let broken = r#"{"name": 5, "path": ["not", "text"], "kind": "loose"}"#;
        std::fs::write(d.join("shared.json"), format!(r#"{{"format": 1, "clients": [{{"name": "us", "path": "/games/us"}}, {future}, {cased}, {broken}]}}"#)).unwrap();
        let list = load_clients(&d).unwrap();
        assert_eq!(list.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["us", "srv", "gx"], "the unknown kinds are listed");
        assert_eq!(list[1].kind, ClientKind::Other("server".into()));
        assert_eq!(list[2].kind, ClientKind::Other("Loose".into()), "letter case is not guessed");
        assert!(list[0].kind.is_supported() && !list[1].kind.is_supported() && !list[2].kind.is_supported());
        // an unrelated change: a note on the first entry and an added client
        update_clients(&d, &mut |l| {
            l[0].note = "n".into();
            l.push(client("fr", "/games/fr"));
            Ok(())
        })
        .unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(d.join("shared.json")).unwrap()).unwrap();
        let a = v["clients"].as_array().unwrap();
        assert_eq!(a.len(), 5, "nothing was dropped: {a:?}");
        let by_name = |n: &str| a.iter().find(|c| c["name"] == n).unwrap().clone();
        assert_eq!(by_name("srv")["kind"], "server", "the kind text of the unknown entry is as it was written");
        assert_eq!(by_name("srv")["path"], "/games/srv");
        assert_eq!(by_name("srv")["extra"], serde_json::json!({"a": [1, 2]}), "the keys this version does not know are kept");
        assert_eq!(by_name("gx")["kind"], "Loose");
        assert!(a.iter().any(|c| *c == serde_json::from_str::<Value>(broken).unwrap()), "an entry that does not parse is written back as it was");
        assert_eq!(by_name("fr")["path"], "/games/fr");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_are_validated_and_found_without_case() {
        for ok in ["US 108", "x", &"a".repeat(40), "\u{C6D0}\u{BC14}"] {
            assert!(validate_client_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "   ", &"a".repeat(41), "a/b", "a\\b", "@us", "a@b", "tab\there"] {
            assert!(matches!(validate_client_name(bad), Err(Error::Refused(_))), "{bad:?}");
        }
        let list = vec![client("US 108", "p")];
        assert!(find_client(&list, "us 108").is_some());
        assert!(find_client(&list, "ru").is_none());
    }
}
