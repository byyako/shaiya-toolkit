//! SHA-256 and hex helpers.

use crate::error::{Error, Result};
use crate::job::{Job, Unit};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Incremental SHA-256 (for hashing while streaming).
#[derive(Default)]
pub struct Hasher(Sha256);

impl Hasher {
    pub fn new() -> Hasher {
        Hasher(Sha256::new())
    }
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
    pub fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// Streams the file with a 1 MiB buffer and reports `Unit::Bytes` progress; stops with `Error::Cancelled` when the job is cancelled.
pub fn sha256_file(path: &Path, job: &Job) -> Result<[u8; 32]> {
    let mut f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let len = f.metadata().map(|m| m.len()).ok();
    job.total(len, Unit::Bytes);
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        job.check()?;
        let n = f.read(&mut buf).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        job.advance(n as u64);
    }
    Ok(h.finalize().into())
}

pub fn to_hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
}

pub fn from_hex<const N: usize>(s: &str) -> Result<[u8; N]> {
    let s = s.trim();
    let bad = |what: &str| Error::format("hex", None, what.to_string());
    if s.len() != N * 2 || !s.is_ascii() {
        return Err(bad(&format!("expected {} hex digits, got {}", N * 2, s.len())));
    }
    let mut out = [0u8; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| bad("not a hex digit"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{Cancel, Silent};

    #[test]
    fn sha256_known_answer() {
        assert_eq!(to_hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(to_hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn hex_round_trip_and_errors() {
        let b: [u8; 4] = from_hex("00ff10Ab").unwrap();
        assert_eq!(b, [0, 255, 16, 171]);
        assert_eq!(to_hex(&b), "00ff10ab");
        assert!(from_hex::<4>("00ff10").is_err());
        assert!(from_hex::<2>("zz00").is_err());
        assert!(from_hex::<2>("00\u{e9}").is_err());
    }

    #[test]
    fn file_hash_streams_and_cancels() {
        let dir = std::env::temp_dir().join(format!("shaiya-core-hash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.bin");
        let data: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&p, &data).unwrap();
        assert_eq!(sha256_file(&p, &Job::silent()).unwrap(), sha256(&data));
        let c = Cancel::new();
        c.cancel();
        assert!(matches!(sha256_file(&p, &Job::new(&Silent, c)), Err(Error::Cancelled)));
        assert!(sha256_file(&dir.join("missing"), &Job::silent()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
