//! Line-preserving reader for the server's `Map.ini` and `MapWar.ini`.
//!
//! The files are cp949 text with CRLF and Korean comments. The model keeps every line as raw bytes with its own ending, so
//! `to_bytes()` gives the input back unchanged. Parsing skips the `/* ... */` block at the top (it holds a sample `MapCount = 9`)
//! and `//` comments. Sections and `key = value` pairs point at their line, and each value keeps its byte range inside that line,
//! so a later edit can replace a value without touching any other byte.

use std::ops::Range;

/// One line of the file: the text without its ending, and the ending (`\r\n`, `\n` or empty for a last line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawLine {
    pub text: Vec<u8>,
    pub eol: Vec<u8>,
}

/// `name = value` inside a section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub name: String,
    pub line: usize,
    /// Byte range of the value inside `lines[line].text`: after the `=` and its blanks, before a `//` comment, trailing blanks off.
    pub value: Range<usize>,
}

/// `[NAME]` and the keys under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// Without the brackets, as written (`BASE`, `DEFAULT_SET_ZONE`, `SET_ZONE_5`).
    pub name: String,
    /// The line of the `[NAME]` head (the first line of the file for keys that come before any head).
    pub line: usize,
    pub keys: Vec<Key>,
}

/// A rebirth position of a zone: `RebirthMapPos`, `RebirthMapPos1` or `RebirthMapPos2`.
#[derive(Clone, Debug, PartialEq)]
pub struct Rebirth {
    pub key: &'static str,
    pub map: i64,
    pub pos: [f32; 3],
}

pub const REBIRTH_KEYS: [&str; 3] = ["RebirthMapPos", "RebirthMapPos1", "RebirthMapPos2"];

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MapIni {
    pub lines: Vec<RawLine>,
    pub sections: Vec<Section>,
}

fn is_blank(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r')
}

