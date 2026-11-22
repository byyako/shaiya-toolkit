//! Previews of `.VANI`, `.MANI`, `.3DO` and `.3DE` files, built on the worker thread (see `preview::make_preview_with`). The pictures come
//! from `shaiya_clientdata::render`; a file that does not read to its last byte is an error, which the caller shows as Failed with hex.

use crate::preview::{FieldRow, MeshChoice, PreviewData};
use shaiya_archive::Archive;
use shaiya_clientdata::mani::{ManiType, ManiValue};
use shaiya_clientdata::mesh3de::De3Layout;
use shaiya_clientdata::render::{self, MorphItem, MAX_ANIM_FRAMES};
use shaiya_core::Job;

/// Playback speed when a file's own rate is not a positive number.
const FALLBACK_RATE: f32 = 30.0;

fn n(count: usize, one: &str, many: &str) -> String {
    shaiya_inspect::counted(count as u64, one, many)
}

fn cancelled() -> shaiya_clientdata::Error {
    shaiya_clientdata::Error::Archive("cancelled".into())
}

/// A texture of the client's `Entity/Texture` folder by the `.smod` rule (the name as written, else the same stem as `.dds`, else `.tga`).
fn archive_texture(archive: &Archive, job: &Job, name: &str) -> Option<shaiya_clientdata::Image> {
    if job.check().is_err() {
        return None;
    }
    let found = shaiya_clientdata::texture_candidates("Entity/Texture", name).into_iter().find_map(|c| archive.index().get(&c).map(|e| (c, e)));
    let (c, e) = found?;
    shaiya_clientdata::texture::decode(&archive.read_entry(e).ok()?, &c).ok()
}

fn prepared_note(step: f32, unit: &str) -> Option<String> {
    (step > 1.0).then(|| format!("the frames are prepared before playing: one every {step} {unit}s (at most {MAX_ANIM_FRAMES} frames)"))
}

/// The `.VANI` preview: the model's pictures, one per step, played at one step per `frame_ms` milliseconds (inferred).
pub fn vani_preview(archive: &Archive, data: &[u8], path: &str, job: &Job) -> shaiya_clientdata::Result<PreviewData> {
    let v = shaiya_clientdata::Vani::parse(data, path)?;
    let (verts, tris) = v.counts();
    let items: Vec<MorphItem> = v.items.iter().enumerate().map(|(k, it)| MorphItem { texture: v.texture_name(k), faces: &it.faces, uv: v.step_vertices(k, 0).iter().map(|x| x.uv).collect() }).collect();
    let made = render::morph_frames(
        &items,
        v.step_count,
        &mut |k, step| v.step_vertices(k, step).iter().map(|x| x.pos).collect(),
        &mut |name| archive_texture(archive, job, name),
        &mut || job.check().is_err(),
    )
    .ok_or_else(cancelled)?;
    job.check().map_err(|_| cancelled())?;
    let positive = v.frame_ms > 0;
    let rate = if positive { 1000.0 / v.frame_ms as f32 } else { FALLBACK_RATE };
    let mut header = vec![
        format!("Vertex-animated model: {}, {} per step, {}", n(v.items.len(), "item", "items"), n(verts, "vertex", "vertices"), n(tris, "triangle", "triangles")),
        if positive {
            format!("{}, about {} ms per step (inferred from the file; not checked in the game)", n(v.step_count, "step", "steps"), v.frame_ms)
        } else {
            format!("{}, the file's time per step ({} ms) is not a positive number; played at {FALLBACK_RATE} steps per second", n(v.step_count, "step", "steps"), v.frame_ms)
        },
    ];
    if v.has_vertex_colours() {
        header.push("some vertices carry a colour other than white (not used by the picture)".to_string());
    }
    let mut notes = Vec::new();
    if !made.missing.is_empty() {
        notes.push(format!("texture missing or unreadable, drawn grey: {}", made.missing.join(", ")));
    }
    if v.collision.is_some() {
        notes.push("the file holds a collision mesh (read, not drawn)".to_string());
    }
    notes.extend(prepared_note(made.frames.step, "step"));
    Ok(PreviewData::Anim {
        header,
        issues: Vec::new(),
        bones: Vec::new(),
        end_key: v.step_count.saturating_sub(1) as i32,
        frames: made.frames,
        mesh: MeshChoice::Skeleton,
        mesh_choices: Vec::new(),
        notes,
        rate,
        unit: "step",
    })
}

fn texture_note(name: &str) -> Option<String> {
    (!name.is_empty()).then(|| format!("the file names the texture {name}; textures are not looked up (shown in plain grey)"))
}

fn rest_picture(vertices: &[([f32; 3], [f32; 2])], faces: &[[u16; 3]]) -> (u32, u32, Vec<u8>) {
    let pos: Vec<[f32; 3]> = vertices.iter().map(|v| v.0).collect();
    let uv: Vec<[f32; 2]> = vertices.iter().map(|v| v.1).collect();
    match render::plain_mesh_picture(&pos, &uv, faces) {
        Some(p) => (p.width as u32, p.height as u32, p.rgba),
        None => (0, 0, Vec::new()),
    }
}

