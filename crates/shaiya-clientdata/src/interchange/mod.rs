//! glTF 2.0 (and later OBJ and BVH) as a lossy view of the game's model and animation files, hand-written on `serde_json`.
//!
//! The mapping: one mirror (`z` becomes `-z`, triangles swap two corners, quaternions become `(-x, -y, z, w)`, matrices `S M S` copied
//! without transposition), time `(key - start) / 30` seconds, two bone influences per vertex, names only for textures. glTF, OBJ and BVH
//! are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte. Nothing here writes a file.

pub mod gltf_read;
pub mod gltf_write;
pub mod mesh;
pub mod selfcheck;
pub mod xform;

use crate::anim::Ani;
use crate::mesh3dc::C3dc;
use crate::mesh3de::Mesh3de;
use crate::mesh3do::Mesh3do;
use mesh::Collect;

/// The format string written into every file's `asset.extras.shaiya.format`.
pub const GLTF_FORMAT: &str = "shaiya-toolkit-gltf-v1";

/// Why an export refused (a stable, short name for counting).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RefusalKind {
    NotFinite,
    NotAffine,
    Singular,
    Shear,
    Reflection,
    KeysNotIncreasing,
    Weight,
    JointRange,
    Skeleton,
    Empty,
    Index,
    CloakAnimation,
}

impl RefusalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalKind::NotFinite => "not-finite",
            RefusalKind::NotAffine => "not-affine",
            RefusalKind::Singular => "singular",
            RefusalKind::Shear => "shear",
            RefusalKind::Reflection => "reflection",
            RefusalKind::KeysNotIncreasing => "keys-not-increasing",
            RefusalKind::Weight => "weight",
            RefusalKind::JointRange => "joint-range",
            RefusalKind::Skeleton => "skeleton",
            RefusalKind::Empty => "empty",
            RefusalKind::Index => "index",
            RefusalKind::CloakAnimation => "cloak-animation",
        }
    }
}

/// One thing that cannot be written (or, in [`Report::refused`], was projected because of `--allow-lossy`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub kind: RefusalKind,
    /// The bone, vertex or file part, e.g. `huwm_000_a, bone 12`.
    pub what: String,
    pub reason: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} ({})", self.what, self.reason, self.kind.as_str())
    }
}

#[derive(Debug)]
pub enum InterchangeError {
    /// The file cannot be written as asked: every reason found (at most 200), and how many more there were.
    Refused { list: Vec<Refusal>, more: usize },
    /// The toolkit's own check of what it made failed: a bug of ours; nothing is returned.
    SelfCheck(String),
}

impl InterchangeError {
    /// True for the toolkit's own failure (exit 5), false for a file that cannot be written (exit 3).
    pub fn is_bug(&self) -> bool {
        matches!(self, InterchangeError::SelfCheck(_))
    }

    /// The kinds of the refusals, each once, in order of first appearance.
    pub fn kinds(&self) -> Vec<RefusalKind> {
        let mut out = Vec::new();
        if let InterchangeError::Refused { list, .. } = self {
            for r in list {
                if !out.contains(&r.kind) {
                    out.push(r.kind);
                }
            }
        }
        out
    }
}

impl std::fmt::Display for InterchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterchangeError::Refused { list, more } => {
                let shown: Vec<String> = list.iter().take(3).map(|r| r.to_string()).collect();
                write!(f, "cannot be written as glTF: {}", shown.join("; "))?;
                let rest = list.len().saturating_sub(3) + more;
                if rest > 0 {
                    write!(f, "; and {rest} more (--allow-lossy projects what can be projected)")?;
                }
                Ok(())
            }
            InterchangeError::SelfCheck(m) => write!(f, "the glTF made did not pass the toolkit's own check: {m}"),
        }
    }
}

impl std::error::Error for InterchangeError {}

