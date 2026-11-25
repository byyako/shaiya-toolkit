//! Ogg Vorbis header facts (channels, sample rate, duration) from the first page and the last page's granule position. Nothing is decoded.

use crate::error::{Error, Result};

const WHAT: &str = "Ogg file";

/// What the Vorbis identification header and the last granule position say.
#[derive(Clone, Debug, PartialEq)]
pub struct OggInfo {
    pub channels: u8,
    pub rate: u32,
    /// Last granule position / rate; 0 when no page carries a position.
    pub duration_s: f64,
}

fn fail(what: impl Into<String>, offset: usize) -> Error {
    Error::Format { file: WHAT.to_string(), what: what.into(), offset }
}

/// One page header at `at`: `(granule, header length, body length)`; `None` when it is not a page header or runs past the end.
fn page(bytes: &[u8], at: usize) -> Option<(i64, usize, usize)> {
    let h = bytes.get(at..at.checked_add(27)?)?;
    if &h[..4] != b"OggS" || h[4] != 0 {
        return None;
    }
    let granule = i64::from_le_bytes(h[6..14].try_into().ok()?);
    let nseg = h[26] as usize;
    let table = bytes.get(at + 27..at + 27 + nseg)?;
    let body: usize = table.iter().map(|&b| b as usize).sum();
    (at + 27 + nseg + body <= bytes.len()).then_some((granule, 27 + nseg, body))
}

/// Reads the Vorbis identification header of the first page and the granule position of the last page that has one.
pub fn ogg_info(bytes: &[u8]) -> Result<OggInfo> {
    let Some((_, hlen, blen)) = page(bytes, 0) else { return Err(fail("not an Ogg file (no page at the start)", 0)) };
    let body = &bytes[hlen..hlen + blen];
    if body.len() < 30 || body[0] != 1 || &body[1..7] != b"vorbis" {
        return Err(fail("the first packet is not a Vorbis identification header", hlen));
    }
    let version = u32::from_le_bytes(body[7..11].try_into().expect("4 bytes"));
    if version != 0 {
        return Err(fail(format!("Vorbis version {version} is not known"), hlen + 7));
    }
    let channels = body[11];
    let rate = u32::from_le_bytes(body[12..16].try_into().expect("4 bytes"));
    if channels == 0 || rate == 0 {
        return Err(fail(format!("Vorbis header with {channels} channels and {rate} Hz"), hlen + 11));
    }
    // the last page with a granule position: search backwards for a page tag that parses
    let mut granule = 0i64;
    let mut at = bytes.len().saturating_sub(4);
    let floor = bytes.len().saturating_sub(70_000);
    loop {
        if bytes.get(at..at + 4) == Some(b"OggS") {
            if let Some((g, _, _)) = page(bytes, at) {
                if g >= 0 {
                    granule = g;
                    break;
                }
            }
        }
        if at <= floor {
            break;
        }
        at -= 1;
    }
    Ok(OggInfo { channels, rate, duration_s: granule as f64 / rate as f64 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One Ogg page (version 0) with the given granule and body (a single lacing run of up to 255 x 255 bytes).
    pub fn ogg_page(granule: i64, seq: u32, kind: u8, body: &[u8]) -> Vec<u8> {
        let mut segs = vec![255u8; body.len() / 255];
        segs.push((body.len() % 255) as u8);
        let mut p = b"OggS".to_vec();
        p.extend([0, kind]);
        p.extend(granule.to_le_bytes());
        p.extend(7u32.to_le_bytes()); // serial
        p.extend(seq.to_le_bytes());
        p.extend(0u32.to_le_bytes()); // checksum (not checked)
        p.push(segs.len() as u8);
        p.extend(&segs);
        p.extend(body);
        p
    }

    fn ident(channels: u8, rate: u32) -> Vec<u8> {
        let mut b = vec![1];
        b.extend(b"vorbis");
        b.extend(0u32.to_le_bytes());
        b.push(channels);
        b.extend(rate.to_le_bytes());
        b.extend([0u8; 12]); // bitrates
        b.push(0xb8); // blocksizes
        b.push(1); // framing
        b
    }

    #[test]
    fn ogg_info_reads_channels_rate_and_duration() {
        let mut f = ogg_page(0, 0, 2, &ident(2, 44100));
        f.extend(ogg_page(0, 1, 0, &[3, b'v', b'o', b'r', b'b', b'i', b's']));
        f.extend(ogg_page(22050, 2, 0, &[0x55; 300]));
        f.extend(ogg_page(-1, 3, 0, &[0x66; 40])); // a page with no position is skipped
        f.extend(ogg_page(88200, 4, 4, &[0x77; 10])); // not the last: the next page wins
        f.extend(ogg_page(132_300, 5, 4, &[0x77; 700]));
        let info = ogg_info(&f).unwrap();
        assert_eq!((info.channels, info.rate), (2, 44100));
        assert!((info.duration_s - 3.0).abs() < 1e-9, "{}", info.duration_s);
        // only the identification page: duration 0
        let one = ogg_page(0, 0, 2, &ident(1, 22050));
        let info = ogg_info(&one).unwrap();
        assert_eq!((info.channels, info.rate, info.duration_s), (1, 22050, 0.0));
        // a page tag inside a body that does not parse as a page is not taken for the last page
        let mut tricky = ogg_page(0, 0, 2, &ident(1, 1000));
        tricky.extend(ogg_page(2000, 1, 4, b"xxOggS\0\x02\xff\xff\xff\xff\xff\xff\xff\x7fyyyy"));
        assert!((ogg_info(&tricky).unwrap().duration_s - 2.0).abs() < 1e-9);
    }

    #[test]
    fn ogg_info_refuses_other_files_and_never_panics() {
        assert!(ogg_info(b"").is_err());
        assert!(ogg_info(b"OggS").is_err());
        assert!(ogg_info(b"RIFF....WAVE").is_err());
        // an Opus or a bad header, zero channels, a version other than 0
        let mut opus = vec![b'O', b'p', b'u', b's', b'H', b'e', b'a', b'd'];
        opus.resize(40, 0);
        assert!(ogg_info(&ogg_page(0, 0, 2, &opus)).is_err());
        assert!(ogg_info(&ogg_page(0, 0, 2, &ident(0, 44100))).is_err());
        assert!(ogg_info(&ogg_page(0, 0, 2, &ident(2, 0))).is_err());
        let mut v1 = ident(2, 44100);
        v1[7] = 1;
        assert!(ogg_info(&ogg_page(0, 0, 2, &v1)).unwrap_err().to_string().contains("Vorbis version 1"));
        let good = {
            let mut f = ogg_page(0, 0, 2, &ident(2, 44100));
            f.extend(ogg_page(44100, 1, 4, &[1; 50]));
            f
        };
        for n in 0..good.len() {
            let _ = ogg_info(&good[..n]);
        }
        for i in 0..80 {
            let mut b = good.clone();
            b[i] ^= 0xff;
            let _ = ogg_info(&b);
        }
    }
}
