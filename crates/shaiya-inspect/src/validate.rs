//! Checks on one file before it goes into an archive or a patch. Findings are advice: they warn and never refuse anything.
//!
//! What is checked, and only that: the SEED wrapper of `.SData` files (checksum, a second wrapper inside the first, plain versus
//! encrypted compared with the copy already in the client), whether the toolkit's own reader accepts a `.dds`/`.tga` header, and empty
//! files. Nothing here parses an SData layout, so no SData file can be reported for its contents.

use serde::Serialize;
use shaiya_archive::edit::Source;
use shaiya_clientdata::texture;
use shaiya_core::Job;
use shaiya_patch::BuildInput;
use std::path::Path;
use shaiya_seed as seed;

/// A finding's code (stable; printed by the CLI, in `--json` and in the Toolkit).
pub mod code {
    pub const SDATA_DOUBLE_ENCRYPTED: &str = "sdata-double-encrypted";
    pub const SDATA_CRC_BAD: &str = "sdata-crc-bad";
    pub const SDATA_STATE_DIFFERS: &str = "sdata-state-differs";
    pub const TEXTURE_UNREADABLE: &str = "texture-unreadable";
    pub const EMPTY_FILE: &str = "empty-file";
    /// Not a property of a file: the Toolkit's check did not finish, so the files are unchecked (it asks like a broken file).
    pub const CHECK_INCOMPLETE: &str = "check-incomplete";
    /// Every code, for tests and documentation.
    pub const ALL: [&str; 5] = [SDATA_DOUBLE_ENCRYPTED, SDATA_CRC_BAD, SDATA_STATE_DIFFERS, TEXTURE_UNREADABLE, EMPTY_FILE];
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// The archive path or file name the finding is about.
    pub path: String,
    pub code: &'static str,
    pub message: String,
}

impl Finding {
    /// The findings the Toolkit asks about once ("Add anyway" / Cancel) before it queues or builds: a double wrapper, a bad checksum, and a check that did not finish; every other finding only warns.
    pub fn asks(&self) -> bool {
        self.code == code::SDATA_DOUBLE_ENCRYPTED || self.code == code::SDATA_CRC_BAD || self.code == code::CHECK_INCOMPLETE
    }

    /// The finding that stands in for a check that did not finish (cancelled, failed or crashed): the files are unchecked, so the
    /// Toolkit asks before it queues or builds them.
    pub fn incomplete(reason: &str) -> Finding {
        Finding { path: "(all files)".to_string(), code: code::CHECK_INCOMPLETE, message: format!("the file check did not finish ({reason}); the files were not checked") }
    }

    /// `path: message [code]`, the line the CLI prints after `warning: `.
    pub fn line(&self) -> String {
        format!("{}: {} [{}]", self.path, self.message, self.code)
    }
}

fn last_segment(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn ext_is(name: &str, ext: &str) -> bool {
    last_segment(name).rsplit_once('.').is_some_and(|(_, e)| e.eq_ignore_ascii_case(ext))
}

pub fn is_sdata_name(name: &str) -> bool {
    ext_is(name, "sdata")
}

pub fn is_texture_name(name: &str) -> bool {
    ext_is(name, "dds") || ext_is(name, "tga")
}

/// Whether [`check_file`] has anything to say about a file of this name that needs its bytes (so callers can skip reading the rest).
pub fn wants_bytes(name: &str) -> bool {
    is_sdata_name(name) || is_texture_name(name)
}

/// Checks one file. `bytes` is the whole file as it would go in; `client_copy` is the file of the same name already in the client
/// (the whole file, SData only matters), when there is one. Returns nothing for a file that looks fine.
pub fn check_file(name: &str, bytes: &[u8], client_copy: Option<&[u8]>) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |code: &'static str, message: String| out.push(Finding { path: name.to_string(), code, message });
    if bytes.is_empty() {
        add(code::EMPTY_FILE, "the file is empty (0 bytes)".to_string());
        return out;
    }
    if is_sdata_name(name) {
        let encrypted = seed::is_encrypted(bytes);
        if encrypted {
            match seed::decrypt_unchecked(bytes, name) {
                Ok((plain, crc_ok)) => {
                    if !crc_ok {
                        add(code::SDATA_CRC_BAD, "the SEED checksum fails (damaged file, or a different key); some loaders ignore a bad checksum, others reject the file".to_string());
                    }
                    if seed::is_encrypted(&plain) {
                        add(
                            code::SDATA_DOUBLE_ENCRYPTED,
                            "the file is SEED-encrypted twice (its decrypted body carries the SEED signature again); a reader that removes one layer is left with encrypted bytes".to_string(),
                        );
                    }
                }
                Err(e) => add(code::SDATA_CRC_BAD, format!("the SEED wrapper is damaged ({e})")),
            }
        }
        if let Some(copy) = client_copy.filter(|c| !c.is_empty()) {
            let copy_encrypted = seed::is_encrypted(copy);
            if encrypted != copy_encrypted {
                let (this, that) = if encrypted { ("encrypted", "plain") } else { ("plain", "encrypted") };
                add(code::SDATA_STATE_DIFFERS, format!("this file is {this} but the copy in the client is {that}; check which form your client expects"));
            }
        }
    } else if is_texture_name(name) {
        if let Err(e) = texture::decode(bytes, name) {
            add(code::TEXTURE_UNREADABLE, format!("the toolkit cannot read this texture ({e}); the game may still accept it"));
        }
    }
    out
}

