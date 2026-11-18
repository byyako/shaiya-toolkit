//! Adding a map end to end: the rules for a new map id, the files of a new map built from a
//! client map or a blank field, the minimap picture, and the ordered, verified writes.
//!
//! The pure parts (id rules, building, the picture) have no side effects. [`execute`] writes in this order: the staged `.wld`,
//! the staged `<id>.txt`, the minimap, a notes file in the staging folder, the `.svmap` in the server map folder, and last the
//! `Map.ini` zone. The zone goes last so the server never lists an id whose map file is not there yet. Nothing is written into
//! the client folder, `data.sah` or `data.saf`; every write is the verified save of `client_files::write_verified`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use shaiya_clientdata::area_txt::AreaTxt;
use shaiya_clientdata::wld::WldKind;
use shaiya_clientdata::wld_raw::{Codepage, RawTerrain, WldRaw};
use shaiya_svmap::Svmap;

use crate::client_files::{self, BackupPlan, ClientWld, Saved};
use crate::client_rules::ClientRules;
use crate::ini_doc::{IniDoc, IniKind};
use crate::mapini::WAR_FIRST;
use crate::keys::TargetServer;

/// The highest map id Afterglow takes.
pub const AFTERGLOW_MAX_ID: u32 = 255;
/// Never used as a map id (Afterglow keeps it for something else; the wizard refuses it for both targets).
pub const RESERVED_ID: u32 = 200;
/// The client forces this map size on map id 70.
pub const FORCED_SIZE_ID: u32 = 70;
pub const FORCED_SIZE: u32 = 1600;
/// The sizes a blank field offers.
pub const BLANK_SIZES: [u32; 2] = [1024, 2048];
/// Flat ground of a blank field: raw height 10000 is 0.0 units (`raw * 0.02 - 200`).
const FLAT_RAW: i16 = 10_000;
pub const MINIMAP_SIDE: usize = 512;

// ------------------------------------------------------------------------------------------------------------------ id rules

/// What the folders already hold, as far as the id rules need it.
#[derive(Clone, Debug, Default)]
pub struct Known {
    /// `[BASE] MapCount` of `Map.ini`; `None` when there is no readable `Map.ini`.
    pub map_count: Option<u32>,
    /// Ids the server loads (`Map.ini` 0..MapCount and `MapWar.ini` 100..).
    pub loaded: BTreeSet<u32>,
    /// Ids with an `.svmap` in the server map folder.
    pub svmaps: BTreeSet<u32>,
    /// Ids with a client map (the client's own files and staged ones).
    pub client: BTreeSet<u32>,
}

/// The ids of the staged `.wld` files of a staging folder (`<out>/data/world/<id>.wld`).
pub fn staged_ids(out: &Path) -> Vec<u32> {
    let dir = client_files::staged_path(out, "world");
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.strip_suffix(".wld").and_then(|s| s.parse::<u32>().ok())
        })
        .collect()
}

impl Known {
    /// What the id rules need from `Map.ini` and `MapWar.ini` (either may be missing), the ids that have an `.svmap` and the
    /// ids that have a client map.
    pub fn from_parts(map: Option<&crate::mapini::MapIni>, war: Option<&crate::mapini::MapIni>, svmaps: impl IntoIterator<Item = u32>, client: impl IntoIterator<Item = u32>) -> Known {
        let map_count = map.and_then(crate::mapini::MapIni::map_count);
        let mut loaded: BTreeSet<u32> = map_count.map(|n| (0..n).collect()).unwrap_or_default();
        if let Some(n) = war.and_then(crate::mapini::MapIni::map_count) {
            loaded.extend(WAR_FIRST..WAR_FIRST.saturating_add(n));
        }
        Known { map_count, loaded, svmaps: svmaps.into_iter().collect(), client: client.into_iter().collect() }
    }
}

/// The verdict on a map id: `refuse` stops the wizard, the warnings are shown and the user may go on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdCheck {
    pub refuse: Option<String>,
    pub warnings: Vec<String>,
}

impl IdCheck {
    pub fn ok(&self) -> bool {
        self.refuse.is_none()
    }
}

/// The only id the original server accepts for a new map: `MapCount`, the next id.
pub fn next_original_id(k: &Known) -> Option<u32> {
    k.map_count
}

/// The first free id for Afterglow (0 to 255, never 200, not loaded, no svmap, no client map).
pub fn first_free_afterglow(k: &Known) -> Option<u32> {
    (0..=AFTERGLOW_MAX_ID).find(|id| *id != RESERVED_ID && !k.loaded.contains(id) && !k.svmaps.contains(id) && !k.client.contains(id))
}

/// The rules for the id of a new map. The original server loads the ids 0 to `MapCount` - 1 and does not
/// start when one of them has no map file, so only the next id is allowed there. Afterglow skips a missing map file, so any free
/// id up to 255 works, except 200. The id 70 gets a warning: the client forces the size 1600 on it.
pub fn check_new_id(target: TargetServer, id: u32, k: &Known) -> IdCheck {
    let mut c = IdCheck::default();
    if id == RESERVED_ID {
        c.refuse = Some("Map id 200 is never used.".to_string());
        return c;
    }
    match target {
        TargetServer::Original => match k.map_count {
            None => c.refuse = Some("The original server needs Map.ini: its MapCount decides the next id. Set the server data folder first.".to_string()),
            Some(n) if id != n => {
                c.refuse = Some(format!(
                    "The original server loads the ids 0 to {} and does not start when one of them has no map file. Only the next id, {n}, is offered.",
                    n.saturating_sub(1)
                ))
            }
            _ => {}
        },
        TargetServer::Afterglow => {
            if id > AFTERGLOW_MAX_ID {
                c.refuse = Some(format!("Afterglow takes map ids up to {AFTERGLOW_MAX_ID}."));
            } else if k.loaded.contains(&id) {
                c.refuse = Some(format!("Map {id} is already in Map.ini or MapWar.ini."));
            }
        }
    }
    if c.refuse.is_none() && k.svmaps.contains(&id) {
        c.refuse = Some(format!("The map folder already has {id}.svmap."));
    }
    if c.refuse.is_none() && k.client.contains(&id) {
        c.refuse = Some(format!("The client already has a map with id {id}."));
    }
    if c.refuse.is_none() {
        if id == FORCED_SIZE_ID {
            c.warnings.push(format!("The client forces the map size {FORCED_SIZE} on map {FORCED_SIZE_ID}. A map of another size is not shown right there."));
        }
        if (100..=RESERVED_ID).contains(&id) {
            c.warnings.push("The client refuses some item uses on maps 100 to 200.".to_string());
        }
    }
    c
}

/// The rules for a new `[SET_ZONE_n]` made by the Map.ini editor. `Map.ini`: as [`check_new_id`] but the map files must already
/// exist (the original server does not start without the `.svmap`). `MapWar.ini`: only the next war id, `100 + MapCount`.
pub fn check_new_zone(kind: IniKind, target: TargetServer, id: u32, k: &Known, war_count: u32, svmap_folder_known: bool) -> IdCheck {
    let mut c = IdCheck::default();
    if id == RESERVED_ID {
        c.refuse = Some("Map id 200 is never used.".to_string());
        return c;
    }
    match kind {
        IniKind::War => {
            let next = WAR_FIRST + war_count;
            if id != next {
                c.refuse = Some(format!("The war server counts its maps from 100. Only the next war id, {next}, is offered."));
            }
            return c;
        }
        IniKind::Map => {}
    }
    match target {
        TargetServer::Original => match k.map_count {
            None => c.refuse = Some("Map.ini has no MapCount.".to_string()),
            Some(n) if id != n => c.refuse = Some(format!("The original server needs a map file for every id below MapCount. Only the next id, {n}, is offered.")),
            _ => {}
        },
        TargetServer::Afterglow => {
            if id > AFTERGLOW_MAX_ID {
                c.refuse = Some(format!("Afterglow takes map ids up to {AFTERGLOW_MAX_ID}."));
            } else if k.loaded.contains(&id) {
                c.refuse = Some(format!("Map {id} is already in Map.ini."));
            }
        }
    }
    if c.refuse.is_none() && !k.svmaps.contains(&id) {
        let why = format!("The map folder has no {id}.svmap.");
        match (target, svmap_folder_known) {
            (TargetServer::Original, true) => c.refuse = Some(format!("{why} The original server does not start without it. Make the map first (Tools > Maps > Add a map...).")),
            (_, true) => c.warnings.push(format!("{why} Afterglow skips the id until the file is there.")),
            (_, false) => c.warnings.push("The map folder is not open, so the .svmap of this id was not checked.".to_string()),
        }
    }
    if c.refuse.is_none() && id == FORCED_SIZE_ID {
        c.warnings.push(format!("The client forces the map size {FORCED_SIZE} on map {FORCED_SIZE_ID}."));
    }
    c
}

