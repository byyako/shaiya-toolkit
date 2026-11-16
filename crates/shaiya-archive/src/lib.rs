//! Shaiya client archives: `data.sah` (index) + `data.saf` (blob).
//!
//! Moved from `rust/archive/src/lib.rs` of the Shaiya EP4 Client repo; the private reader is replaced by `shaiya_core::bin::Reader`.
//! - index: `"SAH"`, i32 version, u32 file count, 40 zero bytes, then a directory tree
//!   (`str name; u32 nFiles; {str name; i64 offset; u32 length; i32 version}; u32 nDirs; dirs...`),
//!   then a free list (`u32 count; entries shaped like files`) and 4 trailing bytes;
//! - strings are `i32 length` (including the NUL) + bytes, cp949/ASCII;
//! - the client finds names by the djb2 hash of the lower-cased name, and with duplicate names the later entry wins, so this crate
//!   replaces in place instead of adding a second entry.
//!
//! [`Archive`] reads files from an archive on disk; [`compact`] rewrites `data.saf` without dead space. Patches live in `shaiya-patch`.

pub mod check;
pub mod client;
pub mod compact;
pub mod diff;
pub mod edit;
pub mod extract;
pub mod journal;
pub mod layout;
pub mod pack;
#[cfg(feature = "serde")]
pub(crate) mod ser;
pub mod swap;

use shaiya_core::bin::Reader;
use shaiya_core::glob::Filter;
use shaiya_core::source::ByteSource;
use shaiya_core::{Job, Unit};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use shaiya_core::{Error, Result};

/// Hash the client uses to look names up (`FUN_0040a490`): djb2 over the name with ASCII A-Z lower-cased, characters taken as signed
/// bytes.
pub fn name_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0x1505;
    for &b in name {
        let mut c = b as i8 as i32;
        if (0x41..0x5b).contains(&c) {
            c += 0x20;
        }
        h = h.wrapping_mul(33).wrapping_add(c as u32);
    }
    h
}

/// A name as the index stores it: code page 949 (Korean names such as `Item/3DO/10051원.3do`; ASCII is unchanged). A character 949
/// cannot hold is kept as UTF-8, which simply matches nothing in a real archive.
pub fn encode_name(name: &str) -> Vec<u8> {
    let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(name);
    if unmappable {
        name.as_bytes().to_vec()
    } else {
        bytes.into_owned()
    }
}

/// The reverse of [`encode_name`], for paths shown to people and used in patches (display only).
pub fn decode_name(raw: &[u8]) -> String {
    encoding_rs::EUC_KR.decode_without_bom_handling(raw).0.into_owned()
}

/// cp949 encoding that fails instead of falling back to UTF-8 (pack, add and patch build use it; [`encode_name`] keeps its T0 behaviour).
pub fn encode_name_strict(name: &str) -> Result<Vec<u8>> {
    let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(name);
    if unmappable {
        let bad = name.chars().find(|c| {
            let mut b = [0u8; 4];
            encoding_rs::EUC_KR.encode(c.encode_utf8(&mut b)).2
        });
        return Err(Error::format("name", None, format!("{name:?}: character {:?} cannot be stored in an archive name (code page 949)", bad.unwrap_or('?'))));
    }
    Ok(bytes.into_owned())
}

