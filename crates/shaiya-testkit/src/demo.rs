//! A synthetic client folder for screenshots and for the tests of the screenshot scenes. Every byte is made up here: neutral names, a
//! gradient texture, a box model, a hill-shaped map, a three-bone arm animation with its mesh and a checker texture, and a crate model; the list files that pair them. It holds no game data and no game art.

use crate::{bmp_stub, tga_stub, SahBuilder};
use std::path::Path;

/// The loose file that marks a folder as a demo client. The screenshot scenes of Shaiya Toolkit refuse any folder without it, because
/// one of them (apply-history) makes a real change to the folder it is given.
pub const DEMO_MARKER: &str = "shaiya-demo-client.txt";

/// What `demo_client` puts into the archive, in the order they are stored.
pub const DEMO_ARCHIVE_FILES: &[&str] = &[
    "Interface/demo.tga",
    "Interface/demo-small.bmp",
    "Item/Item.SData",
    "Entity/Building/demo-box.smod",
    "Entity/Texture/demo.tga",
    "world/7.wld",
    "Demo/readme.txt",
    "Demo/notes.txt",
    "Monster/demo.MON",
    "Monster/ANI/demo_wave.ANI",
    "Monster/3DC/demo_arm.3DC",
    "Monster/DDS/demo_arm.dds",
    "Item/demo.ITM",
    "Item/3DO/demo_crate.3DO",
    "Item/DDS/demo_crate.dds",
];

/// The pack time (Unix seconds, 2010-01-01 00:00 UTC) of every file of the demo archive.
pub const DEMO_PACKED_AT: i32 = 1_262_304_000;

/// The map side of the demo field (the terrain has `DEMO_MAP_SIZE / 2 + 1` corners per side).
pub const DEMO_MAP_SIZE: u32 = 32;

/// Zero bytes after the terrain of the demo field map: the counts and names of everything else, all empty.
const WLD_ZERO_TAIL: usize = 1456;

fn u32le(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}

fn lenstr(v: &mut Vec<u8>, s: &str) {
    u32le(v, s.len() as u32);
    v.extend_from_slice(s.as_bytes());
}

/// An Item table that fits the 81-byte layout (`item-81`): two banks of made-up items. The file is stored plain (not SEED-wrapped).
pub fn demo_item_sdata() -> Vec<u8> {
    // (name, description, type, type id, required level, buy, sell, stack, hp)
    type Row = (&'static str, &'static str, u8, u8, u16, u32, u32, u8, u16);
    let banks: [&[Row]; 2] = [
        &[
            ("Demo Training Sword", "A made-up sword for screenshots.", 1, 1, 1, 120, 30, 1, 0),
            ("Demo Wooden Shield", "A made-up shield for screenshots.", 1, 2, 3, 200, 50, 1, 0),
            ("Demo Hunting Bow", "A made-up bow for screenshots.", 1, 3, 5, 340, 85, 1, 0),
            ("Demo Iron Helm", "A made-up helmet for screenshots.", 2, 1, 8, 520, 130, 1, 20),
            ("Demo Cloth Robe", "A made-up robe for screenshots.", 2, 2, 8, 480, 120, 1, 15),
        ],
        &[
            ("Demo Healing Draught", "Restores a little made-up health.", 3, 1, 1, 15, 3, 100, 50),
            ("Demo Mana Draught", "Restores a little made-up mana.", 3, 2, 1, 18, 4, 100, 0),
            ("Demo Silver Ring", "A made-up ring for screenshots.", 4, 1, 12, 900, 225, 1, 30),
            ("Demo Travel Charm", "A made-up charm for screenshots.", 4, 2, 15, 1250, 310, 1, 45),
        ],
    ];
    let mut v = Vec::new();
    u32le(&mut v, banks.len() as u32);
    for bank in banks {
        u32le(&mut v, bank.len() as u32);
        for (name, desc, kind, id, level, buy, sell, stack, hp) in bank.iter().copied() {
            lenstr(&mut v, name);
            lenstr(&mut v, desc);
            let mut b = [0u8; 81];
            b[0] = kind;
            b[1] = id;
            b[4..6].copy_from_slice(&level.to_le_bytes());
            b[47..49].copy_from_slice(&hp.to_le_bytes());
            b[67..71].copy_from_slice(&buy.to_le_bytes());
            b[71..75].copy_from_slice(&sell.to_le_bytes());
            b[80] = stack;
            v.extend_from_slice(&b);
        }
    }
    v
}