/// What to export.
#[derive(Clone, Debug)]
pub enum GltfInput {
    /// A `.3DC` with zero or more animations (one skeleton) that move it.
    Mesh { c3dc: C3dc, anims: Vec<Ani> },
    /// An animation alone: the skeleton and the clip, no mesh.
    Anim(Ani),
    Mesh3do(Mesh3do),
    Mesh3de(Mesh3de),
}

#[derive(Clone, Debug)]
pub struct GltfOptions {
    /// Make a `.glb` (one file) instead of `.gltf` + `.bin`.
    pub binary: bool,
    /// Project what can be projected (shear, mirror, NaN placements; non-increasing keys dropped) instead of refusing the file.
    pub allow_lossy: bool,
    /// Add a key at the animation's end key to every channel that stops earlier.
    pub pad_end: bool,
    /// Write `images` and texture references as `<base>/<name>.png` (the toolkit never writes image data); `None` writes names only.
    pub texture_uri_base: Option<String>,
    /// The archive path or file name of the input (never a folder path); names the meshes and goes into the extras.
    pub source: String,
    /// The names of the animations, in order; missing ones are `Animation_NNN`.
    pub anim_names: Vec<String>,
    /// The `uri` of the buffer of a `.gltf` (the name of the `.bin` next to it).
    pub bin_name: String,
}

impl Default for GltfOptions {
    fn default() -> Self {
        GltfOptions { binary: false, allow_lossy: false, pad_end: false, texture_uri_base: None, source: String::new(), anim_names: Vec::new(), bin_name: "model.bin".to_string() }
    }
}

/// What an export did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// What `--allow-lossy` projected instead of refusing (counted in `projected`; empty without it).
    pub refused: Vec<Refusal>,
    pub projected: usize,
    /// Things left out or written differently (frames not written, a second skin), in words.
    pub notes: Vec<String>,
    pub nodes: usize,
    pub meshes: usize,
    pub vertices: usize,
    pub animations: usize,
}

#[derive(Clone, Debug)]
pub struct GltfOutput {
    /// The `.gltf` text (also made for a `.glb`; its buffer then has no `uri`).
    pub json: Vec<u8>,
    /// The buffer (the `.bin`), padded to a multiple of 4; the `.glb`'s BIN chunk holds the same bytes.
    pub bin: Vec<u8>,
    /// The complete `.glb` file, when `binary` was asked.
    pub glb: Option<Vec<u8>>,
    pub report: Report,
}

/// Writes `input` as glTF 2.0 and checks the result before returning it (structure, ranges, finite numbers, weights, quaternions, and
/// the skinned pose against the toolkit's own renderer).
pub fn export_gltf(input: GltfInput, opts: &GltfOptions) -> Result<GltfOutput, InterchangeError> {
    let mut c = Collect::new(opts.allow_lossy);
    let scene = match &input {
        GltfInput::Mesh { c3dc, anims } => mesh::scene_of_c3dc(c3dc, anims, &mut c, opts),
        GltfInput::Anim(a) => mesh::scene_of_anim(a, &mut c, opts),
        GltfInput::Mesh3do(m) => mesh::scene_of_3do(m, &mut c, opts),
        GltfInput::Mesh3de(m) => mesh::scene_of_3de(m, &mut c, opts),
    };
    if c.failed() {
        return Err(InterchangeError::Refused { list: c.refused, more: c.more });
    }
    let built = gltf_write::write(&scene, opts, &input);
    selfcheck::check_output(&built.json, &built.bin, built.glb.as_deref(), &input, c.projected == 0)?;
    let report = Report {
        refused: c.overridden,
        projected: c.projected,
        notes: scene.notes.clone(),
        nodes: scene.rig.as_ref().map_or(0, |r| r.parents.len()),
        meshes: scene.meshes.len(),
        vertices: scene.meshes.iter().map(|m| m.positions.len()).sum(),
        animations: scene.clips.len(),
    };
    Ok(GltfOutput { json: built.json, bin: built.bin, glb: built.glb, report })
}