/// ASCII-lower-cased raw path: two visible entries with equal keys are the same file to the client (used by diff and patch verify).
pub fn path_key(raw_path: &[u8]) -> Vec<u8> {
    raw_path.to_ascii_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Entry {
    /// Raw name bytes (no NUL).
    #[cfg_attr(feature = "serde", serde(serialize_with = "ser::name"))]
    pub name: Vec<u8>,
    pub offset: u64,
    pub length: u32,
    /// Unix time the file was packed.
    pub version: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dir {
    pub name: Vec<u8>,
    pub files: Vec<Entry>,
    pub dirs: Vec<Dir>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    pub version: i32,
    pub root: Dir,
    /// "rest space" list used by the original updater; the client ignores it.
    pub free: Vec<Entry>,
    pub trailer: Vec<u8>,
}

/// Smallest possible entry (`str` of one NUL, offset, length, version) and directory (name, two counts).
const MIN_ENTRY: usize = 4 + 1 + 8 + 4 + 4;
const MIN_DIR: usize = 4 + 1 + 4 + 4;

fn read_string(r: &mut Reader) -> Result<Vec<u8>> {
    let at = r.pos();
    let n = r.i32("name length")?;
    if !(0..=4096).contains(&n) {
        r.seek(at)?;
        return Err(r.fail(format!("bad name length {n}")));
    }
    let raw = r.bytes(n as usize, "name")?;
    Ok(raw.split(|&b| b == 0).next().unwrap_or(&[]).to_vec())
}

fn read_entry(r: &mut Reader) -> Result<Entry> {
    let name = read_string(r)?;
    let offset = r.u64("entry offset")?;
    let length = r.u32("entry length")?;
    let version = r.i32("entry version")?;
    Ok(Entry { name, offset, length, version })
}

fn read_dir(r: &mut Reader, depth: u32) -> Result<Dir> {
    if depth > 64 {
        return Err(r.fail("directory tree too deep"));
    }
    let name = read_string(r)?;
    let nf = r.count(MIN_ENTRY, usize::MAX, "file count")?;
    let mut files = Vec::with_capacity(nf);
    for _ in 0..nf {
        files.push(read_entry(r)?);
    }
    let nd = r.count(MIN_DIR, usize::MAX, "folder count")?;
    let mut dirs = Vec::with_capacity(nd);
    for _ in 0..nd {
        dirs.push(read_dir(r, depth + 1)?);
    }
    Ok(Dir { name, files, dirs })
}

fn put_str(out: &mut Vec<u8>, s: &[u8]) {
    out.extend_from_slice(&((s.len() + 1) as i32).to_le_bytes());
    out.extend_from_slice(s);
    out.push(0);
}

fn put_entry(out: &mut Vec<u8>, e: &Entry) {
    put_str(out, &e.name);
    out.extend_from_slice(&e.offset.to_le_bytes());
    out.extend_from_slice(&e.length.to_le_bytes());
    out.extend_from_slice(&e.version.to_le_bytes());
}

impl Dir {
    fn write(&self, out: &mut Vec<u8>) {
        put_str(out, &self.name);
        out.extend_from_slice(&(self.files.len() as u32).to_le_bytes());
        for f in &self.files {
            put_entry(out, f);
        }
        out.extend_from_slice(&(self.dirs.len() as u32).to_le_bytes());
        for d in &self.dirs {
            d.write(out);
        }
    }

    fn count(&self) -> usize {
        self.files.len() + self.dirs.iter().map(Dir::count).sum::<usize>()
    }

    fn walk<'a>(&'a self, prefix: &str, out: &mut Vec<(String, &'a Entry)>) {
        let here = if self.name.is_empty() { prefix.to_string() } else { format!("{prefix}{}/", decode_name(&self.name)) };
        for f in &self.files {
            out.push((format!("{here}{}", decode_name(&f.name)), f));
        }
        for d in &self.dirs {
            d.walk(&here, out);
        }
    }
}

impl Index {
    pub fn new() -> Self {
        Index { version: 0, root: Dir::default(), free: Vec::new(), trailer: vec![0; 4] }
    }

    /// Parses an index; errors name the file `data.sah`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        Self::parse_named(data, "data.sah")
    }

    /// Like [`Index::parse`] with a file name for error messages (a patch's `update.sah`, a path on disk).
    pub fn parse_named(data: &[u8], file: &str) -> Result<Self> {
        if data.len() < 51 || &data[..3] != b"SAH" {
            return Err(Error::format(file, Some(0), "not a SAH index"));
        }
        let mut r = Reader::new(data, file);
        r.seek(3)?;
        let version = r.i32("index version")?;
        let declared = r.u32("file count")? as usize;
        r.skip(40, "header padding")?;
        let root = read_dir(&mut r, 0)?;
        let mut free = Vec::new();
        if r.left() >= 4 {
            let n = r.count(MIN_ENTRY, usize::MAX, "free list count")?;
            for _ in 0..n {
                free.push(read_entry(&mut r)?);
            }
        }
        let trailer = data[r.pos()..].to_vec();
        let idx = Index { version, root, free, trailer };
        if idx.file_count() != declared {
            return Err(Error::format(file, Some(7), format!("header says {declared} files, tree has {}", idx.file_count())));
        }
        Ok(idx)
    }

    /// Reads and parses an index file; errors carry the path.
    pub fn read(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Self::parse_named(&data, &path.display().to_string())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 << 20);
        out.extend_from_slice(b"SAH");
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(self.file_count() as u32).to_le_bytes());
        out.extend_from_slice(&[0u8; 40]);
        self.root.write(&mut out);
        out.extend_from_slice(&(self.free.len() as u32).to_le_bytes());
        for e in &self.free {
            put_entry(&mut out, e);
        }
        out.extend_from_slice(&self.trailer);
        out
    }

    pub fn file_count(&self) -> usize {
        self.root.count()
    }

    /// All files as (path with '/' separators, entry), in index order.
    pub fn files(&self) -> Vec<(String, &Entry)> {
        let mut out = Vec::new();
        self.root.walk("", &mut out);
        out
    }

    /// Find a file the way the client does (case-insensitive by hash; the later of duplicates wins).
    pub fn get(&self, path: &str) -> Option<&Entry> {
        let (dirs, name) = split_path(path)?;
        let mut d = &self.root;
        for part in dirs {
            let h = name_hash(&encode_name(part));
            d = d.dirs.iter().rev().find(|x| name_hash(&x.name) == h)?;
        }
        let h = name_hash(&encode_name(name));
        d.files.iter().rev().find(|f| name_hash(&f.name) == h)
    }

    /// Insert or replace a file; returns the previous entry if one was replaced. Directories are created as needed.
    /// Existing duplicates of the name are collapsed into one entry, kept at the position of the last one.
    pub fn put(&mut self, path: &str, entry: Entry) -> Result<Option<Entry>> {
        let (dirs, _) = split_path(path).ok_or_else(|| Error::format("path", None, format!("bad path {path:?}")))?;
        let mut d = &mut self.root;
        for part in dirs {
            let h = name_hash(&encode_name(part));
            let pos = match d.dirs.iter().rposition(|x| name_hash(&x.name) == h) {
                Some(p) => p,
                None => {
                    d.dirs.push(Dir { name: encode_name(part), ..Dir::default() });
                    d.dirs.len() - 1
                }
            };
            d = &mut d.dirs[pos];
        }
        let h = name_hash(&entry.name);
        let hits: Vec<usize> = d.files.iter().enumerate().filter(|(_, f)| name_hash(&f.name) == h).map(|(i, _)| i).collect();
        match hits.last() {
            Some(&last) => {
                let old = std::mem::replace(&mut d.files[last], entry);
                for &i in hits[..hits.len() - 1].iter().rev() {
                    d.files.remove(i);
                }
                Ok(Some(old))
            }
            None => {
                d.files.push(entry);
                Ok(None)
            }
        }
    }

    /// Remove a file (every entry with its name, as the client would find any of them); directories left empty by it are removed
    /// too. Returns the removed entries (empty when the path is not in the index).
    pub fn remove(&mut self, path: &str) -> Result<Vec<Entry>> {
        let (dirs, name) = split_path(path).ok_or_else(|| Error::format("path", None, format!("bad path {path:?}")))?;
        let hashes: Vec<u32> = dirs.iter().map(|p| name_hash(&encode_name(p))).collect();
        Ok(Self::remove_in(&mut self.root, &hashes, name_hash(&encode_name(name))))
    }

    fn remove_in(d: &mut Dir, dirs: &[u32], h: u32) -> Vec<Entry> {
        match dirs.split_first() {
            None => {
                let (gone, keep) = std::mem::take(&mut d.files).into_iter().partition(|f| name_hash(&f.name) == h);
                d.files = keep;
                gone
            }
            Some((&first, rest)) => {
                let Some(pos) = d.dirs.iter().rposition(|x| name_hash(&x.name) == first) else { return Vec::new() };
                let gone = Self::remove_in(&mut d.dirs[pos], rest, h);
                if !gone.is_empty() && d.dirs[pos].files.is_empty() && d.dirs[pos].dirs.is_empty() {
                    d.dirs.remove(pos);
                }
                gone
            }
        }
    }

    /// Drop shadowed duplicates (same name in one directory: the client uses the later entry, so the earlier ones are never read).
    /// Returns their paths.
    pub fn dedupe(&mut self) -> Vec<String> {
        fn walk(d: &mut Dir, prefix: &str, out: &mut Vec<String>) {
            let here = if d.name.is_empty() { prefix.to_string() } else { format!("{prefix}{}/", decode_name(&d.name)) };
            let mut last = std::collections::HashMap::new();
            for (i, f) in d.files.iter().enumerate() {
                last.insert(name_hash(&f.name), i);
            }
            let mut i = 0;
            d.files.retain(|f| {
                let keep = last[&name_hash(&f.name)] == i;
                if !keep {
                    out.push(format!("{here}{}", decode_name(&f.name)));
                }
                i += 1;
                keep
            });
            for c in &mut d.dirs {
                walk(c, &here, out);
            }
        }
        let mut out = Vec::new();
        walk(&mut self.root, "", &mut out);
        out
    }
}