/// A field map (`FLD`): a smooth hill on a 17 x 17 grid of corners, every list empty.
pub fn demo_wld() -> Vec<u8> {
    let n = (DEMO_MAP_SIZE / 2 + 1) as usize;
    let mut v = b"FLD\0".to_vec();
    v.extend((DEMO_MAP_SIZE as i32).to_le_bytes());
    let mid = (n as f64 - 1.0) / 2.0;
    for z in 0..n {
        for x in 0..n {
            let (dx, dz) = (x as f64 - mid, z as f64 - mid);
            let hill = 700.0 * (-(dx * dx + dz * dz) / 14.0).exp() + 120.0 * (x as f64 / 3.0).sin();
            v.extend(((9000.0 + hill).round() as i16).to_le_bytes());
        }
    }
    v.extend(std::iter::repeat_n(0u8, n * n));
    v.extend(std::iter::repeat_n(0u8, WLD_ZERO_TAIL));
    v
}

/// A model of one box (6 faces, 24 vertices, 12 triangles) with one texture, `demo.tga`, that the demo client stores under `Entity/Texture`.
pub fn demo_box_smod() -> Vec<u8> {
    let mut d = Vec::new();
    let f = |d: &mut Vec<u8>, v: f32| d.extend_from_slice(&v.to_le_bytes());
    let i = |d: &mut Vec<u8>, v: i32| d.extend_from_slice(&v.to_le_bytes());
    for v in [2.0f32, 0.0, 2.0] {
        f(&mut d, v);
    }
    i(&mut d, 0);
    for v in [0.0f32, 0.0, 0.0, 4.0, 4.0, 4.0] {
        f(&mut d, v);
    }
    i(&mut d, 1);
    let name = "demo.tga\0";
    i(&mut d, name.len() as i32);
    d.extend_from_slice(name.as_bytes());
    // faces: normal, then four corners (counter-clockwise seen from outside)
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 1.0, 0.0], [[0.0, 4.0, 0.0], [4.0, 4.0, 0.0], [4.0, 4.0, 4.0], [0.0, 4.0, 4.0]]),
        ([0.0, -1.0, 0.0], [[0.0, 0.0, 4.0], [4.0, 0.0, 4.0], [4.0, 0.0, 0.0], [0.0, 0.0, 0.0]]),
        ([0.0, 0.0, 1.0], [[0.0, 0.0, 4.0], [0.0, 4.0, 4.0], [4.0, 4.0, 4.0], [4.0, 0.0, 4.0]]),
        ([0.0, 0.0, -1.0], [[4.0, 0.0, 0.0], [4.0, 4.0, 0.0], [0.0, 4.0, 0.0], [0.0, 0.0, 0.0]]),
        ([1.0, 0.0, 0.0], [[4.0, 0.0, 4.0], [4.0, 4.0, 4.0], [4.0, 4.0, 0.0], [4.0, 0.0, 0.0]]),
        ([-1.0, 0.0, 0.0], [[0.0, 0.0, 0.0], [0.0, 4.0, 0.0], [0.0, 4.0, 4.0], [0.0, 0.0, 4.0]]),
    ];
    let uvs = [[0.0f32, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]];
    i(&mut d, 24);
    for (normal, corners) in &faces {
        for (p, uv) in corners.iter().zip(uvs) {
            for v in p.iter().chain(normal) {
                f(&mut d, *v);
            }
            d.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            f(&mut d, uv[0]);
            f(&mut d, uv[1]);
        }
    }
    i(&mut d, 12);
    for face in 0..6u16 {
        for k in [0u16, 1, 2, 0, 2, 3] {
            d.extend_from_slice(&(face * 4 + k).to_le_bytes());
        }
    }
    for v in [0.0f32, 0.0, 0.0, 4.0, 4.0, 4.0] {
        f(&mut d, v);
    }
    i(&mut d, 0);
    d
}

/// The texture of the demo arm: a checker of 8-pixel squares, orange and teal (uncompressed; generated here, not art).
pub fn demo_arm_texture() -> Vec<u8> {
    crate::tex::checker(64, 8, [230, 150, 40], [30, 140, 160])
}

/// A monster list file with one made-up record whose one part is the demo arm and its texture.
pub fn demo_mon() -> Vec<u8> {
    crate::lists::mon_bytes(&[crate::lists::MonRec { name: "Demo Arm", flag: 0, slots: [""; 17], parts: &[("demo_arm.3DC", "demo_arm.dds")], size: 1.0, attachments: &[] }])
}

/// An item list file with one made-up record: the demo crate model and its texture.
pub fn demo_itm() -> Vec<u8> {
    crate::lists::itm_bytes(&["demo_crate.3DO"], &["demo_crate.dds"], &[crate::lists::ItmRec { words: [0; 6], extra: None }])
}

