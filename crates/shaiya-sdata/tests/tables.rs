//! Synthetic tables built here, byte by byte, in the shapes the layouts describe. No game data.

use shaiya_core::text::Encoding;
use shaiya_sdata::{csv, csv_text, read, read_file, Hint, TableKind, Value};

fn u32le(v: &mut Vec<u8>, n: u32) {
    v.extend_from_slice(&n.to_le_bytes());
}

fn lenstr(v: &mut Vec<u8>, b: &[u8]) {
    u32le(v, b.len() as u32);
    v.extend_from_slice(b);
}

/// `fixed` bytes of a block: byte i = (seed + i) mod 251, with the first fields set so the test can read them back.
fn block(seed: u8, size: usize) -> Vec<u8> {
    (0..size).map(|i| ((usize::from(seed) + i) % 251) as u8).collect()
}

/// (name, description, seed) of one record.
type Rec<'a> = (&'a [u8], &'a [u8], u8);

/// An Item file: banks of records.
fn item(banks: &[Vec<Rec>], fixed: usize) -> Vec<u8> {
    let mut v = Vec::new();
    u32le(&mut v, banks.len() as u32);
    for b in banks {
        u32le(&mut v, b.len() as u32);
        for (n, d, s) in b {
            lenstr(&mut v, n);
            lenstr(&mut v, d);
            v.extend_from_slice(&block(*s, fixed));
        }
    }
    v
}

fn skill(entries: usize, levels: usize, fixed: usize) -> Vec<u8> {
    let mut v = Vec::new();
    u32le(&mut v, entries as u32);
    for e in 0..entries {
        for l in 0..levels {
            lenstr(&mut v, format!("Skill {e}").as_bytes());
            lenstr(&mut v, format!("level {l}").as_bytes());
            v.extend_from_slice(&block((e * 7 + l) as u8, fixed));
        }
    }
    v
}

fn monster(names: &[&[u8]]) -> Vec<u8> {
    monster_of(names, 31)
}

/// A Monster file with a fixed block of `fixed` bytes per record.
fn monster_of(names: &[&[u8]], fixed: usize) -> Vec<u8> {
    let mut v = Vec::new();
    u32le(&mut v, names.len() as u32);
    for (i, n) in names.iter().enumerate() {
        let mut with_nul = n.to_vec();
        with_nul.push(0);
        lenstr(&mut v, &with_nul);
        let mut b = block(i as u8, fixed);
        b[0..2].copy_from_slice(&(100 + i as i16).to_le_bytes());
        b[5..9].copy_from_slice(&(5000i32).to_le_bytes());
        v.extend_from_slice(&b);
    }
    v
}

/// An NpcQuest file with one merchant, no gatekeepers, empty kinds 3..13, empty links, and `quests` quests of the 165-byte shape.
fn npcquest(quests: usize) -> Vec<u8> {
    let mut v = Vec::new();
    u32le(&mut v, 1); // merchants
    v.extend_from_slice(&block(1, 20));
    lenstr(&mut v, b"Smith");
    lenstr(&mut v, b"Welcome");
    u32le(&mut v, 2); // stock: two (u8, u8)
    v.extend_from_slice(&[1, 2, 3, 4]);
    u32le(&mut v, 1); // in quests
    v.extend_from_slice(&7u16.to_le_bytes());
    u32le(&mut v, 0); // out quests
    u32le(&mut v, 0); // gatekeepers
    for _ in 0..11 {
        u32le(&mut v, 0);
    }
    v.extend(std::iter::repeat_n(0u8, 65536 * 8)); // links: two empty lists each
    u32le(&mut v, quests as u32);
    for q in 0..quests {
        v.extend_from_slice(&(q as u16 + 1).to_le_bytes());
        lenstr(&mut v, b"Quest title");
        lenstr(&mut v, b"Quest text");
        v.extend_from_slice(&block(q as u8, 165));
        for s in 0..7 {
            lenstr(&mut v, format!("s{s}").as_bytes());
        }
    }
    v
}

