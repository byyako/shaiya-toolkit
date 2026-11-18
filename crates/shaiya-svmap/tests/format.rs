//! Unit-level tests per section on a synthetic map: read/write, JSON, truncation, validation rules.

use shaiya_svmap::*;

#[allow(clippy::approx_constant)] // 3.14 and 6.28 are test data, not constants
fn sample() -> Svmap {
    let mut m = Svmap::new(16);
    m.blocked.set(1, 0, true);
    m.blocked.set(15, 15, true);
    m.section_a.push(RawA { pos: [1.5, 2.0, 3.25], a: 7, b: 9 });
    m.points_b = vec![[4.0, 5.0, 6.0], [-0.0, 0.1, 15.9]];
    m.mob_areas = vec![
        MobArea {
            min: [2.0, 10.0, 2.0],
            max: [6.0, 12.0, 6.0],
            mobs: vec![MobEntry { mob: 12, count: 3 }, MobEntry { mob: 40, count: 8 }],
        },
        MobArea { min: [8.0, 0.0, 8.0], max: [9.0, 2.0, 9.0], mobs: vec![] },
    ];
    m.npcs = vec![
        NpcEntry { kind: 4, type_id: 2, points: vec![NpcPoint { pos: [3.0, 1.0, 3.0], dir: 1.57 }] },
        NpcEntry {
            kind: 7,
            type_id: 0,
            points: vec![
                NpcPoint { pos: [5.0, 1.0, 5.0], dir: 0.0 },
                NpcPoint { pos: [6.0, 1.0, 5.0], dir: 3.14 },
                NpcPoint { pos: [7.0, 1.0, 5.5], dir: 6.28 },
            ],
        },
    ];
    m.portals = vec![
        Portal { pos: [10.0, 0.0, 10.0], country: 1, level_min: 0, level_max: 15, to_map: 2, to_pos: [100.5, 3.0, 200.5] },
        Portal { pos: [11.0, 0.0, 10.0], country: 1001, level_min: 20, level_max: 80, to_map: 200, to_pos: [0.0, 0.0, 0.0] },
    ];
    m.binds = vec![
        Bind { country: 0, attr: 0, boxes: vec![Aabb { min: [1.0, 0.0, 1.0], max: [3.0, 2.0, 3.0] }] },
        Bind {
            country: 2,
            attr: 5,
            boxes: vec![
                Aabb { min: [4.0, 0.0, 4.0], max: [5.0, 2.0, 5.0] },
                Aabb { min: [6.0, 0.0, 6.0], max: [7.0, 2.0, 7.0] },
            ],
        },
    ];
    m.regions = vec![
        Region { min: [0.0, 0.0, 0.0], max: [8.0, 5.0, 8.0], kind: 2, b: 1, c: 3, d: 100 },
        Region { min: [8.0, 0.0, 8.0], max: [15.0, 5.0, 15.0], kind: 0, b: 0, c: 0, d: 0 },
        Region { min: [1.0, 0.0, 1.0], max: [2.0, 5.0, 2.0], kind: 1, b: 255, c: 255, d: 65535 },
    ];
    m
}

fn codes(f: &[Finding]) -> Vec<&'static str> {
    f.iter().map(|x| x.code).collect()
}

#[test]
fn write_then_read_gives_the_same_map_and_bytes() {
    let m = sample();
    let bytes = write(&m);
    let back = read(&bytes).unwrap();
    assert_eq!(back, m);
    assert_eq!(write(&back), bytes);
}

#[test]
fn layout_matches_the_documented_sizes() {
    let m = sample();
    let expect = 4 // size
        + 32 // 16*16/8
        + 4 + 20 // A
        + 4 + 2 * 12 // B
        + 4 + (24 + 4 + 16) + (24 + 4) // monster areas
        + 4 + (8 + 4 + 16) + (8 + 4 + 48) // NPCs
        + 4 + 2 * 36 // portals
        + 4 + (12 + 24) + (12 + 48) // binds
        + 4 + 3 * 32; // regions
    assert_eq!(write(&m).len(), expect);
}

#[test]
fn bit_order_in_the_file() {
    let mut m = Svmap::new(16);
    m.blocked.set(1, 0, true); // bit 1 of byte 0
    m.blocked.set(0, 1, true); // bit 16 = byte 2 bit 0
    let b = write(&m);
    assert_eq!(&b[0..4], &16u32.to_le_bytes());
    assert_eq!(b[4], 0b10);
    assert_eq!(b[5], 0);
    assert_eq!(b[6], 1);
}