// ------------------------------------------------------------------------------------------------------------------ building

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A copy of a client map: its `.wld`, its area names, and its NPCs, portals, regions and bind areas as the `.svmap`.
    Copy(u32),
    /// A flat field of `size` that takes textures, water, sky, colours and fog from the field map `look`.
    Blank { size: u32, look: u32 },
}

#[derive(Clone, Debug)]
pub struct Wanted {
    pub id: u32,
    pub source: Source,
    pub minimap: bool,
    /// A name the user wants for the map. The client's name table is not edited; the name goes into the notes file.
    pub name: String,
    pub target: TargetServer,
}

/// What the dialog shows about a client map.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceInfo {
    pub dungeon: bool,
    pub size: u32,
    pub counts: String,
}

pub fn source_info(rules: &ClientRules, id: u32) -> Result<SourceInfo, String> {
    let w = rules.wld(id).ok_or_else(|| format!("world/{id}.wld could not be read"))?;
    let counts = format!("{} NPCs, {} portals, {} regions, {} bind areas", w.npcs.len(), w.portals.len(), w.areas.len(), w.zone_groups.len());
    match (&w.terrain, w.kind == WldKind::Dungeon) {
        (Some(t), _) => Ok(SourceInfo { dungeon: false, size: t.size, counts }),
        (None, true) => {
            let dg = rules.source.dg(&w.dg).map_err(|e| e.to_string())?;
            Ok(SourceInfo { dungeon: true, size: shaiya_clientdata::dg::dungeon_map_size(&dg.bounds), counts })
        }
        _ => Err("this .wld has neither terrain nor a dungeon".to_string()),
    }
}

/// Everything a new map is made of, in memory.
pub struct Built {
    pub id: u32,
    pub wld: WldRaw,
    pub txt: Vec<u8>,
    pub minimap: Option<Vec<u8>>,
    /// The terrain chunk textures of a field, `(path below the data folder, bytes)`: `world/Field/<id>/<id>_<r><c>_L.dds` and
    /// `_A<k>.dds`. Empty for a dungeon, and for a copied field whose source has none. [`execute`] writes them after the minimap.
    pub terrain_files: Vec<(String, Vec<u8>)>,
    pub svmap: Svmap,
    pub zone_keys: Vec<(String, String)>,
    pub notes: String,
    /// The server the id was checked for: [`execute`] checks it again against `Map.ini` as it is on disk then.
    pub target: TargetServer,
}

/// The things the wizard cannot do. The first lines are the same for every map; the last depend on the target.
pub fn cannot_do(id: u32, has_name: bool, dungeon: bool, target: TargetServer) -> Vec<String> {
    let mut v = Vec::new();
    if has_name {
        v.push(format!("Map name: the client already has a name for map {id}."));
    } else {
        v.push(format!("Map name: the client shows \"Map {id}\" until a name is added to its text tables. The editor never edits those."));
    }
    if !dungeon {
        v.push(format!("Terrain textures (world/Field/{id}/): a blank field gets a slope-shaded light map and plain masks; a copy gets its source's files renamed. Painting them is up to you."));
    } else {
        v.push("Terrain textures: a dungeon has no world/Field folder, so none are made.".to_string());
    }
    v.push(format!("Loading picture {id}.bmp: not made. Without it the loading screen of map {id} has no picture."));
    v.push("Music zones: a copy keeps the music names of its source. A blank field has none.".to_string());
    v.push("Rebirth positions: the new Map.ini zone has none. Set them in the Map.ini editor.".to_string());
    if dungeon {
        v.push("Blocked cells: the new map starts with every cell free. Use Tools > Client > Generate blocked cells.".to_string());
    }
    if target == TargetServer::Afterglow {
        v.push("Afterglow reads its map files at tools/world/import.py: run it again.".to_string());
    }
    v
}

/// Builds the files of a new map. `has_name`: the client's name table already names the id.
pub fn build(rules: &ClientRules, w: &Wanted, has_name: bool) -> Result<Built, String> {
    let (wld, txt, size, dungeon) = match &w.source {
        Source::Copy(src) => {
            let info = source_info(rules, *src)?;
            let model = ClientWld::open(rules, *src)?.model;
            let txt = match rules.read_file(&format!("world/{src}.txt")) {
                Ok((b, _)) => b,
                Err(_) => vec![0xFF, 0xFE],
            };
            (model, txt, info.size, info.dungeon)
        }
        Source::Blank { size, look } => {
            if !BLANK_SIZES.contains(size) {
                return Err(format!("A blank field is {} or {} units wide.", BLANK_SIZES[0], BLANK_SIZES[1]));
            }
            (blank_field(rules, *size, *look)?, vec![0xFF, 0xFE], *size, false)
        }
    };
    let bad = wld.validate();
    if !bad.is_empty() {
        return Err(format!("The new map would not read back: {}", bad.join("; ")));
    }
    // the txt must be a text the editor can write back
    if AreaTxt::parse(&txt).to_bytes() != txt {
        return Err("the source's area names do not read back unchanged".to_string());
    }
    let minimap = match (&wld.terrain, w.minimap) {
        (Some(t), true) => Some(minimap_tga(t)?),
        _ => None,
    };
    // a field needs its chunk textures; a dungeon keeps its ground in the .dg model and has no Field folder
    let terrain_files = match (&w.source, &wld.terrain) {
        (Source::Blank { .. }, Some(t)) => terrain_textures(w.id, t, wld.textures.len()),
        (Source::Copy(src), Some(t)) => {
            // a chunk whose light map the source lacks would stop the client's map load: it gets a generated one
            let mut files = copy_terrain_files(rules, *src, w.id, size);
            if terrain_light_maps(w.id, t).iter().any(|n| !files.iter().any(|(f, _)| f == n)) {
                for (name, bytes) in terrain_textures(w.id, t, wld.textures.len()) {
                    if name.ends_with("_L.dds") && !files.iter().any(|(f, _)| *f == name) {
                        files.push((name, bytes));
                    }
                }
            }
            files
        }
        _ => Vec::new(),
    };
    let svmap = match &w.source {
        Source::Copy(_) => {
            let decoded = wld.to_wld(Codepage::Cp949);
            let target = crate::sync::target_from_wld(&decoded);
            let mut d = crate::doc::Doc::new_unsaved(0, Some(w.id), Svmap::new(size));
            let plan = crate::sync::plan(&d.map, &target);
            let ops = plan.ops(&d.map);
            d.commit("fill from the client map", ops);
            d.map
        }
        Source::Blank { .. } => Svmap::new(size),
    };
    let zone_keys = vec![("MapType".to_string(), if dungeon { "D" } else { "F" }.to_string())];
    let cannot = cannot_do(w.id, has_name, dungeon, w.target);
    let notes = notes_text(w, size, dungeon, has_name, &cannot);
    Ok(Built { id: w.id, wld, txt, minimap, terrain_files, svmap, zone_keys, notes, target: w.target })
}

