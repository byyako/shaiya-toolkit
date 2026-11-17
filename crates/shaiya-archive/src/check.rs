//! Archive health: what `shaiya archive check` and Workbench's Check report.

use crate::layout::{self, Layout};
use crate::{decode_name, name_hash, Archive, Dir, Entry, Index};
use shaiya_core::fs::portable_name;
use shaiya_core::time::human_size;
use shaiya_core::{Job, Result, Unit};
use std::collections::HashMap;
use std::io::Read;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Severity {
    Info,
    Warning,
    Problem,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Finding {
    pub severity: Severity,
    pub code: &'static str,
    pub path: Option<String>,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CheckOptions {
    /// Read every visible entry once (streaming); I/O errors become findings.
    pub deep: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct CheckReport {
    pub sah_version: i32,
    pub files: usize,
    pub visible: usize,
    pub dirs: usize,
    pub free_entries: usize,
    #[cfg_attr(feature = "serde", serde(serialize_with = "crate::ser::hex"))]
    pub trailer: Vec<u8>,
    pub layout: Layout,
    pub findings: Vec<Finding>,
}

impl CheckReport {
    pub fn problems(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Problem).count()
    }
    pub fn warnings(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Warning).count()
    }
    /// Findings with this code.
    pub fn count(&self, code: &str) -> usize {
        self.findings.iter().filter(|f| f.code == code).count()
    }
}

fn add(v: &mut Vec<Finding>, severity: Severity, code: &'static str, path: Option<String>, message: String) {
    v.push(Finding { severity, code, path, message });
}

fn undecodable(raw: &[u8]) -> bool {
    encoding_rs::EUC_KR.decode_without_bom_handling_and_without_replacement(raw).is_none()
}

fn walk_names(d: &Dir, prefix: &str, f: &mut dyn FnMut(&str, &[u8], bool)) {
    let here = if d.name.is_empty() { prefix.to_string() } else { format!("{prefix}{}/", decode_name(&d.name)) };
    if !d.name.is_empty() {
        f(&here, &d.name, true);
    }
    for e in &d.files {
        f(&format!("{here}{}", decode_name(&e.name)), &e.name, false);
    }
    for c in &d.dirs {
        walk_names(c, &here, f);
    }
}

/// Collisions inside one folder: distinct names with the same `name_hash` (the client shadows the earlier one too).
fn hash_collisions(d: &Dir, prefix: &str, out: &mut Vec<(String, String)>) {
    let here = if d.name.is_empty() { prefix.to_string() } else { format!("{prefix}{}/", decode_name(&d.name)) };
    let mut by: HashMap<u32, Vec<&Entry>> = HashMap::new();
    for e in &d.files {
        by.entry(name_hash(&e.name)).or_default().push(e);
    }
    let mut keys: Vec<&u32> = by.keys().collect();
    keys.sort();
    for k in keys {
        let g = &by[k];
        if g.len() > 1 && g.iter().any(|e| !e.name.eq_ignore_ascii_case(&g[0].name)) {
            let names: Vec<String> = g.iter().map(|e| decode_name(&e.name)).collect();
            out.push((format!("{here}{}", names[names.len() - 1]), format!("{} share one name hash ({k:#010x})", names.join(", "))));
        }
    }
    for c in &d.dirs {
        hash_collisions(c, &here, out);
    }
}

/// Index only; no SAF reads. `saf_len` None = `data.saf` not found.
pub fn check_index(index: &Index, saf_len: Option<u64>) -> CheckReport {
    let all = index.entries();
    let layout = layout::analyse(index, saf_len);
    let mut findings = Vec::new();
    let visible_total = all.iter().filter(|f| !f.shadowed).count();

    // out of bounds
    for p in &layout.out_of_bounds {
        let size = saf_len.unwrap_or(0);
        add(&mut findings, Severity::Problem, "out_of_bounds", Some(p.clone()), format!("lies outside data.saf ({})", counted(size, "byte", "bytes")));
    }
    if layout.out_of_bounds_count > layout.out_of_bounds.len() {
        add(&mut findings, Severity::Problem, "out_of_bounds", None, format!("and {} outside data.saf", counted((layout.out_of_bounds_count - layout.out_of_bounds.len()) as u64, "more entry", "more entries")));
    }

    // overlaps: partial overlaps are warnings, exactly shared ranges are info
    let partial = layout.overlaps.saturating_sub(layout.shared_ranges);
    if partial > 0 {
        add(&mut findings, Severity::Warning, "overlap", None, format!("{} the data of an earlier entry (partial overlap)", if partial == 1 { "1 entry starts inside".to_string() } else { format!("{partial} entries start inside") }));
    }
    if layout.shared_ranges > 0 {
        add(&mut findings, Severity::Info, "overlap", None, format!("{} exactly the same data range as another entry", if layout.shared_ranges == 1 { "1 entry shares".to_string() } else { format!("{} entries share", layout.shared_ranges) }));
    }

    // shadowed entries: name the visible one
    let mut visible_by_dir: HashMap<(Vec<Vec<u8>>, u32), &str> = HashMap::new();
    for f in all.iter().filter(|f| !f.shadowed) {
        let mut dir_segs = f.segments.clone();
        let name = dir_segs.pop().unwrap_or_default();
        visible_by_dir.insert((dir_segs.iter().map(|s| s.to_ascii_lowercase()).collect(), name_hash(&name)), &f.path);
    }
    for f in all.iter().filter(|f| f.shadowed) {
        let mut dir_segs = f.segments.clone();
        let name = dir_segs.pop().unwrap_or_default();
        let key = (dir_segs.iter().map(|s| s.to_ascii_lowercase()).collect::<Vec<_>>(), name_hash(&name));
        let msg = match visible_by_dir.get(&key) {
            Some(v) => format!("hidden by a later entry with the same name: {v}"),
            None => "hidden by a later folder or entry with the same name".to_string(),
        };
        add(&mut findings, Severity::Warning, "shadowed", Some(f.path.clone()), msg);
    }
    let mut coll = Vec::new();
    hash_collisions(&index.root, "", &mut coll);
    for (p, m) in coll {
        add(&mut findings, Severity::Warning, "hash_collision", Some(p), m);
    }

    // names
    walk_names(&index.root, "", &mut |display, raw, _is_dir| {
        if undecodable(raw) {
            add(&mut findings, Severity::Warning, "undecodable_name", Some(display.to_string()), "the name is not valid code page 949 text".to_string());
        }
    });
    let mut escaped = 0usize;
    for f in &all {
        if f.segments.iter().any(|s| portable_name(&decode_name(s)).is_some()) {
            escaped += 1;
            if escaped <= 100 {
                add(&mut findings, Severity::Info, "escaped_name", Some(f.path.clone()), "would be renamed when extracted (a character Windows does not allow)".to_string());
            }
        }
    }

    // informational
    add(&mut findings, Severity::Info, "zero_length", None, counted(layout.zero_length as u64, "file", "files"));
    match saf_len {
        None => add(&mut findings, Severity::Info, "saf_missing", None, "data.saf not found; checked the index only".to_string()),
        Some(n) => {
            let dead = layout.dead_bytes.unwrap_or(0);
            let top: Vec<String> = layout.largest_gaps.iter().take(5).map(|(o, b)| format!("{} at {o}", human_size(*b))).collect();
            let pct = if n > 0 { dead as f64 * 100.0 / n as f64 } else { 0.0 };
            add(
                &mut findings,
                Severity::Info,
                "dead_space",
                None,
                format!("{} unused ({pct:.1} %) in {}{}", human_size(dead), counted(layout.gap_count as u64, "gap", "gaps"), if top.is_empty() { String::new() } else { format!("; largest: {}", top.join(", ")) }),
            );
        }
    }
    if !index.free.is_empty() {
        let outside = saf_len.map(|n| index.free.iter().filter(|e| e.offset.saturating_add(e.length as u64) > n).count());
        add(
            &mut findings,
            Severity::Info,
            "free_list",
            None,
            match outside {
                Some(o) => format!("{}, {} outside data.saf (the client ignores the list)", counted(index.free.len() as u64, "entry", "entries"), if o == 1 { "1 lies".to_string() } else { format!("{o} lie") }),
                None => format!("{} (the client ignores the list)", counted(index.free.len() as u64, "entry", "entries")),
            },
        );
    }
    add(&mut findings, Severity::Info, "trailer", None, shaiya_core::hash::to_hex(&index.trailer));

    CheckReport {
        sah_version: index.version,
        files: all.len(),
        visible: visible_total,
        dirs: index.dir_count(),
        free_entries: index.free.len(),
        trailer: index.trailer.clone(),
        layout,
        findings,
    }
}

/// `check_index` plus, with `deep`, one streaming read of every visible entry (read errors become `unreadable_entry` problems).
pub fn check_archive(archive: &Archive, opts: &CheckOptions, job: &Job) -> Result<CheckReport> {
    job.stage("checking");
    let mut report = check_index(archive.index(), Some(archive.saf_len()));
    if opts.deep {
        let vis = archive.index().visible();
        let total: u64 = vis.iter().map(|f| f.entry.length as u64).sum();
        job.stage("checking");
        job.total(Some(total), Unit::Bytes);
        let mut buf = vec![0u8; 1 << 20];
        for f in vis {
            job.check()?;
            let read = (|| -> Result<()> {
                let mut r = archive.reader(f.entry)?;
                loop {
                    job.check()?;
                    let n = r.read(&mut buf).map_err(shaiya_core::Error::from)?;
                    if n == 0 {
                        break;
                    }
                    job.advance(n as u64);
                }
                Ok(())
            })();
            match read {
                Ok(()) => {}
                Err(shaiya_core::Error::Cancelled) => return Err(shaiya_core::Error::Cancelled),
                Err(e) => {
                    // an entry that already failed the bounds check is reported once, as out_of_bounds
                    if !report.findings.iter().any(|x| x.code == "out_of_bounds" && x.path.as_deref() == Some(&f.path)) {
                        add(&mut report.findings, Severity::Problem, "unreadable_entry", Some(f.path.clone()), e.to_string());
                    }
                }
            }
        }
    }
    Ok(report)
}

/// `1 entry` / `2 entries`.
fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dir;
    use shaiya_testkit::{SahBuilder, TempDir};

    fn en(n: &str, offset: u64, length: u32) -> Entry {
        Entry { name: n.as_bytes().to_vec(), offset, length, version: 1 }
    }

    /// A SAH that produces every finding code once.
    fn messy() -> (Index, u64) {
        let mut i = Index::new();
        i.free.push(en("rest", 500, 5));
        // out of bounds, partial overlap, exactly shared, zero length, shadowed, hash collision, bad name, escapable name
        i.root.dirs.push(Dir {
            name: b"d".to_vec(),
            files: vec![
                en("a.bin", 0, 10),
                en("b.bin", 5, 10),   // partial overlap with a.bin
                en("c.bin", 20, 10),
                en("c2.bin", 20, 10), // exactly shared with c.bin
                en("z.bin", 40, 0),
                en("S.txt", 50, 1),
                en("s.TXT", 51, 1),   // shadows S.txt
                en("oob.bin", 90, 20), // past a 100-byte data.saf
                Entry { name: vec![b'x', 0xFF, b'y'], offset: 60, length: 1, version: 1 },
                en("a:b.txt", 70, 1),
            ],
            dirs: vec![],
        });
        (i, 100)
    }

    #[test]
    fn every_finding_code_is_produced_by_a_built_index() {
        let (idx, saf) = messy();
        let r = check_index(&idx, Some(saf));
        let codes: std::collections::BTreeSet<&str> = r.findings.iter().map(|f| f.code).collect();
        for want in ["out_of_bounds", "overlap", "shadowed", "undecodable_name", "escaped_name", "zero_length", "dead_space", "free_list", "trailer"] {
            assert!(codes.contains(want), "missing {want}: {codes:?}");
        }
        assert!(!codes.contains("saf_missing"));
        assert_eq!((r.files, r.visible, r.dirs, r.free_entries), (10, 9, 1, 1));
        assert_eq!(r.count("shadowed"), 1);
        assert_eq!(r.problems(), 1);
        let sh = r.findings.iter().find(|f| f.code == "shadowed").unwrap();
        assert_eq!(sh.path.as_deref(), Some("d/S.txt"));
        assert!(sh.message.contains("d/s.TXT"), "{}", sh.message);
        assert!(r.findings.iter().any(|f| f.code == "overlap" && f.severity == Severity::Warning));
        assert!(r.findings.iter().any(|f| f.code == "overlap" && f.severity == Severity::Info));
        let fl = r.findings.iter().find(|f| f.code == "free_list").unwrap();
        assert!(fl.message.contains("1 entry, 1 lies outside"), "{}", fl.message);
        // no saf: info only, no dead space
        let r = check_index(&idx, None);
        assert!(r.findings.iter().any(|f| f.code == "saf_missing") && r.problems() == 0);
        assert!(!r.findings.iter().any(|f| f.code == "dead_space"));
    }

    #[test]
    fn hash_collisions_between_different_names_are_reported() {
        // 97*33 + 97 == 98*33 + 64: "aa" and "b@" have the same hash
        let n1 = b"aa".to_vec();
        let n2 = b"b@".to_vec();
        assert_eq!(crate::name_hash(&n1), crate::name_hash(&n2));
        let mut i = Index::new();
        i.root.files = vec![Entry { name: n1, offset: 0, length: 1, version: 0 }, Entry { name: n2, offset: 1, length: 1, version: 0 }];
        let r = check_index(&i, Some(2));
        assert_eq!(r.count("hash_collision"), 1);
        assert_eq!(r.count("shadowed"), 1, "the client shadows the earlier one too");
    }

    #[test]
    fn a_clean_archive_has_no_problems_and_no_warnings() {
        let d = TempDir::new("check-clean");
        SahBuilder::new(0).file(b"a/b.txt", b"hello").file(b"c.txt", b"world").write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        let r = check_archive(&a, &CheckOptions { deep: true }, &Job::silent()).unwrap();
        assert_eq!((r.problems(), r.warnings()), (0, 0));
        assert_eq!(r.layout.dead_bytes, Some(0));
        assert_eq!(r.layout.gap_count, 0);
        assert_eq!(r.trailer, vec![0; 4]);
    }

    #[test]
    fn deep_check_reports_unreadable_entries() {
        let d = TempDir::new("check-deep");
        SahBuilder::new(0).file(b"a.txt", b"hello").file(b"b.txt", b"world").write_to(d.path());
        // cut the SAF short: b.txt now lies outside
        std::fs::write(d.path().join("data.saf"), b"hello").unwrap();
        let a = Archive::open(d.path()).unwrap();
        let shallow = check_archive(&a, &CheckOptions::default(), &Job::silent()).unwrap();
        assert_eq!(shallow.count("out_of_bounds"), 1);
        let deep = check_archive(&a, &CheckOptions { deep: true }, &Job::silent()).unwrap();
        assert_eq!(deep.problems(), 1, "reported once, not twice");
    }
}