/// Files larger than this are not read for the checks (a texture or SData of this size is not a client file).
const READ_LIMIT: u64 = 512 << 20;

/// [`check_file`] for a file that is not in memory: reads it only when its name asks for a content check, and reports an empty one
/// from its length. A source that cannot be read gives no findings (the build or the edit reports the read error itself).
pub fn check_source(name: &str, src: &Source, client_copy: Option<&[u8]>) -> Vec<Finding> {
    let Ok(len) = src.len() else { return Vec::new() };
    if len == 0 {
        return check_file(name, &[], client_copy);
    }
    if !wants_bytes(name) || len > READ_LIMIT {
        return Vec::new();
    }
    let mut bytes = Vec::with_capacity(len as usize);
    if src.copy_to(&mut bytes, &Job::silent()).is_err() {
        return Vec::new();
    }
    check_file(name, &bytes, client_copy)
}

/// [`check_source`] for a loose file on disk.
pub fn check_path(name: &str, path: &Path, client_copy: Option<&[u8]>) -> Vec<Finding> {
    check_source(name, &Source::File(path.to_path_buf()), client_copy)
}

/// Findings for everything a patch would carry (archive files and loose files). No client copy is known here, so
/// `sdata-state-differs` is never reported for a patch.
pub fn check_input(input: &BuildInput) -> Vec<Finding> {
    let mut out = Vec::new();
    for (path, src, _) in &input.files {
        out.extend(check_source(path, src, None));
    }
    for (name, file) in &input.loose {
        out.extend(check_path(name, file, None));
    }
    out
}

/// Findings for the files inside a finished patch (archive files and loose files), read from the patch without unpacking it to disk.
/// No client copy is known, so `sdata-state-differs` is never reported. Empty files are reported from the patch's own listing.
pub fn check_patch(patch: &Path, job: &Job) -> shaiya_core::Result<Vec<Finding>> {
    let mut out = Vec::new();
    let c = shaiya_patch::contents(patch)?;
    for f in &c.files {
        if f.length == 0 {
            out.extend(check_file(&f.path, &[], None));
        }
    }
    for (name, size) in &c.loose {
        if *size == 0 {
            out.extend(check_file(name, &[], None));
        }
    }
    shaiya_patch::read_files(patch, READ_LIMIT, &wants_bytes, &mut |name, bytes| out.extend(check_file(name, bytes, None)), job)?;
    Ok(out)
}

