//! Paired documents. A map tab holds the svmap and, when the client is
//! open, an editable copy of the map's client `.wld` (the lossless client model) and its `world/<id>.txt`. An edit of an NPC, a
//! portal, a region or a bind area changes both in one undo step.
//!
//! The pairing is per section and by count: a section is paired while the svmap and the `.wld` list have the same number of
//! records, and then record `i` of one is record `i` of the other. A section with other counts is unpaired and its edits stay on
//! the svmap side (Sync opens).
//!
//! Everything here is plain data: [`mirror`] turns the svmap ops of a step into client ops ([`COp`]) against a scratch copy of
//! the client lists, the doc applies both lists of ops in one step, and undo applies the inverse of each. A mirrored edit only
//! copies the fields the svmap edit changed, so a client value the svmap never held (a portal's raw name, an area's picture) is
//! never touched, and an unrelated edit never overwrites a difference that already existed between the two sides.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use shaiya_clientdata::area_txt::AreaTxt;
use shaiya_clientdata::text::AreaNames;
use shaiya_clientdata::wld::{Placement, Wld, WldNpc, ZoneBox, ZoneGroup};
use shaiya_clientdata::wld_raw::{Codepage, Name256, RawArea, RawPlaced, RawPortal};
use shaiya_svmap::{Bind, NpcEntry, Portal, Region, SectionId, Svmap};

use crate::client_files::ClientWld;
use crate::client_rules::ClientRules;
use crate::edit::{Op, OpKind, Rec};
use crate::place::PlaceList;
use crate::terrain::TerrainDiff;

/// The four paired sections, in the order of `sync::SECTIONS`.
pub const SECTIONS: [SectionId; 4] = crate::sync::SECTIONS;

/// Index of a section in [`SECTIONS`]; `None` for a section the client does not mirror.
pub fn slot(s: SectionId) -> Option<usize> {
    SECTIONS.iter().position(|x| *x == s)
}

// ---------------------------------------------------------------------------------------------------------------- lists

/// The client records a paired edit can change, with the bytes of `world/<id>.txt`. A scratch copy of these is what [`mirror`]
/// works on, so the real model is only touched when a step is applied.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lists {
    pub portals: Vec<RawPortal>,
    pub npcs: Vec<WldNpc>,
    pub areas: Vec<RawArea>,
    pub zones: Vec<ZoneGroup>,
    pub txt: Vec<u8>,
    /// The four placement lists, in the order of [`PlaceList::ALL`]. Empty in the copies the paired-record mirroring
    /// works on (it never reads them).
    pub places: [RawPlaced; 4],
    /// The terrain corner heights; empty in the copies the paired-record mirroring works on, and for a dungeon.
    pub terrain: Vec<i16>,
}

impl Lists {
    pub fn count(&self, s: SectionId) -> usize {
        match s {
            SectionId::Npcs => self.npcs.len(),
            SectionId::Portals => self.portals.len(),
            SectionId::Regions => self.areas.len(),
            SectionId::Binds => self.zones.len(),
            _ => 0,
        }
    }
}

/// One change of one list.
#[derive(Clone, Debug, PartialEq)]
pub enum CKind<T> {
    Replace { before: T, after: T },
    Insert(T),
    Remove(T),
}

impl<T: Clone> CKind<T> {
    fn invert(&self) -> CKind<T> {
        match self {
            CKind::Replace { before, after } => CKind::Replace { before: after.clone(), after: before.clone() },
            CKind::Insert(r) => CKind::Remove(r.clone()),
            CKind::Remove(r) => CKind::Insert(r.clone()),
        }
    }

    fn apply(&self, v: &mut Vec<T>, index: usize) {
        match self {
            CKind::Replace { after, .. } => {
                if let Some(x) = v.get_mut(index) {
                    *x = after.clone();
                }
            }
            CKind::Insert(r) => v.insert(index.min(v.len()), r.clone()),
            CKind::Remove(_) => {
                if index < v.len() {
                    v.remove(index);
                }
            }
        }
    }
}

/// One client operation. A paired step holds the svmap ops and these; undo applies the inverse of each.
#[derive(Clone, Debug, PartialEq)]
pub enum COp {
    Portal(usize, CKind<RawPortal>),
    Npc(usize, CKind<WldNpc>),
    Area(usize, CKind<RawArea>),
    Zone(usize, CKind<ZoneGroup>),
    /// One placement of a building, shape, tree or dungeon list.
    Place(PlaceList, usize, CKind<Placement>),
    /// One name of such a list. Names are only ever appended (and the append undone), so no placement index shifts.
    PlaceName(PlaceList, usize, CKind<Name256>),
    /// The corners one terrain stroke changed: index, before, after. Applying writes the `after` values, so applying
    /// a stroke whose corners already hold them (a stroke is drawn live) changes nothing more.
    Terrain(TerrainDiff),
    /// The whole `world/<id>.txt` (a name edit, or the numbers of the lines after an inserted or removed area).
    Txt { before: Vec<u8>, after: Vec<u8> },
    /// All four lists and the text at once (restoring recovered work).
    Lists { before: Box<Lists>, after: Box<Lists> },
}

impl COp {
    pub fn invert(&self) -> COp {
        match self {
            COp::Portal(i, k) => COp::Portal(*i, k.invert()),
            COp::Npc(i, k) => COp::Npc(*i, k.invert()),
            COp::Area(i, k) => COp::Area(*i, k.invert()),
            COp::Zone(i, k) => COp::Zone(*i, k.invert()),
            COp::Place(l, i, k) => COp::Place(*l, *i, k.invert()),
            COp::PlaceName(l, i, k) => COp::PlaceName(*l, *i, k.invert()),
            COp::Terrain(d) => COp::Terrain(d.invert()),
            COp::Txt { before, after } => COp::Txt { before: after.clone(), after: before.clone() },
            COp::Lists { before, after } => COp::Lists { before: after.clone(), after: before.clone() },
        }
    }

    /// A replace of one record (what a merged typing step can hold).
    pub fn is_replace(&self) -> bool {
        matches!(
            self,
            COp::Portal(_, CKind::Replace { .. }) | COp::Npc(_, CKind::Replace { .. }) | COp::Area(_, CKind::Replace { .. }) | COp::Zone(_, CKind::Replace { .. }) | COp::Place(_, _, CKind::Replace { .. }) | COp::Txt { .. }
        )
    }

    /// Same list and index (the merge test of a coalesced step).
    fn same_target(&self, o: &COp) -> bool {
        match (self, o) {
            (COp::Portal(a, _), COp::Portal(b, _)) | (COp::Npc(a, _), COp::Npc(b, _)) | (COp::Area(a, _), COp::Area(b, _)) | (COp::Zone(a, _), COp::Zone(b, _)) => a == b,
            (COp::Place(la, a, _), COp::Place(lb, b, _)) => la == lb && a == b,
            (COp::Txt { .. }, COp::Txt { .. }) => true,
            _ => false,
        }
    }

    /// Takes the `after` of `newer` into this op (both replace the same record): this op keeps its `before`.
    fn merge_after(&mut self, newer: &COp) {
        match (self, newer) {
            (COp::Portal(_, CKind::Replace { after, .. }), COp::Portal(_, CKind::Replace { after: n, .. })) => *after = n.clone(),
            (COp::Npc(_, CKind::Replace { after, .. }), COp::Npc(_, CKind::Replace { after: n, .. })) => *after = n.clone(),
            (COp::Area(_, CKind::Replace { after, .. }), COp::Area(_, CKind::Replace { after: n, .. })) => *after = n.clone(),
            (COp::Zone(_, CKind::Replace { after, .. }), COp::Zone(_, CKind::Replace { after: n, .. })) => *after = n.clone(),
            (COp::Place(_, _, CKind::Replace { after, .. }), COp::Place(_, _, CKind::Replace { after: n, .. })) => *after = *n,
            (COp::Txt { after, .. }, COp::Txt { after: n, .. }) => *after = n.clone(),
            _ => {}
        }
    }

    /// Whether this op leaves the record as it was (a merged step that came back to its start).
    fn is_noop(&self) -> bool {
        match self {
            COp::Portal(_, CKind::Replace { before, after }) => before == after,
            COp::Npc(_, CKind::Replace { before, after }) => before == after,
            COp::Area(_, CKind::Replace { before, after }) => before == after,
            COp::Zone(_, CKind::Replace { before, after }) => before == after,
            COp::Place(_, _, CKind::Replace { before, after }) => before == after,
            COp::Txt { before, after } => before == after,
            _ => false,
        }
    }

    /// Applies the op to the five containers.
    pub fn apply_to(&self, l: &mut Lists) {
        match self {
            COp::Portal(i, k) => k.apply(&mut l.portals, *i),
            COp::Npc(i, k) => k.apply(&mut l.npcs, *i),
            COp::Area(i, k) => k.apply(&mut l.areas, *i),
            COp::Zone(i, k) => k.apply(&mut l.zones, *i),
            COp::Place(pl, i, k) => k.apply(&mut l.places[pl.slot()].items, *i),
            COp::PlaceName(pl, i, k) => k.apply(&mut l.places[pl.slot()].names, *i),
            COp::Terrain(d) => {
                for (i, _, a) in &d.cells {
                    if let Some(h) = l.terrain.get_mut(*i as usize) {
                        *h = *a;
                    }
                }
            }
            COp::Txt { after, .. } => l.txt = after.clone(),
            COp::Lists { after, .. } => *l = (**after).clone(),
        }
    }
}

/// Whether every op of the two lists is a replace at the same place, so the newer list can merge into the older one.
pub fn mergeable(old: &[COp], new: &[COp]) -> bool {
    old.len() == new.len() && old.iter().zip(new).all(|(a, b)| a.is_replace() && a.same_target(b))
}

/// Merges `new` into `old` (see [`mergeable`]); true when the result changes nothing (the step can go).
pub fn merge_into(old: &mut [COp], new: &[COp]) -> bool {
    for (a, b) in old.iter_mut().zip(new) {
        a.merge_after(b);
    }
    old.iter().all(COp::is_noop)
}

// ---------------------------------------------------------------------------------------------------------------- the part

/// The client half of a tab: the lossless `.wld` model, the text of `<id>.txt` and what they were when read or last saved.
pub struct ClientPart {
    pub wld: ClientWld,
    /// Bytes of `world/<id>.txt` as it is now (empty when the client has none).
    pub txt: Vec<u8>,
    txt_saved: Vec<u8>,
    txt_existed: bool,
    /// The lists as read or last saved (the Changes tab counts the differences).
    pub base: Lists,
    /// Names the user types are encoded in this page (the setting `client_codepage`).
    pub cp: Codepage,
    rev: u64,
    /// Counts the placement ops applied or undone (what the 3D scene and the 2D dots follow).
    place_rev: u64,
    /// Which terrain corners changed when: the views redraw only these.
    terrain_log: TerrainLog,
    /// The number of corners that differ from `base`, with the (terrain revision, base buffer) it was counted for: the Changes tab asks every frame.
    terr_changed: Mutex<Option<(u64, usize, usize)>>,
    /// The corners a terrain stroke in progress has drawn (no step yet), with their value before it.
    live: HashMap<u32, i16>,
    /// Unique per part read (see [`version`](Self::version)).
    serial: u64,
    wld_cache: Mutex<Option<(u64, Arc<Wld>)>>,
    names: AreaNames,
}

