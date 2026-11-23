//! `.3DO` as JSON: schema `shaiya-toolkit-3do-v1`.

use super::types::{check_columns, check_limit, hex_encode, texture_from_hex, Face, Obj, Size, F32};
use super::{json_error, JsonError, ModelFormat};
use crate::mesh3do::{Frame3do, Mesh3do, Vertex3do, COUNT_LIMIT, FRAME_LIMIT, TEXTURE_LIMIT};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

const VERTEX_COLUMNS: [&str; 8] = ["x", "y", "z", "nx", "ny", "nz", "u", "v"];
const FRAME_COLUMNS: [&str; 6] = ["x", "y", "z", "nx", "ny", "nz"];

/// `[x, y, z, nx, ny, nz, u, v]`
type VertexRec = (F32, F32, F32, F32, F32, F32, F32, F32);
/// `[x, y, z, nx, ny, nz]`
type FrameRec = (F32, F32, F32, F32, F32, F32);

// ---- export -----------------------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct Inferred {
    frame_rate: &'static str,
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
struct Out<'a> {
    format: &'static str,
    source: &'a str,
    inferred: Inferred,
    columns: Columns,
    texture_hex: String,
    vertices: Vec<VertexRec>,
    faces: Vec<Face>,
    frame_rate: i32,
    frames: Vec<FrameOut>,
}

