//! What the Archive tab shows for one entry: a decoded texture, text, an SData summary, or hex. Built on a worker thread from the
//! entry's bytes (never on the UI thread).

use shaiya_archive::{Archive, Entry};
use shaiya_core::text::Encoding;
pub use shaiya_clientdata::meshpick::{mesh_candidates_in, MeshInfo};
use shaiya_core::{Error, Job, Result, Unit};
use std::path::Path;

pub const TEXT_FIRST: u64 = 1 << 20;
pub const TEXT_ALL: u64 = 16 << 20;
pub const HEX_MAX: u64 = 64 << 20;
pub const IMAGE_SIDE_MAX: u32 = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdataSummary {
    pub encrypted: bool,
    pub crc: Option<u32>,
    pub crc_ok: Option<bool>,
    pub plain_size: u64,
    pub padding_zero: Option<bool>,
    pub tail_zero: Option<bool>,
}

#[derive(Clone, Debug)]
pub enum PreviewData {
    Empty,
    Image { width: u32, height: u32, rgba: Vec<u8>, header: String, downscaled: bool },
    /// A `.wld` map: the grey hillshade top view of a field (`rgba` is empty for a dungeon map, which has `note` instead).
    Map { header: String, height_range: Option<(f32, f32)>, note: Option<String>, width: u32, height: u32, rgba: Vec<u8>, downscaled: bool },
    /// A `.smod` model: the textured, lit picture of the Models window of the SVMap Editor (`rgba` is empty when the model has no
    /// triangles). `header` is the counts line; the lists name the textures by what the picture does with them.
    /// A `.3DC` skinned mesh uses the same variant: its rest pose in plain grey (`missing`, `see_through`, `invisible` are empty and
    /// `notes` says what the picture is).
    Model { header: String, missing: Vec<String>, see_through: Vec<String>, invisible: Vec<String>, notes: Vec<String>, width: u32, height: u32, rgba: Vec<u8> },
    /// An `.ANI` skeletal animation with its pre-rendered frames (rendered on the worker thread; the UI only swaps one texture).
    Anim {
        header: Vec<String>,
        /// (text, is an error): one line per error code, then one weak line counting the warnings.
        issues: Vec<(String, bool)>,
        bones: Vec<AnimBoneRow>,
        end_key: i32,
        frames: shaiya_clientdata::render::AnimFrames,
        mesh: MeshChoice,
        mesh_choices: Vec<(MeshChoice, String)>,
        notes: Vec<String>,
    },
    /// The raw bytes (decoded for display with an encoding chosen by the person).
    Text { bytes: Vec<u8>, total: u64, truncated: bool, guess: Encoding },
    /// `plain` is the decrypted bytes (or the file itself when it is not encrypted).
    Sdata { summary: SdataSummary, plain: Vec<u8>, raw: Vec<u8> },
    /// `bytes` start at entry offset `base`.
    Hex { bytes: Vec<u8>, base: u64, total: u64 },
    /// A decoder said no: the message and the beginning of the bytes (hex fallback).
    Failed { message: String, bytes: Vec<u8>, total: u64 },
}

impl PreviewData {
    /// A short name for tests and the status line.
    pub fn kind(&self) -> &'static str {
        match self {
            PreviewData::Empty => "empty",
            PreviewData::Image { .. } => "image",
            PreviewData::Map { .. } => "map",
            PreviewData::Model { .. } => "model",
            PreviewData::Anim { .. } => "anim",
            PreviewData::Text { .. } => "text",
            PreviewData::Sdata { .. } => "sdata",
            PreviewData::Hex { .. } => "hex",
            PreviewData::Failed { .. } => "failed",
        }
    }
}

/// Which meshes an `.ANI` preview is drawn on.
#[derive(Clone, Debug, PartialEq)]
pub enum MeshChoice {
    /// The default parts (a character's torso, lower, hand, boots, face and hair, else every compatible mesh up to 8).
    Default,
    Skeleton,
    /// One `.3DC` by archive path.
    One(String),
}

/// One row of the Bones table of an `.ANI` preview.
#[derive(Clone, Debug)]
pub struct AnimBoneRow {
    pub index: usize,
    pub parent: i32,
    pub rot_keys: usize,
    pub trans_keys: usize,
    pub first_key: Option<i32>,
    pub last_key: Option<i32>,
}

fn ext_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// Text when under 1 % of the first 64 KiB are control bytes other than tab, CR and LF.
pub fn looks_like_text(head: &[u8]) -> bool {
    if head.is_empty() {
        return false;
    }
    if head.starts_with(&[0xFF, 0xFE]) {
        return true;
    }
    let ctl = head.iter().filter(|&&b| b < 0x20 && !matches!(b, b'\t' | b'\r' | b'\n')).count();
    ctl * 100 < head.len()
}

pub fn guess_encoding(bytes: &[u8]) -> Encoding {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        Encoding::Utf16Le
    } else if std::str::from_utf8(bytes).is_ok() || (bytes.len() > 3 && std::str::from_utf8(&bytes[..bytes.len() - 3]).is_ok()) {
        Encoding::Utf8
    } else {
        Encoding::Cp949
    }
}

/// Decodes preview text with the chosen encoding (a UTF-16 byte-order mark is skipped).
pub fn decode_text(bytes: &[u8], enc: Encoding) -> String {
    let b = if enc == Encoding::Utf16Le && bytes.starts_with(&[0xFF, 0xFE]) { &bytes[2..] } else { bytes };
    // a cut multi-byte tail would show as a replacement character; trim UTF-16 to an even length
    let b = if enc == Encoding::Utf16Le && b.len() % 2 == 1 { &b[..b.len() - 1] } else { b };
    enc.decode(b).0
}

pub fn encoding_by_name(name: &str) -> Option<Encoding> {
    Some(match name.to_ascii_lowercase().as_str() {
        "cp949" => Encoding::Cp949,
        "cp1252" => Encoding::Cp1252,
        "cp1251" => Encoding::Cp1251,
        "cp936" => Encoding::Cp936,
        "utf-8" | "utf8" => Encoding::Utf8,
        "utf-16le" | "utf16le" => Encoding::Utf16Le,
        _ => return None,
    })
}

pub fn encoding_name(e: Encoding) -> &'static str {
    match e {
        Encoding::Cp949 => "cp949",
        Encoding::Cp1252 => "cp1252",
        Encoding::Cp1251 => "cp1251",
        Encoding::Cp936 => "cp936",
        Encoding::Utf8 => "UTF-8",
        Encoding::Utf16Le => "UTF-16LE",
    }
}

fn image_header(data: &[u8], w: u32, h: u32) -> String {
    let what = if data.starts_with(b"DDS ") && data.len() >= 88 {
        let fourcc = &data[84..88];
        if fourcc.iter().all(|&b| b == 0) { "DDS uncompressed".to_string() } else { format!("DDS {}", String::from_utf8_lossy(fourcc).trim_end_matches('\0')) }
    } else if data.starts_with(b"BM") && data.len() >= 30 {
        format!("BMP {} bit", u16::from_le_bytes([data[28], data[29]]))
    } else if data.len() >= 18 {
        format!("TGA type {}, {} bit", data[2], data[16])
    } else {
        "image".to_string()
    };
    format!("{what}, {w} x {h}")
}

/// Keeps every `k`th pixel so neither side is over [`IMAGE_SIDE_MAX`].
fn shrink(w: u32, h: u32, rgba: &[u8]) -> (u32, u32, Vec<u8>) {
    let k = w.max(h).div_ceil(IMAGE_SIDE_MAX);
    if k <= 1 {
        return (w, h, rgba.to_vec());
    }
    let (nw, nh) = (w / k, h / k);
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let i = (((y * k) * w + x * k) * 4) as usize;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    (nw, nh, out)
}

