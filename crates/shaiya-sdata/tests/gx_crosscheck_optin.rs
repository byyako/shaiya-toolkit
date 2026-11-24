//! Opt-in re-measurements of the Shaiya-GX cross-check on the SData files of a real client (read only; nothing is written).
//! `SHAIYA_CLIENT` is a client folder with `data.sah` and `data.saf` (US EP4.5). Each test prints "skipped: <VAR> not set" and returns
//! when its variable is unset. The numbers are measured on US EP4.5 only.

use shaiya_archive::Archive;
use shaiya_core::text::Encoding;
use shaiya_sdata::{read_file, Hint, Table, TableKind, Value};
use shaiya_testkit::env_client;
use std::collections::BTreeSet;

fn table(a: &Archive, path: &str, kind: TableKind) -> Table {
    let file = a.index().visible().into_iter().find(|f| f.path.eq_ignore_ascii_case(path)).unwrap_or_else(|| panic!("{path} is not in the archive"));
    let raw = a.read_entry(file.entry).unwrap();
    read_file(&raw, path, Hint::Table(kind), Encoding::Cp949).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn col(sheet: &shaiya_sdata::Sheet, name: &str) -> usize {
    sheet.columns.iter().position(|c| c.name == name).unwrap_or_else(|| panic!("no column {name}"))
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        other => panic!("not a whole number: {other:?}"),
    }
}

#[test]
fn npcquest_stock_pairs_resolve_as_two_bytes() {
    let Some(dir) = env_client() else {
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let items = table(&a, "Item/Item.SData", TableKind::Item);
    let items = items.primary();
    let (t, id) = (col(items, "type"), col(items, "type_id"));
    let known: BTreeSet<(i64, i64)> = items.rows.iter().map(|r| (int(&r[t]), int(&r[id]))).collect();
    let quest = table(&a, "Npc/NpcQuest.SData", TableKind::NpcQuest);
    let merchants = quest.sheet("merchants").expect("merchants sheet");
    let stock = col(merchants, "stock");
    let (mut rows, mut pairs, mut empty, mut resolved, mut as_i16) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut max_type, mut max_id) = (0i64, 0i64);
    for r in &merchants.rows {
        rows += 1;
        let Value::Text(s) = &r[stock] else { panic!("stock is not text") };
        for p in s.split(' ').filter(|p| !p.is_empty()) {
            let (x, y) = p.split_once(':').expect("a pair");
            let (x, y): (i64, i64) = (x.parse().unwrap(), y.parse().unwrap());
            if (x, y) == (0, 0) {
                empty += 1;
                continue;
            }
            pairs += 1;
            resolved += known.contains(&(x, y)) as usize;
            max_type = max_type.max(x);
            max_id = max_id.max(y);
            // the same two bytes as one little-endian i16, split by 1000
            let w = x | (y << 8);
            as_i16 += known.contains(&(w / 1000, w % 1000)) as usize;
        }
    }
    println!("merchants {rows}; non-empty pairs {pairs}, empty {empty}; resolve as (u8 type, u8 id) {resolved}; as i16 / 1000 {as_i16}; max type {max_type}, max id {max_id}; item rows {}", items.rows.len());
    assert_eq!(rows, 252, "merchants");
    assert_eq!((pairs, empty), (2703, 705));
    assert_eq!(resolved, 2703, "every non-empty pair is an item as (type, type id)");
    assert_eq!(as_i16, 652, "the i16 reading resolves far fewer");
    assert_eq!((max_type, max_id), (100, 238));
    assert_eq!(items.rows.len(), 16_045);
}

#[test]
fn npcskill_offset_4_is_a_u16_above_255_in_4_rows() {
    let Some(dir) = env_client() else {
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let t = table(&a, "Monster/NpcSkill.SData", TableKind::Skill);
    let s = t.primary();
    let f4 = col(s, "f4");
    let values: Vec<i64> = s.rows.iter().map(|r| int(&r[f4])).collect();
    let above: Vec<i64> = values.iter().copied().filter(|v| *v > 255).collect();
    println!("NpcSkill rows {}; f4 non-zero {}; max {:?}; above 255 {above:?}", values.len(), values.iter().filter(|v| **v != 0).count(), values.iter().max());
    assert_eq!(values.len(), 1044);
    assert_eq!(values.iter().max().copied(), Some(324));
    assert_eq!(above.len(), 4);
    assert!(above.iter().all(|v| (321..=324).contains(v)), "{above:?}");
}
