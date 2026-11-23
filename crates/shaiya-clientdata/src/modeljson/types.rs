//! Value types of the model JSON: `F32` (a float as a number, or as `"bits:XXXXXXXX"` when a number would not read back to the same bits)
//! and hex helpers for byte strings.

use super::JsonError;
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;

/// A 32-bit float in the JSON.
///
/// Written as a JSON number in its shortest form when that text reads back to the same bits (`text.parse::<f64>()` then `as f32`, the way
/// the import reads it); otherwise (NaN, infinity, or a value whose text would not come back) as the string `"bits:XXXXXXXX"`. Read from a
/// number or from that string.
#[derive(Clone, Copy, Debug)]
pub(crate) struct F32(pub f32);

impl F32 {
    fn number_reads_back(v: f32) -> bool {
        match serde_json::to_string(&v) {
            Ok(text) => text.parse::<f64>().map(|x| (x as f32).to_bits()) == Ok(v.to_bits()),
            Err(_) => false,
        }
    }
}

impl Serialize for F32 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.0.is_finite() && Self::number_reads_back(self.0) {
            s.serialize_f32(self.0)
        } else {
            s.serialize_str(&format!("bits:{:08x}", self.0.to_bits()))
        }
    }
}

/// The text of the error that [`F32`] gives for a number that does not fit.
pub(crate) const OUT_OF_RANGE: &str = "out of the 32-bit float range";

struct F32Visitor {
    /// A number too large for 32 bits reads as infinity instead of failing (see [`KeyF32`]).
    overflow_as_infinity: bool,
}

impl<'de> Visitor<'de> for F32Visitor {
    type Value = F32;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a float is a number or \"bits:XXXXXXXX\"")
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<F32, E> {
        let f = v as f32;
        if v.is_finite() && f.is_infinite() {
            if self.overflow_as_infinity {
                return Ok(F32(f));
            }
            return Err(E::custom(OUT_OF_RANGE));
        }
        Ok(F32(f))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<F32, E> {
        self.visit_f64(v as f64)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<F32, E> {
        self.visit_f64(v as f64)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<F32, E> {
        let digits = v.strip_prefix("bits:").filter(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_hexdigit()));
        match digits.and_then(|d| u32::from_str_radix(d, 16).ok()) {
            Some(bits) => Ok(F32(f32::from_bits(bits))),
            None => Err(E::custom("a float is a number or \"bits:XXXXXXXX\"")),
        }
    }
}

impl<'de> Deserialize<'de> for F32 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<F32, D::Error> {
        d.deserialize_any(F32Visitor { overflow_as_infinity: false })
    }
}

/// A float that is read like [`F32`], except that a number too large for 32 bits reads as infinity (never wrapped or clamped). For a value
/// the caller refuses when it is not finite: the caller's check then names the field (`bones[3].rotation_keys[5]`), where a failure inside
/// the parser can only give a line and column. Written like an [`F32`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct KeyF32(pub f32);

impl<'de> Deserialize<'de> for KeyF32 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<KeyF32, D::Error> {
        d.deserialize_any(F32Visitor { overflow_as_infinity: true }).map(|f| KeyF32(f.0))
    }
}

/// A value that must be written as a JSON object: the positional (array) form that serde accepts for a struct is refused, so every field is
/// named and the documented schema is the only one.
pub(crate) struct Obj<T>(pub T);

struct ObjVisitor<T>(std::marker::PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjVisitor<T> {
    type Value = Obj<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object with named keys")
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Obj<T>, A::Error> {
        T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Obj)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Obj<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Obj<T>, D::Error> {
        d.deserialize_map(ObjVisitor(std::marker::PhantomData))
    }
}

/// For a key whose value may be `null` but that must be present (`Option` fields are otherwise optional in serde): use as
/// `#[serde(deserialize_with = "required")]`.
pub(crate) fn required<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

/// The start of the error text of a [`Bounded`] list that is over its limit ([`json_error`](super::json_error) turns it into `TooLarge`).
pub(crate) const TOO_MANY: &str = "too many entries";

/// The most entries a `columns` list may hold (the documented lists have fewer than ten).
pub(crate) const COLUMN_LIMIT: usize = 64;

/// A list that is read with a bound: the reader stops with an error at entry `N + 1` instead of building the whole list first, so a
/// hand-made file with millions of tiny entries cannot exhaust memory before the limit check. Derefs to the `Vec`.
#[derive(Debug)]
pub(crate) struct Bounded<T, const N: usize>(pub Vec<T>);

impl<T, const N: usize> std::ops::Deref for Bounded<T, N> {
    type Target = Vec<T>;

    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

struct BoundedVisitor<T, const N: usize>(std::marker::PhantomData<T>);

impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for BoundedVisitor<T, N> {
    type Value = Bounded<T, N>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list")
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<Bounded<T, N>, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096).min(N));
        while let Some(x) = seq.next_element::<T>()? {
            if out.len() >= N {
                return Err(de::Error::custom(format!("{TOO_MANY}: a list may hold at most {N}")));
            }
            out.push(x);
        }
        Ok(Bounded(out))
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Bounded<T, N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_seq(BoundedVisitor::<T, N>(std::marker::PhantomData))
    }
}