#[test]
fn empty_map_roundtrips() {
    for size in [0u32, 1, 3, 8, 96] {
        let m = Svmap::new(size);
        let b = write(&m);
        assert_eq!(read(&b).unwrap(), m);
    }
}

#[test]
fn padding_bits_in_the_grid_are_preserved() {
    let m = Svmap::new(3); // 9 cells -> 2 bytes, 7 padding bits
    let mut bytes = write(&m);
    bytes[4 + 1] = 0xFE; // stray padding bits set
    let m = read(&bytes).unwrap();
    assert_eq!(write(&m), bytes);
}

#[test]
fn negative_zero_and_nan_bits_survive_the_binary_roundtrip() {
    let mut m = sample();
    m.points_b[0] = [-0.0, f32::from_bits(0x7FC0_1234), f32::INFINITY];
    let b = write(&m);
    let back = read(&b).unwrap();
    assert_eq!(write(&back), b);
    assert_eq!(back.points_b[0][1].to_bits(), 0x7FC0_1234);
}

#[test]
fn truncation_at_every_section_boundary_names_the_next_section() {
    let m = sample();
    let full = write(&m);
    let mut cuts: Vec<(usize, &str)> = Vec::new();
    let mut off = 4;
    cuts.push((off, "blocked cells"));
    off += 32;
    cuts.push((off, "section A"));
    off += 4 + 20;
    cuts.push((off, "section B"));
    off += 4 + 24;
    cuts.push((off, "monster areas"));
    off += 4 + 44 + 28;
    cuts.push((off, "NPCs"));
    off += 4 + 28 + 60;
    cuts.push((off, "portals"));
    off += 4 + 72;
    cuts.push((off, "bind areas"));
    off += 4 + 36 + 60;
    cuts.push((off, "regions"));
    off += 4 + 96;
    assert_eq!(off, full.len());
    for (cut, section) in cuts {
        let e = read(&full[..cut]).unwrap_err();
        assert_eq!(e.section, section, "cut at {cut}: {e}");
        assert!(matches!(e.kind, ReadErrorKind::Truncated { .. }), "{e}");
        assert!(e.offset <= cut + 4, "{e}");
    }
    // size itself
    for cut in 0..4 {
        assert_eq!(read(&full[..cut]).unwrap_err().section, "size");
    }
}

#[test]
fn truncation_inside_records_names_section_and_record() {
    let m = sample();
    let full = write(&m);
    // 3 bytes before the end: inside the last region
    let e = read(&full[..full.len() - 3]).unwrap_err();
    assert_eq!((e.section, e.index), ("regions", Some(2)), "{e}");
    assert!(e.to_string().starts_with("regions record 2 at byte"), "{e}");
    // every shorter prefix is an error and never a panic
    for cut in 0..full.len() {
        assert!(read(&full[..cut]).is_err(), "prefix {cut} parsed");
    }
}

#[test]
fn trailing_bytes_are_an_error_with_the_count() {
    let mut b = write(&sample());
    let len = b.len();
    b.extend_from_slice(&[1, 2, 3]);
    let e = read(&b).unwrap_err();
    assert_eq!(e.kind, ReadErrorKind::TrailingBytes { count: 3 });
    assert_eq!(e.offset, len);
    assert!(e.to_string().contains("3 unexpected bytes"));
}

#[test]
fn absurd_counts_are_rejected_without_allocating() {
    // size 0, then section A count = 0xFFFFFFFF
    let mut b = vec![0u8; 4];
    b.extend_from_slice(&u32::MAX.to_le_bytes());
    let e = read(&b).unwrap_err();
    assert_eq!(e.section, "section A");
    assert_eq!(e.kind, ReadErrorKind::CountTooLarge { count: u32::MAX });
    // a count at the cap that the file cannot hold is truncation
    let mut b = vec![0u8; 4];
    b.extend_from_slice(&1_000_000u32.to_le_bytes());
    let e = read(&b).unwrap_err();
    assert!(matches!(e.kind, ReadErrorKind::Truncated { .. }), "{e}");
    // huge size with no grid bytes
    let e = read(&u32::MAX.to_le_bytes()).unwrap_err();
    assert_eq!(e.section, "blocked cells");
    assert!(matches!(e.kind, ReadErrorKind::Truncated { .. }));
}

#[test]
fn nested_counts_are_checked_too() {
    // a monster area claiming 500000 entries in a tiny file
    let mut m = Svmap::new(0);
    m.mob_areas.push(MobArea { min: [0.0; 3], max: [1.0; 3], mobs: vec![] });
    let mut b = write(&m);
    // the entry count is the u32 after the two vec3 of the one area: 4 (size) + 4 (A) + 4 (B) + 4 (areas) + 24
    let at = 4 + 4 + 4 + 4 + 24;
    b[at..at + 4].copy_from_slice(&500_000u32.to_le_bytes());
    let e = read(&b).unwrap_err();
    assert_eq!((e.section, e.index), ("monster areas", Some(0)));
    assert!(matches!(e.kind, ReadErrorKind::Truncated { .. }));
}

