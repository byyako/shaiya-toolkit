//! The settings-level enums and the path key the checks and the editor share. The serde names are those of the editor's settings file,
//! so an existing settings file keeps parsing.

use serde::{Deserialize, Serialize};

/// What the Changes tab compares the map against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompareAgainst {
    /// The map as it was loaded or last saved.
    #[default]
    Saved,
    /// The `<file>.bak` the first save made: the original before the editor touched it.
    Backup,
    /// The same map in the reference folder.
    Reference,
    /// The same map in the other folder of Compare folders: Take brings its records over.
    Other,
}

impl CompareAgainst {
    pub const ALL: [CompareAgainst; 4] = [CompareAgainst::Saved, CompareAgainst::Backup, CompareAgainst::Reference, CompareAgainst::Other];

    pub fn label(self) -> &'static str {
        match self {
            CompareAgainst::Saved => "last save",
            CompareAgainst::Backup => "original (.bak)",
            CompareAgainst::Reference => "reference",
            CompareAgainst::Other => "other folder",
        }
    }

    /// The word the change rows use: "changed vs <this>".
    pub fn short(self) -> &'static str {
        match self {
            CompareAgainst::Saved => "last save",
            CompareAgainst::Backup => "original",
            CompareAgainst::Reference => "reference",
            CompareAgainst::Other => "other folder",
        }
    }
}

/// Which server the cross-file checks judge a folder for: the original `ps_game` stops at an id without a map file, Afterglow skips it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetServer {
    #[default]
    Original,
    Afterglow,
}

impl TargetServer {
    pub const ALL: [TargetServer; 2] = [TargetServer::Original, TargetServer::Afterglow];

    pub fn label(self) -> &'static str {
        match self {
            TargetServer::Original => "original",
            TargetServer::Afterglow => "Afterglow",
        }
    }
}

/// 16 hex digits of FNV-1a 64 over the lower-cased path: the key of recovery entries, backups and bookmarks.
pub fn path_key(p: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in p.to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_key_is_stable_and_case_blind() {
        assert_eq!(path_key("D:\\Maps"), path_key("d:\\maps"));
        assert_eq!(path_key("").len(), 16);
        // FNV-1a 64 of "a"
        assert_eq!(path_key("a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn the_enums_keep_their_settings_names() {
        assert_eq!(serde_json::to_string(&CompareAgainst::Backup).unwrap(), "\"backup\"");
        assert_eq!(serde_json::to_string(&TargetServer::Afterglow).unwrap(), "\"afterglow\"");
        assert_eq!(serde_json::from_str::<TargetServer>("\"original\"").unwrap(), TargetServer::Original);
        assert_eq!(CompareAgainst::default(), CompareAgainst::Saved);
    }
}