/// An NpcQuest file in the 287-byte quest shape: one merchant, one gatekeeper, no NPCs, empty links, `quests` fixed 287-byte quests
/// and no strings anywhere.
fn npcquest_287(quests: usize) -> Vec<u8> {
    let mut v = Vec::new();
    u32le(&mut v, 1); // merchants: fixed 20, then three lists (two (u8, u8) pairs, one u16, none)
    v.extend_from_slice(&block(1, 20));
    u32le(&mut v, 2);
    v.extend_from_slice(&[1, 2, 3, 4]);
    u32le(&mut v, 1);
    v.extend_from_slice(&7u16.to_le_bytes());
    u32le(&mut v, 0);
    u32le(&mut v, 1); // gatekeepers: fixed 19, three gates of 18, two lists
    v.extend_from_slice(&block(2, 19 + 3 * 18));
    u32le(&mut v, 0);
    u32le(&mut v, 0);
    for _ in 0..11 {
        u32le(&mut v, 0);
    }
    v.extend(std::iter::repeat_n(0u8, 65536 * 8));
    u32le(&mut v, quests as u32);
    for q in 0..quests {
        v.extend_from_slice(&block(q as u8, 287));
    }
    v
}

fn read_ok(data: &[u8], hint: Hint) -> shaiya_sdata::Table {
    read(data, hint, Encoding::Cp949).unwrap_or_else(|e| panic!("{e}: {:?}", e.tried))
}

#[test]
fn an_item_table_with_two_banks_and_cp949_names() {
    // "검" = B0 CB in cp949
    let data = item(&[vec![(&b"Sword"[..], &b"A sword"[..], 1), (&[0xB0, 0xCB][..], &b"cp949 name"[..], 2)], vec![(&b"Axe"[..], &b""[..], 3)]], 81);
    let t = read_ok(&data, Hint::Auto);
    assert_eq!(t.layout.variant, "item-81");
    assert_eq!(t.layout.table, TableKind::Item);
    let s = t.primary();
    assert_eq!(s.rows.len(), 3);
    let names: Vec<&str> = s.columns.iter().take(4).map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["bank", "name", "description", "type"]);
    assert_eq!(s.columns.len(), 3 + 51, "bank, name, description and the 51 fields of the 81-byte block");
    assert_eq!(s.rows[0][..3], [Value::Int(0), Value::Text("Sword".into()), Value::Text("A sword".into())]);
    let hangul = Encoding::Cp949.decode(&[0xB0, 0xCB]).0;
    assert!(hangul.chars().count() == 1 && !hangul.is_ascii(), "the two bytes are one Korean syllable");
    assert_eq!(s.rows[1][1], Value::Text(hangul));
    assert_eq!(s.rows[2][0], Value::Int(1), "the third record is in bank 1");
    // type = byte 0 of the block (seed 1), type_id = byte 1, req_level = u16 at 4
    assert_eq!(s.rows[0][3], Value::Int(1));
    assert_eq!(s.rows[0][4], Value::Int(2));
    let req = s.columns.iter().position(|c| c.name == "req_level").unwrap();
    assert_eq!(s.rows[0][req], Value::Int(i64::from(u16::from_le_bytes([5, 6]))));
    // another code page gives other text for the same bytes
    let t1252 = read(&data, Hint::Auto, Encoding::Cp1252).unwrap();
    assert_eq!(t1252.primary().rows[1][1], Value::Text("\u{B0}\u{CB}".into()));
}

#[test]
fn a_truncated_file_and_one_extra_byte_do_not_fit() {
    let data = item(&[vec![(&b"Sword"[..], &b"A sword"[..], 1)], vec![(&b"Axe"[..], &b"x"[..], 3)]], 81);
    read_ok(&data, Hint::Auto);
    let cut = &data[..data.len() - 1];
    let e = read(cut, Hint::Auto, Encoding::Cp949).unwrap_err();
    assert_eq!(e.len, cut.len());
    assert!(e.ambiguous.is_empty() && !e.tried.is_empty());
    assert!(e.to_string().starts_with(&format!("No table layout fits this file ({} bytes)", cut.len())), "{e}");
    let mut more = data.clone();
    more.push(0);
    let e = read(&more, Hint::Auto, Encoding::Cp949).unwrap_err();
    assert!(e.tried.iter().any(|(v, _, why)| v == "item-81" && why.contains("1 bytes are left after the table")), "{:?}", e.tried);
    // nothing is not a table; neither is random text
    assert!(read(&[], Hint::Auto, Encoding::Cp949).is_err());
    assert!(read(b"This is not a table at all, only text.", Hint::Auto, Encoding::Cp949).is_err());
}

