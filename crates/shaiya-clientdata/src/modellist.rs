//! The model list files: `.MLT` (character parts), `.MON` (monsters, NPCs, vehicles) and `.ITM` (item models). They name the mesh and
//! the texture files of the models; a `.3DC` names no texture itself. Every layout below was read from client files (US EP4.5; see the
//! format pages for the evidence of each field); what a number means is labelled on those pages.
//!
//! Names are kept as the raw bytes stored in the file (an `.MLT` string has its NUL, a `.MON` string has none), numbers as read, so a
//! parsed file writes back byte for byte. [`name_text`] decodes a name for display. Another episode's layout (`MO4`, `IT2`, ...) is
//! refused with a reason, never half-read. Counts and string lengths are checked before anything is allocated.

use crate::error::{decode_cp949, Cur, Error, Result};

/// Most records, names or parts one list holds (the reader refuses a larger count before allocating).
pub const LIST_LIMIT: usize = 1 << 16;
/// Longest name in bytes (the reader refuses a longer string before reading it).
pub const NAME_LIMIT: usize = 1 << 12;
/// How many name slots a `.MON` record has.
pub const MON_SLOTS: usize = 17;

/// The three kinds of list file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Mlt,
    Mon,
    Itm,
}

impl ListKind {
    /// The three signature bytes at the start of the file.
    pub fn signature(self) -> &'static [u8; 3] {
        match self {
            ListKind::Mlt => b"ML2",
            ListKind::Mon => b"MO2",
            ListKind::Itm => b"ITM",
        }
    }

    /// The kind whose signature starts `bytes`, if any.
    pub fn of_signature(bytes: &[u8]) -> Option<ListKind> {
        [ListKind::Mlt, ListKind::Mon, ListKind::Itm].into_iter().find(|k| bytes.starts_with(k.signature()))
    }

    pub fn label(self) -> &'static str {
        match self {
            ListKind::Mlt => "MLT",
            ListKind::Mon => "MON",
            ListKind::Itm => "ITM",
        }
    }
}

/// A name for display: the bytes up to the first NUL, decoded as CP949.
pub fn name_text(raw: &[u8]) -> String {
    decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[]))
}

fn signature(r: &mut Cur, kind: ListKind) -> Result<()> {
    let sig = r.bytes(3, "signature").map_err(|_| r.fail("too short to be a list file"))?;
    if sig == kind.signature() {
        return Ok(());
    }
    let shown = if sig.iter().all(|b| b.is_ascii_graphic()) { String::from_utf8_lossy(sig).into_owned() } else { format!("{:02x}{:02x}{:02x}", sig[0], sig[1], sig[2]) };
    r.p = 0;
    Err(r.fail(format!("`{shown}`: a layout of another episode, not read (this reader takes `{}`)", String::from_utf8_lossy(kind.signature()))))
}

/// `i32 length` + that many bytes, kept raw.
fn string(r: &mut Cur, what: &str) -> Result<Vec<u8>> {
    let at = r.p;
    let n = r.i32(what)?;
    if n < 0 || n as usize > NAME_LIMIT || n as usize > r.left() {
        r.p = at;
        return Err(r.fail(format!("{what}: bad string length {n}")));
    }
    Ok(r.bytes(n as usize, what)?.to_vec())
}

fn strings(r: &mut Cur, what: &str) -> Result<Vec<Vec<u8>>> {
    let n = r.count(4, LIST_LIMIT, what)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(string(r, what)?);
    }
    Ok(out)
}

/// A refusal of the writer (there is no file or byte offset: the list is not a file yet).
fn bad(what: String) -> Error {
    Error::Format { file: "list writer".into(), what: format!("the list cannot be written: {what}"), offset: 0 }
}

struct Out(Vec<u8>);

impl Out {
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn len(&mut self, n: usize, what: &str, limit: usize) -> Result<()> {
        if n > limit {
            return Err(bad(format!("{what}: {n} is over the limit of {limit}")));
        }
        self.i32(n as i32);
        Ok(())
    }
    fn string(&mut self, s: &[u8], what: &str) -> Result<()> {
        self.len(s.len(), what, NAME_LIMIT)?;
        self.0.extend_from_slice(s);
        Ok(())
    }
    fn strings(&mut self, list: &[Vec<u8>], what: &str) -> Result<()> {
        self.len(list.len(), what, LIST_LIMIT)?;
        list.iter().try_for_each(|s| self.string(s, what))
    }
}