/// One file of an index with its path and whether the client can still see it.
#[derive(Clone, Debug)]
pub struct FileRef<'a> {
    /// Display path, `/` separated, [`decode_name`] per segment.
    pub path: String,
    /// Raw segments joined by `b'/'`.
    pub raw_path: Vec<u8>,
    /// The raw segments themselves (folders, then the file name); a name that holds a `/` byte stays one segment.
    pub segments: Vec<Vec<u8>>,
    pub entry: &'a Entry,
    /// A later entry in the same folder has the same [`name_hash`], or a later folder of the same name hash holds this one.
    pub shadowed: bool,
}

fn collect_files<'a>(d: &'a Dir, segs: &mut Vec<&'a [u8]>, shadowed: bool, out: &mut Vec<FileRef<'a>>) {
    let mut last: HashMap<u32, usize> = HashMap::with_capacity(d.files.len());
    for (i, f) in d.files.iter().enumerate() {
        last.insert(name_hash(&f.name), i);
    }
    for (i, f) in d.files.iter().enumerate() {
        let sh = shadowed || last[&name_hash(&f.name)] != i;
        let mut segments: Vec<Vec<u8>> = segs.iter().map(|s| s.to_vec()).collect();
        segments.push(f.name.clone());
        let path = segments.iter().map(|s| decode_name(s)).collect::<Vec<_>>().join("/");
        let raw_path = segments.join(&b'/');
        out.push(FileRef { path, raw_path, segments, entry: f, shadowed: sh });
    }
    let mut dlast: HashMap<u32, usize> = HashMap::with_capacity(d.dirs.len());
    for (i, c) in d.dirs.iter().enumerate() {
        dlast.insert(name_hash(&c.name), i);
    }
    for (i, c) in d.dirs.iter().enumerate() {
        let sh = shadowed || dlast[&name_hash(&c.name)] != i;
        segs.push(&c.name);
        collect_files(c, segs, sh, out);
        segs.pop();
    }
}

