//! `.MANI` as JSON: schema `shaiya-toolkit-mani-v1`. Every meaning is inferred and was not checked in the game.

use super::types::{Obj, F32};
use super::{json_error, JsonError, ModelFormat};
use crate::mani::{Mani, ManiType, ManiValue, MANI_FIELDS};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

fn type_name(t: ManiType) -> &'static str {
    match t {
        ManiType::I32 => "i32",
        ManiType::U16 => "u16",
        ManiType::F32 => "f32",
        ManiType::F32x3 => "f32x3",
    }
}

// ---- export -----------------------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct Inferred {
    fields: &'static str,
}

/// A field value: an integer, a float, or three floats.
#[derive(Serialize)]
#[serde(untagged)]
enum ValueOut {
    Int(i64),
    Float(F32),
    Triple([F32; 3]),
}

#[derive(Serialize)]
struct FieldOut {
    offset: usize,
    name: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    meaning: &'static str,
    value: ValueOut,
}

#[derive(Serialize)]
struct ManiOut<'a> {
    format: &'static str,
    source: &'a str,
    inferred: Inferred,
    fields: Vec<FieldOut>,
}

pub(super) fn export(bytes: &[u8], source: &str, json_cap: u64) -> Result<String, JsonError> {
    let m = Mani::parse(bytes, source).map_err(JsonError::Unreadable)?;
    let fields = m
        .fields()
        .into_iter()
        .map(|(f, v)| FieldOut {
            offset: f.offset,
            name: f.name,
            kind: type_name(f.kind),
            meaning: f.meaning,
            value: match v {
                ManiValue::I32(x) => ValueOut::Int(x as i64),
                ManiValue::U16(x) => ValueOut::Int(x as i64),
                ManiValue::F32(x) => ValueOut::Float(F32(x)),
                ManiValue::F32x3(x) => ValueOut::Triple([F32(x[0]), F32(x[1]), F32(x[2])]),
            },
        })
        .collect();
    let out = ManiOut { format: ModelFormat::Mani.schema(), source, inferred: Inferred { fields: "every meaning is inferred and was not checked in the game" }, fields };
    super::render(&out, json_cap)
}

// ---- import -----------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FieldIn {
    offset: u64,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    #[allow(dead_code)]
    meaning: Option<IgnoredAny>,
    value: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManiIn {
    #[allow(dead_code)]
    format: String,
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<IgnoredAny>,
    #[serde(default)]
    #[allow(dead_code)]
    inferred: Option<IgnoredAny>,
    fields: Vec<Obj<FieldIn>>,
}

fn value_of(f: &FieldIn, i: usize, kind: ManiType) -> Result<ManiValue, JsonError> {
    let at = format!("fields[{i}].value");
    let bad = |why: &str| JsonError::Schema(format!("{at}: {why}"));
    match kind {
        ManiType::I32 => f.value.as_i64().and_then(|v| i32::try_from(v).ok()).map(ManiValue::I32).ok_or_else(|| bad("an i32 is a whole number from -2147483648 to 2147483647")),
        ManiType::U16 => f.value.as_i64().and_then(|v| u16::try_from(v).ok()).map(ManiValue::U16).ok_or_else(|| bad("a u16 is a whole number from 0 to 65535")),
        ManiType::F32 => F32::deserialize(f.value.clone()).map(|x| ManiValue::F32(x.0)).map_err(|e| bad(&e.to_string())),
        ManiType::F32x3 => <[F32; 3]>::deserialize(f.value.clone()).map(|x| ManiValue::F32x3([x[0].0, x[1].0, x[2].0])).map_err(|e| bad(&e.to_string())),
    }
}