/// The preview of a `.wld` map from its bytes: a field gets the grey hillshade (one pixel per corner, north up, shrunk to
/// [`IMAGE_SIDE_MAX`]) and its height range; a dungeon map has no terrain and names its `.dg` instead. A file that does not parse
/// is an error (the caller shows it as Failed).
pub fn map_preview(data: &[u8], path: &str) -> shaiya_clientdata::Result<PreviewData> {
    let wld = shaiya_clientdata::Wld::parse(data, path)?;
    let file = path.rsplit('/').next().unwrap_or(path);
    let id = file.rsplit_once('.').map_or(file, |(s, _)| s);
    Ok(match &wld.terrain {
        Some(t) => {
            let (w, h, rgba) = shaiya_clientdata::render::hillshade_grey(t);
            let (sw, sh, rgba) = shrink(w as u32, h as u32, &rgba);
            PreviewData::Map {
                header: format!("Map {id}, {} x {} corners, {} units", t.n, t.n, t.size),
                height_range: Some(t.height_range()),
                note: None,
                width: sw,
                height: sh,
                downscaled: sw != w as u32,
                rgba,
            }
        }
        None => {
            let note = if wld.dg.is_empty() { "Dungeon map: no dungeon file is named; no top view yet".to_string() } else { format!("Dungeon map: the floor is in {}; no top view yet", wld.dg) };
            PreviewData::Map { header: format!("Map {id}, dungeon"), height_range: None, note: Some(note), width: 0, height: 0, rgba: Vec::new(), downscaled: false }
        }
    })
}

/// The preview of a `.smod` model from its bytes: the picture of the SVMap Editor's Models window (`shaiya_clientdata::render`).
/// Each texture a module names is looked up in `archive` under `Entity/Texture` by the editor's rule (the name as written, else
/// the same stem as `.dds`, else as `.tga`); one that is not there or does not decode is counted as missing and drawn plain grey.
/// A file that does not parse is an error (the caller shows it as Failed).
pub fn model_preview(archive: &Archive, data: &[u8], path: &str, job: &Job) -> shaiya_clientdata::Result<PreviewData> {
    let sm = shaiya_clientdata::Smod::parse(data, path)?;
    let p = shaiya_clientdata::render::model_picture(&sm, &mut |name| {
        if job.check().is_err() {
            return None;
        }
        let found = shaiya_clientdata::texture_candidates("Entity/Texture", name).into_iter().find_map(|c| archive.index().get(&c).map(|e| (c, e)));
        let (c, e) = found?;
        shaiya_clientdata::texture::decode(&archive.read_entry(e).ok()?, &c).ok()
    });
    let n = |n: usize, one: &str, many: &str| shaiya_inspect::counted(n as u64, one, many);
    let header = format!("{}, {}, {} ({} missing)", n(p.modules, "module", "modules"), n(p.triangles, "triangle", "triangles"), n(p.textures, "texture", "textures"), p.missing.len());
    let (width, height, rgba) = match p.picture {
        Some(pic) => (pic.width as u32, pic.height as u32, pic.rgba),
        None => (0, 0, Vec::new()),
    };
    Ok(PreviewData::Model { header, missing: p.missing, see_through: p.see_through, invisible: p.invisible, notes: Vec::new(), width, height, rgba })
}

/// Model formats the client reads that this toolkit has no picture for yet (the format is not documented): they stay as hex.
pub fn undocumented_model_format(path: &str) -> bool {
    matches!(ext_of(path).as_str(), "3do")
}