/// A flat field with the textures, water, sky, colours and fog of the field map `look`. Placements, NPCs, portals, areas,
/// bind areas, sounds and effects are empty.
fn blank_field(rules: &ClientRules, size: u32, look: u32) -> Result<WldRaw, String> {
    let mut m = ClientWld::open(rules, look)?.model;
    if m.kind != WldKind::Field || m.terrain.is_none() {
        return Err(format!("Map {look} is a dungeon. A blank field takes its textures and sky from a field map."));
    }
    let n = size as usize / 2 + 1;
    m.terrain = Some(RawTerrain { size: size as i32, heights: vec![FLAT_RAW; n * n], tex_index: vec![0; n * n] });
    m.building = Default::default();
    m.shape = Default::default();
    m.tree = Default::default();
    m.grass = Default::default();
    m.vani = Default::default();
    m.vani2 = Default::default();
    m.dungeon = Default::default();
    m.mani = Default::default();
    m.effects = Vec::new();
    m.object = Default::default();
    m.object2 = Default::default();
    m.sound = Default::default();
    m.boxes = Vec::new();
    m.portals = Vec::new();
    m.zone_groups = Vec::new();
    m.areas = Vec::new();
    m.npcs = Vec::new();
    Ok(m)
}

fn notes_text(w: &Wanted, size: u32, dungeon: bool, has_name: bool, cannot: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("New map {} made by svmap-editor\r\n", w.id));
    s.push_str(&format!("Target server: {}\r\n", w.target.label()));
    match &w.source {
        Source::Copy(src) => s.push_str(&format!("Source: a copy of client map {src} ({}, size {size})\r\n", if dungeon { "dungeon" } else { "field" })),
        Source::Blank { size, look } => s.push_str(&format!("Source: a flat field of {size} units; textures, sky and fog from map {look}\r\n")),
    }
    if !w.name.trim().is_empty() {
        s.push_str(&format!("Name you chose: {}\r\n", w.name.trim()));
        if has_name {
            s.push_str("The client already has a name for this id. Yours was not written anywhere.\r\n");
        } else {
            s.push_str("Your name was not written into the game. Add it where the client keeps its map names; the choice is yours.\r\n");
        }
    }
    s.push_str("\r\nNot done:\r\n");
    for c in cannot {
        s.push_str(&format!("- {c}\r\n"));
    }
    s
}

// ------------------------------------------------------------------------------------------------------------------ the minimap

fn height_at(t: &RawTerrain, x: f32, z: f32) -> f32 {
    let n = t.corners();
    let g = |cx: usize, cz: usize| t.heights[cz.min(n - 1) * n + cx.min(n - 1)] as f32 * 0.02 - 200.0;
    let (fx, fz) = ((x / 2.0).clamp(0.0, (n - 1) as f32), (z / 2.0).clamp(0.0, (n - 1) as f32));
    let (cx, cz) = (fx.floor() as usize, fz.floor() as usize);
    let (tx, tz) = (fx - cx as f32, fz - cz as f32);
    let a = g(cx, cz) * (1.0 - tx) + g(cx + 1, cz) * tx;
    let b = g(cx, cz + 1) * (1.0 - tx) + g(cx + 1, cz + 1) * tx;
    a * (1.0 - tz) + b * tz
}

/// The 26-byte footer of a Targa 2.0 file (no extension or developer area). The client's own minimaps end with it.
const TGA_FOOTER: &[u8; 26] = b"\0\0\0\0\0\0\0\0TRUEVISION-XFILE.\0";