fn name_at(list: &[Vec<u8>], index: i32) -> Option<&[u8]> {
    usize::try_from(index).ok().and_then(|i| list.get(i)).map(|v| v.as_slice())
}

// ----------------------------------------------------------------------------------------------------------------------------------
// .MLT

/// One `.MLT` record: indices into the file's mesh and texture lists, and the alpha flag. Out-of-range indices are kept as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MltRecord {
    pub mesh: i32,
    pub texture: i32,
    pub alpha_flag: i32,
}

/// A `.MLT` file: `"ML2"`, the `.3DC` names, the `.DDS` names (each string's length includes its NUL, which is kept) and the records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mlt {
    pub meshes: Vec<Vec<u8>>,
    pub textures: Vec<Vec<u8>>,
    pub records: Vec<MltRecord>,
}

impl Mlt {
    pub fn parse(data: &[u8], file: &str) -> Result<Mlt> {
        let mut r = Cur::new(data, file);
        signature(&mut r, ListKind::Mlt)?;
        let meshes = strings(&mut r, "mesh names")?;
        let textures = strings(&mut r, "texture names")?;
        let n = r.count(12, LIST_LIMIT, "record count")?;
        let mut records = Vec::with_capacity(n);
        for _ in 0..n {
            records.push(MltRecord { mesh: r.i32("mesh index")?, texture: r.i32("texture index")?, alpha_flag: r.i32("alpha flag")? });
        }
        r.end("list")?;
        Ok(Mlt { meshes, textures, records })
    }

    /// The mesh name a record points at, raw; `None` when the index is out of range.
    pub fn mesh_name(&self, rec: &MltRecord) -> Option<&[u8]> {
        name_at(&self.meshes, rec.mesh)
    }

    /// The texture name a record points at, raw; `None` when the index is out of range.
    pub fn texture_name(&self, rec: &MltRecord) -> Option<&[u8]> {
        name_at(&self.textures, rec.texture)
    }

    /// The bytes of this list; for a parsed file, exactly the bytes that were read. Panics only for a list [`Mlt::try_to_bytes`] refuses,
    /// which no parsed file is.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("list cannot be written")
    }

    /// Like [`Mlt::to_bytes`], but refuses a list over the reader's limits; the bytes are read back with [`Mlt::parse`] and refused unless
    /// they read to the same list.
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let mut o = Out(ListKind::Mlt.signature().to_vec());
        o.strings(&self.meshes, "mesh names")?;
        o.strings(&self.textures, "texture names")?;
        o.len(self.records.len(), "records", LIST_LIMIT)?;
        for rec in &self.records {
            o.i32(rec.mesh);
            o.i32(rec.texture);
            o.i32(rec.alpha_flag);
        }
        if Mlt::parse(&o.0, "list")? != *self {
            return Err(bad("the bytes do not read back to the same list".into()));
        }
        Ok(o.0)
    }
}

// ----------------------------------------------------------------------------------------------------------------------------------
// .MON

/// One mesh and texture pair of a `.MON` record (names without a NUL).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonPart {
    pub mesh: Vec<u8>,
    pub texture: Vec<u8>,
}

/// One `.MON` record. Records are equal when every field is, `size` compared by its bits (so NaN equals the same NaN and `-0.0` is not `0.0`).
#[derive(Clone, Debug)]
pub struct MonRecord {
    pub name: Vec<u8>,
    pub flag: u8,
    /// The 17 name slots; most are empty.
    pub slots: [Vec<u8>; MON_SLOTS],
    pub parts: Vec<MonPart>,
    pub size: f32,
    /// `(a, b)` pairs, inferred to be a bone index and an effect scene of the monster effect file; kept as read.
    pub attachments: Vec<(i32, i32)>,
}

impl PartialEq for MonRecord {
    fn eq(&self, o: &MonRecord) -> bool {
        self.name == o.name
            && self.flag == o.flag
            && self.slots == o.slots
            && self.parts == o.parts
            && self.size.to_bits() == o.size.to_bits()
            && self.attachments == o.attachments
    }
}

impl Eq for MonRecord {}

/// A `.MON` file: `"MO2"`, a record count and the records (equal when the records are, see [`MonRecord`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mon {
    pub records: Vec<MonRecord>,
}

/// Smallest possible record: name length, flag, 17 slot lengths, part count, size, attachment count.
const MON_MIN_RECORD: usize = 4 + 1 + 4 * MON_SLOTS + 4 + 4 + 4;

