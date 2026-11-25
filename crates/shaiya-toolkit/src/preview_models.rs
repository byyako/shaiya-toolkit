//! Previews of `.VANI`, `.MANI`, `.3DO` and `.3DE` files, built on the worker thread (see `preview::make_preview_with`). The pictures come
//! from `shaiya_clientdata::render`; a file that does not read to its last byte is an error, which the caller shows as Failed with hex.

use crate::preview::{file_name, FieldRow, MeshChoice, PreviewData, TextureIndexCache};
use crate::preview_tex::{self, Look};
use crate::source::FileSource;
use shaiya_clientdata::mani::{ManiType, ManiValue};
use shaiya_clientdata::mesh3de::De3Layout;
use shaiya_clientdata::render::{self, AlphaMode, MipImage, MorphItem, View, MAX_ANIM_FRAMES};
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
fn archive_texture(src: &dyn FileSource, job: &Job, name: &str) -> Option<shaiya_clientdata::Image> {
    if job.check().is_err() {
        return None;
    }
    let c = shaiya_clientdata::texture_candidates("Entity/Texture", name).into_iter().find(|c| src.has(c))?;
    image_at(src, job, &c)
}

/// The texture file at the archive path `path`, decoded.
fn image_at(src: &dyn FileSource, job: &Job, path: &str) -> Option<shaiya_clientdata::Image> {
    if job.check().is_err() {
        return None;
    }
    shaiya_clientdata::texture::decode(&src.read(path, preview_tex::TEXTURE_CAP, &crate::source::quiet(job)).ok()?, path).ok()
}

fn prepared_note(step: f32, unit: &str) -> Option<String> {
    (step > 1.0).then(|| format!("the frames are prepared before playing: one every {step} {unit}s (at most {MAX_ANIM_FRAMES} frames)"))
}

