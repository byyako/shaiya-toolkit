//! Byte builders for the model list files (`.MLT`, `.MON`, `.ITM`), written from the documented layouts without using
//! `shaiya-clientdata`: an independent writer, so a reader/writer round trip is tested against bytes it did not make. Synthetic data only.

fn i32le(d: &mut Vec<u8>, v: i32) {
    d.extend_from_slice(&v.to_le_bytes());
}

/// `i32 length` + the bytes; `nul` appends a NUL that the length counts (the `.MLT` form).
fn string(d: &mut Vec<u8>, s: &str, nul: bool) {
    i32le(d, (s.len() + nul as usize) as i32);
    d.extend_from_slice(s.as_bytes());
    if nul {
        d.push(0);
    }
}

/// An `.MLT`: `"ML2"`, `i32 count` and the `.3DC` names, the same for the `.DDS` names (every string's length counts its NUL), then
/// `i32 count` and `{i32 mesh, i32 texture, i32 alpha_flag}` per record.
pub fn mlt_bytes(meshes: &[&str], textures: &[&str], records: &[(i32, i32, i32)]) -> Vec<u8> {
    let mut d = b"ML2".to_vec();
    for list in [meshes, textures] {
        i32le(&mut d, list.len() as i32);
        for s in list {
            string(&mut d, s, true);
        }
    }
    i32le(&mut d, records.len() as i32);
    for r in records {
        for v in [r.0, r.1, r.2] {
            i32le(&mut d, v);
        }
    }
    d
}

/// One `.MON` record: strings carry no NUL.
pub struct MonRec<'a> {
    pub name: &'a str,
    pub flag: u8,
    pub slots: [&'a str; 17],
    pub parts: &'a [(&'a str, &'a str)],
    pub size: f32,
    pub attachments: &'a [(i32, i32)],
}

/// A `.MON`: `"MO2"`, `i32 record count`, then per record `str name; u8 flag; str slot[17]; i32 parts, {str mesh, str texture}; f32 size;
/// i32 attachments, {i32 a, i32 b}`.
pub fn mon_bytes(records: &[MonRec]) -> Vec<u8> {
    let mut d = b"MO2".to_vec();
    i32le(&mut d, records.len() as i32);
    for r in records {
        string(&mut d, r.name, false);
        d.push(r.flag);
        for s in r.slots {
            string(&mut d, s, false);
        }
        i32le(&mut d, r.parts.len() as i32);
        for (m, t) in r.parts {
            string(&mut d, m, false);
            string(&mut d, t, false);
        }
        d.extend_from_slice(&r.size.to_bits().to_le_bytes());
        i32le(&mut d, r.attachments.len() as i32);
        for (a, b) in r.attachments {
            i32le(&mut d, *a);
            i32le(&mut d, *b);
        }
    }
    d
}

/// One `.ITM` record: `words` are model, texture, glow, unk1, format, unk2; `extra` (four words) is written after them, and the file
/// reads it only when `format` (`words[4]`) is 1.
pub struct ItmRec {
    pub words: [i32; 6],
    pub extra: Option<[i32; 4]>,
}

/// An `.ITM`: `"ITM"`, the `.3DO` names, the `.DDS` names (no NUL), `i32 record count`, then the records.
pub fn itm_bytes(models: &[&str], textures: &[&str], records: &[ItmRec]) -> Vec<u8> {
    let mut d = b"ITM".to_vec();
    for list in [models, textures] {
        i32le(&mut d, list.len() as i32);
        for s in list {
            string(&mut d, s, false);
        }
    }
    i32le(&mut d, records.len() as i32);
    for r in records {
        for v in r.words {
            i32le(&mut d, v);
        }
        for v in r.extra.iter().flatten() {
            i32le(&mut d, *v);
        }
    }
    d
}
