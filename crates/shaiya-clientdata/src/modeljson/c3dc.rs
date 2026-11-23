//! `.3DC` as JSON: schema `shaiya-toolkit-3dc-v1`.

use super::types::{check_columns, check_limit, hex_encode, hex_exact, matrix_bytes, matrix_from_bytes, texture_from_hex, Face, Obj, Size, F32};
use super::{json_error, JsonError, ModelFormat};
use crate::mesh3dc::{C3dc, C3dcLayout, C3dcMesh, C3dcVertex, BONE_LIMIT, COUNT_LIMIT, TEXTURE_LIMIT};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

const VERTEX_COLUMNS: [&str; 12] = ["x", "y", "z", "weight", "bone1", "bone2", "pad", "nx", "ny", "nz", "u", "v"];

/// `[x, y, z, weight, bone1, bone2, pad, nx, ny, nz, u, v]`
type VertexRec = (F32, F32, F32, F32, u8, u8, u16, F32, F32, F32, F32, F32);

fn layout_name(l: C3dcLayout) -> &'static str {
    match l {
        C3dcLayout::Skinned => "skinned",
        C3dcLayout::SkinnedWithSecondMesh => "skinned-with-second-mesh",
        C3dcLayout::Cloak => "cloak",
    }
}

// ---- export -----------------------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct Columns {
    vertices: &'static [&'static str],
}

#[derive(Serialize)]
struct MeshOut {
    texture_hex: String,
    bones_hex: Vec<String>,
    vertices: Vec<VertexRec>,
    faces: Vec<Face>,
}

#[derive(Serialize)]
struct C3dcOut<'a> {
    format: &'static str,
    source: &'a str,
    columns: Columns,
    layout: &'static str,
    meshes: Vec<MeshOut>,
}

pub(super) fn export(bytes: &[u8], source: &str, json_cap: u64) -> Result<String, JsonError> {
    let c = C3dc::parse(bytes, source).map_err(JsonError::Unreadable)?;
    let meshes = c
        .meshes
        .iter()
        .map(|m| MeshOut {
            texture_hex: hex_encode(&m.texture),
            bones_hex: m.bones.iter().map(|b| hex_encode(&matrix_bytes(b))).collect(),
            vertices: m
                .vertices
                .iter()
                .map(|v| (F32(v.pos[0]), F32(v.pos[1]), F32(v.pos[2]), F32(v.weight), v.bone1, v.bone2, v.pad, F32(v.normal[0]), F32(v.normal[1]), F32(v.normal[2]), F32(v.uv[0]), F32(v.uv[1])))
                .collect(),
            faces: m.faces.clone(),
        })
        .collect();
    let out = C3dcOut { format: ModelFormat::C3dc.schema(), source, columns: Columns { vertices: &VERTEX_COLUMNS }, layout: layout_name(c.layout), meshes };
    super::render(&out, json_cap)
}

