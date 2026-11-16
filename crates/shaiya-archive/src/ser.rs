//! Serialisers for the `serde` feature: raw name bytes as decoded text, byte strings as lower-case hex.

use serde::Serializer;

pub fn name<S: Serializer>(raw: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&shaiya_core::text::display_lossless(raw, shaiya_core::text::Encoding::Cp949))
}

pub fn hex<S: Serializer>(raw: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&shaiya_core::hash::to_hex(raw))
}
