//! `.3DE` as JSON: schema `shaiya-toolkit-3de-v1`. A file the reader refuses (neither layout reads it) is not exported.

use super::types::{check_columns, check_limit, hex_encode, required, texture_from_hex, Face, Obj, Size, F32};
use super::{json_error, JsonError, ModelFormat};
use crate::mesh3de::{Frame3de, FrameBlock3de, FrameVertex3de, Mesh3de, Vertex3de, COUNT_LIMIT, FRAME_LIMIT, TEXTURE_LIMIT};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

const VERTEX_COLUMNS: [&str; 6] = ["x", "y", "z", "field", "u", "v"];
const FRAME_COLUMNS: [&str; 5] = ["dx", "dy", "dz", "u", "v"];

/// `[x, y, z, field, u, v]`
type VertexRec = (F32, F32, F32, i32, F32, F32);
/// `[dx, dy, dz, u, v]`
type FrameRec = (F32, F32, F32, F32, F32);

// ---- export -----------------------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct Inferred {
    max_key: &'static str,
    frame_vertices: &'static str,
}

#[derive(Serialize)]
struct Columns {
    vertices: &'static [&'static str],
    frame_vertices: &'static [&'static str],
}

#[derive(Serialize)]
struct FrameOut {
    key: i32,
    vertices: Vec<FrameRec>,
}

#[derive(Serialize)]
struct BlockOut {
    max_key: i32,
    frames: Vec<FrameOut>,
}

#[derive(Serialize)]
struct Out<'a> {
    format: &'static str,
    source: &'a str,
    inferred: Inferred,
    columns: Columns,
    layout: &'static str,
    texture_hex: String,
    vertices: Vec<VertexRec>,
    faces: Vec<Face>,
    frame_block: Option<BlockOut>,
}

pub(super) fn export(bytes: &[u8], source: &str, json_cap: u64) -> Result<String, JsonError> {
    let m = Mesh3de::parse(bytes, source).map_err(JsonError::Unreadable)?;
    let out = Out {
        format: ModelFormat::Mesh3de.schema(),
        source,
        inferred: Inferred { max_key: "inferred", frame_vertices: "dx, dy, dz: inferred to be a position change" },
        columns: Columns { vertices: &VERTEX_COLUMNS, frame_vertices: &FRAME_COLUMNS },
        layout: if m.block.is_some() { "standard" } else { "static" },
        texture_hex: hex_encode(&m.texture),
        vertices: m.vertices.iter().map(|v| (F32(v.pos[0]), F32(v.pos[1]), F32(v.pos[2]), v.flag, F32(v.uv[0]), F32(v.uv[1]))).collect(),
        faces: m.faces.clone(),
        frame_block: m.block.as_ref().map(|b| BlockOut {
            max_key: b.max_key,
            frames: b
                .frames
                .iter()
                .map(|f| FrameOut { key: f.key, vertices: f.vertices.iter().map(|v| (F32(v.delta[0]), F32(v.delta[1]), F32(v.delta[2]), F32(v.uv[0]), F32(v.uv[1]))).collect() })
                .collect(),
        }),
    };
    super::render(&out, json_cap)
}

