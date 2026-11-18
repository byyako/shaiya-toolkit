//! Compare folders: every map id of two folders, same / differs / only here / only there /
//! unreadable. Pure: files are read, nothing is written. "Here" is the folder being edited, "there" the other one. The rows count
//! what the open map has that the other lacks (`+`), what the other has that it lacks (`-`) and what differs (`~`), as the
//! Changes tab does when it compares against the other folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use shaiya_svmap::{SectionId, Svmap};

use crate::compare;


/// Per-section counts of a map that differs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// (section, records only here, records only there, records that differ), sections with a difference only.
    pub sections: Vec<(SectionId, usize, usize, usize)>,
    /// Blocked cells that differ.
    pub cells: u64,
    /// (there, here) when the sizes differ.
    pub size: Option<(u32, u32)>,
}

/// The five outcomes, for the filter chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Same,
    Differs,
    OnlyHere,
    OnlyThere,
    Unreadable,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Differs, Kind::OnlyHere, Kind::OnlyThere, Kind::Unreadable, Kind::Same];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Same => "same",
            Kind::Differs => "differs",
            Kind::OnlyHere => "only here",
            Kind::OnlyThere => "only there",
            Kind::Unreadable => "unreadable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Same,
    Differs(Counts),
    OnlyHere,
    OnlyThere,
    Unreadable(String),
}

impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Status::Same => "same",
            Status::Differs(_) => "differs",
            Status::OnlyHere => "only here",
            Status::OnlyThere => "only there",
            Status::Unreadable(_) => "unreadable",
        }
    }

    /// The five kinds as the filter chips use them (same order as [`Kind`]).
    pub fn kind(&self) -> Kind {
        match self {
            Status::Same => Kind::Same,
            Status::Differs(_) => Kind::Differs,
            Status::OnlyHere => Kind::OnlyHere,
            Status::OnlyThere => Kind::OnlyThere,
            Status::Unreadable(_) => Kind::Unreadable,
        }
    }
}

/// One map id of either folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: u32,
    pub here: Option<PathBuf>,
    pub there: Option<PathBuf>,
    pub status: Status,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub same: usize,
    pub differs: usize,
    pub only_here: usize,
    pub only_there: usize,
    pub unreadable: usize,
}

impl Totals {
    /// "73 same · 3 differ · 1 only here · 1 only there" (kinds with none are left out, except "same").
    pub fn text(&self) -> String {
        let mut v = vec![format!("{} same", self.same)];
        if self.differs > 0 {
            v.push(format!("{} differ", self.differs));
        }
        if self.only_here > 0 {
            v.push(format!("{} only here", self.only_here));
        }
        if self.only_there > 0 {
            v.push(format!("{} only there", self.only_there));
        }
        if self.unreadable > 0 {
            v.push(format!("{} unreadable", self.unreadable));
        }
        v.join(" \u{b7} ")
    }

    /// Whether anything is not "same".
    pub fn any(&self) -> bool {
        self.differs + self.only_here + self.only_there + self.unreadable > 0
    }
}

pub fn totals(rows: &[Row]) -> Totals {
    let mut t = Totals::default();
    for r in rows {
        match r.status {
            Status::Same => t.same += 1,
            Status::Differs(_) => t.differs += 1,
            Status::OnlyHere => t.only_here += 1,
            Status::OnlyThere => t.only_there += 1,
            Status::Unreadable(_) => t.unreadable += 1,
        }
    }
    t
}

fn section_word(s: SectionId) -> &'static str {
    match s {
        SectionId::SectionA => "A",
        SectionId::SectionB => "B",
        SectionId::MobAreas => "areas",
        SectionId::Npcs => "npcs",
        SectionId::Portals => "portals",
        SectionId::Binds => "binds",
        SectionId::Regions => "regions",
        _ => "other",
    }
}

impl Counts {
    /// "areas +2 -1 ~4 · npcs ~1 · cells 312" (+ only here, - only there, ~ differs).
    pub fn text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some((there, here)) = self.size {
            parts.push(format!("size {there} to {here}"));
        }
        for (s, plus, minus, tilde) in &self.sections {
            let mut v = Vec::new();
            if *plus > 0 {
                v.push(format!("+{plus}"));
            }
            if *minus > 0 {
                v.push(format!("-{minus}"));
            }
            if *tilde > 0 {
                v.push(format!("~{tilde}"));
            }
            parts.push(format!("{} {}", section_word(*s), v.join(" ")));
        }
        if self.cells > 0 {
            parts.push(format!("cells {}", self.cells));
        }
        if parts.is_empty() {
            "bytes differ".to_string()
        } else {
            parts.join(" \u{b7} ")
        }
    }
}

