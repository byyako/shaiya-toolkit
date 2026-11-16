//! Known-answer tests for the SEED wrapper. The expected bytes were produced once by the Python reference of the Shaiya EP4 Client
//! repo (`tools/sdata_crypt.py`, `encrypt()`), with this command run from any folder:
//!
//!     python -I make_vectors.py
//!
//! where make_vectors.py puts `<client repo>/tools` on `sys.path`, imports `sdata_crypt as s` and prints
//! `s.encrypt(b"")`, `s.encrypt(b"hello")`, `s.encrypt(bytes(range(37)))` as hex and the SHA-256 of
//! `s.encrypt(bytes(i % 251 for i in range(65536)))`. Each was checked to decrypt back with `s.decrypt`.

use shaiya_core::hash::{sha256, to_hex};
use shaiya_seed::{decrypt, encrypt};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

const EMPTY: &str = "30303031434243454243354232373834443346433941324139444238344431433346454236453939000000000000000000000000000000000000000000000000";
const HELLO: &str = "3030303143424345424335423237383444334643394132413944423834443143334645423645393986a61036050000000000000000000000000000000000000040eb466f057c6c93f7b0f4ccdebdfba9";
const SEQ37: &str = "30303031434243454243354232373834443346433941324139444238344431433346454236453939e9ef2282250000000000000000000000000000000000000099b30c054dac6a680aa9198163cbfbe1c9bf2f6868d232634684cfc4e3f6e6cecb77cd17c2f5f35e77a4751c1507f331";
const BIG_SHA256: &str = "2424483d0e61322925c3efafd8ecbc49057b1de7e2fd1b6d73ccd8ee08834fe7";

#[test]
fn empty_matches_python() {
    assert_eq!(encrypt(b""), unhex(EMPTY));
    assert_eq!(decrypt(&unhex(EMPTY), "empty").unwrap(), b"");
}

#[test]
fn hello_matches_python() {
    assert_eq!(encrypt(b"hello"), unhex(HELLO));
    assert_eq!(decrypt(&unhex(HELLO), "hello").unwrap(), b"hello");
}

#[test]
fn sequence_of_37_bytes_matches_python() {
    let plain: Vec<u8> = (0..=36u8).collect();
    assert_eq!(encrypt(&plain), unhex(SEQ37));
    assert_eq!(decrypt(&unhex(SEQ37), "seq").unwrap(), plain);
}

#[test]
fn big_buffer_matches_python_digest() {
    let plain: Vec<u8> = (0..65536usize).map(|i| (i % 251) as u8).collect();
    let enc = encrypt(&plain);
    assert_eq!(to_hex(&sha256(&enc)), BIG_SHA256);
    assert_eq!(decrypt(&enc, "big").unwrap(), plain);
}