#[test]
fn json_roundtrip_is_exact() {
    let mut m = sample();
    // awkward floats: shortest-decimal round trips
    m.points_b.push([0.1, 1.0 / 3.0, 16777217.0]);
    m.points_b.push([f32::MIN_POSITIVE, f32::MAX, -0.0]);
    let j = to_json(&m).unwrap();
    let back = from_json(&j).unwrap();
    assert_eq!(back, m);
    assert_eq!(write(&back), write(&m));
    let pretty = to_json_pretty(&m).unwrap();
    assert_eq!(write(&from_json(&pretty).unwrap()), write(&m));
}

#[test]
fn json_field_names_and_blocked_encoding() {
    let m = sample();
    let v: serde_json::Value = serde_json::from_str(&to_json(&m).unwrap()).unwrap();
    for k in ["size", "blocked", "section_a", "points_b", "mob_areas", "npcs", "portals", "binds", "regions"] {
        assert!(v.get(k).is_some(), "missing {k}");
    }
    assert!(v["blocked"].is_string());
    assert_eq!(v["mob_areas"][0]["mobs"][1]["count"], 8);
    assert_eq!(v["portals"][0]["to_map"], 2);
    assert_eq!(v["regions"][2]["d"], 65535);
}

#[test]
fn json_errors() {
    let m = sample();
    let good = to_json(&m).unwrap();
    assert!(matches!(from_json("not json"), Err(JsonError::Parse(_))));
    let v: serde_json::Value = serde_json::from_str(&good).unwrap();
    let blocked = v["blocked"].as_str().unwrap().to_string();
    let bad_b64 = good.replace(&blocked, "!!!");
    assert!(matches!(from_json(&bad_b64), Err(JsonError::BadBase64(_))));
    let short = good.replacen("\"size\":16", "\"size\":64", 1);
    assert!(matches!(from_json(&short), Err(JsonError::BadBlockedLength { size: 64, .. })));
    let mut nan = m.clone();
    nan.points_b[0][0] = f32::NAN;
    assert!(matches!(to_json(&nan), Err(JsonError::NonFinite { section: "section B", index: 0 })));
}

// ---- validation ----

#[test]
fn sample_has_no_errors() {
    let f = validate(&sample());
    assert!(f.iter().all(|x| x.severity != Severity::Error), "{f:?}");
    // the empty area is info, the 8-count is fine
    assert!(codes(&f).contains(&"area-empty"));
}

#[test]
fn position_and_box_rules() {
    let mut m = sample();
    m.points_b[0] = [16.0, 0.0, 1.0]; // x == size is outside
    m.points_b[1] = [1.0, 0.0, -0.5];
    m.mob_areas[0].min = [9.0, 0.0, 2.0]; // min x > max x
    let c = codes(&validate(&m));
    assert_eq!(c.iter().filter(|c| **c == "pos-out-of-range").count(), 2);
    assert!(c.contains(&"box-inverted"));
}

#[test]
fn nan_is_an_error_and_not_a_position_error() {
    let mut m = sample();
    m.points_b[0] = [f32::NAN, 0.0, 0.0];
    m.portals[0].to_pos = [f32::INFINITY, 0.0, 0.0];
    let f = validate(&m);
    assert_eq!(f.iter().filter(|x| x.code == "non-finite").count(), 2);
    assert!(!codes(&f).contains(&"pos-out-of-range"));
}

#[test]
fn monster_area_rules() {
    let mut m = sample();
    m.mob_areas[0].mobs = vec![
        MobEntry { mob: 0, count: 1 },
        MobEntry { mob: 5, count: 0 },
        MobEntry { mob: 5, count: 9 },
    ];
    let f = validate(&m);
    let find = |c: &str| f.iter().find(|x| x.code == c).unwrap_or_else(|| panic!("{c}"));
    assert_eq!(find("area-mob-zero").severity, Severity::Warning);
    assert_eq!((find("area-count-zero").severity, find("area-count-zero").sub), (Severity::Error, Some(1)));
    assert_eq!((find("area-count-high").severity, find("area-count-high").sub), (Severity::Warning, Some(2)));
    assert_eq!(find("area-empty").severity, Severity::Info);
}