impl Mon {
    pub fn parse(data: &[u8], file: &str) -> Result<Mon> {
        let mut r = Cur::new(data, file);
        signature(&mut r, ListKind::Mon)?;
        let n = r.count(MON_MIN_RECORD, LIST_LIMIT, "record count")?;
        let mut records = Vec::with_capacity(n);
        for _ in 0..n {
            let name = string(&mut r, "name")?;
            let flag = r.u8("flag")?;
            let mut slots: [Vec<u8>; MON_SLOTS] = Default::default();
            for s in slots.iter_mut() {
                *s = string(&mut r, "slot")?;
            }
            let np = r.count(8, LIST_LIMIT, "part count")?;
            let mut parts = Vec::with_capacity(np);
            for _ in 0..np {
                parts.push(MonPart { mesh: string(&mut r, "part mesh")?, texture: string(&mut r, "part texture")? });
            }
            let size = r.f32("size")?;
            let na = r.count(8, LIST_LIMIT, "attachment count")?;
            let mut attachments = Vec::with_capacity(na);
            for _ in 0..na {
                attachments.push((r.i32("attachment a")?, r.i32("attachment b")?));
            }
            records.push(MonRecord { name, flag, slots, parts, size, attachments });
        }
        r.end("list")?;
        Ok(Mon { records })
    }

    /// Total parts over all records.
    pub fn part_count(&self) -> usize {
        self.records.iter().map(|r| r.parts.len()).sum()
    }

    /// The bytes of this list; for a parsed file, exactly the bytes that were read (see [`Mlt::to_bytes`] for the panic).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("list cannot be written")
    }

    /// Like [`Mon::to_bytes`], but refuses a list over the reader's limits and reads the bytes back (floats are compared by their bits).
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let mut o = Out(ListKind::Mon.signature().to_vec());
        o.len(self.records.len(), "records", LIST_LIMIT)?;
        for rec in &self.records {
            o.string(&rec.name, "name")?;
            o.0.push(rec.flag);
            for s in &rec.slots {
                o.string(s, "slot")?;
            }
            o.len(rec.parts.len(), "parts", LIST_LIMIT)?;
            for p in &rec.parts {
                o.string(&p.mesh, "part mesh")?;
                o.string(&p.texture, "part texture")?;
            }
            o.0.extend_from_slice(&rec.size.to_bits().to_le_bytes());
            o.len(rec.attachments.len(), "attachments", LIST_LIMIT)?;
            for (a, b) in &rec.attachments {
                o.i32(*a);
                o.i32(*b);
            }
        }
        if Mon::parse(&o.0, "list")? != *self {
            return Err(bad("the bytes do not read back to the same list".into()));
        }
        Ok(o.0)
    }
}

// ----------------------------------------------------------------------------------------------------------------------------------
// .ITM

/// One `.ITM` record: six words and, when `format` is 1, four more. `unk1` and `unk2` keep their name: nothing is known about them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItmRecord {
    pub model: i32,
    pub texture: i32,
    pub glow: i32,
    pub unk1: i32,
    pub format: i32,
    pub unk2: i32,
    /// Present exactly when `format == 1`.
    pub extra: Option<[i32; 4]>,
}

/// An `.ITM` file: `"ITM"`, the `.3DO` names, the `.DDS` names (no NUL stored) and the records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Itm {
    pub models: Vec<Vec<u8>>,
    pub textures: Vec<Vec<u8>>,
    pub records: Vec<ItmRecord>,
}

impl Itm {
    pub fn parse(data: &[u8], file: &str) -> Result<Itm> {
        let mut r = Cur::new(data, file);
        signature(&mut r, ListKind::Itm)?;
        let models = strings(&mut r, "model names")?;
        let textures = strings(&mut r, "texture names")?;
        let n = r.count(24, LIST_LIMIT, "record count")?;
        let mut records = Vec::with_capacity(n);
        for _ in 0..n {
            let (model, texture, glow, unk1, format, unk2) = (r.i32("model")?, r.i32("texture")?, r.i32("glow")?, r.i32("unk1")?, r.i32("format")?, r.i32("unk2")?);
            let extra = if format == 1 { Some([r.i32("extra")?, r.i32("extra")?, r.i32("extra")?, r.i32("extra")?]) } else { None };
            records.push(ItmRecord { model, texture, glow, unk1, format, unk2, extra });
        }
        r.end("list")?;
        Ok(Itm { models, textures, records })
    }