/// The `.3DO` preview: the rest mesh in plain grey; a mesh with frames plays them at its frame rate (inferred).
pub fn m3do_preview(data: &[u8], path: &str, job: &Job) -> shaiya_clientdata::Result<PreviewData> {
    let m = shaiya_clientdata::Mesh3do::parse(data, path)?;
    let (verts, tris) = m.counts();
    let first = format!("3DO mesh: {}, {}", n(verts, "vertex", "vertices"), n(tris, "triangle", "triangles"));
    let mut notes: Vec<String> = texture_note(&m.texture_name()).into_iter().collect();
    if m.frames.is_empty() {
        let (width, height, rgba) = rest_picture(&m.vertices.iter().map(|x| (x.pos, x.uv)).collect::<Vec<_>>(), &m.faces);
        return Ok(PreviewData::Model { header: first, missing: Vec::new(), see_through: Vec::new(), invisible: Vec::new(), notes, width, height, rgba });
    }
    let positive = m.frame_rate > 0;
    let rate = if positive { m.frame_rate as f32 } else { FALLBACK_RATE };
    let second = if positive {
        format!("{} at {} per second (inferred from the file; not checked in the game)", n(m.frames.len(), "frame", "frames"), m.frame_rate)
    } else {
        format!("{}, the file's rate ({}) is not a positive number; played at {FALLBACK_RATE} frames per second", n(m.frames.len(), "frame", "frames"), m.frame_rate)
    };
    let items = [MorphItem { texture: String::new(), faces: &m.faces, uv: m.vertices.iter().map(|x| x.uv).collect() }];
    let made = render::morph_frames(&items, m.frames.len(), &mut |_, f| m.frames[f].vertices.iter().map(|x| x.0).collect(), &mut |_| None, &mut || job.check().is_err()).ok_or_else(cancelled)?;
    job.check().map_err(|_| cancelled())?;
    notes.extend(prepared_note(made.frames.step, "frame"));
    Ok(PreviewData::Anim {
        header: vec![first, second],
        issues: Vec::new(),
        bones: Vec::new(),
        end_key: m.frames.len().saturating_sub(1) as i32,
        frames: made.frames,
        mesh: MeshChoice::Skeleton,
        mesh_choices: Vec::new(),
        notes,
        rate,
        unit: "frame",
    })
}

/// The `.3DE` preview: the static mesh in plain grey, and how many frames the file holds (not played: their meaning is inferred). A file in
/// neither known layout is an error (shown as Failed with its bytes).
pub fn m3de_preview(data: &[u8], path: &str) -> shaiya_clientdata::Result<PreviewData> {
    let m = shaiya_clientdata::Mesh3de::parse(data, path)?;
    let (verts, tris) = m.counts();
    let header = format!("Effect mesh: {}, {}", n(verts, "vertex", "vertices"), n(tris, "triangle", "triangles"));
    let mut notes = Vec::new();
    match m.layout() {
        De3Layout::Static => notes.push("no frame block: the file ends after the faces".to_string()),
        De3Layout::Standard if m.frame_count() == 0 => notes.push("0 frames".to_string()),
        De3Layout::Standard => notes.push(format!("{} (not played: their meaning is inferred)", n(m.frame_count(), "frame", "frames"))),
    }
    notes.extend(texture_note(&m.texture_name()));
    let (width, height, rgba) = rest_picture(&m.vertices.iter().map(|x| (x.pos, x.uv)).collect::<Vec<_>>(), &m.faces);
    Ok(PreviewData::Model { header, missing: Vec::new(), see_through: Vec::new(), invisible: Vec::new(), notes, width, height, rgba })
}

/// The `.MANI` preview: a labelled table of the 20 fields; every meaning is inferred. The value is shown as a number; the stored bits of a
/// float are in `raw` (shown on hover).
pub fn mani_preview(data: &[u8], path: &str) -> shaiya_clientdata::Result<PreviewData> {
    let m = shaiya_clientdata::Mani::parse(data, path)?;
    let bits = |at: usize| format!("0x{:08x}", u32::from_le_bytes([m.raw[at], m.raw[at + 1], m.raw[at + 2], m.raw[at + 3]]));
    let rows = m
        .fields()
        .into_iter()
        .map(|(f, v)| {
            let (mut value, raw) = match v {
                ManiValue::I32(x) => (x.to_string(), format!("{x} stored as 0x{:08x}", x as u32)),
                ManiValue::U16(x) => (x.to_string(), format!("{x} stored as 0x{x:04x}")),
                ManiValue::F32(x) => (x.to_string(), format!("stored as {}", bits(f.offset))),
                ManiValue::F32x3(x) => (format!("[{}, {}, {}]", x[0], x[1], x[2]), format!("stored as {} {} {}", bits(f.offset), bits(f.offset + 4), bits(f.offset + 8))),
            };
            if f.offset == 104 && f.kind == ManiType::F32 {
                if let Some(i) = m.distance_as_integer() {
                    value.push_str(&format!(" or as an integer: {i}"));
                }
            }
            FieldRow { offset: f.offset, name: f.name.to_string(), value, raw, meaning: f.meaning.to_string(), inferred: true }
        })
        .collect();
    let mut notes = Vec::new();
    if m.is_integer_generation() {
        notes.push("this file holds an integer at offset 104 and zero bytes at offsets 80 to 83 (one of two writer generations; shown, not interpreted)".to_string());
    }
    Ok(PreviewData::Fields {
        header: "Motion parameters for a placed building model (108 bytes). Field meanings are inferred and were not checked in the game.".to_string(),
        notes,
        rows,
    })
}
