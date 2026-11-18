//! Helpers for the unit tests: every test gets a temp directory of its own (process id + a counter + the clock), so tests that
//! run in parallel threads never share files, and the directory is removed when the guard drops (also after a failed assert).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct TempDir(PathBuf);

impl TempDir {
    /// A new, empty directory named after the test (`tag`) and unique in this process and across processes.
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let p = std::env::temp_dir().join(format!("svmap-editor-test-{tag}-{}-{n}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).expect("create the test's temp directory");
        TempDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}


/// A small field map for tests (`size` is the terrain side, even): one terrain texture, one portal, one NPC, three sky names.
pub fn tiny_wld(size: u32) -> shaiya_clientdata::wld_raw::WldRaw {
    use shaiya_clientdata::wld::{WldKind, WldNpc};
    use shaiya_clientdata::wld_raw::*;
    let n = size as usize / 2 + 1;
    let name = |s: &str| {
        let mut b = [0u8; 256];
        b[..s.len()].copy_from_slice(s.as_bytes());
        b
    };
    let mut dest = [0u8; 248];
    dest[..4].copy_from_slice(b"Home");
    WldRaw {
        kind: WldKind::Field,
        terrain: Some(RawTerrain { size: size as i32, heights: vec![9000; n * n], tex_index: vec![0; n * n] }),
        textures: vec![RawTexture { name: name("grass.dds"), tiling: 1.0, footstep: name("ch_walkg") }],
        water: name(""),
        dg: [0; 256],
        building: RawPlaced::default(),
        shape: RawPlaced::default(),
        tree: RawPlaced::default(),
        grass: RawPlaced::default(),
        vani: RawPlaced::default(),
        vani2: RawPlaced::default(),
        dungeon: RawPlaced::default(),
        mani: RawMani::default(),
        effect_file: name(""),
        effects: vec![],
        object: RawObjects::default(),
        object2: RawPlaced::default(),
        sound: RawSound::default(),
        boxes: vec![],
        portals: vec![RawPortal { min: [10.0, 1.0, 10.0], max: [14.0, 5.0, 14.0], rot_y: 0.0, name: name("p"), dest_name: dest, country: 1, level_min: 0, level_max: 999, dest_map: 2, dest_pos: [1.0, 2.0, 3.0] }],
        zone_groups: vec![],
        areas: vec![],
        npcs: vec![WldNpc { kind: 1, type_id: 1, pos: [1.0, 1.0, 1.0], angle: 0.0, extra: vec![] }],
        sky: vec![name("sky.bmp"), name("c1.tga"), name("c2.tga")],
        colours: [[255.0; 3]; 3],
        fog_near: 300.0,
        fog_far: 600.0,
    }
}

/// The bytes of a map that reads fine and does not write back the same: one bind group of two boxes whose declared total says 1.
pub fn not_lossless_wld(size: u32) -> Vec<u8> {
    use shaiya_clientdata::wld::{ZoneBox, ZoneGroup};
    let mut m = tiny_wld(size);
    let b = ZoneBox { min: [0.0; 3], max: [1.0; 3], rot_y: 0.0, country: 1, attr: 0 };
    m.zone_groups = vec![ZoneGroup { boxes: vec![b, b] }];
    let mut bytes = m.to_bytes();
    // the total (2) is followed by the group size (2): no other place in this map has two such ints in a row
    let pat: Vec<u8> = [2i32.to_le_bytes(), 2i32.to_le_bytes()].concat();
    let pos = bytes.windows(8).rposition(|w| w == pat.as_slice()).expect("total and group size");
    bytes[pos..pos + 4].copy_from_slice(&1i32.to_le_bytes());
    bytes
}

/// UTF-16 text with a byte order mark, as `world/<id>.txt` is.
pub fn utf16_bom(s: &str) -> Vec<u8> {
    let mut v = vec![0xff, 0xfe];
    for u in s.encode_utf16() {
        v.extend_from_slice(&u.to_le_bytes());
    }
    v
}

/// A client map for the paired-document tests: three portals, two NPCs (the second walks a path), two named areas and one zone group; its
/// `world/<id>.txt`; and the svmap that mirrors it record for record (so every section is paired).
pub fn paired_fixture(size: u32) -> (shaiya_clientdata::wld_raw::WldRaw, shaiya_svmap::Svmap, Vec<u8>) {
    use shaiya_clientdata::wld::{WldNpc, ZoneBox, ZoneGroup};
    use shaiya_clientdata::wld_raw::RawArea;
    let mut w = tiny_wld(size);
    let mut p2 = w.portals[0].clone();
    p2.min = [30.0, 2.0, 30.0];
    p2.max = [32.0, 6.0, 32.0];
    p2.dest_map = 3;
    let mut p3 = w.portals[0].clone();
    p3.min = [40.0, 2.0, 10.0];
    p3.max = [42.0, 6.0, 12.0];
    p3.dest_map = 4;
    w.portals.push(p2);
    w.portals.push(p3);
    w.npcs.push(WldNpc { kind: 2, type_id: 5, pos: [20.0, 1.0, 20.0], angle: 1.5, extra: vec![[22.0, 1.0, 20.0], [24.0, 1.0, 20.0]] });
    let area = |min: [f32; 3], max: [f32; 3], mode: i32| RawArea { min, max, rot_y: 0.0, name: [0; 256], tga: [0; 256], mode, extra: 0 };
    w.areas = vec![area([2.0, 0.0, 2.0], [8.0, 4.0, 8.0], 0), area([12.0, 0.0, 12.0], [18.0, 4.0, 18.0], 1)];
    w.zone_groups = vec![ZoneGroup { boxes: vec![ZoneBox { min: [50.0, 0.0, 50.0], max: [56.0, 2.0, 56.0], rot_y: 0.0, country: 1, attr: 0 }] }];
    let t = crate::sync::target_from_wld(&w.to_wld(shaiya_clientdata::wld_raw::Codepage::Cp949));
    let mut m = shaiya_svmap::Svmap::new(size);
    m.npcs = t.npcs;
    m.portals = t.portals;
    m.regions = t.regions;
    m.binds = t.binds;
    (w, m, utf16_bom("0\tAlpha\r\n1\tBeta\r\n"))
}

/// A tab (map 9) over [`paired_fixture`]: the svmap and the client half in memory, every section paired.
pub fn paired_doc() -> crate::doc::Doc {
    use crate::client_files::ClientWld;
    use crate::doc::{ClientSlot, Doc};
    use crate::pair::ClientPart;
    let (w, m, txt) = paired_fixture(64);
    let mut d = Doc::new(1, Some(9), None, m, None);
    d.set_client(ClientSlot::Part(Box::new(ClientPart::from_model(ClientWld::from_model(9, w), Some(txt)))));
    d
}

/// A client folder (extracted layout) with map 9 = [`paired_fixture`] (`world/9.wld`, `world/9.txt`) and map 10 that does not
/// write back byte for byte, and a map folder with the svmap of map 9 and an empty svmap of map 10. Returns (client, maps).
pub fn paired_folders(root: &TempDir) -> (PathBuf, PathBuf) {
    let client = root.join("client");
    std::fs::create_dir_all(client.join("world")).unwrap();
    let (w, m, txt) = paired_fixture(64);
    std::fs::write(client.join("world").join("9.wld"), w.to_bytes()).unwrap();
    std::fs::write(client.join("world").join("9.txt"), txt).unwrap();
    std::fs::write(client.join("world").join("10.wld"), not_lossless_wld(64)).unwrap();
    let maps = root.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    std::fs::write(maps.join("9.svmap"), shaiya_svmap::write(&m)).unwrap();
    std::fs::write(maps.join("10.svmap"), shaiya_svmap::write(&shaiya_svmap::Svmap::new(64))).unwrap();
    (client, maps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directories_are_unique_and_removed() {
        let (a, b) = (TempDir::new("same"), TempDir::new("same"));
        assert_ne!(a.path(), b.path());
        std::fs::write(a.join("f"), b"x").unwrap();
        let gone = a.path().to_path_buf();
        drop(a);
        assert!(!gone.exists() && b.path().is_dir());
    }
}
