//! Round trips and robustness of the SEED wrapper on synthetic data.

use shaiya_seed::{decrypt_unchecked, encrypt, info, unwrap, wrap, Wrapper, HEADER_SIZE, SIGNATURE};

fn sample(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(31) ^ (i >> 3)) as u8).collect()
}

/// An encrypted file the way the game's own tools may write it: non-zero header padding and non-zero bytes after the plaintext.
fn awkward(plain: &[u8]) -> Vec<u8> {
    let wrapper = Wrapper::Seed { crc: 0, size: plain.len() as u32, padding: *b"padding-padding!", tail: vec![0xA5; plain.len().div_ceil(16) * 16 - plain.len()] };
    wrap(plain, &wrapper)
}

#[test]
fn wrap_unwrap_is_identity_for_plain_and_encrypted_data() {
    let lengths: Vec<usize> = (0..=64).chain([65_536]).collect();
    for n in lengths {
        let plain = sample(n);
        // plain files
        let (p, w) = unwrap(&plain, "plain").unwrap();
        assert_eq!(w, Wrapper::Plain);
        assert_eq!(wrap(&p, &w), plain);
        // zero padding and zero tail (what encrypt writes)
        let enc = encrypt(&plain);
        let (p, w) = unwrap(&enc, "enc").unwrap();
        assert_eq!(p, plain);
        assert_eq!(wrap(&p, &w), enc, "length {n}");
        // non-zero padding and tail must survive too
        let odd = awkward(&plain);
        let (p, w) = unwrap(&odd, "odd").unwrap();
        assert_eq!(p, plain);
        assert_eq!(wrap(&p, &w), odd, "length {n} with padding and tail");
    }
}

#[test]
fn wrap_with_a_changed_length_recomputes_size_crc_and_drops_the_tail() {
    let plain = sample(21);
    let odd = awkward(&plain);
    let (_, w) = unwrap(&odd, "odd").unwrap();
    let longer = sample(40);
    let out = wrap(&longer, &w);
    let (p, w2) = unwrap(&out, "out").unwrap();
    assert_eq!(p, longer);
    match w2 {
        Wrapper::Seed { padding, tail, size, .. } => {
            assert_eq!(&padding, b"padding-padding!", "header padding is reused");
            assert_eq!(size, 40);
            assert_eq!(tail, vec![0u8; 8], "zero fill after a length change");
        }
        Wrapper::Plain => panic!("must stay wrapped"),
    }
}

#[test]
fn every_truncation_of_a_seed_file_is_an_error() {
    let enc = encrypt(&sample(50));
    for cut in 0..enc.len() {
        let t = &enc[..cut];
        // below the signature length the data is simply "not encrypted" and passes through as a plain file
        let r = unwrap(t, "cut");
        if cut >= SIGNATURE.len() {
            assert!(r.is_err(), "cut at {cut} must fail");
        } else {
            assert!(matches!(r, Ok((_, Wrapper::Plain))), "cut at {cut}");
        }
    }
}

#[test]
fn a_bit_flip_never_panics_and_a_flip_in_the_body_fails_the_checksum() {
    let enc = encrypt(&sample(100));
    for i in 0..enc.len() {
        let mut m = enc.clone();
        m[i] ^= 0x10;
        let r = unwrap(&m, "flip");
        let _ = info(&m, "flip");
        let _ = decrypt_unchecked(&m, "flip");
        if i >= HEADER_SIZE {
            assert!(r.is_err(), "a flip in the body at {i} must fail the checksum");
        }
        if i < 40 {
            // the signature is damaged: the file is no longer recognised and passes through as plain data
            assert!(matches!(r, Ok((_, Wrapper::Plain))));
        }
    }
}

#[test]
fn header_padding_flips_are_tolerated_and_kept() {
    let enc = encrypt(&sample(30));
    let mut m = enc.clone();
    m[50] ^= 0xFF;
    let (p, w) = unwrap(&m, "pad").unwrap();
    assert_eq!(p, sample(30));
    assert_eq!(wrap(&p, &w), m);
}
