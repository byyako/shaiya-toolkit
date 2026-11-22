//! Errors of the client data readers. Every parse error names the file and the byte offset.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Reading from disk failed.
    Io { path: String, source: std::io::Error },
    /// The client has no such file (archive entry or loose file).
    NotFound(String),
    /// A file did not parse: which file, what was being read, at which byte.
    Format { file: String, what: String, offset: usize },
    /// The archive index could not be read.
    Archive(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{path}: {source}"),
            Error::NotFound(p) => write!(f, "{p}: not in the client data"),
            Error::Format { file, what, offset } => write!(f, "{file}: {what} (at byte {offset})"),
            Error::Archive(s) => write!(f, "archive: {s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<shaiya_core::Error> for Error {
    fn from(e: shaiya_core::Error) -> Self {
        Error::Archive(e.to_string())
    }
}

/// Bounds-checked little-endian reader over a byte slice; every failure becomes an `Error::Format` with the offset.
pub(crate) struct Cur<'a> {
    pub d: &'a [u8],
    pub p: usize,
    pub file: &'a str,
}

impl<'a> Cur<'a> {
    pub fn new(d: &'a [u8], file: &'a str) -> Self {
        Cur { d, p: 0, file }
    }

    pub fn fail(&self, what: impl Into<String>) -> Error {
        Error::Format { file: self.file.to_string(), what: what.into(), offset: self.p }
    }

    pub fn left(&self) -> usize {
        self.d.len() - self.p
    }

    pub fn done(&self) -> bool {
        self.p == self.d.len()
    }

    pub fn bytes(&mut self, n: usize, what: &str) -> Result<&'a [u8]> {
        if n > self.left() {
            return Err(self.fail(format!("{what}: needs {n} bytes, {} left", self.left())));
        }
        let s = &self.d[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }

    pub fn skip(&mut self, n: usize, what: &str) -> Result<()> {
        self.bytes(n, what).map(|_| ())
    }

    pub fn u8(&mut self, what: &str) -> Result<u8> {
        Ok(self.bytes(1, what)?[0])
    }
    pub fn u16(&mut self, what: &str) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2, what)?.try_into().unwrap()))
    }
    pub fn i16(&mut self, what: &str) -> Result<i16> {
        Ok(self.u16(what)? as i16)
    }
    pub fn u32(&mut self, what: &str) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4, what)?.try_into().unwrap()))
    }
    pub fn i32(&mut self, what: &str) -> Result<i32> {
        Ok(self.u32(what)? as i32)
    }
    pub fn f32(&mut self, what: &str) -> Result<f32> {
        Ok(f32::from_bits(self.u32(what)?))
    }
    pub fn f3(&mut self, what: &str) -> Result<[f32; 3]> {
        Ok([self.f32(what)?, self.f32(what)?, self.f32(what)?])
    }

    /// A signed count of records of `elem` bytes each; must be >= 0, <= `limit`, and fit in the rest of the file
    /// (`elem` is the smallest possible size of one record), so a bad count never allocates much.
    pub fn count(&mut self, elem: usize, limit: usize, what: &str) -> Result<usize> {
        let at = self.p;
        let n = self.i32(what)?;
        if n < 0 || n as usize > limit {
            self.p = at;
            return Err(self.fail(format!("{what}: bad count {n}")));
        }
        let n = n as usize;
        if elem > 0 && n.saturating_mul(elem) > self.left() {
            self.p = at;
            return Err(self.fail(format!("{what}: count {n} does not fit in the file")));
        }
        Ok(n)
    }

    /// `i32 length` + bytes (cp949, NUL-terminated), the string layout of SData files.
    pub fn str_i32(&mut self, what: &str) -> Result<String> {
        let at = self.p;
        let n = self.u32(what)? as usize;
        if n > self.left() || n > 1 << 20 {
            self.p = at;
            return Err(self.fail(format!("{what}: bad string length {n}")));
        }
        let raw = self.bytes(n, what)?;
        Ok(decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[])))
    }

    /// Like `str_i32`, but gives the raw bytes exactly as stored (the NUL and anything after it included), so a writer can give them back.
    pub fn raw_str_i32(&mut self, what: &str) -> Result<&'a [u8]> {
        let at = self.p;
        let n = self.u32(what)? as usize;
        if n > self.left() || n > 1 << 20 {
            self.p = at;
            return Err(self.fail(format!("{what}: bad string length {n}")));
        }
        self.bytes(n, what)
    }

    /// Fixed 256-byte name buffer; bytes after the NUL are garbage from the map tool.
    pub fn name256(&mut self, what: &str) -> Result<String> {
        let raw = self.bytes(256, what)?;
        Ok(decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[])))
    }

    pub fn name_n(&mut self, n: usize, what: &str) -> Result<String> {
        let raw = self.bytes(n, what)?;
        Ok(decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[])))
    }

    pub fn end(&self, what: &str) -> Result<()> {
        if self.done() {
            Ok(())
        } else {
            Err(self.fail(format!("{what}: {} trailing bytes", self.left())))
        }
    }
}

pub fn decode_cp949(raw: &[u8]) -> String {
    encoding_rs::EUC_KR.decode_without_bom_handling(raw).0.into_owned()
}