fn trim(s: &[u8]) -> &[u8] {
    let a = s.iter().position(|b| !is_blank(*b)).unwrap_or(s.len());
    let z = s.iter().rposition(|b| !is_blank(*b)).map_or(a, |p| p + 1);
    &s[a..z.max(a)]
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl MapIni {
    pub fn parse(bytes: &[u8]) -> MapIni {
        let mut lines = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let end = bytes[at..].iter().position(|b| *b == b'\n').map(|p| at + p + 1).unwrap_or(bytes.len());
            let mut text_end = if bytes[end - 1] == b'\n' { end - 1 } else { end };
            let has_nl = text_end != end;
            if has_nl && text_end > at && bytes[text_end - 1] == b'\r' {
                text_end -= 1;
            }
            lines.push(RawLine { text: bytes[at..text_end].to_vec(), eol: bytes[text_end..end].to_vec() });
            at = end;
        }
        MapIni::from_lines(lines)
    }

    /// Builds the section and key index over `lines`.
    fn from_lines(lines: Vec<RawLine>) -> MapIni {
        let mut sections: Vec<Section> = Vec::new();
        let mut in_block = false;
        for (n, l) in lines.iter().enumerate() {
            let text = &l.text;
            if in_block {
                if find(text, b"*/").is_some() {
                    in_block = false;
                }
                continue;
            }
            // a UTF-8 byte order mark before the first line is read past (it stays in the bytes)
            let base = if n == 0 && text.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
            let lead = base + text[base..].iter().position(|b| !is_blank(*b)).unwrap_or(text.len() - base);
            let t = &text[lead..];
            if t.is_empty() || t.starts_with(b"//") {
                continue;
            }
            if t.starts_with(b"/*") {
                if find(&t[2..], b"*/").is_none() {
                    in_block = true;
                }
                continue;
            }
            if t[0] == b'[' {
                if let Some(close) = t.iter().position(|b| *b == b']') {
                    let name = String::from_utf8_lossy(trim(&t[1..close])).into_owned();
                    sections.push(Section { name, line: n, keys: Vec::new() });
                }
                continue;
            }
            let Some(eq) = text[lead..].iter().position(|b| *b == b'=').map(|p| lead + p) else { continue };
            let name = String::from_utf8_lossy(trim(&text[lead..eq])).into_owned();
            if name.is_empty() {
                continue;
            }
            let vstart = eq + 1 + text[eq + 1..].iter().position(|b| !is_blank(*b)).unwrap_or(text.len() - eq - 1);
            let vstop = find(&text[vstart..], b"//").map_or(text.len(), |p| vstart + p);
            let vend = vstart + trim(&text[vstart..vstop]).len();
            if sections.is_empty() {
                sections.push(Section { name: String::new(), line: 0, keys: Vec::new() });
            }
            if let Some(s) = sections.last_mut() {
                s.keys.push(Key { name, line: n, value: vstart..vend });
            }
        }
        MapIni { lines, sections }
    }

    /// The file's bytes again, every line with its own ending (edited files are written with it).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(self.lines.iter().map(|l| l.text.len() + l.eol.len()).sum());
        for l in &self.lines {
            o.extend_from_slice(&l.text);
            o.extend_from_slice(&l.eol);
        }
        o
    }

    /// The first section called `name`, without regard to case.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// The value of `key` in a section (the last one wins when it is written twice), as text.
    pub fn value(&self, s: &Section, key: &str) -> Option<String> {
        let k = s.keys.iter().rev().find(|k| k.name.eq_ignore_ascii_case(key))?;
        Some(String::from_utf8_lossy(&self.lines[k.line].text[k.value.clone()]).into_owned())
    }

    /// `[BASE] MapCount`. The `/* ... */` sample at the top is not read.
    pub fn map_count(&self) -> Option<u32> {
        let base = self.section("BASE")?;
        number(&self.value(base, "MapCount")?).and_then(|n| u32::try_from(n as i64).ok())
    }

    /// The ids of the `[SET_ZONE_n]` sections, ascending.
    pub fn zone_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.sections.iter().filter_map(|s| zone_id(&s.name)).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn zone(&self, id: u32) -> Option<&Section> {
        self.sections.iter().find(|s| zone_id(&s.name) == Some(id))
    }

    /// The zone's value of `key`, else `[DEFAULT_SET_ZONE]`'s (inferred to be the fallback).
    pub fn zone_value(&self, id: u32, key: &str) -> Option<String> {
        self.zone(id).and_then(|z| self.value(z, key)).or_else(|| self.section("DEFAULT_SET_ZONE").and_then(|d| self.value(d, key)))
    }

    /// The rebirth positions a zone sets. `map = -1` (ignored by the server) and `0, 0, 0` are left out.
    pub fn rebirths(&self, id: u32) -> Vec<Rebirth> {
        let mut out = Vec::new();
        for key in REBIRTH_KEYS {
            let Some(text) = self.zone_value(id, key) else { continue };
            let nums = numbers(&text, 4);
            let [map, x, y, z] = nums.as_slice() else { continue };
            if *map < 0.0 || (*x == 0.0 && *y == 0.0 && *z == 0.0) {
                continue;
            }
            out.push(Rebirth { key, map: *map as i64, pos: [*x as f32, *y as f32, *z as f32] });
        }
        out
    }
}

// ------------------------------------------------------------------------------------------------------------------ editing

/// The first map id of the war server: zone `n` of `MapWar.ini` is map `n`, and the war maps start at 100.
pub const WAR_FIRST: u32 = 100;

/// One change of the file as a splice of whole lines: the lines `at..at + removed.len()` are replaced by `inserted`. It carries
/// what it removed, so its [`inverse`](LineEdit::inverse) undoes it exactly. Every line outside the splice keeps its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineEdit {
    pub at: usize,
    pub removed: Vec<RawLine>,
    pub inserted: Vec<RawLine>,
}

impl LineEdit {
    pub fn inverse(&self) -> LineEdit {
        LineEdit { at: self.at, removed: self.inserted.clone(), inserted: self.removed.clone() }
    }
}

/// `Err` says why a value cannot be written: it stays on one line, in the file's code page, and does not start a comment.
pub fn check_value(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("The value is empty. Use the remove button to drop the key.".to_string());
    }
    if text.contains(['\r', '\n']) {
        return Err("A value stays on one line.".to_string());
    }
    if !text.is_ascii() {
        return Err("Only plain ASCII text is written. The file is cp949 and the server reads these values as numbers or letters.".to_string());
    }
    if text.contains("//") || text.contains("/*") {
        return Err("A value cannot hold // or /*. The server would read the rest as a comment.".to_string());
    }
    Ok(())
}

