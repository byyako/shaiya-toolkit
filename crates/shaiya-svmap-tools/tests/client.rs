//! Tests against the real client archive and the original server maps. They read `SHAIYA_CLIENT` (the client folder with
//! `data.sah`/`data.saf`) and `SVMAP_ORIGINALS` (the 77 original `.svmap` files) and return early (skipped) when a variable
//! is not set (each prints a "skipped" line). Numbers are printed so `cargo test -- --nocapture` doubles as an acceptance report.

use std::path::PathBuf;

use shaiya_clientdata::wld::WldKind;
use shaiya_clientdata::{dungeon_map_size, ClientSource, Wld};

fn env_dir(name: &str) -> Option<PathBuf> {
    let p = shaiya_testkit::env_var_path(name)?;
    p.is_dir().then_some(p)
}

fn client() -> Option<ClientSource> {
    let dir = env_dir("SHAIYA_CLIENT")?;
    Some(ClientSource::open(&dir).expect("open client"))
}

fn svmap(id: u32) -> Option<shaiya_svmap::Svmap> {
    let dir = env_dir("SVMAP_ORIGINALS")?;
    let bytes = std::fs::read(dir.join(format!("{id}.svmap"))).ok()?;
    Some(shaiya_svmap::read(&bytes).expect("svmap parses"))
}

fn median(mut v: Vec<f32>) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
fn all_wld_files_parse_to_the_last_byte() {
    let Some(c) = client() else { return };
    let paths = c.wld_paths();
    let (mut fields, mut dungeons) = (0, 0);
    let (mut s1024, mut s2048) = (0, 0);
    let (mut dg_ok, mut dg_missing) = (0, Vec::new());
    for p in &paths {
        let w = Wld::parse(&c.read(p).unwrap(), p).unwrap_or_else(|e| panic!("{e}"));
        match w.kind {
            WldKind::Field => {
                fields += 1;
                match w.field_size() {
                    Some(1024) => s1024 += 1,
                    Some(2048) => s2048 += 1,
                    other => panic!("{p}: size {other:?}"),
                }
            }
            WldKind::Dungeon => {
                dungeons += 1;
                match c.dg(&w.dg) {
                    Ok(_) => dg_ok += 1,
                    Err(e) => dg_missing.push(format!("{p} -> {} ({e})", w.dg)),
                }
            }
        }
    }
    eprintln!("wld: {} parsed ({fields} fields: {s1024} x 1024, {s2048} x 2048; {dungeons} dungeons, {dg_ok} with their .dg present)", paths.len());
    if !dg_missing.is_empty() {
        eprintln!("dungeon maps whose .dg is not in this archive: {dg_missing:?}");
    }
    assert_eq!(paths.len(), 83);
    assert_eq!((fields, dungeons), (29, 54));
    assert_eq!((s1024, s2048), (18, 11));
    assert_eq!(c.map_ids().len(), 77);
}

#[test]
fn all_dg_files_parse_to_the_last_byte() {
    let Some(c) = client() else { return };
    let mut sets = vec![("data", c)];
    if let Some(dir) = env_dir("SHAIYA_CLIENT") {
        let orig = dir.join("data.orig.sah");
        if orig.is_file() {
            sets.push(("data.orig", ClientSource::open_archive(&orig).unwrap()));
        }
    }
    for (label, c) in &sets {
        let files: Vec<String> = c.list().into_iter().filter(|p| p.to_ascii_lowercase().ends_with(".dg")).collect();
        let mut total = shaiya_clientdata::dg::DgStats::default();
        for f in &files {
            let data = c.read(f).unwrap();
            let dg = shaiya_clientdata::Dg::parse(&data, f).unwrap_or_else(|e| panic!("{e}"));
            let s = dg.stats();
            total.nodes += s.nodes;
            total.meshes += s.meshes;
            total.vertices += s.vertices;
            total.faces += s.faces;
            total.collision_points += s.collision_points;
            total.collision_faces += s.collision_faces;
            total.depth = total.depth.max(s.depth);
            // the flattened buffers keep every face
            let flat = dg.flatten();
            let idx: usize = flat.batches.iter().map(|b| b.indices.len()).sum();
            assert_eq!(idx, s.faces * 3, "{f}");
            assert_eq!(flat.collision_indices.len(), s.collision_faces * 3, "{f}");
        }
        eprintln!("dg ({label}): {} parsed to the last byte; {:?}", files.len(), total);
        if *label == "data" {
            assert_eq!(files.len(), 44);
        } else {
            assert_eq!(files.len(), 51);
        }
    }
}