#[test]
fn npc_rules() {
    let mut m = sample();
    m.npcs[0].points.clear();
    m.npcs[1].kind = 14;
    m.npcs.push(NpcEntry { kind: 0, type_id: 0, points: vec![NpcPoint { pos: [1.0, 0.0, 1.0], dir: 0.0 }] });
    m.npcs.push(NpcEntry { kind: 13, type_id: 0, points: vec![NpcPoint { pos: [15.5, 0.0, 15.5], dir: 0.0 }] });
    let f = validate(&m);
    let sev = |c: &str| f.iter().find(|x| x.code == c).map(|x| x.severity);
    assert_eq!(sev("npc-no-points"), Some(Severity::Error));
    assert_eq!(sev("npc-type-invalid"), Some(Severity::Error));
    assert_eq!(sev("npc-type-zero"), Some(Severity::Info));
    // type 13 is valid; the NPC on the blocked cell (15,15) warns
    assert_eq!(sev("npc-on-blocked"), Some(Severity::Warning));
    assert_eq!(f.iter().filter(|x| x.code == "npc-type-invalid").count(), 1);
}

#[test]
fn portal_rules() {
    let mut m = sample();
    m.portals[0].level_min = 50;
    m.portals[0].level_max = 10;
    m.portals[1].country = 7;
    m.portals.push(Portal { level_min: 50, level_max: 0, ..m.portals[0] }); // max 0 = no limit: fine
    let f = validate(&m);
    assert_eq!(f.iter().filter(|x| x.code == "portal-levels").count(), 1);
    assert_eq!(f.iter().filter(|x| x.code == "portal-country").count(), 1);
}

#[test]
fn bind_rules() {
    let mut m = sample();
    m.binds[0].boxes.clear();
    m.binds[1].country = 4;
    m.binds[1].boxes[0] = Aabb { min: [14.0, 0.0, 14.0], max: [16.0, 2.0, 16.0] }; // centre (15,15) blocked, max corner 16 outside
    let f = validate(&m);
    let c = codes(&f);
    assert!(c.contains(&"bind-no-box") && c.contains(&"bind-country") && c.contains(&"bind-on-blocked"));
    assert!(c.contains(&"pos-out-of-range"));
}

#[test]
fn area_centre_on_blocked_cell() {
    let mut m = sample();
    m.mob_areas[1].min = [0.5, 0.0, 0.0];
    m.mob_areas[1].max = [2.5, 2.0, 2.0]; // centre (1.5, 1.0) -> cell (1, 1): free
    assert!(!codes(&validate(&m)).contains(&"area-on-blocked"));
    m.mob_areas[1].max = [2.0, 2.0, 0.5]; // centre (1.25, 0.25) -> cell (1, 0): blocked
    assert!(codes(&validate(&m)).contains(&"area-on-blocked"));
}

#[test]
fn cross_file_rules() {
    let mut a = sample();
    let mut dest = Svmap::new(256);
    dest.blocked.set(100, 200, true); // portal 0 destination (100.5, 200.5)
    a.portals.push(Portal { to_map: 9, ..a.portals[0] }); // not in the folder
    a.portals.push(Portal { to_map: 2, to_pos: [300.0, 0.0, 10.0], ..a.portals[0] }); // outside map 2
    let lookup = |id: u32| if id == 2 { Some(&dest) } else { None };
    let f = validate_cross(&a, &lookup);
    let by = |c: &str| f.iter().filter(|x| x.code == c).collect::<Vec<_>>();
    assert_eq!(by("portal-dest-blocked").len(), 1);
    assert_eq!(by("portal-dest-blocked")[0].index, Some(0));
    assert_eq!(by("portal-map-missing").len(), 1);
    assert_eq!(by("portal-map-missing")[0].index, Some(2));
    assert_eq!(by("portal-dest-outside").len(), 1);
    assert_eq!(by("portal-dest-outside")[0].severity, Severity::Error);
    // portal 1 goes to map 200 (guild ranking): never reported
    assert!(f.iter().all(|x| x.index != Some(1)));
}

#[test]
fn blocked_grid_size_mismatch_is_reported() {
    let mut m = sample();
    m.size = 32;
    assert!(codes(&validate(&m)).contains(&"blocked-size"));
}

#[test]
fn region_overhang_is_a_warning_not_an_error() {
    let mut m = sample();
    m.regions[0].min = [-2.0, 0.0, 3.0]; // the originals have such regions on maps 1, 44 and 100
    let f = validate(&m);
    let r = f.iter().find(|x| x.code == "region-out-of-range").expect("reported");
    assert_eq!((r.severity, r.section, r.index), (Severity::Warning, SectionId::Regions, Some(0)));
    assert!(f.iter().all(|x| x.severity != Severity::Error));
}