pub(super) fn import(json: &[u8], _model_cap: u64) -> Result<Vec<u8>, JsonError> {
    let Obj(input): Obj<ManiIn> = serde_json::from_slice(json).map_err(json_error)?;
    if input.fields.len() != MANI_FIELDS.len() {
        return Err(JsonError::Schema(format!("fields: a .MANI has {} fields, found {}", MANI_FIELDS.len(), input.fields.len())));
    }
    let mut values = Vec::with_capacity(MANI_FIELDS.len());
    for (i, (Obj(f), want)) in input.fields.iter().zip(&MANI_FIELDS).enumerate() {
        if f.offset != want.offset as u64 || f.name != want.name || f.kind != type_name(want.kind) {
            return Err(JsonError::Schema(format!(
                "fields[{i}] must be offset {}, name \"{}\", type \"{}\", found offset {}, name \"{}\", type \"{}\"",
                want.offset,
                want.name,
                type_name(want.kind),
                f.offset,
                f.name,
                f.kind
            )));
        }
        values.push(value_of(f, i, want.kind)?);
    }
    let values: [ManiValue; 20] = values.try_into().map_err(|_| JsonError::Schema("fields: not 20 entries".into()))?;
    let mani = Mani::from_values(&values).map_err(|e| JsonError::NotWritable(e.to_string()))?;
    // the file is exactly 108 bytes; the reader accepts exactly that
    let bytes = mani.to_bytes();
    Mani::parse(&bytes, "mani").map_err(|e| JsonError::NotWritable(format!("the bytes do not read back: {e}")))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::super::{export, import, ModelFormat};
    use super::*;
    use shaiya_testkit::anim::mani_bytes;

    fn round_trip(bytes: &[u8]) -> String {
        let text = export(ModelFormat::Mani, bytes, "t.mani").unwrap();
        let (f, back) = import(text.as_bytes()).unwrap();
        assert_eq!(f, ModelFormat::Mani);
        assert_eq!(back, bytes);
        text
    }

    #[test]
    fn the_mani_test_file_exports_to_the_documented_json() {
        // Recorded from the code at the first build, then checked line by line against the schema table and the layout rules.
        let expected = r#"{
  "format": "shaiya-toolkit-mani-v1",
  "source": "wall.MANI",
  "inferred": {
    "fields": "every meaning is inferred and was not checked in the game"
  },
  "fields": [
    {
      "offset": 0,
      "name": "tick_ms",
      "type": "i32",
      "meaning": "the motion steps once per this many milliseconds",
      "value": 33
    },
    {
      "offset": 4,
      "name": "swing_on",
      "type": "i32",
      "meaning": "rocking about swing_axis (0 or 1)",
      "value": 0
    },
    {
      "offset": 8,
      "name": "swing_axis",
      "type": "f32x3",
      "meaning": "axis of the rocking",
      "value": [1.0, 0.0, 0.0]
    },
    {
      "offset": 20,
      "name": "swing_limit",
      "type": "f32",
      "meaning": "radians; the direction flips at this angle",
      "value": 0.0
    },
    {
      "offset": 24,
      "name": "swing_step",
      "type": "f32",
      "meaning": "radians per step",
      "value": 0.0
    },
    {
      "offset": 28,
      "name": "swing_unused",
      "type": "f32",
      "meaning": "no use known",
      "value": 1.0
    },
    {
      "offset": 32,
      "name": "swing_pause_ms",
      "type": "i32",
      "meaning": "wait at each end of the rocking, in milliseconds",
      "value": 80
    },
    {
      "offset": 36,
      "name": "orbit_on",
      "type": "i32",
      "meaning": "circular offset about orbit_axis (0 or 1)",
      "value": 0
    },
    {
      "offset": 40,
      "name": "orbit_axis",
      "type": "f32x3",
      "meaning": "axis of the circular offset",
      "value": [1.0, 0.0, 0.0]
    },
    {
      "offset": 52,
      "name": "orbit_step",
      "type": "f32",
      "meaning": "radians per step",
      "value": 0.0
    },
    {
      "offset": 56,
      "name": "orbit_radius",
      "type": "f32",
      "meaning": "world units",
      "value": 0.0
    },
    {
      "offset": 60,
      "name": "spin_on",
      "type": "i32",
      "meaning": "continuous rotation about spin_axis (0 or 1)",
      "value": 0
    },
    {
      "offset": 64,
      "name": "spin_axis",
      "type": "f32x3",
      "meaning": "axis of the rotation",
      "value": [0.0, 0.0, 1.0]
    },
    {
      "offset": 76,
      "name": "spin_step",
      "type": "f32",
      "meaning": "radians per step",
      "value": -0.01745
    },
    {
      "offset": 80,
      "name": "move_on",
      "type": "u16",
      "meaning": "back-and-forth movement along move_dir (0 or 1)",
      "value": 1
    },
    {
      "offset": 82,
      "name": "move_pause_ms",
      "type": "u16",
      "meaning": "wait at each end of the movement, in milliseconds",
      "value": 500
    },
    {
      "offset": 84,
      "name": "move_dir",
      "type": "f32x3",
      "meaning": "direction of the movement",
      "value": [1.0, 0.0, 0.0]
    },
    {
      "offset": 96,
      "name": "move_speed",
      "type": "f32",
      "meaning": "start distance per step",
      "value": 0.0
    },
    {
      "offset": 100,
      "name": "move_accel",
      "type": "f32",
      "meaning": "speed factor per step",
      "value": 1.01
    },
    {
      "offset": 104,
      "name": "move_distance",
      "type": "f32",
      "meaning": "end point = move_dir times this",
      "value": 10.0
    }
  ]
}
"#;
        assert_eq!(export(ModelFormat::Mani, &mani_bytes(33, 1, 500, 10.0f32.to_bits()), "wall.MANI").unwrap(), expected);
    }

    #[test]
    fn every_mani_test_file_round_trips() {
        let text = round_trip(&mani_bytes(33, 1, 500, 10.0f32.to_bits()));
        assert!(text.contains("\"value\": 33") && text.contains("\"value\": 10.0"), "{text}");
        // the integer generation: the denormal at offset 104 is a plain number that comes back as the same bits
        let text = round_trip(&mani_bytes(33, 0, 0, 500));
        assert!(text.contains("\"name\": \"move_distance\"") && text.contains("e-43"), "{text}");
        // NaN bits in an axis
        let mut d = mani_bytes(33, 1, 500, 10.0f32.to_bits());
        d[8..12].copy_from_slice(&0xFFC0_0000u32.to_le_bytes());
        d[20..24].copy_from_slice(&0x7FA0_0001u32.to_le_bytes());
        let text = round_trip(&d);
        assert!(text.contains("\"bits:ffc00000\"") && text.contains("\"bits:7fa00001\""), "{text}");
        // an all-zero file and an all-ones file
        round_trip(&[0u8; 108]);
        round_trip(&[0xFFu8; 108]);
    }

    #[test]
    fn an_edited_value_comes_through() {
        let original = mani_bytes(33, 1, 500, 10.0f32.to_bits());
        let text = export(ModelFormat::Mani, &original, "t.mani").unwrap();
        let edited = text.replacen("\"value\": 33", "\"value\": 40", 1);
        assert_ne!(edited, text);
        let (_, bytes) = import(edited.as_bytes()).unwrap();
        let diff: Vec<usize> = (0..108).filter(|&i| bytes[i] != original[i]).collect();
        assert_eq!(diff, vec![0], "{diff:?}");
        assert_eq!(bytes[0], 40);
    }

    #[test]
    fn a_mani_field_out_of_order_or_of_another_type_is_refused() {
        let text = export(ModelFormat::Mani, &mani_bytes(33, 1, 500, 10.0f32.to_bits()), "t.mani").unwrap();
        let swapped = text.replacen("\"name\": \"tick_ms\"", "\"name\": \"swing_on\"", 1);
        assert!(matches!(import(swapped.as_bytes()), Err(JsonError::Schema(_))));
        let offset = text.replacen("\"offset\": 4,", "\"offset\": 5,", 1);
        assert!(matches!(import(offset.as_bytes()), Err(JsonError::Schema(_))));
        let kind = text.replacen("\"type\": \"i32\"", "\"type\": \"f32\"", 1);
        assert!(matches!(import(kind.as_bytes()), Err(JsonError::Schema(_))));
        // a value of another type: a fraction for an i32, a negative for a u16, one number for three
        let frac = text.replacen("\"value\": 33", "\"value\": 33.5", 1);
        let e = import(frac.as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("fields[0].value"), "{e}");
        let neg = text.replacen("\"value\": 500", "\"value\": -1", 1);
        assert!(matches!(import(neg.as_bytes()), Err(JsonError::Schema(_))));
        let big = text.replacen("\"value\": 33", "\"value\": 2147483648", 1);
        assert!(matches!(import(big.as_bytes()), Err(JsonError::Schema(_))));
        let one = text.replacen("\"value\": [1.0, 0.0, 0.0]", "\"value\": 1.0", 1);
        assert_ne!(one, text);
        assert!(matches!(import(one.as_bytes()), Err(JsonError::Schema(_))));
        // a float written as the bits string where an integer is expected
        let s = text.replacen("\"value\": 33", "\"value\": \"bits:00000021\"", 1);
        assert!(matches!(import(s.as_bytes()), Err(JsonError::Schema(_))));
    }

    #[test]
    fn a_mani_needs_all_twenty_fields_and_no_unknown_keys() {
        let text = export(ModelFormat::Mani, &mani_bytes(33, 1, 500, 10.0f32.to_bits()), "t.mani").unwrap();
        let start = text.find("    {\n      \"offset\": 100").unwrap();
        let end = text.find("    {\n      \"offset\": 104").unwrap();
        let fewer = format!("{}{}", &text[..start], &text[end..]);
        let e = import(fewer.as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("20 fields"), "{e}");
        let extra = text.replacen("\"offset\": 0,", "\"offset\": 0,\n      \"extra\": 1,", 1);
        assert!(matches!(import(extra.as_bytes()), Err(JsonError::Json { .. })));
        // a field written as an array is not an object; the meaning label is ignored
        let e = import(br#"{"format": "shaiya-toolkit-mani-v1", "fields": [[0, "tick_ms", "i32", "", 1]]}"#);
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
        let relabelled = text.replacen("\"meaning\": \"the motion steps once per this many milliseconds\"", "\"meaning\": \"my words\"", 1);
        assert_ne!(relabelled, text);
        assert_eq!(import(relabelled.as_bytes()).unwrap().1, mani_bytes(33, 1, 500, 10.0f32.to_bits()));
        // a MANI JSON has no `columns`; one that is present is an unknown key
        let with = text.replacen("\"fields\": [", "\"columns\": {},\n  \"fields\": [", 1);
        assert!(matches!(import(with.as_bytes()), Err(JsonError::Json { .. })));
    }
}