// ---- import -----------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnsIn {
    vertices: Vec<String>,
    frame_vertices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameIn {
    key: i32,
    vertices: Vec<FrameRec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockIn {
    max_key: i32,
    frames: Vec<Obj<FrameIn>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct In {
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
    layout: String,
    texture_hex: String,
    vertices: Vec<VertexRec>,
    faces: Vec<Face>,
    #[serde(deserialize_with = "required")]
    frame_block: Option<Obj<BlockIn>>,
}

pub(super) fn import(json: &[u8], model_cap: u64) -> Result<Vec<u8>, JsonError> {
    let Obj(input): Obj<In> = serde_json::from_slice(json).map_err(json_error)?;
    if let Some(Obj(c)) = &input.columns {
        check_columns(&c.vertices, &VERTEX_COLUMNS, "columns.vertices")?;
        check_columns(&c.frame_vertices, &FRAME_COLUMNS, "columns.frame_vertices")?;
    }
    match (input.layout.as_str(), &input.frame_block) {
        ("standard", Some(_)) | ("static", None) => {}
        ("standard", None) => return Err(JsonError::Schema("layout \"standard\" needs a frame_block (it is null)".into())),
        ("static", Some(_)) => return Err(JsonError::Schema("layout \"static\" has no frames: frame_block must be null".into())),
        (other, _) => return Err(JsonError::Schema(format!("layout \"{other}\" is not one of \"standard\", \"static\""))),
    }
    check_limit(input.texture_hex.len() / 2, TEXTURE_LIMIT, "texture_hex")?;
    check_limit(input.vertices.len(), COUNT_LIMIT, "vertices")?;
    check_limit(input.faces.len(), COUNT_LIMIT, "faces")?;
    let mut size = Size::new(4 + (input.texture_hex.len() / 2) as u64 + 4);
    size.add_mul(input.vertices.len(), 24);
    size.add(4);
    size.add_mul(input.faces.len(), 6);
    if let Some(Obj(b)) = &input.frame_block {
        check_limit(b.frames.len(), FRAME_LIMIT, "frame_block.frames")?;
        for (i, Obj(f)) in b.frames.iter().enumerate() {
            if f.vertices.len() != input.vertices.len() {
                return Err(JsonError::Schema(format!("frame_block.frames[{i}].vertices: {} entries for {} vertices (a frame holds one entry per vertex)", f.vertices.len(), input.vertices.len())));
            }
        }
        size.add(8);
        size.add_mul(b.frames.len(), 4);
        size.add_mul(b.frames.iter().map(|f| f.0.vertices.len()).sum(), 20);
    }
    size.finish(model_cap)?;

    let block = input.frame_block.as_ref().map(|Obj(b)| FrameBlock3de {
        max_key: b.max_key,
        frames: b
            .frames
            .iter()
            .map(|Obj(f)| Frame3de { key: f.key, vertices: f.vertices.iter().map(|v| FrameVertex3de { delta: [v.0 .0, v.1 .0, v.2 .0], uv: [v.3 .0, v.4 .0] }).collect() })
            .collect(),
    });
    let mesh = Mesh3de {
        texture: texture_from_hex(&input.texture_hex, TEXTURE_LIMIT, "texture_hex")?,
        vertices: input.vertices.iter().map(|v| Vertex3de { pos: [v.0 .0, v.1 .0, v.2 .0], flag: v.3, uv: [v.4 .0, v.5 .0] }).collect(),
        faces: input.faces.clone(),
        block,
    };
    mesh.try_to_bytes().map_err(|e| JsonError::NotWritable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::{export, import, ModelFormat};
    use super::*;
    use crate::mesh3de::NOT_THE_LAYOUT;
    use shaiya_testkit::anim::{box_3de, mesh3de_bytes, Block3deSpec, Mesh3deSpec, Vert3deSpec};

    fn tiny(block: bool) -> Vec<u8> {
        let v = |x: f32| Vert3deSpec { pos: [x, 0.5, 0.25], flag: -1, uv: [0.5, 0.125] };
        mesh3de_bytes(&Mesh3deSpec {
            texture: b"t.tga\0".to_vec(),
            vertices: vec![v(0.0), v(1.0)],
            faces: vec![[0, 1, 1]],
            block: block.then(|| Block3deSpec { max_key: 7, frames: vec![(0, vec![([0.0, 0.5, 0.0], [0.25, 0.75]); 2]), (7, vec![([0.0, 1.5, 0.0], [0.25, 0.75]); 2])] }),
        })
    }

    fn round_trip(bytes: &[u8]) -> String {
        let text = export(ModelFormat::Mesh3de, bytes, "t.3de").unwrap();
        let (f, back) = import(text.as_bytes()).unwrap();
        assert_eq!(f, ModelFormat::Mesh3de);
        assert_eq!(back, bytes);
        text
    }

    #[test]
    fn the_3de_test_file_exports_to_the_documented_json() {
        // Recorded from the code at the first build, then checked line by line against the schema table and the layout rules.
        let expected = r#"{
  "format": "shaiya-toolkit-3de-v1",
  "source": "tiny.3DE",
  "inferred": {
    "max_key": "inferred",
    "frame_vertices": "dx, dy, dz: inferred to be a position change"
  },
  "columns": {
    "vertices": [
      "x",
      "y",
      "z",
      "field",
      "u",
      "v"
    ],
    "frame_vertices": [
      "dx",
      "dy",
      "dz",
      "u",
      "v"
    ]
  },
  "layout": "standard",
  "texture_hex": "742e74676100",
  "vertices": [
    [0.0, 0.5, 0.25, -1, 0.5, 0.125],
    [1.0, 0.5, 0.25, -1, 0.5, 0.125]
  ],
  "faces": [
    [0, 1, 1]
  ],
  "frame_block": {
    "max_key": 7,
    "frames": [
      {
        "key": 0,
        "vertices": [
          [0.0, 0.5, 0.0, 0.25, 0.75],
          [0.0, 0.5, 0.0, 0.25, 0.75]
        ]
      },
      {
        "key": 7,
        "vertices": [
          [0.0, 1.5, 0.0, 0.25, 0.75],
          [0.0, 1.5, 0.0, 0.25, 0.75]
        ]
      }
    ]
  }
}
"#;
        assert_eq!(export(ModelFormat::Mesh3de, &tiny(true), "tiny.3DE").unwrap(), expected);
    }

    #[test]
    fn every_3de_test_file_round_trips() {
        round_trip(&tiny(true));
        let text = round_trip(&tiny(false));
        assert!(text.contains("\"layout\": \"static\"") && text.contains("\"frame_block\": null"), "{text}");
        for frames in [Some(4), Some(0), None] {
            round_trip(&box_3de(frames));
        }
        // a vertex field other than -1, bytes after the NUL, NaN in a frame entry
        let spec = Mesh3deSpec {
            texture: b"x\0junk".to_vec(),
            vertices: vec![Vert3deSpec { pos: [0.0; 3], flag: 12345, uv: [0.0; 2] }],
            faces: vec![],
            block: Some(Block3deSpec { max_key: -2, frames: vec![(3, vec![([f32::NAN, 0.0, 0.0], [0.0, f32::NEG_INFINITY])])] }),
        };
        let text = round_trip(&mesh3de_bytes(&spec));
        assert!(text.contains("12345") && text.contains("\"bits:7fc00000\"") && text.contains("\"bits:ff800000\"") && text.contains("\"78006a756e6b\""), "{text}");
    }

    #[test]
    fn an_edited_vertex_coordinate_comes_through() {
        let original = tiny(true);
        let text = export(ModelFormat::Mesh3de, &original, "t.3de").unwrap();
        let edited = text.replacen("[1.0, 0.5, 0.25,", "[1.5, 0.5, 0.25,", 1);
        assert_ne!(edited, text);
        let (_, bytes) = import(edited.as_bytes()).unwrap();
        let diff: Vec<usize> = (0..bytes.len()).filter(|&i| bytes[i] != original[i]).collect();
        assert!(!diff.is_empty() && diff.len() <= 4, "{diff:?}");
        assert_eq!(Mesh3de::parse(&bytes, "x").unwrap().vertices[1].pos[0], 1.5);
    }

    #[test]
    fn a_3de_layout_that_disagrees_with_its_frame_block_is_refused() {
        let with = export(ModelFormat::Mesh3de, &tiny(true), "t.3de").unwrap();
        let e = import(with.replacen("\"standard\"", "\"static\"", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("frame_block must be null"), "{e}");
        let without = export(ModelFormat::Mesh3de, &tiny(false), "t.3de").unwrap();
        let e = import(without.replacen("\"static\"", "\"standard\"", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("needs a frame_block"), "{e}");
        let e = import(without.replacen("\"static\"", "\"other\"", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)), "{e}");
        // the key must be present
        let missing = without.replacen(",\n  \"frame_block\": null", "", 1);
        assert_ne!(missing, without);
        assert!(matches!(import(missing.as_bytes()), Err(JsonError::Json { .. })));
    }

    #[test]
    fn a_3de_frame_with_a_missing_entry_is_refused() {
        let text = export(ModelFormat::Mesh3de, &tiny(true), "t.3de").unwrap();
        let line = "          [0.0, 1.5, 0.0, 0.25, 0.75],\n";
        assert!(text.contains(line), "{text}");
        let e = import(text.replacen(line, "", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("frame_block.frames[1].vertices"), "{e}");
    }

    #[test]
    fn a_3de_in_another_layout_is_not_exported() {
        // a file that neither layout reads: a texture name, then a vertex count with no vertices
        let mut d = 2u32.to_le_bytes().to_vec();
        d.extend_from_slice(b"a\0");
        d.extend_from_slice(&5i32.to_le_bytes());
        let e = export(ModelFormat::Mesh3de, &d, "other.3de").unwrap_err();
        assert!(matches!(e, JsonError::Unreadable(_)) && e.to_string().contains(NOT_THE_LAYOUT), "{e}");
        // the standard layout with bytes after the last frame is not read either
        let mut longer = tiny(true);
        longer.push(0);
        let e = export(ModelFormat::Mesh3de, &longer, "other.3de").unwrap_err();
        assert!(e.to_string().contains(NOT_THE_LAYOUT), "{e}");
    }

    #[test]
    fn an_empty_3de_is_not_exported() {
        let e = export(ModelFormat::Mesh3de, &[], "e.3de").unwrap_err();
        assert!(matches!(e, JsonError::Unreadable(_)) && e.to_string().contains("empty file"), "{e}");
    }

    #[test]
    fn unknown_keys_columns_and_records_are_refused() {
        let text = export(ModelFormat::Mesh3de, &tiny(true), "t.3de").unwrap();
        assert!(matches!(import(text.replacen("\"faces\"", "\"extra\": 1,\n  \"faces\"", 1).as_bytes()), Err(JsonError::Json { .. })));
        assert!(matches!(import(text.replacen("\"dx\"", "\"dy\"", 1).as_bytes()), Err(JsonError::Schema(_))));
        assert!(matches!(import(text.replacen("\"max_key\": 7,", "\"max_key\": 7,\n    \"extra\": 1,", 1).as_bytes()), Err(JsonError::Json { .. })));
        // a vertex field that is not a whole number
        assert!(matches!(import(text.replacen("0.25, -1, 0.5", "0.25, -1.5, 0.5", 1).as_bytes()), Err(JsonError::Json { .. })));
        let e = import(br#"{"format": "shaiya-toolkit-3de-v1", "layout": "standard", "frame_block": [7, []]}"#);
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
    }
}
