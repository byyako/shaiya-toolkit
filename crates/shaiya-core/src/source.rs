//! Named byte blobs from a folder, an archive or memory; used by SData batch work and patch building.

use crate::error::{Error, Result};
use crate::glob::Filter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub trait ByteSource {
    /// `folder D:\mods`, `archive Z:\Games\Shaiya\data.sah`.
    fn label(&self) -> String;
    /// `/`-separated names in a stable order.
    fn names(&self) -> Vec<String>;
    fn size(&self, name: &str) -> Option<u64>;
    fn read(&self, name: &str) -> Result<Vec<u8>>;
    /// `Some` for files on disk (in-place writes).
    fn file_path(&self, _name: &str) -> Option<PathBuf> {
        None
    }
}

struct Item {
    name: String,
    path: PathBuf,
    size: u64,
}

/// Files on disk, named by file name (an input file) or by path relative to the input folder.
pub struct FolderSource {
    items: Vec<Item>,
    by_name: HashMap<String, usize>,
    skipped: Vec<String>,
    label: String,
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, skipped: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))?.collect::<std::io::Result<Vec<_>>>().map_err(|e| Error::io(dir, e))?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let link = std::fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?;
        let rel_name = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        if link.file_type().is_symlink() {
            match std::fs::metadata(&path) {
                Ok(m) if m.is_dir() => skipped.push(format!("{rel_name} (symbolic link to a folder, not followed)")),
                Ok(_) => out.push((rel_name, path)),
                Err(_) => skipped.push(format!("{rel_name} (broken symbolic link)")),
            }
        } else if link.is_dir() {
            walk(&path, &rel_name, out, skipped)?;
        } else {
            out.push((rel_name, path));
        }
    }
    Ok(())
}

impl FolderSource {
    /// Each input file is named by its file name; each input folder contributes its files (recursively, sorted) by path relative to that
    /// folder; `filter` applies to those names. Two inputs giving the same name (case-insensitive) -> `Error::Refused` naming both.
    /// Symlinked folders are not followed (listed in [`FolderSource::skipped`]); symlinked files are read.
    pub fn new(inputs: &[PathBuf], filter: &Filter) -> Result<FolderSource> {
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        let mut skipped = Vec::new();
        for input in inputs {
            let meta = std::fs::metadata(input).map_err(|e| Error::io(input, e))?;
            if meta.is_dir() {
                walk(input, "", &mut found, &mut skipped)?;
            } else {
                let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                found.push((name, input.clone()));
            }
        }
        let mut items: Vec<Item> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        for (name, path) in found {
            let size = std::fs::metadata(&path).map_err(|e| Error::io(&path, e))?.len();
            if !filter.matches(&name, size) {
                continue;
            }
            let key = name.to_lowercase();
            if let Some(&i) = by_name.get(&key) {
                return Err(Error::Refused(format!(
                    "two inputs give the name {name:?}: {} and {} (names are compared without regard to case)",
                    items[i].path.display(),
                    path.display()
                )));
            }
            by_name.insert(key, items.len());
            items.push(Item { name, path, size });
        }
        let label = match inputs {
            [one] if one.is_dir() => format!("folder {}", one.display()),
            [one] => format!("file {}", one.display()),
            many => format!("{} inputs", many.len()),
        };
        Ok(FolderSource { items, by_name, skipped, label })
    }

    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    fn find(&self, name: &str) -> Option<&Item> {
        self.by_name.get(&name.replace('\\', "/").to_lowercase()).map(|&i| &self.items[i])
    }
}