/// The `.VANI` preview: the model's pictures, one per step, played at one step per `frame_ms` milliseconds (inferred).
pub fn vani_preview(src: &dyn FileSource, data: &[u8], path: &str, job: &Job) -> shaiya_clientdata::Result<PreviewData> {
    let v = shaiya_clientdata::Vani::parse(data, path)?;
    let (verts, tris) = v.counts();
    let items: Vec<MorphItem> = v.items.iter().enumerate().map(|(k, it)| MorphItem { texture: v.texture_name(k), faces: &it.faces, uv: v.step_vertices(k, 0).iter().map(|x| x.uv).collect() }).collect();
    let made = render::morph_frames(
        &items,
        v.step_count,
        &mut |k, step| v.step_vertices(k, step).iter().map(|x| x.pos).collect(),
        &mut |name| archive_texture(src, job, name),
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

/// The texture a `.3DO` or `.3DE` is drawn with: its archive path (`None`: grey), the alpha rule, and the notes that say where it came from
/// or why there is none.
struct Picked {
    path: Option<String>,
    alpha: AlphaMode,
    notes: Vec<String>,
}

fn grey(note: impl Into<String>) -> Picked {
    Picked { path: None, alpha: AlphaMode::Cutout, notes: vec![note.into()] }
}

/// The texture of the item model `path` (named `name` inside the file): the one its `.ITM` record names, else the file's own name in
/// `Item/DDS`.
fn pick_3do(src: &dyn FileSource, path: &str, name: &str, textures: &TextureIndexCache, look: &Look, job: &Job) -> shaiya_clientdata::Result<Picked> {
    if !look.textured {
        return Ok(grey(preview_tex::TEXTURES_OFF));
    }
    let built = textures.get_or_build(src, job).ok_or_else(cancelled)?;
    if let Some(r) = built.0.for_mesh(path).first() {
        let alpha = preview_tex::alpha_mode(r.alpha);
        let line = preview_tex::line_of(r);
        return Ok(if r.found { Picked { path: Some(r.texture.clone()), alpha, notes: vec![line] } } else { Picked { path: None, alpha, notes: vec![preview_tex::not_found(src.holder(), &r.texture)] } });
    }
    let name = name.trim();
    if name.is_empty() {
        return Ok(grey(preview_tex::NOT_LISTED));
    }
    Ok(match shaiya_clientdata::texture_candidates("Item/DDS", name).into_iter().find(|c| src.has(c)) {
        Some(c) => Picked { notes: vec![format!("texture {} (the name the model file holds; no list file names one); alpha: cut out", file_name(&c))], path: Some(c), alpha: AlphaMode::Cutout },
        None => grey(preview_tex::not_found(src.holder(), name)),
    })
}

fn rest_picture(vertices: &[([f32; 3], [f32; 2])], faces: &[[u16; 3]], tex: Option<(&MipImage, AlphaMode)>, view: View) -> (u32, u32, Vec<u8>) {
    let pos: Vec<[f32; 3]> = vertices.iter().map(|v| v.0).collect();
    let uv: Vec<[f32; 2]> = vertices.iter().map(|v| v.1).collect();
    match render::mesh_picture(&pos, &uv, faces, tex, view) {
        Some(p) => (p.width as u32, p.height as u32, p.rgba),
        None => (0, 0, Vec::new()),
    }
}

/// The rest picture of a model with the texture `picked` names (decoded here; an unreadable one is noted and drawn grey).
fn rest_with(src: &dyn FileSource, picked: &mut Picked, vertices: &[([f32; 3], [f32; 2])], faces: &[[u16; 3]], view: View, job: &Job) -> shaiya_clientdata::Result<(u32, u32, Vec<u8>)> {
    let mip = picked.path.as_ref().and_then(|p| preview_tex::decode_mip(src, p, job));
    job.check().map_err(|_| cancelled())?;
    if let (Some(p), None) = (&picked.path, &mip) {
        let note = preview_tex::unreadable(file_name(p));
        picked.notes.push(note);
    }
    Ok(rest_picture(vertices, faces, mip.as_ref().map(|m| (m, picked.alpha)), view))
}

/// The `.3DO` preview: the rest mesh with its `.ITM` texture; a mesh with frames plays them (textured) at its frame rate (inferred).
pub fn m3do_preview(src: &dyn FileSource, data: &[u8], path: &str, textures: &TextureIndexCache, look: &Look, job: &Job) -> shaiya_clientdata::Result<PreviewData> {
    let m = shaiya_clientdata::Mesh3do::parse(data, path)?;
    let (verts, tris) = m.counts();
    let first = format!("3DO mesh: {}, {}", n(verts, "vertex", "vertices"), n(tris, "triangle", "triangles"));
    let mut picked = pick_3do(src, path, &m.texture_name(), textures, look, job)?;
    if m.frames.is_empty() {
        let (width, height, rgba) = rest_with(src, &mut picked, &m.vertices.iter().map(|x| (x.pos, x.uv)).collect::<Vec<_>>(), &m.faces, look.view, job)?;
        return Ok(PreviewData::Model { header: first, missing: Vec::new(), see_through: Vec::new(), invisible: Vec::new(), notes: picked.notes, texture: None, animations: Vec::new(), width, height, rgba });
    }
    let positive = m.frame_rate > 0;
    let rate = if positive { m.frame_rate as f32 } else { FALLBACK_RATE };
    let second = if positive {
        format!("{} at {} per second (inferred from the file; not checked in the game)", n(m.frames.len(), "frame", "frames"), m.frame_rate)
    } else {
        format!("{}, the file's rate ({}) is not a positive number; played at {FALLBACK_RATE} frames per second", n(m.frames.len(), "frame", "frames"), m.frame_rate)
    };
    let items = [MorphItem { texture: picked.path.clone().unwrap_or_default(), faces: &m.faces, uv: m.vertices.iter().map(|x| x.uv).collect() }];
    let made = render::morph_frames_with(&items, &[picked.alpha], m.frames.len(), look.view, &mut |_, f| m.frames[f].vertices.iter().map(|x| x.0).collect(), &mut |name| image_at(src, job, name), &mut || job.check().is_err()).ok_or_else(cancelled)?;
    job.check().map_err(|_| cancelled())?;
    let mut notes = picked.notes;
    if let Some(name) = made.missing.first() {
        notes.push(preview_tex::unreadable(file_name(name)));
    }
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

/// The note every `.3DE` preview carries.
pub const EFFECT_NOTE: &str = "blend mode and flip-book come from the .EFT: drawn plain with cut-out alpha";

/// The `.3DE` preview: the mesh with its own texture from `Effect/DDS` (cut-out alpha), and the frames of the standard layout played as
/// positions (inferred) at 30 frames per second (the file stores no rate). A file in neither known layout is an error (shown as Failed
/// with its bytes).
pub fn m3de_preview(src: &dyn FileSource, data: &[u8], path: &str, textures: &TextureIndexCache, look: &Look, job: &Job) -> shaiya_clientdata::Result<PreviewData> {
    let m = shaiya_clientdata::Mesh3de::parse(data, path)?;
    let (verts, tris) = m.counts();
    let header = format!("Effect mesh: {}, {}", n(verts, "vertex", "vertices"), n(tris, "triangle", "triangles"));
    let name = m.texture_name();
    let mut picked = if !look.textured {
        grey(preview_tex::TEXTURES_OFF)
    } else if name.trim().is_empty() {
        grey("the file names no texture; drawn grey")
    } else {
        let built = textures.get_or_build(src, job).ok_or_else(cancelled)?;
        match built.0.for_effect_mesh(path, &name) {
            Some(p) => Picked { notes: Vec::new(), path: Some(p), alpha: AlphaMode::Cutout },
            None => grey(preview_tex::not_found(src.holder(), name.trim())),
        }
    };
    let mut notes = Vec::new();
    let block = match (m.layout(), &m.block) {
        (De3Layout::Standard, Some(b)) if !b.frames.is_empty() => Some(b),
        (De3Layout::Static, _) => {
            notes.push("no frame block: the file ends after the faces".to_string());
            None
        }
        _ => {
            notes.push("0 frames".to_string());
            None
        }
    };
    notes.push(EFFECT_NOTE.to_string());
    let Some(block) = block else {
        let (width, height, rgba) = rest_with(src, &mut picked, &m.vertices.iter().map(|x| (x.pos, x.uv)).collect::<Vec<_>>(), &m.faces, look.view, job)?;
        notes.extend(picked.notes);
        return Ok(PreviewData::Model { header, missing: Vec::new(), see_through: Vec::new(), invisible: Vec::new(), notes, texture: None, animations: Vec::new(), width, height, rgba });
    };
    let items = [MorphItem { texture: picked.path.clone().unwrap_or_default(), faces: &m.faces, uv: m.vertices.iter().map(|x| x.uv).collect() }];
    let made = render::morph_frames_with(&items, &[picked.alpha], block.frames.len(), look.view, &mut |_, f| block.frames[f].vertices.iter().map(|x| x.delta).collect(), &mut |name| image_at(src, job, name), &mut || job.check().is_err()).ok_or_else(cancelled)?;
    job.check().map_err(|_| cancelled())?;
    notes.extend(picked.notes);
    if let Some(name) = made.missing.first() {
        notes.push(preview_tex::unreadable(file_name(name)));
    }
    notes.extend(prepared_note(made.frames.step, "frame"));
    Ok(PreviewData::Anim {
        header: vec![header, format!("{}, played as positions (inferred) at {FALLBACK_RATE} frames per second (the file stores no rate)", n(block.frames.len(), "frame", "frames"))],
        issues: Vec::new(),
        bones: Vec::new(),
        end_key: block.frames.len().saturating_sub(1) as i32,
        frames: made.frames,
        mesh: MeshChoice::Skeleton,
        mesh_choices: Vec::new(),
        notes,
        rate: FALLBACK_RATE,
        unit: "frame",
    })
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