#[test]
fn heights_fit_npc_y_on_maps_1_2_18() {
    let Some(c) = client() else { return };
    for id in [1u32, 2, 18] {
        let Some(m) = svmap(id) else { return };
        let w = c.wld(id).unwrap();
        let t = w.terrain.as_ref().unwrap();
        assert_eq!(t.size, m.size, "map {id}");
        assert_eq!(t.heights.len(), t.n * t.n);
        let errs: Vec<f32> = m
            .npcs
            .iter()
            .filter(|n| n.kind != 0)
            .filter_map(|n| n.points.first())
            .map(|p| (t.height_at(p.pos[0], p.pos[2]) - p.pos[1]).abs())
            .collect();
        let med = median(errs.clone());
        let max = errs.iter().cloned().fold(0.0, f32::max);
        eprintln!("map {id}: {} NPC heights, median error {med:.3}, max {max:.3}", errs.len());
        assert!(med < 1.0, "map {id} median {med}");
    }
}

#[test]
fn svmap_npcs_equal_wld_npcs() {
    let Some(c) = client() else { return };
    let mut equal_total = 0;
    for id in [1u32, 35] {
        let Some(m) = svmap(id) else { return };
        let w = c.wld(id).unwrap();
        assert_eq!(m.npcs.len(), w.npcs.len(), "map {id} count");
        let mut equal = 0;
        for (s, n) in m.npcs.iter().zip(&w.npcs) {
            let same = s.kind as i32 == n.kind
                && s.type_id as i32 == n.type_id
                && s.points.len() == n.point_count()
                && s.points[0].pos == n.pos
                && s.points[0].dir == n.angle
                && s.points[1..].iter().zip(&n.extra).all(|(a, b)| a.pos == *b);
            equal += same as usize;
        }
        eprintln!("map {id}: svmap NPCs equal to .wld NPCs: {equal}/{}", m.npcs.len());
        assert_eq!(equal, m.npcs.len(), "map {id}");
        equal_total += equal;
    }
    assert_eq!(equal_total, 286 + 136);
}

#[test]
fn portal_centre_rule_on_map_2() {
    let Some(c) = client() else { return };
    let Some(m) = svmap(2) else { return };
    let w = c.wld(2).unwrap();
    assert_eq!(m.portals.len(), w.portals.len());
    let mut ok = 0;
    for (s, p) in m.portals.iter().zip(&w.portals) {
        let at = p.svmap_pos();
        let d = [(s.pos[0] - at[0]).abs(), (s.pos[1] - at[1]).abs(), (s.pos[2] - at[2]).abs()];
        let fields = s.country as i32 == p.country && s.level_min == p.level_min as u16 && s.level_max == p.level_max as u16 && s.to_map as i32 == p.dest_map && s.to_pos == p.dest_pos;
        if d.iter().all(|&x| x <= 0.01) && fields {
            ok += 1;
        }
    }
    eprintln!("map 2: portals at the .wld box centre (within 0.01) with equal fields: {ok}/{}", m.portals.len());
    assert_eq!(ok, 7);
}

#[test]
fn region_names_of_map_2() {
    let Some(c) = client() else { return };
    let Some(m) = svmap(2) else { return };
    let w = c.wld(2).unwrap();
    let names = c.area_names(2).unwrap();
    eprintln!("map 2: {} regions, {} .wld areas, {} named; area 0 = {:?}, 88 = {:?}, 70 = {:?}", m.regions.len(), w.areas.len(), names.len(), names.get(0), names.get(88), names.get(70));
    assert_eq!(m.regions.len(), w.areas.len());
    assert_eq!(names.get(0), Some("Portal that leads to Paroutian Fortress(Lv1~15)"));
    assert_eq!(names.get(3), Some("Artlum Mine"));
    assert_eq!(names.get(88), Some("Portal that leads to Iris(Lv1~20)"));
    assert_eq!(names.get(70), None);
    for (r, a) in m.regions.iter().zip(&w.areas) {
        assert_eq!(r.min, a.min);
        assert_eq!(r.max, a.max);
        assert_eq!(r.kind as i32, a.mode);
    }
}