/// The minimap of a field: a 512 x 512, 24-bit, uncompressed Targa, bottom-up, shaded from the heights (a hillshade with the light
/// from the north-west, a green to sand tint by height, blue under -0.4). The top row is the north edge (the highest z).
pub fn minimap_tga(t: &RawTerrain) -> Result<Vec<u8>, String> {
    let n = t.corners();
    if t.size <= 0 || t.heights.len() != n * n {
        return Err("the terrain grid does not match its size".to_string());
    }
    let side = MINIMAP_SIDE;
    let unit = t.size as f32 / side as f32;
    let mut out = Vec::with_capacity(18 + side * side * 3 + TGA_FOOTER.len());
    out.extend_from_slice(&[0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    out.extend_from_slice(&(side as u16).to_le_bytes());
    out.extend_from_slice(&(side as u16).to_le_bytes());
    out.extend_from_slice(&[24, 0]);
    // rows are stored bottom-up: the first stored row is the south edge
    for row in 0..side {
        let z = (row as f32 + 0.5) * unit;
        for col in 0..side {
            let x = (col as f32 + 0.5) * unit;
            let h = height_at(t, x, z);
            let d = 2.0;
            let sx = (height_at(t, x + d, z) - height_at(t, x - d, z)) / (2.0 * d);
            let sz = (height_at(t, x, z + d) - height_at(t, x, z - d)) / (2.0 * d);
            // light from the north-west: slopes that fall towards the north-west are lit
            let light = (0.85 + 1.4 * (-sx * 0.6 + sz * 0.6)).clamp(0.35, 1.5);
            let (r, g, b) = if h < -0.4 {
                let depth = ((-h - 0.4) / 20.0).clamp(0.0, 1.0);
                (50.0 - 20.0 * depth, 90.0 - 30.0 * depth, 150.0 - 40.0 * depth)
            } else {
                let k = ((h + 10.0) / 140.0).clamp(0.0, 1.0);
                ((70.0 + 110.0 * k) * light, (115.0 + 55.0 * k) * light, (60.0 + 70.0 * k) * light)
            };
            out.extend_from_slice(&[b.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, r.clamp(0.0, 255.0) as u8]);
        }
    }
    out.extend_from_slice(TGA_FOOTER);
    Ok(out)
}

// ------------------------------------------------------------------------------------------------------------------ writing

/// Where the files go.
pub struct Dirs<'a> {
    /// The staging folder (`client_out`); the client's files go to `<out>/data/...`.
    pub out: &'a Path,
    /// The client folder: nothing is written inside it.
    pub client: &'a Path,
    /// The server map folder (`<id>.svmap`).
    pub map_dir: &'a Path,
    /// The server data folder (`Map.ini`).
    pub server_dir: &'a Path,
    /// `(<data>\backups\client\<key>, copies to keep)` for staged files.
    pub client_backups: Option<(PathBuf, u32)>,
    /// `(<data>\backups\server\<key>, copies to keep)` for `Map.ini`.
    pub server_backups: Option<(PathBuf, u32)>,
}

/// The files that were written, in order, each with what it is.
#[derive(Debug, Default)]
pub struct Report {
    pub written: Vec<(String, PathBuf)>,
}

fn write_text_like(target: &Path, bytes: &[u8], rel: &str, d: &Dirs) -> Result<Saved, String> {
    let (stem, ext) = client_files::backup_stem(rel);
    let plan = d.client_backups.as_ref().map(|(dir, keep)| BackupPlan { dir, stem: &stem, ext: &ext, keep: *keep });
    let want = bytes.to_vec();
    let verify = move |back: &[u8]| -> Result<(), String> {
        if back == want.as_slice() {
            Ok(())
        } else {
            Err("the file differs from what was meant to be written".to_string())
        }
    };
    client_files::write_verified(target, bytes, &verify, plan, Some(d.client))
}

/// Writes the new map. On an error the report holds what was written before it (new files the user may delete); nothing is
/// rolled back. Refuses before the first write when the svmap file or the zone is already there.
pub fn execute(b: &Built, d: &Dirs) -> Result<Report, (Report, String)> {
    let mut rep = Report::default();
    let id = b.id;
    let fail = |rep: Report, e: String| Err((rep, e));
    // checks before the first write
    if let Err(e) = client_files::check_out_dir(d.out, d.client) {
        return fail(rep, e);
    }
    let svmap_path = d.map_dir.join(format!("{id}.svmap"));
    if svmap_path.exists() {
        return fail(rep, format!("{} already exists. Nothing was written.", svmap_path.display()));
    }
    let mut ini = match IniDoc::open(IniKind::Map, &d.server_dir.join("Map.ini")) {
        Ok(i) => i,
        Err(e) => return fail(rep, e),
    };
    // the id rules again, on the Map.ini read now (the window's copy may be older)
    if id == RESERVED_ID {
        return fail(rep, "Map id 200 is never used. Nothing was written.".to_string());
    }
    match b.target {
        TargetServer::Original if ini.ini.map_count() != Some(id) => {
            return fail(rep, format!("Map.ini now has MapCount {}; the original server takes only that id. Nothing was written.", ini.ini.map_count().unwrap_or(0)));
        }
        TargetServer::Afterglow if id > AFTERGLOW_MAX_ID => {
            return fail(rep, format!("Afterglow takes map ids up to {AFTERGLOW_MAX_ID}. Nothing was written."));
        }
        _ => {}
    }
    let keys: Vec<(&str, &str)> = b.zone_keys.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    if let Err(e) = ini.add_zone(id, &keys) {
        return fail(rep, format!("{e} Nothing was written."));
    }
    // 1. the client map
    let mut wld = ClientWld::new_map(id, b.wld.clone(), d.client);
    let backups = d.client_backups.as_ref().map(|(p, k)| (p.as_path(), *k));
    match wld.save(d.out, backups) {
        Ok(s) => rep.written.push((format!("client map world/{id}.wld"), s.path)),
        Err(e) => return fail(rep, e),
    }
    // 2. the area names
    let rel = format!("world/{id}.txt");
    match write_text_like(&client_files::staged_path(d.out, &rel), &b.txt, &rel, d) {
        Ok(s) => rep.written.push((format!("area names {rel}"), s.path)),
        Err(e) => return fail(rep, e),
    }
    // 3. the minimap
    if let Some(tga) = &b.minimap {
        let rel = format!("interface/MiniMap/{id}.tga");
        match write_text_like(&client_files::staged_path(d.out, &rel), tga, &rel, d) {
            Ok(s) => rep.written.push((format!("minimap {rel}"), s.path)),
            Err(e) => return fail(rep, e),
        }
    }
    // 3b. the terrain chunk textures (fields only)
    for (rel, bytes) in &b.terrain_files {
        match write_text_like(&client_files::staged_path(d.out, rel), bytes, rel, d) {
            Ok(s) => rep.written.push((format!("terrain texture {rel}"), s.path)),
            Err(e) => return fail(rep, e),
        }
    }
    // 4. the notes, beside `data`, so a patch built from `data` does not carry them
    let notes_path = d.out.join(format!("svmap-editor-new-map-{id}.txt"));
    match write_text_like(&notes_path, b.notes.as_bytes(), &format!("notes/{id}.txt"), d) {
        Ok(s) => rep.written.push(("notes (what is not done)".to_string(), s.path)),
        Err(e) => return fail(rep, e),
    }
    // 5. the server map
    let bytes = shaiya_svmap::write(&b.svmap);
    let verify = |back: &[u8]| -> Result<(), String> {
        let m = shaiya_svmap::read(back).map_err(|e| e.to_string())?;
        if m == b.svmap {
            Ok(())
        } else {
            Err("the map differs from the model".to_string())
        }
    };
    match client_files::write_verified(&svmap_path, &bytes, &verify, None, Some(d.client)) {
        Ok(s) => rep.written.push((format!("server map {id}.svmap"), s.path)),
        Err(e) => return fail(rep, e),
    }
    // 6. Map.ini, last
    let backups = d.server_backups.as_ref().map(|(p, k)| (p.as_path(), *k));
    match ini.save(backups, Some(d.client)) {
        Ok(s) => rep.written.push((format!("Map.ini zone {id} and MapCount"), s.path)),
        Err(e) => return fail(rep, e),
    }
    Ok(rep)
}

// ------------------------------------------------------------------------------------------------------------------ terrain textures

/// Terrain chunks per side: a field is cut into 1024 x 1024 unit chunks (a map smaller than that has one).
pub fn chunks_per_side(size: u32) -> u32 {
    (size / 1024).max(1)
}

/// The chunk textures a field needs, as `(path relative to the client's data folder, bytes)`: for every chunk (row r, column c,
/// `chunks_per_side` each way) `world/Field/<id>/<id>_<r><c>_L.dds` (1024 x 1024 DXT1 light map, slope shaded to about the
/// brightness of the original maps) and `<id>_<r><c>_A<k>.dds` (256 x 256 DXT5, alpha = where terrain texture k lies; A0 is the
/// opaque base, the others are painted over it as in the original maps). Image rows run along +z (measured on map 105). The
/// client shows a popup and stops loading when a `_L.dds` is missing; a missing `_A<k>.dds` is not fatal.
pub fn terrain_textures(id: u32, t: &RawTerrain, textures: usize) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let per = chunks_per_side(t.size as u32) as usize;
    let n = t.size as usize / 2 + 1;
    let hu = |cx: usize, cz: usize| crate::terrain::units_of(t.heights[cz.min(n - 1) * n + cx.min(n - 1)]);
    for r in 0..per {
        for c in 0..per {
            let (oz, ox) = (r * 512, c * 512); // the chunk's first corner
            let side = 1024usize;
            let mut light = vec![0u8; side * side];
            let sun = [0.45f32, 0.75, 0.48];
            let sl = (sun[0] * sun[0] + sun[1] * sun[1] + sun[2] * sun[2]).sqrt();
            for py in 0..side {
                let cz = oz + py / 2;
                for px in 0..side {
                    let cx = ox + px / 2;
                    let gx = (hu(cx + 1, cz) - hu(cx.saturating_sub(1), cz)) / 4.0;
                    let gz = (hu(cx, cz + 1) - hu(cx, cz.saturating_sub(1))) / 4.0;
                    let nl = (gx * gx + 1.0 + gz * gz).sqrt();
                    let d = (-gx * sun[0] + sun[1] - gz * sun[2]) / (nl * sl);
                    light[py * side + px] = (255.0 * (0.5 + 0.45 * d).clamp(0.25, 1.0)) as u8;
                }
            }
            let mut l = dds_header(1024, 11, b"DXT1");
            for (img, w) in mip_chain(light, side) {
                l.extend(dxt1_gray(&img, w));
            }
            files.push((format!("world/Field/{id}/{id}_{r}{c}_L.dds"), l));
            let a_side = 256usize;
            for k in 0..textures.min(8) {
                let mut mask = vec![0u8; a_side * a_side];
                for py in 0..a_side {
                    let cz0 = oz + py * 2;
                    for px in 0..a_side {
                        let cx0 = ox + px * 2;
                        let mut hit = 0u32;
                        for dz in 0..3usize {
                            for dx in 0..3usize {
                                let (cx, cz) = ((cx0 + dx).min(n - 1), (cz0 + dz).min(n - 1));
                                hit += (t.tex_index[cz * n + cx] as usize == k) as u32;
                            }
                        }
                        // measured on map 105: the mask is exactly 255 where the corner at (2 px, 2 py) uses texture k
                        let centre = t.tex_index[cz0.min(n - 1) * n + cx0.min(n - 1)] as usize == k;
                        mask[py * a_side + px] = if k == 0 || centre { 255 } else { (hit * 255 / 18) as u8 };
                    }
                }
                let mut a = dds_header(256, 9, b"DXT5");
                for (img, w) in mip_chain(mask, a_side) {
                    a.extend(dxt5_alpha(&img, w));
                }
                files.push((format!("world/Field/{id}/{id}_{r}{c}_A{k}.dds"), a));
            }
        }
    }
    for (name, bytes) in &files {
        let want = if name.ends_with("_L.dds") { 699_192 } else { 87_536 };
        assert_eq!(bytes.len(), want, "{name} has the size of the originals");
    }
    files
}

