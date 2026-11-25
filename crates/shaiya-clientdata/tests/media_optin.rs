//! Opt-in tests of the sound and picture readers on a real client (read only). `SHAIYA_CLIENT` is a client folder with `data.sah` and
//! `data.saf` (US EP4.5); each test prints "skipped: SHAIYA_CLIENT not set" and returns when it is unset. The numbers are measured on US EP4.5.

use shaiya_archive::Archive;
use shaiya_clientdata::texture::{decode, picture_info};
use shaiya_clientdata::wav::{decode as wav_decode, parse_header};
use shaiya_testkit::env_client;
use std::collections::BTreeMap;

fn files_with(a: &Archive, ext: &str) -> Vec<(String, shaiya_archive::Entry)> {
    a.index().visible().into_iter().filter(|f| f.path.to_ascii_lowercase().ends_with(ext)).map(|f| (f.path, f.entry.clone())).collect()
}

#[test]
fn every_wav_of_the_us_client_reads_its_header() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let wavs = files_with(&a, ".wav");
    let mut tags: BTreeMap<u16, u64> = BTreeMap::new();
    let mut shapes: BTreeMap<(u32, u16, u16, u16), u64> = BTreeMap::new();
    let (mut decoded, mut seconds) = (0u64, 0f64);
    for (path, entry) in &wavs {
        let bytes = a.read_entry(entry).unwrap();
        let info = parse_header(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        *tags.entry(info.tag).or_default() += 1;
        *shapes.entry((info.rate, info.channels, info.bits, info.tag)).or_default() += 1;
        seconds += info.duration_s;
        // the whole sound decodes, to as many frames as the header's duration says
        let pcm = wav_decode(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!((pcm.duration_s() - info.duration_s).abs() < 1e-6, "{path}: {} vs {}", pcm.duration_s(), info.duration_s);
        decoded += 1;
    }
    println!("wav files {}, decoded {decoded}, total {seconds:.0} s; tags {tags:?}", wavs.len());
    println!("(rate, channels, bits, tag) -> files: {shapes:?}");
    assert_eq!(wavs.len(), 1_019);
    assert_eq!(tags, BTreeMap::from([(1u16, 1_018u64), (17, 1)]));
    assert_eq!(decoded, 1_019);
}

#[test]
fn every_jpg_of_the_us_client_decodes() {
    let Some(dir) = env_client() else {
        println!("skipped: SHAIYA_CLIENT not set");
        return;
    };
    let a = Archive::open(&dir).unwrap();
    let jpgs = files_with(&a, ".jpg");
    let mut kinds: BTreeMap<String, u64> = BTreeMap::new();
    for (path, entry) in &jpgs {
        let bytes = a.read_entry(entry).unwrap();
        let info = picture_info(&bytes).unwrap_or_else(|| panic!("{path}: no header"));
        let img = decode(&bytes, path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!((img.width, img.height), (info.width, info.height), "{path}");
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        *kinds.entry(format!("{} {}x{}", info.what, info.width, info.height)).or_default() += 1;
    }
    println!("jpg files {}: {kinds:?}", jpgs.len());
    assert_eq!(jpgs.len(), 22);
    let baseline: u64 = kinds.iter().filter(|(k, _)| k.contains("baseline")).map(|(_, v)| v).sum();
    let progressive: u64 = kinds.iter().filter(|(k, _)| k.contains("progressive")).map(|(_, v)| v).sum();
    assert_eq!((baseline, progressive), (15, 7));
}