/// The `.3DC` files that suit the animation at `ani_path`, by names only (no list file is read): the folder next to the `ANI` folder
/// whose name is `3DC` (any letter case); files whose stem equals the prefix or starts with the prefix and `_`. For
/// `Character/<race>/ANI/<rig>_...` the prefix is the text before the first `_`; otherwise it is the animation's stem without its last
/// `_segment`. At most 64 candidates (by name) are read with `read` (archive path to bytes); one is offered when it is a skinned
/// layout, has vertices and every bone its vertices name is below `bone_count`. Returns (compatible, default parts), both archive
/// paths in name order.
pub fn mesh_candidates(index: &shaiya_archive::Index, ani_path: &str, bone_count: usize, read: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> (Vec<String>, Vec<String>) {
    let paths: Vec<String> = index.visible().into_iter().map(|f| f.path).filter(|p| ext_of(p) == "3dc").collect();
    mesh_candidates_in(&paths, ani_path, bone_count, &mut |p| {
        let c = shaiya_clientdata::C3dc::parse(&read(p)?, p).ok()?;
        Some(MeshInfo::of(&c))
    })
}

fn kind_line(path: &str) -> String {
    let info = shaiya_clientdata::anim::rig_of(path);
    match info.rig {
        Some((rig, race)) => format!("Character animation, rig {rig} ({race})"),
        None => {
            let folder = path.rsplit_once('/').map_or("", |(d, _)| d);
            format!("Animation in {folder}")
        }
    }
}

/// The preview of an `.ANI` from its bytes: header, validation lines, bones table and the frames pre-rendered on the meshes chosen by
/// `mesh` (found in `archive` by name, see [`mesh_candidates`]). A file that does not read is an error (the caller shows it as Failed).
/// The frames are made here, on the worker thread, and the job's cancel stops them.
pub fn anim_preview(archive: &Archive, data: &[u8], path: &str, mesh: &MeshChoice, job: &Job) -> shaiya_clientdata::Result<PreviewData> {
    use shaiya_clientdata::anim::{self, Severity};
    let ani = shaiya_clientdata::Ani::parse(data, path)?;
    let (rot, trans) = ani.key_counts();
    let n = |n: usize, one: &str, many: &str| shaiya_inspect::counted(n as u64, one, many);
    let secs = ani.end_key.max(0) as f32 / 30.0;
    let mut header = vec![
        kind_line(path),
        format!(
            "{}, end key {} ({secs:.1} s at 30 keys per second), {}, {}",
            n(ani.bones.len(), "bone", "bones"),
            ani.end_key,
            n(rot, "rotation key", "rotation keys"),
            n(trans, "translation key", "translation keys")
        ),
    ];
    if ani.first_field != 0 {
        header.push(format!("first field {} (inferred: start key; kept as it is)", ani.first_field));
    }
    let found = anim::validate(&ani);
    let mut issues: Vec<(String, bool)> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for i in found.iter().filter(|i| i.severity == Severity::Error) {
        if !seen.contains(&i.code) {
            seen.push(i.code);
            let more = found.iter().filter(|j| j.code == i.code).count() - 1;
            issues.push((if more == 0 { format!("{}: {}", i.code, i.message) } else { format!("{}: {} (and {more} more)", i.code, i.message) }, true));
        }
    }
    let mut warn_codes: Vec<&str> = Vec::new();
    let mut warn_count = 0;
    for i in found.iter().filter(|i| i.severity == Severity::Warning) {
        warn_count += 1;
        if !warn_codes.contains(&i.code) {
            warn_codes.push(i.code);
        }
    }
    if warn_count > 0 {
        issues.push((format!("{}: {}", n(warn_count, "warning", "warnings"), warn_codes.join(", ")), false));
    }
    let bones: Vec<AnimBoneRow> = ani
        .bones
        .iter()
        .enumerate()
        .map(|(index, b)| AnimBoneRow {
            index,
            parent: b.parent,
            rot_keys: b.rot.len(),
            trans_keys: b.trans.len(),
            first_key: b.rot.first().map(|k| k.key).into_iter().chain(b.trans.first().map(|k| k.key)).min(),
            last_key: b.rot.last().map(|k| k.key).into_iter().chain(b.trans.last().map(|k| k.key)).max(),
        })
        .collect();

    let mut read = |p: &str| -> Option<Vec<u8>> {
        job.check().ok()?;
        archive.read_entry(archive.index().get(p)?).ok()
    };
    let (compatible, defaults) = mesh_candidates(archive.index(), path, ani.bones.len(), &mut read);
    let mesh = match mesh {
        MeshChoice::One(_) | MeshChoice::Default if compatible.is_empty() => MeshChoice::Skeleton,
        MeshChoice::One(p) if !compatible.contains(p) => MeshChoice::Default,
        other => other.clone(),
    };
    let chosen: Vec<String> = match &mesh {
        MeshChoice::Default => defaults.clone(),
        MeshChoice::Skeleton => Vec::new(),
        MeshChoice::One(p) => vec![p.clone()],
    };
    let mut parsed = Vec::new();
    for p in &chosen {
        if let Some(c) = read(p).and_then(|b| shaiya_clientdata::C3dc::parse(&b, p).ok()) {
            parsed.push(c);
        }
    }
    job.check()?;
    let meshes: Vec<&shaiya_clientdata::mesh3dc::C3dcMesh> = parsed.iter().flat_map(|c| c.meshes.iter()).collect();
    let frames = shaiya_clientdata::render::anim_frames(&ani, &meshes, &mut || job.check().is_err()).ok_or_else(|| shaiya_clientdata::Error::Archive("cancelled".into()))?;

    let mut notes = Vec::new();
    if compatible.is_empty() {
        let folder = path.rsplit_once('/').map_or("", |(d, _)| d);
        let sibling = folder.rsplit_once('/').map_or("3DC".to_string(), |(up, _)| format!("{up}/3DC"));
        notes.push(format!("no matching .3DC in {sibling}; showing the skeleton"));
    }
    if frames.unposed_vertices > 0 {
        notes.push(format!("{} name bones this animation does not have; drawn unposed", n(frames.unposed_vertices, "vertex", "vertices")));
    }
    if frames.step > 1.0 {
        notes.push(format!("the frames are prepared before playing: one every {} key units (at most {} frames)", frames.step, shaiya_clientdata::render::MAX_ANIM_FRAMES));
    }
    let mut mesh_choices = vec![(MeshChoice::Skeleton, "Skeleton only".to_string())];
    if !compatible.is_empty() {
        mesh_choices.push((MeshChoice::Default, format!("Default parts ({})", defaults.len())));
    }
    for p in &compatible {
        mesh_choices.push((MeshChoice::One(p.clone()), file_name(p).to_string()));
    }
    Ok(PreviewData::Anim { header, issues, bones, end_key: ani.end_key, frames, mesh, mesh_choices, notes })
}

/// The preview of a `.3DC` from its bytes: the first mesh at rest in plain grey with the wire. A file that does not read (or is
/// ambiguous) is an error (the caller shows it as Failed).
pub fn c3dc_preview(data: &[u8], path: &str) -> shaiya_clientdata::Result<PreviewData> {
    use shaiya_clientdata::mesh3dc::C3dcLayout;
    let c = shaiya_clientdata::C3dc::parse(data, path)?;
    let n = |n: usize, one: &str, many: &str| shaiya_inspect::counted(n as u64, one, many);
    let first = c.meshes.first();
    let (v, t) = first.map_or((0, 0), |m| (m.vertices.len(), m.faces.len()));
    let bones = n(first.map_or(0, |m| m.bones.len()), "bone", "bones");
    let header = match c.layout {
        C3dcLayout::Skinned => format!("Skinned mesh: {}, {}, {bones}", n(v, "vertex", "vertices"), n(t, "triangle", "triangles")),
        C3dcLayout::Cloak => format!("Cloak mesh without bones: {}, {}", n(v, "vertex", "vertices"), n(t, "triangle", "triangles")),
        C3dcLayout::SkinnedWithSecondMesh => {
            let second = c.meshes.get(1).map_or(0, |m| m.vertices.len());
            format!(
                "Skinned mesh: {}, {}, {bones}; a second mesh follows ({}); the picture shows the first",
                n(v, "vertex", "vertices"),
                n(t, "triangle", "triangles"),
                n(second, "vertex", "vertices")
            )
        }
    };
    let note = if c.texture_name().is_empty() {
        "shown in the mesh's own rest pose, plain grey (the mesh file names no texture)"
    } else {
        "shown in the mesh's own rest pose, plain grey (textures are not looked up for skinned meshes)"
    };
    let (width, height, rgba) = match shaiya_clientdata::render::c3dc_picture(&c) {
        Some(p) => (p.width as u32, p.height as u32, p.rgba),
        None => (0, 0, Vec::new()),
    };
    Ok(PreviewData::Model { header, missing: Vec::new(), see_through: Vec::new(), invisible: Vec::new(), notes: vec![note.to_string()], width, height, rgba })
}

fn read_all(archive: &Archive, entry: &Entry, cap: u64, job: &Job) -> Result<Vec<u8>> {
    let n = (entry.length as u64).min(cap) as usize;
    let mut out = Vec::with_capacity(n);
    job.total(Some(n as u64), Unit::Bytes);
    let mut done = 0usize;
    while done < n {
        job.check()?;
        let m = (n - done).min(1 << 20);
        out.extend_from_slice(&archive.read_range(entry, done as u64, m)?);
        job.advance(m as u64);
        done += m;
    }
    Ok(out)
}

/// Builds the preview of `entry` (`path` decides what it should be, the bytes can overrule). `text_all`: read up to 16 MiB of text
/// instead of 1 MiB. `hex_from`: where the hex window starts for entries over 64 MiB.
pub fn make_preview(archive: &Archive, entry: &Entry, path: &str, text_all: bool, hex_from: u64, job: &Job) -> Result<PreviewData> {
    make_preview_with(archive, entry, path, text_all, hex_from, &MeshChoice::Default, job)
}

/// [`make_preview`] with the mesh choice of an `.ANI` preview.
pub fn make_preview_with(archive: &Archive, entry: &Entry, path: &str, text_all: bool, hex_from: u64, mesh: &MeshChoice, job: &Job) -> Result<PreviewData> {
    if entry.length == 0 {
        return Ok(PreviewData::Empty);
    }
    let total = entry.length as u64;
    let ext = ext_of(path);
    let head = archive.read_range(entry, 0, total.min(64 << 10) as usize)?;
    let is_image = matches!(ext.as_str(), "dds" | "tga" | "bmp") || head.starts_with(b"DDS ") || (head.starts_with(b"BM") && ext == "dds");
    if is_image {
        let data = read_all(archive, entry, 128 << 20, job)?;
        return Ok(match shaiya_clientdata::texture::decode(&data, path) {
            Ok(img) => {
                let header = image_header(&data, img.width, img.height);
                let (w, h, rgba) = shrink(img.width, img.height, &img.rgba);
                PreviewData::Image { width: w, height: h, downscaled: w != img.width, rgba, header }
            }
            Err(e) => PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total },
        });
    }
    if ext == "sdata" || shaiya_seed::is_encrypted(&head) {
        let data = read_all(archive, entry, 128 << 20, job)?;
        let size = data.len() as u64;
        return Ok(match shaiya_seed::info(&data, path) {
            Ok(Some(h)) => {
                let (plain, ok) = shaiya_seed::decrypt_unchecked(&data, path)?;
                let tail_zero = {
                    // the bytes after the plaintext in the last block are part of the file; zero when the writer zero-padded
                    match shaiya_seed::unwrap(&data, path) {
                        Ok((_, shaiya_seed::Wrapper::Seed { tail, .. })) => Some(tail.iter().all(|&b| b == 0)),
                        _ => None,
                    }
                };
                PreviewData::Sdata {
                    summary: SdataSummary { encrypted: true, crc: Some(h.crc), crc_ok: Some(ok), plain_size: plain.len() as u64, padding_zero: Some(h.padding.iter().all(|&b| b == 0)), tail_zero },
                    plain,
                    raw: data,
                }
            }
            Ok(None) => PreviewData::Sdata { summary: SdataSummary { encrypted: false, crc: None, crc_ok: None, plain_size: size, padding_zero: None, tail_zero: None }, plain: data.clone(), raw: data },
            Err(e) => PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total },
        });
    }
    if ext == "wld" {
        let data = read_all(archive, entry, 128 << 20, job)?;
        return Ok(map_preview(&data, path).unwrap_or_else(|e| PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total }));
    }
    if ext == "smod" {
        let data = read_all(archive, entry, 128 << 20, job)?;
        let made = model_preview(archive, &data, path, job);
        job.check()?;
        return Ok(match made {
            Ok(p) => p,
            Err(e) => PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total },
        });
    }
    if ext == "ani" || ext == "3dc" {
        let data = read_all(archive, entry, 128 << 20, job)?;
        let made = if ext == "ani" { anim_preview(archive, &data, path, mesh, job) } else { c3dc_preview(&data, path) };
        job.check()?;
        return Ok(match made {
            Ok(p) => p,
            Err(e) => PreviewData::Failed { message: e.to_string(), bytes: data.into_iter().take(64 << 10).collect(), total },
        });
    }
    let by_ext = matches!(ext.as_str(), "txt" | "ini" | "xml" | "csv" | "cfg" | "lst" | "log");
    if !undocumented_model_format(path) && (by_ext || looks_like_text(&head)) {
        let cap = if text_all { TEXT_ALL } else { TEXT_FIRST };
        let bytes = read_all(archive, entry, cap, job)?;
        let guess = guess_encoding(&bytes);
        return Ok(PreviewData::Text { truncated: total > bytes.len() as u64, total, bytes, guess });
    }
    // hex: the whole entry up to 64 MiB, else a window of 64 MiB at `hex_from`
    if total <= HEX_MAX {
        let bytes = read_all(archive, entry, HEX_MAX, job)?;
        return Ok(PreviewData::Hex { bytes, base: 0, total });
    }
    let base = hex_from.min(total - 1) / 16 * 16;
    let n = (total - base).min(16 << 20) as usize;
    let bytes = archive.read_range(entry, base, n).map_err(|e| match e {
        Error::Cancelled => Error::Cancelled,
        other => other,
    })?;
    Ok(PreviewData::Hex { bytes, base, total })
}

