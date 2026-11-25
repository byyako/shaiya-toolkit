//! `shaiya info` on sounds and pictures: the header facts, found by content. The files are built here (a tone, an Ogg page, a JPEG frame header,
//! a PNG header); nothing is decoded.

use shaiya_core::Job;
use shaiya_inspect::{describe, inspect, Details, Kind, Options};
use shaiya_testkit::TempDir;
use std::path::Path;

fn info(p: &Path) -> shaiya_inspect::Info {
    inspect(p, &Options::default(), &Job::silent()).unwrap()
}

fn wav(tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    let mut fmt = Vec::new();
    fmt.extend(tag.to_le_bytes());
    fmt.extend(channels.to_le_bytes());
    fmt.extend(rate.to_le_bytes());
    fmt.extend((rate * channels as u32 * bits as u32 / 8).to_le_bytes());
    fmt.extend((channels * bits / 8).to_le_bytes());
    fmt.extend(bits.to_le_bytes());
    let mut body = b"WAVE".to_vec();
    body.extend(b"fmt ");
    body.extend((fmt.len() as u32).to_le_bytes());
    body.extend(&fmt);
    body.extend(b"data");
    body.extend((data.len() as u32).to_le_bytes());
    body.extend(data);
    let mut out = b"RIFF".to_vec();
    out.extend((body.len() as u32).to_le_bytes());
    out.extend(body);
    out
}

fn ogg_page(granule: i64, body: &[u8]) -> Vec<u8> {
    let mut p = b"OggS\0\x02".to_vec();
    p.extend(granule.to_le_bytes());
    p.extend([0u8; 12]);
    p.push(1);
    p.push(body.len() as u8);
    p.extend(body);
    p
}

#[test]
fn a_wav_is_described_by_its_header_whatever_its_name() {
    let d = TempDir::new("inspect-wav");
    let f = d.path().join("tone.bin");
    std::fs::write(&f, wav(1, 2, 44100, 16, &vec![0u8; 44100 * 4])).unwrap();
    let i = info(&f);
    assert_eq!(i.kind, Kind::Wav);
    assert!(matches!(&i.details, Details::Sound { channels: 2, rate: 44100, .. }));
    assert_eq!(describe(&i), format!("WAV, PCM 16 bit, 2 channels, 44100 Hz, 1.0 s, {} bytes", shaiya_inspect::group(i.size)));
    assert!(i.problems.is_empty());
    // an unsupported tag still reads and says so
    let g = d.path().join("float.wav");
    std::fs::write(&g, wav(3, 1, 22050, 32, &[0; 16])).unwrap();
    let j = info(&g);
    assert_eq!(j.kind, Kind::Wav);
    assert!(j.problems.iter().any(|p| p.contains("unsupported format (tag 3, 32 bits)")), "{:?}", j.problems);
    // a damaged one is a WAV with a problem line and zeros
    let h = d.path().join("cut.wav");
    let mut cut = wav(1, 1, 8000, 16, &[0; 100]);
    cut.truncate(60);
    std::fs::write(&h, cut).unwrap();
    let k = info(&h);
    assert_eq!(k.kind, Kind::Wav);
    assert!(matches!(&k.details, Details::Sound { channels: 0, rate: 0, .. }));
    assert!(!k.problems.is_empty());
}

#[test]
fn an_ogg_is_described_by_its_vorbis_header() {
    let d = TempDir::new("inspect-ogg");
    let mut ident = vec![1];
    ident.extend(b"vorbis");
    ident.extend(0u32.to_le_bytes());
    ident.push(2);
    ident.extend(32000u32.to_le_bytes());
    ident.extend([0u8; 14]);
    let mut f = ogg_page(0, &ident);
    f.extend(ogg_page(64000, &[1, 2, 3]));
    let p = d.path().join("m.ogg");
    std::fs::write(&p, &f).unwrap();
    let i = info(&p);
    assert_eq!(i.kind, Kind::Ogg);
    assert_eq!(describe(&i), format!("Ogg Vorbis, 2 channels, 32000 Hz, 2.0 s, {} bytes", i.size));
}