/// The per-section counts of `here` against `there`.
pub fn counts(there: &Svmap, here: &Svmap) -> Counts {
    let mut c = Counts::default();
    if there.size != here.size {
        c.size = Some((there.size, here.size));
    }
    for s in compare::LIST_SECTIONS {
        let rows = compare::section_rows(there, here, s);
        if rows.is_empty() {
            continue;
        }
        let n = |k: shaiya_svmap::diff::DiffKind| rows.iter().filter(|r| r.kind == k).count();
        c.sections.push((s, n(shaiya_svmap::diff::DiffKind::Added), n(shaiya_svmap::diff::DiffKind::Removed), n(shaiya_svmap::diff::DiffKind::Changed)));
    }
    if there.size == here.size {
        c.cells = there.blocked.differing_cells(&here.blocked).unwrap_or(0);
    }
    c
}

/// The `.svmap` files of a folder by map id (the file stem is the id; `.bak` and other files are not maps).
pub fn list(dir: &Path) -> BTreeMap<u32, PathBuf> {
    let mut out = BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x == "svmap") {
                continue;
            }
            if let Some(id) = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) {
                out.insert(id, p);
            }
        }
    }
    out
}

fn compare_pair(id: u32, here: &Path, there: &Path) -> Row {
    let row = |status| Row { id, here: Some(here.to_path_buf()), there: Some(there.to_path_buf()), status };
    let (a, b) = match (std::fs::read(here), std::fs::read(there)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) => return row(Status::Unreadable(format!("{}: {e}", here.display()))),
        (_, Err(e)) => return row(Status::Unreadable(format!("{}: {e}", there.display()))),
    };
    if a == b {
        return row(Status::Same);
    }
    let ma = match shaiya_svmap::read(&a) {
        Ok(m) => m,
        Err(e) => return row(Status::Unreadable(format!("{}: {e}", here.display()))),
    };
    let mb = match shaiya_svmap::read(&b) {
        Ok(m) => m,
        Err(e) => return row(Status::Unreadable(format!("{}: {e}", there.display()))),
    };
    row(Status::Differs(counts(&mb, &ma)))
}

/// Compares two folders: one row per map id of either, in id order. Reads on up to 4 threads.
pub fn scan(here: &Path, there: &Path) -> Vec<Row> {
    let (a, b) = (list(here), list(there));
    let ids: Vec<u32> = a.keys().chain(b.keys()).copied().collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    let next = AtomicUsize::new(0);
    let results = std::sync::Mutex::new(BTreeMap::<u32, Row>::new());
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(&id) = ids.get(k) else { break };
                let row = match (a.get(&id), b.get(&id)) {
                    (Some(x), Some(y)) => compare_pair(id, x, y),
                    (Some(x), None) => Row { id, here: Some(x.clone()), there: None, status: Status::OnlyHere },
                    (None, Some(y)) => Row { id, here: None, there: Some(y.clone()), status: Status::OnlyThere },
                    (None, None) => continue,
                };
                if let Ok(mut r) = results.lock() {
                    r.insert(id, row);
                }
            });
        }
    });
    results.into_inner().map(|m| m.into_values().collect()).unwrap_or_default()
}

/// The rows as the CLI prints them: `<id>  <status>  <detail>`, then a totals line.
pub fn report_lines(rows: &[Row], name: &dyn Fn(u32) -> String) -> Vec<String> {
    let mut v = Vec::new();
    for r in rows {
        let detail = match &r.status {
            Status::Differs(c) => c.text(),
            Status::Unreadable(e) => e.clone(),
            _ => String::new(),
        };
        let line = format!("{:>4}  {:<10}  {:<28}  {detail}", r.id, r.status.label(), name(r.id));
        v.push(line.trim_end().to_string());
    }
    v.push(totals(rows).text());
    v
}

