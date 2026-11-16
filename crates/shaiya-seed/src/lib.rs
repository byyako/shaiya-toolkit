//! SEED encryption of `.SData` files (KISA SEED in ECB mode with a fixed key), moved from the client repo's `clientdata::seed`
//! (decrypt) and ported from `tools/sdata_crypt.py` (encrypt).
//!
//! Wrapper: 40-byte signature, u32 CRC-32 of the plaintext, u32 plaintext length, 16 bytes padding, then the plaintext zero-padded to
//! 16 bytes and encrypted block by block. [`Wrapper`] remembers the parts of a file that are not the plaintext (header padding and the
//! decrypted bytes after the plaintext in the last block), so [`wrap`] can reproduce an unchanged file exactly.

mod tables;

use shaiya_core::{Error, Result};
use tables::{K, SS};

pub const SIGNATURE: &[u8; 40] = b"0001CBCEBC5B2784D3FC9A2A9DB84D1C3FEB6E99";
/// Signature 40, crc u32, size u32, padding 16.
pub const HEADER_SIZE: usize = 64;

pub fn is_encrypted(data: &[u8]) -> bool {
    data.len() >= 40 && &data[..40] == SIGNATURE
}

fn g(v: u32) -> u32 {
    SS[0][(v & 0xff) as usize] ^ SS[1][((v >> 8) & 0xff) as usize] ^ SS[2][((v >> 16) & 0xff) as usize] ^ SS[3][(v >> 24) as usize]
}

fn f(r0: u32, r1: u32, k: usize) -> (u32, u32) {
    let mut t0 = r0 ^ K[k];
    let mut t1 = (r1 ^ K[k + 1]) ^ t0;
    t1 = g(t1);
    t0 = g(t0.wrapping_add(t1));
    t1 = g(t1.wrapping_add(t0));
    t0 = t0.wrapping_add(t1);
    (t0, t1)
}

/// One 16-byte block. Decryption uses the round keys from the end (`30, 28, ...`), encryption from the start (`0, 2, ...`).
fn crypt_block(block: &[u8], encrypt: bool) -> [u8; 16] {
    let w = |i: usize| u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().expect("16-byte block"));
    let (mut l0, mut l1, mut r0, mut r1) = (w(0), w(1), w(2), w(3));
    for i in 0..16usize {
        let k = if encrypt { i * 2 } else { 30 - i * 2 };
        if i % 2 == 0 {
            let (a, b) = f(r0, r1, k);
            l0 ^= a;
            l1 ^= b;
        } else {
            let (a, b) = f(l0, l1, k);
            r0 ^= a;
            r1 ^= b;
        }
    }
    let mut out = [0u8; 16];
    for (i, v) in [r0, r1, l0, l1].into_iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

const CRC_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            j += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// Standard CRC-32 (zlib).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wrapper {
    Plain,
    /// `tail`: the decrypted bytes after `size` in the last block, kept so `wrap` reproduces the file exactly.
    Seed { crc: u32, size: u32, padding: [u8; 16], tail: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedInfo {
    pub crc: u32,
    pub size: u32,
    pub padding: [u8; 16],
    pub body_len: usize,
}

fn bad(file: &str, offset: u64, what: &str) -> Error {
    Error::format(file, Some(offset), what)
}

/// Header of an encrypted file; `None` when the data is not SEED-encrypted. Errors when the header or block length is wrong.
pub fn info(data: &[u8], file: &str) -> Result<Option<SeedInfo>> {
    if !is_encrypted(data) {
        return Ok(None);
    }
    if data.len() < HEADER_SIZE {
        return Err(bad(file, data.len() as u64, "SEED header is cut short"));
    }
    let body_len = data.len() - HEADER_SIZE;
    if body_len % 16 != 0 {
        return Err(bad(file, HEADER_SIZE as u64, "SEED body is not a whole number of 16-byte blocks"));
    }
    let crc = u32::from_le_bytes(data[40..44].try_into().expect("4 bytes"));
    let size = u32::from_le_bytes(data[44..48].try_into().expect("4 bytes"));
    if size as usize > body_len || body_len - size as usize > 15 {
        return Err(bad(file, 44, "SEED plaintext length does not match the encrypted body"));
    }
    let padding: [u8; 16] = data[48..64].try_into().expect("16 bytes");
    Ok(Some(SeedInfo { crc, size, padding, body_len }))
}

fn decrypt_body(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() - HEADER_SIZE);
    for block in data[HEADER_SIZE..].chunks_exact(16) {
        out.extend_from_slice(&crypt_block(block, false));
    }
    out
}

