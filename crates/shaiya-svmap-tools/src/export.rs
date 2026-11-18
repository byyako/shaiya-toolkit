//! Export: CSV per section, the folder's monster areas, the map as JSON. Pure text; the app
//! chooses the file. CSV is UTF-8 with a BOM and CRLF line ends, one header row; a text cell that starts with `=`, `+`, `-`, `@`, a tab
//! or a carriage return gets a leading `'` so a spreadsheet does not run it as a formula.

use shaiya_svmap::Svmap;

use crate::names;
use crate::search::Env;

const BOM: &str = "\u{feff}";

/// The sections the CSV export knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Areas,
    Npcs,
    Portals,
    Binds,
    Regions,
}

impl Kind {
    pub const ALL: [Kind; 5] = [Kind::Areas, Kind::Npcs, Kind::Portals, Kind::Binds, Kind::Regions];

    /// The file name part: `<map>-<this>.csv`.
    pub fn file_part(self) -> &'static str {
        match self {
            Kind::Areas => "monster-areas",
            Kind::Npcs => "npcs",
            Kind::Portals => "portals",
            Kind::Binds => "binds",
            Kind::Regions => "regions",
        }
    }
}

/// A text cell: formula-safe, quoted when it holds a comma, a quote or a line break.
pub fn text(s: &str) -> String {
    let mut t = s.to_string();
    if t.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        t.insert(0, '\'');
    }
    quote(&t)
}

fn quote(t: &str) -> String {
    if t.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", t.replace('"', "\"\""))
    } else {
        t.to_string()
    }
}

fn f(v: f32) -> String {
    format!("{v}")
}

fn join(out: &mut String, cells: &[String]) {
    out.push_str(&cells.join(","));
    out.push_str("\r\n");
}

fn header(cols: &str) -> String {
    format!("{BOM}{}\r\n", cols)
}

pub const AREA_COLUMNS: &str = "index,x1,y1,z1,x2,y2,z2,width,depth,mob,mob_name,mob_level,count";
pub const NPC_COLUMNS: &str = "index,type,type_name,type_id,name,points,x,y,z,dir_deg";
pub const PORTAL_COLUMNS: &str = "index,x,y,z,country,level_min,level_max,to_map,to_map_name,to_x,to_y,to_z";
pub const BIND_COLUMNS: &str = "index,country,attr,box,x1,y1,z1,x2,y2,z2";
pub const REGION_COLUMNS: &str = "index,name,kind,b,c,d,x1,y1,z1,x2,y2,z2";

fn area_rows(out: &mut String, m: &Svmap, env: &Env, prefix: &[String]) {
    for (i, a) in m.mob_areas.iter().enumerate() {
        let mut base: Vec<String> = prefix.to_vec();
        base.extend([i.to_string(), f(a.min[0]), f(a.min[1]), f(a.min[2]), f(a.max[0]), f(a.max[1]), f(a.max[2]), f((a.max[0] - a.min[0]).abs()), f((a.max[2] - a.min[2]).abs())]);
        if a.mobs.is_empty() {
            let mut row = base.clone();
            row.extend(["".into(), "".into(), "".into(), "".into()]);
            join(out, &row);
        }
        for e in &a.mobs {
            let mut row = base.clone();
            let known = (env.mob)(e.mob);
            row.push(e.mob.to_string());
            row.push(known.as_ref().map_or(String::new(), |(n, _)| text(n.trim())));
            row.push(known.as_ref().map_or(String::new(), |(_, l)| l.to_string()));
            row.push(e.count.to_string());
            join(out, &row);
        }
    }
}

pub fn areas_csv(m: &Svmap, env: &Env) -> String {
    let mut out = header(AREA_COLUMNS);
    area_rows(&mut out, m, env, &[]);
    out
}

/// Every map's monster areas in one file, with the `map` column first.
pub fn folder_areas_csv(maps: &[(u32, &Svmap)], env: &Env) -> String {
    let mut out = header(&format!("map,{AREA_COLUMNS}"));
    for (id, m) in maps {
        area_rows(&mut out, m, env, &[id.to_string()]);
    }
    out
}

pub fn npcs_csv(m: &Svmap, env: &Env) -> String {
    let mut out = header(NPC_COLUMNS);
    for (i, n) in m.npcs.iter().enumerate() {
        let (x, y, z) = n.points.first().map_or((0.0, 0.0, 0.0), |p| (p.pos[0], p.pos[1], p.pos[2]));
        let dir = n.points.first().map_or(0.0, |p| p.dir.to_degrees());
        let name = (env.npc)(n.kind, n.type_id).unwrap_or_default();
        join(
            &mut out,
            &[i.to_string(), n.kind.to_string(), text(&names::npc_kind_name(n.kind)), n.type_id.to_string(), text(name.trim()), n.points.len().to_string(), f(x), f(y), f(z), format!("{dir:.2}")],
        );
    }
    out
}