impl Index {
    /// Every file in index order.
    pub fn entries(&self) -> Vec<FileRef<'_>> {
        let mut out = Vec::with_capacity(self.file_count());
        collect_files(&self.root, &mut Vec::new(), false, &mut out);
        out
    }

    /// Entries with `shadowed == false`: what [`Index::get`] returns for their paths.
    pub fn visible(&self) -> Vec<FileRef<'_>> {
        let mut v = self.entries();
        v.retain(|f| !f.shadowed);
        v
    }

    /// Folders, excluding the root.
    pub fn dir_count(&self) -> usize {
        fn n(d: &Dir) -> usize {
            d.dirs.iter().map(|c| 1 + n(c)).sum()
        }
        n(&self.root)
    }

    /// `max(offset + length)` over files, 0 when empty.
    pub fn data_end(&self) -> u64 {
        fn m(d: &Dir) -> u64 {
            d.files.iter().map(|f| f.offset.saturating_add(f.length as u64)).chain(d.dirs.iter().map(m)).max().unwrap_or(0)
        }
        m(&self.root)
    }
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

/// Split "a/b/c.dds" (or with backslashes) into (["a", "b"], "c.dds"); rejects empty, absolute and ".." paths.
pub fn split_path(path: &str) -> Option<(Vec<&str>, &str)> {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() || parts.iter().any(|p| *p == ".." || *p == "." || p.contains(':')) {
        return None;
    }
    let (name, dirs) = parts.split_last()?;
    Some((dirs.to_vec(), name))
}

