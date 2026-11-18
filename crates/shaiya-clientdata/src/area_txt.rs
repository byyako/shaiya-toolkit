//! Line-preserving `world/<id>.txt`: UTF-16LE with a BOM, one `index<TAB>name`
//! per line. The file is kept as its lines with their own endings, so a file that nothing touched writes back byte for byte
//! (BOM or not, CRLF or LF, a missing final newline, an odd trailing byte, lines this reader does not understand).
//! [`AreaTxt::set_name`] changes or inserts one line; every other line stays as it was, apart from one case named there.

use crate::text::AreaNames;

const BOM: [u8; 2] = [0xff, 0xfe];
const LF: u16 = 0x000a;
const CR: u16 = 0x000d;
const TAB: u16 = 0x0009;

#[derive(Clone, Debug, PartialEq)]
struct Line {
    /// The line without its ending, as UTF-16 units.
    text: Vec<u16>,
    /// `[]` (last line, no newline), `[LF]` or `[CR, LF]`.
    ending: Vec<u16>,
}

impl Line {
    /// The area number and the position of the first unit of the name, when the line is `index<TAB>name`.
    fn parts(&self) -> Option<(u32, usize)> {
        let tab = self.text.iter().position(|&u| u == TAB)?;
        let index = String::from_utf16_lossy(&self.text[..tab]);
        Some((index.trim().parse().ok()?, tab + 1))
    }

