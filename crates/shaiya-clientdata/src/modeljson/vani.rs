//! `.VANI` as JSON: schema `shaiya-toolkit-vani-v1`.

use super::types::{Bounded, COLUMN_LIMIT, check_columns, check_limit, hex_encode, required, texture_from_hex, Face, Obj, Size, F32};
use super::{json_error, JsonError, ModelFormat};
use crate::vani::{Vani, VaniCollision, VaniItem, VaniVertex, COUNT_LIMIT, ITEM_LIMIT, STEP_LIMIT, TEXTURE_LIMIT};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

const VERTEX_COLUMNS: [&str; 9] = ["x", "y", "z", "nx", "ny", "nz", "diffuse", "u", "v"];

/// `[x, y, z, nx, ny, nz, diffuse, u, v]`
type VertexRec = (F32, F32, F32, F32, F32, F32, u32, F32, F32);
type Vec3 = [F32; 3];

fn vec3(v: [f32; 3]) -> Vec3 {
    [F32(v[0]), F32(v[1]), F32(v[2])]
}

fn plain3(v: Vec3) -> [f32; 3] {
    [v[0].0, v[1].0, v[2].0]
}

// ---- export -----------------------------------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct Inferred {
    frame_ms: &'static str,
}

#[derive(Serialize)]
struct Columns {
    vertices: &'static [&'static str],
}

#[derive(Serialize)]
struct ItemOut {
    texture_hex: String,
    faces: Vec<Face>,
    verts_per_step: usize,
    vertices: Vec<VertexRec>,
}

#[derive(Serialize)]
struct CollisionOut {
    points: Vec<Vec3>,
    faces: Vec<Face>,
}

#[derive(Serialize)]
struct VaniOut<'a> {
    format: &'static str,
    source: &'a str,
    inferred: Inferred,
    columns: Columns,
    sphere_centre: Vec3,
    sphere_radius: F32,
    box_min: Vec3,
    box_max: Vec3,
    step_count: usize,
    frame_ms: i32,
    items: Vec<ItemOut>,
    box2_min: Vec3,
    box2_max: Vec3,
    collision: Option<CollisionOut>,
}

pub(super) fn export(bytes: &[u8], source: &str, json_cap: u64) -> Result<String, JsonError> {
    let v = Vani::parse(bytes, source).map_err(JsonError::Unreadable)?;
    let items = v
        .items
        .iter()
        .map(|it| ItemOut {
            texture_hex: hex_encode(&it.texture),
            faces: it.faces.clone(),
            verts_per_step: it.verts_per_step,
            vertices: it
                .vertices
                .iter()
                .map(|x| (F32(x.pos[0]), F32(x.pos[1]), F32(x.pos[2]), F32(x.normal[0]), F32(x.normal[1]), F32(x.normal[2]), x.diffuse, F32(x.uv[0]), F32(x.uv[1])))
                .collect(),
        })
        .collect();
    let out = VaniOut {
        format: ModelFormat::Vani.schema(),
        source,
        inferred: Inferred { frame_ms: "milliseconds per step" },
        columns: Columns { vertices: &VERTEX_COLUMNS },
        sphere_centre: vec3(v.centre),
        sphere_radius: F32(v.radius),
        box_min: vec3(v.min),
        box_max: vec3(v.max),
        step_count: v.step_count,
        frame_ms: v.frame_ms,
        items,
        box2_min: vec3(v.box2.0),
        box2_max: vec3(v.box2.1),
        collision: v.collision.as_ref().map(|c| CollisionOut { points: c.points.iter().map(|p| vec3(*p)).collect(), faces: c.faces.clone() }),
    };
    super::render(&out, json_cap)
}