    /// The model name a record points at, raw; `None` when the index is out of range.
    pub fn model_name(&self, rec: &ItmRecord) -> Option<&[u8]> {
        name_at(&self.models, rec.model)
    }

    /// The texture name a record points at, raw; `None` when the index is out of range.
    pub fn texture_name(&self, rec: &ItmRecord) -> Option<&[u8]> {
        name_at(&self.textures, rec.texture)
    }

    /// The bytes of this list; for a parsed file, exactly the bytes that were read (see [`Mlt::to_bytes`] for the panic).
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("list cannot be written")
    }

    /// Like [`Itm::to_bytes`], but refuses a list over the reader's limits and a record whose `extra` does not match its `format`; the
    /// bytes are read back with [`Itm::parse`].
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let mut o = Out(ListKind::Itm.signature().to_vec());
        o.strings(&self.models, "model names")?;
        o.strings(&self.textures, "texture names")?;
        o.len(self.records.len(), "records", LIST_LIMIT)?;
        for (i, rec) in self.records.iter().enumerate() {
            if rec.extra.is_some() != (rec.format == 1) {
                return Err(bad(format!("record {i}: four extra words are present exactly when format is 1")));
            }
            for v in [rec.model, rec.texture, rec.glow, rec.unk1, rec.format, rec.unk2] {
                o.i32(v);
            }
            for v in rec.extra.iter().flatten() {
                o.i32(*v);
            }
        }
        if Itm::parse(&o.0, "list")? != *self {
            return Err(bad("the bytes do not read back to the same list".into()));
        }
        Ok(o.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::lists::{itm_bytes, mlt_bytes, mon_bytes, ItmRec, MonRec};

    fn mlt() -> Vec<u8> {
        mlt_bytes(&["a.3DC", "b.3DC"], &["x.DDS", "y.DDS", "z.DDS"], &[(0, 1, 1), (1, 2, 0), (1, 0, 1)])
    }

    fn mon() -> Vec<u8> {
        let mut slots: [&str; 17] = [""; 17];
        slots[0] = "head";
        slots[16] = "tail";
        let a = MonRec { name: "Wolf", flag: 3, slots, parts: &[("wolf.3DC", "wolf.DDS"), ("fang.3DC", "")], size: 1.5, attachments: &[(2, 7), (3, 8)] };
        let b = MonRec { name: "Bat", flag: 0, slots: [""; 17], parts: &[], size: -0.0, attachments: &[] };
        mon_bytes(&[a, b])
    }

    fn itm() -> Vec<u8> {
        itm_bytes(
            &["sword.3DO", "axe.3DO"],
            &["sword.DDS"],
            &[
                ItmRec { words: [0, 0, 2, -1, 0, 5], extra: None },
                ItmRec { words: [1, 0, -1, 7, 1, 9], extra: Some([10, 20, 30, 40]) },
            ],
        )
    }

    #[test]
    fn an_mlt_reads_to_the_last_byte_and_keeps_the_nul() {
        let d = mlt();
        let m = Mlt::parse(&d, "t.mlt").unwrap();
        assert_eq!(m.meshes, vec![b"a.3DC\0".to_vec(), b"b.3DC\0".to_vec()]);
        assert_eq!(m.textures.len(), 3);
        assert_eq!(m.records.len(), 3);
        assert_eq!(m.records[0], MltRecord { mesh: 0, texture: 1, alpha_flag: 1 });
        assert_eq!(m.mesh_name(&m.records[1]), Some(&b"b.3DC\0"[..]));
        assert_eq!(name_text(m.texture_name(&m.records[1]).unwrap()), "z.DDS");
        assert_eq!(m.to_bytes(), d);
        // an index out of range is kept and has no name
        let odd = Mlt { records: vec![MltRecord { mesh: 9, texture: -1, alpha_flag: 7 }], ..m.clone() };
        assert_eq!(odd.mesh_name(&odd.records[0]), None);
        assert_eq!(odd.texture_name(&odd.records[0]), None);
        assert_eq!(Mlt::parse(&odd.to_bytes(), "t.mlt").unwrap(), odd);
    }

    #[test]
    fn a_mon_record_with_parts_slots_and_attachments_reads() {
        let d = mon();
        let m = Mon::parse(&d, "t.mon").unwrap();
        assert_eq!(m.records.len(), 2);
        let w = &m.records[0];
        assert_eq!((name_text(&w.name).as_str(), w.flag, w.size), ("Wolf", 3, 1.5));
        assert_eq!((name_text(&w.slots[0]).as_str(), name_text(&w.slots[1]).as_str(), name_text(&w.slots[16]).as_str()), ("head", "", "tail"));
        assert_eq!(w.parts.len(), 2);
        assert_eq!((name_text(&w.parts[1].mesh).as_str(), w.parts[1].texture.len()), ("fang.3DC", 0));
        assert_eq!(w.attachments, vec![(2, 7), (3, 8)]);
        assert_eq!(m.part_count(), 2);
        // strings carry no NUL
        assert_eq!(w.name, b"Wolf");
        // a negative zero survives
        assert_eq!(m.records[1].size.to_bits(), (-0.0f32).to_bits());
        assert_eq!(m.to_bytes(), d);
    }

    #[test]
    fn an_itm_record_of_format_1_has_four_more_words() {
        let d = itm();
        let m = Itm::parse(&d, "t.itm").unwrap();
        assert_eq!(m.records.len(), 2);
        assert_eq!(m.records[0].extra, None);
        assert_eq!(m.records[1].extra, Some([10, 20, 30, 40]));
        assert_eq!((m.records[0].glow, m.records[0].unk1, m.records[1].unk2), (2, -1, 9));
        assert_eq!(name_text(m.model_name(&m.records[1]).unwrap()), "axe.3DO");
        assert_eq!(m.texture_name(&m.records[0]), Some(&b"sword.DDS"[..]));
        // 24 bytes for the first record, 40 for the second
        let header = d.len() - 24 - 40;
        assert_eq!(&d[header..header + 4], &0i32.to_le_bytes());
        assert_eq!(m.to_bytes(), d);
        // a record whose extra does not match its format is not written
        let mut broken = m.clone();
        broken.records[0].extra = Some([1, 2, 3, 4]);
        assert!(broken.try_to_bytes().is_err());
        let mut broken = m;
        broken.records[1].extra = None;
        assert!(broken.try_to_bytes().is_err());
    }

    #[test]
    fn another_signature_is_refused_with_the_reason() {
        for (sig, parse_one) in [
            ("MO4", (|d: &[u8]| Mon::parse(d, "t.mon").map(|_| ())) as fn(&[u8]) -> Result<()>),
            ("IT2", |d| Itm::parse(d, "t.itm").map(|_| ())),
            ("ML3", |d| Mlt::parse(d, "t.mlt").map(|_| ())),
        ] {
            let mut d = sig.as_bytes().to_vec();
            d.extend_from_slice(&0i32.to_le_bytes());
            let e = parse_one(&d).unwrap_err().to_string();
            assert!(e.contains(&format!("`{sig}`: a layout of another episode, not read")), "{e}");
        }
        // a list of the right kind in the wrong reader is refused the same way
        let e = Mlt::parse(&mon(), "t.mlt").unwrap_err().to_string();
        assert!(e.contains("`MO2`: a layout of another episode, not read"), "{e}");
        // non-text signature bytes are shown as hex; too short is its own message
        assert!(Mon::parse(&[0xff, 0, 1, 0, 0, 0, 0], "t.mon").unwrap_err().to_string().contains("`ff0001`"));
        assert!(Mon::parse(b"MO", "t.mon").unwrap_err().to_string().contains("too short"));
        assert_eq!(ListKind::of_signature(b"ITM\0"), Some(ListKind::Itm));
        assert_eq!(ListKind::of_signature(b"IT2"), None);
    }

    #[test]
    fn a_truncated_or_overlong_list_is_refused() {
        for d in [mlt(), mon(), itm()] {
            let parse = |b: &[u8]| match &d[..2] {
                b"ML" => Mlt::parse(b, "t").map(|_| ()),
                b"MO" => Mon::parse(b, "t").map(|_| ()),
                _ => Itm::parse(b, "t").map(|_| ()),
            };
            parse(&d).unwrap();
            for cut in [d.len() - 1, d.len() / 2, 5, 3] {
                assert!(parse(&d[..cut]).is_err(), "cut at {cut} of {}", d.len());
            }
            let mut long = d.clone();
            long.push(0);
            let e = parse(&long).unwrap_err().to_string();
            assert!(e.contains("trailing"), "{e}");
        }
    }

    #[test]
    fn huge_counts_and_names_are_refused_before_allocating() {
        // a count of two billion names in an 11-byte file
        let mut d = b"ML2".to_vec();
        d.extend_from_slice(&i32::MAX.to_le_bytes());
        d.extend_from_slice(&[0; 4]);
        assert!(Mlt::parse(&d, "t").unwrap_err().to_string().contains("bad count"));
        // a negative count
        let mut d = b"MO2".to_vec();
        d.extend_from_slice(&(-1i32).to_le_bytes());
        assert!(Mon::parse(&d, "t").unwrap_err().to_string().contains("bad count"));
        // a count under the limit that does not fit the bytes left
        let mut d = b"ITM".to_vec();
        d.extend_from_slice(&1000i32.to_le_bytes());
        assert!(Itm::parse(&d, "t").unwrap_err().to_string().contains("does not fit"));
        // a name over the limit, and a name longer than the file
        for n in [NAME_LIMIT as i32 + 1, i32::MAX, -5] {
            let mut d = b"ITM".to_vec();
            d.extend_from_slice(&1i32.to_le_bytes());
            d.extend_from_slice(&n.to_le_bytes());
            d.extend_from_slice(&vec![b'a'; NAME_LIMIT + 8]);
            assert!(Itm::parse(&d, "t").unwrap_err().to_string().contains("bad string length"), "{n}");
        }
        // a MON record count that cannot fit even its smallest records
        let mut d = b"MO2".to_vec();
        d.extend_from_slice(&((LIST_LIMIT as i32) - 1).to_le_bytes());
        d.extend_from_slice(&[0; 200]);
        assert!(Mon::parse(&d, "t").unwrap_err().to_string().contains("does not fit"));
        // the writer refuses a list over the limits too
        let big = Mlt { meshes: vec![vec![b'x'; NAME_LIMIT + 1]], textures: vec![], records: vec![] };
        assert!(big.try_to_bytes().is_err());
    }

    #[test]
    fn list_files_write_back_byte_for_byte() {
        assert_eq!(Mlt::parse(&mlt(), "t").unwrap().try_to_bytes().unwrap(), mlt());
        assert_eq!(Mon::parse(&mon(), "t").unwrap().try_to_bytes().unwrap(), mon());
        assert_eq!(Itm::parse(&itm(), "t").unwrap().try_to_bytes().unwrap(), itm());
        // raw bytes after a NUL, and a non-text name, are kept as they are
        let d = mlt_bytes_raw(&[b"a\0junk".as_slice(), &[0xB0, 0xA1, 0]], &[], &[]);
        let m = Mlt::parse(&d, "t").unwrap();
        assert_eq!(m.meshes[0], b"a\0junk");
        assert_eq!(m.to_bytes(), d);
        // NaN bits of a MON size survive
        let mut m = Mon::parse(&mon(), "t").unwrap();
        m.records[0].size = f32::from_bits(0x7fc0_1234);
        let back = Mon::parse(&m.to_bytes(), "t").unwrap();
        assert_eq!(back.records[0].size.to_bits(), 0x7fc0_1234);
        // equality is by the bits of the size
        assert!(back == m, "the same NaN equals itself");
        let mut z = back.clone();
        z.records[0].size = f32::from_bits(0x7fc0_1235);
        assert!(z != m, "another NaN is another value");
        let (mut a, mut b) = (m.clone(), m.clone());
        (a.records[0].size, b.records[0].size) = (0.0, -0.0);
        assert!(a != b, "-0.0 is not 0.0");
    }

    fn mlt_bytes_raw(meshes: &[&[u8]], textures: &[&[u8]], recs: &[(i32, i32, i32)]) -> Vec<u8> {
        let mut d = b"ML2".to_vec();
        for list in [meshes, textures] {
            d.extend_from_slice(&(list.len() as i32).to_le_bytes());
            for s in list {
                d.extend_from_slice(&(s.len() as i32).to_le_bytes());
                d.extend_from_slice(s);
            }
        }
        d.extend_from_slice(&(recs.len() as i32).to_le_bytes());
        for r in recs {
            for v in [r.0, r.1, r.2] {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        d
    }

    #[test]
    fn names_decode_from_cp949_for_display() {
        // "가" in CP949 is B0 A1; the text stops at the first NUL
        assert_eq!(name_text(&[0xB0, 0xA1, b'.', b'3', b'D', b'C', 0, b'x']), "가.3DC");
        assert_eq!(name_text(b""), "");
        assert_eq!(name_text(b"plain.DDS"), "plain.DDS");
        let d = mlt_bytes_raw(&[&[0xB0, 0xA1, 0]], &[], &[(0, -1, 1)]);
        let m = Mlt::parse(&d, "t").unwrap();
        assert_eq!(name_text(m.mesh_name(&m.records[0]).unwrap()), "가");
    }
}