impl ByteSource for FolderSource {
    fn label(&self) -> String {
        self.label.clone()
    }
    fn names(&self) -> Vec<String> {
        self.items.iter().map(|i| i.name.clone()).collect()
    }
    fn size(&self, name: &str) -> Option<u64> {
        self.find(name).map(|i| i.size)
    }
    fn read(&self, name: &str) -> Result<Vec<u8>> {
        let i = self.find(name).ok_or_else(|| Error::io(name, std::io::Error::new(std::io::ErrorKind::NotFound, "not in this source")))?;
        std::fs::read(&i.path).map_err(|e| Error::io(&i.path, e))
    }
    fn file_path(&self, name: &str) -> Option<PathBuf> {
        self.find(name).map(|i| i.path.clone())
    }
}

/// Named blobs held in memory (tests and the GUI).
#[derive(Default)]
pub struct MemorySource {
    pub label: String,
    pub items: Vec<(String, Vec<u8>)>,
}

impl ByteSource for MemorySource {
    fn label(&self) -> String {
        if self.label.is_empty() { "memory".to_string() } else { self.label.clone() }
    }
    fn names(&self) -> Vec<String> {
        self.items.iter().map(|(n, _)| n.clone()).collect()
    }
    fn size(&self, name: &str) -> Option<u64> {
        self.items.iter().find(|(n, _)| n == name).map(|(_, d)| d.len() as u64)
    }
    fn read(&self, name: &str) -> Result<Vec<u8>> {
        self.items
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, d)| d.clone())
            .ok_or_else(|| Error::io(name, std::io::Error::new(std::io::ErrorKind::NotFound, "not in this source")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glob::Pattern;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shaiya-core-source-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_come_from_file_names_and_relative_paths() {
        let d = tmp("names");
        std::fs::create_dir_all(d.join("mods/sub")).unwrap();
        std::fs::write(d.join("mods/b.txt"), b"bb").unwrap();
        std::fs::write(d.join("mods/a.txt"), b"a").unwrap();
        std::fs::write(d.join("mods/sub/c.txt"), b"ccc").unwrap();
        std::fs::write(d.join("lone.bin"), b"l").unwrap();
        let s = FolderSource::new(&[d.join("mods"), d.join("lone.bin")], &Filter::default()).unwrap();
        assert_eq!(s.names(), vec!["a.txt", "b.txt", "sub/c.txt", "lone.bin"]);
        assert_eq!(s.size("SUB/C.TXT"), Some(3));
        assert_eq!(s.read("a.txt").unwrap(), b"a");
        assert!(s.file_path("lone.bin").is_some());
        assert!(s.read("nope").is_err());
        assert!(s.label().starts_with("2 inputs"));
        let only_sub = FolderSource::new(&[d.join("mods")], &Filter { include: vec![Pattern::new("sub/").unwrap()], ..Filter::default() }).unwrap();
        assert_eq!(only_sub.names(), vec!["sub/c.txt"]);
        assert!(only_sub.label().starts_with("folder "));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn two_inputs_with_one_name_are_refused() {
        let d = tmp("collide");
        std::fs::create_dir_all(d.join("x")).unwrap();
        std::fs::create_dir_all(d.join("y")).unwrap();
        std::fs::write(d.join("x/Item.SData"), b"1").unwrap();
        std::fs::write(d.join("y/item.sdata"), b"2").unwrap();
        let e = FolderSource::new(&[d.join("x/Item.SData"), d.join("y/item.sdata")], &Filter::default()).err().unwrap();
        match e {
            Error::Refused(m) => assert!(m.contains("Item.SData") && m.contains("item.sdata"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(FolderSource::new(&[d.join("missing")], &Filter::default()).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn memory_source_serves_its_items() {
        let m = MemorySource { label: String::new(), items: vec![("a".into(), vec![1, 2]), ("b/c".into(), vec![])] };
        assert_eq!(m.names(), vec!["a", "b/c"]);
        assert_eq!((m.size("a"), m.size("zz")), (Some(2), None));
        assert_eq!(m.read("b/c").unwrap(), Vec::<u8>::new());
        assert!(m.read("zz").is_err());
        assert_eq!(m.label(), "memory");
        assert!(m.file_path("a").is_none());
    }
}