/// A triangle: three `u16` indices, written on one line.
pub(crate) type Face = [u16; 3];

/// `Ok` when the `columns` list of a JSON equals the documented one exactly (a reordered or renamed column is never read silently).
pub(crate) fn check_columns(found: &[String], want: &[&str], path: &str) -> Result<(), JsonError> {
    if found.len() == want.len() && found.iter().zip(want).all(|(a, b)| a == b) {
        return Ok(());
    }
    Err(JsonError::Schema(format!("{path} must be {want:?}, found {found:?}")))
}

/// `Ok` when `n` entries are within the reader's limit `max`.
pub(crate) fn check_limit(n: usize, max: usize, path: &str) -> Result<(), JsonError> {
    if n > max {
        return Err(JsonError::TooLarge(format!("{path}: {n} entries is over the limit of {max}")));
    }
    Ok(())
}

/// The encoded size of a model, summed with checked arithmetic (an overflow counts as too large).
pub(crate) struct Size(Option<u64>);

impl Size {
    pub fn new(start: u64) -> Self {
        Size(Some(start))
    }

    pub fn add(&mut self, n: u64) {
        self.0 = self.0.and_then(|s| s.checked_add(n));
    }

    /// Adds `count * each` bytes.
    pub fn add_mul(&mut self, count: usize, each: u64) {
        let n = (count as u64).checked_mul(each);
        self.0 = self.0.zip(n).and_then(|(s, n)| s.checked_add(n));
    }

    /// The total, refused when it overflowed or is over the model cap.
    pub fn finish(self, model_cap: u64) -> Result<u64, JsonError> {
        let total = self.0.ok_or_else(|| JsonError::TooLarge("the model is larger than the size limit".into()))?;
        super::check_model_len(total, model_cap)?;
        Ok(total)
    }
}

/// The 16 floats of a matrix as 64 bytes in file order (kept bit for bit).
pub(crate) fn matrix_bytes(m: &[f32; 16]) -> Vec<u8> {
    m.iter().flat_map(|f| f.to_bits().to_le_bytes()).collect()
}

/// The matrix of 64 stored bytes.
pub(crate) fn matrix_from_bytes(raw: &[u8; 64]) -> [f32; 16] {
    let mut m = [0f32; 16];
    for (i, c) in m.iter_mut().enumerate() {
        *c = f32::from_bits(u32::from_le_bytes([raw[4 * i], raw[4 * i + 1], raw[4 * i + 2], raw[4 * i + 3]]));
    }
    m
}

/// A texture name from its hex: the length is checked against `limit` before the bytes are decoded.
pub(crate) fn texture_from_hex(text: &str, limit: usize, path: &str) -> Result<Vec<u8>, JsonError> {
    check_limit(text.len() / 2, limit, path)?;
    hex_decode(text, path)
}

/// Lower-case hex of `bytes`, in file order.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

/// Hex (either case) to bytes; an odd length or a non-hex character is refused with the field `path`.
pub(crate) fn hex_decode(text: &str, path: &str) -> Result<Vec<u8>, JsonError> {
    let b = text.as_bytes();
    if !b.len().is_multiple_of(2) {
        return Err(JsonError::Schema(format!("{path}: hex needs an even number of digits, found {}", b.len())));
    }
    let nibble = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks(2) {
        match (nibble(pair[0]), nibble(pair[1])) {
            (Some(h), Some(l)) => out.push(h << 4 | l),
            _ => return Err(JsonError::Schema(format!("{path}: hex holds a character that is not a hex digit"))),
        }
    }
    Ok(out)
}