#[test]
fn the_variants_are_told_apart_by_fit_alone() {
    let book = |fixed| item(&[vec![(&b"A"[..], &b"B"[..], 1)]], fixed);
    assert_eq!(read_ok(&book(81), Hint::Auto).layout.variant, "item-81");
    assert_eq!(read_ok(&book(132), Hint::Auto).layout.variant, "item-132");
    assert_eq!(read_ok(&book(132), Hint::Table(TableKind::Item)).layout.variant, "item-132");
    for (levels, fixed, variant) in [(3, 116, "skill-3"), (9, 116, "skill-9"), (15, 138, "skill-15")] {
        let t = read_ok(&skill(4, levels, fixed), Hint::Auto);
        assert_eq!((t.layout.variant.as_str(), t.primary().rows.len()), (variant, 4 * levels));
        // the first column is the skill number, counted from 1; every skill has `levels` rows
        assert_eq!(t.primary().rows[levels][0], Value::Int(2));
    }
    // a 3-level reader never accepts the 9-level file
    assert!(read(&skill(4, 9, 116), Hint::Table(TableKind::Skill), Encoding::Cp949).unwrap().layout.variant == "skill-9");
}

#[test]
fn nothing_ambiguous_is_picked() {
    // zero banks, zero skills and zero monsters are all the same four bytes: every layout of those tables fits
    let zero = [0u8; 4];
    let e = read(&zero, Hint::Auto, Encoding::Cp949).unwrap_err();
    assert!(e.ambiguous.len() > 1 && e.ambiguous.contains(&"item-81".to_string()) && e.ambiguous.contains(&"monster-31".to_string()), "{e:?}");
    assert!(e.to_string().contains("more than one layout reads it to the end"));
    let e = read(&zero, Hint::Table(TableKind::Item), Encoding::Cp949).unwrap_err();
    assert_eq!(e.ambiguous, ["item-81", "item-132", "item-134"]);
}

#[test]
fn monster_and_npcquest_tables() {
    let t = read_ok(&monster(&[b"Wolf", b"Bear"]), Hint::Auto);
    assert_eq!(t.layout.variant, "monster-31");
    let s = t.primary();
    assert_eq!(s.rows.len(), 2);
    assert_eq!(s.rows[1][..3], [Value::Int(1), Value::Text("Bear".into()), Value::Int(101)], "the trailing NUL of the stored name is not shown");
    let hp = s.columns.iter().position(|c| c.name == "hp").unwrap();
    assert_eq!(s.rows[0][hp], Value::Int(5000));

    let t = read_ok(&npcquest(3), Hint::Auto);
    assert_eq!(t.layout.variant, "npcquest-165");
    let names: Vec<&str> = t.sheets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["merchants", "gatekeepers", "npcs", "quest_links", "quests"]);
    assert_eq!(t.primary().name, "quests");
    assert_eq!(t.primary().rows.len(), 3);
    assert_eq!(t.sheet("quest_links").unwrap().rows.len(), 65536);
    assert!(t.sheet("npcs").unwrap().rows.is_empty());
    let m = t.sheet("merchants").unwrap();
    assert_eq!(m.rows.len(), 1);
    let col = |n: &str| m.columns.iter().position(|c| c.name == n).unwrap();
    assert_eq!(m.rows[0][col("stock")], Value::Text("1:2 3:4".into()));
    assert_eq!(m.rows[0][col("in_quests")], Value::Text("7".into()));
    assert_eq!(m.rows[0][col("out_quests")], Value::Text(String::new()));
    assert_eq!(m.rows[0][col("name")], Value::Text("Smith".into()));
    assert_eq!(m.rows[0][col("id")], Value::Int(1));
    let q = t.primary();
    assert_eq!(q.rows[2][0], Value::Int(3), "quests are numbered from 1");
    assert_eq!(q.columns.last().unwrap().name, "text7");
    // a quest record of the other shape is another variant (87 bytes, six sub-blocks) and does not fit this file
    assert!(read(&npcquest(3), Hint::Table(TableKind::Monster), Encoding::Cp949).is_err());
}

