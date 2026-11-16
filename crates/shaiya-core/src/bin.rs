//! A bounds-checked little-endian reader and a matching writer. A read past the end is `Error::Format` with the byte offset; it never panics.

use crate::error::{Error, Result};

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    file: &'a str,
}

macro_rules! num {
    ($name:ident, $t:ty, $n:expr) => {
        pub fn $name(&mut self, what: &str) -> Result<$t> {
            let b = self.bytes($n, what)?;
            Ok(<$t>::from_le_bytes(b.try_into().expect("length checked")))
        }
    };
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8], file: &'a str) -> Reader<'a> {
        Reader { data, pos: 0, file }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn left(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn is_done(&self) -> bool {
        self.pos >= self.data.len()
    }
    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(self.fail(format!("position {pos} is past the end ({} bytes)", self.data.len())));
        }
        self.pos = pos;
        Ok(())
    }
    pub fn bytes(&mut self, n: usize, what: &str) -> Result<&'a [u8]> {
        if n > self.left() {
            return Err(self.fail(format!("{what}: need {n} bytes, {} left", self.left())));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize, what: &str) -> Result<()> {
        self.bytes(n, what).map(|_| ())
    }
    num!(u8, u8, 1);
    num!(i8, i8, 1);
    num!(u16, u16, 2);
    num!(i16, i16, 2);
    num!(u32, u32, 4);
    num!(i32, i32, 4);
    num!(u64, u64, 8);
    num!(i64, i64, 8);
    num!(f32, f32, 4);
    /// A `u32` count, sanity-checked: at most `limit`, and `count * elem_size` must fit in what is left.
    pub fn count(&mut self, elem_size: usize, limit: usize, what: &str) -> Result<usize> {
        let start = self.pos;
        let n = self.u32(what)? as usize;
        if n > limit {
            self.pos = start;
            return Err(self.fail(format!("{what}: count {n} is over the limit {limit}")));
        }
        if n.checked_mul(elem_size).is_none_or(|b| b > self.left()) {
            self.pos = start;
            return Err(self.fail(format!("{what}: count {n} needs more bytes than the {} left", self.left().saturating_sub(4))));
        }
        Ok(n)
    }
    /// An `Error::Format` at the current position.
    pub fn fail(&self, what: impl Into<String>) -> Error {
        Error::format(self.file, Some(self.pos as u64), what)
    }
    /// Error when bytes are left over.
    pub fn end(&self, what: &str) -> Result<()> {
        if self.left() != 0 {
            return Err(self.fail(format!("{what}: {} unexpected bytes left over", self.left())));
        }
        Ok(())
    }
}

/// Little-endian writer over a `Vec<u8>`.
#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

macro_rules! put {
    ($name:ident, $t:ty) => {
        pub fn $name(&mut self, v: $t) {
            self.buf.extend_from_slice(&v.to_le_bytes());
        }
    };
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }
    put!(u8, u8);
    put!(i8, i8);
    put!(u16, u16);
    put!(i16, i16);
    put!(u32, u32);
    put!(i32, i32);
    put!(u64, u64);
    put!(i64, i64);
    put!(f32, f32);
    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_width_and_tracks_position() {
        let mut w = Writer::new();
        w.u8(1);
        w.i8(-2);
        w.u16(0x0302);
        w.i16(-3);
        w.u32(0xdead_beef);
        w.i32(-4);
        w.u64(1 << 40);
        w.i64(-5);
        w.f32(1.5);
        w.bytes(b"tail");
        let mut r = Reader::new(&w.buf, "t.bin");
        assert_eq!(r.u8("a").unwrap(), 1);
        assert_eq!(r.i8("b").unwrap(), -2);
        assert_eq!(r.u16("c").unwrap(), 0x0302);
        assert_eq!(r.i16("d").unwrap(), -3);
        assert_eq!(r.u32("e").unwrap(), 0xdead_beef);
        assert_eq!(r.i32("f").unwrap(), -4);
        assert_eq!(r.u64("g").unwrap(), 1 << 40);
        assert_eq!(r.i64("h").unwrap(), -5);
        assert_eq!(r.f32("i").unwrap(), 1.5);
        assert_eq!(r.pos(), 1 + 1 + 2 + 2 + 4 + 4 + 8 + 8 + 4);
        assert_eq!(r.bytes(4, "tail").unwrap(), b"tail");
        assert!(r.is_done());
        r.end("done").unwrap();
    }

    #[test]
    fn short_reads_report_the_offset() {
        let data = [1u8, 2, 3];
        let mut r = Reader::new(&data, "x.bin");
        r.u8("first").unwrap();
        let e = r.u32("a word").unwrap_err();
        match e {
            Error::Format { file, offset, what } => {
                assert_eq!(file, "x.bin");
                assert_eq!(offset, Some(1));
                assert!(what.contains("a word"), "{what}");
            }
            other => panic!("{other:?}"),
        }
        // a failed read does not move the position
        assert_eq!(r.pos(), 1);
        assert!(r.seek(4).is_err());
        r.seek(3).unwrap();
        assert!(r.u8("past end").is_err());
        assert!(Reader::new(&data, "x").end("trailing").is_err());
    }

    #[test]
    fn count_is_sanity_checked() {
        let mut w = Writer::new();
        w.u32(3);
        w.bytes(&[0; 12]);
        assert_eq!(Reader::new(&w.buf, "c").count(4, 10, "records").unwrap(), 3);
        assert!(Reader::new(&w.buf, "c").count(4, 2, "records").is_err()); // over the limit
        assert!(Reader::new(&w.buf, "c").count(8, 10, "records").is_err()); // needs 24, has 12
        let mut huge = Writer::new();
        huge.u32(u32::MAX);
        let e = Reader::new(&huge.buf, "c").count(usize::MAX, usize::MAX, "records").unwrap_err();
        assert!(matches!(e, Error::Format { offset: Some(0), .. }));
    }
}