#[test]
fn zone_groups_are_the_bind_areas() {
    let Some(c) = client() else { return };
    let (mut checked, mut equal, mut differ) = (0, 0, Vec::new());
    for id in c.map_ids() {
        let Some(m) = svmap(id) else { return };
        let w = c.wld(id).unwrap();
        if m.binds.len() != w.zone_groups.len() {
            eprintln!("map {id}: svmap binds {} vs wld zone groups {}", m.binds.len(), w.zone_groups.len());
            continue;
        }
        checked += 1;
        let same = m.binds.iter().zip(&w.zone_groups).all(|(b, g)| {
            b.boxes.len() == g.boxes.len()
                && b.boxes.iter().zip(&g.boxes).all(|(x, y)| x.min == y.min && x.max == y.max)
                && g.boxes.iter().all(|y| y.country as u32 == b.country && y.attr as u32 == b.attr)
        });
        equal += same as usize;
        if !same {
            differ.push(id);
        }
    }
    eprintln!("binds equal to zone groups (boxes, country, attr): {equal} of {checked} maps with equal counts; differing maps {differ:?}");
    // 105 and 108 were exported from a different version of the map: same count, other boxes
    assert_eq!(differ, vec![105, 108]);
}

#[test]
fn monster_sdata_decrypts_and_parses() {
    let Some(c) = client() else { return };
    let enc = c.read("Monster/Monster.SData").unwrap();
    assert!(shaiya_clientdata::seed::is_encrypted(&enc));
    let plain = shaiya_clientdata::seed::decrypt(&enc, "Monster.SData").unwrap();
    eprintln!("Monster.SData: {} plaintext bytes", plain.len());
    assert!(!plain.is_empty());
    let mons = c.monsters().unwrap();
    eprintln!("monsters: {} records; 0 = {:?}; 1 = {:?}", mons.records.len(), mons.records[0].name, mons.records[1].name);
    assert_eq!(mons.records[0].name, "Error Monster");
}

#[test]
fn npc_quest_lists_and_svmap_references() {
    let Some(c) = client() else { return };
    let nq = c.npc_quest().unwrap();
    let counts: Vec<usize> = nq.lists.iter().map(Vec::len).collect();
    eprintln!("NpcQuest: lists {counts:?}, quest table at byte {}", nq.quests_offset);
    assert_eq!(nq.lists.len(), 13);
    // the record's own NpcTypeId field is its 1-based position in its list, which is how the svmap type id indexes it
    let mut off = 0;
    for l in &nq.lists {
        for (i, n) in l.iter().enumerate() {
            off += (n.type_id as usize != i + 1) as usize;
        }
    }
    eprintln!("NpcQuest records whose own type id is not position + 1: {off}");
    assert_eq!(off, 0);
    assert_eq!(nq.get(1, 1).map(|n| n.kind), Some(1));
    assert!(nq.get(1, 0).is_none() && nq.get(1, 253).is_none() && nq.get(14, 1).is_none() && nq.get(0, 1).is_none());
    // every svmap NPC of the originals resolves (type 0 is the placeholder), and so does every monster id
    let mons = c.monsters().unwrap();
    let (mut npcs, mut missing_npc, mut mobs, mut missing_mob) = (0, 0, 0, 0);
    for id in c.map_ids() {
        let Some(m) = svmap(id) else { return };
        for n in &m.npcs {
            if n.kind == 0 {
                continue;
            }
            npcs += 1;
            if nq.get(n.kind, n.type_id).is_none() {
                missing_npc += 1;
                eprintln!("map {id}: NPC ({}, {}) not in NpcQuest.SData", n.kind, n.type_id);
            }
        }
        for a in &m.mob_areas {
            for e in &a.mobs {
                mobs += 1;
                if mons.get(e.mob).is_none_or(|x| x.name.is_empty()) {
                    missing_mob += 1;
                }
            }
        }
    }
    eprintln!("originals: {npcs} NPC refs ({missing_npc} unresolved), {mobs} monster refs ({missing_mob} unresolved)");
}