pub fn portals_csv(m: &Svmap, env: &Env) -> String {
    let mut out = header(PORTAL_COLUMNS);
    for (i, p) in m.portals.iter().enumerate() {
        join(
            &mut out,
            &[i.to_string(), f(p.pos[0]), f(p.pos[1]), f(p.pos[2]), p.country.to_string(), p.level_min.to_string(), p.level_max.to_string(), p.to_map.to_string(), text(&(env.map_name)(p.to_map)), f(p.to_pos[0]), f(p.to_pos[1]), f(p.to_pos[2])],
        );
    }
    out
}

pub fn binds_csv(m: &Svmap) -> String {
    let mut out = header(BIND_COLUMNS);
    for (i, b) in m.binds.iter().enumerate() {
        if b.boxes.is_empty() {
            join(&mut out, &[i.to_string(), b.country.to_string(), b.attr.to_string(), "".into(), "".into(), "".into(), "".into(), "".into(), "".into(), "".into()]);
        }
        for (k, x) in b.boxes.iter().enumerate() {
            join(&mut out, &[i.to_string(), b.country.to_string(), b.attr.to_string(), k.to_string(), f(x.min[0]), f(x.min[1]), f(x.min[2]), f(x.max[0]), f(x.max[1]), f(x.max[2])]);
        }
    }
    out
}

/// `map_id` finds the regions' names through `env.region`.
pub fn regions_csv(m: &Svmap, map_id: Option<u32>, env: &Env) -> String {
    let mut out = header(REGION_COLUMNS);
    for (i, r) in m.regions.iter().enumerate() {
        let name = map_id.and_then(|id| (env.region)(id, i)).unwrap_or_default();
        join(&mut out, &[i.to_string(), text(&name), r.kind.to_string(), r.b.to_string(), r.c.to_string(), r.d.to_string(), f(r.min[0]), f(r.min[1]), f(r.min[2]), f(r.max[0]), f(r.max[1]), f(r.max[2])]);
    }
    out
}

pub fn section_csv(kind: Kind, m: &Svmap, map_id: Option<u32>, env: &Env) -> String {
    match kind {
        Kind::Areas => areas_csv(m, env),
        Kind::Npcs => npcs_csv(m, env),
        Kind::Portals => portals_csv(m, env),
        Kind::Binds => binds_csv(m),
        Kind::Regions => regions_csv(m, map_id, env),
    }
}

/// The map as JSON, the same document `shaiya svmap dump --json` prints (pretty, blocked cells as base64).
pub fn map_json(m: &Svmap) -> Result<String, String> {
    shaiya_svmap::to_json_pretty(m).map_err(|e| e.to_string())
}

/// Whether an export may write `path`: never a map, client archive or program file (`.svmap`, `.sah`, `.saf`, `.exe`, `.dll`,
/// whatever the dialog's filter said), never one of the `protected` files (the open tabs' and the folder's maps), and never into,
/// around or at one of the `client_dirs` (every client folder the editor reads). Err holds the reason.
pub fn check_target(path: &std::path::Path, protected: &[&std::path::Path], client_dirs: &[std::path::PathBuf]) -> Result<(), String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if matches!(ext.as_str(), "svmap" | "sah" | "saf" | "exe" | "dll") {
        return Err(format!("{}: an export never writes a .{ext} file (pick a .csv, .json or .png name)", path.display()));
    }
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let folder = abs.parent().map(std::path::Path::to_path_buf).unwrap_or_else(|| abs.clone());
    for c in client_dirs {
        if crate::client_files::lies_in(&abs, c) || crate::client_files::lies_in(&folder, c) || crate::client_files::lies_in(c, &folder) {
            return Err(format!("{}: that is in or around the client folder {}. An export never writes the client folder; pick another folder.", path.display(), c.display()));
        }
    }
    let same = |a: &std::path::Path, b: &std::path::Path| match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a.as_os_str().to_string_lossy().eq_ignore_ascii_case(&b.as_os_str().to_string_lossy()),
    };
    if protected.iter().any(|p| same(path, p)) {
        return Err(format!("{}: that is an open map file; an export never replaces it", path.display()));
    }
    Ok(())
}

