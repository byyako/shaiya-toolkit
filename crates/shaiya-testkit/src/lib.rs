//! Test helpers for the Shaiya toolkit (dev-dependency only). Std only, independent of the code it tests: it writes the formats by hand,
//! so it is an oracle, not a mirror. It returns bytes and paths, never toolkit types. No game data.

pub mod anim;
pub mod demo;
pub use demo::demo_client;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh empty folder under the system temp folder, removed (with everything in it) on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let tag: String = tag.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
        let path = std::env::temp_dir().join(format!("shaiya-test-{tag}-{}-{n}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct FileSpec {
    path: Vec<u8>,
    data: Vec<u8>,
    version: i32,
}

/// Builds a `data.sah` / `data.saf` pair by hand from the documented layout:
/// `"SAH"`, i32 version, u32 file count, 40 zero bytes, then the root directory
/// (`str name; u32 nFiles; {str name; u64 offset; u32 length; i32 version}; u32 nDirs; dirs...`), then the free list
/// (`u32 count; entries`) and the trailer. A `str` is an i32 length including the NUL, then the bytes and the NUL.
pub struct SahBuilder {
    version: i32,
    files: Vec<FileSpec>,
    free: Vec<(Vec<u8>, u64, u32)>,
    trailer: Vec<u8>,
}

#[derive(Default)]
struct Node {
    name: Vec<u8>,
    files: Vec<(Vec<u8>, u64, u32, i32)>,
    dirs: Vec<Node>,
}

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&((s.len() + 1) as i32).to_le_bytes());
    out.extend_from_slice(s);
    out.push(0);
}

fn put_entry(out: &mut Vec<u8>, name: &[u8], offset: u64, length: u32, version: i32) {
    put_str(out, name);
    out.extend_from_slice(&offset.to_le_bytes());
    out.extend_from_slice(&length.to_le_bytes());
    out.extend_from_slice(&version.to_le_bytes());
}

fn write_node(out: &mut Vec<u8>, n: &Node) {
    put_str(out, &n.name);
    out.extend_from_slice(&(n.files.len() as u32).to_le_bytes());
    for (name, offset, length, version) in &n.files {
        put_entry(out, name, *offset, *length, *version);
    }
    out.extend_from_slice(&(n.dirs.len() as u32).to_le_bytes());
    for d in &n.dirs {
        write_node(out, d);
    }
}

impl SahBuilder {
    pub fn new(version: i32) -> Self {
        SahBuilder { version, files: Vec::new(), free: Vec::new(), trailer: vec![0; 4] }
    }

    /// Adds a file; nested folders come from `/` in the raw name bytes (cp949 test names included). File version stamp 1.
    pub fn file(self, path_cp949: &[u8], data: &[u8]) -> Self {
        self.file_versioned(path_cp949, data, 1)
    }

    pub fn file_versioned(mut self, path_cp949: &[u8], data: &[u8], version: i32) -> Self {
        self.files.push(FileSpec { path: path_cp949.to_vec(), data: data.to_vec(), version });
        self
    }

    pub fn free(mut self, name: &[u8], offset: u64, length: u32) -> Self {
        self.free.push((name.to_vec(), offset, length));
        self
    }

    pub fn trailer(mut self, bytes: &[u8]) -> Self {
        self.trailer = bytes.to_vec();
        self
    }

    /// (sah, saf). File data is stored back to back in the order the files were added.
    pub fn build(&self) -> (Vec<u8>, Vec<u8>) {
        let mut saf = Vec::new();
        let mut root = Node::default();
        for f in &self.files {
            let offset = saf.len() as u64;
            saf.extend_from_slice(&f.data);
            let mut parts: Vec<&[u8]> = f.path.split(|&b| b == b'/').filter(|p| !p.is_empty()).collect();
            let name = parts.pop().expect("a file needs a name");
            let mut node = &mut root;
            for part in parts {
                let pos = match node.dirs.iter().position(|d| d.name == part) {
                    Some(p) => p,
                    None => {
                        node.dirs.push(Node { name: part.to_vec(), ..Node::default() });
                        node.dirs.len() - 1
                    }
                };
                node = &mut node.dirs[pos];
            }
            node.files.push((name.to_vec(), offset, f.data.len() as u32, f.version));
        }
        let mut sah = Vec::new();
        sah.extend_from_slice(b"SAH");
        sah.extend_from_slice(&self.version.to_le_bytes());
        sah.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        sah.extend_from_slice(&[0u8; 40]);
        write_node(&mut sah, &root);
        sah.extend_from_slice(&(self.free.len() as u32).to_le_bytes());
        for (name, offset, length) in &self.free {
            put_entry(&mut sah, name, *offset, *length, 0);
        }
        sah.extend_from_slice(&self.trailer);
        (sah, saf)
    }