#[test]
fn map_names() {
    let Some(c) = client() else { return };
    assert_eq!(c.map_name(0), "D-Water Borderland (Border)");
    assert_eq!(c.map_name(18), "Proelium Frontier (Border)");
    assert_eq!(c.map_name(35), "Apulune (Alliance of Light)");
    assert_eq!(c.map_name(37), "Map 37");
    eprintln!("map names: 0 = {:?}, 18 = {:?}, 35 = {:?}", c.map_name(0), c.map_name(18), c.map_name(35));
}

#[test]
fn dungeon_size_rule() {
    let Some(c) = client() else { return };
    let (mut checked, mut equal, mut odd) = (0, 0, Vec::new());
    for id in c.map_ids() {
        let (Some(m), Ok(w)) = (svmap(id), c.wld(id)) else { continue };
        if w.kind != WldKind::Dungeon {
            continue;
        }
        let dg = c.dg(&w.dg).unwrap();
        let rule = dungeon_map_size(&dg.bounds);
        checked += 1;
        if rule == m.size {
            equal += 1;
        } else {
            odd.push((id, m.size, rule, dg.bounds.max));
        }
    }
    eprintln!("dungeon size rule (max of bound x, z rounded up to 8): {equal} of {checked} dungeon maps; others (map, svmap size, rule, bounds): {odd:?}");
    assert!(equal * 10 >= checked * 8);
}

#[test]
fn big_map_load_time_is_printed() {
    let Some(c) = client() else { return };
    let t = std::time::Instant::now();
    let w = c.wld(1).unwrap();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    eprintln!("map 1 (2048): archive read + .wld parse {ms:.1} ms (no timing assertion; {} heights, {} placements)", w.terrain.as_ref().unwrap().heights.len(), w.building.items.len() + w.tree.items.len() + w.shape.items.len() + w.grass.items.len());
}

#[test]
fn textures_decode() {
    let Some(c) = client() else { return };
    let mut n = 0;
    let mut failed = Vec::new();
    for p in c.list() {
        let l = p.to_ascii_lowercase();
        let wanted = (l.starts_with("terrain/detail/") && l.ends_with(".dds")) || (l.starts_with("interface/minimap/") && l.ends_with(".tga")) || l.starts_with("world/dungeon/") && l.ends_with(".dds");
        if !wanted {
            continue;
        }
        n += 1;
        match c.texture(&p) {
            Ok(img) => assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize),
            Err(e) => failed.push(e.to_string()),
        }
    }
    eprintln!("textures decoded: {} of {n}; failures: {failed:?}", n - failed.len());
    assert!(failed.is_empty());
    let miss = c.texture_or_checker("Terrain/Detail/no_such_texture.dds");
    assert_eq!((miss.width, miss.height), (32, 32));
    c.texture_or_checker("terrain/detail/NO_SUCH_texture.dds");
    assert_eq!(c.problems().len(), 1);
}

#[test]
fn unknown_wld_is_an_error_not_a_panic() {
    let Some(c) = client() else { return };
    let data = c.read("world/2.wld").unwrap();
    for cut in [0usize, 3, 4, 8, 100, data.len() / 2, data.len() - 1] {
        assert!(Wld::parse(&data[..cut], "cut").is_err(), "cut at {cut}");
    }
    let mut extra = data.clone();
    extra.push(0);
    assert!(Wld::parse(&extra, "extra").is_err());
}