/// Read access to data.sah + data.saf. The SAF is opened read-only with the default (shared) access, so a running game does not block
/// reading; nothing here writes. Moved from `clientdata`'s `source.rs` archive backing.
pub struct Archive {
    sah: PathBuf,
    saf: PathBuf,
    index: Index,
    file: Mutex<File>,
    saf_len: u64,
}

pub(crate) fn find_in_dir(dir: &Path, lower_name: &str) -> Option<PathBuf> {
    let direct = dir.join(lower_name);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(lower_name) && e.path().is_file()).map(|e| e.path())
}

impl Archive {
    /// `path` is a folder holding data.sah (any case) or the .sah file itself; the .saf is the sibling with the same stem.
    pub fn open(path: &Path) -> Result<Archive> {
        let (sah, index) = Self::open_index(path)?;
        Self::open_parsed(sah, index)
    }

    /// Opens the `.saf` of an index that was already read (no second parse).
    pub fn open_parsed(sah: PathBuf, index: Index) -> Result<Archive> {
        let saf = saf_of(&sah);
        let file = File::open(&saf).map_err(|e| Error::io(&saf, e))?;
        let saf_len = file.metadata().map_err(|e| Error::io(&saf, e))?.len();
        Ok(Archive { sah, saf, index, file: Mutex::new(file), saf_len })
    }