/// What the terrain views have to redraw: a rectangle of corners per change, newest last.
#[derive(Debug, Default)]
struct TerrainLog {
    rev: u64,
    entries: std::collections::VecDeque<(u64, [u32; 4])>,
}

/// What changed in the terrain since a view last looked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainDirty {
    Nothing,
    /// The corners `[cx0, cz0, cx1, cz1]` (inclusive).
    Rect([u32; 4]),
    /// Too much happened since (or the log does not reach back): redraw everything.
    Full,
}

impl TerrainLog {
    const KEEP: usize = 512;

    fn push(&mut self, r: [u32; 4]) {
        self.rev += 1;
        self.entries.push_back((self.rev, r));
        while self.entries.len() > Self::KEEP {
            self.entries.pop_front();
        }
    }

    fn since(&self, seen: u64) -> TerrainDirty {
        if seen >= self.rev {
            return TerrainDirty::Nothing;
        }
        match self.entries.front() {
            Some((first, _)) if *first <= seen + 1 => {}
            _ => return TerrainDirty::Full,
        }
        let mut r: Option<[u32; 4]> = None;
        for (rev, e) in &self.entries {
            if *rev > seen {
                r = Some(match r {
                    None => *e,
                    Some(o) => [o[0].min(e[0]), o[1].min(e[1]), o[2].max(e[2]), o[3].max(e[3])],
                });
            }
        }
        r.map_or(TerrainDirty::Nothing, TerrainDirty::Rect)
    }
}

fn next_serial() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// What attaching a client part found.
pub enum Attach {
    Part(Box<ClientPart>),
    /// The `.wld` is there but does not write back byte for byte (or does not parse): the tab edits the svmap only.
    ReadOnly(String),
    /// The client has no `.wld` for this map.
    Absent,
}

impl ClientPart {
    /// Reads `world/<id>.wld` and `world/<id>.txt` (a staged copy first) through the client rules.
    pub fn attach(rules: &ClientRules, id: u32, cp: Codepage) -> Attach {
        let rel = format!("world/{id}.wld");
        let staged = rules.staging().is_some_and(|out| crate::client_files::staged_path(&out, &rel).is_file());
        if !staged && !rules.source.exists(&rel) {
            return Attach::Absent;
        }
        let wld = match ClientWld::open(rules, id) {
            Ok(w) => w,
            Err(why) => return Attach::ReadOnly(why),
        };
        let (txt, existed) = match rules.read_file(&format!("world/{id}.txt")) {
            Ok((b, _)) => (b, true),
            Err(shaiya_clientdata::Error::NotFound(_)) => (Vec::new(), false),
            Err(e) => return Attach::ReadOnly(format!("world/{id}.txt cannot be read: {e}")),
        };
        let mut p = ClientPart { wld, txt_saved: txt.clone(), txt, txt_existed: existed, base: Lists::default(), cp, rev: 0, place_rev: 0, terrain_log: TerrainLog::default(), terr_changed: Mutex::new(None), live: HashMap::new(), serial: next_serial(), wld_cache: Mutex::new(None), names: AreaNames::default() };
        p.names = AreaTxt::parse(&p.txt).names();
        p.base = p.lists();
        Attach::Part(Box::new(p))
    }

    /// A part over a model that is already in memory (tests).
    #[cfg(any(test, feature = "testing"))]
    pub fn from_model(wld: ClientWld, txt: Option<Vec<u8>>) -> ClientPart {
        let (txt, existed) = match txt {
            Some(t) => (t, true),
            None => (Vec::new(), false),
        };
        let mut p = ClientPart { wld, txt_saved: txt.clone(), txt, txt_existed: existed, base: Lists::default(), cp: Codepage::Cp949, rev: 0, place_rev: 0, terrain_log: TerrainLog::default(), terr_changed: Mutex::new(None), live: HashMap::new(), serial: next_serial(), wld_cache: Mutex::new(None), names: AreaNames::default() };
        p.names = AreaTxt::parse(&p.txt).names();
        p.base = p.lists();
        p
    }

    /// Which part and which state of it (the second number changes with every applied or undone client op): two halves read
    /// one after the other never give the same pair. Autosave writes the client files of a recovery entry only when this
    /// changed; the fixes cache their sync target by it.
    pub fn version(&self) -> (u64, u64) {
        (self.serial, self.rev)
    }

    /// Everything an edit can change, the placement lists included (a copy: used for the saved base, recovery and revert).
    pub fn lists(&self) -> Lists {
        let mut l = self.lists_core();
        let m = &self.wld.model;
        l.places = PlaceList::ALL.map(|pl| pl.list(m).clone());
        l.terrain = m.terrain.as_ref().map(|t| t.heights.clone()).unwrap_or_default();
        l
    }

    /// The portal, NPC, area and zone lists and the text, without the placements: what the paired-record mirroring and Sync work on
    /// (a copy of thousands of placements for every portal edit would be waste).
    pub fn lists_core(&self) -> Lists {
        let m = &self.wld.model;
        Lists { portals: m.portals.clone(), npcs: m.npcs.clone(), areas: m.areas.clone(), zones: m.zone_groups.clone(), txt: self.txt.clone(), places: Default::default(), terrain: Vec::new() }
    }

    /// Changes with every terrain op applied, undone or drawn live (never goes back).
    pub fn terrain_rev(&self) -> u64 {
        self.terrain_log.rev
    }

    /// The corner grid of a field map.
    pub fn terrain(&self) -> Option<&shaiya_clientdata::wld_raw::RawTerrain> {
        self.wld.model.terrain.as_ref()
    }

    /// What changed since `seen` (a value of [`terrain_rev`](Self::terrain_rev)).
    pub fn terrain_dirty_since(&self, seen: u64) -> TerrainDirty {
        self.terrain_log.since(seen)
    }

    /// Edits the corner grid directly, without an undo step: a stroke in progress draws live this way, and its step is made when
    /// it ends (`COp::Terrain` then applies the same values again). `f` returns the corner rectangle it changed.
    /// `f` also gets the record of the corners drawn so far with their value before the stroke, and adds every corner it changes
    /// for the first time ([`revert_live`](Self::revert_live) puts them back).
    pub fn edit_terrain_live(&mut self, f: impl FnOnce(&mut shaiya_clientdata::wld_raw::RawTerrain, &mut HashMap<u32, i16>) -> Option<[u32; 4]>) {
        let Some(t) = self.wld.model.terrain.as_mut() else { return };
        if let Some(r) = f(t, &mut self.live) {
            self.rev += 1;
            self.terrain_log.push(r);
        }
    }

    /// Puts back the corners a stroke in progress drew (Esc, a cancelled stroke, or a terrain op of the history arriving during
    /// it). True when a corner changed.
    pub fn revert_live(&mut self) -> bool {
        if self.live.is_empty() {
            return false;
        }
        let live = std::mem::take(&mut self.live);
        let Some(t) = self.wld.model.terrain.as_mut() else { return false };
        let n = t.corners() as u32;
        let mut rect: Option<[u32; 4]> = None;
        for (i, v) in live {
            let Some(h) = t.heights.get_mut(i as usize) else { continue };
            if *h != v {
                *h = v;
                let (cx, cz) = (i % n, i / n);
                rect = Some(match rect {
                    None => [cx, cz, cx, cz],
                    Some(o) => [o[0].min(cx), o[1].min(cz), o[2].max(cx), o[3].max(cz)],
                });
            }
        }
        let Some(r) = rect else { return false };
        self.rev += 1;
        self.terrain_log.push(r);
        true
    }

    /// The stroke in progress becomes a step: its drawn corners are no longer put back by [`revert_live`](Self::revert_live).
    pub fn keep_live(&mut self) {
        self.live.clear();
    }

    /// Whether a stroke in progress has drawn corners that are no step yet.
    #[cfg(test)]
    pub fn has_live(&self) -> bool {
        !self.live.is_empty()
    }

    /// Changes with every placement op applied or undone.
    pub fn place_rev(&self) -> u64 {
        self.place_rev
    }

    pub fn count(&self, s: SectionId) -> usize {
        let m = &self.wld.model;
        match s {
            SectionId::Npcs => m.npcs.len(),
            SectionId::Portals => m.portals.len(),
            SectionId::Regions => m.areas.len(),
            SectionId::Binds => m.zone_groups.len(),
            _ => 0,
        }
    }

    /// Which sections are paired with `map`: the same number of records in both.
    pub fn paired(&self, map: &Svmap) -> [bool; 4] {
        let mut p = [false; 4];
        for (k, s) in SECTIONS.iter().enumerate() {
            p[k] = crate::edit::count(map, *s) == self.count(*s);
        }
        p
    }

    /// Applies one client op to the model.
    pub fn apply(&mut self, op: &COp) {
        if matches!(op, COp::Terrain(_) | COp::Lists { .. }) {
            // an undo, a redo or restored work that changes the terrain while a stroke is drawn: the stroke's corners go back
            // first, so the op meets exactly the heights of its history depth (the stroke notices and ends without a step)
            self.revert_live();
        }
        let m = &mut self.wld.model;
        match op {
            COp::Portal(i, k) => k.apply(&mut m.portals, *i),
            COp::Npc(i, k) => k.apply(&mut m.npcs, *i),
            COp::Area(i, k) => k.apply(&mut m.areas, *i),
            COp::Zone(i, k) => k.apply(&mut m.zone_groups, *i),
            COp::Place(pl, i, k) => {
                k.apply(&mut pl.list_mut(m).items, *i);
                self.place_rev += 1;
            }
            COp::PlaceName(pl, i, k) => {
                k.apply(&mut pl.list_mut(m).names, *i);
                self.place_rev += 1;
            }
            COp::Terrain(d) => {
                if let Some(t) = m.terrain.as_mut() {
                    let n = t.corners() as u32;
                    let mut rect: Option<[u32; 4]> = None;
                    for (i, _, a) in &d.cells {
                        let Some(h) = t.heights.get_mut(*i as usize) else { continue };
                        *h = *a;
                        let (cx, cz) = (i % n, i / n);
                        rect = Some(match rect {
                            None => [cx, cz, cx, cz],
                            Some(o) => [o[0].min(cx), o[1].min(cz), o[2].max(cx), o[3].max(cz)],
                        });
                    }
                    if let Some(r) = rect {
                        self.terrain_log.push(r);
                    }
                }
            }
            COp::Txt { after, .. } => {
                self.txt = after.clone();
                self.names = AreaTxt::parse(&self.txt).names();
            }
            COp::Lists { after, .. } => {
                m.portals = after.portals.clone();
                m.npcs = after.npcs.clone();
                m.areas = after.areas.clone();
                m.zone_groups = after.zones.clone();
                for pl in PlaceList::ALL {
                    *pl.list_mut(m) = after.places[pl.slot()].clone();
                }
                self.place_rev += 1;
                if let Some(t) = m.terrain.as_mut() {
                    if after.terrain.len() == t.heights.len() && after.terrain != t.heights {
                        t.heights = after.terrain.clone();
                        let n = t.corners() as u32;
                        self.terrain_log.push([0, 0, n - 1, n - 1]);
                    }
                }
                self.txt = after.txt.clone();
                self.names = AreaTxt::parse(&self.txt).names();
            }
        }
        self.rev += 1;
    }