/// Findings that make the Toolkit ask once before going on.
pub fn any_asks(findings: &[Finding]) -> bool {
    findings.iter().any(Finding::asks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::tga_stub;

    fn codes(f: &[Finding]) -> Vec<&'static str> {
        f.iter().map(|x| x.code).collect()
    }

    fn payload() -> Vec<u8> {
        (0..300u32).map(|i| (i * 13 + 5) as u8).collect()
    }

    #[test]
    fn an_unfinished_check_asks_and_names_itself() {
        let f = Finding::incomplete("cancelled");
        assert!(f.asks() && any_asks(std::slice::from_ref(&f)));
        assert_eq!(f.code, code::CHECK_INCOMPLETE);
        assert!(f.line().contains("did not finish") && f.line().contains("[check-incomplete]"), "{}", f.line());
        assert!(!code::ALL.contains(&code::CHECK_INCOMPLETE), "it is not a finding about a file");
    }

    #[test]
    fn a_normal_encrypted_and_a_normal_plain_sdata_have_no_findings() {
        let plain = payload();
        let enc = seed::encrypt(&plain);
        assert!(check_file("data/Item.SData", &enc, None).is_empty());
        assert!(check_file("data/Item.SData", &plain, None).is_empty());
        assert!(check_file("Item.SData", &enc, Some(&enc)).is_empty());
        assert!(check_file("Item.SData", &plain, Some(&plain)).is_empty());
    }

    #[test]
    fn encrypting_twice_is_double_encrypted() {
        let once = seed::encrypt(&payload());
        let twice = seed::encrypt(&once);
        assert_eq!(seed::decrypt(&twice, "t").unwrap(), once, "one decrypt gives the singly wrapped file back");
        let f = check_file("Item.SData", &twice, None);
        assert_eq!(codes(&f), [code::SDATA_DOUBLE_ENCRYPTED]);
        assert!(f[0].asks());
        // three layers are still reported once
        let thrice = seed::encrypt(&twice);
        assert_eq!(codes(&check_file("Item.sdata", &thrice, None)), [code::SDATA_DOUBLE_ENCRYPTED]);
    }

    #[test]
    fn a_finished_patch_is_checked_from_its_archive_and_loose_files() {
        let t = shaiya_testkit::TempDir::new("check-patch");
        let files = t.path().join("files");
        std::fs::create_dir_all(files.join("Item")).unwrap();
        std::fs::create_dir_all(files.join("ui")).unwrap();
        std::fs::write(files.join("Item/Item.SData"), seed::encrypt(&seed::encrypt(&payload()))).unwrap();
        std::fs::write(files.join("Item/Skill.SData"), seed::encrypt(&payload())).unwrap();
        std::fs::write(files.join("ui/broken.dds"), b"DDS not really").unwrap();
        std::fs::write(files.join("ui/ok.tga"), tga_stub(4, 4)).unwrap();
        let loose = t.path().join("loose");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::write(loose.join("Other.SData"), seed::encrypt(&seed::encrypt(&payload()))).unwrap();
        let patch = t.path().join("ps0001.patch");
        shaiya_patch::build(Some(&files), Some(&loose), None, &patch).unwrap();
        let f = check_patch(&patch, &Job::silent()).unwrap();
        let mut got: Vec<(String, &str)> = f.iter().map(|x| (x.path.to_lowercase(), x.code)).collect();
        got.sort();
        assert_eq!(
            got,
            [
                ("item/item.sdata".to_string(), code::SDATA_DOUBLE_ENCRYPTED),
                ("other.sdata".to_string(), code::SDATA_DOUBLE_ENCRYPTED),
                ("ui/broken.dds".to_string(), code::TEXTURE_UNREADABLE)
            ]
        );
    }

    #[test]
    fn a_flipped_byte_is_crc_bad_and_a_cut_wrapper_too() {
        let mut enc = seed::encrypt(&payload());
        let last = enc.len() - 1;
        enc[last] ^= 0x40;
        // the damaged last block is the one holding plaintext, so the checksum fails
        let f = check_file("Item.SData", &enc, None);
        assert_eq!(codes(&f), [code::SDATA_CRC_BAD]);
        assert!(f[0].asks());
        let mut crc_field = seed::encrypt(&payload());
        crc_field[40] ^= 1;
        assert_eq!(codes(&check_file("Item.SData", &crc_field, None)), [code::SDATA_CRC_BAD]);
        let ok = seed::encrypt(&payload());
        assert_eq!(codes(&check_file("Item.SData", &ok[..ok.len() - 5], None)), [code::SDATA_CRC_BAD]);
    }

    #[test]
    fn plain_against_an_encrypted_copy_and_the_reverse_differ() {
        let plain = payload();
        let enc = seed::encrypt(&plain);
        let f = check_file("Item.SData", &plain, Some(&enc));
        assert_eq!(codes(&f), [code::SDATA_STATE_DIFFERS]);
        assert!(!f[0].asks(), "state differences only warn");
        assert!(f[0].message.contains("plain") && f[0].message.contains("encrypted"));
        assert_eq!(codes(&check_file("Item.SData", &enc, Some(&plain))), [code::SDATA_STATE_DIFFERS]);
        // an empty copy says nothing about the expected form
        assert!(check_file("Item.SData", &plain, Some(&[])).is_empty());
        // the comparison is for SData only
        assert!(check_file("notes.txt", &plain, Some(&enc)).is_empty());
    }

    #[test]
    fn textures_are_checked_with_the_toolkits_reader() {
        assert!(check_file("a/b.tga", &tga_stub(4, 4), None).is_empty());
        assert!(check_file("a/B.TGA", &tga_stub(4, 4), None).is_empty());
        let tga = tga_stub(4, 4);
        assert_eq!(codes(&check_file("a/b.tga", &tga[..30], None)), [code::TEXTURE_UNREADABLE]);
        // a DDS cut after its magic
        assert_eq!(codes(&check_file("a/b.dds", b"DDS \x7c\0\0\0", None)), [code::TEXTURE_UNREADABLE]);
        assert_eq!(codes(&check_file("a/b.dds", b"not a dds at all, just bytes", None)), [code::TEXTURE_UNREADABLE]);
        // other names are not looked at
        assert!(check_file("a/b.txt", b"not a dds at all", None).is_empty());
        assert!(check_file("a/b.bmp", b"junk", None).is_empty());
    }

    fn dds(fourcc: &[u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&4u32.to_le_bytes()); // height
        b[16..20].copy_from_slice(&4u32.to_le_bytes()); // width
        b[76..80].copy_from_slice(&32u32.to_le_bytes()); // pixel format size
        b[80..84].copy_from_slice(&4u32.to_le_bytes()); // FOURCC flag
        b[84..88].copy_from_slice(fourcc);
        b.extend_from_slice(&[0u8; 16]); // one block
        b
    }

    #[test]
    fn dxt1_to_5_pass_and_another_fourcc_is_refused_by_the_reader_so_it_is_reported() {
        for ok in [b"DXT1", b"DXT2", b"DXT3", b"DXT4", b"DXT5"] {
            assert!(check_file("a.dds", &dds(ok), None).is_empty(), "{ok:?}");
        }
        for refused in [b"ATI1", b"BC7 "] {
            let f = check_file("a.dds", &dds(refused), None);
            assert_eq!(codes(&f), [code::TEXTURE_UNREADABLE], "{refused:?}");
            assert!(f[0].message.contains("game may still accept it"));
        }
    }

    #[test]
    fn an_empty_file_is_reported_once_whatever_its_kind() {
        for n in ["a.txt", "Item.SData", "a.dds", "a.tga"] {
            assert_eq!(codes(&check_file(n, &[], None)), [code::EMPTY_FILE], "{n}");
        }
    }

    #[test]
    fn names_are_matched_by_extension_on_the_last_segment() {
        assert!(is_sdata_name("data\\sub.d\\Item.SDATA"));
        assert!(!is_sdata_name("Item.SData.bak"));
        assert!(!is_sdata_name("sdata"));
        assert!(is_texture_name("x.Dds") && is_texture_name("dir.tga/x.tga"));
        assert!(!is_texture_name("dir.tga/x"));
        assert!(wants_bytes("Item.SData") && wants_bytes("x.dds") && !wants_bytes("x.txt"));
    }

    #[test]
    fn findings_serialise_with_their_code_and_have_a_printable_line() {
        let f = check_file("a.tga", &[1, 2, 3], None);
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v[0]["code"], "texture-unreadable");
        assert_eq!(v[0]["path"], "a.tga");
        assert!(f[0].line().starts_with("a.tga: "));
        assert!(f[0].line().ends_with("[texture-unreadable]"));
        assert_eq!(code::ALL.len(), 5);
    }
}