/// Plain files pass through as `Wrapper::Plain`; encrypted ones are decrypted and their CRC checked.
pub fn unwrap(data: &[u8], file: &str) -> Result<(Vec<u8>, Wrapper)> {
    let Some(h) = info(data, file)? else {
        return Ok((data.to_vec(), Wrapper::Plain));
    };
    let mut plain = decrypt_body(data);
    let tail = plain.split_off(h.size as usize);
    if crc32(&plain) != h.crc {
        return Err(bad(file, 40, "SEED checksum failed (damaged file or a different key)"));
    }
    Ok((plain, Wrapper::Seed { crc: h.crc, size: h.size, padding: h.padding, tail }))
}

fn build(plain: &[u8], padding: [u8; 16], tail: Option<&[u8]>) -> Vec<u8> {
    let padded_len = plain.len().div_ceil(16) * 16;
    let mut out = Vec::with_capacity(HEADER_SIZE + padded_len);
    out.extend_from_slice(SIGNATURE);
    out.extend_from_slice(&crc32(plain).to_le_bytes());
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    out.extend_from_slice(&padding);
    let mut padded = plain.to_vec();
    match tail {
        Some(t) if t.len() == padded_len - plain.len() => padded.extend_from_slice(t),
        _ => padded.resize(padded_len, 0),
    }
    for block in padded.chunks_exact(16) {
        out.extend_from_slice(&crypt_block(block, true));
    }
    out
}

/// Inverse of [`unwrap`]. Seed: recomputes crc and size from `plain`; reuses the padding; reuses the tail when the length is unchanged,
/// else zero fill.
pub fn wrap(plain: &[u8], wrapper: &Wrapper) -> Vec<u8> {
    match wrapper {
        Wrapper::Plain => plain.to_vec(),
        Wrapper::Seed { size, padding, tail, .. } => {
            let tail = (plain.len() == *size as usize).then_some(tail.as_slice());
            build(plain, *padding, tail)
        }
    }
}

/// Strict form for the CLI: errors when the signature is missing; the checksum is verified.
pub fn decrypt(data: &[u8], file: &str) -> Result<Vec<u8>> {
    if !is_encrypted(data) {
        return Err(bad(file, 0, "not a SEED-encrypted file (signature missing)"));
    }
    unwrap(data, file).map(|(p, _)| p)
}

/// Writes the wrapper with zero padding and a zero tail.
pub fn encrypt(plain: &[u8]) -> Vec<u8> {
    build(plain, [0; 16], None)
}

/// For inspection and repair: the plaintext even when the checksum fails, and whether it matched.
pub fn decrypt_unchecked(data: &[u8], file: &str) -> Result<(Vec<u8>, bool)> {
    let Some(h) = info(data, file)? else {
        return Err(bad(file, 0, "not a SEED-encrypted file (signature missing)"));
    };
    let mut plain = decrypt_body(data);
    plain.truncate(h.size as usize);
    let ok = crc32(&plain) == h.crc;
    Ok((plain, ok))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn plain_data_passes_through_and_short_headers_fail() {
        let (p, w) = unwrap(b"hello", "x").unwrap();
        assert_eq!(p, b"hello");
        assert_eq!(w, Wrapper::Plain);
        assert!(decrypt(b"hello", "x").is_err(), "strict decrypt wants the signature");
        let mut short = SIGNATURE.to_vec();
        short.extend_from_slice(&[0; 10]);
        assert!(unwrap(&short, "x").is_err());
        assert!(info(&short, "x").is_err());
        assert_eq!(info(b"hello", "x").unwrap(), None);
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        for n in [0usize, 1, 15, 16, 17, 40, 4097] {
            let plain: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            let enc = encrypt(&plain);
            assert_eq!(enc.len(), HEADER_SIZE + n.div_ceil(16) * 16);
            assert_eq!(decrypt(&enc, "t").unwrap(), plain);
            let (p, w) = unwrap(&enc, "t").unwrap();
            assert_eq!(p, plain);
            assert_eq!(wrap(&p, &w), enc);
        }
    }

    #[test]
    fn unchecked_decrypt_reports_a_bad_checksum() {
        let mut enc = encrypt(b"some plaintext that is long enough");
        enc[40] ^= 1; // the CRC field
        assert!(unwrap(&enc, "t").is_err());
        let (plain, ok) = decrypt_unchecked(&enc, "t").unwrap();
        assert_eq!(plain, b"some plaintext that is long enough");
        assert!(!ok);
    }
}