#[test]
fn a_jpg_and_a_png_are_described_by_their_headers() {
    let d = TempDir::new("inspect-pic");
    let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
    jpg.extend([0xFF, 0xC2, 0x00, 0x0B, 0x08, 0x01, 0x00, 0x02, 0x00, 0x01, 0x01, 0x11, 0x00]);
    let p = d.path().join("lens.jpg");
    std::fs::write(&p, &jpg).unwrap();
    let i = info(&p);
    assert_eq!(i.kind, Kind::Jpg);
    assert_eq!(describe(&i), format!("JPEG progressive, 1 component, 512 x 256, {} bytes", i.size));
    let mut png = shaiya_clientdata::texture::PNG_MAGIC.to_vec();
    png.extend(13u32.to_be_bytes());
    png.extend(b"IHDR");
    png.extend(64u32.to_be_bytes());
    png.extend(32u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0]);
    png.extend([0u8; 4]);
    let q = d.path().join("a.png");
    std::fs::write(&q, &png).unwrap();
    let j = info(&q);
    assert_eq!(j.kind, Kind::Png);
    assert_eq!(describe(&j), format!("PNG RGBA 8 bit, 64 x 32, {} bytes", j.size));
    // a JPEG whose frame header is missing is a JPEG with a problem line
    let r = d.path().join("bad.jpg");
    std::fs::write(&r, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
    let k = info(&r);
    assert_eq!(k.kind, Kind::Jpg);
    assert!(!k.problems.is_empty());
}

#[test]
fn a_saf_whose_first_file_is_a_sound_or_a_picture_is_still_a_saf() {
    // a data.saf with no data.sah beside it starts with the bytes of its first stored file: it must not be described as that file
    let d = TempDir::new("inspect-saf-media");
    let w = d.path().join("data.saf");
    std::fs::write(&w, wav(1, 1, 8000, 16, &[0; 64])).unwrap();
    assert_eq!(info(&w).kind, Kind::Saf);
    let mut png = shaiya_clientdata::texture::PNG_MAGIC.to_vec();
    png.extend([0u8; 40]);
    let q = d.path().join("other.saf");
    std::fs::write(&q, &png).unwrap();
    assert_eq!(info(&q).kind, Kind::Saf);
    let o = d.path().join("third.saf");
    std::fs::write(&o, ogg_page(0, &[1, 2, 3])).unwrap();
    assert_eq!(info(&o).kind, Kind::Saf);
    // the same bytes under another name are still found by content
    let n = d.path().join("data.bin");
    std::fs::write(&n, wav(1, 1, 8000, 16, &[0; 64])).unwrap();
    assert_eq!(info(&n).kind, Kind::Wav);
}

#[test]
fn a_jpeg_whose_frame_header_lies_past_the_first_64_kib_says_so() {
    let d = TempDir::new("inspect-jpg-long");
    let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    // one APP1 segment of 65,533 bytes (the largest), then a second, so that the frame header starts after 64 KiB
    jpg.extend(65_535u16.to_be_bytes());
    jpg.extend(vec![0u8; 65_533]);
    jpg.extend([0xFF, 0xE1, 0x00, 0x10]);
    jpg.extend([0u8; 14]);
    jpg.extend([0xFF, 0xC2, 0x00, 0x0B, 0x08, 0x01, 0x00, 0x02, 0x00, 0x01, 0x01, 0x11, 0x00]);
    let p = d.path().join("exif.jpg");
    std::fs::write(&p, &jpg).unwrap();
    let i = info(&p);
    assert_eq!(i.kind, Kind::Jpg);
    assert!(i.problems.iter().any(|m| m.contains("no JPEG frame header in the first 64 KiB")), "{:?}", i.problems);
}