    /// The model as the decoded `Wld` the client rules and the sync plan read (built once per change, names read as cp949 like
    /// every other `Wld` of the editor).
    pub fn wld_arc(&self) -> Arc<Wld> {
        let mut g = self.wld_cache.lock().unwrap_or_else(|p| p.into_inner());
        match &*g {
            Some((r, w)) if *r == self.rev => w.clone(),
            _ => {
                let w = Arc::new(self.wld.model.to_wld(Codepage::Cp949));
                *g = Some((self.rev, w.clone()));
                w
            }
        }
    }

    /// The area names of the text as it is now.
    pub fn names(&self) -> &AreaNames {
        &self.names
    }

    /// The destination name of client portal `i`, in the page of the setting.
    pub fn dest_name(&self, i: usize) -> Option<String> {
        self.wld.model.portals.get(i).map(|p| self.cp.decode(&p.dest_name))
    }

    /// The text differs from what was read or last saved.
    pub fn txt_changed(&self) -> bool {
        self.txt != self.txt_saved
    }

    /// Writes what changed to the staging folder through the safe save (`.tmp`, read back, compare, timestamped backup,
    /// `.bak`, atomic rename), the `.wld` first (when it changed, or `force_wld`) and then the text. Returns the paths written. On an error the model stays as it
    /// is and whatever was not written stays modified.
    pub fn save(&mut self, out: &std::path::Path, backups: Option<(&std::path::Path, u32)>, force_wld: bool) -> Result<Vec<std::path::PathBuf>, String> {
        let mut written = Vec::new();
        if force_wld || self.wld.dirty() {
            let s = self.wld.save(out, backups)?;
            written.push(s.path);
        }
        if self.txt_changed() {
            let rel = format!("world/{}.txt", self.wld.id);
            let target = crate::client_files::staged_path(out, &rel);
            crate::client_files::check_out_dir(out, self.wld.client_dir())?;
            let (stem, ext) = crate::client_files::backup_stem(&rel);
            let plan = backups.map(|(dir, keep)| crate::client_files::BackupPlan { dir, stem: &stem, ext: &ext, keep });
            let want = self.txt.clone();
            let verify = move |back: &[u8]| -> Result<(), String> {
                if AreaTxt::parse(back).to_bytes() != want {
                    return Err("the text does not read back as the model".to_string());
                }
                Ok(())
            };
            let s = crate::client_files::write_verified(&target, &self.txt, &verify, plan, Some(self.wld.client_dir()))?;
            self.txt_saved = self.txt.clone();
            self.txt_existed = true;
            written.push(s.path);
        }
        self.rebase();
        Ok(written)
    }

