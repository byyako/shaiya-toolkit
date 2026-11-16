//! Text files of the client: `sysmsg-uni.txt` (system messages) and `world/<id>.txt` (area names), both UTF-16LE with a
//! BOM; and the map names the client shows (`CPlayerData::GetMapName`, `0x4c9100`).

use std::collections::HashMap;

use crate::error::{Error, Result};

/// UTF-16LE (BOM optional) to a string; an odd trailing byte is an error.
pub fn decode_utf16le(data: &[u8], file: &str) -> Result<String> {
    let data = data.strip_prefix(&[0xff, 0xfe][..]).unwrap_or(data);
    if data.len() % 2 != 0 {
        return Err(Error::Format { file: file.into(), what: "UTF-16 text with an odd number of bytes".into(), offset: data.len() });
    }
    let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Ok(String::from_utf16_lossy(&units))
}

/// `sysmsg-uni.txt`: lines `id<whitespace>"text"`; the text is what lies between the first and the last quote.
#[derive(Clone, Debug, Default)]
pub struct SysMsg {
    pub messages: HashMap<u32, String>,
}

impl SysMsg {
    pub fn parse(data: &[u8], file: &str) -> Result<SysMsg> {
        let text = decode_utf16le(data, file)?;
        let mut messages = HashMap::new();
        for line in text.split('\n') {
            let line = line.trim_end_matches('\r');
            let digits = line.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                continue;
            }
            let Ok(id) = line[..digits].parse::<u32>() else { continue };
            let rest = &line[digits..];
            let Some(open) = rest.find('"') else { continue };
            if !rest[..open].chars().all(char::is_whitespace) {
                continue;
            }
            let body = &rest[open + 1..];
            let body = match body.rfind('"') {
                Some(close) => &body[..close],
                None => body,
            };
            messages.insert(id, body.to_string());
        }
        Ok(SysMsg { messages })
    }

    pub fn get(&self, id: u32) -> Option<&str> {
        self.messages.get(&id).map(String::as_str)
    }
}

/// `world/<id>.txt`: lines `index<TAB>name`. The index is the number of the `.wld` area the name belongs to; it need not
/// run without gaps (map 2 names areas 0-66, 87 and 88 and leaves the picture-caption areas 67-86 unnamed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AreaNames {
    pub entries: Vec<(u32, String)>,
}

impl AreaNames {
    pub fn get(&self, area: usize) -> Option<&str> {
        self.entries.iter().find(|(i, _)| *i as usize == area).map(|(_, n)| n.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub fn parse_area_names(data: &[u8], file: &str) -> Result<AreaNames> {
    let text = decode_utf16le(data, file)?;
    let mut entries = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let Some((index, name)) = line.split_once('\t') else { continue };
        let Ok(index) = index.trim().parse::<u32>() else { continue };
        entries.push((index, name.to_string()));
    }
    Ok(AreaNames { entries })
}

/// Sysmsg id of a map's name, decoded from the jump table of `CPlayerData::GetMapName` (`None`: the client shows the
/// number).
pub fn map_name_msg(map: u32) -> Option<u32> {
    Some(match map {
        0 => 5068,
        1 => 5069,
        2 => 5070,
        3 | 4 => 5071,
        5 | 6 => 5072,
        7 | 8 => 5073,
        9..=11 => 5096,
        12..=14 => 5097,
        15..=17 => 5098,
        18 => 5074,
        19 => 5099,
        20 => 5100,
        21 | 22 => 5101,
        23 | 24 => 5102,
        25 => 5103,
        26 | 27 => 5104,
        28 => 5105,
        29 => 5106,
        30 => 5107,
        31 => 5108,
        32 => 5109,
        33 => 5110,
        34 => 5111,
        35 => 5112,
        36 => 5113,
        42 => 5419,
        43 => 5375,
        44 => 5376,
        45 => 5378,
        46 => 5379,
        47 => 5380,
        48 | 49 => 5405,
        50 => 5406,
        51 | 52 => 5407,
        53 | 54 => 5408,
        55 | 56 => 5409,
        57 => 5410,
        58 => 5411,
        59 => 5412,
        60 => 5413,
        61 => 5414,
        62 => 5415,
        63 => 5416,
        68 => 5425,
        69 => 5426,
        70 => 5427,
        102..=105 | 108 => 5417,
        _ => return None,
    })
}

/// The map's name as the client shows it; `"Map <id>"` when there is no entry or no text.
pub fn map_name(map: u32, msgs: Option<&SysMsg>) -> String {
    map_name_msg(map)
        .and_then(|id| msgs.and_then(|m| m.get(id)))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Map {map}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut v = vec![0xff, 0xfe];
        for u in s.encode_utf16() {
            v.extend_from_slice(&u.to_le_bytes());
        }
        v
    }

    #[test]
    fn sysmsg_lines() {
        let m = SysMsg::parse(&utf16("1\t\t\"You have died\"\r\n5068\t\t\"D-Water \"quoted\" Land\"\r\nbad line\r\n7 \"open\r\n"), "t").unwrap();
        assert_eq!(m.get(1), Some("You have died"));
        assert_eq!(m.get(5068), Some("D-Water \"quoted\" Land"));
        assert_eq!(m.get(7), Some("open"));
        assert_eq!(m.get(2), None);
        assert_eq!(map_name(0, Some(&m)), "D-Water \"quoted\" Land");
        assert_eq!(map_name(1, Some(&m)), "Map 1");
        assert_eq!(map_name(37, Some(&m)), "Map 37");
    }

    #[test]
    fn area_names_by_index() {
        let n = parse_area_names(&utf16("0\tFirst\r\n5\tSecond area\r\nno tab\r\n"), "t").unwrap();
        assert_eq!(n.len(), 2);
        assert_eq!(n.get(0), Some("First"));
        assert_eq!(n.get(5), Some("Second area"));
        assert_eq!(n.get(1), None);
        assert!(decode_utf16le(&[0x41, 0x00, 0x42], "t").is_err());
    }

    #[test]
    fn map_table_edges() {
        assert_eq!(map_name_msg(18), Some(5074));
        assert_eq!(map_name_msg(108), Some(5417));
        assert_eq!(map_name_msg(37), None);
        assert_eq!(map_name_msg(106), None);
        assert_eq!(map_name_msg(70), Some(5427));
    }
}