/// Hex of exactly `N` bytes (`2 * N` digits).
pub(crate) fn hex_exact<const N: usize>(text: &str, path: &str) -> Result<[u8; N], JsonError> {
    if text.len() != 2 * N {
        return Err(JsonError::Schema(format!("{path}: expected {} hex digits ({N} bytes), found {}", 2 * N, text.len())));
    }
    let v = hex_decode(text, path)?;
    let mut a = [0u8; N];
    a.copy_from_slice(&v);
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(v: f32) -> String {
        let mut out = Vec::new();
        F32(v).serialize(&mut serde_json::Serializer::new(&mut out)).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn read(text: &str) -> Result<F32, serde_json::Error> {
        serde_json::from_str(text)
    }

    #[test]
    fn the_float_forms_round_trip() {
        assert_eq!(written(0.1), "0.1");
        assert_eq!(written(1.0), "1.0");
        assert_eq!(written(-0.0), "-0.0");
        for v in [0.1f32, 1.0, -0.0, f32::MIN_POSITIVE, f32::from_bits(500), f32::MAX, f32::MIN, f32::from_bits(1), 7e-43] {
            let text = written(v);
            assert!(!text.starts_with('"'), "{v:e} -> {text}");
            assert_eq!(read(&text).unwrap().0.to_bits(), v.to_bits(), "{text}");
        }
        assert_eq!(written(f32::from_bits(0x7fc0_0001)), "\"bits:7fc00001\"");
        assert_eq!(written(f32::INFINITY), "\"bits:7f800000\"");
        assert_eq!(read("\"bits:7fc00001\"").unwrap().0.to_bits(), 0x7fc0_0001);
        assert_eq!(read("\"bits:FFC00000\"").unwrap().0.to_bits(), 0xffc0_0000);
        assert_eq!(read("3").unwrap().0, 3.0);
        assert_eq!(read("-2").unwrap().0, -2.0);
        for bad in ["\"bits:7fc0000\"", "\"bits:7fc000011\"", "\"0.5\"", "\"bits:zzzzzzzz\"", "true", "null", "[1]"] {
            assert!(read(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_number_too_large_for_f32_is_refused() {
        let e = read("1e39").unwrap_err().to_string();
        assert!(e.contains(OUT_OF_RANGE), "{e}");
        assert!(read("-1e39").is_err());
        assert!(read("3.4028235e38").is_ok());
        // a huge integer is refused the same way
        assert!(read("100000000000000000000000000000000000000000").is_err());
    }

    #[test]
    fn many_float_bit_patterns_round_trip_through_the_json_number() {
        let mut patterns: Vec<u32> = Vec::new();
        let mut x: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..200_000 {
            // a fixed linear congruential generator (Knuth's MMIX constants)
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            patterns.push((x >> 32) as u32);
        }
        for exp in 0u32..=255 {
            for mant in [0u32, 1, 2, 0x3f_ffff, 0x40_0000, 0x7f_fffe, 0x7f_ffff] {
                for sign in [0u32, 1] {
                    patterns.push(sign << 31 | exp << 23 | mant);
                }
            }
        }
        for mant in (0u32..=0x7f_ffff).step_by(997) {
            patterns.push(mant);
            patterns.push(0x8000_0000 | mant);
        }
        let (mut fallbacks, mut finite_fallbacks) = (0usize, 0usize);
        for bits in patterns {
            let v = f32::from_bits(bits);
            let text = written(v);
            if text.starts_with('"') {
                fallbacks += 1;
                if v.is_finite() {
                    finite_fallbacks += 1;
                    // the fallback for a finite value must still come back
                    assert_eq!(read(&text).unwrap().0.to_bits(), bits, "{text}");
                }
                continue;
            }
            assert_eq!(read(&text).unwrap().0.to_bits(), bits, "{bits:08x} -> {text}");
        }
        // not asserted: how many finite values took the `bits:` form (expected 0); NaN and infinity always do
        println!("bits: fallbacks in the run: {fallbacks} (finite values: {finite_fallbacks}, expected 0)");
    }

    #[test]
    fn the_float_roundtrip_feature_is_on() {
        // serde_json's own number parser must equal `str::parse` bit for bit: the `float_roundtrip` feature of the Cargo.toml line
        let texts = [
            "7e-43",
            "1.4e-45",
            "3.4028235e38",
            "0.1",
            "1e22",
            "1e23",
            "8.41e21",
            "2.2250738585072011e-308",
            "5e-324",
            "1.7976931348623157e308",
            "0.30000000000000004",
            "123456789012345678",
            "9007199254740993",
            "1.00000000000000011102230246251565404236316680908203125",
            "4.9406564584124654e-324",
            "2.4703282292062327e-324",
            "1e-30",
            "1e+30",
            "123456.78901234567e-10",
            "0.000123456789012345678e30",
            "722818e-26",
            // these four read one unit in the last place differently from `str::parse` without the `float_roundtrip` feature
            "0.10957860598549463e-10",
            "0.79386843516156103e28",
            "0.16164283719081163e18",
            "0.47795269883162916e4",
        ];
        for t in texts {
            let a: f64 = serde_json::from_str(t).unwrap();
            let b: f64 = t.parse().unwrap();
            assert_eq!(a.to_bits(), b.to_bits(), "{t}");
        }
    }

    #[test]
    fn bad_hex_is_refused() {
        assert!(hex_decode("abc", "p").is_err(), "odd length");
        assert!(hex_decode("zz", "p").is_err());
        assert!(hex_exact::<2>("00ff0", "p").is_err());
        assert!(hex_exact::<2>("00ff", "p").is_ok());
        assert_eq!(hex_decode("00FFaB", "p").unwrap(), vec![0, 255, 0xab]);
        assert_eq!(hex_encode(&[0, 255, 0xab]), "00ffab");
        let e = hex_exact::<2>("0", "bones[0].matrix_hex").unwrap_err().to_string();
        assert!(e.contains("bones[0].matrix_hex"), "{e}");
    }
}