// ---- import -----------------------------------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnsIn {
    vertices: Bounded<String, COLUMN_LIMIT>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemIn {
    texture_hex: String,
    faces: Bounded<Face, COUNT_LIMIT>,
    verts_per_step: u64,
    vertices: Bounded<VertexRec, COUNT_LIMIT>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollisionIn {
    points: Bounded<Vec3, COUNT_LIMIT>,
    faces: Bounded<Face, COUNT_LIMIT>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VaniIn {
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
    sphere_centre: Vec3,
    sphere_radius: F32,
    box_min: Vec3,
    box_max: Vec3,
    step_count: u64,
    frame_ms: i32,
    items: Bounded<Obj<ItemIn>, ITEM_LIMIT>,
    box2_min: Vec3,
    box2_max: Vec3,
    #[serde(deserialize_with = "required")]
    collision: Option<Obj<CollisionIn>>,
}

pub(super) fn import(json: &[u8], model_cap: u64) -> Result<Vec<u8>, JsonError> {
    let Obj(input): Obj<VaniIn> = serde_json::from_slice(json).map_err(json_error)?;
    if let Some(Obj(c)) = &input.columns {
        check_columns(&c.vertices, &VERTEX_COLUMNS, "columns.vertices")?;
    }
    if input.step_count > STEP_LIMIT as u64 {
        return Err(JsonError::TooLarge(format!("step_count: {} is over the limit of {STEP_LIMIT}", input.step_count)));
    }
    let step_count = input.step_count as usize;
    check_limit(input.items.len(), ITEM_LIMIT, "items")?;
    // 40 bytes of sphere and boxes, three counts, the second box and the collision flag
    let mut size = Size::new(40 + 12 + 24 + 4);
    for (i, Obj(it)) in input.items.iter().enumerate() {
        check_limit(it.texture_hex.len() / 2, TEXTURE_LIMIT, &format!("items[{i}].texture_hex"))?;
        check_limit(it.faces.len(), COUNT_LIMIT, &format!("items[{i}].faces"))?;
        if it.verts_per_step > COUNT_LIMIT as u64 {
            return Err(JsonError::TooLarge(format!("items[{i}].verts_per_step: {} is over the limit of {COUNT_LIMIT}", it.verts_per_step)));
        }
        let per_step = it.verts_per_step as usize;
        // checked before any byte is built: the vertex list holds `verts_per_step x step_count` records
        if per_step.checked_mul(step_count) != Some(it.vertices.len()) {
            return Err(JsonError::Schema(format!("items[{i}].vertices: {} records are not verts_per_step {per_step} x step_count {step_count}", it.vertices.len())));
        }
        size.add(4 + (it.texture_hex.len() / 2) as u64 + 4);
        size.add_mul(it.faces.len(), 6);
        size.add(4);
        size.add_mul(it.vertices.len(), 36);
    }
    if let Some(Obj(c)) = &input.collision {
        check_limit(c.points.len(), COUNT_LIMIT, "collision.points")?;
        check_limit(c.faces.len(), COUNT_LIMIT, "collision.faces")?;
        size.add(4 + 4);
        size.add_mul(c.points.len(), 12);
        size.add_mul(c.faces.len(), 6);
    }
    size.finish(model_cap)?;

    let mut items = Vec::with_capacity(input.items.len());
    for (i, Obj(it)) in input.items.iter().enumerate() {
        let texture = texture_from_hex(&it.texture_hex, TEXTURE_LIMIT, &format!("items[{i}].texture_hex"))?;
        let vertices = it
            .vertices
            .iter()
            .map(|x| VaniVertex { pos: [x.0 .0, x.1 .0, x.2 .0], normal: [x.3 .0, x.4 .0, x.5 .0], diffuse: x.6, uv: [x.7 .0, x.8 .0] })
            .collect();
        items.push(VaniItem { texture, faces: it.faces.clone(), verts_per_step: it.verts_per_step as usize, vertices });
    }
    let collision = input.collision.as_ref().map(|Obj(c)| VaniCollision { points: c.points.iter().map(|p| plain3(*p)).collect(), faces: c.faces.clone() });
    let vani = Vani {
        centre: plain3(input.sphere_centre),
        radius: input.sphere_radius.0,
        min: plain3(input.box_min),
        max: plain3(input.box_max),
        step_count,
        frame_ms: input.frame_ms,
        items,
        box2: (plain3(input.box2_min), plain3(input.box2_max)),
        collision,
    };
    vani.try_to_bytes().map_err(|e| JsonError::NotWritable(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::super::{export, import, ModelFormat};
    use super::*;
    use shaiya_testkit::anim::{box_vani, vani_bytes, CollisionSpec, VaniItemSpec, VaniSpec, VaniVertSpec};

    fn vv(x: f32, diffuse: u32) -> VaniVertSpec {
        VaniVertSpec { pos: [x, 0.5, 0.25], normal: [0.0, 1.0, 0.0], diffuse, uv: [0.5, 0.125] }
    }

    fn spec(steps: i32, items: Vec<VaniItemSpec>, collision: Option<CollisionSpec>) -> VaniSpec {
        VaniSpec { centre: [0.5, 0.5, 0.5], radius: 0.866, min: [0.0; 3], max: [1.0; 3], step_count: steps, frame_ms: 33, items, box2: ([0.0; 3], [1.0; 3]), collision }
    }

    fn small() -> Vec<u8> {
        let item = VaniItemSpec { texture: b"t.tga\0".to_vec(), faces: vec![[0, 1, 2]], vertices: vec![vv(0.0, 0xFFFF_FFFF), vv(1.0, 0xFFFF_FFFF), vv(2.0, 0xFFFF_FFFF), vv(0.0, 0xFFFF_FFFF), vv(1.5, 0xFFFF_FFFF), vv(2.0, 0xFFFF_FFFF)] };
        vani_bytes(&spec(2, vec![item], None))
    }

    fn round_trip(bytes: &[u8]) -> String {
        let text = export(ModelFormat::Vani, bytes, "t.vani").unwrap();
        let (f, back) = import(text.as_bytes()).unwrap();
        assert_eq!(f, ModelFormat::Vani);
        assert_eq!(back, bytes);
        text
    }

    #[test]
    fn the_vani_test_file_exports_to_the_documented_json() {
        // Recorded from the code at the first build, then checked line by line against the schema table and the layout rules.
        let expected = r#"{
  "format": "shaiya-toolkit-vani-v1",
  "source": "small.VANI",
  "inferred": {
    "frame_ms": "milliseconds per step"
  },
  "columns": {
    "vertices": [
      "x",
      "y",
      "z",
      "nx",
      "ny",
      "nz",
      "diffuse",
      "u",
      "v"
    ]
  },
  "sphere_centre": [0.5, 0.5, 0.5],
  "sphere_radius": 0.866,
  "box_min": [0.0, 0.0, 0.0],
  "box_max": [1.0, 1.0, 1.0],
  "step_count": 2,
  "frame_ms": 33,
  "items": [
    {
      "texture_hex": "742e74676100",
      "faces": [
        [0, 1, 2]
      ],
      "verts_per_step": 3,
      "vertices": [
        [0.0, 0.5, 0.25, 0.0, 1.0, 0.0, 4294967295, 0.5, 0.125],
        [1.0, 0.5, 0.25, 0.0, 1.0, 0.0, 4294967295, 0.5, 0.125],
        [2.0, 0.5, 0.25, 0.0, 1.0, 0.0, 4294967295, 0.5, 0.125],
        [0.0, 0.5, 0.25, 0.0, 1.0, 0.0, 4294967295, 0.5, 0.125],
        [1.5, 0.5, 0.25, 0.0, 1.0, 0.0, 4294967295, 0.5, 0.125],
        [2.0, 0.5, 0.25, 0.0, 1.0, 0.0, 4294967295, 0.5, 0.125]
      ]
    }
  ],
  "box2_min": [0.0, 0.0, 0.0],
  "box2_max": [1.0, 1.0, 1.0],
  "collision": null
}
"#;
        assert_eq!(export(ModelFormat::Vani, &small(), "small.VANI").unwrap(), expected);
    }

    #[test]
    fn every_vani_test_file_round_trips() {
        round_trip(&small());
        round_trip(&box_vani(4, 66));
        round_trip(&box_vani(0, 100));
        // two items
        let a = VaniItemSpec { texture: b"a.tga\0".to_vec(), faces: vec![[0, 1, 1]], vertices: vec![vv(0.0, 1), vv(1.0, 2)] };
        let b = VaniItemSpec { texture: Vec::new(), faces: vec![], vertices: vec![vv(3.0, 3), vv(4.0, 4)] };
        round_trip(&vani_bytes(&spec(1, vec![a, b], None)));
        // the collision branch
        let c = CollisionSpec { points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.5]], faces: vec![[0, 1, 2]] };
        let text = round_trip(&vani_bytes(&spec(1, vec![], Some(c))));
        assert!(text.contains("\"collision\": {"), "{text}");
        assert!(round_trip(&small()).contains("\"collision\": null"));
        // a non-white colour is the stored integer
        let item = VaniItemSpec { texture: b"t\0".to_vec(), faces: vec![], vertices: vec![vv(0.0, 0x80FF_0010)] };
        let text = round_trip(&vani_bytes(&spec(1, vec![item], None)));
        assert!(text.contains(&0x80FF_0010u32.to_string()), "{text}");
        // a NaN radius and an infinite box
        let mut s = spec(0, vec![], None);
        s.radius = f32::NAN;
        s.min = [f32::NEG_INFINITY, 0.0, 0.0];
        let text = round_trip(&vani_bytes(&s));
        assert!(text.contains("\"bits:7fc00000\"") && text.contains("\"bits:ff800000\""), "{text}");
    }

    #[test]
    fn an_edited_vertex_coordinate_comes_through() {
        let original = small();
        let text = export(ModelFormat::Vani, &original, "t.vani").unwrap();
        let edited = text.replacen("[1.5, 0.5, 0.25,", "[1.75, 0.5, 0.25,", 1);
        assert_ne!(edited, text);
        let (_, bytes) = import(edited.as_bytes()).unwrap();
        assert_eq!(bytes.len(), original.len());
        let diff: Vec<usize> = (0..bytes.len()).filter(|&i| bytes[i] != original[i]).collect();
        assert!(!diff.is_empty() && diff.len() <= 4, "{diff:?}");
        assert_eq!(Vani::parse(&bytes, "x").unwrap().items[0].vertices[4].pos[0], 1.75);
    }

    #[test]
    fn a_vani_with_the_wrong_vertex_count_is_refused() {
        // verts_per_step 2^24 with one vertex: refused by the count check, no byte buffer is made
        let text = format!(
            r#"{{"format": "shaiya-toolkit-vani-v1", "sphere_centre": [0, 0, 0], "sphere_radius": 1, "box_min": [0, 0, 0], "box_max": [0, 0, 0], "step_count": 1, "frame_ms": 33,
               "items": [{{"texture_hex": "", "faces": [], "verts_per_step": {}, "vertices": [[0, 0, 0, 0, 0, 0, 0, 0, 0]]}}], "box2_min": [0, 0, 0], "box2_max": [0, 0, 0], "collision": null}}"#,
            1u32 << 24
        );
        let e = import(text.as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("items[0].vertices"), "{e}");
        // over the limit
        let e = import(text.replacen("16777216", "16777217", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::TooLarge(_)), "{e}");
        // an ordinary mismatch
        let json = export(ModelFormat::Vani, &small(), "t.vani").unwrap();
        let e = import(json.replacen("\"verts_per_step\": 3", "\"verts_per_step\": 2", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::Schema(_)) && e.to_string().contains("items[0].vertices"), "{e}");
        // step_count over the limit
        let e = import(json.replacen("\"step_count\": 2", "\"step_count\": 65537", 1).as_bytes()).unwrap_err();
        assert!(matches!(e, JsonError::TooLarge(_)), "{e}");
    }

    #[test]
    fn unknown_keys_columns_and_a_missing_collision_are_refused() {
        let text = export(ModelFormat::Vani, &small(), "t.vani").unwrap();
        assert!(matches!(import(text.replacen("\"frame_ms\": 33,", "\"frame_ms\": 33,\n  \"extra\": 1,", 1).as_bytes()), Err(JsonError::Json { .. })));
        assert!(matches!(import(text.replacen("\"diffuse\"", "\"u\"", 1).as_bytes()), Err(JsonError::Schema(_))));
        let without = text.replacen(",\n  \"collision\": null", "", 1);
        assert_ne!(without, text);
        assert!(matches!(import(without.as_bytes()), Err(JsonError::Json { .. })));
        // an item written as an array is not an object
        let e = import(br#"{"format": "shaiya-toolkit-vani-v1", "items": [["", [], 0, []]]}"#);
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
        // a negative diffuse colour does not fit
        let e = import(text.replacen("4294967295", "-1", 1).as_bytes());
        assert!(matches!(e, Err(JsonError::Json { .. })), "{e:?}");
    }
}