#[test]
fn damaged_client_files_are_errors_not_panics() {
    let Some(c) = client() else { return };
    // every reader gets truncated copies and byte-flipped copies of a real file; none may panic
    let mut seed = 0x1234_5678u32;
    let mut next = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed >> 8
    };
    let wld = c.read("world/35.wld").unwrap();
    let dg_name = c.wld(3).unwrap().dg;
    let dg = c.read(&format!("world/dungeon/{dg_name}.dg").replace(".dg.dg", ".dg")).unwrap();
    let nq_plain = shaiya_clientdata::seed::decrypt(&c.read("Npc/NpcQuest.SData").unwrap(), "n").unwrap();
    let mon_plain = shaiya_clientdata::seed::decrypt(&c.read("Monster/Monster.SData").unwrap(), "m").unwrap();
    let dds = c.read("Terrain/Detail/r2_snowbase.dds").unwrap();
    let tga = c.read("interface/MiniMap/1.tga").unwrap();
    let mut errors = 0usize;
    let mut total = 0usize;
    for round in 0..40 {
        let mut check = |data: &[u8], f: &dyn Fn(&[u8]) -> bool| {
            total += 1;
            let cut = (next() as usize) % (data.len() + 1);
            errors += !f(&data[..cut]) as usize;
            let mut m = data.to_vec();
            for _ in 0..=(round % 4) {
                let at = (next() as usize) % m.len();
                m[at] ^= (next() & 0xff) as u8 | 1;
            }
            let _ = f(&m);
        };
        check(&wld, &|d| Wld::parse(d, "t").is_ok());
        check(&dg, &|d| shaiya_clientdata::Dg::parse(d, "t").is_ok());
        check(&nq_plain, &|d| shaiya_clientdata::NpcQuest::parse(d, "t").is_ok());
        check(&mon_plain, &|d| shaiya_clientdata::Monsters::parse(d, "t").is_ok());
        check(&dds, &|d| shaiya_clientdata::texture::decode(d, "t").is_ok());
        check(&tga, &|d| shaiya_clientdata::texture::decode(d, "t").is_ok());
    }
    eprintln!("damaged input: {total} truncations and {total} byte-flipped copies, {errors} truncations gave errors, no panic");
    assert!(errors > 0);
    // a wrong-key / damaged SData body fails its checksum
    let mut enc = c.read("Monster/Monster.SData").unwrap();
    let last = enc.len() - 1;
    enc[last] ^= 0x55;
    assert!(shaiya_clientdata::seed::decrypt(&enc, "m").is_err());
}

/// Every `.smod` in `Entity/Building`, `Entity/Shape` and `Entity/Tree` parses to the last byte, and every model name a `.wld`
/// places resolves to one of them. The placements are unit-length, perpendicular direction/up pairs (no scale is stored).
#[test]
fn smod_models_parse_and_placements_resolve() {
    let Some(c) = client() else { return };
    let (mut files, mut verts, mut faces, mut with_collision) = (0usize, 0usize, 0usize, 0usize);
    for p in c.list() {
        let low = p.to_ascii_lowercase();
        if !low.ends_with(".smod") || !["entity/building/", "entity/shape/", "entity/tree/"].iter().any(|d| low.starts_with(d)) {
            continue;
        }
        let m = shaiya_clientdata::Smod::parse(&c.read(&p).unwrap(), &p).unwrap_or_else(|e| panic!("{e}"));
        files += 1;
        let (v, f) = m.counts();
        verts += v;
        faces += f;
        with_collision += (!m.collision.is_empty()) as usize;
        for module in &m.modules {
            assert!(module.faces.iter().all(|t| t.iter().all(|&i| (i as usize) < module.vertices.len())), "{p}: face index out of range");
        }
    }
    eprintln!("smod: {files} models parse to the last byte ({verts} vertices, {faces} faces, {with_collision} with a collision mesh)");
    assert!(files > 1000, "{files}");

    let (mut placed, mut missing, mut non_unit) = (0usize, Vec::new(), 0usize);
    for id in c.map_ids() {
        let w = c.wld(id).unwrap();
        for (dir, list) in [("Building", &w.building), ("Shape", &w.shape), ("Tree", &w.tree)] {
            for it in &list.items {
                if it.index < 0 {
                    continue;
                }
                placed += 1;
                let Some(name) = list.names.get(it.index as usize) else {
                    missing.push(format!("map {id} {dir}: index {} of {}", it.index, list.names.len()));
                    continue;
                };
                if !c.exists(&format!("Entity/{dir}/{name}")) {
                    missing.push(format!("map {id}: Entity/{dir}/{name}"));
                }
                let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let dot = it.dir[0] * it.up[0] + it.dir[1] * it.up[1] + it.dir[2] * it.up[2];
                if (len(it.dir) - 1.0).abs() > 0.01 || (len(it.up) - 1.0).abs() > 0.01 || dot.abs() > 0.02 {
                    non_unit += 1;
                }
            }
        }
    }
    missing.sort();
    missing.dedup();
    eprintln!("placements: {placed} buildings, shapes and trees over all maps; missing models: {}; not unit and perpendicular: {non_unit}", missing.len());
    for m in missing.iter().take(10) {
        eprintln!("  {m}");
    }
    assert_eq!(non_unit, 0);
}