    /// Same path rules as [`Archive::open`]; reads only the index (a client with no `data.saf`). Returns (sah path, index).
    pub fn open_index(path: &Path) -> Result<(PathBuf, Index)> {
        let sah = if path.is_dir() {
            find_in_dir(path, "data.sah")
                .ok_or_else(|| Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, "no data.sah in this folder")))?
        } else {
            path.to_path_buf()
        };
        let index = Index::read(&sah)?;
        Ok((sah, index))
    }

    /// Part of an entry, bounds-checked against the entry and `saf_len` (hex paging of large entries).
    pub fn read_range(&self, entry: &Entry, start: u64, len: usize) -> Result<Vec<u8>> {
        self.check_bounds(entry)?;
        if start.checked_add(len as u64).is_none_or(|end| end > entry.length as u64) {
            return Err(Error::format(
                self.saf.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "data.saf".into()),
                Some(entry.offset.saturating_add(start)),
                format!("range of {len} bytes at {start} lies outside the entry ({} bytes)", entry.length),
            ));
        }
        let mut buf = vec![0u8; len];
        self.read_at(entry.offset + start, &mut buf)?;
        Ok(buf)
    }

    /// A streaming reader over one entry (bounds-checked once, here).
    pub fn reader(&self, entry: &Entry) -> Result<EntryReader<'_>> {
        self.check_bounds(entry)?;
        Ok(EntryReader { archive: self, pos: entry.offset, left: entry.length as u64 })
    }

    pub fn index(&self) -> &Index {
        &self.index
    }
    pub fn sah_path(&self) -> &Path {
        &self.sah
    }
    pub fn saf_path(&self) -> &Path {
        &self.saf
    }
    pub fn saf_len(&self) -> u64 {
        self.saf_len
    }

    fn check_bounds(&self, entry: &Entry) -> Result<()> {
        if entry.offset.checked_add(entry.length as u64).is_none_or(|end| end > self.saf_len) {
            return Err(Error::format(
                self.saf.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "data.saf".into()),
                Some(entry.offset),
                format!("archive entry of {} bytes lies outside the data file ({} bytes)", entry.length, self.saf_len),
            ));
        }
        Ok(())
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let mut f = self.file.lock().unwrap_or_else(|p| p.into_inner());
        f.seek(SeekFrom::Start(offset)).and_then(|_| f.read_exact(buf)).map_err(|e| Error::io(&self.saf, e))
    }

    /// Whole entry, case-insensitive path, `/` or `\`.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let e = self
            .index
            .get(path)
            .ok_or_else(|| Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, "not in the archive")))?;
        self.read_entry(e)
    }

    /// Bounds-checked against the size of `data.saf`.
    pub fn read_entry(&self, entry: &Entry) -> Result<Vec<u8>> {
        self.check_bounds(entry)?;
        let mut buf = vec![0u8; entry.length as usize];
        self.read_at(entry.offset, &mut buf)?;
        Ok(buf)
    }

    /// Streams an entry into `out` with a buffer of at most 1 MiB; progress in bytes, cancellable.
    pub fn copy_entry(&self, entry: &Entry, out: &mut dyn Write, job: &Job) -> Result<u64> {
        self.check_bounds(entry)?;
        job.total(Some(entry.length as u64), Unit::Bytes);
        let mut buf = vec![0u8; (1usize << 20).min(entry.length as usize).max(1)];
        let (mut pos, mut left) = (entry.offset, entry.length as u64);
        while left > 0 {
            job.check()?;
            let n = left.min(buf.len() as u64) as usize;
            self.read_at(pos, &mut buf[..n])?;
            out.write_all(&buf[..n]).map_err(Error::from)?;
            pos += n as u64;
            left -= n as u64;
            job.advance(n as u64);
        }
        Ok(entry.length as u64)
    }
}

/// The `.saf` that goes with a `.sah`: the sibling with the same stem (found in any case on case-sensitive systems).
pub fn saf_of(sah: &Path) -> PathBuf {
    let dir = sah.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    let stem = sah.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    find_in_dir(&dir, &format!("{stem}.saf")).unwrap_or_else(|| sah.with_extension("saf"))
}

/// `std::io::Read` over one archive entry.
pub struct EntryReader<'a> {
    archive: &'a Archive,
    pos: u64,
    left: u64,
}

impl Read for EntryReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = (buf.len() as u64).min(self.left) as usize;
        if n == 0 {
            return Ok(0);
        }
        let mut f = self.archive.file.lock().unwrap_or_else(|p| p.into_inner());
        f.seek(SeekFrom::Start(self.pos))?;
        let got = f.read(&mut buf[..n])?;
        self.pos += got as u64;
        self.left -= got as u64;
        if got == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "data.saf is shorter than its index"));
        }
        Ok(got)
    }
}

/// Visible (or all) entries of an archive as a [`ByteSource`], filtered. Reading by name follows the client (the visible entry).
pub struct ArchiveSource<'a> {
    pub archive: &'a Archive,
    pub filter: Filter,
    pub include_shadowed: bool,
}

