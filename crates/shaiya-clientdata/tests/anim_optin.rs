//! Opt-in tests of the `.ANI` reader on a real client (read only; nothing is written). `SHAIYA_CLIENT` is a client folder with `data.sah`
//! and `data.saf` (US EP4.5); `SHAIYA_CLIENT_ORIGINAL` is an archive holding the stock files (a `.sah` file or a folder). Each test prints
//! "skipped: <VAR> not set" and returns when its variable is unset. The numbers are measured on US EP4.5 only.

use shaiya_archive::Archive;
use shaiya_clientdata::anim::{issue, passes, rig_of, validate, Severity, RIGS};
use shaiya_clientdata::Ani;
use shaiya_testkit::{env_client, env_client_original};
use std::collections::{BTreeMap, BTreeSet};

fn is_ani(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".ani")
}

/// Calls `f(path, bytes)` for every visible `.ani` of the archive, one file in memory at a time.
fn each_ani(a: &Archive, mut f: impl FnMut(&str, &[u8])) -> usize {
    let mut n = 0;
    for file in a.index().visible() {
        if is_ani(&file.path) {
            let bytes = a.read_entry(file.entry).unwrap();
            f(&file.path, &bytes);
            n += 1;
        }
    }
    n
}

#[test]
fn every_ani_of_the_us_client_reads_writes_back_and_has_no_error() {
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    let mut by_code: BTreeMap<&'static str, usize> = BTreeMap::new();
    let (mut errors, mut late_tracks, mut late_files) = (0usize, 0usize, 0usize);
    let (mut late_rot, mut late_trans, mut minus_one_tracks) = (0usize, 0usize, 0usize);
    let (mut first_minus_one, mut starts_at_minus_one): (BTreeSet<String>, BTreeSet<String>) = Default::default();
    let mut other_first_fields = BTreeSet::new();
    let n = each_ani(&a, |path, bytes| {
        let ani = Ani::parse(bytes, path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(ani.to_bytes() == bytes, "{path}: written back differently");
        let found = validate(&ani);
        errors += found.iter().filter(|i| i.severity == Severity::Error).count();
        let codes: BTreeSet<&str> = found.iter().map(|i| i.code).collect();
        for c in codes {
            *by_code.entry(c).or_default() += 1;
        }
        let late_r = ani.bones.iter().filter(|b| b.rot.first().is_some_and(|k| k.key > 0)).count();
        let late_t = ani.bones.iter().filter(|b| b.trans.first().is_some_and(|k| k.key > 0)).count();
        late_rot += late_r;
        late_trans += late_t;
        late_tracks += late_r + late_t;
        if late_r + late_t > 0 {
            late_files += 1;
        }
        match ani.first_field {
            -1 => {
                first_minus_one.insert(path.to_string());
            }
            0 => {}
            other => {
                other_first_fields.insert(other);
            }
        }
        minus_one_tracks += ani.bones.iter().filter(|b| b.rot.first().is_some_and(|k| k.key == -1)).count() + ani.bones.iter().filter(|b| b.trans.first().is_some_and(|k| k.key == -1)).count();
        if ani.bones.iter().any(|b| b.rot.first().is_some_and(|k| k.key == -1) || b.trans.first().is_some_and(|k| k.key == -1)) {
            starts_at_minus_one.insert(path.to_string());
        }
    });
    let nonunit_files = by_code.get(issue::QUAT_NOT_UNIT).copied().unwrap_or(0);
    println!("ANI files {n}; files per code {by_code:?}; late tracks {late_tracks} (rotation {late_rot}, translation {late_trans}) in {late_files} files");
    println!("first field -1 in {} files, other values {other_first_fields:?}; quat-not-unit files {nonunit_files} (printed, not asserted)", first_minus_one.len());
    assert_eq!(n, 3_509);
    assert_eq!(errors, 0, "no file has a validation error");
    assert_eq!(by_code.get(issue::MATRIX_NOT_FINITE).copied().unwrap_or(0), 20);
    assert_eq!(by_code.get(issue::KEY_BEFORE_START).copied().unwrap_or(0), 0);
    assert_eq!(by_code.get(issue::KEYS_CLOSE).copied().unwrap_or(0), 0);
    assert_eq!((late_tracks, late_files), (382, 254));
    assert_eq!((late_rot, late_trans), (382, 0), "every late track is a rotation track");
    assert_eq!(minus_one_tracks, 269, "tracks whose first key is -1");
    assert_eq!(by_code.get(issue::TRACK_STARTS_LATE).copied().unwrap_or(0), late_files);
    assert!(other_first_fields.is_empty());
    assert_eq!(first_minus_one.len(), 7);
    assert_eq!(first_minus_one, starts_at_minus_one, "the first field is -1 in exactly the files with a track starting at key -1");
}

#[test]
fn no_ani_holds_a_name_and_rigs_keep_their_own_reference_matrices() {
    let Some(dir) = env_client() else { return };
    let a = Archive::open(&dir).unwrap();
    // per rig: the number of files and the distinct skeletons (parents + matrix bits of every bone)
    let mut rigs: BTreeMap<&'static str, (usize, BTreeSet<Vec<u8>>)> = BTreeMap::new();
    each_ani(&a, |path, bytes| {
        let ani = Ani::parse(bytes, path).unwrap();
        // the layout has no name field: every byte is accounted for by the numbers read (the reader ends exactly at the last byte)
        assert_eq!(ani.to_bytes().len(), bytes.len());
        if let Some((rig, _)) = rig_of(path).rig {
            let mut sig = Vec::new();
            for b in &ani.bones {
                sig.extend_from_slice(&b.parent.to_le_bytes());
                for m in b.matrix {
                    sig.extend_from_slice(&m.to_bits().to_le_bytes());
                }
            }
            let e = rigs.entry(rig).or_default();
            e.0 += 1;
            e.1.insert(sig);
        }
    });
    let (mut files, mut distinct) = (0, 0);
    for (name, _) in RIGS {
        let (n, set) = rigs.get(name).cloned().unwrap_or_default();
        println!("rig {name}: {n} files, {} distinct skeletons", set.len());
        assert!(n > 0, "rig {name} has files");
        assert!(set.len() * 100 >= n * 80, "rig {name}: {} distinct skeletons in {n} files is under 80 %", set.len());
        files += n;
        distinct += set.len();
    }
    println!("all rigs: {files} files, {distinct} distinct skeletons");
}

#[test]
fn the_changed_animations_pass_the_rig_check_against_the_stock_files() {
    let (Some(dir), Some(orig)) = (env_client(), env_client_original()) else { return };
    let (now, stock) = (Archive::open(&dir).unwrap(), Archive::open(&orig).unwrap());
    let mut changed = Vec::new();
    each_ani(&now, |path, bytes| {
        let Some(e) = stock.index().get(path) else { return };
        let old = stock.read_entry(e).unwrap();
        if old != bytes {
            let (new, old) = (Ani::parse(bytes, path).unwrap(), Ani::parse(&old, path).unwrap());
            assert!(passes(&new, &old), "{path}: the changed file does not pass the rig check against its stock file");
            changed.push(path.to_string());
        }
    });
    println!("{} changed .ani files pass the rig check against the stock files", changed.len());
    assert_eq!(changed.len(), 2, "{changed:?}");
}