fn check_key(name: &str) -> Result<(), String> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("{name:?} is not a key name (letters, digits and _)."));
    }
    Ok(())
}

impl MapIni {
    /// Applies one edit. An edit made for another file state (a line range past the end) is ignored.
    pub fn apply(&mut self, e: &LineEdit) {
        if e.at + e.removed.len() > self.lines.len() {
            return;
        }
        self.lines.splice(e.at..e.at + e.removed.len(), e.inserted.iter().cloned());
        let lines = std::mem::take(&mut self.lines);
        *self = MapIni::from_lines(lines);
    }

    /// The line ending the file uses: the first ending that is not empty, else CRLF.
    pub fn eol(&self) -> Vec<u8> {
        self.lines.iter().map(|l| l.eol.as_slice()).find(|e| !e.is_empty()).map_or_else(|| b"\r\n".to_vec(), <[u8]>::to_vec)
    }

    fn section_at(&self, name: &str) -> Result<&Section, String> {
        self.section(name).ok_or_else(|| format!("[{name}] is not in the file"))
    }

    /// The edits that make `key` of the section `name` read `value`. When the key is there its value's own bytes change and the
    /// rest of the line (a `//` comment included) stays. Else a new line follows the section's last key.
    pub fn set_value_edits(&self, name: &str, key: &str, value: &str) -> Result<Vec<LineEdit>, String> {
        check_key(key)?;
        check_value(value)?;
        let s = self.section_at(name)?;
        let value = value.trim();
        if let Some(k) = s.keys.iter().rev().find(|k| k.name.eq_ignore_ascii_case(key)) {
            let old = &self.lines[k.line];
            if old.text[k.value.clone()] == *value.as_bytes() {
                return Ok(Vec::new());
            }
            let mut text = old.text[..k.value.start].to_vec();
            text.extend_from_slice(value.as_bytes());
            text.extend_from_slice(&old.text[k.value.end..]);
            return Ok(vec![LineEdit { at: k.line, removed: vec![old.clone()], inserted: vec![RawLine { text, eol: old.eol.clone() }] }]);
        }
        let after = s.keys.iter().map(|k| k.line).max().unwrap_or(s.line);
        Ok(vec![self.insert_after(after, format!("{key}\t = {value}").into_bytes())])
    }

    /// The edit that drops the key's line (the last one when the key is written twice). `None` when the section has no such key.
    pub fn remove_key_edit(&self, name: &str, key: &str) -> Option<LineEdit> {
        let s = self.section(name)?;
        let k = s.keys.iter().rev().find(|k| k.name.eq_ignore_ascii_case(key))?;
        let old = &self.lines[k.line];
        // the last line of a file has no ending: the line before it must not keep one
        if old.eol.is_empty() && k.line > 0 {
            let prev = &self.lines[k.line - 1];
            return Some(LineEdit { at: k.line - 1, removed: vec![prev.clone(), old.clone()], inserted: vec![RawLine { text: prev.text.clone(), eol: Vec::new() }] });
        }
        Some(LineEdit { at: k.line, removed: vec![old.clone()], inserted: Vec::new() })
    }

    /// A new line after line `after`. When that is the last line and has no ending it gets one, and the new line has none.
    fn insert_after(&self, after: usize, text: Vec<u8>) -> LineEdit {
        let old = &self.lines[after];
        if old.eol.is_empty() {
            let first = RawLine { text: old.text.clone(), eol: self.eol() };
            return LineEdit { at: after, removed: vec![old.clone()], inserted: vec![first, RawLine { text, eol: Vec::new() }] };
        }
        LineEdit { at: after + 1, removed: Vec::new(), inserted: vec![RawLine { text, eol: self.eol() }] }
    }