#[test]
fn a_seed_wrapped_file_is_unwrapped_first_and_a_damaged_one_is_refused() {
    let plain = monster(&[b"Wolf"]);
    let enc = shaiya_seed::encrypt(&plain);
    let t = read_file(&enc, "Monster.SData", Hint::Auto, Encoding::Cp949).unwrap();
    assert_eq!(t.primary().rows.len(), 1);
    // a plain file works too
    assert_eq!(read_file(&plain, "Monster.SData", Hint::Auto, Encoding::Cp949).unwrap().primary().rows.len(), 1);
    // a flipped ciphertext byte fails the checksum: not a table, an unreadable file
    let mut bad = enc.clone();
    bad[70] ^= 1;
    assert!(matches!(read_file(&bad, "Monster.SData", Hint::Auto, Encoding::Cp949), Err(shaiya_sdata::ReadError::Unreadable(_))));
    // a readable file with no layout
    let e = read_file(&shaiya_seed::encrypt(b"no table here"), "X.SData", Hint::Auto, Encoding::Cp949).unwrap_err();
    assert!(matches!(e, shaiya_sdata::ReadError::NoFit(_)));
    assert!(e.into_error("X.SData").to_string().contains("No table layout fits this file (13 bytes)"));
}

#[test]
fn csv_escaping_bom_and_line_ends() {
    assert_eq!(csv_text("plain"), "plain");
    assert_eq!(csv_text("a,b"), "\"a,b\"");
    assert_eq!(csv_text("say \"hi\""), "\"say \"\"hi\"\"\"");
    assert_eq!(csv_text("two\nlines"), "\"two\nlines\"");
    for lead in ["=SUM(A1)", "+1", "-1", "@x", "\tx", "\rx"] {
        assert!(csv_text(lead).starts_with('\'') || csv_text(lead).starts_with("\"'"), "{lead:?} is neutralised: {}", csv_text(lead));
    }
    assert_eq!(csv_text("=1+1"), "'=1+1");
    let t = read_ok(&monster(&[b"=cmd", b"A,B", b"-dash"]), Hint::Auto);
    let out = csv(t.primary());
    assert!(out.starts_with('\u{feff}'), "UTF-8 byte-order mark");
    let lines: Vec<&str> = out.trim_start_matches('\u{feff}').split("\r\n").collect();
    assert_eq!(lines.last(), Some(&""), "every line ends with CRLF");
    assert!(!out.replace("\r\n", "").contains('\n'), "no bare line feeds");
    assert!(lines[0].starts_with("id,name,model,level,"), "{}", lines[0]);
    assert!(lines[1].starts_with("0,'=cmd,100,"), "{}", lines[1]);
    assert!(lines[2].starts_with("1,\"A,B\",101,"), "{}", lines[2]);
    assert!(lines[3].starts_with("2,'-dash,102,"), "{}", lines[3]);
    // numbers are never touched: a negative number stays a number
    let mut data = monster(&[b"X"]);
    let at = 4 + 4 + 2; // count, name length, "X\0", then block: model at 0
    data[at..at + 2].copy_from_slice(&(-5i16).to_le_bytes());
    let t = read_ok(&data, Hint::Auto);
    assert!(csv(t.primary()).contains(",X,-5,"), "{}", csv(t.primary()));
}