pub(super) fn export(bytes: &[u8], source: &str, json_cap: u64) -> Result<String, JsonError> {
    let m = Mesh3do::parse(bytes, source).map_err(JsonError::Unreadable)?;
    let out = Out {
        format: ModelFormat::Mesh3do.schema(),
        source,
        inferred: Inferred { frame_rate: "frames per second" },
        columns: Columns { vertices: &VERTEX_COLUMNS, frame_vertices: &FRAME_COLUMNS },
        texture_hex: hex_encode(&m.texture),
        vertices: m.vertices.iter().map(|v| (F32(v.pos[0]), F32(v.pos[1]), F32(v.pos[2]), F32(v.normal[0]), F32(v.normal[1]), F32(v.normal[2]), F32(v.uv[0]), F32(v.uv[1]))).collect(),
        faces: m.faces.clone(),
        frame_rate: m.frame_rate,
        frames: m
            .frames
            .iter()
            .map(|f| FrameOut { key: f.key, vertices: f.vertices.iter().map(|(p, n)| (F32(p[0]), F32(p[1]), F32(p[2]), F32(n[0]), F32(n[1]), F32(n[2]))).collect() })
            .collect(),
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
    texture_hex: String,
    vertices: Vec<VertexRec>,
    faces: Vec<Face>,
    frame_rate: i32,
    frames: Vec<Obj<FrameIn>>,
}

pub(super) fn import(json: &[u8], model_cap: u64) -> Result<Vec<u8>, JsonError> {
    let Obj(input): Obj<In> = serde_json::from_slice(json).map_err(json_error)?;
    if let Some(Obj(c)) = &input.columns {
        check_columns(&c.vertices, &VERTEX_COLUMNS, "columns.vertices")?;
        check_columns(&c.frame_vertices, &FRAME_COLUMNS, "columns.frame_vertices")?;
    }
    check_limit(input.texture_hex.len() / 2, TEXTURE_LIMIT, "texture_hex")?;
    check_limit(input.vertices.len(), COUNT_LIMIT, "vertices")?;
    check_limit(input.faces.len(), COUNT_LIMIT, "faces")?;
    check_limit(input.frames.len(), FRAME_LIMIT, "frames")?;
    for (i, Obj(f)) in input.frames.iter().enumerate() {
        if f.vertices.len() != input.vertices.len() {
            return Err(JsonError::Schema(format!("frames[{i}].vertices: {} entries for {} vertices (a frame holds one entry per vertex)", f.vertices.len(), input.vertices.len())));
        }
    }
    let mut size = Size::new(4 + (input.texture_hex.len() / 2) as u64 + 4);
    size.add_mul(input.vertices.len(), 32);
    size.add(4 + 4 + 4);
    size.add_mul(input.faces.len(), 6);
    size.add_mul(input.frames.len(), 4);
    size.add_mul(input.frames.iter().map(|f| f.0.vertices.len()).sum(), 24);
    size.finish(model_cap)?;

    let mesh = Mesh3do {
        texture: texture_from_hex(&input.texture_hex, TEXTURE_LIMIT, "texture_hex")?,
        vertices: input.vertices.iter().map(|v| Vertex3do { pos: [v.0 .0, v.1 .0, v.2 .0], normal: [v.3 .0, v.4 .0, v.5 .0], uv: [v.6 .0, v.7 .0] }).collect(),
        faces: input.faces.clone(),
        frame_rate: input.frame_rate,
        frames: input
            .frames
            .iter()
            .map(|Obj(f)| Frame3do { key: f.key, vertices: f.vertices.iter().map(|v| ([v.0 .0, v.1 .0, v.2 .0], [v.3 .0, v.4 .0, v.5 .0])).collect() })
            .collect(),
    };
    mesh.try_to_bytes().map_err(|e| JsonError::NotWritable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::{export, import, ModelFormat};
    use super::*;
    use shaiya_testkit::anim::{animated_3do, box_3do, mesh3do_bytes, Mesh3doSpec, Vert3doSpec};

    fn tiny() -> Vec<u8> {
        let v = |x: f32| Vert3doSpec { pos: [x, 0.5, 0.25], normal: [0.0, 1.0, 0.0], uv: [0.5, 0.125] };
        mesh3do_bytes(&Mesh3doSpec {
            texture: b"t.tga\0".to_vec(),
            vertices: vec![v(0.0), v(1.0), v(2.0)],
            faces: vec![[0, 1, 2]],
            frame_rate: 20,
            frames: vec![(0, vec![([0.0, 0.5, 0.25], [0.0, 1.0, 0.0]); 3]), (5, vec![([0.0, 0.75, 0.25], [0.0, 1.0, 0.0]); 3])],
        })
    }

    fn round_trip(bytes: &[u8]) -> String {
        let text = export(ModelFormat::Mesh3do, bytes, "t.3do").unwrap();
        let (f, back) = import(text.as_bytes()).unwrap();
        assert_eq!(f, ModelFormat::Mesh3do);
        assert_eq!(back, bytes);
        text
    }

    #[test]
    fn the_3do_test_file_exports_to_the_documented_json() {
        // Recorded from the code at the first build, then checked line by line against the schema table and the layout rules.
        let expected = r#"{
  "format": "shaiya-toolkit-3do-v1",
  "source": "tiny.3DO",
  "inferred": {
    "frame_rate": "frames per second"
  },
  "columns": {
    "vertices": [
      "x",
      "y",
      "z",
      "nx",
      "ny",
      "nz",
      "u",
      "v"
    ],
    "frame_vertices": [
      "x",
      "y",
      "z",
      "nx",
      "ny",
      "nz"
    ]
  },
  "texture_hex": "742e74676100",
  "vertices": [
    [0.0, 0.5, 0.25, 0.0, 1.0, 0.0, 0.5, 0.125],
    [1.0, 0.5, 0.25, 0.0, 1.0, 0.0, 0.5, 0.125],
    [2.0, 0.5, 0.25, 0.0, 1.0, 0.0, 0.5, 0.125]
  ],
  "faces": [
    [0, 1, 2]
  ],
  "frame_rate": 20,
  "frames": [
    {
      "key": 0,
      "vertices": [
        [0.0, 0.5, 0.25, 0.0, 1.0, 0.0],
        [0.0, 0.5, 0.25, 0.0, 1.0, 0.0],
        [0.0, 0.5, 0.25, 0.0, 1.0, 0.0]
      ]
    },
    {
      "key": 5,
      "vertices": [
        [0.0, 0.75, 0.25, 0.0, 1.0, 0.0],
        [0.0, 0.75, 0.25, 0.0, 1.0, 0.0],
        [0.0, 0.75, 0.25, 0.0, 1.0, 0.0]
      ]
    }
  ]
}
"#;
        assert_eq!(export(ModelFormat::Mesh3do, &tiny(), "tiny.3DO").unwrap(), expected);
    }

    #[test]
    fn every_3do_test_file_round_trips() {
        round_trip(&tiny());
        let text = round_trip(&box_3do());
        assert!(text.contains("\"frames\": []"), "{text}");
        round_trip(&animated_3do(11, 20));
        round_trip(&animated_3do(0, 0));
        // bytes after the NUL of the texture name, and NaN in a frame normal
        let mut spec = Mesh3doSpec {
            texture: b"x\0junk".to_vec(),
            vertices: vec![Vert3doSpec { pos: [f32::NAN, 0.0, 0.0], normal: [0.0; 3], uv: [0.0; 2] }],
            faces: vec![],
            frame_rate: -1,
            frames: vec![(-3, vec![([0.0; 3], [f32::INFINITY, 0.0, 0.0])])],
        };
        let text = round_trip(&mesh3do_bytes(&spec));
        assert!(text.contains("\"bits:7fc00000\"") && text.contains("\"bits:7f800000\"") && text.contains("\"78006a756e6b\""), "{text}");
        spec.frames.clear();
        round_trip(&mesh3do_bytes(&spec));
    }

    #[test]
    fn an_edited_vertex_coordinate_comes_through() {
        let original = tiny();
        let text = export(ModelFormat::Mesh3do, &original, "t.3do").unwrap();
        let edited = text.replacen("[1.0, 0.5, 0.25,", "[1.5, 0.5, 0.25,", 1);
        assert_ne!(edited, text);
        let (_, bytes) = import(edited.as_bytes()).unwrap();
        let diff: Vec<usize> = (0..bytes.len()).filter(|&i| bytes[i] != original[i]).collect();
        assert!(!diff.is_empty() && diff.len() <= 4, "{diff:?}");
        assert_eq!(Mesh3do::parse(&bytes, "x").unwrap().vertices[1].pos[0], 1.5);
    }

    #[test]
    fn a_3do_frame_with_a_missing_vertex_is_refused() {
        let text = export(ModelFormat::Mesh3do, &tiny(), "t.3do").unwrap();
        let line = "        [0.0, 0.75, 0.25, 0.0, 1.0, 0.0],\n";
        assert!(text.contains(line), "{text}");
        let e = import(text.replacen(line, "", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("frames[1].vertices"), "{e}");
    }

    #[test]
    fn unknown_keys_columns_and_records_are_refused() {
        let text = export(ModelFormat::Mesh3do, &tiny(), "t.3do").unwrap();
        assert!(matches!(import(text.replacen("\"faces\"", "\"extra\": 1,\n  \"faces\"", 1).as_bytes()), Err(JsonError::Json { .. })));
        assert!(matches!(import(text.replacen("\"nx\"", "\"nz\"", 1).as_bytes()), Err(JsonError::Schema(_))));
        assert!(matches!(import(text.replacen("\"key\": 5,", "\"key\": 5,\n      \"extra\": 1,", 1).as_bytes()), Err(JsonError::Json { .. })));
        assert!(matches!(import(text.replacen("[1.0, 0.5, 0.25, 0.0, 1.0, 0.0, 0.5, 0.125]", "[1.0, 0.5, 0.25, 0.0, 1.0, 0.0, 0.5]", 1).as_bytes()), Err(JsonError::Json { .. })));
        let e = import(br#"{"format": "shaiya-toolkit-3do-v1", "frames": [[5, []]]}"#);
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
    }
}