    /// Writes `data.sah` and `data.saf` into `dir`; returns their paths.
    pub fn write_to(&self, dir: &Path) -> (PathBuf, PathBuf) {
        let (sah, saf) = self.build();
        let (p_sah, p_saf) = (dir.join("data.sah"), dir.join("data.saf"));
        std::fs::write(&p_sah, sah).expect("write data.sah");
        std::fs::write(&p_saf, saf).expect("write data.saf");
        (p_sah, p_saf)
    }
}

/// A 512-byte MZ/PE header (not a program): `MZ`, `e_lfanew` = 0x80, `PE\0\0`, the COFF header with the given machine and link
/// timestamp, zero sections, and an optional-header magic. Enough for tools that read the machine type and timestamp.
pub fn pe_stub(machine: u16, timestamp: u32) -> Vec<u8> {
    let mut b = vec![0u8; 512];
    b[0] = b'M';
    b[1] = b'Z';
    b[0x3C..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    b[0x80..0x84].copy_from_slice(b"PE\0\0");
    b[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    b[0x86..0x88].copy_from_slice(&0u16.to_le_bytes()); // sections
    b[0x88..0x8C].copy_from_slice(&timestamp.to_le_bytes());
    let opt_size: u16 = if machine == 0x8664 { 240 } else { 224 };
    b[0x94..0x96].copy_from_slice(&opt_size.to_le_bytes());
    b[0x96..0x98].copy_from_slice(&0x0102u16.to_le_bytes()); // executable, 32-bit machine
    let magic: u16 = if machine == 0x8664 { 0x20B } else { 0x10B };
    b[0x98..0x9A].copy_from_slice(&magic.to_le_bytes());
    b
}

/// The first `at` bytes (all of them when `at` is larger).
pub fn truncate(bytes: &[u8], at: usize) -> Vec<u8> {
    bytes[..at.min(bytes.len())].to_vec()
}

/// A copy with one bit (the lowest) of the byte at `at` flipped; unchanged when `at` is out of range.
pub fn flip(bytes: &[u8], at: usize) -> Vec<u8> {
    let mut v = bytes.to_vec();
    if let Some(b) = v.get_mut(at) {
        *b ^= 1;
    }
    v
}

/// The path in `var`, or `None` (printing `skipped: <var> not set`) when it is unset or empty. When `var` is listed in `require`
/// (a comma-separated list of names, the value of `SHAIYA_OPTIN_REQUIRE`), a missing variable panics instead, so a test that would
/// skip silently fails.
pub fn env_path_with(var: &str, require: &str) -> Option<PathBuf> {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => {
            if require.split(',').any(|n| n.trim() == var) {
                panic!("{var} is required by SHAIYA_OPTIN_REQUIRE but not set");
            }
            eprintln!("skipped: {var} not set");
            None
        }
    }
}

fn env_path(var: &str) -> Option<PathBuf> {
    let require = std::env::var("SHAIYA_OPTIN_REQUIRE").unwrap_or_default();
    env_path_with(var, &require)
}

/// Opt-in data for any other variable (for example `SVMAP_ORIGINALS`), with the same skip and `SHAIYA_OPTIN_REQUIRE` rules.
pub fn env_var_path(var: &str) -> Option<PathBuf> {
    env_path(var)
}

/// An optional integer from the environment (no skip message): `None` when unset, empty or not a number.
pub fn env_usize(var: &str) -> Option<usize> {
    std::env::var(var).ok()?.trim().parse().ok()
}

/// Opt-in data. Prints "skipped: SHAIYA_CLIENT not set" to stderr and returns None when unset.
pub fn env_client() -> Option<PathBuf> {
    env_path("SHAIYA_CLIENT")
}

/// Opt-in data: the version-matrix folder (`SHAIYA_TEST_CLIENTS`).
pub fn env_test_clients() -> Option<PathBuf> {
    env_path("SHAIYA_TEST_CLIENTS")
}


/// Opt-in scratch space for tests that write gigabytes (`SHAIYA_SCRATCH`): a folder outside every repo.
pub fn env_scratch() -> Option<PathBuf> {
    env_path("SHAIYA_SCRATCH")
}

/// Opt-in data: an archive holding the stock files (`SHAIYA_CLIENT_ORIGINAL`): a `.sah` file (its `.saf` is the sibling with the same
/// stem) or a folder with `data.sah`. Read only.
pub fn env_client_original() -> Option<PathBuf> {
    env_path("SHAIYA_CLIENT_ORIGINAL")
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// A ZIP file written by hand: stored (uncompressed) entries, CRC-32 computed here, times fixed at 1980-01-01. An oracle independent
/// of the `zip` crate. Entry names are written exactly as given (so `../x` and case twins can be tested).
#[derive(Default)]
pub struct ZipBuilder {
    entries: Vec<(String, Vec<u8>)>,
}

impl ZipBuilder {
    pub fn new() -> ZipBuilder {
        ZipBuilder::default()
    }

    pub fn add(mut self, name: &str, data: &[u8]) -> ZipBuilder {
        self.entries.push((name.to_string(), data.to_vec()));
        self
    }

    pub fn build(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in &self.entries {
            let offset = out.len() as u32;
            let crc = crc32(data);
            let (n, size) = (name.as_bytes(), data.len() as u32);
            let mut common = Vec::new();
            common.extend_from_slice(&20u16.to_le_bytes()); // version needed
            common.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
            common.extend_from_slice(&0u16.to_le_bytes()); // stored
            common.extend_from_slice(&0u16.to_le_bytes()); // time 00:00:00
            common.extend_from_slice(&0x0021u16.to_le_bytes()); // date 1980-01-01
            common.extend_from_slice(&crc.to_le_bytes());
            common.extend_from_slice(&size.to_le_bytes());
            common.extend_from_slice(&size.to_le_bytes());
            common.extend_from_slice(&(n.len() as u16).to_le_bytes());
            common.extend_from_slice(&0u16.to_le_bytes()); // extra length
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&common);
            out.extend_from_slice(n);
            out.extend_from_slice(data);
            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&common);
            central.extend_from_slice(&0u16.to_le_bytes()); // comment length
            central.extend_from_slice(&0u16.to_le_bytes()); // disk
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(n);
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    pub fn write_to(&self, path: &Path) {
        std::fs::write(path, self.build()).expect("write zip");
    }
}

/// An uncompressed 24-bit true-colour TGA (type 2, bottom-up) with a red-to-blue gradient.
pub fn tga_stub(w: u16, h: u16) -> Vec<u8> {
    let mut b = vec![0u8; 18];
    b[2] = 2;
    b[12..14].copy_from_slice(&w.to_le_bytes());
    b[14..16].copy_from_slice(&h.to_le_bytes());
    b[16] = 24;
    for y in 0..h {
        for x in 0..w {
            // BGR order
            b.extend_from_slice(&[(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 200]);
        }
    }
    b
}

/// An uncompressed 24-bit BMP (bottom-up, rows padded to 4 bytes).
pub fn bmp_stub(w: u32, h: u32) -> Vec<u8> {
    let stride = (w * 3).div_ceil(4) * 4;
    let size = 54 + stride * h;
    let mut b = Vec::with_capacity(size as usize);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&size.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0u8; 24]); // compression 0, image size, resolution, colours
    for y in 0..h {
        let mut row = Vec::new();
        for x in 0..w {
            row.extend_from_slice(&[(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 100]);
        }
        row.resize(stride as usize, 0);
        b.extend_from_slice(&row);
    }
    b
}

pub const TEST_CLIENT_IDS: [&str; 8] = ["ru-ep3", "ru-ep45", "ru-ep54", "fr-ep54", "phoenix-ep63", "kr-ep8", "fawkes-v68", "cn-ep4"];

#[cfg(test)]
mod tests {
    use super::*;

    fn probe() -> String {
        format!("SHAIYA_TESTKIT_PROBE_{}", std::process::id())
    }

    #[test]
    #[should_panic(expected = "is required by SHAIYA_OPTIN_REQUIRE but not set")]
    fn a_required_variable_that_is_missing_panics() {
        let var = probe();
        let _ = env_path_with(&var, &format!("SHAIYA_CLIENT, {var}"));
    }

    #[test]
    fn an_unlisted_missing_variable_skips() {
        let var = probe();
        assert_eq!(env_path_with(&var, ""), None);
        assert_eq!(env_path_with(&var, "SHAIYA_CLIENT,SHAIYA_SCRATCH"), None);
    }

    fn le32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn temp_dirs_are_unique_and_removed_on_drop() {
        let (a, b) = (TempDir::new("a b"), TempDir::new("a b"));
        assert_ne!(a.path(), b.path());
        assert!(a.path().is_dir());
        let p = a.path().to_path_buf();
        std::fs::write(p.join("x"), b"1").unwrap();
        drop(a);
        assert!(!p.exists());
    }

    #[test]
    fn empty_sah_has_the_documented_bytes() {
        let (sah, saf) = SahBuilder::new(0).build();
        assert!(saf.is_empty());
        let mut want = b"SAH".to_vec();
        want.extend_from_slice(&0i32.to_le_bytes()); // version
        want.extend_from_slice(&0u32.to_le_bytes()); // file count
        want.extend_from_slice(&[0; 40]);
        want.extend_from_slice(&[1, 0, 0, 0, 0]); // root name: length 1, NUL
        want.extend_from_slice(&[0; 4]); // no files
        want.extend_from_slice(&[0; 4]); // no folders
        want.extend_from_slice(&[0; 4]); // free list count
        want.extend_from_slice(&[0; 4]); // trailer
        assert_eq!(sah, want);
    }

    #[test]
    fn nested_files_and_offsets_follow_the_layout() {
        let (sah, saf) = SahBuilder::new(1).file(b"a/b.txt", b"hello").file(b"a/c/d.bin", &[9, 9]).file(b"top", b"T").build();
        assert_eq!(saf, b"hello\x09\x09T");
        assert_eq!(&sah[..3], b"SAH");
        assert_eq!(le32(&sah, 3), 1);
        assert_eq!(le32(&sah, 7), 3);
        // root: name(5 bytes) then 1 file ("top")
        let root = 51;
        assert_eq!(&sah[root..root + 5], &[1, 0, 0, 0, 0]);
        assert_eq!(le32(&sah, root + 5), 1);
        // "top": length 4 incl NUL, bytes, NUL, offset 7, length 1, version 1
        let e = root + 9;
        assert_eq!(le32(&sah, e), 4);
        assert_eq!(&sah[e + 4..e + 8], b"top\0");
        assert_eq!(u64::from_le_bytes(sah[e + 8..e + 16].try_into().unwrap()), 7);
        assert_eq!(le32(&sah, e + 16), 1);
        assert_eq!(le32(&sah, e + 20), 1);
        // one folder "a"
        assert_eq!(le32(&sah, e + 24), 1);
        let a = e + 28;
        assert_eq!(&sah[a..a + 6], &[2, 0, 0, 0, b'a', 0]);
        assert_eq!(le32(&sah, a + 6), 1); // b.txt
        // the file ends the stream with free list 0 and trailer 4
        assert_eq!(&sah[sah.len() - 8..], &[0; 8]);
    }

    #[test]
    fn cp949_names_free_entries_and_trailer_are_written_raw() {
        let name = [0xB8, 0xF3, 0xBD, 0xBA, 0xC5, 0xCD, b'/', b'x'];
        let (sah, _) = SahBuilder::new(0).file(&name, b"1").free(b"old", 100, 20).trailer(&[1, 2, 3, 4, 5]).build();
        assert!(sah.windows(7).any(|w| w == [7, 0, 0, 0, 0xB8, 0xF3, 0xBD]), "folder name keeps its bytes");
        assert!(sah.ends_with(&[1, 2, 3, 4, 5]));
        let free_at = sah.len() - 5 - (4 + 4 + 8 + 4 + 4);
        assert_eq!(le32(&sah, free_at - 4), 1, "free count");
        assert_eq!(&sah[free_at + 4..free_at + 8], b"old\0");
    }

    #[test]
    fn write_to_creates_both_files() {
        let d = TempDir::new("sah");
        let (sah, saf) = SahBuilder::new(0).file(b"f", b"abc").write_to(d.path());
        assert_eq!(std::fs::read(saf).unwrap(), b"abc");
        assert!(std::fs::read(sah).unwrap().starts_with(b"SAH"));
    }

    #[test]
    fn pe_stub_fields() {
        let b = pe_stub(0x014C, 0x4BD4_E2AC);
        assert_eq!(b.len(), 512);
        assert_eq!(&b[..2], b"MZ");
        let pe = le32(&b, 0x3C) as usize;
        assert_eq!(&b[pe..pe + 4], b"PE\0\0");
        assert_eq!(u16::from_le_bytes([b[pe + 4], b[pe + 5]]), 0x014C);
        assert_eq!(le32(&b, pe + 8), 0x4BD4_E2AC);
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn zip_builder_writes_the_documented_structure() {
        let z = ZipBuilder::new().add("a.txt", b"hello").add("../x", b"").build();
        assert_eq!(le32(&z, 0), 0x0403_4b50);
        // first local header: stored, crc of "hello", sizes 5, name length 5
        assert_eq!(le32(&z, 14), crc32(b"hello"));
        assert_eq!((le32(&z, 18), le32(&z, 22)), (5, 5));
        assert_eq!(&z[30..35], b"a.txt");
        assert_eq!(&z[35..40], b"hello");
        // end of central directory: two entries, and it points at a central directory header
        let e = z.len() - 22;
        assert_eq!(le32(&z, e), 0x0605_4b50);
        assert_eq!(u16::from_le_bytes([z[e + 10], z[e + 11]]), 2);
        let cd = le32(&z, e + 16) as usize;
        assert_eq!(le32(&z, cd), 0x0201_4b50);
        assert_eq!(le32(&z, e + 12) as usize, e - cd);
        assert!(z.windows(4).any(|w| w == b"../x"), "names are written as given");
    }

    #[test]
    fn image_stubs_have_valid_headers() {
        let t = tga_stub(4, 2);
        assert_eq!((t[2], t[16], u16::from_le_bytes([t[12], t[13]]), u16::from_le_bytes([t[14], t[15]])), (2, 24, 4, 2));
        assert_eq!(t.len(), 18 + 4 * 2 * 3);
        let b = bmp_stub(3, 2);
        assert_eq!(&b[..2], b"BM");
        assert_eq!(le32(&b, 2) as usize, b.len());
        assert_eq!(b.len(), 54 + 12 * 2, "3 pixels of 3 bytes pad to 12");
    }

    #[test]
    fn mutators_and_env_helpers() {
        let v = [1u8, 2, 3, 4];
        assert_eq!(truncate(&v, 2), vec![1, 2]);
        assert_eq!(truncate(&v, 99), v.to_vec());
        assert_eq!(flip(&v, 1), vec![1, 3, 3, 4]);
        assert_eq!(flip(&v, 9), v.to_vec());
        assert_eq!(TEST_CLIENT_IDS.len(), 8);
        // an unset variable is a skip, not a failure
        assert_eq!(env_path("SHAIYA_TESTKIT_SURELY_UNSET_VARIABLE"), None);
    }
}