/// Crops a captured window image (`rgba`, `w` x `h` pixels) to a rectangle given in points (`scale` pixels per point) and returns
/// (width, height, rgba) of the crop.
pub fn crop_rgba(rgba: &[u8], w: usize, h: usize, rect_pts: [f32; 4], scale: f32) -> Option<(usize, usize, Vec<u8>)> {
    let x0 = ((rect_pts[0] * scale).round().max(0.0) as usize).min(w);
    let y0 = ((rect_pts[1] * scale).round().max(0.0) as usize).min(h);
    let x1 = ((rect_pts[2] * scale).round().max(0.0) as usize).min(w);
    let y1 = ((rect_pts[3] * scale).round().max(0.0) as usize).min(h);
    if x1 <= x0 || y1 <= y0 || rgba.len() < w * h * 4 {
        return None;
    }
    let mut out = Vec::with_capacity((x1 - x0) * (y1 - y0) * 4);
    for y in y0..y1 {
        out.extend_from_slice(&rgba[(y * w + x0) * 4..(y * w + x1) * 4]);
    }
    Some((x1 - x0, y1 - y0, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_svmap::{Aabb, Bind, MobArea, MobEntry, NpcEntry, NpcPoint, Portal, Region};

    fn with_env<R>(f: impl FnOnce(&Env) -> R) -> R {
        let mob = |id: u32| match id {
            1 => Some(("=SUM(1)".to_string(), 7)),
            2 => Some(("Wolf, grey".to_string(), 12)),
            _ => None,
        };
        let npc = |k: u32, id: u32| (k == 1).then(|| format!("Merchant {id}"));
        let name = |id: u32| format!("Map {id}");
        let region = |_m: u32, i: usize| Some(format!("+Region {i}"));
        f(&Env { mob: &mob, npc: &npc, map_name: &name, region: &region })
    }

    fn sample() -> Svmap {
        let mut m = Svmap::new(64);
        m.mob_areas.push(MobArea { min: [1.0, 0.0, 2.0], max: [5.5, 2.0, 9.0], mobs: vec![MobEntry { mob: 1, count: 3 }, MobEntry { mob: 2, count: 1 }, MobEntry { mob: 99, count: 2 }] });
        m.mob_areas.push(MobArea { min: [0.0; 3], max: [1.0, 2.0, 1.0], mobs: vec![] });
        m.npcs.push(NpcEntry { kind: 1, type_id: 4, points: vec![NpcPoint { pos: [1.0, 2.0, 3.0], dir: std::f32::consts::PI }] });
        m.portals.push(Portal { pos: [1.0, 2.0, 3.0], country: 1, level_min: 5, level_max: 9, to_map: 2, to_pos: [4.0, 5.0, 6.0] });
        m.binds.push(Bind { country: 2, attr: 0, boxes: vec![Aabb { min: [0.0; 3], max: [2.0; 3] }, Aabb { min: [4.0; 3], max: [6.0; 3] }] });
        m.regions.push(Region { min: [0.0; 3], max: [8.0, 2.0, 8.0], kind: 2, b: 1, c: 2, d: 100 });
        m
    }

    #[test]
    fn csv_export() {
        let m = sample();
        with_env(|env| {
            let csv = areas_csv(&m, env);
            assert!(csv.starts_with('\u{feff}'), "UTF-8 BOM");
            assert!(!csv.replace("\r\n", "").contains('\n') && !csv.replace("\r\n", "").contains('\r'), "CRLF only");
            let lines: Vec<&str> = csv.trim_start_matches('\u{feff}').split("\r\n").collect();
            assert_eq!(lines[0], AREA_COLUMNS);
            // one row per entry, and an empty area is one row with empty monster columns
            assert_eq!(lines.len(), 1 + 3 + 1 + 1, "header, 3 entries, 1 empty area, trailing break");
            assert_eq!(lines[1], "0,1,0,2,5.5,2,9,4.5,7,1,'=SUM(1),7,3", "the formula name is neutralised");
            assert_eq!(lines[2], "0,1,0,2,5.5,2,9,4.5,7,2,\"Wolf, grey\",12,1", "a comma quotes the cell");
            assert_eq!(lines[3], "0,1,0,2,5.5,2,9,4.5,7,99,,,2", "an unknown monster has no name or level");
            assert_eq!(lines[4], "1,0,0,0,1,2,1,1,1,,,,");
            // the other sections
            let n = npcs_csv(&m, env);
            assert!(n.contains("0,1,Merchant,4,Merchant 4,1,1,2,3,180.00"), "{n}");
            let p = portals_csv(&m, env);
            assert!(p.contains("0,1,2,3,1,5,9,2,Map 2,4,5,6"), "{p}");
            let b = binds_csv(&m);
            assert!(b.contains("0,2,0,0,0,0,0,2,2,2") && b.contains("0,2,0,1,4,4,4,6,6,6"), "{b}");
            let r = regions_csv(&m, Some(1), env);
            assert!(r.contains("0,'+Region 0,2,1,2,100,0,0,0,8,2,8"), "{r}");
            // the folder file starts with the map column
            let folder = folder_areas_csv(&[(7, &m), (8, &m)], env);
            assert!(folder.starts_with(&format!("\u{feff}map,{AREA_COLUMNS}\r\n7,0,1,0,2")), "{folder}");
            assert_eq!(folder.matches("\r\n").count(), 1 + 2 * 4);
        });
    }

    #[test]
    fn text_cells_are_formula_safe_and_numbers_are_not_touched() {
        assert_eq!(text("=1+1"), "'=1+1");
        assert_eq!(text("+x"), "'+x");
        assert_eq!(text("-x"), "'-x");
        assert_eq!(text("@x"), "'@x");
        assert_eq!(text("\tx"), "'\tx");
        assert_eq!(text("plain"), "plain");
        assert_eq!(text("a\"b"), "\"a\"\"b\"");
        let m = {
            let mut m = Svmap::new(8);
            m.points_b.push([0.0; 3]);
            m.regions.push(Region { min: [-1.5, 0.0, -2.0], max: [1.0; 3], kind: 0, b: 0, c: 0, d: 0 });
            m
        };
        with_env(|env| assert!(regions_csv(&m, None, env).contains(",-1.5,0,-2,"), "negative numbers stay numbers"));
    }

    #[test]
    fn exports_never_target_map_or_client_files() {
        let dir = std::env::temp_dir().join(format!("svmap-export-target-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let map = dir.join("1.svmap");
        std::fs::write(&map, b"x").unwrap();
        let odd = dir.join("notes.txt");
        std::fs::write(&odd, b"x").unwrap();
        for bad in ["1.svmap", "2.SVMAP", "data.sah", "data.SAF", "game.exe", "Game.EXE", "d3d9.dll", "X.Dll"] {
            assert!(check_target(&dir.join(bad), &[], &[]).is_err(), "{bad}");
        }
        // an open file under any extension is refused, also when spelled with another case
        assert!(check_target(&odd, &[odd.as_path()], &[]).is_err());
        assert!(check_target(&dir.join("NOTES.TXT"), &[odd.as_path()], &[]).is_err());
        assert!(check_target(&dir.join("1-monster-areas.csv"), &[map.as_path(), odd.as_path()], &[]).is_ok());
        assert!(check_target(&dir.join("1.json"), &[map.as_path()], &[]).is_ok());
        // a client folder: a file in it, in a folder inside it, a folder around it, or the folder itself is refused
        let client = dir.join("client");
        std::fs::create_dir_all(client.join("sub")).unwrap();
        let other = dir.join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        let clients = [client.clone()];
        for bad in [client.join("a.csv"), client.join("sub").join("a.json"), client.join("sub").join("not-yet").join("a.png"), dir.join("a.csv"), client.clone()] {
            assert!(check_target(&bad, &[], &clients).is_err(), "{}", bad.display());
        }
        assert!(check_target(&other.join("a.csv"), &[], &clients).is_ok());
        #[cfg(windows)]
        {
            // the same folder under another spelling (upper case, forward slashes) is the same folder
            let s = client.to_string_lossy().to_uppercase().replace('\\', "/");
            assert!(check_target(&std::path::PathBuf::from(format!("{s}/a.csv")), &[], &clients).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_is_the_dump_document_and_the_crop_takes_the_viewport() {
        let m = sample();
        let j = map_json(&m).unwrap();
        assert_eq!(shaiya_svmap::from_json(&j).unwrap(), m);
        // a 4x3 image, 1 pixel per point, crop x 1..3, y 1..2
        let img: Vec<u8> = (0..4 * 3).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        let (w, h, px) = crop_rgba(&img, 4, 3, [1.0, 1.0, 3.0, 2.0], 1.0).unwrap();
        assert_eq!((w, h), (2, 1));
        assert_eq!(px, vec![5, 0, 0, 255, 6, 0, 0, 255]);
        assert!(crop_rgba(&img, 4, 3, [3.0, 1.0, 1.0, 2.0], 1.0).is_none());
        // hi-dpi: 2 pixels per point
        let (w2, h2, _) = crop_rgba(&[0u8; 8 * 6 * 4], 8, 6, [1.0, 1.0, 3.0, 2.0], 2.0).unwrap();
        assert_eq!((w2, h2), (4, 2));
    }
}
