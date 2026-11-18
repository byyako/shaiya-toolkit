//! Reader for `.svmap` files. Every failure names the section, the record and the byte offset.

use std::fmt;

use crate::bitgrid::{packed_len, BitGrid};
use crate::model::*;

/// Counts above this are rejected before anything is allocated.
pub const MAX_COUNT: u32 = 1_000_000;

/// The nine sections in file order.
pub const SECTION_NAMES: [&str; 9] = [
    "size",
    "blocked cells",
    "section A",
    "section B",
    "monster areas",
    "NPCs",
    "portals",
    "bind areas",
    "regions",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadErrorKind {
    /// Needed `need` bytes, only `have` were left.
    Truncated { need: u64, have: u64 },
    /// A record count above [`MAX_COUNT`].
    CountTooLarge { count: u32 },
    /// Bytes left after the last section.
    TrailingBytes { count: usize },
}

/// A read failure. `section` is one of [`SECTION_NAMES`]; `index` is the record (or, for nested lists, the outer record)
/// being read; `offset` is the byte offset in the file where the failing read started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub section: &'static str,
    pub index: Option<usize>,
    pub offset: usize,
    pub kind: ReadErrorKind,
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.section)?;
        if let Some(i) = self.index {
            write!(f, " record {i}")?;
        }
        write!(f, " at byte {}: ", self.offset)?;
        match &self.kind {
            ReadErrorKind::Truncated { need, have } => {
                write!(f, "file ends early (needs {need} bytes, {have} left)")
            }
            ReadErrorKind::CountTooLarge { count } => {
                write!(f, "record count {count} is above the limit {MAX_COUNT}")
            }
            ReadErrorKind::TrailingBytes { count } => {
                write!(f, "{count} unexpected bytes after the last section")
            }
        }
    }
}

impl std::error::Error for ReadError {}

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    section: &'static str,
    index: Option<usize>,
}

impl<'a> Cursor<'a> {
    fn err(&self, kind: ReadErrorKind) -> ReadError {
        ReadError { section: self.section, index: self.index, offset: self.pos, kind }
    }

    fn left(&self) -> usize {
        self.data.len() - self.pos
    }

    fn need(&self, n: u64) -> Result<(), ReadError> {
        if (self.left() as u64) < n {
            Err(self.err(ReadErrorKind::Truncated { need: n, have: self.left() as u64 }))
        } else {
            Ok(())
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ReadError> {
        self.need(n as u64)?;
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ReadError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, ReadError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, ReadError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn vec3(&mut self) -> Result<[f32; 3], ReadError> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    /// Reads a record count (capped at [`MAX_COUNT`]). A count the rest of the file cannot hold is not rejected here: the
    /// records fail one by one with their index; `cap` keeps the allocation bounded by the file size meanwhile.
    fn count(&mut self) -> Result<usize, ReadError> {
        let at = self.pos;
        let n = self.u32()?;
        if n > MAX_COUNT {
            self.pos = at;
            return Err(self.err(ReadErrorKind::CountTooLarge { count: n }));
        }
        Ok(n as usize)
    }

    /// Vec capacity for `n` records of at least `min_record` bytes each.
    fn cap(&self, n: usize, min_record: usize) -> usize {
        n.min(self.left() / min_record)
    }

    fn section(&mut self, idx: usize) {
        self.section = SECTION_NAMES[idx];
        self.index = None;
    }
}

/// Parses an `.svmap` file. The whole input must be consumed.
pub fn read(data: &[u8]) -> Result<Svmap, ReadError> {
    let mut c = Cursor { data, pos: 0, section: SECTION_NAMES[0], index: None };

    let size = c.u32()?;

    c.section(1);
    let nbytes = packed_len(size);
    c.need(nbytes)?;
    let blocked = BitGrid::from_bytes(size, c.take(nbytes as usize)?.to_vec()).expect("length checked");

    c.section(2);
    let n = c.count()?;
    let mut section_a = Vec::with_capacity(c.cap(n, 20));
    for i in 0..n {
        c.index = Some(i);
        section_a.push(RawA { pos: c.vec3()?, a: c.u32()?, b: c.u32()? });
    }

    c.section(3);
    let n = c.count()?;
    let mut points_b = Vec::with_capacity(c.cap(n, 12));
    for i in 0..n {
        c.index = Some(i);
        points_b.push(c.vec3()?);
    }

    c.section(4);
    let n = c.count()?;
    let mut mob_areas = Vec::with_capacity(c.cap(n, 28));
    for i in 0..n {
        c.index = Some(i);
        let min = c.vec3()?;
        let max = c.vec3()?;
        let m = c.count()?;
        let mut mobs = Vec::with_capacity(c.cap(m, 8));
        for _ in 0..m {
            mobs.push(MobEntry { mob: c.u32()?, count: c.u32()? });
        }
        mob_areas.push(MobArea { min, max, mobs });
    }

    c.section(5);
    let n = c.count()?;
    let mut npcs = Vec::with_capacity(c.cap(n, 12));
    for i in 0..n {
        c.index = Some(i);
        let kind = c.u32()?;
        let type_id = c.u32()?;
        let m = c.count()?;
        let mut points = Vec::with_capacity(c.cap(m, 16));
        for _ in 0..m {
            points.push(NpcPoint { pos: c.vec3()?, dir: c.f32()? });
        }
        npcs.push(NpcEntry { kind, type_id, points });
    }

    c.section(6);
    let n = c.count()?;
    let mut portals = Vec::with_capacity(c.cap(n, 36));
    for i in 0..n {
        c.index = Some(i);
        portals.push(Portal {
            pos: c.vec3()?,
            country: c.u32()?,
            level_min: c.u16()?,
            level_max: c.u16()?,
            to_map: c.u32()?,
            to_pos: c.vec3()?,
        });
    }

    c.section(7);
    let n = c.count()?;
    let mut binds = Vec::with_capacity(c.cap(n, 12));
    for i in 0..n {
        c.index = Some(i);
        let m = c.count()?;
        let country = c.u32()?;
        let attr = c.u32()?;
        let mut boxes = Vec::with_capacity(c.cap(m, 24));
        for _ in 0..m {
            boxes.push(Aabb { min: c.vec3()?, max: c.vec3()? });
        }
        binds.push(Bind { country, attr, boxes });
    }

    c.section(8);
    let n = c.count()?;
    let mut regions = Vec::with_capacity(c.cap(n, 32));
    for i in 0..n {
        c.index = Some(i);
        regions.push(Region {
            min: c.vec3()?,
            max: c.vec3()?,
            kind: c.u32()?,
            b: c.u8()?,
            c: c.u8()?,
            d: c.u16()?,
        });
    }

    if c.left() > 0 {
        c.index = None;
        return Err(c.err(ReadErrorKind::TrailingBytes { count: c.left() }));
    }

    Ok(Svmap { size, blocked, section_a, points_b, mob_areas, npcs, portals, binds, regions })
}