    /// The state now is the saved one (the Changes tab compares against it). Forgets the counted terrain changes too: their key
    /// holds the address of the old base's heights, which a later base could reuse.
    pub fn rebase(&mut self) {
        self.base = self.lists();
        *self.terr_changed.get_mut().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// One line per list that differs from the saved state, for the Changes tab.
    pub fn changes(&self) -> Vec<String> {
        let now = self.lists_core();
        let mut out = Vec::new();
        let mut list = |name: &str, a: usize, b: usize, differing: usize| {
            let mut parts = Vec::new();
            if differing > 0 {
                parts.push(format!("{differing} changed"));
            }
            if b > a {
                parts.push(format!("{} added", b - a));
            }
            if a > b {
                parts.push(format!("{} removed", a - b));
            }
            if !parts.is_empty() {
                out.push(format!("client {name}: {}", parts.join(", ")));
            }
        };
        fn diff<T: PartialEq>(a: &[T], b: &[T]) -> usize {
            a.iter().zip(b).filter(|(x, y)| x != y).count()
        }
        list("portals", self.base.portals.len(), now.portals.len(), diff(&self.base.portals, &now.portals));
        list("NPCs", self.base.npcs.len(), now.npcs.len(), diff(&self.base.npcs, &now.npcs));
        list("areas", self.base.areas.len(), now.areas.len(), diff(&self.base.areas, &now.areas));
        list("zone groups", self.base.zones.len(), now.zones.len(), diff(&self.base.zones, &now.zones));
        if now.txt != self.base.txt {
            out.push("client area names (the .txt file) changed".to_string());
        }
        if let Some(t) = self.wld.model.terrain.as_ref() {
            let key = (self.terrain_log.rev, self.base.terrain.as_ptr() as usize);
            let mut g = self.terr_changed.lock().unwrap_or_else(|p| p.into_inner());
            let d = match *g {
                Some((r, p, d)) if (r, p) == key => d,
                _ => {
                    let d = t.heights.iter().zip(&self.base.terrain).filter(|(a, b)| a != b).count();
                    *g = Some((key.0, key.1, d));
                    d
                }
            };
            if d > 0 {
                out.push(format!("client terrain: {d} corner{} changed", if d == 1 { "" } else { "s" }));
            }
        }
        for pl in PlaceList::ALL {
            let (a, b) = (&self.base.places[pl.slot()], pl.list(&self.wld.model));
            let names_added = b.names.len().saturating_sub(a.names.len());
            if a.items != b.items || names_added > 0 {
                let mut parts = Vec::new();
                let differing = diff(&a.items, &b.items);
                if differing > 0 {
                    parts.push(format!("{differing} changed"));
                }
                if b.items.len() > a.items.len() {
                    parts.push(format!("{} added", b.items.len() - a.items.len()));
                }
                if a.items.len() > b.items.len() {
                    parts.push(format!("{} removed", a.items.len() - b.items.len()));
                }
                if names_added > 0 {
                    parts.push(format!("{names_added} new name{}", if names_added == 1 { "" } else { "s" }));
                }
                if !parts.is_empty() {
                    out.push(format!("client {}: {}", pl.plural(), parts.join(", ")));
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------------------------- mirroring

fn f3_eq(a: [f32; 3], b: [f32; 3]) -> bool {
    a[0] == b[0] && a[1] == b[1] && a[2] == b[2]
}

/// Where the svmap portal that belongs to this box stands: centre (x, z), min y.
pub fn box_pos(p: &RawPortal) -> [f32; 3] {
    [(p.min[0] + p.max[0]) * 0.5, p.min[1], (p.min[2] + p.max[2]) * 0.5]
}

/// The box moved so its centre (x, z) and min y are `pos`; the size stays.
fn recentre(c: &RawPortal, pos: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let (hx, hz, h) = ((c.max[0] - c.min[0]) * 0.5, (c.max[2] - c.min[2]) * 0.5, c.max[1] - c.min[1]);
    ([pos[0] - hx, pos[1], pos[2] - hz], [pos[0] + hx, pos[1] + h, pos[2] + hz])
}

fn i16_of(v: u16) -> i16 {
    v.min(i16::MAX as u16) as i16
}

/// The client portal after the svmap portal changed from `b` to `a`. `b` = `None` takes every field of `a` ("take server").
fn portal_after(c: &RawPortal, b: Option<&Portal>, a: &Portal) -> RawPortal {
    let mut n = c.clone();
    if b.is_none_or(|b| !f3_eq(b.pos, a.pos)) && !f3_eq(box_pos(c), a.pos) {
        let (min, max) = recentre(c, a.pos);
        n.min = min;
        n.max = max;
    }
    if b.is_none_or(|b| b.country != a.country) {
        n.country = a.country as i32;
    }
    if b.is_none_or(|b| b.level_min != a.level_min) {
        n.level_min = i16_of(a.level_min);
    }
    if b.is_none_or(|b| b.level_max != a.level_max) {
        n.level_max = i16_of(a.level_max);
    }
    if b.is_none_or(|b| b.to_map != a.to_map) {
        n.dest_map = a.to_map as i32;
    }
    if b.is_none_or(|b| !f3_eq(b.to_pos, a.to_pos)) {
        n.dest_pos = a.to_pos;
    }
    n
}

/// A new client portal for a portal the svmap gained: the box has the size of the first client portal (2 x 4 x 2 when there
/// is none), stands where the svmap portal stands, and has no names.
fn portal_new(l: &Lists, a: &Portal) -> RawPortal {
    let size = l.portals.first().map_or([2.0, 4.0, 2.0], |p| [p.max[0] - p.min[0], p.max[1] - p.min[1], p.max[2] - p.min[2]]);
    let proto = RawPortal {
        min: [0.0; 3],
        max: size,
        rot_y: 0.0,
        name: [0; 256],
        dest_name: [0; 248],
        country: 0,
        level_min: 0,
        level_max: 0,
        dest_map: 0,
        dest_pos: [0.0; 3],
    };
    portal_after(&proto, None, a)
}

/// The client NPC after the svmap NPC changed from `b` to `a`. The client keeps the angle of the first point only; the path
/// points that follow carry a position and nothing else.
fn npc_after(c: &WldNpc, b: Option<&NpcEntry>, a: &NpcEntry) -> WldNpc {
    let mut n = c.clone();
    if b.is_none_or(|b| b.kind != a.kind) {
        n.kind = a.kind as i32;
    }
    if b.is_none_or(|b| b.type_id != a.type_id) {
        n.type_id = a.type_id as i32;
    }
    if let Some(p) = a.points.first() {
        let bp = b.and_then(|b| b.points.first());
        if bp.is_none_or(|bp| !f3_eq(bp.pos, p.pos)) {
            n.pos = p.pos;
        }
        if bp.is_none_or(|bp| bp.dir != p.dir) {
            n.angle = p.dir;
        }
        let rest = &a.points[1..];
        let same_rest = b.is_some_and(|b| b.points.len() == a.points.len() && b.points[1..].iter().zip(rest).all(|(x, y)| f3_eq(x.pos, y.pos)));
        if !same_rest {
            n.extra = rest.iter().map(|q| q.pos).collect();
        }
    }
    n
}

fn tail_of(r: &Region) -> i32 {
    (r.b as u32 | (r.c as u32) << 8 | (r.d as u32) << 16) as i32
}

fn area_after(c: &RawArea, b: Option<&Region>, a: &Region) -> RawArea {
    let mut n = c.clone();
    if b.is_none_or(|b| !f3_eq(b.min, a.min)) {
        n.min = a.min;
    }
    if b.is_none_or(|b| !f3_eq(b.max, a.max)) {
        n.max = a.max;
    }
    if b.is_none_or(|b| b.kind != a.kind) {
        n.mode = a.kind as i32;
    }
    if b.is_none_or(|b| (b.b, b.c, b.d) != (a.b, a.c, a.d)) {
        n.extra = tail_of(a);
    }
    n
}

fn area_new(a: &Region) -> RawArea {
    let proto = RawArea { min: [0.0; 3], max: [0.0; 3], rot_y: 0.0, name: [0; 256], tga: [0; 256], mode: 0, extra: 0 };
    area_after(&proto, None, a)
}

/// The client zone group after the bind changed from `b` to `a`: boxes that changed take their corners, boxes beyond the old
/// count are new (no rotation, the bind's country and attr), boxes beyond the new count go. A change of country or attr is
/// written to every box.
fn zone_after(c: &ZoneGroup, b: Option<&Bind>, a: &Bind) -> ZoneGroup {
    let mut n = c.clone();
    for (k, x) in a.boxes.iter().enumerate() {
        let changed = b.is_none_or(|b| b.boxes.get(k).is_none_or(|y| !f3_eq(y.min, x.min) || !f3_eq(y.max, x.max)));
        match n.boxes.get_mut(k) {
            Some(zb) => {
                if changed {
                    zb.min = x.min;
                    zb.max = x.max;
                }
            }
            None => n.boxes.push(ZoneBox { min: x.min, max: x.max, rot_y: 0.0, country: a.country as i32, attr: a.attr as i32 }),
        }
    }
    n.boxes.truncate(a.boxes.len());
    if b.is_none_or(|b| b.country != a.country) {
        for zb in &mut n.boxes {
            zb.country = a.country as i32;
        }
    }
    if b.is_none_or(|b| b.attr != a.attr) {
        for zb in &mut n.boxes {
            zb.attr = a.attr as i32;
        }
    }
    n
}

fn zone_new(a: &Bind) -> ZoneGroup {
    zone_after(&ZoneGroup::default(), None, a)
}

/// The text with the area numbers after `at` moved by one: `insert` gives lines `>= at` the next number, otherwise line `at` goes
/// and later lines get the previous one. `None` when the bytes are not UTF-16 area text (the editor then leaves them alone).
pub fn txt_shift(bytes: &[u8], at: u32, insert: bool) -> Option<Vec<u8>> {
    const BOM: [u8; 2] = [0xff, 0xfe];
    const LF: u16 = 0x000a;
    const TAB: u16 = 0x0009;
    let (bom, body) = match bytes.strip_prefix(&BOM[..]) {
        Some(r) => (true, r),
        None => (false, bytes),
    };
    if body.len() % 2 == 1 || (!bom && !body.is_empty() && AreaTxt::parse(bytes).names().is_empty()) {
        return None;
    }
    let units: Vec<u16> = body.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let mut out: Vec<u16> = Vec::with_capacity(units.len() + 4);
    let mut start = 0;
    while start < units.len() {
        let end = units[start..].iter().position(|&u| u == LF).map_or(units.len(), |p| start + p + 1);
        let line = &units[start..end];
        let text_len = line.iter().rposition(|&u| u != LF && u != 0x000d).map_or(0, |p| p + 1);
        let tab = line[..text_len].iter().position(|&u| u == TAB);
        let index = tab.and_then(|t| String::from_utf16_lossy(&line[..t]).trim().parse::<u32>().ok());
        match (index, tab) {
            (Some(i), Some(t)) => {
                let new = if insert {
                    Some(if i >= at { i + 1 } else { i })
                } else if i == at {
                    None
                } else {
                    Some(if i > at { i - 1 } else { i })
                };
                if let Some(n) = new {
                    if n == i {
                        out.extend_from_slice(line);
                    } else {
                        out.extend(n.to_string().encode_utf16());
                        out.extend_from_slice(&line[t..]);
                    }
                }
            }
            _ => out.extend_from_slice(line),
        }
        start = end;
    }
    let mut v = Vec::with_capacity(out.len() * 2 + 2);
    if bom {
        v.extend_from_slice(&BOM);
    }
    for u in out {
        v.extend_from_slice(&u.to_le_bytes());
    }
    Some(v)
}

/// The text with area `index` named `name`.
pub fn txt_with_name(bytes: &[u8], index: u32, name: &str) -> Result<Vec<u8>, String> {
    let mut t = AreaTxt::parse(bytes);
    t.set_name(index, name)?;
    Ok(t.to_bytes())
}

/// Emits a text op (when the bytes change) for an inserted or removed area.
fn txt_op(l: &Lists, index: usize, insert: bool) -> Option<COp> {
    let after = txt_shift(&l.txt, index as u32, insert)?;
    (after != l.txt).then(|| COp::Txt { before: l.txt.clone(), after })
}

/// Applies each op to the scratch lists as it is made, so the next one sees the new state.
fn push(l: &mut Lists, out: &mut Vec<COp>, op: COp) {
    op.apply_to(l);
    out.push(op);
}

fn replace_ops(l: &mut Lists, out: &mut Vec<COp>, sec: SectionId, i: usize, before: Option<&Rec>, after: &Rec) {
    match (sec, after) {
        (SectionId::Portals, Rec::Portal(a)) => {
            if let Some(c) = l.portals.get(i) {
                let n = portal_after(c, if let Some(Rec::Portal(b)) = before { Some(b) } else { None }, a);
                if &n != c {
                    push(l, out, COp::Portal(i, CKind::Replace { before: c.clone(), after: n }));
                }
            }
        }
        (SectionId::Npcs, Rec::Npc(a)) => {
            if let Some(c) = l.npcs.get(i) {
                let n = npc_after(c, if let Some(Rec::Npc(b)) = before { Some(b) } else { None }, a);
                if &n != c {
                    push(l, out, COp::Npc(i, CKind::Replace { before: c.clone(), after: n }));
                }
            }
        }
        (SectionId::Regions, Rec::Region(a)) => {
            if let Some(c) = l.areas.get(i) {
                let n = area_after(c, if let Some(Rec::Region(b)) = before { Some(b) } else { None }, a);
                if &n != c {
                    push(l, out, COp::Area(i, CKind::Replace { before: c.clone(), after: n }));
                }
            }
        }
        (SectionId::Binds, Rec::Bind(a)) => {
            if let Some(c) = l.zones.get(i) {
                let n = zone_after(c, if let Some(Rec::Bind(b)) = before { Some(b) } else { None }, a);
                if &n != c {
                    push(l, out, COp::Zone(i, CKind::Replace { before: c.clone(), after: n }));
                }
            }
        }
        _ => {}
    }
}

fn insert_ops(l: &mut Lists, out: &mut Vec<COp>, sec: SectionId, i: usize, rec: &Rec) {
    match (sec, rec) {
        (SectionId::Portals, Rec::Portal(a)) => {
            let n = portal_new(l, a);
            push(l, out, COp::Portal(i.min(l.portals.len()), CKind::Insert(n)));
        }
        (SectionId::Npcs, Rec::Npc(a)) => {
            let proto = WldNpc { kind: 0, type_id: 0, pos: [0.0; 3], angle: 0.0, extra: Vec::new() };
            let n = npc_after(&proto, None, a);
            push(l, out, COp::Npc(i.min(l.npcs.len()), CKind::Insert(n)));
        }
        (SectionId::Regions, Rec::Region(a)) => {
            let at = i.min(l.areas.len());
            push(l, out, COp::Area(at, CKind::Insert(area_new(a))));
            if let Some(t) = txt_op(l, at, true) {
                push(l, out, t);
            }
        }
        (SectionId::Binds, Rec::Bind(a)) => {
            let n = zone_new(a);
            push(l, out, COp::Zone(i.min(l.zones.len()), CKind::Insert(n)));
        }
        _ => {}
    }
}

fn remove_ops(l: &mut Lists, out: &mut Vec<COp>, sec: SectionId, i: usize) {
    match sec {
        SectionId::Portals => {
            if let Some(c) = l.portals.get(i).cloned() {
                push(l, out, COp::Portal(i, CKind::Remove(c)));
            }
        }
        SectionId::Npcs => {
            if let Some(c) = l.npcs.get(i).cloned() {
                push(l, out, COp::Npc(i, CKind::Remove(c)));
            }
        }
        SectionId::Regions => {
            if let Some(c) = l.areas.get(i).cloned() {
                let t = txt_op(l, i, false);
                push(l, out, COp::Area(i, CKind::Remove(c)));
                if let Some(t) = t {
                    push(l, out, t);
                }
            }
        }
        SectionId::Binds => {
            if let Some(c) = l.zones.get(i).cloned() {
                push(l, out, COp::Zone(i, CKind::Remove(c)));
            }
        }
        _ => {}
    }
}

/// Two adjacent replaces that swap two records of one section (the portal reorder): the client records swap too, so a box goes
/// with its portal.
fn swap_ops(l: &mut Lists, out: &mut Vec<COp>, sec: SectionId, i: usize, j: usize) {
    macro_rules! swap {
        ($list:ident, $variant:ident) => {{
            if let (Some(a), Some(b)) = (l.$list.get(i).cloned(), l.$list.get(j).cloned()) {
                if a != b {
                    push(l, out, COp::$variant(i, CKind::Replace { before: a.clone(), after: b.clone() }));
                    push(l, out, COp::$variant(j, CKind::Replace { before: b, after: a }));
                }
            }
        }};
    }
    match sec {
        SectionId::Portals => swap!(portals, Portal),
        SectionId::Npcs => swap!(npcs, Npc),
        SectionId::Regions => swap!(areas, Area),
        SectionId::Binds => swap!(zones, Zone),
        _ => {}
    }
}

/// The client ops that make the same change as the svmap `ops`, for the sections that are `paired` (flags in [`SECTIONS`]
/// order, taken before the step). `l` is a scratch copy of the client lists; it ends in the state after the ops.
pub fn mirror(l: &mut Lists, paired: [bool; 4], ops: &[Op]) -> Vec<COp> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < ops.len() {
        let op = &ops[k];
        let sec = op.section();
        let on = slot(sec).is_some_and(|s| paired[s]);
        if !on {
            k += 1;
            continue;
        }
        if let (OpKind::Replace { before: b1, after: a1 }, Some(next)) = (&op.kind, ops.get(k + 1)) {
            if let OpKind::Replace { before: b2, after: a2 } = &next.kind {
                if next.section() == sec && next.index != op.index && b1 == a2 && b2 == a1 && b1 != b2 {
                    swap_ops(l, &mut out, sec, op.index, next.index);
                    k += 2;
                    continue;
                }
            }
        }
        match &op.kind {
            OpKind::Replace { before, after } => replace_ops(l, &mut out, sec, op.index, Some(before), after),
            OpKind::Insert(r) => insert_ops(l, &mut out, sec, op.index, r),
            OpKind::Remove(_) => remove_ops(l, &mut out, sec, op.index),
            OpKind::Cells(_) => {}
        }
        k += 1;
    }
    out
}

/// The client ops that give client record `w` of `sec` the value of the svmap record `rec` ("take server"): every field of the
/// svmap side is written.
pub fn take_server_replace(l: &mut Lists, sec: SectionId, w: usize, rec: &Rec) -> Vec<COp> {
    let mut out = Vec::new();
    replace_ops(l, &mut out, sec, w, None, rec);
    out
}

/// The client ops that remove client record `w` ("take server" on a record the svmap does not have).
pub fn take_server_remove(l: &mut Lists, sec: SectionId, w: usize) -> Vec<COp> {
    let mut out = Vec::new();
    remove_ops(l, &mut out, sec, w);
    out
}

/// The client ops that append a record made from the svmap record `rec` ("take server" on a record the client does not have).
pub fn take_server_append(l: &mut Lists, sec: SectionId, rec: &Rec) -> Vec<COp> {
    let end = l.count(sec);
    take_server_insert(l, sec, end, rec)
}

/// The client ops that insert a record made from the svmap record `rec` at index `at` ("take server" on an NPC the client does
/// not have, put at its place between the paired records).
pub fn take_server_insert(l: &mut Lists, sec: SectionId, at: usize, rec: &Rec) -> Vec<COp> {
    let mut out = Vec::new();
    insert_ops(l, &mut out, sec, at, rec);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::NpcPoint;

    fn map_for(w: &shaiya_clientdata::wld_raw::WldRaw) -> Svmap {
        let mut m = Svmap::new(4);
        for p in &w.portals {
            m.portals.push(Portal { pos: box_pos(p), country: p.country as u32, level_min: p.level_min as u16, level_max: p.level_max as u16, to_map: p.dest_map as u32, to_pos: p.dest_pos });
        }
        for n in &w.npcs {
            m.npcs.push(crate::sync::npc_from_wld(n));
        }
        m
    }

    #[test]
    fn txt_numbers_follow_inserts_and_removes() {
        let t = |s: &str| {
            let mut v = vec![0xff, 0xfe];
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        };
        let b = t("0\tOne\r\n2\tTwo\r\n5\tFive");
        assert_eq!(txt_shift(&b, 2, true).unwrap(), t("0\tOne\r\n3\tTwo\r\n6\tFive"), "lines from the new area on move up");
        assert_eq!(txt_shift(&b, 3, true).unwrap(), t("0\tOne\r\n2\tTwo\r\n6\tFive"));
        assert_eq!(txt_shift(&b, 2, false).unwrap(), t("0\tOne\r\n4\tFive"), "the removed area's line goes, later lines move down");
        assert_eq!(txt_shift(&b, 9, false).unwrap(), b, "nothing after the end changes");
        // insert then remove gives the file back
        let there = txt_shift(&b, 1, true).unwrap();
        assert_eq!(txt_shift(&there, 1, false).unwrap(), b);
        // not area text: left alone
        assert!(txt_shift(&[1, 2, 3], 0, true).is_none());
        assert!(txt_shift(b"abc\n", 0, true).is_none(), "no BOM and no index lines");
        assert_eq!(txt_shift(&[], 0, true).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn a_portal_move_keeps_box_size_and_other_fields() {
        let w = crate::testutil::tiny_wld(4);
        let l0 = Lists { portals: w.portals.clone(), ..Default::default() };
        let m = map_for(&w);
        let before = Rec::Portal(m.portals[0]);
        let mut a = m.portals[0];
        a.pos[0] += 7.0;
        a.pos[2] -= 3.0;
        let mut l = l0.clone();
        let ops = vec![Op::replace(0, before, Rec::Portal(a))];
        let cops = mirror(&mut l, [false, true, false, false], &ops);
        assert_eq!(cops.len(), 1);
        let (b, n) = (&l0.portals[0], &l.portals[0]);
        assert_eq!((n.max[0] - n.min[0], n.max[1] - n.min[1], n.max[2] - n.min[2]), (b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]));
        let c = box_pos(n);
        assert!((c[0] - a.pos[0]).abs() < 1e-4 && (c[2] - a.pos[2]).abs() < 1e-4 && c[1] == a.pos[1]);
        assert_eq!((n.name, n.dest_name, n.country, n.dest_map), (b.name, b.dest_name, b.country, b.dest_map), "other fields stay");
        // unpaired sections are left alone
        let mut l = l0.clone();
        assert!(mirror(&mut l, [false; 4], &ops).is_empty() && l == l0);
        // a change of the destination only leaves the box alone, bit for bit
        let mut d = m.portals[0];
        d.to_map = 7;
        let mut l = l0.clone();
        let cops = mirror(&mut l, [false, true, false, false], &[Op::replace(0, Rec::Portal(m.portals[0]), Rec::Portal(d))]);
        assert_eq!(cops.len(), 1);
        assert_eq!((l.portals[0].min, l.portals[0].max, l.portals[0].dest_map), (b.min, b.max, 7));
        // the inverse puts the record back
        cops[0].invert().apply_to(&mut l);
        assert_eq!(l, l0);
    }

    #[test]
    fn an_edit_never_overwrites_a_difference_it_did_not_make() {
        let w = crate::testutil::tiny_wld(4);
        let mut l = Lists { portals: w.portals.clone(), ..Default::default() };
        l.portals[0].dest_map = 99; // the client says 99, the svmap says 2
        let m = map_for(&w);
        let mut a = m.portals[0];
        a.level_max = 500;
        let cops = mirror(&mut l, [false, true, false, false], &[Op::replace(0, Rec::Portal(m.portals[0]), Rec::Portal(a))]);
        assert_eq!(cops.len(), 1);
        assert_eq!((l.portals[0].dest_map, l.portals[0].level_max), (99, 500));
    }

    #[test]
    fn insert_remove_and_swap_use_the_same_index() {
        let w = crate::testutil::tiny_wld(4);
        let mut base = Lists { portals: w.portals.clone(), ..Default::default() };
        let mut second = base.portals[0].clone();
        second.min = [30.0, 1.0, 30.0];
        second.max = [32.0, 3.0, 32.0];
        second.dest_map = 5;
        base.portals.push(second);
        let mut l = base.clone();
        let p = Portal { pos: [50.0, 2.0, 60.0], country: 2, level_min: 3, level_max: 40, to_map: 4, to_pos: [1.0, 2.0, 3.0] };
        let cops = mirror(&mut l, [false, true, false, false], &[Op::insert(1, Rec::Portal(p))]);
        assert_eq!(cops.len(), 1);
        assert_eq!(l.portals.len(), 3);
        assert_eq!(l.portals[2], base.portals[1], "the old second portal moved to index 2");
        let n = &l.portals[1];
        assert_eq!((n.dest_map, n.country, n.level_max), (4, 2, 40));
        assert_eq!((n.max[0] - n.min[0], n.max[2] - n.min[2]), (base.portals[0].max[0] - base.portals[0].min[0], base.portals[0].max[2] - base.portals[0].min[2]));
        let c = box_pos(n);
        assert!((c[0] - 50.0).abs() < 1e-4 && (c[2] - 60.0).abs() < 1e-4 && c[1] == 2.0);
        // remove it again
        let mut l2 = l.clone();
        mirror(&mut l2, [false, true, false, false], &[Op::remove(1, Rec::Portal(p))]);
        assert_eq!(l2, base);
        // swap: two adjacent replaces exchanging the records
        let ra = Portal { pos: box_pos(&base.portals[0]), country: 1, level_min: 0, level_max: 999, to_map: 2, to_pos: [1.0, 2.0, 3.0] };
        let rb = Portal { pos: box_pos(&base.portals[1]), country: 1, level_min: 0, level_max: 999, to_map: 5, to_pos: [1.0, 2.0, 3.0] };
        let mut l3 = base.clone();
        let cops = mirror(&mut l3, [false, true, false, false], &[Op::replace(0, Rec::Portal(ra), Rec::Portal(rb)), Op::replace(1, Rec::Portal(rb), Rec::Portal(ra))]);
        assert_eq!(cops.len(), 2);
        assert_eq!((&l3.portals[0], &l3.portals[1]), (&base.portals[1], &base.portals[0]), "the boxes went with their portals");
    }

    #[test]
    fn npc_edits_and_paths_are_mirrored() {
        let mut w = crate::testutil::tiny_wld(4);
        w.npcs[0].extra = vec![[5.0, 1.0, 5.0]];
        let m = map_for(&w);
        let l0 = Lists { npcs: w.npcs.clone(), ..Default::default() };
        let before = Rec::Npc(m.npcs[0].clone());
        let mut a = m.npcs[0].clone();
        a.points.push(NpcPoint { pos: [9.0, 1.0, 9.0], dir: 1.0 });
        a.points[0].pos[0] += 2.0;
        a.points[0].dir = 2.5;
        a.type_id = 8;
        let mut l = l0.clone();
        let cops = mirror(&mut l, [true, false, false, false], &[Op::replace(0, before, Rec::Npc(a.clone()))]);
        assert_eq!(cops.len(), 1);
        let n = &l.npcs[0];
        assert_eq!((n.kind, n.type_id, n.pos, n.angle), (1, 8, a.points[0].pos, 2.5));
        assert_eq!(n.extra, vec![[5.0, 1.0, 5.0], [9.0, 1.0, 9.0]]);
        // only a path point's own heading changed: the client has no place for it, so nothing is mirrored
        let mut h = m.npcs[0].clone();
        h.points[1].dir = 0.123;
        let mut l = l0.clone();
        assert!(mirror(&mut l, [true, false, false, false], &[Op::replace(0, Rec::Npc(m.npcs[0].clone()), Rec::Npc(h))]).is_empty());
        // add and remove
        let mut l = l0.clone();
        let c = mirror(&mut l, [true, false, false, false], &[Op::insert(1, Rec::Npc(a.clone()))]);
        assert_eq!((c.len(), l.npcs.len()), (1, 2));
        assert_eq!(l.npcs[1].extra.len(), 2);
    }

    #[test]
    fn regions_and_binds_follow_and_the_text_renumbers() {
        use shaiya_svmap::{Aabb, Bind};
        let mut l = Lists::default();
        let a0 = Region { min: [0.0, 0.0, 0.0], max: [5.0, 2.0, 5.0], kind: 0, b: 0, c: 0, d: 0 };
        let a1 = Region { min: [10.0, 0.0, 10.0], max: [15.0, 2.0, 15.0], kind: 1, b: 2, c: 3, d: 4 };
        l.txt = {
            let mut v = vec![0xff, 0xfe];
            for u in "0\tZero\r\n1\tOne\r\n".encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        };
        let base_txt = l.txt.clone();
        let mut l1 = l.clone();
        mirror(&mut l1, [false, false, true, false], &[Op::insert(0, Rec::Region(a0)), Op::insert(1, Rec::Region(a1))]);
        assert_eq!(l1.areas.len(), 2);
        assert_eq!((l1.areas[1].mode, l1.areas[1].extra), (1, 2 | 3 << 8 | 4 << 16));
        let names = AreaTxt::parse(&l1.txt).names();
        assert_eq!((names.get(2), names.get(3)), (Some("Zero"), Some("One")), "the names moved with their areas");
        // removing area 0 again: its line (index 0) goes, the others come back down by one per insert
        let mut l2 = l1.clone();
        mirror(&mut l2, [false, false, true, false], &[Op::remove(1, Rec::Region(a1)), Op::remove(0, Rec::Region(a0))]);
        assert!(l2.areas.is_empty());
        assert_eq!(l2.txt, base_txt);
        // binds
        let b = Bind { country: 1, attr: 0, boxes: vec![Aabb { min: [0.0; 3], max: [4.0, 2.0, 4.0] }] };
        let mut l = Lists::default();
        mirror(&mut l, [false, false, false, true], &[Op::insert(0, Rec::Bind(b.clone()))]);
        assert_eq!(l.zones.len(), 1);
        assert_eq!((l.zones[0].boxes[0].country, l.zones[0].boxes[0].max), (1, [4.0, 2.0, 4.0]));
        let mut nb = b.clone();
        nb.boxes.push(Aabb { min: [8.0; 3], max: [9.0; 3] });
        nb.country = 2;
        mirror(&mut l, [false, false, false, true], &[Op::replace(0, Rec::Bind(b), Rec::Bind(nb))]);
        assert_eq!(l.zones[0].boxes.len(), 2);
        assert!(l.zones[0].boxes.iter().all(|x| x.country == 2));
    }

    // ------------------------------------------------------------------------------------------------ paired documents

    use crate::doc::{ClientSlot, Doc};
    use crate::testutil::{paired_doc, paired_fixture, utf16_bom, TempDir};
    use shaiya_svmap::Aabb;

    fn lists(d: &Doc) -> Lists {
        d.part().expect("a client half").lists()
    }

    fn size3(p: &RawPortal) -> [f32; 3] {
        [p.max[0] - p.min[0], p.max[1] - p.min[1], p.max[2] - p.min[2]]
    }

    fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    fn portal_move(d: &mut Doc, i: usize, dx: f32, dz: f32, dy: f32) {
        let before = crate::edit::get(&d.map, SectionId::Portals, i).unwrap();
        let mut after = before.clone();
        crate::edit::translate(&mut after, dx, dz);
        crate::edit::translate_y(&mut after, dy);
        d.commit("move portal", vec![Op::replace(i, before, after)]);
    }

    /// A moved portal moves its client box in the same undo step: the svmap position stays the box centre (x, z) and its min y, and the
    /// box keeps its size and every other field.
    #[test]
    fn pair_portal_move_both() {
        let mut d = paired_doc();
        let (map0, l0) = (d.map.clone(), lists(&d));
        assert!(!d.is_dirty());
        portal_move(&mut d, 1, 5.0, -3.0, 1.5);
        assert_eq!(d.undo.len(), 1, "one step for both sides");
        let (m, p) = (d.map.portals[1], d.part().unwrap().wld.model.portals[1].clone());
        let q = &l0.portals[1];
        assert!(close3(box_pos(&p), m.pos), "svmap {:?} box {:?}", m.pos, box_pos(&p));
        assert_eq!(box_pos(&p)[1], m.pos[1], "y is the box min y");
        assert!(close3(size3(&p), size3(q)), "the box keeps its size");
        assert_eq!((p.name, p.dest_name, p.dest_map, p.country, p.rot_y), (q.name, q.dest_name, q.dest_map, q.country, q.rot_y));
        assert_eq!(d.part().unwrap().wld.model.portals[0], l0.portals[0], "the other boxes stay");
        assert!(d.client_dirty() && d.is_dirty());
        d.undo();
        assert_eq!(d.map, map0);
        assert_eq!(lists(&d), l0);
        assert!(!d.is_dirty(), "undo to the start is clean on both sides");
        d.redo();
        assert!(d.client_dirty() && close3(box_pos(&d.part().unwrap().wld.model.portals[1]), d.map.portals[1].pos));
        // a destination edit leaves the box bit for bit as it was
        let l1 = lists(&d);
        let before = crate::edit::get(&d.map, SectionId::Portals, 0).unwrap();
        let mut after = before.clone();
        if let Rec::Portal(p) = &mut after {
            p.to_map = 7;
            p.level_max = 80;
        }
        d.commit("destination", vec![Op::replace(0, before, after)]);
        let c = &d.part().unwrap().wld.model.portals[0];
        assert_eq!((c.dest_map, c.level_max, c.min, c.max), (7, 80, l1.portals[0].min, l1.portals[0].max));
    }

    /// The same, on the client's real map 2: portal 0 moves, the client box follows, and the model still writes back.
    #[test]
    fn pair_portal_move_both_on_map_2() {
        let (Some(c), Some(s)) = (std::env::var_os("SHAIYA_CLIENT"), std::env::var_os("SVMAP_ORIGINALS")) else {
            eprintln!("skipped: SVMAP_ORIGINALS and SHAIYA_CLIENT not set");
            return;
        };
        let rules = ClientRules::open(std::path::Path::new(&c)).unwrap();
        let svmap = shaiya_svmap::read(&std::fs::read(std::path::Path::new(&s).join("2.svmap")).unwrap()).unwrap();
        let Attach::Part(p) = ClientPart::attach(&rules, 2, Codepage::Cp949) else { panic!("map 2 has an editable .wld") };
        assert_eq!(p.paired(&svmap), [true; 4], "map 2 pairs in every section");
        let mut d = Doc::new(1, Some(2), None, svmap, None);
        d.set_client(ClientSlot::Part(p));
        let orig_bytes = d.part().unwrap().wld.model.to_bytes();
        portal_move(&mut d, 0, 5.0, 0.0, 0.0);
        let (m, b) = (d.map.portals[0], d.part().unwrap().wld.model.portals[0].clone());
        assert!(close3(box_pos(&b), m.pos));
        assert!((b.min[0] - 99.17466).abs() < 0.01, "the box moved 5 units in x: {}", b.min[0]);
        let bytes = d.part().unwrap().wld.model.to_bytes();
        assert_eq!(bytes.len(), orig_bytes.len());
        shaiya_clientdata::wld_raw::lossless_check(&bytes, "edited map 2").unwrap();
        d.undo();
        assert_eq!(d.part().unwrap().wld.model.to_bytes(), orig_bytes, "undo gives the original bytes back");
    }

    #[test]
    fn pair_insert_portal_same_index() {
        let mut d = paired_doc();
        let (map0, l0) = (d.map.clone(), lists(&d));
        let p = Portal { pos: [50.0, 3.0, 60.0], country: 2, level_min: 5, level_max: 60, to_map: 4, to_pos: [1.0, 2.0, 3.0] };
        d.commit("add portal", vec![Op::insert(1, Rec::Portal(p))]);
        assert_eq!((d.map.portals.len(), d.part().unwrap().count(SectionId::Portals)), (4, 4));
        let (a, b) = (&d.map.portals[1], &d.part().unwrap().wld.model.portals[1]);
        assert_eq!((a.country, a.to_map), (2, 4));
        assert_eq!((b.country, b.dest_map, b.level_min, b.level_max), (2, 4, 5, 60));
        assert!(close3(box_pos(b), a.pos));
        assert_eq!(d.part().unwrap().wld.model.portals[2], l0.portals[1], "the old second portal is third on both sides");
        assert_eq!(d.undo.len(), 1);
        // append at the end, then delete the first one: every step keeps the lists in step
        let end = d.map.portals.len();
        d.commit("add at the end", vec![Op::insert(end, Rec::Portal(p))]);
        d.select_only(SectionId::Portals, 0);
        d.delete_selected();
        assert_eq!((d.map.portals.len(), d.part().unwrap().count(SectionId::Portals)), (4, 4));
        for (i, m) in d.map.portals.iter().enumerate() {
            assert_eq!(d.part().unwrap().wld.model.portals[i].dest_map as u32, m.to_map, "portal {i}");
        }
        while d.undo().is_some() {}
        assert_eq!(d.map, map0);
        assert_eq!(lists(&d), l0);
        // a reorder moves the boxes with their portals
        let (a, b) = (d.map.portals[0], d.map.portals[1]);
        d.commit("reorder portals", vec![Op::replace(0, Rec::Portal(a), Rec::Portal(b)), Op::replace(1, Rec::Portal(b), Rec::Portal(a))]);
        assert_eq!(d.undo.len(), 1);
        assert_eq!(d.part().unwrap().wld.model.portals[0], l0.portals[1]);
        assert_eq!(d.part().unwrap().wld.model.portals[1], l0.portals[0]);
        d.undo();
        assert_eq!(lists(&d), l0);
    }

    #[test]
    fn pair_npc_path_both() {
        let mut d = paired_doc();
        let (map0, l0) = (d.map.clone(), lists(&d));
        let edit_npc = |d: &mut Doc, f: &dyn Fn(&mut shaiya_svmap::NpcEntry)| {
            let Rec::Npc(before) = crate::edit::get(&d.map, SectionId::Npcs, 1).unwrap() else { panic!() };
            let mut after = before.clone();
            f(&mut after);
            d.commit("edit NPC", vec![Op::replace(1, Rec::Npc(before), Rec::Npc(after))]);
        };
        // add a path point, move a middle one, turn the NPC, remove the first extra point
        edit_npc(&mut d, &|n| n.points.push(NpcPoint { pos: [26.0, 1.0, 20.0], dir: 0.0 }));
        assert_eq!(d.part().unwrap().wld.model.npcs[1].extra, vec![[22.0, 1.0, 20.0], [24.0, 1.0, 20.0], [26.0, 1.0, 20.0]]);
        edit_npc(&mut d, &|n| n.points[2].pos[2] += 3.0);
        assert_eq!(d.part().unwrap().wld.model.npcs[1].extra[1], [24.0, 1.0, 23.0]);
        edit_npc(&mut d, &|n| n.points[0].dir = 2.25);
        assert_eq!(d.part().unwrap().wld.model.npcs[1].angle, 2.25);
        edit_npc(&mut d, &|n| {
            n.points.remove(1);
        });
        let c = &d.part().unwrap().wld.model.npcs[1];
        assert_eq!(c.extra, vec![[24.0, 1.0, 23.0], [26.0, 1.0, 20.0]]);
        assert_eq!((c.kind, c.type_id, c.pos), (2, 5, [20.0, 1.0, 20.0]));
        assert_eq!(d.undo.len(), 4);
        // moving the whole NPC (a drag) moves the first point and every path point
        let before = crate::edit::get(&d.map, SectionId::Npcs, 1).unwrap();
        let mut after = before.clone();
        crate::edit::translate(&mut after, 10.0, 10.0);
        d.commit("move NPC", vec![Op::replace(1, before, after)]);
        let c = &d.part().unwrap().wld.model.npcs[1];
        assert_eq!((c.pos, c.extra.clone()), ([30.0, 1.0, 30.0], vec![[34.0, 1.0, 33.0], [36.0, 1.0, 30.0]]));
        // the client record equals what Sync would build from the svmap record
        assert!(crate::sync::differences(&Rec::Npc(d.map.npcs[1].clone()), &Rec::Npc(crate::sync::npc_from_wld(c))).is_empty());
        while d.undo().is_some() {}
        assert_eq!((d.map.clone(), lists(&d)), (map0, l0));
    }

    /// A section whose counts differ is not paired: its edits stay on the svmap, the other sections still follow.
    #[test]
    fn pair_unpaired_when_counts_differ() {
        let mut d = paired_doc();
        let before = crate::edit::get(&d.map, SectionId::Portals, 2).unwrap();
        d.commit_server_only("drop a portal on the svmap", vec![Op::remove(2, before)]);
        assert_eq!(d.part().unwrap().paired(&d.map), [true, false, true, true]);
        let l0 = lists(&d);
        portal_move(&mut d, 0, 4.0, 4.0, 0.0);
        assert_eq!(lists(&d), l0, "an unpaired section changes the svmap only");
        assert!(d.undo.last().unwrap().cops.is_empty());
        assert!(!d.client_dirty());
        // a paired section still follows
        let before = crate::edit::get(&d.map, SectionId::Regions, 1).unwrap();
        let mut after = before.clone();
        crate::edit::translate(&mut after, 1.0, 1.0);
        d.commit("move region", vec![Op::replace(1, before, after)]);
        assert!(d.client_dirty());
        // the client's real maps: map 35 does not pair its portals, map 2 pairs everything
        let (Some(c), Some(s)) = (std::env::var_os("SHAIYA_CLIENT"), std::env::var_os("SVMAP_ORIGINALS")) else {
            eprintln!("skipped: SVMAP_ORIGINALS and SHAIYA_CLIENT not set");
            return;
        };
        let rules = ClientRules::open(std::path::Path::new(&c)).unwrap();
        let load = |id: u32| shaiya_svmap::read(&std::fs::read(std::path::Path::new(&s).join(format!("{id}.svmap"))).unwrap()).unwrap();
        let m35 = load(35);
        let Attach::Part(p35) = ClientPart::attach(&rules, 35, Codepage::Cp949) else { panic!("map 35 has an editable .wld") };
        let flags = p35.paired(&m35);
        assert!(!flags[1], "map 35 has {} portals and the .wld {}", m35.portals.len(), p35.count(SectionId::Portals));
        let mut d35 = Doc::new(1, Some(35), None, m35, None);
        d35.set_client(ClientSlot::Part(p35));
        let l35 = lists(&d35);
        portal_move(&mut d35, 0, 3.0, 0.0, 0.0);
        assert_eq!(lists(&d35), l35, "map 35's portals are edited on the svmap only");
        let first_region = d35.map.regions.first().cloned();
        if let (true, Some(r)) = (flags[2], first_region) {
            let mut after = r;
            after.min[0] += 1.0;
            d35.commit("move region", vec![Op::replace(0, Rec::Region(r), Rec::Region(after))]);
            assert_ne!(lists(&d35), l35, "a paired section of the same map follows");
        }
    }

    #[test]
    fn region_name_writes_txt() {
        let root = TempDir::new("c2-region-name");
        let (client, _maps) = crate::testutil::paired_folders(&root);
        let rules = ClientRules::open(&client).unwrap();
        let Attach::Part(p) = ClientPart::attach(&rules, 9, Codepage::Cp949) else { panic!("map 9 is editable") };
        let (_, m, _) = paired_fixture(64);
        let mut d = Doc::new(1, Some(9), None, m, None);
        d.set_client(ClientSlot::Part(p));
        assert_eq!(d.part().unwrap().names().get(1), Some("Beta"));
        // a rename is one client-only step
        let txt = d.part().unwrap().txt.clone();
        let after = txt_with_name(&txt, 1, "Gamma").unwrap();
        d.commit_client("rename region #1", vec![COp::Txt { before: txt.clone(), after }]).unwrap();
        assert_eq!(d.undo.len(), 1);
        assert_eq!(d.part().unwrap().names().get(1), Some("Gamma"));
        assert!(d.client_dirty() && !d.svmap_dirty(), "only the client half is modified");
        // a new region before it shifts the lines: the names stay with their areas
        let r = Region { min: [0.0; 3], max: [1.0; 3], kind: 0, b: 0, c: 0, d: 0 };
        d.commit("add region", vec![Op::insert(0, Rec::Region(r))]);
        let n = d.part().unwrap().names();
        assert_eq!((n.get(0), n.get(1), n.get(2)), (None, Some("Alpha"), Some("Gamma")));
        // save: the staging folder gets the text and the .wld, the client folder is untouched
        let out = root.join("out");
        let before_client = std::fs::read(client.join("world").join("9.txt")).unwrap();
        let written = d.part_mut().unwrap().save(&out, None, false).unwrap();
        assert_eq!(written.len(), 2, "{written:?}");
        d.mark_client_saved();
        assert!(!d.client_dirty());
        let staged = std::fs::read(out.join("data").join("world").join("9.txt")).unwrap();
        let names = AreaTxt::parse(&staged).names();
        assert_eq!((names.get(1), names.get(2)), (Some("Alpha"), Some("Gamma")));
        assert_eq!(std::fs::read(client.join("world").join("9.txt")).unwrap(), before_client, "the client's own text is untouched");
        let wld = std::fs::read(out.join("data").join("world").join("9.wld")).unwrap();
        assert_eq!(shaiya_clientdata::wld_raw::WldRaw::parse(&wld, "staged").unwrap().areas.len(), 3);
        // a name with a tab is refused
        assert!(txt_with_name(&d.part().unwrap().txt, 0, "a\tb").is_err());
        // undo of both steps restores the text bytes
        d.undo();
        d.undo();
        assert_eq!(d.part().unwrap().txt, utf16_bom("0\tAlpha\r\n1\tBeta\r\n"));
    }

    /// Two-way Sync: one row takes the client's value into the svmap, another the svmap's value into the client map, a third
    /// removes a client record the svmap does not have, a fourth appends a record the client lacks. One undo step.
    #[test]
    fn sync_two_way() {
        use crate::sync::{plan, target_from_wld, RowKind, Take};
        let mut d = paired_doc();
        let (map0, l0) = (d.map.clone(), lists(&d));
        // the svmap and the client map drift apart
        let b = crate::edit::get(&d.map, SectionId::Portals, 0).unwrap();
        let mut a = b.clone();
        if let Rec::Portal(p) = &mut a {
            p.country = 2;
        }
        d.commit_server_only("svmap portal 0", vec![Op::replace(0, b, a)]);
        let cb = d.part().unwrap().wld.model.portals[1].clone();
        let mut ca = cb.clone();
        ca.dest_map = 99;
        d.commit_client("client portal 1", vec![COp::Portal(1, CKind::Replace { before: cb, after: ca })]).unwrap();
        let rb = crate::edit::get(&d.map, SectionId::Binds, 0).unwrap();
        d.commit_server_only("svmap loses its bind", vec![Op::remove(0, rb)]);
        let extra = Region { min: [60.0, 0.0, 60.0], max: [62.0, 2.0, 62.0], kind: 0, b: 0, c: 0, d: 0 };
        d.commit_server_only("svmap gains a region", vec![Op::insert(2, Rec::Region(extra))]);
        let (map1, l1) = (d.map.clone(), lists(&d));
        let mut p = plan(&d.map, &target_from_wld(&d.part().unwrap().wld_arc()));
        let kinds: Vec<(SectionId, RowKind, usize)> = p.rows.iter().map(|r| (r.section, r.kind, r.index)).collect();
        assert_eq!(kinds, vec![(SectionId::Portals, RowKind::Changed, 0), (SectionId::Portals, RowKind::Changed, 1), (SectionId::Regions, RowKind::Removed, 2), (SectionId::Binds, RowKind::Added, 0)]);
        // portal 0: take server (the client gets country 2); portal 1: take client (the svmap gets destination 99);
        // region 2: take server (appended to the client); bind 0: take server (removed from the client)
        for r in &mut p.rows {
            r.checked = true;
            r.dir = if r.section == SectionId::Portals && r.index == 1 { Take::Client } else { Take::Server };
        }
        let ops = p.ops(&d.map);
        let cops = p.client_ops(&mut lists(&d));
        d.commit_both("sync with client", ops, cops).unwrap();
        assert_eq!(d.undo.len(), 5, "four setup steps and one sync step");
        assert_eq!(d.part().unwrap().wld.model.portals[0].country, 2, "take server: the client has the svmap's value");
        assert_eq!(d.map.portals[1].to_map, 99, "take client: the svmap has the client's value");
        assert_eq!(d.part().unwrap().wld.model.portals[1].dest_map, 99);
        assert_eq!(d.part().unwrap().count(SectionId::Regions), 3, "the missing area was appended");
        assert!(close3(d.part().unwrap().wld.model.areas[2].min, [60.0, 0.0, 60.0]));
        assert_eq!(d.part().unwrap().count(SectionId::Binds), 0, "the extra zone group was removed");
        assert_eq!(d.part().unwrap().paired(&d.map), [true; 4]);
        assert!(plan(&d.map, &target_from_wld(&d.part().unwrap().wld_arc())).rows.is_empty(), "nothing is left to sync");
        d.undo();
        assert_eq!((d.map.clone(), lists(&d)), (map1, l1), "one undo gives both sides back");
        while d.undo().is_some() {}
        assert_eq!((d.map.clone(), lists(&d)), (map0, l0));
    }

    /// Every kind of paired edit is one step; undoing them all gives both sides back, byte for byte; the dirty flags follow.
    #[test]
    fn paired_undo_one_step() {
        let mut d = paired_doc();
        let (map0, l0) = (d.map.clone(), lists(&d));
        let bytes0 = d.part().unwrap().wld.model.to_bytes();
        portal_move(&mut d, 0, 1.0, 1.0, 0.0);
        let r = Region { min: [3.0, 0.0, 3.0], max: [4.0, 2.0, 4.0], kind: 2, b: 1, c: 2, d: 3 };
        d.commit("add region", vec![Op::insert(1, Rec::Region(r))]);
        let b = Bind { country: 2, attr: 0, boxes: vec![Aabb { min: [1.0; 3], max: [2.0; 3] }, Aabb { min: [5.0; 3], max: [6.0; 3] }] };
        d.commit("add bind", vec![Op::insert(1, Rec::Bind(b))]);
        let txt = d.part().unwrap().txt.clone();
        let after = txt_with_name(&txt, 2, "Delta").unwrap();
        d.commit_client("rename", vec![COp::Txt { before: txt, after }]).unwrap();
        d.select_only(SectionId::Regions, 0);
        d.delete_selected();
        assert_eq!(d.undo.len(), 5, "one step each");
        assert!(d.client_dirty());
        let (map5, l5) = (d.map.clone(), lists(&d));
        let bytes5 = d.part().unwrap().wld.model.to_bytes();
        assert_ne!(bytes5, bytes0);
        shaiya_clientdata::wld_raw::lossless_check(&bytes5, "edited").unwrap();
        let mut steps = 0;
        while d.undo().is_some() {
            steps += 1;
        }
        assert_eq!(steps, 5);
        assert_eq!((d.map.clone(), lists(&d)), (map0.clone(), l0.clone()));
        assert_eq!(d.part().unwrap().wld.model.to_bytes(), bytes0, "the model writes the original bytes again");
        assert!(!d.is_dirty());
        while d.redo().is_some() {}
        assert_eq!((d.map.clone(), lists(&d)), (map5, l5));
        assert_eq!(d.part().unwrap().wld.model.to_bytes(), bytes5);
        // jumping in the history moves both sides
        d.jump_to(2);
        assert_eq!(d.part().unwrap().count(SectionId::Regions), 3);
        d.jump_to(0);
        assert_eq!(lists(&d), l0);
        assert!(!d.client_dirty());
        // a step above a saved client state that is dropped by a new edit leaves the client dirty
        d.jump_to(5);
        d.mark_client_saved();
        assert!(!d.client_dirty());
        d.undo();
        assert!(d.client_dirty(), "the saved state is one step ahead");
        d.redo();
        assert!(!d.client_dirty());
        d.undo();
        portal_move(&mut d, 2, 1.0, 0.0, 0.0);
        assert!(d.client_dirty(), "the saved state cannot be reached any more");
    }

    /// Typing a number is one step with the client following; typing the old value back removes the step.
    #[test]
    fn typed_edits_merge_with_the_client_side() {
        let mut d = paired_doc();
        let l0 = lists(&d);
        for v in [20.0f32, 21.0, 22.0] {
            let Some(Rec::Portal(mut p)) = crate::edit::get(&d.map, SectionId::Portals, 1) else { panic!() };
            let before = Rec::Portal(p);
            p.pos[0] = v;
            d.commit_coalesced("portal1:x", "edit Portals #1", vec![Op::replace(1, before, Rec::Portal(p))]);
        }
        assert_eq!(d.undo.len(), 1);
        assert!((box_pos(&d.part().unwrap().wld.model.portals[1])[0] - 22.0).abs() < 1e-3);
        let Some(Rec::Portal(mut p)) = crate::edit::get(&d.map, SectionId::Portals, 1) else { panic!() };
        let before = Rec::Portal(p);
        p.pos[0] = 31.0; // the original centre x
        d.commit_coalesced("portal1:x", "edit Portals #1", vec![Op::replace(1, before, Rec::Portal(p))]);
        assert_eq!(d.undo.len(), 0, "back to the start");
        assert!(close3(box_pos(&d.part().unwrap().wld.model.portals[1]), box_pos(&l0.portals[1])));
        assert!(!d.is_dirty());
    }

    /// A tab whose `.wld` cannot be written back refuses client edits with the reason and still edits the svmap; a read-only tab
    /// refuses both.
    #[test]
    fn read_only_refuses_wld_edits() {
        // the real reason: a .wld that does not write back byte for byte
        let root = TempDir::new("c2-readonly");
        let (client, _maps) = crate::testutil::paired_folders(&root);
        let rules = ClientRules::open(&client).unwrap();
        let slot = match ClientPart::attach(&rules, 10, Codepage::Cp949) {
            Attach::ReadOnly(why) => ClientSlot::ReadOnly(why),
            _ => panic!("map 10 must be read-only"),
        };
        let mut d = Doc::new(1, Some(10), None, shaiya_svmap::Svmap::new(64), None);
        d.set_client(slot);
        let cop = COp::Txt { before: Vec::new(), after: utf16_bom("0\tX\r\n") };
        let err = d.commit_client("rename", vec![cop.clone()]).unwrap_err();
        assert!(err.contains("client map is read-only") && err.contains("first difference"), "{err}");
        assert!(d.undo.is_empty() && !d.is_dirty());
        assert_eq!(d.client_edit_check().unwrap_err(), err);
        // the svmap side still edits, unpaired, and the client is never touched
        d.commit("add B", vec![Op::insert(0, Rec::B([1.0, 2.0, 3.0]))]);
        assert_eq!(d.undo.len(), 1);
        assert!(d.undo[0].cops.is_empty() && d.part().is_none() && !d.client_dirty());
        // a read-only tab with a client half refuses everything
        let mut d = paired_doc();
        let (map0, l0) = (d.map.clone(), lists(&d));
        d.read_only = true;
        assert!(d.commit_client("x", vec![cop]).unwrap_err().contains("read-only"));
        portal_move(&mut d, 0, 5.0, 5.0, 0.0);
        let txt = d.part().unwrap().txt.clone();
        let after = txt_with_name(&txt, 0, "Z").unwrap();
        assert!(d.commit_both("sync", Vec::new(), vec![COp::Txt { before: txt, after }]).is_err());
        d.commit_both("sync", vec![Op::insert(0, Rec::B([0.0; 3]))], Vec::new()).unwrap();
        assert_eq!((d.map.clone(), lists(&d)), (map0, l0));
        assert!(d.undo.is_empty() && !d.is_dirty() && d.refused >= 3);
    }

    /// A reload from disk drops the unsaved client edits with the history.
    #[test]
    fn reload_drops_client_edits() {
        let root = TempDir::new("c2-reload");
        let (_, m, _) = paired_fixture(64);
        let file = root.join("9.svmap");
        std::fs::write(&file, shaiya_svmap::write(&m)).unwrap();
        let mut d = Doc::open(1, &file).unwrap();
        let (w, _, txt) = paired_fixture(64);
        d.set_client(ClientSlot::Part(Box::new(ClientPart::from_model(crate::client_files::ClientWld::from_model(9, w), Some(txt)))));
        let l0 = lists(&d);
        portal_move(&mut d, 0, 9.0, 9.0, 0.0);
        assert!(d.client_dirty());
        d.reload_from_disk().unwrap();
        assert_eq!(lists(&d), l0);
        assert!(!d.is_dirty() && d.undo.is_empty());
    }

    /// A tab that gets a new client half (Save as to another number, another client folder, the same file read again after a
    /// save) keeps the svmap side of its history only: undo and redo never apply the old half's records to the new one.
    #[test]
    fn a_new_client_half_drops_the_client_side_of_the_history() {
        let mut d = paired_doc();
        let p = Portal { pos: [50.0, 3.0, 60.0], country: 2, level_min: 5, level_max: 60, to_map: 4, to_pos: [1.0, 2.0, 3.0] };
        d.commit("add portal", vec![Op::insert(1, Rec::Portal(p))]);
        portal_move(&mut d, 0, 3.0, 0.0, 0.0);
        d.mark_client_saved();
        // the half is read again (as written: it has the new portal and the moved box)
        let (w, _, txt) = paired_fixture(64);
        let mut fresh = ClientPart::from_model(crate::client_files::ClientWld::from_model(9, w), Some(txt));
        fresh.apply(&COp::Lists { before: Box::new(fresh.lists()), after: Box::new(lists(&d)) });
        d.set_client(ClientSlot::Part(Box::new(fresh)));
        let l1 = lists(&d);
        assert!(d.undo.iter().all(|s| s.cops.is_empty()));
        d.undo();
        d.undo();
        assert_eq!(lists(&d), l1, "undo leaves the new half alone");
        d.redo();
        d.redo();
        assert_eq!(lists(&d), l1, "redo does not insert the portal a second time");
        assert_eq!(d.part().unwrap().count(SectionId::Portals), 4);
        assert!(!d.client_dirty());
    }

    /// The destination name of a client portal is a client-only step, written in the page of the setting.
    #[test]
    fn dest_name_edit_is_one_client_step() {
        let mut d = paired_doc();
        let l0 = lists(&d);
        assert_eq!(d.part().unwrap().dest_name(0).as_deref(), Some("Home"));
        let before = d.part().unwrap().wld.model.portals[0].clone();
        let mut after = before.clone();
        d.part().unwrap().cp.set_fixed(&mut after.dest_name, "마을 입구").unwrap();
        d.commit_client("rename portal #0 destination", vec![COp::Portal(0, CKind::Replace { before, after })]).unwrap();
        assert_eq!(d.part().unwrap().dest_name(0).as_deref(), Some("마을 입구"));
        assert!(d.client_dirty() && !d.svmap_dirty());
        assert_eq!(d.part().unwrap().wld.model.portals[1], l0.portals[1], "the other portals keep their bytes");
        assert!(Codepage::Cp949.set_fixed(&mut [0u8; 248], &"x".repeat(300)).is_err(), "too long for the 248 bytes");
        assert!(Codepage::Latin1.set_fixed(&mut [0u8; 248], "마을").is_err(), "not in the page");
        d.undo();
        assert_eq!(lists(&d), l0);
    }

    // ------------------------------------------------------------------------------------------------ terrain

    fn corner_op(d: &Doc, cells: &[(u32, i16)]) -> COp {
        let h = &d.part().unwrap().terrain().unwrap().heights;
        COp::Terrain(TerrainDiff { cells: cells.iter().map(|(i, a)| (*i, h[*i as usize], *a)).collect() })
    }

    #[test]
    fn the_terrain_log_tells_a_view_what_to_redraw() {
        let mut d = paired_doc(); // 64 cells: 33 corners a side
        let n = 33u32;
        assert_eq!(d.part().unwrap().terrain_rev(), 0);
        assert_eq!(d.part().unwrap().terrain_dirty_since(0), TerrainDirty::Nothing);
        let op = corner_op(&d, &[(5 * n + 4, 9100), (7 * n + 9, 9100)]);
        d.commit_client("terrain", vec![op]).unwrap();
        let p = d.part().unwrap();
        assert_eq!(p.terrain_rev(), 1);
        assert_eq!(p.terrain_dirty_since(0), TerrainDirty::Rect([4, 5, 9, 7]));
        assert_eq!(p.terrain_dirty_since(1), TerrainDirty::Nothing);
        // undo is a change too (the views redraw it), and the revision never goes back
        d.undo();
        let p = d.part().unwrap();
        assert_eq!(p.terrain_rev(), 2);
        assert_eq!(p.terrain_dirty_since(1), TerrainDirty::Rect([4, 5, 9, 7]));
        assert_eq!(p.terrain_dirty_since(0), TerrainDirty::Rect([4, 5, 9, 7]));
        // a log that no longer reaches back asks for everything
        let mut log = TerrainLog::default();
        for i in 0..(TerrainLog::KEEP as u32 + 10) {
            log.push([i, 0, i, 0]);
        }
        assert_eq!(log.since(0), TerrainDirty::Full);
        assert_eq!(log.since(log.rev - 3), TerrainDirty::Rect([log.rev as u32 - 3, 0, log.rev as u32 - 1, 0]));
    }

    #[test]
    fn lists_carry_the_terrain_for_recovery_reload_and_changes() {
        let mut d = paired_doc();
        let h0 = d.part().unwrap().terrain().unwrap().heights.clone();
        let l0 = lists(&d);
        assert_eq!(l0.terrain, h0, "the full lists hold the heights, the core copy none");
        assert!(d.part().unwrap().lists_core().terrain.is_empty());
        let op = corner_op(&d, &[(40, 9500), (41, 9600)]);
        d.commit_client("terrain", vec![op]).unwrap();
        assert!(d.part().unwrap().changes().iter().any(|l| l == "client terrain: 2 corners changed"), "{:?}", d.part().unwrap().changes());
        // recovered work: the lists of the edited model applied to a fresh half give the edited heights
        let l1 = lists(&d);
        let (w, _, txt) = paired_fixture(64);
        let mut fresh = ClientPart::from_model(crate::client_files::ClientWld::from_model(9, w), Some(txt));
        fresh.apply(&COp::Lists { before: Box::new(fresh.lists()), after: Box::new(l1.clone()) });
        assert_eq!(fresh.terrain().unwrap().heights, l1.terrain);
        assert_eq!(fresh.terrain_dirty_since(0), TerrainDirty::Rect([0, 0, 32, 32]), "a restored half redraws everything");
        // reload from disk drops the edit and the terrain goes back
        let dir = TempDir::new("terrain-reload");
        let file = dir.join("9.svmap");
        std::fs::write(&file, shaiya_svmap::write(&d.map)).unwrap();
        d.path = Some(file);
        d.reload_from_disk().unwrap();
        assert_eq!(d.part().unwrap().terrain().unwrap().heights, h0);
        assert_eq!(lists(&d), l0);
        // an undone stroke gives back the heights exactly, and a list op for a grid of another size is ignored
        let mut other = l0.clone();
        other.terrain.truncate(10);
        d.part_mut().unwrap().apply(&COp::Lists { before: Box::new(l0.clone()), after: Box::new(other) });
        assert_eq!(d.part().unwrap().terrain().unwrap().heights, h0);
    }
}