/// Writes a synthetic client into `dir` (which must exist; it is not emptied):
///
/// * `data.sah` and `data.saf` with [`DEMO_ARCHIVE_FILES`] and some unused space at the end of `data.saf`,
/// * `Version.ini` (CheckVersion 3, CurrentVersion 3),
/// * [`DEMO_MARKER`].
///
/// Returns the archive paths.
pub fn demo_client(dir: &Path) -> Vec<String> {
    let readme: &[u8] = b"Demo client\r\nA folder made up for screenshots. It holds no game files.\r\n";
    let notes: &[u8] = b"Notes\r\n- made-up item names\r\n- a box model\r\n- a hill-shaped map\r\n";
    let files: Vec<(&str, Vec<u8>)> = vec![
        ("Interface/demo.tga", tga_stub(96, 64)),
        ("Interface/demo-small.bmp", bmp_stub(48, 32)),
        ("Item/Item.SData", demo_item_sdata()),
        ("Entity/Building/demo-box.smod", demo_box_smod()),
        ("Entity/Texture/demo.tga", tga_stub(64, 64)),
        ("world/7.wld", demo_wld()),
        ("Demo/readme.txt", readme.to_vec()),
        ("Demo/notes.txt", notes.to_vec()),
        ("Monster/demo.MON", demo_mon()),
        ("Monster/ANI/demo_wave.ANI", crate::anim::arm_ani(60)),
        ("Monster/3DC/demo_arm.3DC", crate::anim::arm_mesh_3dc()),
        ("Monster/DDS/demo_arm.dds", demo_arm_texture()),
        ("Item/demo.ITM", demo_itm()),
        ("Item/3DO/demo_crate.3DO", crate::anim::box_3do()),
        ("Item/DDS/demo_crate.dds", crate::tex::dds_dxt1_solid(8, 8, 0xFD20)),
    ];
    debug_assert_eq!(files.iter().map(|(p, _)| *p).collect::<Vec<_>>(), DEMO_ARCHIVE_FILES);
    let mut b = SahBuilder::new(0);
    for (path, data) in &files {
        b = b.file_versioned(path.as_bytes(), data, DEMO_PACKED_AT);
    }
    let (_, saf) = b.write_to(dir);
    // some space nobody uses: what an earlier change leaves behind (Client health shows it as dead space)
    let mut grown = std::fs::read(&saf).expect("read data.saf");
    grown.extend(std::iter::repeat_n(0u8, 48 * 1024));
    std::fs::write(&saf, grown).expect("write data.saf");
    std::fs::write(dir.join("Version.ini"), b"[Version]\r\nCheckVersion=3\r\nCurrentVersion=3\r\n").expect("write Version.ini");
    std::fs::create_dir_all(dir.join("Demo")).expect("create Demo");
    std::fs::write(dir.join("Demo/readme.txt"), b"Demo client\r\nThis loose file replaces the one in the archive.\r\n").expect("write readme");
    std::fs::write(dir.join("Demo/new-file.txt"), b"A new made-up file for a patch.\r\n").expect("write new-file");
    std::fs::write(dir.join(DEMO_MARKER), b"This folder was made by cargo xtask demo-client. It is a synthetic client for screenshots; it holds no game files.\r\n").expect("write marker");
    DEMO_ARCHIVE_FILES.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_client_has_an_index_a_data_file_and_the_loose_files() {
        let t = crate::TempDir::new("demo-client");
        let listed = demo_client(t.path());
        assert_eq!(listed.len(), DEMO_ARCHIVE_FILES.len());
        let sah = std::fs::read(t.path().join("data.sah")).unwrap();
        assert_eq!(&sah[..3], b"SAH");
        assert!(std::fs::metadata(t.path().join("data.saf")).unwrap().len() > 48 * 1024);
        for f in ["Version.ini", "Demo/readme.txt", "Demo/new-file.txt", DEMO_MARKER] {
            assert!(t.path().join(f).is_file(), "{f}");
        }
    }

    #[test]
    fn the_demo_pieces_have_the_expected_shape() {
        let wld = demo_wld();
        assert_eq!(&wld[..4], b"FLD\0");
        assert_eq!(i32::from_le_bytes(wld[4..8].try_into().unwrap()), DEMO_MAP_SIZE as i32);
        let smod = demo_box_smod();
        assert_eq!(smod.len(), 12 + 4 + 24 + 4 + 4 + 9 + 4 + 24 * 36 + 4 + 12 * 6 + 24 + 4);
        let items = demo_item_sdata();
        assert_eq!(u32::from_le_bytes(items[..4].try_into().unwrap()), 2);
    }

    #[test]
    fn the_demo_archive_has_only_neutral_folders_and_made_up_item_names() {
        for p in DEMO_ARCHIVE_FILES {
            assert!(p.starts_with("Interface/") || p.starts_with("Item/") || p.starts_with("Entity/") || p.starts_with("world/") || p.starts_with("Demo/") || p.starts_with("Monster/"), "{p}");
        }
        let items = String::from_utf8_lossy(&demo_item_sdata()).to_string();
        assert!(items.contains("Demo Training Sword") && items.contains("made-up"));
    }
}