    /// The `MapCount` the file needs: for `Map.ini` the highest zone id + 1; for `MapWar.ini` (`war`) the number of war maps, the
    /// highest id - 99, because the war server counts its maps from 100.
    pub fn needed_map_count(&self, war: bool) -> u32 {
        let Some(top) = self.zone_ids().last().copied() else { return 0 };
        if war {
            top.saturating_sub(WAR_FIRST - 1)
        } else {
            top + 1
        }
    }

    /// The edits that append `[SET_ZONE_<id>]` with `keys` at the end of the file (a blank line before it when the file does not
    /// end with one) and raise `[BASE] MapCount` to what the file needs then. An id that is already there is refused.
    pub fn add_zone_edits(&self, id: u32, keys: &[(&str, &str)], war: bool) -> Result<Vec<LineEdit>, String> {
        if self.zone(id).is_some() {
            return Err(format!("[SET_ZONE_{id}] is already in the file."));
        }
        for (k, v) in keys {
            check_key(k)?;
            check_value(v)?;
        }
        let eol = self.eol();
        let mut inserted: Vec<RawLine> = Vec::new();
        let mut removed: Vec<RawLine> = Vec::new();
        let mut at = self.lines.len();
        if let Some(last) = self.lines.last() {
            if last.eol.is_empty() {
                at -= 1;
                removed.push(last.clone());
                inserted.push(RawLine { text: last.text.clone(), eol: eol.clone() });
            }
            if !trim(&last.text).is_empty() {
                inserted.push(RawLine { text: Vec::new(), eol: eol.clone() });
            }
        }
        inserted.push(RawLine { text: format!("[SET_ZONE_{id}]").into_bytes(), eol: eol.clone() });
        for (k, v) in keys {
            inserted.push(RawLine { text: format!("{k}\t = {}", v.trim()).into_bytes(), eol: eol.clone() });
        }
        let mut out = vec![LineEdit { at, removed, inserted }];
        let mut next = self.clone();
        next.apply(&out[0]);
        let need = next.needed_map_count(war);
        if need > next.map_count().unwrap_or(0) {
            out.extend(next.set_value_edits("BASE", "MapCount", &need.to_string())?);
        }
        Ok(out)
    }
}

fn zone_id(section: &str) -> Option<u32> {
    let s = section.trim();
    let head = s.get(..9)?;
    if !head.eq_ignore_ascii_case("SET_ZONE_") {
        return None;
    }
    s[9..].trim().parse().ok()
}

/// The first number of a value (`68`, `-1`, `1.5`).
pub fn number(text: &str) -> Option<f64> {
    numbers(text, 1).first().copied()
}