/// One hex row: `00000010  64 00 00 00 ...  d...`.
pub fn hex_row(bytes: &[u8], base: u64, row: usize) -> String {
    let start = row * 16;
    let chunk = &bytes[start.min(bytes.len())..(start + 16).min(bytes.len())];
    let mut s = format!("{:08x}  ", base + start as u64);
    for i in 0..16 {
        match chunk.get(i) {
            Some(b) => s.push_str(&format!("{b:02x} ")),
            None => s.push_str("   "),
        }
        if i == 7 {
            s.push(' ');
        }
    }
    s.push(' ');
    s.extend(chunk.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }));
    s
}

pub fn hex_rows(len: usize) -> usize {
    len.div_ceil(16)
}

/// Used by the Archive tab's image drawing: a short file name.
pub fn file_name(path: &str) -> &str {
    Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::{bmp_stub, tga_stub, SahBuilder, TempDir};

    fn open(d: &TempDir, files: &[(&str, Vec<u8>)]) -> Archive {
        let mut b = SahBuilder::new(0);
        for (p, data) in files {
            b = b.file(p.as_bytes(), data);
        }
        b.write_to(d.path());
        Archive::open(d.path()).unwrap()
    }

    fn pv(a: &Archive, path: &str) -> PreviewData {
        make_preview(a, a.index().get(path).unwrap(), path, false, 0, &Job::silent()).unwrap()
    }

    #[test]
    fn every_kind_of_preview() {
        let d = TempDir::new("preview-kinds");
        let a = open(
            &d,
            &[
                ("i/a.tga", tga_stub(8, 4)),
                ("i/b.bmp", bmp_stub(5, 3)),
                ("i/bad.dds", b"not an image at all, but named dds".to_vec()),
                ("t/readme.txt", "hello, world".as_bytes().to_vec()),
                ("t/ko.ini", vec![0xB8, 0xF3, 0xBD, 0xBA, 0xC5, 0xCD, b'\n']),
                ("s/Item.SData", shaiya_seed::encrypt(b"0123456789abcdef-more")),
                ("s/Plain.SData", b"not encrypted".to_vec()),
                ("x/blob.bin", vec![0u8, 1, 2, 3, 0, 0, 7, 0, 0, 0]),
                ("x/empty.bin", vec![]),
            ],
        );
        match pv(&a, "i/a.tga") {
            PreviewData::Image { width, height, rgba, header, downscaled } => {
                assert_eq!((width, height, rgba.len(), downscaled), (8, 4, 8 * 4 * 4, false));
                assert!(header.contains("TGA type 2, 24 bit") && header.contains("8 x 4"), "{header}");
            }
            other => panic!("{other:?}"),
        }
        match pv(&a, "i/b.bmp") {
            PreviewData::Image { width, height, header, .. } => assert_eq!((width, height, header.as_str()), (5, 3, "BMP 24 bit, 5 x 3")),
            other => panic!("{other:?}"),
        }
        assert!(matches!(pv(&a, "i/bad.dds"), PreviewData::Failed { .. }), "a decode failure falls back to hex");
        match pv(&a, "t/readme.txt") {
            PreviewData::Text { bytes, guess, truncated, .. } => assert_eq!((bytes, guess, truncated), (b"hello, world".to_vec(), Encoding::Utf8, false)),
            other => panic!("{other:?}"),
        }
        match pv(&a, "t/ko.ini") {
            PreviewData::Text { bytes, guess, .. } => {
                assert_eq!(guess, Encoding::Cp949);
                assert_eq!(decode_text(&bytes, guess).trim(), "\u{BAAC}\u{C2A4}\u{D130}");
            }
            other => panic!("{other:?}"),
        }
        match pv(&a, "s/Item.SData") {
            PreviewData::Sdata { summary, plain, .. } => {
                assert_eq!((summary.encrypted, summary.crc_ok, summary.plain_size, summary.padding_zero, summary.tail_zero), (true, Some(true), 21, Some(true), Some(true)));
                assert_eq!(plain, b"0123456789abcdef-more");
            }
            other => panic!("{other:?}"),
        }
        match pv(&a, "s/Plain.SData") {
            PreviewData::Sdata { summary, .. } => assert!(!summary.encrypted && summary.plain_size == 13),
            other => panic!("{other:?}"),
        }
        match pv(&a, "x/blob.bin") {
            PreviewData::Hex { bytes, base, total } => assert_eq!((bytes.len(), base, total), (10, 0, 10)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(pv(&a, "x/empty.bin"), PreviewData::Empty));
    }

    #[test]
    fn text_detection_encodings_and_hex_rows() {
        assert!(looks_like_text(b"plain text\r\nwith lines\t"));
        assert!(!looks_like_text(&[0u8; 100]));
        assert!(!looks_like_text(&[]));
        let mut mostly = vec![b'a'; 1000];
        mostly[5] = 1;
        assert!(looks_like_text(&mostly), "under 1 % control bytes");
        mostly[6..20].fill(2);
        assert!(!looks_like_text(&mostly));
        let utf16 = [0xFF, 0xFE, b'h', 0, b'i', 0];
        assert_eq!(guess_encoding(&utf16), Encoding::Utf16Le);
        assert_eq!(decode_text(&utf16, Encoding::Utf16Le), "hi");
        assert_eq!(guess_encoding("h\u{E9}llo".as_bytes()), Encoding::Utf8);
        assert_eq!(encoding_by_name("CP949"), Some(Encoding::Cp949));
        assert_eq!(encoding_by_name("nope"), None);
        let row = hex_row(&(0u8..20).collect::<Vec<_>>(), 0x100, 1);
        assert!(row.starts_with("00000110  10 11 12 13 ") && row.ends_with("...."), "{row}");
        assert_eq!(hex_rows(33), 3);
        assert_eq!(hex_rows(0), 0);
    }

    #[test]
    fn big_images_are_shrunk_and_text_is_capped() {
        let (w, h, rgba) = shrink(8192, 100, &vec![5u8; 8192 * 100 * 4]);
        assert_eq!((w, h, rgba.len()), (4096, 50, 4096 * 50 * 4));
        assert_eq!(shrink(100, 100, &[1u8; 100 * 100 * 4]).0, 100);
        let d = TempDir::new("preview-text");
        let big = vec![b'x'; 3 << 20];
        let a = open(&d, &[("t/big.txt", big)]);
        match pv(&a, "t/big.txt") {
            PreviewData::Text { bytes, truncated, total, .. } => assert_eq!((bytes.len() as u64, truncated, total), (TEXT_FIRST, true, 3 << 20)),
            other => panic!("{other:?}"),
        }
        let e = a.index().get("t/big.txt").unwrap();
        match make_preview(&a, e, "t/big.txt", true, 0, &Job::silent()).unwrap() {
            PreviewData::Text { bytes, truncated, .. } => assert_eq!((bytes.len(), truncated), (3 << 20, false)),
            other => panic!("{other:?}"),
        }
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;
    use shaiya_testkit::{SahBuilder, TempDir};

    /// A `.wld` head followed by zeros until it parses to its last byte (every later count, name and number is 0).
    fn zero_tail(head: Vec<u8>) -> Vec<u8> {
        zero_tail_len(&head, 0)
    }

    /// The same, starting the search at `from` zeros (the length does not depend on the size of the terrain).
    fn zero_tail_len(head: &[u8], from: usize) -> Vec<u8> {
        for n in from..4096 {
            let mut b = head.to_vec();
            b.extend(std::iter::repeat_n(0u8, n));
            if shaiya_clientdata::Wld::parse(&b, "x").is_ok() {
                return b;
            }
        }
        panic!("no zero tail makes a parsable .wld");
    }

    fn tail_len() -> usize {
        let small = field_head(32);
        zero_tail_len(&small, 0).len() - small.len()
    }

    fn field_head(size: u32) -> Vec<u8> {
        let n = size as usize / 2 + 1;
        let mut h = b"FLD\0".to_vec();
        h.extend((size as i32).to_le_bytes());
        for i in 0..n * n {
            let (x, z) = ((i % n) as i32, (i / n) as i32);
            h.extend(((9000 + 20 * x - 20 * z) as i16).to_le_bytes());
        }
        h.extend(vec![0u8; n * n]);
        h.extend(0i32.to_le_bytes()); // no terrain textures
        h
    }

    /// A field with a ramp: height rises towards the east and falls towards the north.
    pub fn field_wld(size: u32) -> Vec<u8> {
        let mut h = field_head(size);
        let t = tail_len();
        h.extend(std::iter::repeat_n(0u8, t));
        assert!(shaiya_clientdata::Wld::parse(&h, "x").is_ok());
        h
    }

    pub fn dungeon_wld(dg: &str) -> Vec<u8> {
        let mut h = b"DUN\0".to_vec();
        let mut name = dg.as_bytes().to_vec();
        name.resize(256, 0);
        h.extend(name);
        zero_tail(h)
    }

    #[test]
    fn a_field_previews_as_a_map_with_its_header_and_height_range() {
        let d = TempDir::new("preview-map-field");
        SahBuilder::new(0).file(b"world/12.wld", &field_wld(32)).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        match make_preview(&a, a.index().get("world/12.wld").unwrap(), "world/12.wld", false, 0, &Job::silent()).unwrap() {
            PreviewData::Map { header, height_range, note, width, height, rgba, downscaled } => {
                assert_eq!(header, "Map 12, 17 x 17 corners, 32 units");
                assert_eq!((width, height, rgba.len(), downscaled, note), (17, 17, 17 * 17 * 4, false, None));
                let (lo, hi) = height_range.unwrap();
                assert!((lo - (9000.0 - 20.0 * 16.0) * 0.02 + 200.0).abs() < 1e-3 && (hi - (9000.0 + 20.0 * 16.0) * 0.02 + 200.0).abs() < 1e-3, "{lo} {hi}");
                assert!(rgba.chunks(4).all(|p| p[3] == 255 && p[0] == p[1] && p[1] == p[2]), "grey and opaque");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_dungeon_map_names_its_floor_file_and_has_no_picture() {
        match map_preview(&dungeon_wld("dg_cave.dg"), "world/31.wld").unwrap() {
            PreviewData::Map { header, height_range, note, rgba, .. } => {
                assert_eq!(header, "Map 31, dungeon");
                assert_eq!(note.as_deref(), Some("Dungeon map: the floor is in dg_cave.dg; no top view yet"));
                assert!(height_range.is_none() && rgba.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_wld_that_does_not_parse_is_failed_with_hex() {
        let d = TempDir::new("preview-map-bad");
        let mut bad = b"FLD\0".to_vec();
        bad.extend(7i32.to_le_bytes()); // an odd map size
        bad.extend([1u8; 40]);
        SahBuilder::new(0).file(b"world/9.wld", &bad).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        match make_preview(&a, a.index().get("world/9.wld").unwrap(), "world/9.wld", false, 0, &Job::silent()).unwrap() {
            PreviewData::Failed { message, bytes, total } => assert!(!message.is_empty() && bytes.len() == 48 && total == 48),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_largest_map_is_shrunk_to_the_image_limit() {
        match map_preview(&field_wld(8192), "world/1.wld").unwrap() {
            PreviewData::Map { header, width, height, downscaled, rgba, .. } => {
                assert_eq!(header, "Map 1, 4097 x 4097 corners, 8192 units");
                assert!(downscaled && width <= IMAGE_SIDE_MAX && height <= IMAGE_SIDE_MAX);
                assert_eq!(rgba.len(), (width * height * 4) as usize);
            }
            other => panic!("{other:?}"),
        }
    }
}

#[cfg(test)]
mod map_note_tests {
    use super::map_tests::dungeon_wld;
    use super::*;
    use shaiya_testkit::{SahBuilder, TempDir};

    #[test]
    fn a_dungeon_map_without_a_dungeon_file_says_so() {
        match map_preview(&dungeon_wld(""), "world/32.wld").unwrap() {
            PreviewData::Map { header, note, rgba, .. } => {
                assert_eq!(header, "Map 32, dungeon");
                assert_eq!(note.as_deref(), Some("Dungeon map: no dungeon file is named; no top view yet"));
                assert!(rgba.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_wld_that_does_not_parse_is_failed_whatever_its_first_bytes_are() {
        let d = TempDir::new("preview-map-bad-head");
        SahBuilder::new(0).file(b"world/1.wld", b"just some words, not a map").file(b"world/2.wld", &[0u8; 8]).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        for p in ["world/1.wld", "world/2.wld"] {
            match make_preview(&a, a.index().get(p).unwrap(), p, false, 0, &Job::silent()).unwrap() {
                PreviewData::Failed { message, bytes, total } => assert!(!message.is_empty() && bytes.len() as u64 == total, "{p}"),
                other => panic!("{p}: {other:?}"),
            }
        }
    }
}

#[cfg(test)]
mod model_tests {
    use super::*;
    use shaiya_testkit::{tga_stub, SahBuilder, TempDir};

    /// A `.smod` file: modules of (texture name, face corners as quads).
    fn smod_bytes(modules: &[(&str, Vec<[[f32; 3]; 4]>)]) -> Vec<u8> {
        let mut d = Vec::new();
        let f = |d: &mut Vec<u8>, v: f32| d.extend_from_slice(&v.to_le_bytes());
        for v in [2.0f32, 2.0, 2.0] {
            f(&mut d, v);
        }
        d.extend_from_slice(&0i32.to_le_bytes());
        for v in [0.0f32, 0.0, 0.0, 4.0, 4.0, 4.0] {
            f(&mut d, v);
        }
        d.extend_from_slice(&(modules.len() as i32).to_le_bytes());
        for (tex, quads) in modules {
            let name = format!("{tex}\0");
            d.extend_from_slice(&(name.len() as i32).to_le_bytes());
            d.extend_from_slice(name.as_bytes());
            d.extend_from_slice(&((quads.len() * 4) as i32).to_le_bytes());
            for q in quads {
                for (p, uv) in q.iter().zip([[0.0f32, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]) {
                    for v in p {
                        f(&mut d, *v);
                    }
                    for v in [0.0f32, 1.0, 0.0] {
                        f(&mut d, v);
                    }
                    d.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
                    f(&mut d, uv[0]);
                    f(&mut d, uv[1]);
                }
            }
            d.extend_from_slice(&((quads.len() * 2) as i32).to_le_bytes());
            for k in 0..quads.len() as u16 {
                for i in [0u16, 1, 2, 0, 2, 3] {
                    d.extend_from_slice(&(k * 4 + i).to_le_bytes());
                }
            }
        }
        for v in [0.0f32, 0.0, 0.0, 4.0, 4.0, 4.0] {
            f(&mut d, v);
        }
        d.extend_from_slice(&0i32.to_le_bytes());
        d
    }

    /// A 4 x 4 x 4 box in three modules: the top (texture "top.tga", stored as is), the two sides along x (named "side.dds",
    /// stored as `side.tga`) and the two sides along z (texture "gone.tga", not in the archive).
    fn box_bytes() -> Vec<u8> {
        smod_bytes(&[
            ("top.tga", vec![[[0.0, 4.0, 0.0], [4.0, 4.0, 0.0], [4.0, 4.0, 4.0], [0.0, 4.0, 4.0]]]),
            ("gone.tga", vec![[[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 4.0, 0.0], [0.0, 4.0, 0.0]], [[0.0, 0.0, 4.0], [0.0, 4.0, 4.0], [4.0, 4.0, 4.0], [4.0, 0.0, 4.0]]]),
            ("side.dds", vec![[[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [0.0, 4.0, 4.0], [0.0, 4.0, 0.0]], [[4.0, 0.0, 0.0], [4.0, 4.0, 0.0], [4.0, 4.0, 4.0], [4.0, 0.0, 4.0]]]),
        ])
    }

    #[test]
    fn a_box_model_previews_with_counts_and_names_the_missing_texture() {
        let d = TempDir::new("preview-model");
        let tga = tga_stub(8, 8);
        SahBuilder::new(0).file(b"Entity/Building/box.smod", &box_bytes()).file(b"Entity/Texture/top.tga", &tga).file(b"Entity/Texture/side.tga", &tga).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        let p = "Entity/Building/box.smod";
        match make_preview(&a, a.index().get(p).unwrap(), p, false, 0, &Job::silent()).unwrap() {
            PreviewData::Model { header, missing, see_through, invisible, width, height, rgba, .. } => {
                assert_eq!(header, "3 modules, 10 triangles, 3 textures (1 missing)");
                assert_eq!((missing, see_through.len(), invisible.len()), (vec!["gone.tga".to_string()], 0, 0), "side.dds is found as side.tga");
                assert_eq!((width, height, rgba.len()), (480, 440, 480 * 440 * 4));
                assert!(rgba.chunks(4).any(|px| px[3] == 255), "something is drawn");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_model_that_does_not_parse_is_failed_with_hex_and_other_model_formats_stay_hex() {
        let d = TempDir::new("preview-model-bad");
        SahBuilder::new(0).file(b"m/bad.smod", b"short").file(b"m/a.3DC", &[1u8, 2, 3, 0, 0, 0, 9, 9]).file(b"m/b.3do", &[7u8; 20]).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        assert!(matches!(make_preview(&a, a.index().get("m/bad.smod").unwrap(), "m/bad.smod", false, 0, &Job::silent()).unwrap(), PreviewData::Failed { .. }));
        assert!(matches!(make_preview(&a, a.index().get("m/a.3DC").unwrap(), "m/a.3DC", false, 0, &Job::silent()).unwrap(), PreviewData::Failed { .. }), "a .3DC that does not read is Failed, with hex");
        assert!(matches!(make_preview(&a, a.index().get("m/b.3do").unwrap(), "m/b.3do", false, 0, &Job::silent()).unwrap(), PreviewData::Hex { .. }));
        assert!(undocumented_model_format("m/b.3do") && !undocumented_model_format("m/a.3DC"));
        assert!(!undocumented_model_format("Entity/Building/box.smod") && !undocumented_model_format("a/b.bin"));
    }

    #[test]
    fn a_model_without_triangles_has_a_header_and_no_picture() {
        let d = TempDir::new("preview-model-empty");
        let bytes = smod_bytes(&[("top.tga", Vec::new())]);
        SahBuilder::new(0).file(b"Entity/Building/e.smod", &bytes).write_to(d.path());
        let a = Archive::open(d.path()).unwrap();
        let p = "Entity/Building/e.smod";
        match make_preview(&a, a.index().get(p).unwrap(), p, false, 0, &Job::silent()).unwrap() {
            PreviewData::Model { header, rgba, missing, .. } => assert_eq!((header.as_str(), rgba.len(), missing.len()), ("1 module, 0 triangles, 1 texture (1 missing)", 0, 1)),
            other => panic!("{other:?}"),
        }
    }
}

#[cfg(test)]
mod anim_tests {
    use super::*;
    use shaiya_core::{Cancel, Silent};
    use shaiya_testkit::anim::{ani_bytes, arm_ani, arm_mesh_3dc, c3dc_bytes, c3dc_cloak_bytes, identity_bits, translation_bits, BoneSpec, VertSpec};
    use shaiya_testkit::{SahBuilder, TempDir};

    const ANI: &str = "Character/Human/ANI/huwm_117_test.ANI";

    fn vert(bone1: u8, bone2: u8) -> VertSpec {
        VertSpec { pos: [0.0, 1.0, 0.0], weight: 1.0, bone1, bone2, pad: 0, normal: [0.0, 1.0, 0.0], uv: [0.0, 0.0] }
    }

    /// A skinned mesh whose vertices name bone 9 (the arm has 3 bones).
    fn bad_mesh() -> Vec<u8> {
        let ids = [identity_bits(), translation_bits(0.0, -1.0, 0.0), translation_bits(0.0, -2.0, 0.0)];
        c3dc_bytes(b"\0", &ids, &[vert(0, 9), vert(1, 1), vert(2, 2)], &[[0, 1, 2]])
    }

    fn open(d: &TempDir, files: &[(&str, Vec<u8>)]) -> Archive {
        let mut b = SahBuilder::new(0);
        for (p, data) in files {
            b = b.file(p.as_bytes(), data);
        }
        b.write_to(d.path());
        Archive::open(d.path()).unwrap()
    }

    fn character(d: &TempDir) -> Archive {
        open(
            d,
            &[
                (ANI, arm_ani(60)),
                ("Character/Human/3DC/huwm_torso001.3DC", arm_mesh_3dc()),
                ("Character/Human/3DC/huwm_bad001.3DC", bad_mesh()),
                ("Character/Human/3DC/elmr_torso001.3DC", arm_mesh_3dc()),
            ],
        )
    }

    fn pv(a: &Archive, path: &str, mesh: &MeshChoice) -> PreviewData {
        make_preview_with(a, a.index().get(path).unwrap(), path, false, 0, mesh, &Job::silent()).unwrap()
    }

    struct Seen {
        header: Vec<String>,
        issues: Vec<(String, bool)>,
        bones: Vec<AnimBoneRow>,
        frames: shaiya_clientdata::render::AnimFrames,
        mesh: MeshChoice,
        choices: Vec<(MeshChoice, String)>,
        notes: Vec<String>,
    }

    fn anim(p: PreviewData) -> Seen {
        match p {
            PreviewData::Anim { header, issues, bones, frames, mesh, mesh_choices, notes, .. } => Seen { header, issues, bones, frames, mesh, choices: mesh_choices, notes },
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_ani_previews_with_its_rig_and_default_parts() {
        let d = TempDir::new("w4a-rig");
        let a = character(&d);
        let p = pv(&a, ANI, &MeshChoice::Default);
        assert_eq!(p.kind(), "anim");
        let s = anim(p);
        assert_eq!(s.header[0], "Character animation, rig huwm (Human)");
        assert_eq!(s.header[1], "3 bones, end key 60 (2.0 s at 30 keys per second), 2 rotation keys, 0 translation keys");
        assert_eq!(s.header.len(), 2, "no first-field line when it is 0");
        assert!(s.issues.is_empty() && s.notes.is_empty(), "{:?} {:?}", s.issues, s.notes);
        assert_eq!(s.bones.len(), 3);
        assert_eq!((s.bones[1].parent, s.bones[1].rot_keys, s.bones[1].first_key, s.bones[1].last_key), (0, 2, Some(0), Some(60)));
        assert_eq!((s.bones[0].first_key, s.bones[0].last_key), (None, None));
        assert_eq!(s.mesh, MeshChoice::Default);
        let labels: Vec<&str> = s.choices.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, ["Skeleton only", "Default parts (1)", "huwm_torso001.3DC"], "the bad mesh and the other rig's mesh are not offered");
        let f = &s.frames;
        assert_eq!((f.frames.len(), f.bones.len(), f.width, f.height, f.step), (61, 61, 240, 220, 1.0));
        assert!(f.frames.iter().all(|px| px.len() == 240 * 220 * 4));
        assert_eq!(f.unposed_vertices, 0);
        // the candidates themselves, with the default parts
        let mut read = |p: &str| a.index().get(p).and_then(|e| a.read_entry(e).ok());
        let (all, defaults) = mesh_candidates(a.index(), ANI, 3, &mut read);
        assert_eq!(all, vec!["Character/Human/3DC/huwm_torso001.3DC".to_string()]);
        assert_eq!(defaults, all);
        // with fewer bones than the mesh names, nothing is offered
        let (none, _) = mesh_candidates(a.index(), ANI, 2, &mut read);
        assert!(none.is_empty(), "a mesh is offered only when every bone it uses exists in the animation");
    }

    #[test]
    fn skeleton_only_has_bones_and_no_pictures() {
        let d = TempDir::new("w4a-skel");
        let a = character(&d);
        let s = anim(pv(&a, ANI, &MeshChoice::Skeleton));
        assert_eq!(s.mesh, MeshChoice::Skeleton);
        assert!(s.frames.frames.is_empty() && s.notes.is_empty());
        assert_eq!((s.frames.bones.len(), s.frames.bones[0].len()), (61, 3));
        assert!(s.frames.bones[30].iter().all(|b| b[0].is_finite() && b[1].is_finite()));
    }

    #[test]
    fn one_mesh_choice_renders_only_that_mesh() {
        let d = TempDir::new("w4a-one");
        let a = character(&d);
        let torso = "Character/Human/3DC/huwm_torso001.3DC";
        let s = anim(pv(&a, ANI, &MeshChoice::One(torso.into())));
        assert_eq!(s.mesh, MeshChoice::One(torso.into()));
        assert_eq!((s.frames.frames.len(), s.frames.triangles), (61, 24), "the arm mesh has 24 triangles");
        // a mesh that is not compatible falls back to the default and is not drawn on its own
        let s = anim(pv(&a, ANI, &MeshChoice::One("Character/Human/3DC/huwm_bad001.3DC".into())));
        assert_eq!(s.mesh, MeshChoice::Default);
        assert_eq!(s.frames.triangles, 24, "the default parts are drawn instead");
    }

    #[test]
    fn a_monster_prefix_finds_its_parts() {
        let d = TempDir::new("w4a-monster");
        let arm = arm_mesh_3dc();
        let a = open(
            &d,
            &[
                ("Monster/ANI/Mob_T_01_Att1.ANI", arm_ani(30)),
                ("Monster/3DC/Mob_T_01_A.3DC", arm.clone()),
                ("Monster/3DC/Mob_T_01_B.3DC", arm.clone()),
                ("Monster/3DC/MOB_T_01.3dc", arm.clone()),
                ("Monster/3DC/Mob_T_02_A.3DC", arm.clone()),
                ("Monster/3DC/Mob_T_010_A.3DC", arm),
            ],
        );
        let mut read = |p: &str| a.index().get(p).and_then(|e| a.read_entry(e).ok());
        let (all, defaults) = mesh_candidates(a.index(), "Monster/ANI/Mob_T_01_Att1.ANI", 3, &mut read);
        let mut names: Vec<String> = all.iter().map(|p| p.rsplit('/').next().unwrap().to_ascii_lowercase()).collect();
        names.sort();
        assert_eq!(names, ["mob_t_01.3dc", "mob_t_01_a.3dc", "mob_t_01_b.3dc"], "any case; not _02 and not _010");
        assert_eq!(defaults.len(), 3, "a monster gets every compatible mesh");
        let s = anim(pv(&a, "Monster/ANI/Mob_T_01_Att1.ANI", &MeshChoice::Default));
        assert_eq!(s.header[0], "Animation in Monster/ANI");
        assert_eq!(s.frames.triangles, 3 * 24);
    }

    #[test]
    fn a_one_request_with_no_compatible_mesh_becomes_skeleton() {
        let d = TempDir::new("w4afix-none");
        let a = open(&d, &[("Monster/ANI/Mob_T_01_Att1.ANI", arm_ani(30))]);
        let s = anim(pv(&a, "Monster/ANI/Mob_T_01_Att1.ANI", &MeshChoice::One("Monster/3DC/Mob_T_01_A.3DC".into())));
        assert_eq!(s.mesh, MeshChoice::Skeleton);
        assert_eq!(s.frames.triangles, 0);
    }

    #[test]
    fn a_character_gets_the_first_part_of_each_word_in_order() {
        let d = TempDir::new("w4a-parts");
        let arm = arm_mesh_3dc();
        let a = open(
            &d,
            &[
                (ANI, arm_ani(10)),
                ("Character/Human/3DC/huwm_torso001.3DC", arm.clone()),
                ("Character/Human/3DC/huwm_torso002.3DC", arm.clone()),
                ("Character/Human/3DC/HUWM_hair001.3DC", arm.clone()),
                ("Character/Human/3DC/huwm_lower001.3DC", arm.clone()),
                ("Character/Human/3DC/huwm_cape001.3DC", arm),
            ],
        );
        let mut read = |p: &str| a.index().get(p).and_then(|e| a.read_entry(e).ok());
        let (all, defaults) = mesh_candidates(a.index(), ANI, 3, &mut read);
        assert_eq!(all.len(), 5);
        let short: Vec<&str> = defaults.iter().map(|p| p.rsplit('/').next().unwrap()).collect();
        assert_eq!(short, ["huwm_torso001.3DC", "huwm_lower001.3DC", "HUWM_hair001.3DC"], "torso, lower, hair; the cape is not a default part");
    }

    #[test]
    fn no_3dc_folder_gives_the_skeleton_note() {
        let d = TempDir::new("w4a-none");
        let a = open(&d, &[("Monster/ANI/Lonely_Att1.ANI", arm_ani(30))]);
        let s = anim(pv(&a, "Monster/ANI/Lonely_Att1.ANI", &MeshChoice::Default));
        assert_eq!(s.notes, ["no matching .3DC in Monster/3DC; showing the skeleton"]);
        assert_eq!((s.mesh, s.choices.len(), s.frames.frames.len(), s.frames.bones.len()), (MeshChoice::Skeleton, 1, 0, 31));
    }

    #[test]
    fn validation_errors_show_in_the_preview() {
        let d = TempDir::new("w4a-issues");
        let rot = |keys: &[i32]| BoneSpec { parent: -1, matrix: identity_bits(), rot: keys.iter().map(|&k| (k, [0.0, 0.0, 0.0, 1.0])).collect(), trans: Vec::new() };
        let bytes = ani_bytes(0, 30, &[rot(&[10, 5]), rot(&[1, 2, 30])]);
        let a = open(&d, &[("Monster/ANI/Bad_Att1.ANI", bytes)]);
        let s = anim(pv(&a, "Monster/ANI/Bad_Att1.ANI", &MeshChoice::Default));
        assert!(s.issues.iter().any(|(t, err)| *err && t.starts_with("keys-not-increasing")), "{:?}", s.issues);
        assert!(s.issues.iter().any(|(t, err)| !*err && t.contains("warning") && t.contains("track-starts-late") && t.contains("keys-close")), "{:?}", s.issues);
    }

    #[test]
    fn a_nonzero_first_field_is_shown_as_inferred() {
        let d = TempDir::new("w4a-first");
        let bone = BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![(-1, [0.0, 0.0, 0.0, 1.0]), (10, [0.0, 0.0, 0.0, 1.0])], trans: Vec::new() };
        let a = open(&d, &[("Monster/ANI/First_Att1.ANI", ani_bytes(-1, 10, &[bone]))]);
        let s = anim(pv(&a, "Monster/ANI/First_Att1.ANI", &MeshChoice::Default));
        assert!(s.header.iter().any(|l| l == "first field -1 (inferred: start key; kept as it is)"), "{:?}", s.header);
        assert!(s.issues.is_empty(), "tracks from key -1 are no issue: {:?}", s.issues);
    }

    #[test]
    fn a_bad_ani_is_failed_with_hex() {
        let d = TempDir::new("w4a-badani");
        let mut longer = arm_ani(30);
        longer.push(0);
        let a = open(&d, &[("Monster/ANI/Long_Att1.ANI", longer), ("Monster/ANI/Short_Att1.ANI", vec![1, 2, 3])]);
        for p in ["Monster/ANI/Long_Att1.ANI", "Monster/ANI/Short_Att1.ANI"] {
            match pv(&a, p, &MeshChoice::Default) {
                PreviewData::Failed { message, bytes, total } => assert!(!message.is_empty() && bytes.len() as u64 == total, "{p}"),
                other => panic!("{p}: {other:?}"),
            }
        }
    }

    #[test]
    fn c3dc_previews_each_layout_with_its_header() {
        let d = TempDir::new("w4a-3dc");
        let ids = [identity_bits(), translation_bits(0.0, -1.0, 0.0), translation_bits(0.0, -2.0, 0.0)];
        let tri = [vert(0, 1), VertSpec { pos: [1.0, 0.0, 0.0], ..vert(1, 2) }, VertSpec { pos: [0.0, 0.0, 1.0], ..vert(2, 2) }];
        let skinned = c3dc_bytes(b"\0", &ids, &tri, &[[0, 1, 2]]);
        let named = c3dc_bytes(b"skin.dds\0", &ids, &tri, &[[0, 1, 2]]);
        let cloak = c3dc_cloak_bytes(b"\0", &tri, &[[0, 1, 2]]);
        let two = [c3dc_bytes(b"\0", &ids, &tri, &[[0, 1, 2]]), c3dc_bytes(b"\0", &ids, &tri[..2], &[])].concat();
        let a = open(&d, &[("Monster/3DC/s.3DC", skinned), ("Monster/3DC/n.3DC", named), ("Cloak/Character/3DC/c.3DC", cloak), ("Monster/3DC/t.3DC", two)]);
        let model = |p: &str| match pv(&a, p, &MeshChoice::Default) {
            PreviewData::Model { header, notes, rgba, width, height, missing, .. } => {
                assert!(missing.is_empty());
                (header, notes, rgba.len(), (width, height))
            }
            other => panic!("{p}: {other:?}"),
        };
        let (h, notes, px, size) = model("Monster/3DC/s.3DC");
        assert_eq!(h, "Skinned mesh: 3 vertices, 1 triangle, 3 bones");
        assert_eq!(notes, ["shown in the mesh's own rest pose, plain grey (the mesh file names no texture)"]);
        assert_eq!((px, size), (480 * 440 * 4, (480, 440)), "drawn at the rest-pose scale");
        let (_, notes, _, _) = model("Monster/3DC/n.3DC");
        assert_eq!(notes, ["shown in the mesh's own rest pose, plain grey (textures are not looked up for skinned meshes)"]);
        let (h, _, px, _) = model("Cloak/Character/3DC/c.3DC");
        assert_eq!(h, "Cloak mesh without bones: 3 vertices, 1 triangle");
        assert!(px > 0);
        let (h, _, _, _) = model("Monster/3DC/t.3DC");
        assert_eq!(h, "Skinned mesh: 3 vertices, 1 triangle, 3 bones; a second mesh follows (2 vertices); the picture shows the first");
    }

    #[test]
    fn a_text_like_3do_stays_hex_with_the_note() {
        let d = TempDir::new("w4a-3do");
        let a = open(&d, &[("Item/3DO/word.3DO", b"just words, plain text, nothing binary".to_vec())]);
        assert!(matches!(pv(&a, "Item/3DO/word.3DO", &MeshChoice::Default), PreviewData::Hex { .. }), "a .3DO is never shown as Text");
        assert!(undocumented_model_format("Item/3DO/word.3DO"));
    }

    #[test]
    fn a_text_like_3dc_or_ani_is_failed_with_hex_not_text() {
        let d = TempDir::new("w4a-text");
        let words = b"just words, plain text, nothing binary".to_vec();
        let a = open(&d, &[("Monster/3DC/word.3DC", words.clone()), ("Monster/ANI/word_Att1.ANI", words)]);
        for p in ["Monster/3DC/word.3DC", "Monster/ANI/word_Att1.ANI"] {
            assert!(matches!(pv(&a, p, &MeshChoice::Default), PreviewData::Failed { .. }), "{p}");
        }
    }

    #[test]
    fn a_cancelled_anim_preview_ends_cancelled() {
        let d = TempDir::new("w4a-cancel");
        let a = character(&d);
        let cancel = Cancel::new();
        let job = Job::new(&Silent, cancel.clone());
        cancel.cancel();
        let e = make_preview_with(&a, a.index().get(ANI).unwrap(), ANI, false, 0, &MeshChoice::Default, &job).unwrap_err();
        assert!(matches!(e, Error::Cancelled), "{e:?}");
        let data = a.read_entry(a.index().get(ANI).unwrap()).unwrap();
        assert!(anim_preview(&a, &data, ANI, &MeshChoice::Default, &job).is_err());
    }

    #[test]
    fn a_long_animation_says_it_is_shown_every_few_keys() {
        let d = TempDir::new("w4a-long");
        let a = open(&d, &[("Monster/ANI/Long_Att1.ANI", arm_ani(1000))]);
        let s = anim(pv(&a, "Monster/ANI/Long_Att1.ANI", &MeshChoice::Default));
        assert_eq!((s.frames.step, s.frames.bones.len()), (5.0, 201));
        assert!(s.notes.iter().any(|n| n == "the frames are prepared before playing: one every 5 key units (at most 241 frames)"), "{:?}", s.notes);
    }
}