/// The light map names (`world/Field/<id>/<id>_<r><c>_L.dds`) the client needs for every chunk of the field.
fn terrain_light_maps(id: u32, t: &RawTerrain) -> Vec<String> {
    let per = chunks_per_side(t.size as u32);
    (0..per).flat_map(|r| (0..per).map(move |c| format!("world/Field/{id}/{id}_{r}{c}_L.dds"))).collect()
}

/// The terrain files of a copied map: the source's `world/Field/<src>/*` renamed to the new id. Only the names the client loads
/// are tried (`_L` and `_A0` to `_A7` of every chunk); a file the source lacks is skipped.
fn copy_terrain_files(rules: &ClientRules, src: u32, id: u32, size: u32) -> Vec<(String, Vec<u8>)> {
    let per = chunks_per_side(size);
    let mut out = Vec::new();
    for r in 0..per {
        for c in 0..per {
            let mut tails = vec!["L".to_string()];
            tails.extend((0..8).map(|k| format!("A{k}")));
            for tail in tails {
                if let Ok((b, _)) = rules.read_file(&format!("world/Field/{src}/{src}_{r}{c}_{tail}.dds")) {
                    out.push((format!("world/Field/{id}/{id}_{r}{c}_{tail}.dds"), b));
                }
            }
        }
    }
    out
}

/// The DDS header the client's own field textures use (124 bytes + magic, `flags` 0x21007, caps 0x401008).
fn dds_header(side: u32, mips: u32, fourcc: &[u8; 4]) -> Vec<u8> {
    let mut v = Vec::with_capacity(128);
    for x in [0x2053_4444u32, 124, 135_175, side, side, 0, 0, mips] {
        v.extend(x.to_le_bytes());
    }
    v.extend([0u8; 44]);
    for x in [32u32, 4] {
        v.extend(x.to_le_bytes());
    }
    v.extend(fourcc);
    v.extend([0u8; 20]);
    v.extend(4_198_408u32.to_le_bytes());
    v.extend([0u8; 16]);
    assert_eq!(v.len(), 128);
    v
}

/// Box-filtered 8 bit mip chain from `side` down to 1 x 1.
fn mip_chain(base: Vec<u8>, side: usize) -> Vec<(Vec<u8>, usize)> {
    let mut out = vec![(base, side)];
    while out.last().unwrap().1 > 1 {
        let (src, w) = out.last().unwrap();
        let w = *w;
        let h = w / 2;
        let mut dst = vec![0u8; h * h];
        for y in 0..h {
            for x in 0..h {
                let sum = src[2 * y * w + 2 * x] as u32 + src[2 * y * w + 2 * x + 1] as u32 + src[(2 * y + 1) * w + 2 * x] as u32 + src[(2 * y + 1) * w + 2 * x + 1] as u32;
                dst[y * h + x] = ((sum + 2) / 4) as u8;
            }
        }
        out.push((dst, h));
    }
    out
}

fn rgb565_gray(v: i32) -> u16 {
    let (r, g) = ((v as u32 * 31 + 127) / 255, (v as u32 * 63 + 127) / 255);
    ((r << 11) | (g << 5) | r) as u16
}

/// DXT1 of a grey picture (opaque, 4 colour mode).
fn dxt1_gray(px: &[u8], w: usize) -> Vec<u8> {
    let b = w.div_ceil(4).max(1);
    let mut out = Vec::with_capacity(b * b * 8);
    for by in 0..b {
        for bx in 0..b {
            let get = |i: usize| px[(by * 4 + i / 4).min(w - 1) * w + (bx * 4 + i % 4).min(w - 1)] as i32;
            let (mut hi, mut lo) = (0, 255);
            for i in 0..16 {
                hi = hi.max(get(i));
                lo = lo.min(get(i));
            }
            let (c0, c1) = (rgb565_gray(hi), rgb565_gray(lo));
            out.extend(c0.to_le_bytes());
            out.extend(c1.to_le_bytes());
            let pal = [hi, lo, (2 * hi + lo) / 3, (hi + 2 * lo) / 3];
            let mut bits = 0u32;
            if c0 != c1 {
                for i in 0..16 {
                    let k = (0..4).min_by_key(|k| (pal[*k] - get(i)).abs()).unwrap();
                    bits |= (k as u32) << (2 * i);
                }
            }
            out.extend(bits.to_le_bytes());
        }
    }
    out
}

