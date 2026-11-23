//! Opt-in tests of the model JSON on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder with `data.sah` and
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

/// What one format's files gave: exported (and imported to the same bytes), refused (with the reason), the files whose JSON holds a
/// `bits:` float, the largest JSON.
#[derive(Default)]
struct Tally {
    exported: usize,
    refused: Vec<String>,
    bits_form: Vec<String>,
    largest: usize,
}

#[test]
fn every_model_file_of_the_us_client_exports_and_imports_to_the_same_bytes() {
    let Some(dir) = env_client() else { return };
    let archive = Archive::open(&dir).unwrap();
    let started = std::time::Instant::now();
    let formats = [ModelFormat::C3dc, ModelFormat::Vani, ModelFormat::Mani, ModelFormat::Mesh3do, ModelFormat::Mesh3de];
    let mut tallies: Vec<(ModelFormat, Tally)> = formats.iter().map(|f| (*f, Tally::default())).collect();
    for file in archive.index().visible() {
        let Some(format) = ModelFormat::from_file_name(&file.path).filter(|f| formats.contains(f)) else { continue };
        let tally = &mut tallies.iter_mut().find(|(f, _)| *f == format).unwrap().1;
        let bytes = archive.read_entry(file.entry).unwrap();
        match export(format, &bytes, &file.path) {
            Ok(text) => {
                tally.largest = tally.largest.max(text.len());
                if text.contains("\"bits:") {
                    tally.bits_form.push(file.path.clone());
                }
                let (back_format, back) = import(text.as_bytes()).unwrap_or_else(|e| panic!("{}: {e}", file.path));
                assert_eq!(back_format, format, "{}", file.path);
                assert!(back == bytes, "{}: imported to different bytes", file.path);
                tally.exported += 1;
            }
            Err(e) => tally.refused.push(format!("{}: {e}", file.path)),
        }
    }
    for (format, t) in &tallies {
        println!(
            "{} json: {} exported and imported to the same bytes, {} refused {:?}, {} with a `bits:` float {:?}, largest JSON {} bytes",
            format.label(),
            t.exported,
            t.refused.len(),
            t.refused,
            t.bits_form.len(),
            t.bits_form,
            t.largest
        );
    }
    println!("model json: {:.1} s", started.elapsed().as_secs_f64());
    let get = |f: ModelFormat| &tallies.iter().find(|(x, _)| *x == f).unwrap().1;
    for (format, exported) in [(ModelFormat::C3dc, 2_431), (ModelFormat::Vani, 66), (ModelFormat::Mani, 23), (ModelFormat::Mesh3do, 271), (ModelFormat::Mesh3de, 467)] {
        assert_eq!(get(format).exported, exported, "{}", format.label());
    }
    for format in [ModelFormat::C3dc, ModelFormat::Vani, ModelFormat::Mani, ModelFormat::Mesh3do] {
        assert!(get(format).refused.is_empty(), "{}: {:?}", format.label(), get(format).refused);
    }
    // the two `.3DE` the reader refuses stay refused, by name
    let refused = &get(ModelFormat::Mesh3de).refused;
    assert_eq!(refused.len(), 2, "{refused:?}");
    let names: Vec<String> = refused.iter().map(|r| r.split(": ").next().unwrap_or("").replace('\\', "/").to_lowercase()).collect();
    assert!(names.iter().any(|n| n.ends_with("/004.3de")), "{names:?}");
    assert!(names.iter().any(|n| n.ends_with("/deathknight007.3de")), "{names:?}");
    // the `bits:` form: exactly 6 `.3DC` (vertex fields), none of the others
    assert_eq!(get(ModelFormat::C3dc).bits_form.len(), 6, "{:?}", get(ModelFormat::C3dc).bits_form);
    for format in [ModelFormat::Vani, ModelFormat::Mani, ModelFormat::Mesh3do, ModelFormat::Mesh3de] {
        assert!(get(format).bits_form.is_empty(), "{}: {:?}", format.label(), get(format).bits_form);
    }
}