    fn name(&self) -> Option<(u32, String)> {
        let (i, at) = self.parts()?;
        let name = String::from_utf16_lossy(&self.text[at..]);
        Some((i, name.trim_end_matches('\r').to_string()))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AreaTxt {
    bom: bool,
    lines: Vec<Line>,
    /// A trailing byte that does not make a UTF-16 unit; kept so the file writes back as it was.
    odd_tail: Option<u8>,
}

impl AreaTxt {
    /// Reads the file. Any bytes are accepted; what is not understood stays as it is.
    pub fn parse(data: &[u8]) -> AreaTxt {
        let (bom, body) = match data.strip_prefix(&BOM[..]) {
            Some(rest) => (true, rest),
            None => (false, data),
        };
        let odd_tail = (body.len() % 2 == 1).then(|| body[body.len() - 1]);
        let even = &body[..body.len() - body.len() % 2];
        let units: Vec<u16> = even.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let mut lines = Vec::new();
        let mut start = 0;
        for (i, &u) in units.iter().enumerate() {
            if u == LF {
                let cr = i > start && units[i - 1] == CR;
                let end = if cr { i - 1 } else { i };
                lines.push(Line { text: units[start..end].to_vec(), ending: units[end..=i].to_vec() });
                start = i + 1;
            }
        }
        if start < units.len() {
            lines.push(Line { text: units[start..].to_vec(), ending: Vec::new() });
        }
        AreaTxt { bom, lines, odd_tail }
    }

    /// The file's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.bom {
            out.extend_from_slice(&BOM);
        }
        for l in &self.lines {
            for u in l.text.iter().chain(&l.ending) {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        out.extend(self.odd_tail);
        out
    }

    /// The names as the client's reader sees them (`text::parse_area_names`).
    pub fn names(&self) -> AreaNames {
        AreaNames { entries: self.lines.iter().filter_map(Line::name).collect() }
    }

    /// The line ending the file uses (the first one it has; CRLF when it has none).
    fn eol(&self) -> Vec<u16> {
        self.lines.iter().find(|l| !l.ending.is_empty()).map_or_else(|| vec![CR, LF], |l| l.ending.clone())
    }

    /// Sets the name of area `index`. A line `index<TAB>...` that exists keeps its index text and ending and gets the new name;
    /// otherwise a line is inserted before the first line with a greater index (at the end when there is none). The one
    /// untouched line that can change: a last line without a newline gets one when a line is appended after it.
    /// A name with a tab, CR or LF is refused, and so is any change to a file that does not look like UTF-16 text (an odd
    /// byte count, or no BOM and no `index<TAB>name` line), so UTF-16 is never mixed into a file in another encoding. An
    /// empty file gets the BOM the client's files have.
    pub fn set_name(&mut self, index: u32, text: &str) -> Result<(), String> {
        if text.contains(['\t', '\r', '\n']) {
            return Err("an area name cannot contain a tab or a line break".to_string());
        }
        if self.odd_tail.is_some() || (!self.bom && !self.lines.is_empty() && self.names().is_empty()) {
            return Err("the file is not UTF-16 area text, so the editor does not change it".to_string());
        }
        if self.lines.is_empty() {
            self.bom = true;
        }
        let units: Vec<u16> = text.encode_utf16().collect();
        if let Some(l) = self.lines.iter_mut().find(|l| l.parts().is_some_and(|(i, _)| i == index)) {
            let (_, at) = l.parts().expect("checked");
            l.text.truncate(at);
            l.text.extend(units);
            return Ok(());
        }
        let eol = self.eol();
        let mut text_units: Vec<u16> = index.to_string().encode_utf16().collect();
        text_units.push(TAB);
        text_units.extend(units);
        let before = self.lines.iter().position(|l| l.parts().is_some_and(|(i, _)| i > index));
        match before {
            Some(at) => self.lines.insert(at, Line { text: text_units, ending: eol }),
            None => {
                let mut ending = eol.clone();
                if let Some(last) = self.lines.last_mut() {
                    if last.ending.is_empty() {
                        last.ending = eol;
                        ending = Vec::new();
                    }
                }
                self.lines.push(Line { text: text_units, ending });
            }
        }
        Ok(())
    }

    /// How many lines the file has.
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::parse_area_names;

    fn utf16(s: &str, bom: bool) -> Vec<u8> {
        let mut v = if bom { BOM.to_vec() } else { Vec::new() };
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn untouched_files_write_back_identically() {
        let cases: Vec<Vec<u8>> = vec![
            utf16("0\tOne\r\n1\tTwo\r\n", true),
            utf16("0\tOne\n1\tTwo\n", true),
            utf16("0\tOne\r\n1\tTwo", true),
            utf16("0\tOne\r\n\r\nnot a line\r\n5\tFive\r\n", false),
            utf16("", true),
            utf16("", false),
            utf16("a\r\r\nb\r", true),
            vec![0xff, 0xfe, 0x30, 0x00, 0x09],
            vec![1, 2, 3],
        ];
        for c in cases {
            assert_eq!(AreaTxt::parse(&c).to_bytes(), c, "{c:?}");
        }
        // a pseudo-random byte soup
        let mut x = 12345u32;
        for len in 0..200 {
            let b: Vec<u8> = (0..len)
                .map(|_| {
                    x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                    [0x0a, 0x0d, 0x09, 0x00, (x >> 24) as u8][(x >> 8) as usize % 5]
                })
                .collect();
            assert_eq!(AreaTxt::parse(&b).to_bytes(), b, "len {len}");
        }
    }

    #[test]
    fn names_match_the_clients_reader() {
        let b = utf16("0\tMeadow\r\n1\tForest\r\n\r\n67\t\r\n88\tLast", true);
        assert_eq!(AreaTxt::parse(&b).names(), parse_area_names(&b, "t.txt").unwrap());
        assert_eq!(AreaTxt::parse(&b).names().get(1), Some("Forest"));
    }

    #[test]
    fn set_name_changes_one_line_and_inserts_in_order() {
        let b = utf16("0\tOne\r\n2\tTwo\r\n5\tFive\r\n", true);
        let mut t = AreaTxt::parse(&b);
        t.set_name(2, "Zwei").unwrap();
        assert_eq!(t.to_bytes(), utf16("0\tOne\r\n2\tZwei\r\n5\tFive\r\n", true), "only line 2 changed");
        t.set_name(3, "Three").unwrap();
        assert_eq!(t.to_bytes(), utf16("0\tOne\r\n2\tZwei\r\n3\tThree\r\n5\tFive\r\n", true), "inserted in index order");
        t.set_name(9, "Nine").unwrap();
        t.set_name(0, "").unwrap();
        assert_eq!(t.to_bytes(), utf16("0\t\r\n2\tZwei\r\n3\tThree\r\n5\tFive\r\n9\tNine\r\n", true));
        assert_eq!(t.names().get(3), Some("Three"));
        // the same name again changes nothing
        let mut same = AreaTxt::parse(&b);
        same.set_name(5, "Five").unwrap();
        assert_eq!(same.to_bytes(), b);
        // refused text
        assert!(t.set_name(1, "a\tb").is_err() && t.set_name(1, "a\nb").is_err());
        assert_eq!(t.names().get(1), None, "a refused name adds nothing");
    }

    #[test]
    fn appending_after_a_last_line_without_a_newline() {
        let mut t = AreaTxt::parse(&utf16("0\tOne\n1\tTwo", true));
        t.set_name(7, "Seven").unwrap();
        assert_eq!(t.to_bytes(), utf16("0\tOne\n1\tTwo\n7\tSeven", true), "the file keeps its LF style and its missing final newline");
        let mut e = AreaTxt::parse(&utf16("", true));
        e.set_name(0, "A").unwrap();
        assert_eq!(e.to_bytes(), utf16("0\tA\r\n", true), "an empty file starts with CRLF");
        let mut e = AreaTxt::parse(b"");
        e.set_name(0, "A").unwrap();
        assert_eq!(e.to_bytes(), utf16("0\tA\r\n", true), "a new file gets the BOM");
        // a file in another encoding (UTF-8, cp949) or with an odd byte is not changed
        for other in ["0\tOne\r\n1\tTwo\r\n".as_bytes().to_vec(), vec![0xff, 0xfe, 0x30, 0x00, 0x09, 0x00, 0x41]] {
            let mut x = AreaTxt::parse(&other);
            assert!(x.set_name(1, "B").is_err(), "{other:?}");
            assert_eq!(x.to_bytes(), other);
        }
        // Korean text and a character outside the BMP (a surrogate pair) round-trip through set_name
        let mut k = AreaTxt::parse(&utf16("0\tOne\r\n", true));
        k.set_name(0, "\u{d3ec}\u{d0c8} \u{1f600}").unwrap();
        assert_eq!(k.names().get(0), Some("\u{d3ec}\u{d0c8} \u{1f600}"));
        assert_eq!(parse_area_names(&k.to_bytes(), "t").unwrap().get(0), Some("\u{d3ec}\u{d0c8} \u{1f600}"));
        // a name after an odd tail byte or non-matching lines keeps them
        let mut o = AreaTxt::parse(&utf16("junk\r\n3\tC\r\n", false));
        o.set_name(1, "B").unwrap();
        assert_eq!(o.to_bytes(), utf16("junk\r\n1\tB\r\n3\tC\r\n", false));
    }

    /// Every `world/<id>.txt` of the client writes back identically, reads like the existing reader, and a rename of one
    /// line changes only that line.
    #[test]
    fn area_txt_roundtrip() {
        let Some(dir) = std::env::var_os("SHAIYA_CLIENT").map(std::path::PathBuf::from).filter(|d| d.is_dir()) else {
            eprintln!("skipped: SHAIYA_CLIENT not set");
            return;
        };
        let c = crate::source::ClientSource::open(&dir).expect("open the client");
        let mut seen = 0;
        for id in c.map_ids() {
            let path = format!("world/{id}.txt");
            let Ok(bytes) = c.read(&path) else { continue };
            seen += 1;
            let t = AreaTxt::parse(&bytes);
            assert_eq!(t.to_bytes(), bytes, "{path}");
            assert_eq!(t.names(), parse_area_names(&bytes, &path).unwrap(), "{path}");
            // rename the first named area and put the name back
            let Some((i, old)) = t.names().entries.first().cloned() else { continue };
            let mut e = t.clone();
            e.set_name(i, "renamed").unwrap();
            assert_ne!(e.to_bytes(), bytes);
            assert_eq!(e.names().get(i as usize), Some("renamed"));
            assert_eq!(e.names().len(), t.names().len());
            e.set_name(i, &old).unwrap();
            assert_eq!(e.to_bytes(), bytes, "{path}: the name put back gives the original bytes");
        }
        eprintln!("area_txt_roundtrip: {seen} files");
        assert!(seen > 0, "the client has world/<id>.txt files");
    }
}
