//! SEED decryption of `.SData` files. The cipher lives in the `shaiya-seed` crate; this module keeps the names this crate has always
//! exported.

use crate::error::{Error, Result};

pub use shaiya_seed::{crc32, is_encrypted, HEADER_SIZE, SIGNATURE};

/// Plaintext of an SData file; a file without the signature is returned as it is. The checksum is verified.
pub fn decrypt(data: &[u8], file: &str) -> Result<Vec<u8>> {
    match shaiya_seed::unwrap(data, file) {
        Ok((plain, _)) => Ok(plain),
        Err(shaiya_core::Error::Format { file, offset, what }) => Err(Error::Format { file, what, offset: offset.unwrap_or(0) as usize }),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_data_passes_through_and_short_headers_fail() {
        assert_eq!(decrypt(b"hello", "x").unwrap(), b"hello");
        let mut short = SIGNATURE.to_vec();
        short.extend_from_slice(&[0; 10]);
        assert!(decrypt(&short, "x").is_err());
    }

    #[test]
    fn an_encrypted_file_decrypts_and_a_damaged_one_reports_format() {
        let enc = shaiya_seed::encrypt(b"monster data");
        assert_eq!(decrypt(&enc, "m").unwrap(), b"monster data");
        let mut bad = enc.clone();
        bad[HEADER_SIZE] ^= 1;
        assert!(matches!(decrypt(&bad, "m"), Err(Error::Format { .. })));
    }
}