// ------------------------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use shaiya_svmap::{MobArea, MobEntry};

    fn map_with(areas: usize) -> Svmap {
        let mut m = Svmap::new(64);
        for i in 0..areas {
            m.mob_areas.push(MobArea { min: [i as f32, 0.0, 0.0], max: [i as f32 + 2.0, 2.0, 2.0], mobs: vec![MobEntry { mob: 1, count: 1 }] });
        }
        m
    }

    fn put(dir: &Path, id: u32, m: &Svmap) {
        std::fs::write(dir.join(format!("{id}.svmap")), shaiya_svmap::write(m)).unwrap();
    }

    #[test]
    fn scan_classifies_every_id() {
        let (a, b) = (TempDir::new("fd-a"), TempDir::new("fd-b"));
        put(a.path(), 1, &map_with(2));
        put(b.path(), 1, &map_with(2));
        put(a.path(), 2, &map_with(3));
        put(b.path(), 2, &map_with(5));
        put(a.path(), 3, &map_with(1));
        put(b.path(), 4, &map_with(1));
        std::fs::write(a.join("5.svmap"), b"junk").unwrap();
        std::fs::write(b.join("5.svmap"), shaiya_svmap::write(&map_with(1))).unwrap();
        std::fs::write(a.join("6.svmap.bak"), b"x").unwrap();
        let rows = scan(a.path(), b.path());
        let ids: Vec<u32> = rows.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
        assert_eq!(rows[0].status, Status::Same);
        match &rows[1].status {
            Status::Differs(c) => assert_eq!(c.text(), "areas -2"),
            s => panic!("{s:?}"),
        }
        assert_eq!(rows[2].status, Status::OnlyHere);
        assert_eq!(rows[3].status, Status::OnlyThere);
        assert!(matches!(rows[4].status, Status::Unreadable(_)));
        let t = totals(&rows);
        assert_eq!((t.same, t.differs, t.only_here, t.only_there, t.unreadable), (1, 1, 1, 1, 1));
        assert!(t.text().starts_with("1 same"));
        assert!(t.any());
    }

    /// A copy of the 77 originals with 3 maps edited, one deleted and one added: 73 same, 3 differ, 1 only here, 1 only there.
    #[test]
    fn folder_diff_originals_vs_copy() {
        let Some(dir) = std::env::var_os("SVMAP_ORIGINALS") else {
            eprintln!("skipped: SVMAP_ORIGINALS not set");
            return;
        };
        let orig = PathBuf::from(dir);
        let copy = TempDir::new("fd-originals");
        for (id, p) in list(&orig) {
            std::fs::copy(p, copy.join(&format!("{id}.svmap"))).unwrap();
        }
        let edit = |id: u32, f: &dyn Fn(&mut Svmap)| {
            let p = copy.join(&format!("{id}.svmap"));
            let mut m = shaiya_svmap::read(&std::fs::read(&p).unwrap()).unwrap();
            f(&mut m);
            std::fs::write(&p, shaiya_svmap::write(&m)).unwrap();
        };
        edit(1, &|m| {
            m.mob_areas[3].min[0] += 5.0;
            m.mob_areas[3].max[0] += 5.0;
        });
        edit(35, &|m| {
            m.npcs.remove(0);
        });
        edit(2, &|m| m.portals[0].to_map = 18);
        std::fs::remove_file(copy.join("108.svmap")).unwrap();
        std::fs::write(copy.join("999.svmap"), shaiya_svmap::write(&map_with(2))).unwrap();
        let rows = scan(copy.path(), &orig);
        let t = totals(&rows);
        assert_eq!((t.same, t.differs, t.only_here, t.only_there, t.unreadable), (73, 3, 1, 1, 0), "{}", t.text());
        let text = |id: u32| match &rows.iter().find(|r| r.id == id).unwrap().status {
            Status::Differs(c) => c.text(),
            s => panic!("{id}: {s:?}"),
        };
        assert_eq!(text(1), "areas ~1");
        assert_eq!(text(35), "npcs -1");
        assert_eq!(text(2), "portals ~1");
        assert_eq!(rows.iter().find(|r| r.id == 999).unwrap().status, Status::OnlyHere);
        assert_eq!(rows.iter().find(|r| r.id == 108).unwrap().status, Status::OnlyThere);
        assert!(rows.windows(2).all(|w| w[0].id < w[1].id), "id order");
    }

    #[test]
    fn counts_text_reads_plus_minus_tilde_and_cells() {
        let there = map_with(3);
        let mut here = map_with(3);
        here.mob_areas[0].min[0] += 5.0;
        here.mob_areas.push(map_with(1).mob_areas[0].clone());
        here.blocked.set(1, 1, true);
        let c = counts(&there, &here);
        assert_eq!(c.text(), "areas +1 ~1 \u{b7} cells 1");
        assert_eq!(counts(&here, &there).text(), "areas -1 ~1 \u{b7} cells 1");
    }
}