impl ByteSource for ArchiveSource<'_> {
    fn label(&self) -> String {
        format!("archive {}", self.archive.sah_path().display())
    }
    fn names(&self) -> Vec<String> {
        let idx = self.archive.index();
        let all = if self.include_shadowed { idx.entries() } else { idx.visible() };
        all.into_iter().filter(|f| self.filter.matches(&f.path, f.entry.length as u64)).map(|f| f.path).collect()
    }
    fn size(&self, name: &str) -> Option<u64> {
        self.archive.index().get(name).map(|e| e.length as u64)
    }
    fn read(&self, name: &str) -> Result<Vec<u8>> {
        self.archive.read(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, offset: u64) -> Entry {
        Entry { name: name.as_bytes().to_vec(), offset, length: 1, version: 7 }
    }

    #[test]
    fn hash_matches_client_rules() {
        assert_eq!(name_hash(b"R1_DUN3.dg"), name_hash(b"r1_dun3.dg"));
        assert_ne!(name_hash(b"a.dds"), name_hash(b"b.dds"));
        assert_eq!(name_hash(b""), 0x1505);
    }

    #[test]
    fn roundtrip_and_put() {
        let mut idx = Index::new();
        assert!(idx.put("Item/Item.SData", e("Item.SData", 0)).unwrap().is_none());
        assert!(idx.put("interface/icon/29.dds", e("29.dds", 10)).unwrap().is_none());
        let old = idx.put("ITEM/item.sdata", e("item.sdata", 20)).unwrap().unwrap();
        assert_eq!(old.offset, 0);
        assert_eq!(idx.file_count(), 2);
        assert_eq!(idx.get("item/ITEM.SDATA").unwrap().offset, 20);
        let bytes = idx.to_bytes();
        assert_eq!(Index::parse(&bytes).unwrap(), idx);
    }

    #[test]
    fn duplicates_resolve_to_last() {
        let mut idx = Index::new();
        idx.root.dirs.push(Dir { name: b"world".to_vec(), files: vec![e("A.dg", 1), e("a.DG", 2)], dirs: vec![] });
        assert_eq!(idx.get("world/a.dg").unwrap().offset, 2);
        idx.put("world/a.dg", e("a.dg", 3)).unwrap();
        assert_eq!(idx.root.dirs[0].files.len(), 1);
        assert_eq!(idx.get("WORLD/A.DG").unwrap().offset, 3);
    }

    #[test]
    fn remove_and_dedupe() {
        let mut idx = Index::new();
        idx.put("Effect/test/a.eft", e("a.eft", 1)).unwrap();
        idx.put("Effect/b.eft", e("b.eft", 2)).unwrap();
        idx.root.dirs.push(Dir { name: b"world".to_vec(), files: vec![e("A.dg", 3), e("x.dg", 4), e("a.DG", 5)], dirs: vec![] });
        // case-insensitive; the emptied "test" directory goes, "Effect" (still holding b.eft) stays
        assert_eq!(idx.remove("EFFECT/Test/A.EFT").unwrap().len(), 1);
        assert!(idx.get("Effect/b.eft").is_some());
        assert_eq!(idx.root.dirs[0].dirs.len(), 0);
        assert!(idx.remove("Effect/missing.eft").unwrap().is_empty());
        assert!(idx.remove("nodir/x.eft").unwrap().is_empty());
        // the shadowed A.dg (offset 3) is dropped; the client's choice (offset 5) and x.dg stay in order
        assert_eq!(idx.dedupe(), vec!["world/A.dg"]);
        let w = &idx.root.dirs[1].files;
        assert_eq!(w.iter().map(|f| f.offset).collect::<Vec<_>>(), vec![4, 5]);
        // Korean names: stored as code page 949, found and removed by their Unicode path
        let raw = vec![b'1', b'0', b'0', b'5', b'1', 0xbf, 0xf8, b'.', b'3', b'd', b'o'];
        idx.root.dirs.push(Dir { name: b"Item".to_vec(), files: vec![Entry { name: raw.clone(), offset: 9, length: 1, version: 0 }], dirs: vec![] });
        assert_eq!(encode_name("10051원.3do"), raw);
        assert!(idx.files().iter().any(|(p, _)| p == "Item/10051원.3do"));
        assert_eq!(idx.remove("item/10051원.3DO").unwrap().len(), 1);
        // removing a name with duplicates removes all of them
        let mut idx2 = Index::new();
        idx2.root.files = vec![e("z", 1), e("Z", 2)];
        assert_eq!(idx2.remove("z").unwrap().len(), 2);
        assert_eq!(idx2.file_count(), 0);
    }

    #[test]
    fn rejects_bad_paths() {
        assert!(split_path("../x.dds").is_none());
        assert!(split_path("C:/x.dds").is_none());
        assert!(split_path("").is_none());
    }
}