/// DXT5 whose colour part is plain white and whose alpha carries the 8 bit picture.
fn dxt5_alpha(px: &[u8], w: usize) -> Vec<u8> {
    let b = w.div_ceil(4).max(1);
    let mut out = Vec::with_capacity(b * b * 16);
    for by in 0..b {
        for bx in 0..b {
            let get = |i: usize| px[(by * 4 + i / 4).min(w - 1) * w + (bx * 4 + i % 4).min(w - 1)] as i32;
            let (mut hi, mut lo) = (0, 255);
            for i in 0..16 {
                hi = hi.max(get(i));
                lo = lo.min(get(i));
            }
            out.push(hi as u8);
            out.push(lo as u8);
            let mut pal = [hi, lo, 0, 0, 0, 0, 0, 0];
            for k in 1..7 {
                pal[k + 1] = ((7 - k as i32) * hi + k as i32 * lo) / 7;
            }
            let mut bits = 0u64;
            if hi != lo {
                for i in 0..16 {
                    let k = (0..8).min_by_key(|k| (pal[*k] - get(i)).abs()).unwrap();
                    bits |= (k as u64) << (3 * i);
                }
            }
            out.extend(&bits.to_le_bytes()[..6]);
            out.extend(0xFFFFu16.to_le_bytes());
            out.extend(0xFFFFu16.to_le_bytes());
            out.extend([0u8; 4]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(count: u32) -> Known {
        Known { map_count: Some(count), loaded: (0..count).collect(), svmaps: (0..count).collect(), client: (0..count).collect() }
    }

    /// The id rules: the original takes the next id only, Afterglow any free id up to 255 except 200, id 70 warns.
    #[test]
    fn new_map_id_rules() {
        let k = known(68);
        assert!(check_new_id(TargetServer::Original, 68, &k).ok());
        for id in [0, 5, 67, 69, 100, 200, 255] {
            assert!(check_new_id(TargetServer::Original, id, &k).refuse.is_some(), "original {id}");
        }
        let why = check_new_id(TargetServer::Original, 90, &k).refuse.unwrap();
        assert!(why.contains("does not start") && why.contains("68"), "{why}");
        assert!(check_new_id(TargetServer::Original, 68, &Known::default()).refuse.is_some(), "no Map.ini");
        assert_eq!(next_original_id(&k), Some(68));
        // Afterglow
        assert!(check_new_id(TargetServer::Afterglow, 150, &k).ok(), "a gap is fine");
        assert!(check_new_id(TargetServer::Afterglow, 255, &k).ok());
        assert!(check_new_id(TargetServer::Afterglow, 256, &k).refuse.is_some());
        assert!(check_new_id(TargetServer::Afterglow, 200, &k).refuse.is_some(), "never 200");
        assert!(check_new_id(TargetServer::Original, 200, &known(200)).refuse.is_some(), "never 200, on the original too");
        assert!(check_new_id(TargetServer::Afterglow, 5, &k).refuse.is_some(), "loaded");
        let mut k2 = known(68);
        k2.loaded.clear();
        k2.client.clear();
        assert!(check_new_id(TargetServer::Afterglow, 5, &k2).refuse.unwrap().contains("5.svmap"), "svmap exists");
        k2.svmaps.clear();
        k2.client.insert(5);
        assert!(check_new_id(TargetServer::Afterglow, 5, &k2).refuse.unwrap().contains("client"));
        assert_eq!(first_free_afterglow(&k), Some(68));
        let mut full = known(68);
        full.loaded.extend(68..=255);
        assert_eq!(first_free_afterglow(&full), None);
        let mut gap = known(68);
        gap.loaded.extend(68..200);
        assert_eq!(first_free_afterglow(&gap), Some(201), "200 is skipped");
        // warnings
        let mut k70 = known(70);
        k70.map_count = Some(70);
        let c = check_new_id(TargetServer::Original, 70, &k70);
        assert!(c.ok() && c.warnings.iter().any(|w| w.contains("1600")), "{c:?}");
        assert!(check_new_id(TargetServer::Afterglow, 70, &known(10)).warnings.iter().any(|w| w.contains("1600")));
        assert!(check_new_id(TargetServer::Afterglow, 150, &k).warnings.iter().any(|w| w.contains("100 to 200")));
        assert!(check_new_id(TargetServer::Afterglow, 90, &k).warnings.is_empty());
        // the Map.ini editor's zones
        let z = |kind, target, id, k: &Known, war| check_new_zone(kind, target, id, k, war, true);
        let mut with = known(68);
        with.svmaps.insert(68);
        assert!(z(IniKind::Map, TargetServer::Original, 68, &with, 0).ok());
        assert!(z(IniKind::Map, TargetServer::Original, 68, &known(68), 0).refuse.unwrap().contains("68.svmap"), "the svmap must exist first");
        assert!(z(IniKind::Map, TargetServer::Afterglow, 150, &known(68), 0).ok(), "Afterglow warns only");
        assert!(!z(IniKind::Map, TargetServer::Afterglow, 150, &known(68), 0).warnings.is_empty());
        assert!(z(IniKind::War, TargetServer::Original, 109, &known(68), 9).ok());
        assert!(z(IniKind::War, TargetServer::Original, 110, &known(68), 9).refuse.is_some());
        assert!(z(IniKind::War, TargetServer::Original, 108, &known(68), 9).refuse.is_some());
    }

    fn terrain(size: i32, f: impl Fn(usize, usize) -> f32) -> RawTerrain {
        let n = size as usize / 2 + 1;
        let mut heights = vec![0i16; n * n];
        for cz in 0..n {
            for cx in 0..n {
                heights[cz * n + cx] = ((f(cx, cz) + 200.0) / 0.02).round() as i16;
            }
        }
        RawTerrain { size, heights, tex_index: vec![0; n * n] }
    }

    fn pixel(tga: &[u8], col: usize, row_from_top: usize) -> [u8; 3] {
        // stored bottom-up
        let row = MINIMAP_SIDE - 1 - row_from_top;
        let o = 18 + (row * MINIMAP_SIDE + col) * 3;
        [tga[o + 2], tga[o + 1], tga[o]]
    }

    /// 512 x 512, 24 bit, uncompressed, bottom-up; flat ground is one colour, a hill is shaded, water is blue, the north is on top.
    #[test]
    fn minimap_tga_512() {
        for size in [1024, 2048] {
            let flat = minimap_tga(&terrain(size, |_, _| 5.0)).unwrap();
            assert_eq!(flat.len(), 18 + 512 * 512 * 3 + 26, "header, pixels and the 26-byte footer the client's own minimaps have");
            assert!(flat.ends_with(b"TRUEVISION-XFILE.\0"));
            assert_eq!(&flat[..3], &[0, 0, 2], "no id field, no colour map, uncompressed true colour");
            assert_eq!(u16::from_le_bytes([flat[12], flat[13]]), 512);
            assert_eq!(u16::from_le_bytes([flat[14], flat[15]]), 512);
            assert_eq!(flat[16], 24);
            assert_eq!(flat[17] & 0x20, 0, "bottom-up");
            let first = &flat[18..21];
            assert!(flat[18..18 + 512 * 512 * 3].chunks(3).all(|p| p == first), "flat ground has one colour at size {size}");
        }
        // a hill in the north-east corner: the pixels of the top right differ from the bottom left, and the top is the north
        let n_side = 1024 / 2 + 1;
        let hill = terrain(1024, |cx, cz| if cx > n_side * 3 / 4 && cz > n_side * 3 / 4 { 60.0 } else { 0.0 });
        let tga = minimap_tga(&hill).unwrap();
        let top_right = pixel(&tga, 480, 20);
        let bottom_left = pixel(&tga, 20, 490);
        let bottom_right = pixel(&tga, 480, 490);
        assert!(top_right[0] > bottom_left[0] + 30, "high ground is lighter: {top_right:?} {bottom_left:?}");
        assert_eq!(bottom_right, bottom_left, "the hill is in the north only");
        // the editor's own decoder (the one that draws the client's minimaps over the 2D view, top = highest z) agrees
        let img = shaiya_clientdata::texture::decode_tga(&tga, "minimap").unwrap();
        assert_eq!((img.width, img.height), (512, 512));
        let at = |col: usize, row: usize| img.rgba[(row * 512 + col) * 4];
        assert!(at(480, 20) > at(20, 490) + 30 && at(480, 490) == at(20, 490), "the hill is at the top right of the decoded picture");
        // water
        let wet = minimap_tga(&terrain(1024, |_, _| -30.0)).unwrap();
        let p = pixel(&wet, 100, 100);
        assert!(p[2] > p[0] + 50, "blue: {p:?}");
        // a bad grid is refused
        let mut bad = terrain(1024, |_, _| 0.0);
        bad.heights.pop();
        assert!(minimap_tga(&bad).is_err());
    }

    use crate::testutil::{paired_folders, TempDir};

    /// A server data folder for the wizard tests: `Map.ini` with MapCount 11 and the zones 9 and 10 (cp949 bytes in a comment).
    fn server_folder(root: &TempDir) -> PathBuf {
        let dir = root.join("server");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Map.ini"), b"[BASE]\r\nMapCount\t  = 11\r\n\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\n\r\n[SET_ZONE_9]\r\nMapType = F\t// \xB0\xA1\r\n\r\n[SET_ZONE_10]\r\nMapType = F\r\n\r\n").unwrap();
        dir
    }

    /// Every file under a folder with its bytes, to prove that nothing was written there.
    fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push((p.clone(), std::fs::read(&p).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    /// The wizard writes the staged client files, the notes, the server map and the zone, in that order, and nothing else: the
    /// client folder stays as it was, and a second run for the same id refuses before it writes anything.
    #[test]
    fn new_map_wizard_files() {
        let root = TempDir::new("c5-wizard");
        let (client, maps) = paired_folders(&root);
        let server = server_folder(&root);
        let out = root.join("out");
        let rules = ClientRules::open(&client).unwrap();
        let before_client = snapshot(&client);
        let before_ini = std::fs::read(server.join("Map.ini")).unwrap();
        let known = Known::from_parts(Some(&crate::mapini::MapIni::parse(&before_ini)), None, [9, 10], rules.source.map_ids());
        assert_eq!(next_original_id(&known), Some(11));
        assert!(check_new_id(TargetServer::Original, 11, &known).ok());
        let dirs = Dirs { out: &out, client: &client, map_dir: &maps, server_dir: &server, client_backups: Some((root.join("bk-client"), 5)), server_backups: Some((root.join("bk-server"), 5)) };

        // a copy of map 9, for the original server
        let w = Wanted { id: 11, source: Source::Copy(9), minimap: true, name: "My Map".to_string(), target: TargetServer::Original };
        let built = build(&rules, &w, false).unwrap();
        assert_eq!(built.svmap.size, 64);
        let report = execute(&built, &dirs).unwrap_or_else(|(r, e)| panic!("{e} after {r:?}"));
        let names: Vec<&str> = report.written.iter().map(|(w, _)| w.as_str()).collect();
        // the source has no Field files, so the copy gets a generated light map (the client stops without one)
        assert_eq!(names.len(), 7, "{names:?}");
        assert!(names[0].contains("world/11.wld") && names[1].contains("11.txt") && names[2].contains("MiniMap/11.tga") && names[3].contains("Field/11/11_00_L.dds") && names[4].contains("notes") && names[5].contains("11.svmap") && names[6].contains("Map.ini"), "the zone is written last: {names:?}");
        // the staged client map is the source's model, byte for byte
        let wld = std::fs::read(out.join("data").join("world").join("11.wld")).unwrap();
        let src = std::fs::read(client.join("world").join("9.wld")).unwrap();
        assert_eq!(wld, src, "a copy writes the source's bytes");
        assert_eq!(std::fs::read(out.join("data").join("world").join("11.txt")).unwrap(), std::fs::read(client.join("world").join("9.txt")).unwrap());
        let tga = std::fs::read(out.join("data").join("interface").join("MiniMap").join("11.tga")).unwrap();
        assert_eq!(tga.len(), 18 + 512 * 512 * 3 + 26);
        // the server map: size and the four client sections copied
        let m = shaiya_svmap::read(&std::fs::read(maps.join("11.svmap")).unwrap()).unwrap();
        assert_eq!((m.size, m.portals.len(), m.npcs.len(), m.regions.len(), m.binds.len()), (64, 3, 2, 2, 1));
        assert!(m.mob_areas.is_empty(), "monster areas stay empty");
        // Map.ini: MapCount 12, the zone at the end, every other line as it was, and a .bak with the original
        let ini = std::fs::read(server.join("Map.ini")).unwrap();
        let text = String::from_utf8_lossy(&ini).into_owned();
        assert!(text.contains("MapCount\t  = 12\r\n") && text.ends_with("[SET_ZONE_11]\r\nMapType\t = F\r\n"), "{text}");
        assert!(ini.windows(4).any(|w| w == b"// \xB0"), "the cp949 byte of the comment stays");
        assert_eq!(std::fs::read(server.join("Map.ini.bak")).unwrap(), before_ini);
        assert!(std::fs::read_dir(root.join("bk-server")).unwrap().count() == 1, "a timestamped copy of Map.ini");
        // the notes say what is not done and carry the name the user typed
        let notes = std::fs::read_to_string(out.join("svmap-editor-new-map-11.txt")).unwrap();
        for want in ["Not done:", "Map 11", "Loading picture 11.bmp", "Name you chose: My Map", "the choice is yours"] {
            assert!(notes.contains(want), "{want}: {notes}");
        }
        assert!(!out.join("data").join("svmap-editor-new-map-11.txt").exists(), "the notes are not in the folder a patch is built from");
        // the client folder is exactly as it was, and the staging folder holds nothing but the five files
        assert_eq!(snapshot(&client), before_client, "the client folder is untouched");
        assert_eq!(snapshot(&out).len(), 5, "wld, txt, tga, the light map and the notes");
        assert!(!client.join("data").exists());

        // the same id again: refused before anything is written
        let staged_before = snapshot(&out);
        let ini_before = std::fs::read(server.join("Map.ini")).unwrap();
        let (r, e) = match execute(&built, &dirs) {
            Ok(_) => panic!("a second run must refuse"),
            Err(x) => x,
        };
        assert!(r.written.is_empty() && e.contains("already exists"), "{e}");
        assert_eq!(snapshot(&out), staged_before);
        assert_eq!(std::fs::read(server.join("Map.ini")).unwrap(), ini_before);

        // a blank field for Afterglow: flat ground, nothing placed, textures and sky from the look map
        let known = Known::from_parts(Some(&crate::mapini::MapIni::parse(&ini)), None, [9, 10, 11], rules.source.map_ids());
        assert!(check_new_id(TargetServer::Afterglow, 150, &known).ok());
        let w = Wanted { id: 150, source: Source::Blank { size: 1024, look: 9 }, minimap: true, name: String::new(), target: TargetServer::Afterglow };
        let built = build(&rules, &w, false).unwrap();
        let t = built.wld.terrain.as_ref().unwrap();
        assert_eq!((t.size, t.heights.len()), (1024, 513 * 513));
        assert!(t.heights.iter().all(|h| *h == FLAT_RAW) && t.tex_index.iter().all(|i| *i == 0));
        assert!(built.wld.portals.is_empty() && built.wld.npcs.is_empty() && built.wld.areas.is_empty() && built.wld.zone_groups.is_empty() && built.wld.building.items.is_empty());
        assert_eq!(built.wld.sky.len(), 3, "the sky of the look map");
        assert_eq!(built.txt, vec![0xFF, 0xFE], "no area names");
        assert!(built.wld.validate().is_empty());
        let mm = built.minimap.as_ref().unwrap();
        assert!(mm[18..18 + 512 * 512 * 3].chunks(3).all(|p| p == &mm[18..21]), "flat ground is one colour");
        execute(&built, &dirs).unwrap_or_else(|(r, e)| panic!("{e} after {r:?}"));
        let m = shaiya_svmap::read(&std::fs::read(maps.join("150.svmap")).unwrap()).unwrap();
        assert_eq!(m.size, 1024);
        let ini = crate::mapini::MapIni::parse(&std::fs::read(server.join("Map.ini")).unwrap());
        assert_eq!(ini.map_count(), Some(151), "a gap: MapCount follows the highest id");
        assert_eq!(ini.zone_value(150, "MapType").as_deref(), Some("F"));
        // the blank wld parses back with the client's own reader
        let bytes = std::fs::read(out.join("data").join("world").join("150.wld")).unwrap();
        assert!(shaiya_clientdata::wld::Wld::parse(&bytes, "150.wld").is_ok());
        assert_eq!(WldRaw::parse(&bytes, "150.wld").unwrap().to_bytes(), bytes);
        assert_eq!(snapshot(&client), before_client, "the client folder is still untouched");

        // refusals while building: a size that is not offered, a dungeon as the look map (none here), a client map that is read-only
        let w = Wanted { id: 151, source: Source::Blank { size: 1500, look: 9 }, minimap: false, name: String::new(), target: TargetServer::Afterglow };
        assert!(build(&rules, &w, false).is_err());
        let w = Wanted { id: 151, source: Source::Copy(10), minimap: false, name: String::new(), target: TargetServer::Afterglow };
        assert!(build(&rules, &w, false).err().unwrap().contains("read-only"), "a map that does not write back is not copied");
        let w = Wanted { id: 151, source: Source::Copy(77), minimap: false, name: String::new(), target: TargetServer::Afterglow };
        assert!(build(&rules, &w, false).is_err(), "no such client map");
    }

    /// The staging folder inside the client is refused before any write, and so is a server folder that holds no valid Map.ini.
    #[test]
    fn wizard_refuses_unsafe_folders() {
        let root = TempDir::new("c5-refuse");
        let (client, maps) = paired_folders(&root);
        let server = server_folder(&root);
        let rules = ClientRules::open(&client).unwrap();
        let w = Wanted { id: 11, source: Source::Copy(9), minimap: false, name: String::new(), target: TargetServer::Original };
        let built = build(&rules, &w, true).unwrap();
        let inside = client.join("staging");
        let dirs = Dirs { out: &inside, client: &client, map_dir: &maps, server_dir: &server, client_backups: None, server_backups: None };
        let (r, e) = execute(&built, &dirs).err().unwrap();
        assert!(r.written.is_empty() && e.contains("never writes into the client"), "{e}");
        assert!(!inside.exists() && !maps.join("11.svmap").exists());
        let empty = root.join("empty-server");
        std::fs::create_dir_all(&empty).unwrap();
        let out = root.join("out");
        let dirs = Dirs { out: &out, client: &client, map_dir: &maps, server_dir: &empty, client_backups: None, server_backups: None };
        let (r, _) = execute(&built, &dirs).err().unwrap();
        assert!(r.written.is_empty() && !out.exists() && !maps.join("11.svmap").exists(), "no Map.ini, nothing written");

        // Review: Map.ini changed since the window read it (MapCount now 12): the original server's id 11 is refused
        let ini_path = server.join("Map.ini");
        let ini0 = std::fs::read(&ini_path).unwrap();
        std::fs::write(&ini_path, String::from_utf8_lossy(&ini0).replace("= 11", "= 12").as_bytes()).unwrap();
        let dirs = Dirs { out: &out, client: &client, map_dir: &maps, server_dir: &server, client_backups: None, server_backups: None };
        let (r, e) = execute(&built, &dirs).err().unwrap();
        assert!(r.written.is_empty() && e.contains("MapCount 12") && !out.exists(), "{e}");
        std::fs::write(&ini_path, &ini0).unwrap();

        // a read-only Map.ini: the map files are written, the zone is not (the server never lists a map without its file)
        let mut p = std::fs::metadata(&ini_path).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(&ini_path, p.clone()).unwrap();
        let (r, e) = execute(&built, &dirs).err().unwrap();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        std::fs::set_permissions(&ini_path, p).unwrap();
        assert!(e.contains("read-only") && r.written.iter().any(|(w, _)| w.contains("11.svmap")), "{e}");
        assert_eq!(std::fs::read(&ini_path).unwrap(), ini0, "no zone without its map file");
        assert!(!server.join("Map.ini.tmp").exists() && !maps.join("11.svmap.tmp").exists(), "no stray .tmp");
    }

    #[test]
    fn cannot_do_list_names_the_gaps() {
        let l = cannot_do(70, true, false, TargetServer::Original);
        assert!(l[0].contains("already has a name"));
        let l = cannot_do(71, false, true, TargetServer::Afterglow);
        let all = l.join("\n");
        for want in ["Map 71", "Loading picture 71.bmp", "Music zones", "Rebirth", "Generate blocked", "import.py"] {
            assert!(all.contains(want), "{want}: {all}");
        }
    }

    fn dds_ok(b: &[u8], side: u32, mips: u32, fourcc: &[u8; 4]) -> bool {
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        b.len() > 128 && &b[..4] == b"DDS " && u(4) == 124 && u(12) == side && u(16) == side && u(28) == mips && &b[84..88] == fourcc
    }

    /// A blank field writes the chunk textures the client needs (a missing `_L.dds` stops the map load): one light map and one
    /// mask per texture slot for every chunk, with the sizes and headers of the originals; 2048 makes 2 x 2 chunks; a copy takes
    /// its source's files renamed; the files are staged, never in the client folder.
    #[test]
    fn wizard_writes_the_terrain_textures() {
        let root = TempDir::new("fix1-terrain");
        let (client, maps) = paired_folders(&root);
        let server = server_folder(&root);
        let out = root.join("out");
        let rules = ClientRules::open(&client).unwrap();
        let slots = rules.wld(9).unwrap().textures.len();
        assert!(slots > 0);
        let before_client = snapshot(&client);
        let dirs = Dirs { out: &out, client: &client, map_dir: &maps, server_dir: &server, client_backups: None, server_backups: None };
        let w = Wanted { id: 150, source: Source::Blank { size: 1024, look: 9 }, minimap: false, name: String::new(), target: TargetServer::Afterglow };
        let built = build(&rules, &w, false).unwrap();
        let names: Vec<&str> = built.terrain_files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names.len(), 1 + slots.min(8), "{names:?}");
        assert!(names.contains(&"world/Field/150/150_00_L.dds") && names.contains(&"world/Field/150/150_00_A0.dds"));
        for (n, b) in &built.terrain_files {
            if n.ends_with("_L.dds") {
                assert!(dds_ok(b, 1024, 11, b"DXT1") && b.len() == 699_192, "{n}");
            } else {
                assert!(dds_ok(b, 256, 9, b"DXT5") && b.len() == 87_536, "{n}");
            }
        }
        let rep = execute(&built, &dirs).unwrap_or_else(|(r, e)| panic!("{e} after {r:?}"));
        assert!(rep.written.iter().any(|(w, _)| w.contains("terrain texture") && w.contains("150_00_L.dds")), "{:?}", rep.written);
        let staged = out.join("data").join("world").join("Field").join("150").join("150_00_L.dds");
        assert_eq!(std::fs::read(staged).unwrap().len(), 699_192);
        assert_eq!(snapshot(&client), before_client, "nothing in the client folder");
        // the lists name the new files
        assert!(cannot_do(150, false, false, TargetServer::Afterglow).join("\n").contains("Terrain textures"));
        assert!(cannot_do(150, false, true, TargetServer::Afterglow).join("\n").contains("no world/Field"));

        // 2048: 2 x 2 chunks, names 00 01 10 11
        let w = Wanted { id: 151, source: Source::Blank { size: 2048, look: 9 }, minimap: false, name: String::new(), target: TargetServer::Afterglow };
        let built = build(&rules, &w, false).unwrap();
        for chunk in ["00", "01", "10", "11"] {
            assert!(built.terrain_files.iter().any(|(n, _)| *n == format!("world/Field/151/151_{chunk}_L.dds")), "{chunk}");
        }
        assert_eq!(built.terrain_files.len(), 4 * (1 + slots.min(8)));

        // a copy renames the source's files
        let src = client.join("world").join("Field").join("9");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("9_00_L.dds"), b"light").unwrap();
        std::fs::write(src.join("9_00_A0.dds"), b"mask0").unwrap();
        let rules = ClientRules::open(&client).unwrap();
        let w = Wanted { id: 152, source: Source::Copy(9), minimap: false, name: String::new(), target: TargetServer::Afterglow };
        let built = build(&rules, &w, false).unwrap();
        assert_eq!(built.terrain_files, vec![("world/Field/152/152_00_L.dds".to_string(), b"light".to_vec()), ("world/Field/152/152_00_A0.dds".to_string(), b"mask0".to_vec())]);

        // review: a source without the light map still gives the copy one (a missing _L.dds stops the client's map load)
        std::fs::remove_file(src.join("9_00_L.dds")).unwrap();
        let rules = ClientRules::open(&client).unwrap();
        let built = build(&rules, &w, false).unwrap();
        let names: Vec<&str> = built.terrain_files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["world/Field/152/152_00_A0.dds", "world/Field/152/152_00_L.dds"]);
        assert!(dds_ok(&built.terrain_files[1].1, 1024, 11, b"DXT1"));
    }

    /// Review: terrain edge cases do not break the generator: texture indices past the slots, more than 8 slots (capped),
    /// extreme heights, and a field smaller than one chunk.
    #[test]
    fn terrain_textures_edge_cases() {
        let root = TempDir::new("fix1-edges");
        let (client, _) = paired_folders(&root);
        let rules = ClientRules::open(&client).unwrap();
        let w = Wanted { id: 160, source: Source::Blank { size: 1024, look: 9 }, minimap: false, name: String::new(), target: TargetServer::Afterglow };
        let mut t = build(&rules, &w, false).unwrap().wld.terrain.unwrap();
        for (i, v) in t.tex_index.iter_mut().enumerate() {
            *v = if i % 3 == 0 { 255 } else { (i % 11) as u8 };
        }
        for (i, h) in t.heights.iter_mut().enumerate() {
            *h = if i % 2 == 0 { i16::MAX } else { i16::MIN };
        }
        let files = terrain_textures(160, &t, 12);
        assert_eq!(files.len(), 1 + 8, "slots are capped at 8");
        assert!(files.iter().all(|(n, b)| if n.ends_with("_L.dds") { dds_ok(b, 1024, 11, b"DXT1") } else { dds_ok(b, 256, 9, b"DXT5") }));
        t.size = 512;
        assert_eq!(terrain_textures(160, &t, 0).len(), 1, "one chunk, light map only");
    }
}