// ---- import -----------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnsIn {
    vertices: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MeshIn {
    texture_hex: String,
    bones_hex: Vec<String>,
    vertices: Vec<VertexRec>,
    faces: Vec<Face>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct C3dcIn {
    #[allow(dead_code)]
    format: String,
    #[serde(default)]
    #[allow(dead_code)]
    source: Option<IgnoredAny>,
    #[serde(default)]
    columns: Option<Obj<ColumnsIn>>,
    layout: String,
    meshes: Vec<Obj<MeshIn>>,
}

pub(super) fn import(json: &[u8], model_cap: u64) -> Result<Vec<u8>, JsonError> {
    let Obj(input): Obj<C3dcIn> = serde_json::from_slice(json).map_err(json_error)?;
    if let Some(Obj(c)) = &input.columns {
        check_columns(&c.vertices, &VERTEX_COLUMNS, "columns.vertices")?;
    }
    let (layout, count, with_bones) = match input.layout.as_str() {
        "skinned" => (C3dcLayout::Skinned, 1, true),
        "skinned-with-second-mesh" => (C3dcLayout::SkinnedWithSecondMesh, 2, true),
        "cloak" => (C3dcLayout::Cloak, 1, false),
        other => return Err(JsonError::Schema(format!("layout \"{other}\" is not one of \"skinned\", \"skinned-with-second-mesh\", \"cloak\""))),
    };
    if input.meshes.len() != count {
        return Err(JsonError::Schema(format!("layout \"{}\" needs {count} mesh(es), found {}", input.layout, input.meshes.len())));
    }
    let mut size = Size::new(0);
    for (i, Obj(m)) in input.meshes.iter().enumerate() {
        if !with_bones && !m.bones_hex.is_empty() {
            return Err(JsonError::Schema(format!("meshes[{i}].bones_hex: a cloak has no bone block, so the list must be empty")));
        }
        check_limit(m.texture_hex.len() / 2, TEXTURE_LIMIT, &format!("meshes[{i}].texture_hex"))?;
        check_limit(m.bones_hex.len(), BONE_LIMIT, &format!("meshes[{i}].bones_hex"))?;
        check_limit(m.vertices.len(), COUNT_LIMIT, &format!("meshes[{i}].vertices"))?;
        check_limit(m.faces.len(), COUNT_LIMIT, &format!("meshes[{i}].faces"))?;
        size.add(4 + (m.texture_hex.len() / 2) as u64);
        if with_bones {
            size.add(4);
            size.add_mul(m.bones_hex.len(), 64);
        }
        size.add(4);
        size.add_mul(m.vertices.len(), 40);
        size.add(4);
        size.add_mul(m.faces.len(), 6);
    }
    size.finish(model_cap)?;

    let mut meshes = Vec::with_capacity(input.meshes.len());
    for (i, Obj(m)) in input.meshes.iter().enumerate() {
        let texture = texture_from_hex(&m.texture_hex, TEXTURE_LIMIT, &format!("meshes[{i}].texture_hex"))?;
        let mut bones = Vec::with_capacity(m.bones_hex.len());
        for (j, h) in m.bones_hex.iter().enumerate() {
            let raw: [u8; 64] = hex_exact(h, &format!("meshes[{i}].bones_hex[{j}]"))?;
            bones.push(matrix_from_bytes(&raw));
        }
        let vertices = m
            .vertices
            .iter()
            .map(|v| C3dcVertex { pos: [v.0 .0, v.1 .0, v.2 .0], weight: v.3 .0, bone1: v.4, bone2: v.5, pad: v.6, normal: [v.7 .0, v.8 .0, v.9 .0], uv: [v.10 .0, v.11 .0] })
            .collect();
        meshes.push(C3dcMesh { texture, bones, vertices, faces: m.faces.clone() });
    }
    C3dc { layout, meshes }.try_to_bytes().map_err(|e| JsonError::NotWritable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::{export, import, ModelFormat};
    use super::*;
    use shaiya_testkit::anim::{arm_mesh_3dc, c3dc_bytes, c3dc_cloak_bytes, identity_bits, VertSpec};

    fn vert(x: f32, bone1: u8, pad: u16) -> VertSpec {
        VertSpec { pos: [x, 0.5, -0.25], weight: 0.75, bone1, bone2: 1, pad, normal: [0.0, 1.0, 0.0], uv: [0.1, 0.9] }
    }

    fn small() -> Vec<u8> {
        c3dc_bytes(b"a.tga\0", &[identity_bits()], &[vert(0.0, 0, 0), vert(1.0, 0, 0), vert(2.0, 0, 7)], &[[0, 1, 2]])
    }

    fn round_trip(bytes: &[u8]) -> String {
        let text = export(ModelFormat::C3dc, bytes, "t.3dc").unwrap();
        let (f, back) = import(text.as_bytes()).unwrap();
        assert_eq!(f, ModelFormat::C3dc);
        assert_eq!(back, bytes);
        text
    }

    #[test]
    fn the_3dc_test_file_exports_to_the_documented_json() {
        // Recorded from the code at the first build, then checked line by line against the schema table and the layout rules.
        let expected = r#"{
  "format": "shaiya-toolkit-3dc-v1",
  "source": "skin.3DC",
  "columns": {
    "vertices": [
      "x",
      "y",
      "z",
      "weight",
      "bone1",
      "bone2",
      "pad",
      "nx",
      "ny",
      "nz",
      "u",
      "v"
    ]
  },
  "layout": "skinned",
  "meshes": [
    {
      "texture_hex": "632e74676100",
      "bones_hex": [
        "0000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f000000000000000000000000000000000000803f"
      ],
      "vertices": [
        [0.0, 0.5, -0.25, 0.75, 0, 1, 0, 0.0, 1.0, 0.0, 0.1, 0.9]
      ],
      "faces": [
        [0, 0, 0]
      ]
    }
  ]
}
"#;
        let text = export(ModelFormat::C3dc, &c3dc_bytes(b"c.tga\0", &[identity_bits()], &[vert(0.0, 0, 0)], &[[0, 0, 0]]), "skin.3DC").unwrap();
        assert_eq!(text, expected);
    }

    #[test]
    fn every_3dc_test_file_round_trips() {
        round_trip(&small());
        round_trip(&arm_mesh_3dc());
        // second mesh
        let mut two = small();
        two.extend(c3dc_bytes(b"b.tga\0", &[identity_bits(), identity_bits()], &[vert(3.0, 1, 0)], &[]));
        let text = round_trip(&two);
        assert!(text.contains("\"layout\": \"skinned-with-second-mesh\""));
        // cloak
        let text = round_trip(&c3dc_cloak_bytes(b"c.tga\0", &[vert(0.0, 0, 0), vert(1.0, 2, 9)], &[[0, 1, 1]]));
        assert!(text.contains("\"layout\": \"cloak\"") && text.contains("\"bones_hex\": []"));
        // a non-zero pad and bytes after the NUL of the texture name
        let text = round_trip(&c3dc_bytes(b"x.tga\0junk", &[], &[vert(0.0, 3, 65535)], &[[0, 0, 0]]));
        assert!(text.contains("782e7467610") && text.contains("6a756e6b"), "{text}");
        assert!(text.contains("65535"));
        // no vertices, no faces
        round_trip(&c3dc_bytes(b"", &[], &[], &[]));
    }

    #[test]
    fn a_nan_vertex_normal_is_written_as_bits() {
        let mut v = vert(0.0, 0, 0);
        v.normal = [f32::NAN, f32::INFINITY, f32::from_bits(0xFFC0_0001)];
        let text = round_trip(&c3dc_bytes(b"n.tga\0", &[identity_bits()], &[v], &[[0, 0, 0]]));
        assert!(text.contains("\"bits:7fc00000\"") && text.contains("\"bits:7f800000\"") && text.contains("\"bits:ffc00001\""), "{text}");
    }

    #[test]
    fn an_edited_vertex_coordinate_comes_through() {
        let original = small();
        let text = export(ModelFormat::C3dc, &original, "t.3dc").unwrap();
        let edited = text.replacen("[1.0, 0.5, -0.25,", "[1.5, 0.5, -0.25,", 1);
        assert_ne!(edited, text);
        let (_, bytes) = import(edited.as_bytes()).unwrap();
        assert_eq!(bytes.len(), original.len());
        let diff: Vec<usize> = (0..bytes.len()).filter(|&i| bytes[i] != original[i]).collect();
        assert!(!diff.is_empty() && diff.len() <= 4, "{diff:?}");
        let a = C3dc::parse(&bytes, "x").unwrap();
        assert_eq!(a.meshes[0].vertices[1].pos, [1.5, 0.5, -0.25]);
    }

    #[test]
    fn a_cloak_with_bones_is_refused() {
        let text = export(ModelFormat::C3dc, &c3dc_cloak_bytes(b"c.tga\0", &[vert(0.0, 0, 0)], &[]), "c.3dc").unwrap();
        let with = text.replacen("\"bones_hex\": []", &format!("\"bones_hex\": [\"{}\"]", "00".repeat(64)), 1);
        assert_ne!(with, text);
        let e = import(with.as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("meshes[0].bones_hex"), "{e}");
    }

    #[test]
    fn the_mesh_count_must_fit_the_layout() {
        let text = export(ModelFormat::C3dc, &small(), "t.3dc").unwrap();
        let e = import(text.replacen("\"skinned\"", "\"skinned-with-second-mesh\"", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("needs 2 mesh"), "{e}");
        let e = import(text.replacen("\"skinned\"", "\"other\"", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)), "{e}");
    }

    #[test]
    fn a_3dc_that_two_layouts_would_read_is_refused() {
        // The mesh of `mesh3dc.rs` `try_to_bytes_refuses_a_mesh_another_layout_also_reads`, written by hand (it has no file to export):
        // skinned, empty texture, one bone whose element 10 holds the bits 18, two vertices, no faces. The same bytes also read as a cloak.
        let bone = format!("{}12000000{}", "00".repeat(40), "00".repeat(20));
        let vertex = "[0.0, 0.0, 0.0, 0.0, 0, 0, 0, 0.0, 0.0, 0.0, 0.0, 0.0]";
        let text = format!(r#"{{"format": "shaiya-toolkit-3dc-v1", "layout": "skinned", "meshes": [{{"texture_hex": "", "bones_hex": ["{bone}"], "vertices": [{vertex}, {vertex}], "faces": []}}]}}"#);
        let e = import(text.as_bytes());
        assert!(matches!(e, Err(JsonError::NotWritable(_))), "{e:?}");
    }

    #[test]
    fn unknown_keys_columns_and_records_are_refused() {
        let text = export(ModelFormat::C3dc, &small(), "t.3dc").unwrap();
        assert!(matches!(import(text.replacen("\"layout\"", "\"extra\": 1,\n  \"layout\"", 1).as_bytes()), Err(JsonError::Json { .. })));
        assert!(matches!(import(text.replacen("\"faces\"", "\"extra\": 1,\n      \"faces\"", 1).as_bytes()), Err(JsonError::Json { .. })));
        assert!(matches!(import(text.replacen("\"weight\"", "\"bone1\"", 1).as_bytes()), Err(JsonError::Schema(_))));
        assert!(matches!(import(text.replacen("0.75, 0, 1, 0,", "0.75, 0, 1,", 1).as_bytes()), Err(JsonError::Json { .. })));
        // 300 does not fit a bone byte
        assert!(matches!(import(text.replacen("0.75, 0, 1, 0,", "0.75, 300, 1, 0,", 1).as_bytes()), Err(JsonError::Json { .. })));
        // a mesh written as an array is not an object
        let e = import(br#"{"format": "shaiya-toolkit-3dc-v1", "layout": "skinned", "meshes": [["", [], [], []]]}"#);
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
        let e = import(br#"["shaiya-toolkit-3dc-v1", "skinned", []]"#);
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
    }

    #[test]
    fn bad_hex_in_a_3dc_is_refused() {
        let text = export(ModelFormat::C3dc, &small(), "t.3dc").unwrap();
        let first = text.find("0000803f").unwrap();
        let digits = &text[first..first + 128];
        for bad in [&digits[..126], "zz"] {
            let e = import(text.replacen(digits, bad, 1).as_bytes()).unwrap_err();
            assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("meshes[0].bones_hex[0]"), "{e}");
        }
        let e = import(text.replacen("\"texture_hex\": \"", "\"texture_hex\": \"0", 1).as_bytes()).unwrap_err();
        assert!(e.to_string().contains("meshes[0].texture_hex"), "{e}");
    }
}
