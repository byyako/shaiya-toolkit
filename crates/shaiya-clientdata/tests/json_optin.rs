//! Opt-in test of the model JSON on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder with `data.sah` and
//! `data.saf` (US EP4.5). The test prints "skipped: <VAR> not set" and returns when the variable is unset. One file is in memory at a time.
//! Run it with `-- --test-threads=2`.

use shaiya_archive::Archive;
use shaiya_clientdata::modeljson::{export, import, ModelFormat};
use shaiya_testkit::env_client;

#[test]
fn every_ani_of_the_us_client_exports_and_imports_to_the_same_bytes() {
    let Some(dir) = env_client() else { return };
    let archive = Archive::open(&dir).unwrap();
    let started = std::time::Instant::now();
    let (mut exported, mut refused, mut largest) = (0usize, Vec::new(), 0usize);
    for file in archive.index().visible() {
        if ModelFormat::from_file_name(&file.path) != Some(ModelFormat::Ani) {
            continue;
        }
        let bytes = archive.read_entry(file.entry).unwrap();
        match export(ModelFormat::Ani, &bytes, &file.path) {
            Ok(text) => {
                largest = largest.max(text.len());
                let (format, back) = import(text.as_bytes()).unwrap_or_else(|e| panic!("{}: {e}", file.path));
                assert_eq!(format, ModelFormat::Ani, "{}", file.path);
                assert!(back == bytes, "{}: imported to different bytes", file.path);
                exported += 1;
            }
            Err(e) => refused.push(format!("{}: {e}", file.path)),
        }
    }
    println!("ani json: {exported} exported and imported to the same bytes, {} refused, largest JSON {largest} bytes, {:.1} s", refused.len(), started.elapsed().as_secs_f64());
    assert!(refused.is_empty(), "{refused:?}");
    assert_eq!(exported, 3_509);
}
