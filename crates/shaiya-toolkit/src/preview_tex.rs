//! Textures for the model previews: which texture a model gets from the client's list files (see [`shaiya_clientdata::TextureIndex`]),
//! how its alpha channel is used, and the decoding of the texture files. All of it runs on the preview worker; every read and decode asks
//! the job first. A texture that is missing, unreadable or not named by any list file leaves its model plain grey, and a note says why.

use crate::preview::file_name;
use crate::source::FileSource;
use shaiya_clientdata::render::{mip_image_capped, AlphaMode, MipImage};
use shaiya_clientdata::{AlphaUse, Found, TexRef, TextureIndex};
use shaiya_core::Job;
use std::collections::HashMap;

/// Texture files over this many bytes are not read.
pub const TEXTURE_CAP: u64 = 32 << 20;

/// What a preview does about textures: the "Textures" switch and which of the textures a list names for a `.3DC` is drawn (0 = the first,
/// the default; a number past the last is the last).
/// `view` is how the model is turned and zoomed ([`shaiya_clientdata::render::View::DEFAULT`] is the fixed view of 0.4.1).
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub textured: bool,
    pub texture: usize,
    pub view: shaiya_clientdata::render::View,
}

impl Default for Look {
    fn default() -> Look {
        Look { textured: true, texture: 0, view: shaiya_clientdata::render::View::DEFAULT }
    }
}

/// The texture line and the choice of a `.3DC` preview: what is drawn and what else a list names for the mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureInfo {
    /// "texture NAME from LIST (record N); ALPHA".
    pub line: String,
    /// One label per texture the lists name for the mesh, in record order.
    pub choices: Vec<String>,
    pub chosen: usize,
}

/// How a texture's alpha channel is drawn, from what the list file says about it.
pub fn alpha_mode(a: AlphaUse) -> AlphaMode {
    match a {
        AlphaUse::Glow => AlphaMode::Opaque,
        AlphaUse::Transparency | AlphaUse::Unknown => AlphaMode::Cutout,
    }
}

/// The alpha rule in words, for the texture line.
pub fn alpha_words(a: AlphaUse) -> &'static str {
    match a {
        AlphaUse::Glow => "alpha used as glow: ignored",
        AlphaUse::Transparency => "alpha used as transparency",
        AlphaUse::Unknown => "alpha: cut out (the list does not say)",
    }
}

/// Whether the "Textures" switch does anything for the file at `path`: the `.3DC`, `.ANI`, `.3DO` and `.3DE` previews take their textures from
/// the list files; a `.smod` or `.VANI` is always drawn with its own textures, so it shows no switch.
pub fn switch_applies(path: &str) -> bool {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "3dc" | "ani" | "3do" | "3de")
}

/// The note for a mesh no list file names a texture for.
pub const NOT_LISTED: &str = "no list file names a texture for this mesh; drawn grey";

/// The note when the "Textures" switch is off.
pub const TEXTURES_OFF: &str = "textures are turned off; drawn grey";

/// The note for the parts of an animation no list file names a texture for, with `{}` standing for the file name(s).
pub const NOT_LISTED_FOR_PART: &str = "no list file names a texture for {}; drawn grey";

pub fn not_in_archive(name: &str) -> String {
    format!("texture {name} is not in the archive; drawn grey")
}

pub fn unreadable(name: &str) -> String {
    format!("texture {name} could not be read; drawn grey")
}

/// The texture file at `path` decoded and capped for drawing; `None` when it is not there, does not decode or the job was cancelled
/// (the caller asks the job again to tell the last case apart).
pub fn decode_mip(src: &dyn FileSource, path: &str, job: &Job) -> Option<MipImage> {
    job.check().ok()?;
    let bytes = src.read(path, TEXTURE_CAP, &crate::source::quiet(job)).ok()?;
    job.check().ok()?;
    let img = shaiya_clientdata::texture::decode(&bytes, path).ok()?;
    Some(mip_image_capped(&img))
}

/// The textures of the meshes of an animation preview: each distinct texture file once, the alpha rule of each, and for every mesh file
/// the index of its texture (`None`: grey).
pub struct AnimTextures {
    pub textures: Vec<MipImage>,
    pub alpha: Vec<AlphaMode>,
    pub per_mesh: Vec<Option<usize>>,
    pub notes: Vec<String>,
}

/// Gives each mesh file of `mesh_paths` its default texture (the first the lists name for it). `None` when the job was cancelled.
pub fn for_meshes(src: &dyn FileSource, index: &TextureIndex, mesh_paths: &[String], job: &Job) -> Option<AnimTextures> {
    let mut out = AnimTextures { textures: Vec::new(), alpha: Vec::new(), per_mesh: Vec::new(), notes: Vec::new() };
    let mut decoded: HashMap<String, Option<usize>> = HashMap::new();
    let mut unlisted: Vec<String> = Vec::new();
    let mut bad: Vec<String> = Vec::new();
    for p in mesh_paths {
        job.check().ok()?;
        let Some(r) = index.for_mesh(p).first() else {
            unlisted.push(file_name(p).to_string());
            out.per_mesh.push(None);
            continue;
        };
        if !r.found {
            if !bad.contains(&r.texture) {
                bad.push(r.texture.clone());
                out.notes.push(not_in_archive(&r.texture));
            }
            out.per_mesh.push(None);
            continue;
        }
        let key = r.texture.to_lowercase();
        let slot = match decoded.get(&key) {
            Some(s) => *s,
            None => {
                let made = decode_mip(src, &r.texture, job);
                job.check().ok()?;
                let slot = made.map(|m| {
                    out.textures.push(m);
                    out.alpha.push(alpha_mode(r.alpha));
                    out.textures.len() - 1
                });
                if slot.is_none() {
                    out.notes.push(unreadable(file_name(&r.texture)));
                }
                decoded.insert(key, slot);
                slot
            }
        };
        out.per_mesh.push(slot);
    }
    if !unlisted.is_empty() {
        let shown: Vec<&str> = unlisted.iter().take(6).map(String::as_str).collect();
        let more = unlisted.len() - shown.len();
        out.notes.push(if more == 0 { NOT_LISTED_FOR_PART.replace("{}", &shown.join(", ")) } else { format!("no list file names a texture for {} and {more} more; drawn grey", shown.join(", ")) });
    }
    Some(out)
}

/// "texture NAME from LIST (record N); ALPHA", with " (found in another folder)" after the name when the list's own folder has no such file.
pub fn line_of(r: &TexRef) -> String {
    let elsewhere = if r.by == Found::Elsewhere { " (found in another folder)" } else { "" };
    format!("texture {}{elsewhere} from {} (record {}); {}", file_name(&r.texture), r.list, r.record, alpha_words(r.alpha))
}

/// The texture line and choices for the textures `refs` of one `.3DC`; `chosen` is already clamped.
pub fn info_of(refs: &[TexRef], chosen: usize) -> TextureInfo {
    let r = &refs[chosen];
    TextureInfo {
        line: line_of(r),
        choices: refs.iter().map(|r| format!("{} (record {})", file_name(&r.texture), r.record)).collect(),
        chosen,
    }
}
