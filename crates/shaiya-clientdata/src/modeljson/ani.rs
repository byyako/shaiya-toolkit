//! `.ANI` as JSON: schema `shaiya-toolkit-ani-v1`.

use super::types::{hex_encode, hex_exact, KeyF32, Obj, F32};
use super::{json_error, JsonError, ModelFormat};
use crate::anim::{track_fits, Ani, AniBone, RotKey, TransKey, KEY_LIMIT};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

const ROTATION_COLUMNS: [&str; 5] = ["key", "x", "y", "z", "w"];
const TRANSLATION_COLUMNS: [&str; 4] = ["key", "x", "y", "z"];

// ---- export -----------------------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct Inferred {
    first_field: &'static str,
}

#[derive(Serialize)]
struct Columns {
    rotation_keys: &'static [&'static str],
    translation_keys: &'static [&'static str],
}

#[derive(Serialize)]
struct BoneOut {
    index: usize,
    parent: i32,
    matrix_hex: String,
    rotation_keys: Vec<(i32, F32, F32, F32, F32)>,
    translation_keys: Vec<(i32, F32, F32, F32)>,
}

#[derive(Serialize)]
struct AniOut<'a> {
    format: &'static str,
    source: &'a str,
    inferred: Inferred,
    columns: Columns,
    first_field: i32,
    end_key: i32,
    bones: Vec<BoneOut>,
}

fn matrix_bytes(m: &[f32; 16]) -> Vec<u8> {
    m.iter().flat_map(|f| f.to_bits().to_le_bytes()).collect()
}

pub(super) fn export(bytes: &[u8], source: &str, json_cap: u64) -> Result<String, JsonError> {
    let ani = Ani::parse(bytes, source).map_err(JsonError::Unreadable)?;
    let mut bones = Vec::with_capacity(ani.bones.len());
    for (i, b) in ani.bones.iter().enumerate() {
        for (j, k) in b.rot.iter().enumerate() {
            if k.q.iter().any(|c| !c.is_finite()) {
                return Err(JsonError::NotFinite { at: format!("bones[{i}].rotation_keys[{j}]") });
            }
        }
        for (j, k) in b.trans.iter().enumerate() {
            if k.v.iter().any(|c| !c.is_finite()) {
                return Err(JsonError::NotFinite { at: format!("bones[{i}].translation_keys[{j}]") });
            }
        }
        bones.push(BoneOut {
            index: i,
            parent: b.parent,
            matrix_hex: hex_encode(&matrix_bytes(&b.matrix)),
            rotation_keys: b.rot.iter().map(|k| (k.key, F32(k.q[0]), F32(k.q[1]), F32(k.q[2]), F32(k.q[3]))).collect(),
            translation_keys: b.trans.iter().map(|k| (k.key, F32(k.v[0]), F32(k.v[1]), F32(k.v[2]))).collect(),
        });
    }
    let out = AniOut {
        format: ModelFormat::Ani.schema(),
        source,
        inferred: Inferred { first_field: "start key" },
        columns: Columns { rotation_keys: &ROTATION_COLUMNS, translation_keys: &TRANSLATION_COLUMNS },
        first_field: ani.first_field,
        end_key: ani.end_key,
        bones,
    };
    super::render(&out, json_cap)
}