#[test]
fn json_shape() {
    let t = read_ok(&monster(&[b"Wolf"]), Hint::Auto);
    let j = t.to_json(None);
    assert_eq!(j["layout"], "monster-31");
    assert_eq!(j["table"], "Monster");
    let s = &j["sheets"][0];
    assert_eq!((s["name"].as_str(), s["row_count"].as_u64()), (Some("monsters"), Some(1)));
    assert_eq!(s["columns"][1], serde_json::json!({"name": "name", "kind": "text"}));
    assert_eq!(s["rows"][0][1], "Wolf");
    assert_eq!(s["rows"][0][2], 100);
}

#[test]
fn layout_files_are_checked_when_they_load() {
    use shaiya_sdata::parse_layout;
    let ok = r#"{"format":1,"table":"Monster","variant":"x","file":"f","evidence":{"loader":["us-108"],"fit_only":["cn-ep4"]},"sections":[{"name":"s","loops":[{"count":"u32"}],"record":[{"kind":"fixed","size":3,"fields":[{"offset":0,"name":"a","kind":"u8"},{"offset":1,"name":"b","kind":"u16"}]}]}]}"#;
    assert!(parse_layout(ok).is_ok());
    assert!(parse_layout(&ok.replace("\"size\":3", "\"size\":4")).unwrap_err().contains("add up to 3"));
    assert!(parse_layout(&ok.replace("\"offset\":1", "\"offset\":2")).unwrap_err().contains("offset"));
    assert!(parse_layout(&ok.replace("\"name\":\"b\"", "\"name\":\"a\"")).unwrap_err().contains("used twice"));
    assert!(parse_layout(&ok.replace("\"u16\"", "\"u64\"")).unwrap_err().contains("unknown kind"));
    assert!(parse_layout(&ok.replace("\"format\":1", "\"format\":2")).unwrap_err().contains("format"));
    assert!(parse_layout("not json").is_err());
}

#[test]
fn the_table_hint_names() {
    assert_eq!(Hint::from_name("AUTO"), Some(Hint::Auto));
    assert_eq!(Hint::from_name("npcquest"), Some(Hint::Table(TableKind::NpcQuest)));
    assert_eq!(Hint::from_name("Npc-Quest"), Some(Hint::Table(TableKind::NpcQuest)));
    assert_eq!(Hint::from_name("item"), Some(Hint::Table(TableKind::Item)));
    assert_eq!(Hint::from_name("nope"), None);
    assert_eq!(shaiya_sdata::encoding_by_name("CP949"), Some(Encoding::Cp949));
    assert_eq!(shaiya_sdata::encoding_name(Encoding::Utf8), "utf-8");
}

#[test]
fn implausible_counts_do_not_fit() {
    // 256 banks: the loader keeps one byte of the bank count, so no Item layout accepts it
    let mut v = 256u32.to_le_bytes().to_vec();
    v.extend(std::iter::repeat_n(0u8, 256 * 4));
    let e = read(&v, Hint::Table(TableKind::Item), Encoding::Cp949).unwrap_err();
    assert!(e.tried.iter().all(|(_, _, why)| why.contains("not plausible")), "{:?}", e.tried);
    // a count that cannot fit in the bytes left
    let mut v = 3u32.to_le_bytes().to_vec();
    v.extend_from_slice(&[0u8; 8]);
    let e = read(&v, Hint::Table(TableKind::Monster), Encoding::Cp949).unwrap_err();
    assert!(e.tried[0].2.contains("does not fit"), "{:?}", e.tried);
}

// ------------------------------------------------------------------------------------------------ T9 W5: evidence, FIT-ONLY layouts

const MINIMAL: &str = r#"{"format":1,"table":"Monster","variant":"x","file":"f","evidence":{"loader":["us-108"],"fit_only":["cn-ep4"]},"sections":[{"name":"s","loops":[{"count":"u32"}],"record":[{"kind":"fixed","size":3,"fields":[{"offset":0,"name":"a","kind":"u8"},{"offset":1,"name":"b","kind":"u16"}]}]}]}"#;