/// The first `n` numbers of a value, read through commas, brackets and blanks; stops at the first word that is no number.
pub fn numbers(text: &str, n: usize) -> Vec<f64> {
    let mut out = Vec::new();
    for tok in text.split(|c: char| c == ',' || c == '(' || c == ')' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        match tok.parse::<f64>() {
            Ok(v) if v.is_finite() => out.push(v),
            _ => break,
        }
        if out.len() == n {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"//////\r\n/*\r\nMapCount\t  = 9   // sample\r\nHUGameStartMapPos = -1, 1.0, 1.0, 1.0\r\n*/\r\n\r\n[BASE]\r\nMapCount\t  = 68\r\n\r\n[DEFAULT_SET_ZONE]\r\nMapType = F\r\nRebirthMapPos \t  = -1, 0, 0, 0\r\n\r\n[SET_ZONE_5]\r\nMapType\t\t = D\t//comment\r\nRebirthMapPos = 5, 10.5, 3, 20\r\nRebirthMapPos1 = 5, 0, 0, 0\t(note)\r\n[set_zone_7]\nMapName=-1";

    #[test]
    fn parse_skips_the_comment_block_and_gives_the_bytes_back() {
        let m = MapIni::parse(SAMPLE);
        assert_eq!(m.to_bytes(), SAMPLE);
        assert_eq!(m.map_count(), Some(68), "the 9 in the comment block is not read");
        assert_eq!(m.zone_ids(), vec![5, 7]);
        let z = m.zone(5).unwrap();
        assert_eq!(m.value(z, "MapType").as_deref(), Some("D"), "a // comment ends the value");
        assert_eq!(m.value(m.zone(7).unwrap(), "MapName").as_deref(), Some("-1"), "no blanks around =, no ending on the last line");
        let r = m.rebirths(5);
        assert_eq!(r, vec![Rebirth { key: "RebirthMapPos", map: 5, pos: [10.5, 3.0, 20.0] }], "0,0,0 is ignored and -1 as the default is ignored");
        assert_eq!(m.zone_value(7, "MapType").as_deref(), Some("F"), "a zone without the key falls back to the default");
    }

    #[test]
    fn value_ranges_point_at_the_value_bytes() {
        let m = MapIni::parse(SAMPLE);
        let k = m.zone(5).unwrap().keys.iter().find(|k| k.name == "MapType").unwrap();
        assert_eq!(&m.lines[k.line].text[k.value.clone()], b"D");
        let k = m.section("BASE").unwrap().keys.iter().find(|k| k.name == "MapCount").unwrap();
        assert_eq!(&m.lines[k.line].text[k.value.clone()], b"68");
    }

    #[test]
    fn empty_and_odd_files() {
        assert_eq!(MapIni::parse(b"").to_bytes(), b"");
        for raw in [&b"\n"[..], b"\r\n\r\n", b"[a", b"x", b"/* open", b"k = v\r"] {
            assert_eq!(MapIni::parse(raw).to_bytes(), raw, "{raw:?}");
        }
        assert_eq!(numbers("(1,577.98, 78.56, 1762.12)", 4), vec![1.0, 577.98, 78.56, 1762.12]);
        assert_eq!(numbers("-1, 0, 0, 0\t(word 1 2)", 4), vec![-1.0, 0.0, 0.0, 0.0]);
        assert_eq!(number("abc"), None);
    }

    /// A byte order mark, lone and doubled carriage returns, odd spacing and a block comment that ends mid-line: the bytes come
    /// back unchanged and the keys are still found.
    #[test]
    fn bom_endings_and_spacing() {
        let raw: &[u8] = b"\xEF\xBB\xBF[BASE]\r\n  MapCount\t=\t 3 \t\r\r\n/* a */\n/*\r\nMapCount = 9\r\n  */ x\r\n[SET_ZONE_2]\n\rMapType=D\n\nkey without value =\r\n=5\r\n[SET_ZONE_x]\r\nMapType = F";
        let m = MapIni::parse(raw);
        assert_eq!(m.to_bytes(), raw);
        assert_eq!(m.map_count(), Some(3), "the BOM does not hide [BASE]; the 9 inside the block is not read");
        assert_eq!(m.zone_ids(), vec![2], "SET_ZONE_x is no zone id");
        assert_eq!(m.zone_value(2, "MapType").as_deref(), Some("D"), "a lone CR before the key is a blank");
        assert_eq!(m.value(m.zone(2).unwrap(), "key without value").as_deref(), Some(""));
    }

    /// Random bytes never panic and always give their bytes back.
    #[test]
    fn garbage_roundtrips() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let alphabet = b"[]=/*\r\n \t,()-.0123456789aZ_\xEF\xBB\xBF\xB0\x00";
        for len in 0..400 {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    if x.is_multiple_of(4) { (x >> 8) as u8 } else { alphabet[(x >> 8) as usize % alphabet.len()] }
                })
                .collect();
            let m = MapIni::parse(&bytes);
            assert_eq!(m.to_bytes(), bytes);
            let _ = (m.map_count(), m.zone_ids().iter().map(|z| m.rebirths(*z).len()).sum::<usize>());
            let _ = crate::server_data::parse_obelisks(&bytes);
        }
    }

    /// Edits on random files (some with `[BASE]` and zones, some broken) never panic, change only the lines they splice, and their
    /// inverse gives the bytes back.
    #[test]
    fn garbage_edits_are_exact() {
        let mut x: u64 = 0xD1B5_4A32_D192_ED03;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let pieces: [&[u8]; 14] = [b"[BASE]", b"MapCount = 3", b"MapCount=", b"[SET_ZONE_1]", b"[SET_ZONE_7]", b"MapType = F // c", b"/*", b"*/", b"", b"[DEFAULT_SET_ZONE]", b"x = \xB0\xA1", b"MapType", b"\xEF\xBB\xBF[BASE]", b"= ="];
        let eols: [&[u8]; 4] = [b"\r\n", b"\n", b"\r", b""];
        for _ in 0..600 {
            let mut bytes = Vec::new();
            for _ in 0..(next() % 12) {
                bytes.extend_from_slice(pieces[(next() % pieces.len() as u64) as usize]);
                bytes.extend_from_slice(eols[(next() % 3) as usize]);
            }
            if next() % 3 == 0 {
                bytes.extend_from_slice(pieces[(next() % pieces.len() as u64) as usize]);
            }
            let m = MapIni::parse(&bytes);
            let war = next() % 2 == 0;
            let mut tries: Vec<Vec<LineEdit>> = Vec::new();
            for s in ["BASE", "SET_ZONE_1", "SET_ZONE_7", "NOPE"] {
                tries.extend(m.set_value_edits(s, "MapType", "D").ok());
                tries.extend(m.set_value_edits(s, "MapCount", "12").ok());
                tries.extend(m.remove_key_edit(s, "MapType").map(|e| vec![e]));
            }
            tries.extend(m.add_zone_edits((next() % 300) as u32, &[("MapType", "F")], war).ok());
            let _ = m.set_value_edits("BASE", "Map Count", "1").map(|_| panic!("a bad key name is refused"));
            let _ = m.set_value_edits("BASE", "MapCount", "1 // x").map(|_| panic!("a comment in a value is refused"));
            for edits in tries {
                let mut e2 = m.clone();
                for e in &edits {
                    e2.apply(e);
                }
                if let [one] = edits.as_slice() {
                    // lines before and after the splice keep their bytes
                    assert_eq!(e2.lines[..one.at], m.lines[..one.at]);
                    assert_eq!(e2.lines[one.at + one.inserted.len()..], m.lines[one.at + one.removed.len()..]);
                }
                for e in edits.iter().rev() {
                    e2.apply(&e.inverse());
                }
                assert_eq!(e2.to_bytes(), bytes, "undo gives the bytes back");
            }
        }
    }

    fn server_dir() -> Option<std::path::PathBuf> {
        let maps = shaiya_testkit::env_var_path("SVMAP_ORIGINALS")?;
        let dir = maps.parent()?.to_path_buf();
        dir.join("Map.ini").is_file().then_some(dir)
    }

    /// `MapCount` is the 68 of `[BASE]`, not the 9 of the sample in the comment block; 68 zones; `MapWar.ini` has 9 zones, 100 to 108.
    #[test]
    fn mapini_reads_originals() {
        let Some(dir) = server_dir() else { return };
        let m = MapIni::parse(&std::fs::read(dir.join("Map.ini")).unwrap());
        assert_eq!(m.map_count(), Some(68));
        assert_eq!(m.zone_ids(), (0..68).collect::<Vec<u32>>());
        assert_eq!(m.value(m.section("BASE").unwrap(), "HUGameStartMapPos").as_deref(), Some("(1,577.98, 78.56, 1762.12)"));
        assert_eq!(m.zone_value(0, "MapType").as_deref(), Some("F"));
        let w = MapIni::parse(&std::fs::read(dir.join("MapWar.ini")).unwrap());
        assert_eq!(w.map_count(), Some(9));
        assert_eq!(w.zone_ids(), (100..=108).collect::<Vec<u32>>());
        assert_eq!(w.rebirths(100), vec![Rebirth { key: "RebirthMapPos", map: 100, pos: [50.43, 7.09, 86.57] }]);
        let loaded = crate::server_data::ServerData::load(&dir, None);
        assert_eq!(loaded.loaded_ids().unwrap().len(), 77, "68 maps and 9 war maps");
    }

    /// Both files give their own bytes back from the model.
    #[test]
    fn mapini_roundtrip_bytes() {
        let Some(dir) = server_dir() else { return };
        for name in ["Map.ini", "MapWar.ini"] {
            let bytes = std::fs::read(dir.join(name)).unwrap();
            let m = MapIni::parse(&bytes);
            assert!(m.lines.len() > 50, "{name}");
            assert_eq!(m.to_bytes(), bytes, "{name}");
        }
    }
}