// ---- import -----------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnsIn {
    rotation_keys: Vec<String>,
    translation_keys: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BoneIn {
    index: u64,
    parent: i32,
    matrix_hex: String,
    rotation_keys: Vec<(i32, KeyF32, KeyF32, KeyF32, KeyF32)>,
    translation_keys: Vec<(i32, KeyF32, KeyF32, KeyF32)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AniIn {
    #[allow(dead_code)]
    format: String,
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<IgnoredAny>,
    #[serde(default)]
    #[allow(dead_code)]
    inferred: Option<IgnoredAny>,
    #[serde(default)]
    columns: Option<Obj<ColumnsIn>>,
    first_field: i32,
    end_key: i32,
    bones: Vec<Obj<BoneIn>>,
}

fn same(found: &[String], want: &[&str]) -> bool {
    found.len() == want.len() && found.iter().zip(want).all(|(a, b)| a == b)
}

pub(super) fn import(json: &[u8], model_cap: u64) -> Result<Vec<u8>, JsonError> {
    let Obj(input): Obj<AniIn> = serde_json::from_slice(json).map_err(json_error)?;
    if let Some(Obj(c)) = &input.columns {
        if !same(&c.rotation_keys, &ROTATION_COLUMNS) {
            return Err(JsonError::Schema(format!("columns.rotation_keys must be {ROTATION_COLUMNS:?}, found {:?}", c.rotation_keys)));
        }
        if !same(&c.translation_keys, &TRANSLATION_COLUMNS) {
            return Err(JsonError::Schema(format!("columns.translation_keys must be {TRANSLATION_COLUMNS:?}, found {:?}", c.translation_keys)));
        }
    }
    if input.bones.len() > u16::MAX as usize {
        return Err(JsonError::TooLarge(format!("{} bones do not fit the 16-bit bone count (at most {})", input.bones.len(), u16::MAX)));
    }
    let mut size: u64 = 10;
    let too_big = || JsonError::TooLarge("the animation is larger than the size limit".into());
    for (i, Obj(b)) in input.bones.iter().enumerate() {
        if b.index != i as u64 {
            return Err(JsonError::Schema(format!("bone {i} has index {}: bones are listed in order 0, 1, 2, ...", b.index)));
        }
        if !track_fits(b.rotation_keys.len()) || !track_fits(b.translation_keys.len()) {
            return Err(JsonError::TooLarge(format!("bones[{i}]: a track is longer than {KEY_LIMIT} keys")));
        }
        let bone = 76u64
            .checked_add(20 * b.rotation_keys.len() as u64)
            .and_then(|n| n.checked_add(16 * b.translation_keys.len() as u64))
            .ok_or_else(too_big)?;
        size = size.checked_add(bone).ok_or_else(too_big)?;
    }
    super::check_model_len(size, model_cap)?;

    let mut bones = Vec::with_capacity(input.bones.len());
    for (i, Obj(b)) in input.bones.iter().enumerate() {
        let raw: [u8; 64] = hex_exact(&b.matrix_hex, &format!("bones[{i}].matrix_hex"))?;
        let mut matrix = [0f32; 16];
        for (i4, m) in matrix.iter_mut().enumerate() {
            *m = f32::from_bits(u32::from_le_bytes([raw[4 * i4], raw[4 * i4 + 1], raw[4 * i4 + 2], raw[4 * i4 + 3]]));
        }
        let mut rot = Vec::with_capacity(b.rotation_keys.len());
        for (j, k) in b.rotation_keys.iter().enumerate() {
            let q = [k.1 .0, k.2 .0, k.3 .0, k.4 .0];
            if q.iter().any(|c| !c.is_finite()) {
                return Err(JsonError::NotFinite { at: format!("bones[{i}].rotation_keys[{j}]") });
            }
            rot.push(RotKey { key: k.0, q });
        }
        let mut trans = Vec::with_capacity(b.translation_keys.len());
        for (j, k) in b.translation_keys.iter().enumerate() {
            let v = [k.1 .0, k.2 .0, k.3 .0];
            if v.iter().any(|c| !c.is_finite()) {
                return Err(JsonError::NotFinite { at: format!("bones[{i}].translation_keys[{j}]") });
            }
            trans.push(TransKey { key: k.0, v });
        }
        bones.push(AniBone { parent: b.parent, matrix, rot, trans });
    }
    let ani = Ani { first_field: input.first_field, end_key: input.end_key, bones };
    ani.try_to_bytes().map_err(|e| JsonError::NotWritable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::{export, import, ModelFormat};
    use super::*;
    use shaiya_testkit::anim::{ani_bytes, arm_ani, identity_bits, BoneSpec};

    fn arm_json() -> String {
        export(ModelFormat::Ani, &arm_ani(60), "arm.ANI").unwrap()
    }

    fn round_trip(bytes: &[u8]) {
        let text = export(ModelFormat::Ani, bytes, "t.ani").unwrap();
        let (f, back) = import(text.as_bytes()).unwrap();
        assert_eq!(f, ModelFormat::Ani);
        assert_eq!(back, bytes);
    }

    fn spec(parent: i32, matrix: [u32; 16], rot: Vec<(i32, [f32; 4])>, trans: Vec<(i32, [f32; 3])>) -> BoneSpec {
        BoneSpec { parent, matrix, rot, trans }
    }

    #[test]
    fn the_arm_exports_to_the_documented_json() {
        // Recorded from the code at the first build, then checked line by line against the schema table and the layout rules.
        let expected = r#"{
  "format": "shaiya-toolkit-ani-v1",
  "source": "arm.ANI",
  "inferred": {
    "first_field": "start key"
  },
  "columns": {
    "rotation_keys": [
      "key",
      "x",
      "y",
      "z",
      "w"
    ],
    "translation_keys": [
      "key",
      "x",
      "y",
      "z"
    ]
  },
  "first_field": 0,
  "end_key": 60,
  "bones": [
    {
      "index": 0,
      "parent": -1,
      "matrix_hex": "0000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f",
      "rotation_keys": [],
      "translation_keys": []
    },
    {
      "index": 1,
      "parent": 0,
      "matrix_hex": "0000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f00000000000000000000803f000000000000803f",
      "rotation_keys": [
        [0, 0.0, 0.0, 0.0, 1.0],
        [60, 0.0, 0.0, 0.70710677, 0.70710677]
      ],
      "translation_keys": []
    },
    {
      "index": 2,
      "parent": 1,
      "matrix_hex": "0000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f000000000000000000000040000000000000803f",
      "rotation_keys": [],
      "translation_keys": []
    }
  ]
}
"#;
        assert_eq!(arm_json(), expected);
    }

    #[test]
    fn every_ani_test_file_round_trips() {
        round_trip(&arm_ani(60));
        // reference matrices that hold NaN keep their payload bits
        let mut m = identity_bits();
        m[3] = 0xFFC0_0000;
        m[7] = 0x7FA0_0001;
        round_trip(&ani_bytes(0, 0, &[spec(-1, m, vec![], vec![])]));
        // a first field of -1 and negative keys
        round_trip(&ani_bytes(-1, 30, &[spec(-1, identity_bits(), vec![(-1, [0.0, 0.0, 0.0, 1.0]), (30, [0.0, 0.0, 1.0, 0.0])], vec![(-1, [0.1, -0.0, 1e-30])])]));
        // no bones at all
        round_trip(&ani_bytes(0, 0, &[]));
        // denormal and extreme key values
        round_trip(&ani_bytes(0, 5, &[spec(-1, identity_bits(), vec![(0, [f32::from_bits(1), f32::MAX, f32::MIN_POSITIVE, -f32::MAX])], vec![(5, [f32::from_bits(500), 7e-43, 1.0])])]));
    }

    #[test]
    fn an_edited_key_value_comes_through() {
        let original = arm_ani(60);
        let text = arm_json();
        let edited = text.replacen("0.70710677", "0.5", 1);
        assert_ne!(edited, text);
        let (_, bytes) = import(edited.as_bytes()).unwrap();
        assert_eq!(bytes.len(), original.len());
        let diff: Vec<usize> = (0..bytes.len()).filter(|&i| bytes[i] != original[i]).collect();
        assert!(!diff.is_empty());
        let a = Ani::parse(&bytes, "x").unwrap();
        let o = Ani::parse(&original, "x").unwrap();
        let changed: Vec<_> = a.bones[1].rot.iter().zip(&o.bones[1].rot).flat_map(|(x, y)| x.q.iter().zip(&y.q).filter(|(p, q)| p.to_bits() != q.to_bits()).map(|(p, _)| *p)).collect();
        assert_eq!(changed, vec![0.5]);
        // every byte outside that one float is identical
        assert!(diff.len() <= 4, "{diff:?}");
    }

    #[test]
    fn the_bits_fallback_is_v1_compatible() {
        let original = arm_ani(60);
        let mut text = arm_json();
        // write every float of the key tracks in its `bits:` form
        let ani = Ani::parse(&original, "x").unwrap();
        for b in &ani.bones {
            for k in &b.rot {
                for c in k.q {
                    let number = serde_json::to_string(&c).unwrap();
                    let bits = format!("\"bits:{:08x}\"", c.to_bits());
                    text = text.replace(&format!(", {number}"), &format!(", {bits}"));
                    text = text.replace(&format!("[{number},"), &format!("[{bits},"));
                }
            }
        }
        assert!(text.contains("bits:"));
        assert_eq!(import(text.as_bytes()).unwrap().1, original);
    }

    #[test]
    fn a_number_too_large_for_f32_is_refused_by_the_import() {
        let text = arm_json().replacen("0.70710677", "1e39", 1);
        match import(text.as_bytes()).unwrap_err() {
            JsonError::NotFinite { at } => assert_eq!(at, "bones[1].rotation_keys[1]"),
            e => panic!("{e}"),
        }
        let text = arm_json().replacen("[0, 0.0, 0.0, 0.0, 1.0]", "[0, 0.0, 0.0, -1e39, 1.0]", 1);
        match import(text.as_bytes()).unwrap_err() {
            JsonError::NotFinite { at } => assert_eq!(at, "bones[1].rotation_keys[0]"),
            e => panic!("{e}"),
        }
    }

    #[test]
    fn a_positional_array_is_refused_for_the_file_and_for_a_bone() {
        // the whole file as an array of its values
        let e = import(br#"["shaiya-toolkit-ani-v1", null, null, null, 0, 0, []]"#).unwrap_err();
        assert!(matches!(e, JsonError::Json { .. }), "{e}");
        let e = super::super::format_of(br#"["shaiya-toolkit-ani-v1"]"#).unwrap_err();
        assert!(matches!(e, JsonError::Json { .. }), "{e}");
        // one bone as an array
        let matrix = "0000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f";
        let object = format!("{{\n      \"index\": 0,\n      \"parent\": -1,\n      \"matrix_hex\": \"{matrix}\",\n      \"rotation_keys\": [],\n      \"translation_keys\": []\n    }}");
        let text = arm_json();
        assert!(text.contains(&object), "the golden text of bone 0 changed");
        let positional = text.replacen(&object, &format!("[0, -1, \"{matrix}\", [], []]"), 1);
        let e = import(positional.as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Json { .. }), "{e}");
        // the object form still reads
        assert!(import(text.as_bytes()).is_ok());
    }

    fn bones_json(count: usize) -> String {
        let bone = r#"{"index":0,"parent":-1,"matrix_hex":"","rotation_keys":[],"translation_keys":[]}"#;
        let list = vec![bone; count].join(",");
        format!(r#"{{"format":"shaiya-toolkit-ani-v1","first_field":0,"end_key":0,"bones":[{list}]}}"#)
    }

    #[test]
    fn more_than_65535_bones_are_refused_before_anything_is_built() {
        let e = import(bones_json(65_536).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::TooLarge(_)) && e.to_string().contains("65536 bones"), "{e}");
        // at the limit the count is accepted and the next check (the bone index) speaks instead
        let e = import(bones_json(65_535).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("bone 1 has index 0"), "{e}");
    }

    #[test]
    fn a_non_finite_key_is_refused_on_export_and_import() {
        let bad = ani_bytes(0, 0, &[spec(-1, identity_bits(), vec![(0, [f32::NAN, 0.0, 0.0, 1.0])], vec![])]);
        match export(ModelFormat::Ani, &bad, "t.ani").unwrap_err() {
            JsonError::NotFinite { at } => assert_eq!(at, "bones[0].rotation_keys[0]"),
            e => panic!("{e}"),
        }
        let bad_trans = ani_bytes(0, 0, &[spec(-1, identity_bits(), vec![], vec![(0, [0.0, f32::INFINITY, 0.0])])]);
        assert!(matches!(export(ModelFormat::Ani, &bad_trans, "t.ani"), Err(JsonError::NotFinite { .. })));
        let text = arm_json().replacen("0.70710677", "\"bits:7fc00000\"", 1);
        match import(text.as_bytes()).unwrap_err() {
            JsonError::NotFinite { at } => assert_eq!(at, "bones[1].rotation_keys[1]"),
            e => panic!("{e}"),
        }
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let text = arm_json().replacen("\"end_key\"", "\"endkey\": 1,\n  \"end_key\"", 1);
        assert!(matches!(import(text.as_bytes()), Err(JsonError::Json { .. })));
        let inside = arm_json().replacen("\"parent\": -1,", "\"parent\": -1,\n      \"extra\": 1,", 1);
        assert_ne!(inside, arm_json());
        let e = import(inside.as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Json { .. }) && e.to_string().contains("extra"), "{e}");
        // the labels are accepted and ignored
        let labelled = arm_json().replacen("\"source\": \"arm.ANI\"", "\"source\": \"elsewhere\"", 1);
        assert_eq!(import(labelled.as_bytes()).unwrap().1, arm_ani(60));
    }

    #[test]
    fn wrong_columns_are_refused() {
        let text = arm_json();
        let swapped = text.replacen("\"key\",\n      \"x\",\n      \"y\",\n      \"z\",\n      \"w\"", "\"key\",\n      \"y\",\n      \"x\",\n      \"z\",\n      \"w\"", 1);
        assert_ne!(swapped, text, "the column text of the golden changed");
        assert!(matches!(import(swapped.as_bytes()), Err(JsonError::Schema(_))));
        // absent passes
        let start = text.find("  \"columns\"").unwrap();
        let end = text.find("
  \"first_field\": 0").unwrap() + 1;
        let without = format!("{}{}", &text[..start], &text[end..]);
        assert_eq!(import(without.as_bytes()).unwrap().1, arm_ani(60));
        // the positional form is refused: columns is an object
        let positional = format!("{}  \"columns\": [[\"key\", \"x\", \"y\", \"z\", \"w\"], [\"key\", \"x\", \"y\", \"z\"]],
{}", &text[..start], &text[end..]);
        let e = import(positional.as_bytes());
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
    }

    #[test]
    fn a_bone_index_out_of_order_is_refused() {
        let text = arm_json().replacen("\"index\": 1,", "\"index\": 2,", 1);
        let e = import(text.as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("bone 1 has index 2"), "{e}");
    }

    #[test]
    fn bad_hex_is_refused_in_a_matrix() {
        let good = arm_json();
        let first = good.find("\"matrix_hex\": \"").unwrap() + "\"matrix_hex\": \"".len();
        let digits = &good[first..first + 128];
        for bad in [digits[..127].to_string(), format!("{}zz", &digits[..126]), format!("{digits}0"), String::new()] {
            let text = good.replacen(digits, &bad, 1);
            assert!(matches!(import(text.as_bytes()), Err(JsonError::Schema(_))), "{}", bad.len());
        }
        // upper case is accepted
        let upper = good.replacen(digits, &digits.to_ascii_uppercase(), 1);
        assert_eq!(import(upper.as_bytes()).unwrap().1, arm_ani(60));
    }

    #[test]
    fn a_wrong_key_record_length_is_refused() {
        let text = arm_json().replacen("[0, 0.0, 0.0, 0.0, 1.0]", "[0, 0.0, 0.0, 1.0]", 1);
        assert!(matches!(import(text.as_bytes()), Err(JsonError::Json { .. })));
    }

    #[test]
    fn try_to_bytes_reads_its_bytes_back() {
        let a = Ani::parse(&arm_ani(60), "arm").unwrap();
        let bytes = a.try_to_bytes().unwrap();
        assert_eq!(Ani::parse(&bytes, "arm").unwrap(), a);
    }
}
