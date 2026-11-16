//! Comparing two archives (or two indexes).

use crate::{path_key, Archive, Entry, Index};
use shaiya_core::glob::Filter;
use shaiya_core::{Error, Job, Result, Unit};
use std::collections::HashMap;

pub enum Side<'a> {
    Index(&'a Index),
    Archive(&'a Archive),
}

impl Side<'_> {
    fn index(&self) -> &Index {
        match self {
            Side::Index(i) => i,
            Side::Archive(a) => a.index(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Change {
    Added,
    Removed,
    Different,
    Same,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DiffEntry {
    pub path: String,
    pub change: Change,
    pub old: Option<Entry>,
    pub new: Option<Entry>,
}

#[derive(Clone, Debug, Default)]
pub struct DiffOptions {
    pub content: bool,
    pub filter: Filter,
    pub include_shadowed: bool,
}

type Map = HashMap<Vec<u8>, (String, Entry)>;

fn collect(side: &Side, opts: &DiffOptions) -> Map {
    let idx = side.index();
    let all = if opts.include_shadowed { idx.entries() } else { idx.visible() };
    let mut m = HashMap::with_capacity(all.len());
    for f in all {
        if opts.filter.matches(&f.path, f.entry.length as u64) {
            // later entries win, as in the client
            m.insert(path_key(&f.raw_path), (f.path, f.entry.clone()));
        }
    }
    m
}

fn same_bytes(a: &Archive, ea: &Entry, b: &Archive, eb: &Entry, job: &Job) -> Result<bool> {
    let (mut ra, mut rb) = (a.reader(ea)?, b.reader(eb)?);
    let (mut x, mut y) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    let mut left = ea.length as usize;
    while left > 0 {
        job.check()?;
        let n = left.min(x.len());
        std::io::Read::read_exact(&mut ra, &mut x[..n]).map_err(Error::from)?;
        std::io::Read::read_exact(&mut rb, &mut y[..n]).map_err(Error::from)?;
        job.advance(n as u64);
        if x[..n] != y[..n] {
            // account for the bytes not read so progress still ends at the total
            job.advance(left as u64 - n as u64);
            return Ok(false);
        }
        left -= n;
    }
    Ok(true)
}

/// Visible entries matched by `path_key`. Length differs -> `Different`. Same length: with `content` (both `Archive` sides) bytes
/// compared in 1 MiB chunks -> `Same`/`Different`; without `content` (or an `Index` side) equal version -> `Same`, else `Unknown`.
/// Output sorted by path.
pub fn diff(old: &Side, new: &Side, opts: &DiffOptions, job: &Job) -> Result<Vec<DiffEntry>> {
    job.stage("comparing");
    let (om, nm) = (collect(old, opts), collect(new, opts));
    let both = match (old, new) {
        (Side::Archive(a), Side::Archive(b)) if opts.content => Some((*a, *b)),
        _ => None,
    };
    let mut out: Vec<DiffEntry> = Vec::with_capacity(om.len().max(nm.len()));
    let mut to_compare: Vec<(usize, Entry, Entry)> = Vec::new();
    for (k, (path, oe)) in &om {
        match nm.get(k) {
            None => out.push(DiffEntry { path: path.clone(), change: Change::Removed, old: Some(oe.clone()), new: None }),
            Some((npath, ne)) => {
                let change = if oe.length != ne.length {
                    Change::Different
                } else if oe.length == 0 {
                    Change::Same
                } else if both.is_some() {
                    to_compare.push((out.len(), oe.clone(), ne.clone()));
                    Change::Unknown
                } else if oe.version == ne.version {
                    Change::Same
                } else {
                    Change::Unknown
                };
                out.push(DiffEntry { path: npath.clone(), change, old: Some(oe.clone()), new: Some(ne.clone()) });
            }
        }
    }
    for (k, (path, ne)) in &nm {
        if !om.contains_key(k) {
            out.push(DiffEntry { path: path.clone(), change: Change::Added, old: None, new: Some(ne.clone()) });
        }
    }
    if let Some((a, b)) = both {
        job.total(Some(to_compare.iter().map(|c| c.1.length as u64).sum()), Unit::Bytes);
        // sequential reads: compare in offset order of the old side
        to_compare.sort_by_key(|c| c.1.offset);
        for (i, oe, ne) in &to_compare {
            out[*i].change = if same_bytes(a, oe, b, ne, job)? { Change::Same } else { Change::Different };
        }
    }
    out.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()).then(a.path.cmp(&b.path)));
    Ok(out)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DiffSummary {
    pub added: usize,
    pub removed: usize,
    pub different: usize,
    pub same: usize,
    pub unknown: usize,
}

pub fn summarize(d: &[DiffEntry]) -> DiffSummary {
    let mut s = DiffSummary::default();
    for e in d {
        match e.change {
            Change::Added => s.added += 1,
            Change::Removed => s.removed += 1,
            Change::Different => s.different += 1,
            Change::Same => s.same += 1,
            Change::Unknown => s.unknown += 1,
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::{SahBuilder, TempDir};

    fn pair(tag: &str, b: SahBuilder) -> (TempDir, Archive) {
        let d = TempDir::new(tag);
        b.write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        (d, a)
    }

    #[test]
    fn all_five_changes() {
        let (_d1, old) = pair(
            "diff-old",
            SahBuilder::new(0).file(b"a/same.txt", b"same").file(b"a/changed.txt", b"before!").file(b"a/gone.txt", b"bye").file_versioned(b"a/v.txt", b"vv", 1).file(b"a/samelen.txt", b"abcd"),
        );
        let (_d2, new) = pair(
            "diff-new",
            SahBuilder::new(0).file(b"a/SAME.txt", b"same").file(b"a/changed.txt", b"after").file(b"a/new.txt", b"hi").file_versioned(b"a/v.txt", b"ww", 2).file(b"a/samelen.txt", b"abXd"),
        );
        let quick = diff(&Side::Archive(&old), &Side::Archive(&new), &DiffOptions::default(), &Job::silent()).unwrap();
        let get = |d: &[DiffEntry], p: &str| d.iter().find(|e| e.path.eq_ignore_ascii_case(p)).map(|e| e.change).unwrap();
        assert_eq!(get(&quick, "a/same.txt"), Change::Same);
        assert_eq!(get(&quick, "a/changed.txt"), Change::Different);
        assert_eq!(get(&quick, "a/gone.txt"), Change::Removed);
        assert_eq!(get(&quick, "a/new.txt"), Change::Added);
        assert_eq!(get(&quick, "a/v.txt"), Change::Unknown, "same length, different version, no content compare");
        assert_eq!(get(&quick, "a/samelen.txt"), Change::Same, "equal versions are taken as same without content");
        assert_eq!(summarize(&quick), DiffSummary { added: 1, removed: 1, different: 1, same: 2, unknown: 1 });
        let content = diff(&Side::Archive(&old), &Side::Archive(&new), &DiffOptions { content: true, ..Default::default() }, &Job::silent()).unwrap();
        assert_eq!(get(&content, "a/v.txt"), Change::Different);
        assert_eq!(get(&content, "a/samelen.txt"), Change::Different);
        assert_eq!(get(&content, "a/same.txt"), Change::Same);
        let names: Vec<&str> = content.iter().map(|e| e.path.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|s| s.to_lowercase());
        assert_eq!(names, sorted, "output is sorted by path");
        // an index side switches content comparison off
        let idx = diff(&Side::Index(old.index()), &Side::Archive(&new), &DiffOptions { content: true, ..Default::default() }, &Job::silent()).unwrap();
        assert_eq!(get(&idx, "a/v.txt"), Change::Unknown);
        // filters apply to both sides
        let only = diff(&Side::Archive(&old), &Side::Archive(&new), &DiffOptions { filter: Filter::from_args(&["a/n*".into()], &[], None, None).unwrap(), ..Default::default() }, &Job::silent()).unwrap();
        assert_eq!(only.len(), 1);
    }

    #[test]
    fn an_archive_equals_itself_and_shadowed_entries_follow_the_client() {
        let (_d, a) = pair("diff-self", SahBuilder::new(0).file(b"w/A.dg", b"old").file(b"w/a.DG", b"newer").file(b"x", b""));
        let d = diff(&Side::Archive(&a), &Side::Archive(&a), &DiffOptions { content: true, ..Default::default() }, &Job::silent()).unwrap();
        assert_eq!(d.len(), 2, "the shadowed entry is not a separate file");
        assert!(d.iter().all(|e| e.change == Change::Same));
        assert_eq!(d[0].new.as_ref().unwrap().length, 5, "the later entry is the one compared");
        let c = shaiya_core::Cancel::new();
        c.cancel();
        assert!(matches!(diff(&Side::Archive(&a), &Side::Archive(&a), &DiffOptions { content: true, ..Default::default() }, &Job::new(&shaiya_core::Silent, c)), Err(Error::Cancelled)));
    }
}
