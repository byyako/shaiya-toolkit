//! Text encodings for game data. Names keep their raw bytes elsewhere; these functions decode them for display and encode them for writing.

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Cp949,
    Cp1252,
    Cp1251,
    Cp936,
    Utf8,
    Utf16Le,
}

impl Encoding {
    fn rs(self) -> &'static encoding_rs::Encoding {
        match self {
            Encoding::Cp949 => encoding_rs::EUC_KR,
            Encoding::Cp1252 => encoding_rs::WINDOWS_1252,
            Encoding::Cp1251 => encoding_rs::WINDOWS_1251,
            Encoding::Cp936 => encoding_rs::GBK,
            Encoding::Utf8 => encoding_rs::UTF_8,
            Encoding::Utf16Le => encoding_rs::UTF_16LE,
        }
    }

    /// Decodes without byte-order-mark sniffing. The flag says whether undecodable bytes were replaced.
    pub fn decode(self, raw: &[u8]) -> (String, bool) {
        let (s, had_errors) = self.rs().decode_without_bom_handling(raw);
        (s.into_owned(), had_errors)
    }

    /// `Error::Format` naming the first character the encoding cannot hold. UTF-16LE is encoded by hand (encoding_rs has no UTF-16 encoder).
    pub fn encode(self, s: &str) -> Result<Vec<u8>> {
        match self {
            Encoding::Utf8 => Ok(s.as_bytes().to_vec()),
            Encoding::Utf16Le => Ok(s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()),
            _ => {
                let (bytes, _, unmappable) = self.rs().encode(s);
                if !unmappable {
                    return Ok(bytes.into_owned());
                }
                for (i, ch) in s.char_indices() {
                    let mut buf = [0u8; 4];
                    let (_, _, bad) = self.rs().encode(ch.encode_utf8(&mut buf));
                    if bad {
                        return Err(Error::format(
                            "text",
                            Some(i as u64),
                            format!("character {ch:?} (U+{:04X}) cannot be written in {self:?}", ch as u32),
                        ));
                    }
                }
                Err(Error::format("text", None, format!("text cannot be written in {self:?}")))
            }
        }
    }
}

/// For showing raw names: decodes, and shows undecodable bytes as `\xNN` (never used to write).
pub fn display_lossless(raw: &[u8], enc: Encoding) -> String {
    let (s, had_errors) = enc.rs().decode_without_bom_handling_and_without_replacement(raw).map_or_else(
        || (String::new(), true),
        |c| (c.into_owned(), false),
    );
    if !had_errors {
        return s;
    }
    let mut out = String::new();
    let mut pos = 0;
    'outer: while pos < raw.len() {
        for n in 1..=4.min(raw.len() - pos) {
            if let Some(c) = enc.rs().decode_without_bom_handling_and_without_replacement(&raw[pos..pos + n]) {
                out.push_str(&c);
                pos += n;
                continue 'outer;
            }
        }
        out.push_str(&format!("\\x{:02X}", raw[pos]));
        pos += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp949_round_trip() {
        let raw = Encoding::Cp949.encode("\u{BAAC}\u{C2A4}\u{D130}").unwrap();
        assert_eq!(raw, vec![0xB8, 0xF3, 0xBD, 0xBA, 0xC5, 0xCD]);
        let (s, bad) = Encoding::Cp949.decode(&raw);
        assert_eq!(s, "\u{BAAC}\u{C2A4}\u{D130}");
        assert!(!bad);
        assert_eq!(display_lossless(&raw, Encoding::Cp949), s);
    }

    #[test]
    fn utf16le_encodes_by_hand() {
        let b = Encoding::Utf16Le.encode("A\u{BAAC}\u{1F600}").unwrap();
        assert_eq!(b, vec![0x41, 0x00, 0xAC, 0xBA, 0x3D, 0xD8, 0x00, 0xDE]);
        assert_eq!(Encoding::Utf16Le.decode(&b).0, "A\u{BAAC}\u{1F600}");
        // a leading BOM is data, not a hint
        assert_eq!(Encoding::Utf16Le.decode(&[0xFF, 0xFE, 0x41, 0x00]).0, "\u{FEFF}A");
    }

    #[test]
    fn unmappable_character_is_named() {
        let e = Encoding::Cp1252.encode("ok \u{BAAC}").unwrap_err();
        match e {
            Error::Format { offset, what, .. } => {
                assert_eq!(offset, Some(3));
                assert!(what.contains("U+BAAC"), "{what}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(Encoding::Cp1252.encode("caf\u{E9}").unwrap(), vec![b'c', b'a', b'f', 0xE9]);
        assert_eq!(Encoding::Cp1251.decode(&[0xCF, 0xF0, 0xE8]).0, "\u{41F}\u{440}\u{438}");
        assert_eq!(Encoding::Cp936.decode(&[0xD6, 0xD0]).0, "\u{4E2D}");
        assert_eq!(Encoding::Utf8.decode("h\u{E9}".as_bytes()), ("h\u{E9}".to_string(), false));
    }

    #[test]
    fn display_lossless_shows_bad_bytes() {
        // a lone Hangul lead byte at the end, an invalid byte in the middle
        let raw = [b'a', 0xFF, b'b', 0xB8];
        let shown = display_lossless(&raw, Encoding::Cp949);
        assert!(shown.starts_with('a'));
        assert!(shown.contains("\\xFF") || shown.contains("\\xB8"), "{shown}");
        assert_eq!(display_lossless(&[0x80, b'x'], Encoding::Utf8), "\\x80x");
        assert_eq!(display_lossless(b"plain", Encoding::Cp1252), "plain");
        // the replacement character never appears
        assert!(!shown.contains('\u{FFFD}'));
    }
}