#[test]
fn a_layout_without_evidence_is_refused_and_the_evidence_is_read() {
    use shaiya_sdata::{parse_layout, Status};
    let l = parse_layout(MINIMAL).unwrap();
    assert_eq!((l.evidence.loader.as_slice(), l.evidence.fit_only.as_slice()), (&["us-108".to_string()][..], &["cn-ep4".to_string()][..]));
    assert_eq!(l.status(), Status::LoaderVerified);
    let without = MINIMAL.replace(r#""evidence":{"loader":["us-108"],"fit_only":["cn-ep4"]},"#, "");
    assert!(parse_layout(&without).unwrap_err().contains("\"evidence\" is missing"));
    for (from, to, what) in [
        (r#""loader":["us-108"],"#, "", "no \"loader\" list"),
        (r#","fit_only":["cn-ep4"]"#, "", "no \"fit_only\" list"),
        (r#""loader":["us-108"]"#, r#""loader":[7]"#, "not a client id"),
        (r#""fit_only":["cn-ep4"]"#, r#""fit_only":["us-108"]"#, "both loader and fit_only"),
        (r#""loader":["us-108"]"#, r#""loader":["us-108","us-108"]"#, "twice"),
        (r#""loader":["us-108"],"fit_only":["cn-ep4"]"#, r#""loader":[],"fit_only":[]"#, "lists no client"),
        (r#""evidence":{"loader":["us-108"],"fit_only":["cn-ep4"]}"#, r#""evidence":"yes""#, "\"evidence\" is missing"),
    ] {
        assert!(parse_layout(&MINIMAL.replace(from, to)).unwrap_err().contains(what), "{what}");
    }
    // an empty loader list is FIT-ONLY
    let fit_only = parse_layout(&MINIMAL.replace(r#""loader":["us-108"]"#, r#""loader":[]"#)).unwrap();
    assert_eq!(fit_only.status(), Status::FitOnly);
}

#[test]
fn every_embedded_layout_has_evidence_and_the_fit_only_ones_say_so() {
    use shaiya_sdata::{layouts, Status, FIT_ONLY_NOTE};
    let by = |v: &str| layouts().iter().find(|l| l.variant == v).unwrap_or_else(|| panic!("{v}"));
    for l in layouts() {
        assert!(!l.evidence.loader.is_empty() || !l.evidence.fit_only.is_empty(), "{}", l.variant);
        assert_eq!(l.status() == Status::FitOnly, l.evidence.loader.is_empty(), "{}", l.variant);
    }
    for v in ["item-134", "monster-36", "npcquest-287"] {
        let l = by(v);
        assert_eq!((l.status(), l.provenance().as_str()), (Status::FitOnly, FIT_ONLY_NOTE), "{v}");
        assert_eq!(l.evidence.fit_only, ["kr-ep8", "fawkes-v68"], "{v}");
    }
    for v in ["item-81", "item-132", "skill-3", "skill-9", "skill-15", "monster-31", "npcquest-165", "npcquest-87"] {
        assert_eq!(by(v).status(), Status::LoaderVerified, "{v}");
    }
    assert_eq!(by("item-81").evidence.fit_only, ["cn-ep4"]);
    assert_eq!(by("skill-3").evidence.fit_only, ["cn-ep4"]);
    assert_eq!(by("skill-9").evidence.fit_only, ["kr-ep8"], "skill-9 reads the NpcSkill file of kr-ep8");
    assert_eq!(by("npcquest-165").evidence.fit_only, ["cn-ep4"]);
    assert_eq!(by("skill-15").evidence.fit_only, ["kr-ep8", "fawkes-v68"]);
    assert!(by("monster-31").evidence.fit_only.is_empty(), "cn-ep4 Monster has no fit");
    assert_eq!(by("item-81").provenance(), "Loader-verified on: us-108, ru-ep3, ru-ep45, fr-ep54. On cn-ep4 it only fits (FIT-ONLY: no loader read).");
    assert_eq!(by("monster-31").provenance(), "Loader-verified on: us-108, ru-ep3, ru-ep45, fr-ep54, phoenix-ep63.");
    assert!(FIT_ONLY_NOTE.starts_with("FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader."));
}

#[test]
fn the_fit_only_layouts_fit_their_synthetic_table_and_nothing_else() {
    // Item: 134-byte block
    let book = |fixed| item(&[vec![(&b"A"[..], &b"B"[..], 1), (&b"C"[..], &b"D"[..], 2)]], fixed);
    let t = read_ok(&book(134), Hint::Auto);
    assert_eq!((t.layout.variant.as_str(), t.primary().rows.len()), ("item-134", 2));
    assert_eq!(t.primary().columns.iter().map(|c| c.name.as_str()).take(4).collect::<Vec<_>>(), ["bank", "s0", "s1", "f0"], "positional names");
    let mut one_more = book(134);
    one_more.push(0);
    assert!(read(&one_more, Hint::Table(TableKind::Item), Encoding::Cp949).is_err());
    assert!(read(&book(134)[..book(134).len() - 1], Hint::Table(TableKind::Item), Encoding::Cp949).is_err());
    // Monster: 36-byte block (and the 31-byte file stays monster-31)
    let t = read_ok(&monster_of(&[b"Wolf", b"Bear"], 36), Hint::Auto);
    assert_eq!((t.layout.variant.as_str(), t.primary().rows.len()), ("monster-36", 2));
    assert_eq!(read_ok(&monster(&[b"Wolf", b"Bear"]), Hint::Auto).layout.variant, "monster-31");
    let mut more = monster_of(&[b"Wolf"], 36);
    more.push(0);
    assert!(read(&more, Hint::Table(TableKind::Monster), Encoding::Cp949).is_err());
    let one = monster_of(&[b"Wolf"], 36);
    assert!(read(&one[..one.len() - 1], Hint::Table(TableKind::Monster), Encoding::Cp949).is_err());
    // NpcQuest: 287-byte quests, no strings
    let t = read_ok(&npcquest_287(3), Hint::Auto);
    assert_eq!((t.layout.variant.as_str(), t.primary().name.as_str(), t.primary().rows.len()), ("npcquest-287", "quests", 3));
    assert_eq!(t.sheet("quest_links").unwrap().rows.len(), 65536);
    let mut more = npcquest_287(3);
    more.push(0);
    assert!(read(&more, Hint::Table(TableKind::NpcQuest), Encoding::Cp949).is_err());
    let short = npcquest_287(3);
    assert!(read(&short[..short.len() - 1], Hint::Table(TableKind::NpcQuest), Encoding::Cp949).is_err());
    // the verified shapes keep their own layouts
    assert_eq!(read_ok(&npcquest(3), Hint::Auto).layout.variant, "npcquest-165");
    assert_eq!(read_ok(&item(&[vec![(&b"A"[..], &b"B"[..], 1)]], 81), Hint::Auto).layout.variant, "item-81");
}

#[test]
fn json_has_the_status_keys() {
    let t = read_ok(&monster_of(&[b"Wolf"], 36), Hint::Auto);
    let j = t.to_json(None);
    assert_eq!((j["status"].as_str(), j["chosen_by"].as_str()), (Some("fit-only"), Some("fit")));
    assert_eq!(j["loader_verified_on"], serde_json::json!([]));
    assert_eq!(j["fit_only_on"], serde_json::json!(["kr-ep8", "fawkes-v68"]));
    assert_eq!(j["layout"], "monster-36", "the existing keys stay");
    let j = read_ok(&monster(&[b"Wolf"]), Hint::Auto).to_json(None);
    assert_eq!(j["status"], "loader-verified");
    assert_eq!(j["loader_verified_on"], serde_json::json!(["us-108", "ru-ep3", "ru-ep45", "fr-ep54", "phoenix-ep63"]));
    assert_eq!(j["fit_only_on"], serde_json::json!([]));
    let j = read_ok(&item(&[vec![(&b"A"[..], &b"B"[..], 1)]], 81), Hint::Auto).to_json(None);
    assert_eq!(j["fit_only_on"], serde_json::json!(["cn-ep4"]));
}
