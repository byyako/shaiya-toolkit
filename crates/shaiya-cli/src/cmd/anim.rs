//! `shaiya anim ...`: describe and check animation and model files, compare a candidate `.ANI`'s skeleton with the file it replaces, and
//! export any of the six formats to JSON and import it back (`export` and `import` write only their `-o` file).

use super::util::*;
use crate::output::print_json;
use crate::Ctx;
use serde_json::{json, Value};

use shaiya_clientdata::animedit::{self, Tracks};
use shaiya_clientdata::anim::{passes, rig_check, rig_of, same_skeleton, validate, AniIssue, RigDiff, Severity};
use shaiya_clientdata::mesh3dc::C3dcLayout;
use shaiya_clientdata::mani::{ManiValue, MANI_FIELDS, MANI_SIZE};
use shaiya_clientdata::modeljson::{self, JsonError, ModelFormat, MAX_JSON_BYTES, MAX_MODEL_BYTES};
use shaiya_clientdata::mesh3de::De3Layout;
use shaiya_clientdata::{Ani, C3dc, Mani, Mesh3de, Mesh3do, Vani};
use shaiya_core::fs::{client_folder_above, client_folder_for_new, ensure_out_folder, out_key, out_name_problem, same_file, write_atomic_checked};
use shaiya_core::glob::{Filter, Pattern};
use shaiya_core::{Error, Result};
use std::cell::Cell;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

mod gltf;
mod gltf_import;
mod objbvh;

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Describe animation and model files: .ANI (bones, keys, validation), .3DC, .VANI, .3DO and .3DE (counts, frame rates), .MANI (fields, every meaning inferred)
    ///
    /// Files or folders (folders are searched for .ani, .3dc, .vani, .mani, .3do and .3de in any letter case), or --from ARCHIVE with patterns (default: all six). `validate` and `compare` read .ANI files only.
    ///
    /// Example: shaiya anim info --from "@us" "character/human/ani/huwm_*"
    Info(InfoArgs),
    /// Check .ANI files for errors and warnings (exit 1 when a file has an error)
    ///
    /// Example: shaiya anim validate --from "@us"
    Validate(InfoArgs),
    /// Compare a candidate .ANI with the file it would replace: skeleton, first field, validation
    ///
    /// The candidate passes when it has the same skeleton (bone count, parents, reference matrices) as TARGET, the same first field
    /// (inferred to be the start key; not checked in the game) and no validation error. Exit 0 passes, 1 does not, 3 unreadable.
    ///
    /// Example: shaiya anim compare new.ani character/human/ani/huwm_117_x.ani --from "@us"
    Compare(CompareArgs),
    /// Export an animation or model file to JSON: .ANI, .3DC, .VANI, .MANI, .3DO or .3DE (the format comes from the file's extension)
    ///
    /// Writes only the -o file, as a new file (an existing one only with --force), never inside a client folder. The JSON is read back and
    /// compared with the file before anything is written; an unedited JSON imports back to the same bytes. Exit 0 written, 2 -o does not end in
    /// .json, 3 the file does not read (or is over 128 MiB), 4 the output exists, is inside a client folder or is the input, 5 a bug.
    ///
    /// With --out-dir DIR instead of -o, any number of files, folders or (with --from) patterns are exported, one `<path>.json` per model
    /// file under DIR (folders are made; a path inside the archive stays a path under DIR). DIR is checked before anything is read: inside a
    /// client folder it is exit 4. A file that does not read, or whose output exists (without --force), is reported and the rest go on; the
    /// exit code at the end is the largest of 3 (a file that did not read, or whose output name is refused: a path part Windows cannot hold or
    /// named data.sah, a `..` part), 4 (an output that exists, a second input with the same output name, or a folder that would be inside a
    /// client folder or cannot be made) and 5 (a bug), else 0. The last line says `wrote N of M (K failed)`.
    ///
    /// With --format gltf or glb the file goes out as glTF 2.0 instead (.ANI alone, .3DC with --anim ANI files, .3DO, .3DE; -o ends in .gltf and a
    /// .bin is written next to it, or ends in .glb). glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON)
    /// stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes. The result is checked
    /// by the toolkit before it is written (structure, ranges, finite numbers, weights, quaternions and the skinned pose against the toolkit's
    /// renderer; the pose is not compared when --allow-lossy projected something, and the output says so); a file that cannot be written as glTF is exit 3 and the reason names the bone or vertex, a failed check is exit 5.
    ///
    /// With --format obj the file goes out as Wavefront OBJ (a .3DO, .3DE or, at rest with no bones, .3DC; -o ends in .obj, and a .mtl of texture
    /// names is written next to it when the mesh names a texture). With --format bvh an .ANI goes out as BVH (-o ends in .bvh; one frame per key,
    /// 30 per second; a bone that moves relative to its parent is refused unless --drop-translation leaves its motion out; --scale multiplies
    /// offsets and positions). Both are a lossy view of the game file and take -o only (no --out-dir).
    ///
    /// Example: shaiya anim export character/human/3dc/huwm_torso001.3dc --from "@us" --format glb --anim character/human/ani/huwm_000_normal.ani -o torso.glb
    /// Example: shaiya anim export character/human/ani/huwm_000_normal.ani --from "@us" --format bvh -o walk.bvh
    Export(ExportArgs),
    /// Compare the poses of two .ANI files: skeleton, end key and the bones whose pose differs
    ///
    /// A is the first file, B the second; each is a file on disk or, with --from (A) and --from-b (B, default: the same archive), its path in
    /// an archive. The poses are sampled at every integer key up to the larger end key. Exit 0 when the skeletons, the end keys and the poses
    /// are identical, 1 when anything differs, 3 when a file does not read.
    ///
    /// Example: shaiya anim diff character/human/ani/huwm_117_x.ani character/human/ani/huwm_118_x.ani --from "@us"
    Diff(DiffArgs),
    /// Turn a JSON made by `export` back into a file (the format comes from the JSON, the -o extension must match)
    ///
    /// An .ANI needs --like: the file it will replace. It is written only when it passes the same check as `compare` (same skeleton, same first
    /// field, no validation error); exit 1 and nothing written otherwise. Writes only the -o file, as a new file (an existing one only with
    /// --force), never inside a client folder. Put the result into a client with Replace or `archive replace`.
    ///
    /// With IN.gltf or IN.glb (made by `export --format gltf|glb`, or by another program) the file is read as glTF 2.0 and -o must end in .ani,
    /// .3dc, .3do or .3de. An .ANI and a .3DC need --like (the file they replace; for a .3DC also the .ANI of its rig): the skeleton, the
    /// reference matrices and the first field come from it, and a node maps to a bone by its name `Bone_NNN` (any number of digits) or by a
    /// --map FILE of lines `node name = bone index`. The animation is sampled at every whole key (--keys exact uses the file's own keys,
    /// --reduce drops the keys that the others reproduce within 1e-5); a vertex with more than two influences is refused unless
    /// --reduce-influences keeps the two largest. Refused (exit 3, the reason names the node, bone or vertex): scale on a bone, morph targets,
    /// sparse or normalized or quantized data, compressed buffers, required extensions, buffers outside the file's folder, URLs, a skeleton
    /// that is not the like file's. An .ANI that does not pass the rig check is exit 1 and nothing is written. glTF, OBJ and BVH are a lossy
    /// view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks
    /// as `shaiya anim import` before it writes. Exit 2 for a bad argument (no --like for .ANI or .3DC).
    ///
    /// With IN.obj the file is read as Wavefront OBJ and -o must end in .3do or .3de (no --like): triangles and quads, negative indices, one
    /// material; missing normals are computed (and said); a missing texture coordinate, a second material or more than 65,535 vertices are
    /// refused; the .mtl named by mtllib is never opened. With IN.bvh (laid out as `export --format bvh` writes it) -o must end in .ani and
    /// --like is required: a joint is a bone by its name Bone_NNN, the channels must be the ZXY order the export writes, Frame Time 1/30 and only
    /// the root has position channels, every OFFSET must be the like file's reference translation (a --scale other than 1 or a moved Shaiya_Root is refused); a track equal to the like file's reference is left out; written only when it passes the rig check
    /// (exit 1 otherwise), exit 2 without --like. OBJ and BVH are a lossy view of the game file as well.
    ///
    /// Example: shaiya anim import wave.glb -o new.ani --like "character/human/ani/huwm_117_x.ani" --from "@us" --keys exact
    /// Example: shaiya anim import walk.bvh -o new.ani --like "character/human/ani/huwm_000_normal.ani" --from "@us"
    /// Example: shaiya anim import crate.obj -o crate.3do
    /// Example: shaiya anim import wave.json -o new.ani --like "character/human/ani/huwm_117_x.ani" --from "@us"
    Import(ImportArgs),
    /// Retime, trim or copy tracks of an .ANI into a new file (`edit retime`, `edit trim`, `edit copy-track`)
    ///
    /// Each tool reads IN (a file, or with --from its path in the archive), makes a new animation and writes it to -o only when it passes the
    /// same check as `compare` against IN (same skeleton, same first field, no validation error); exit 1 and nothing written otherwise. The
    /// output rules are those of `import`: a new file (an existing one only with --force), never inside a client folder, never IN itself
    /// (exit 4). Exit 2 for a bad argument. Validated offline; the game's playback is not tested.
    ///
    /// Example: shaiya anim edit retime character/human/ani/huwm_117_x.ani --from "@us" --factor 0.5 -o slow.ani
    Edit(EditArgs),
}

#[derive(clap::Args)]
pub struct EditArgs {
    #[command(subcommand)]
    pub tool: EditTool,
}

#[derive(clap::Subcommand)]
pub enum EditTool {
    /// Make the animation F times as long (0.1 to 10), or S seconds long: the poses are sampled at every whole key of the new range
    Retime(RetimeArgs),
    /// Keep the keys START to END (first field <= START < END <= end key) and move them to the first field
    Trim(TrimArgs),
    /// Copy the rotation and/or translation tracks of some bones from SRC into IN (both need the same skeleton and first field)
    CopyTrack(CopyTrackArgs),
}

/// What every `edit` tool takes: the input, where it is read from, the output.
#[derive(clap::Args)]
pub struct EditIo {
    /// The .ANI to edit: a file on disk, or with --from its path in the archive
    #[arg(value_name = "IN")]
    pub input: String,
    /// Read IN (and the --source file) from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// The file to write (a new file; an existing one only with --force)
    #[arg(short = 'o', long = "out", value_name = "OUT")]
    pub out: PathBuf,
    /// Replace an existing output
    #[arg(long)]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct RetimeArgs {
    #[command(flatten)]
    pub io: EditIo,
    /// The new length as a multiple of the old (0.1 to 10; 2 = twice as long)
    #[arg(long, value_name = "F", allow_hyphen_values = true, conflicts_with = "seconds", required_unless_present = "seconds")]
    pub factor: Option<f32>,
    /// The new length in seconds (30 keys are one second), instead of --factor
    #[arg(long, value_name = "S", allow_hyphen_values = true)]
    pub seconds: Option<f32>,
}

#[derive(clap::Args)]
pub struct TrimArgs {
    #[command(flatten)]
    pub io: EditIo,
    /// The first key to keep
    #[arg(long, value_name = "K", allow_hyphen_values = true)]
    pub start: i32,
    /// The last key to keep
    #[arg(long, value_name = "K", allow_hyphen_values = true)]
    pub end: i32,
}

#[derive(clap::Args)]
pub struct CopyTrackArgs {
    #[command(flatten)]
    pub io: EditIo,
    /// The .ANI the tracks come from: a file, or with --from its path in the archive
    #[arg(long, value_name = "SRC")]
    pub source: String,
    /// The bones to copy, as a comma separated list of bone numbers (0 is the first bone)
    #[arg(long, value_name = "LIST", value_delimiter = ',', conflicts_with = "all", required_unless_present = "all")]
    pub bones: Vec<usize>,
    /// Copy every bone
    #[arg(long)]
    pub all: bool,
    /// Copy the rotation tracks
    #[arg(long)]
    pub rotation: bool,
    /// Copy the translation tracks
    #[arg(long)]
    pub translation: bool,
}

#[derive(clap::Args)]
pub struct InfoArgs {
    /// Files or folders; with --from, patterns over the archive's files (default: every format `info` reads; *.ani for validate)
    #[arg(value_name = "INPUT", required_unless_present = "from")]
    pub inputs: Vec<String>,
    /// Read from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
}

#[derive(clap::Args)]
pub struct CompareArgs {
    /// The candidate .ANI, a file on disk
    #[arg(value_name = "CANDIDATE")]
    pub candidate: String,
    /// The file it would replace: a file, or with --from its path in the archive
    #[arg(value_name = "TARGET")]
    pub target: String,
    /// Read TARGET from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
}

#[derive(clap::Args)]
pub struct ExportArgs {
    /// With -o: the file to export, a file on disk or with --from its exact path in the archive (not a pattern). With --out-dir: files,
    /// folders, or with --from patterns over the archive's files
    #[arg(value_name = "INPUT", required = true)]
    pub inputs: Vec<String>,
    /// Read INPUT from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// The JSON file to write (must end in .json); one input only
    #[arg(short = 'o', long = "out", value_name = "OUT.json")]
    pub out: Option<PathBuf>,
    /// A folder to write one `<path>.json` per model file into (instead of -o); never inside a client folder
    #[arg(long, value_name = "DIR")]
    pub out_dir: Option<PathBuf>,
    /// Replace an existing output
    #[arg(long)]
    pub force: bool,
    /// json (the default: the file's own lossless JSON), gltf (a .gltf and a .bin), glb (one .glb file), obj (a .3DO, .3DE or .3DC at rest) or bvh (an .ANI); glTF, OBJ and BVH are a lossy view of the game file
    #[arg(long, value_enum, default_value = "json")]
    pub format: ExportFormat,
    /// glTF of a .3DC: an .ANI (a file, or with --from its path in the archive) that moves it; repeat for more animations of the same skeleton
    #[arg(long = "anim", value_name = "ANI")]
    pub anims: Vec<String>,
    /// glTF: project what can be projected (a sheared, mirrored or NaN placement, keys that do not increase) instead of refusing the file
    #[arg(long)]
    pub allow_lossy: bool,
    /// glTF: add a key at the animation's end key to every channel that stops earlier
    #[arg(long)]
    pub pad_end: bool,
    /// glTF: write texture references as DIR/<name>.png (names only; the toolkit never writes or embeds a picture)
    #[arg(long, value_name = "DIR")]
    pub texture_uri_base: Option<String>,
    /// BVH: leave out the motion of a bone that moves relative to its parent (a BVH keeps only constant offsets) instead of refusing the file
    #[arg(long)]
    pub drop_translation: bool,
    /// BVH: multiply every offset and position by F (100 for tools that expect centimetres); the import reads 1 only
    #[arg(long, value_name = "F", default_value_t = 1.0)]
    pub scale: f64,
}

/// What `export` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportFormat {
    Json,
    Gltf,
    Glb,
    Obj,
    Bvh,
}

#[derive(clap::Args)]
pub struct DiffArgs {
    /// The first .ANI: a file on disk, or with --from its path in the archive
    #[arg(value_name = "A")]
    pub a: String,
    /// The second .ANI: a file on disk, or with --from-b (or --from) its path in the archive
    #[arg(value_name = "B")]
    pub b: String,
    /// Read A (and B, unless --from-b is given) from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// Read B from this archive
    #[arg(long, value_name = "ARCHIVE")]
    pub from_b: Option<String>,
}

#[derive(clap::Args)]
pub struct ImportArgs {
    /// The JSON file made by `export`, a glTF file (.gltf or .glb), an OBJ (.obj) or a BVH (.bvh)
    #[arg(value_name = "IN.json|IN.gltf|IN.glb|IN.obj|IN.bvh")]
    pub input: String,
    /// The file to write; its extension must be the JSON's format (.ANI, .3DC, .VANI, .MANI, .3DO or .3DE); for a glTF input .ani, .3dc, .3do or .3de; for an OBJ .3do or .3de; for a BVH .ani
    #[arg(short = 'o', long = "out", value_name = "OUT")]
    pub out: PathBuf,
    /// For .ANI only (required there): the file the new animation will replace, a file or with --from its path in the archive
    #[arg(long, value_name = "TARGET")]
    pub like: Option<String>,
    /// Read the --like file from this archive (a client folder, a .sah file or @name)
    #[arg(long, value_name = "ARCHIVE")]
    pub from: Option<String>,
    /// Replace an existing output
    #[arg(long)]
    pub force: bool,
    /// glTF import only: a text file of lines `node name = bone index` for nodes that are not named `Bone_NNN`
    #[arg(long, value_name = "FILE")]
    pub map: Option<String>,
    /// glTF import to .3DC: a vertex with more than two bone influences keeps its two largest (renormalised) instead of refusing the file
    #[arg(long)]
    pub reduce_influences: bool,
    /// glTF import to .ANI: drop the keys that the keys around them reproduce within 1e-5
    #[arg(long)]
    pub reduce: bool,
    /// glTF import to .ANI: `exact` uses the file's own key times (each a multiple of 1/30 s) and linear values, with no resampling
    #[arg(long, value_enum, value_name = "MODE")]
    pub keys: Option<KeysMode>,
}

/// The key rule of a glTF import to .ANI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum KeysMode {
    Exact,
}

/// Why a command stopped: a usage error (exit 2), an error (the exit code of its kind) or a bug of the toolkit (exit 5).
enum Fail {
    Usage(String),
    Error(Error),
    Bug(String),
}

impl From<Error> for Fail {
    fn from(e: Error) -> Fail {
        Fail::Error(e)
    }
}

type Res = std::result::Result<i32, Fail>;

fn usage(ctx: &Ctx, message: &str) -> i32 {
    if !ctx.json || !ctx.quiet {
        eprintln!("error: {message}");
    }
    if ctx.json {
        print_json(&json!({ "error": { "kind": "usage", "message": message } }));
    }
    2
}

/// The output did not read back as it was made: a bug of ours, exit 5, nothing written (an existing output is kept).
fn internal(ctx: &Ctx, message: &str) -> i32 {
    eprintln!("error: internal error (this is a bug): {message}");
    eprintln!("hint: please report it at https://github.com/byyako/shaiya-toolkit/issues and say which command you ran");
    if ctx.json {
        print_json(&json!({ "error": { "kind": "internal", "message": message } }));
    }
    5
}

/// The exit code of a stopped command (after printing why).
fn stop(ctx: &Ctx, f: &Fail) -> i32 {
    match f {
        Fail::Usage(m) => usage(ctx, m),
        Fail::Error(e) => fail(ctx, e),
        Fail::Bug(m) => internal(ctx, m),
    }
}

/// A [`JsonError`] as the reason a command stops: the toolkit's own bug (the export's self-check) is exit 5, every problem with the input is
/// an input error (exit 3) that names `name`.
fn json_failure(name: &str, e: JsonError) -> Fail {
    if e.is_bug() {
        return Fail::Bug(e.to_string());
    }
    match e {
        JsonError::Unreadable(inner) => Fail::Error(clientdata_error(inner)),
        other => Fail::Error(Error::format(name, None, other.to_string())),
    }
}

pub fn run(ctx: &Ctx, cmd: &Cmd, job: &shaiya_core::Job) -> i32 {
    let r: Res = match cmd {
        Cmd::Info(a) => describe(ctx, a, false).map_err(Fail::from),
        Cmd::Validate(a) => describe(ctx, a, true).map_err(Fail::from),
        Cmd::Compare(a) => compare(ctx, a).map_err(Fail::from),
        Cmd::Export(a) => export(ctx, a, job),
        Cmd::Diff(a) => diff(ctx, a, job).map_err(Fail::from),
        Cmd::Import(a) => import(ctx, a),
        Cmd::Edit(a) => edit(ctx, a),
    };
    match r {
        Ok(code) => code,
        Err(f) => stop(ctx, &f),
    }
}

// ---- size caps --------------------------------------------------------------------------------------------------------------------

/// True when a file of `len` bytes is over `cap`.
fn too_large(len: u64, cap: u64) -> bool {
    len > cap
}

/// The error for a file over the cap (exit 3). It names `path` when given; `describe` prints the file's name itself.
fn cap_error(path: Option<&Path>, cap: u64) -> Error {
    let why = std::io::Error::other(format!("larger than {} MiB: not read", cap >> 20));
    match path {
        Some(p) => Error::io(p, why),
        None => Error::from(why),
    }
}

/// Reads at most `cap` bytes from `r`: `Ok(None)` when it holds more (at most `cap + 1` bytes are ever read).
fn read_bounded(r: impl Read, cap: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    r.take(cap.saturating_add(1)).read_to_end(&mut buf)?;
    Ok((!too_large(buf.len() as u64, cap)).then_some(buf))
}

/// Reads a file on disk unless it is over `cap`. The length from the metadata refuses a big file before a byte is read; the read itself
/// is bounded too, so a file that grows or reports length 0 cannot get past the cap. `name` is shown in the error when given.
fn read_capped(path: &Path, name: Option<&Path>, cap: u64) -> Result<Vec<u8>> {
    let f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let len = f.metadata().map_err(|e| Error::io(path, e))?.len();
    if too_large(len, cap) {
        return Err(cap_error(name, cap));
    }
    match read_bounded(f, cap).map_err(|e| Error::io(path, e))? {
        Some(bytes) => Ok(bytes),
        None => Err(cap_error(name, cap)),
    }
}

/// Looks up one entry of an archive by its exact path (not in the archive: exit 3) and refuses it when it is over `cap`, from the entry's length.
fn check_entry(archive: &Store, path: &str, named: bool, cap: u64) -> Result<()> {
    let length = archive.len(path).ok_or_else(|| Error::io(path, std::io::Error::new(std::io::ErrorKind::NotFound, "not in the archive")))?;
    if too_large(length, cap) {
        return Err(cap_error(named.then(|| Path::new(path)), cap));
    }
    Ok(())
}

/// Reads one entry of an archive by its exact path unless it is over `cap`.
fn read_entry_capped(archive: &Store, path: &str, named: bool, cap: u64) -> Result<Vec<u8>> {
    check_entry(archive, path, named, cap)?;
    archive.read(path)
}

// ---- inputs -----------------------------------------------------------------------------------------------------------------------

enum Origin {
    Disk(PathBuf),
    Archive,
}

struct Entry {
    name: String,
    origin: Origin,
    /// The path the entry has under an `--out-dir`: its archive path, its path inside a searched folder, or the file name of a file named
    /// on the command line (never a folder path of the machine).
    rel: String,
}

struct Inputs {
    entries: Vec<Entry>,
    archive: Option<Store>,
    /// Folders that could not be listed: (path, error). They are reported as FAILED; the other files are still described.
    failed: Vec<(String, String)>,
}

impl Inputs {
    fn read(&self, e: &Entry) -> Result<Vec<u8>> {
        match (&e.origin, &self.archive) {
            (Origin::Disk(p), _) => read_capped(p, None, MAX_MODEL_BYTES),
            (Origin::Archive, Some(a)) => read_entry_capped(a, &e.name, false, MAX_MODEL_BYTES),
            (Origin::Archive, None) => Err(Error::Refused("no archive".to_string())),
        }
    }
}

/// True when the name ends in one of `exts` (any letter case).
fn has_ext(name: &str, exts: &[&str]) -> bool {
    Path::new(name).extension().is_some_and(|e| exts.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Files below `dir`, sorted by name at each level, as (name relative to `dir` with `/`, path). Folders that are symbolic links are not followed.
/// A folder or entry that cannot be read is added to `failed` (path, error) and the walk goes on.
fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>, failed: &mut Vec<(String, String)>) {
    let listed = std::fs::read_dir(dir).and_then(|r| r.collect::<std::io::Result<Vec<_>>>());
    let mut entries = match listed {
        Ok(v) => v,
        Err(e) => {
            failed.push((dir.display().to_string(), Error::io(dir, e).to_string()));
            return;
        }
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let rel_name = if rel.is_empty() { name } else { format!("{rel}/{name}") };
        let link = match std::fs::symlink_metadata(&path) {
            Ok(l) => l,
            Err(x) => {
                failed.push((path.display().to_string(), Error::io(&path, x).to_string()));
                continue;
            }
        };
        if link.file_type().is_symlink() {
            if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                out.push((rel_name, path));
            }
        } else if link.is_dir() {
            walk(&path, &rel_name, out, failed);
        } else {
            out.push((rel_name, path));
        }
    }
}

fn collect(ctx: &Ctx, a: &InfoArgs, exts: &[&str]) -> Result<Inputs> {
    let mut entries = Vec::new();
    let mut failed = Vec::new();
    match &a.from {
        Some(arch) => {
            let archive = open_store(ctx, arch)?;
            let patterns: Vec<String> = if a.inputs.is_empty() { exts.iter().map(|e| format!("*.{e}")).collect() } else { a.inputs.clone() };
            let filter = Filter::from_args(&patterns, &[], None, None)?;
            for (fpath, flen) in archive.visible() {
                if filter.matches(&fpath, flen) && has_ext(&fpath, exts) {
                    entries.push(Entry { rel: fpath.replace('\\', "/"), name: fpath, origin: Origin::Archive });
                }
            }
            Ok(Inputs { entries, archive: Some(archive), failed: Vec::new() })
        }
        None => {
            // the pattern is only a way to say "any letter case" to the folder search
            let filter = Filter { include: exts.iter().map(|e| Pattern::new(&format!("*.{e}"))).collect::<Result<Vec<_>>>()?, ..Filter::default() };
            for i in &a.inputs {
                let p = resolve(ctx, i)?;
                if p.is_dir() {
                    let mut found = Vec::new();
                    walk(&p, "", &mut found, &mut failed);
                    for (name, path) in found {
                        if filter.matches(&name, 0) {
                            entries.push(Entry { rel: name.clone(), name, origin: Origin::Disk(path) });
                        }
                    }
                } else {
                    entries.push(Entry { rel: file_name(&p), name: i.clone(), origin: Origin::Disk(p) });
                }
            }
            Ok(Inputs { entries, archive: None, failed })
        }
    }
}

fn clientdata_error(e: shaiya_clientdata::Error) -> Error {
    match e {
        shaiya_clientdata::Error::Format { file, what, offset } => Error::format(file, Some(offset as u64), what),
        other => Error::format("animation", None, other.to_string()),
    }
}

// ---- describe ---------------------------------------------------------------------------------------------------------------------

enum Parsed {
    Ani(Ani, Vec<AniIssue>),
    C3dc(C3dc),
    Vani(Vani),
    Mani(Mani),
    Mesh3do(Mesh3do),
    Mesh3de(Mesh3de),
}

/// The formats `anim info` reads. `validate` and `compare` read `.ANI` files only.
const INFO_EXTS: [&str; 6] = ["ani", "3dc", "vani", "mani", "3do", "3de"];

/// Refuses a name that is not a file this command describes. It runs before a named file is read, so a wrong file is never read whole.
fn check_ext(name: &str, ani_only: bool) -> Result<()> {
    if ani_only {
        if has_ext(name, &["ani"]) {
            Ok(())
        } else {
            Err(Error::Refused(format!("{name}: not an .ANI file (validate and compare read .ANI files only; `shaiya anim info` also reads .3DC, .VANI, .MANI, .3DO and .3DE)")))
        }
    } else if has_ext(name, &INFO_EXTS) {
        Ok(())
    } else {
        Err(Error::Refused(format!("{name}: not an .ANI, .3DC, .VANI, .MANI, .3DO or .3DE file")))
    }
}

fn parse(name: &str, bytes: &[u8], ani_only: bool) -> Result<Parsed> {
    check_ext(name, ani_only)?;
    let e = clientdata_error;
    if has_ext(name, &["ani"]) {
        let ani = Ani::parse(bytes, name).map_err(e)?;
        let issues = validate(&ani);
        Ok(Parsed::Ani(ani, issues))
    } else if has_ext(name, &["3dc"]) {
        Ok(Parsed::C3dc(C3dc::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["vani"]) {
        Ok(Parsed::Vani(Vani::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["mani"]) {
        Ok(Parsed::Mani(Mani::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["3do"]) {
        Ok(Parsed::Mesh3do(Mesh3do::parse(bytes, name).map_err(e)?))
    } else if has_ext(name, &["3de"]) {
        Ok(Parsed::Mesh3de(Mesh3de::parse(bytes, name).map_err(e)?))
    } else {
        unreachable!("check_ext accepted a name with an extension this command does not read")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", num(n as u64), if n == 1 { one } else { many })
}

fn severity_text(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
    }
}

fn issues_json(issues: &[AniIssue]) -> Value {
    Value::Array(issues.iter().map(|i| json!({"code": i.code, "severity": severity_text(i.severity), "bone": i.bone, "message": i.message})).collect())
}

fn count_severity(issues: &[AniIssue], s: Severity) -> usize {
    issues.iter().filter(|i| i.severity == s).count()
}

/// The kind and rig an archive path says (a path on disk is not an archive path, so it says nothing).
fn kind_of(name: &str, from_archive: bool) -> Option<shaiya_clientdata::anim::RigInfo> {
    let folders = name.split(['/', '\\']).filter(|s| !s.is_empty()).count();
    (from_archive && folders > 1).then(|| rig_of(name))
}

fn ani_json(name: &str, from_archive: bool, a: &Ani, issues: &[AniIssue]) -> Value {
    let (rot, trans) = a.key_counts();
    let info = kind_of(name, from_archive);
    json!({
        "path": name,
        "format": "ani",
        "kind": info.as_ref().map(|i| i.kind.clone()),
        "rig": info.as_ref().and_then(|i| i.rig).map(|r| r.0),
        "race": info.as_ref().and_then(|i| i.rig).map(|r| r.1),
        "bones": a.bones.len(),
        "end_key": a.end_key,
        "seconds": a.end_key as f64 / 30.0,
        "rotation_keys": rot,
        "translation_keys": trans,
        "first_field": a.first_field,
        "first_field_inferred": "start key",
        "issues": issues_json(issues),
    })
}

fn ani_text(name: &str, from_archive: bool, a: &Ani, issues: &[AniIssue]) -> String {
    let (rot, trans) = a.key_counts();
    let kind = match kind_of(name, from_archive) {
        Some(i) => match i.rig {
            Some((rig, race)) => format!(", Character rig {rig} ({race})"),
            None => format!(", {}", i.kind),
        },
        None => String::new(),
    };
    let first = if a.first_field != 0 { format!(", first field {} (inferred: start key)", a.first_field) } else { String::new() };
    format!(
        "{name}: animation{kind}, {}, end key {} ({:.1} s){first}, {}, {}, {}, {}",
        plural(a.bones.len(), "bone", "bones"),
        a.end_key,
        a.end_key as f64 / 30.0,
        plural(rot, "rotation key", "rotation keys"),
        plural(trans, "translation key", "translation keys"),
        plural(count_severity(issues, Severity::Error), "error", "errors"),
        plural(count_severity(issues, Severity::Warning), "warning", "warnings"),
    )
}

fn layout_name(l: C3dcLayout) -> &'static str {
    match l {
        C3dcLayout::Skinned => "skinned",
        C3dcLayout::SkinnedWithSecondMesh => "skinned-with-second-mesh",
        C3dcLayout::Cloak => "cloak",
    }
}

fn c3dc_json(name: &str, m: &C3dc) -> Value {
    let (v, t) = m.counts();
    json!({
        "path": name,
        "format": "3dc",
        "layout": layout_name(m.layout),
        "vertices": v,
        "triangles": t,
        "bones": m.meshes.first().map_or(0, |x| x.bones.len()),
        "max_vertex_bone": m.max_vertex_bone(),
    })
}

fn c3dc_text(name: &str, m: &C3dc) -> String {
    let (v, t) = m.counts();
    let counts = format!("{}, {}", plural(v, "vertex", "vertices"), plural(t, "triangle", "triangles"));
    let bones = m.meshes.first().map_or(0, |x| x.bones.len());
    let highest = m.max_vertex_bone().map(|b| format!(", highest vertex bone {b}")).unwrap_or_default();
    match m.layout {
        C3dcLayout::Skinned => format!("{name}: skinned mesh, {counts}, {}{highest}", plural(bones, "bone", "bones")),
        C3dcLayout::SkinnedWithSecondMesh => format!("{name}: skinned mesh with a second mesh, {counts}, {}{highest}", plural(bones, "bone", "bones")),
        C3dcLayout::Cloak => format!("{name}: cloak mesh without bones, {counts}{highest}"),
    }
}

fn vani_json(name: &str, v: &Vani) -> Value {
    let (verts, tris) = v.counts();
    json!({
        "path": name,
        "format": "vani",
        "items": v.items.len(),
        "vertices_per_step": verts,
        "triangles": tris,
        "steps": v.step_count,
        "frame_ms": v.frame_ms,
        "frame_ms_inferred": "milliseconds per step",
        "textures": (0..v.items.len()).map(|k| v.texture_name(k)).collect::<Vec<_>>(),
        "collision_mesh": v.collision.is_some(),
    })
}

fn vani_text(name: &str, v: &Vani) -> String {
    let (verts, tris) = v.counts();
    let time = if v.frame_ms > 0 { format!("about {} ms per step (inferred)", v.frame_ms) } else { format!("frame_ms {} is not a positive time", v.frame_ms) };
    format!(
        "{name}: vertex-animated model, {}, {} per step, {}, {}, {time}",
        plural(v.items.len(), "item", "items"),
        plural(verts, "vertex", "vertices"),
        plural(tris, "triangle", "triangles"),
        plural(v.step_count, "step", "steps")
    )
}

fn mani_value_json(v: ManiValue) -> Value {
    // a float that is not finite has no JSON number: it is written as text (the raw bits are given as well)
    let f = |x: f32| if x.is_finite() { json!(x) } else { json!(x.to_string()) };
    match v {
        ManiValue::I32(x) => json!(x),
        ManiValue::U16(x) => json!(x),
        ManiValue::F32(x) => f(x),
        ManiValue::F32x3(x) => json!([f(x[0]), f(x[1]), f(x[2])]),
    }
}

fn mani_json(name: &str, m: &Mani) -> Value {
    let fields: Vec<Value> = m
        .fields()
        .into_iter()
        .map(|(f, v)| {
            let raw: String = m.raw[f.offset..f.offset + f.kind.size()].iter().map(|b| format!("{b:02x}")).collect();
            json!({"offset": f.offset, "name": f.name, "value": mani_value_json(v), "raw": raw, "meaning": f.meaning, "inferred": true})
        })
        .collect();
    json!({"path": name, "format": "mani", "size": MANI_SIZE, "fields": fields})
}

fn mani_text(name: &str, m: &Mani) -> String {
    let tick = match m.value(&MANI_FIELDS[0]) {
        ManiValue::I32(x) => x,
        _ => 0,
    };
    format!("{name}: building motion, {MANI_SIZE} bytes, {} (every meaning inferred), tick {tick} ms", plural(MANI_FIELDS.len(), "field", "fields"))
}

fn mesh3do_json(name: &str, m: &Mesh3do) -> Value {
    let (v, t) = m.counts();
    json!({
        "path": name,
        "format": "3do",
        "vertices": v,
        "triangles": t,
        "frames": m.frames.len(),
        "frame_rate": m.frame_rate,
        "frame_rate_inferred": "frames per second",
        "texture": m.texture_name(),
    })
}

fn mesh3do_text(name: &str, m: &Mesh3do) -> String {
    let (v, t) = m.counts();
    let frames = if m.frames.is_empty() { String::new() } else { format!(", {} at {} per second (inferred)", plural(m.frames.len(), "frame", "frames"), m.frame_rate) };
    format!("{name}: 3DO mesh, {}, {}{frames}", plural(v, "vertex", "vertices"), plural(t, "triangle", "triangles"))
}

fn mesh3de_json(name: &str, m: &Mesh3de) -> Value {
    let (v, t) = m.counts();
    json!({
        "path": name,
        "format": "3de",
        "layout": if m.layout() == De3Layout::Static { "static" } else { "standard" },
        "vertices": v,
        "triangles": t,
        "frames": m.frame_count(),
        "max_key": m.block.as_ref().map(|b| b.max_key),
        "max_key_inferred": true,
        "texture": m.texture_name(),
    })
}

fn mesh3de_text(name: &str, m: &Mesh3de) -> String {
    let (v, t) = m.counts();
    let layout = match m.layout() {
        De3Layout::Static => "static, no frame block".to_string(),
        De3Layout::Standard => plural(m.frame_count(), "frame", "frames"),
    };
    format!("{name}: effect mesh, {}, {}, {layout}", plural(v, "vertex", "vertices"), plural(t, "triangle", "triangles"))
}

fn describe(ctx: &Ctx, a: &InfoArgs, validating: bool) -> Result<i32> {
    let exts: &[&str] = if validating { &["ani"] } else { &INFO_EXTS };
    let inputs = collect(ctx, a, exts)?;
    let mut w = out();
    let mut files = Vec::new();
    let (mut unreadable, mut errors, mut warnings, mut files_with_errors) = (0usize, 0usize, 0usize, 0usize);
    for (path, why) in &inputs.failed {
        unreadable += 1;
        if !ctx.json {
            outln!(w, "{path}: FAILED: {why}");
        }
        files.push(json!({"path": path, "error": why}));
    }
    for e in &inputs.entries {
        let parsed = check_ext(&e.name, validating).and_then(|()| inputs.read(e)).and_then(|b| parse(&e.name, &b, validating));
        match parsed {
            Err(err) => {
                unreadable += 1;
                if !ctx.json {
                    outln!(w, "{}: FAILED: {}", e.name, err);
                }
                files.push(json!({"path": e.name, "error": err.to_string()}));
            }
            Ok(Parsed::Ani(ani, issues)) => {
                let (ne, nw) = (count_severity(&issues, Severity::Error), count_severity(&issues, Severity::Warning));
                errors += ne;
                warnings += nw;
                files_with_errors += usize::from(ne > 0);
                if validating {
                    for i in &issues {
                        let line = format!("{}: {}: {}", e.name, i.code, i.message);
                        if i.severity == Severity::Error {
                            eprintln!("error: {line}");
                        } else if !ctx.quiet {
                            eprintln!("warning: {line}");
                        }
                    }
                } else if !ctx.json {
                    outln!(w, "{}", ani_text(&e.name, matches!(e.origin, Origin::Archive), &ani, &issues));
                }
                files.push(ani_json(&e.name, matches!(e.origin, Origin::Archive), &ani, &issues));
            }
            Ok(Parsed::C3dc(m)) => {
                if !ctx.json {
                    outln!(w, "{}", c3dc_text(&e.name, &m));
                }
                files.push(c3dc_json(&e.name, &m));
            }
            Ok(Parsed::Vani(m)) => {
                if !ctx.json {
                    outln!(w, "{}", vani_text(&e.name, &m));
                }
                files.push(vani_json(&e.name, &m));
            }
            Ok(Parsed::Mani(m)) => {
                if !ctx.json {
                    outln!(w, "{}", mani_text(&e.name, &m));
                }
                files.push(mani_json(&e.name, &m));
            }
            Ok(Parsed::Mesh3do(m)) => {
                if !ctx.json {
                    outln!(w, "{}", mesh3do_text(&e.name, &m));
                }
                files.push(mesh3do_json(&e.name, &m));
            }
            Ok(Parsed::Mesh3de(m)) => {
                if !ctx.json {
                    outln!(w, "{}", mesh3de_text(&e.name, &m));
                }
                files.push(mesh3de_json(&e.name, &m));
            }
        }
    }
    let _ = w.flush();
    drop(w);
    if inputs.entries.is_empty() && inputs.failed.is_empty() {
        note(ctx, if validating { "note: no .ANI files found" } else { "note: no .ANI, .3DC, .VANI, .MANI, .3DO or .3DE files found" });
    }
    if validating {
        note(
            ctx,
            &format!("{}: {} in {}, {}, {} could not be read", count(files.len(), "file"), plural(errors, "error", "errors"), plural(files_with_errors, "file", "files"), plural(warnings, "warning", "warnings"), unreadable),
        );
    }
    if ctx.json {
        let mut doc = json!({ "files": files });
        if validating {
            doc["errors"] = json!(errors);
            doc["warnings"] = json!(warnings);
        }
        print_json(&doc);
    }
    Ok(if unreadable > 0 {
        3
    } else if validating && errors > 0 {
        1
    } else {
        0
    })
}

// ---- compare ----------------------------------------------------------------------------------------------------------------------

fn read_ani(name: &str, bytes: &[u8]) -> Result<Ani> {
    Ani::parse(bytes, name).map_err(clientdata_error)
}

fn diff_json(d: &RigDiff) -> Value {
    match d {
        RigDiff::BoneCount { candidate, target } => json!({"kind": "bone_count", "skeleton": true, "bone": null, "candidate": candidate, "target": target}),
        RigDiff::Parent { bone, candidate, target } => json!({"kind": "parent", "skeleton": true, "bone": bone, "candidate": candidate, "target": target}),
        RigDiff::Matrix { bone } => json!({"kind": "matrix", "skeleton": true, "bone": bone, "candidate": null, "target": null}),
        RigDiff::FirstField { candidate, target } => json!({"kind": "first_field", "skeleton": false, "bone": null, "candidate": candidate, "target": target, "inferred": "start key"}),
    }
}

/// How many differences of one kind the text lists before it says "and N more".
const LISTED: usize = 10;

fn compare(ctx: &Ctx, a: &CompareArgs) -> Result<i32> {
    // only .ANI files are compared; a name with another extension is refused before anything is read (FAILED, exit 3, like `validate`)
    for name in [&a.candidate, &a.target] {
        if let Err(e) = check_ext(name, true) {
            let why = e.to_string();
            if ctx.json {
                print_json(&json!({"candidate": a.candidate, "target": a.target, "error": why}));
            } else {
                eprintln!("{name}: FAILED: {why}");
            }
            return Ok(3);
        }
    }
    let cand_path = resolve(ctx, &a.candidate)?;
    let cand_bytes = read_capped(&cand_path, Some(&cand_path), MAX_MODEL_BYTES)?;
    let candidate = read_ani(&a.candidate, &cand_bytes)?;
    let target_bytes = read_target(ctx, &a.target, a.from.as_deref())?;
    let target = read_ani(&a.target, &target_bytes)?;

    let rig = RigResult::new(&candidate, &target);
    if ctx.json {
        print_json(&rig.json(&a.candidate, &a.target));
        return Ok(i32::from(!rig.ok));
    }
    let mut w = out();
    rig.print(&mut w, &a.target, &candidate, &target);
    let _ = w.flush();
    Ok(i32::from(!rig.ok))
}

/// TARGET of `compare` and `--like` of `import`: a file on disk, or with `from` its exact path in the archive (over the size cap: exit 3).
fn read_target(ctx: &Ctx, target: &str, from: Option<&str>) -> Result<Vec<u8>> {
    match from {
        Some(arch) => {
            let archive = open_store(ctx, arch)?;
            read_entry_capped(&archive, target, true, MAX_MODEL_BYTES)
        }
        None => {
            let p = resolve(ctx, target)?;
            read_capped(&p, Some(&p), MAX_MODEL_BYTES)
        }
    }
}

/// What the rig check says about a candidate `.ANI` and the file it would replace: shared by `compare` and `import`, so both print the same lines.
struct RigResult {
    diffs: Vec<RigDiff>,
    issues: Vec<AniIssue>,
    ok: bool,
}

impl RigResult {
    fn new(candidate: &Ani, target: &Ani) -> RigResult {
        RigResult { diffs: rig_check(candidate, target), issues: validate(candidate), ok: passes(candidate, target) }
    }

    /// The keys of `compare`'s JSON (`import` adds its own).
    fn json(&self, candidate: &str, target: &str) -> Value {
        json!({
            "candidate": candidate,
            "target": target,
            "same_skeleton": same_skeleton(&self.diffs),
            "same_first_field": !self.diffs.iter().any(|d| !d.is_skeleton()),
            "passes": self.ok,
            "differences": self.diffs.iter().map(diff_json).collect::<Vec<_>>(),
            "candidate_issues": issues_json(&self.issues),
        })
    }

    /// The text of `compare`: the skeleton lines, the first-field line, the candidate's validation errors, and `passes` or `does not pass`.
    fn print(&self, w: &mut impl Write, target_name: &str, candidate: &Ani, target: &Ani) {
        print_rig_result(w, target_name, candidate, target, &self.diffs, &self.issues, self.ok);
    }
}

fn print_rig_result(w: &mut impl Write, target_name: &str, candidate: &Ani, target: &Ani, diffs: &[RigDiff], issues: &[AniIssue], ok: bool) {
    let errors: Vec<&AniIssue> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
    if same_skeleton(diffs) {
        outln!(w, "same skeleton as {}: {}, parents and reference matrices identical", target_name, plural(target.bones.len(), "bone", "bones"));
    } else {
        outln!(w, "the skeleton differs from {}:", target_name);
        let counts: Vec<&RigDiff> = diffs.iter().filter(|d| matches!(d, RigDiff::BoneCount { .. })).collect();
        let parents: Vec<&RigDiff> = diffs.iter().filter(|d| matches!(d, RigDiff::Parent { .. })).collect();
        let matrices: Vec<&RigDiff> = diffs.iter().filter(|d| matches!(d, RigDiff::Matrix { .. })).collect();
        for (list, what) in [(counts, "bone count"), (parents, "parent"), (matrices, "reference matrix")] {
            for d in list.iter().take(LISTED) {
                match d {
                    RigDiff::BoneCount { candidate, target } => outln!(w, "  bone count: candidate {candidate}, target {target}"),
                    RigDiff::Parent { bone, candidate, target } => outln!(w, "  bone {bone}: parent is {candidate}, target {target}"),
                    RigDiff::Matrix { bone } => outln!(w, "  bone {bone}: reference matrix differs"),
                    RigDiff::FirstField { .. } => {}
                }
            }
            if list.len() > LISTED {
                outln!(w, "  and {} more {what} differences", num((list.len() - LISTED) as u64));
            }
        }
    }
    match diffs.iter().find_map(|d| if let RigDiff::FirstField { candidate, target } = d { Some((*candidate, *target)) } else { None }) {
        Some((c, t)) => outln!(w, "first field differs: candidate {c}, target {t} (inferred: the start key; not checked in the game)"),
        None => outln!(w, "first field: same ({})", candidate.first_field),
    }
    if !errors.is_empty() {
        outln!(w, "the candidate has {}:", plural(errors.len(), "validation error", "validation errors"));
        for i in &errors {
            outln!(w, "  {}: {}", i.code, i.message);
        }
    }
    outln!(w, "{}", if ok { "passes" } else { "does not pass" });
}

// ---- diff -------------------------------------------------------------------------------------------------------------------------

/// How many bones the text of `diff` lists before it says "and N more".
const BONES_LISTED: usize = 20;

fn diff_side(ctx: &Ctx, name: &str, from: Option<&str>) -> Result<Ani> {
    check_ext(name, true)?;
    let bytes = read_target(ctx, name, from)?;
    read_ani(name, &bytes)
}

fn diff(ctx: &Ctx, a: &DiffArgs, job: &shaiya_core::Job) -> Result<i32> {
    let from_b = a.from_b.as_deref().or(a.from.as_deref());
    let (ani_a, ani_b) = match (diff_side(ctx, &a.a, a.from.as_deref()), diff_side(ctx, &a.b, from_b)) {
        (Ok(x), Ok(y)) => (x, y),
        (x, y) => {
            // both problems are told; the exit code is the first one's
            let errors: Vec<Error> = [x.err(), y.err()].into_iter().flatten().collect();
            if ctx.json {
                print_json(&json!({"a": a.a, "b": a.b, "error": errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ")}));
            }
            for e in &errors {
                eprintln!("error: {e}");
            }
            return Ok(3);
        }
    };
    let d = shaiya_clientdata::anim::diff_with(&ani_a, &ani_b, &mut || job.check().is_err()).ok_or(Error::Cancelled)?;
    if ctx.json {
        let bones: Vec<Value> = d
            .bones
            .iter()
            .map(|b| json!({"bone": b.bone, "max_angle_deg": b.max_angle_deg, "max_offset": b.max_offset, "keys": {"a": b.keys.0, "b": b.keys.1}}))
            .collect();
        print_json(&json!({
            "a": a.a,
            "b": a.b,
            "identical": d.is_identical(),
            "skeleton": d.skeleton.iter().map(diff_json).collect::<Vec<_>>(),
            "end_key": {"a": d.end_key.0, "b": d.end_key.1},
            "bones": bones,
        }));
        return Ok(i32::from(!d.is_identical()));
    }
    let mut w = out();
    if d.is_identical() {
        outln!(w, "identical: same skeleton, end key {}, and the same poses at every key", d.end_key.0);
        let _ = w.flush();
        return Ok(0);
    }
    type Is = fn(&RigDiff) -> bool;
    let kinds: [(&str, Is); 3] =
        [("bone count", |d| matches!(d, RigDiff::BoneCount { .. })), ("parent", |d| matches!(d, RigDiff::Parent { .. })), ("reference matrix", |d| matches!(d, RigDiff::Matrix { .. }))];
    if !same_skeleton(&d.skeleton) {
        outln!(w, "the skeletons differ:");
    }
    for (what, is) in kinds {
        let list: Vec<&RigDiff> = d.skeleton.iter().filter(|x| is(x)).collect();
        for x in list.iter().take(LISTED) {
            match x {
                RigDiff::BoneCount { candidate, target } => outln!(w, "  bone count: A {candidate}, B {target}"),
                RigDiff::Parent { bone, candidate, target } => outln!(w, "  bone {bone}: parent is {candidate} in A, {target} in B"),
                RigDiff::Matrix { bone } => outln!(w, "  bone {bone}: reference matrix differs"),
                RigDiff::FirstField { .. } => {}
            }
        }
        if list.len() > LISTED {
            outln!(w, "  and {} more {what} differences", num((list.len() - LISTED) as u64));
        }
    }
    if let Some((x, y)) = d.skeleton.iter().find_map(|x| if let RigDiff::FirstField { candidate, target } = x { Some((*candidate, *target)) } else { None }) {
        outln!(w, "first field differs: A {x}, B {y} (inferred: the start key; not checked in the game)");
    }
    if d.end_key.0 != d.end_key.1 {
        outln!(w, "end key differs: A {}, B {}", d.end_key.0, d.end_key.1);
    }
    if d.bones.is_empty() {
        outln!(w, "no bone pose differs");
    } else {
        outln!(w, "{} differ in pose (largest turn first):", plural(d.bones.len(), "bone", "bones"));
        for b in d.bones.iter().take(BONES_LISTED) {
            outln!(w, "  bone {}: up to {:.2} degrees, {:.3} units ({} keys in A, {} in B)", b.bone, b.max_angle_deg, b.max_offset, b.keys.0, b.keys.1);
        }
        if d.bones.len() > BONES_LISTED {
            outln!(w, "  and {} more", num((d.bones.len() - BONES_LISTED) as u64));
        }
    }
    let _ = w.flush();
    Ok(1)
}

// ---- export and import ------------------------------------------------------------------------------------------------------------

const MODEL_NAMES: &str = ".ANI, .3DC, .VANI, .MANI, .3DO or .3DE";

/// The checks every `-o` of `export` and `import` goes through, in this order: the folder exists (exit 3), `out` is not one of `inputs`
/// (exit 4), `out` does not exist unless `--force` (exit 4), `out` is not inside a client folder (exit 4: a folder holding `data.sah`, or one of
/// the saved client folders, above it or through a link).
fn check_output(ctx: &Ctx, out: &Path, force: bool, inputs: &[&Path]) -> Result<()> {
    let folder = match out.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    if !folder.is_dir() {
        return Err(Error::io(&folder, std::io::Error::new(std::io::ErrorKind::NotFound, "folder does not exist")));
    }
    if let Some(i) = inputs.iter().find(|i| same_file(i, out)) {
        return Err(Error::Refused(format!("{} is the input {}: write the output to another file", out.display(), i.display())));
    }
    if out.exists() && !force {
        return Err(Error::Refused(format!("{} exists; use --force to replace it", out.display())));
    }
    let saved: Vec<PathBuf> = crate::config::load_clients(ctx.data_dir.as_deref())?.into_iter().map(|c| c.path).collect();
    if let Some(client) = client_folder_above(out, &saved) {
        return Err(Error::Refused(format!(
            "{} is inside the client folder {}: write it elsewhere and put it in with Replace or archive replace",
            out.display(),
            client.display()
        )));
    }
    Ok(())
}

/// The check of `write_atomic_checked`: the temp file read back from disk must hold exactly `want`. A difference sets `bug` (exit 5).
fn same_bytes_check<'a>(want: &'a [u8], bug: &'a Cell<bool>) -> impl Fn(&Path) -> Result<()> + 'a {
    move |temp| {
        let back = std::fs::read(temp).map_err(|e| Error::io(temp, e))?;
        if back == want {
            Ok(())
        } else {
            bug.set(true);
            Err(Error::Refused("the file written did not read back the same".to_string()))
        }
    }
}

fn export(ctx: &Ctx, a: &ExportArgs, job: &shaiya_core::Job) -> Res {
    let interchange = matches!(a.format, ExportFormat::Gltf | ExportFormat::Glb);
    if !interchange && (a.allow_lossy || a.pad_end || a.texture_uri_base.is_some()) {
        return Err(Fail::Usage("--allow-lossy, --pad-end and --texture-uri-base need --format gltf or glb".to_string()));
    }
    if !interchange && !a.anims.is_empty() {
        return Err(Fail::Usage("--anim needs --format gltf or glb".to_string()));
    }
    if a.format != ExportFormat::Bvh && (a.drop_translation || a.scale != 1.0) {
        return Err(Fail::Usage("--drop-translation and --scale need --format bvh".to_string()));
    }
    match a.format {
        ExportFormat::Gltf | ExportFormat::Glb => return gltf::run(ctx, a, job),
        ExportFormat::Obj | ExportFormat::Bvh => return objbvh::run(ctx, a),
        ExportFormat::Json => {}
    }
    match (&a.out, &a.out_dir) {
        (Some(_), Some(_)) => Err(Fail::Usage("-o and --out-dir cannot be used together".to_string())),
        (None, None) => Err(Fail::Usage("give -o OUT.json (one input) or --out-dir DIR".to_string())),
        (Some(out), None) => match a.inputs.as_slice() {
            [input] => export_one(ctx, a, input, out),
            _ => Err(Fail::Usage("more than one input needs --out-dir (-o names one file)".to_string())),
        },
        (None, Some(dir)) => export_dir(ctx, a, dir, job),
    }
}

/// Why one file of an `--out-dir` export did not get written; the kind decides the exit code at the end (3, 4 or 5).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Why {
    /// The file did not read, is not a model file, or its output name is refused: exit 3.
    Input,
    /// The output exists (without --force), is a second output of the same name, would land inside a client folder or its folder cannot be
    /// made: exit 4.
    Safety,
    /// The self-check failed (a bug of ours): exit 5.
    Bug,
}

impl Why {
    fn code(self) -> i32 {
        match self {
            Why::Input => 3,
            Why::Safety => 4,
            Why::Bug => 5,
        }
    }
}

/// The path of the JSON of `rel` under `dir` (`rel` is already checked by `loose_name_problem`).
fn json_target(dir: &Path, rel: &str) -> PathBuf {
    let mut p = dir.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
    }
    let mut name = p.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".json");
    p.set_file_name(name);
    p
}

fn export_dir(ctx: &Ctx, a: &ExportArgs, dir: &Path, job: &shaiya_core::Job) -> Res {
    let saved: Vec<PathBuf> = crate::config::load_clients(ctx.data_dir.as_deref())?.into_iter().map(|c| c.path).collect();
    // before anything is read: DIR must not be (inside) a client folder, and must be a folder or not exist yet
    if let Some(client) = client_folder_for_new(dir, &saved) {
        return Err(Error::Refused(format!("{} is inside the client folder {}: write it elsewhere and put it in with Replace or archive replace", dir.display(), client.display())).into());
    }
    if dir.exists() && !dir.is_dir() {
        return Err(Error::io(dir, std::io::Error::new(std::io::ErrorKind::AlreadyExists, "not a folder")).into());
    }
    let inputs = collect(ctx, &InfoArgs { inputs: a.inputs.clone(), from: a.from.clone() }, &INFO_EXTS)?;
    job.total(Some(inputs.entries.len() as u64), shaiya_core::Unit::Files);
    let mut written: Vec<String> = Vec::new();
    let mut failed: Vec<(String, String, Why)> = inputs.failed.iter().map(|(p, why)| (p.clone(), why.clone(), Why::Input)).collect();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut folders_checked: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut cancelled = false;
    for e in &inputs.entries {
        if job.check().is_err() {
            cancelled = true;
            break;
        }
        match export_entry(&inputs, e, dir, a.force, &saved, &mut taken, &mut folders_checked) {
            Ok(target) => written.push(target.display().to_string()),
            Err((reason, why)) => failed.push((e.name.clone(), reason, why)),
        }
        job.advance(1);
    }
    let total = inputs.entries.len() + inputs.failed.len();
    if ctx.json {
        let list: Vec<Value> = failed.iter().map(|(input, reason, _)| json!({"input": input, "reason": reason})).collect();
        print_json(&json!({ "written": written, "failed": list, "cancelled": cancelled }));
    } else {
        let mut w = out();
        outln!(w, "{}", export_summary(written.len(), total, failed.len(), cancelled));
        let _ = w.flush();
    }
    for (input, reason, _) in &failed {
        eprintln!("{input}: FAILED: {reason}");
    }
    if cancelled {
        // the files already written stay; the summary above says how many
        return Err(Error::Cancelled.into());
    }
    if total == 0 {
        note(ctx, "note: no .ANI, .3DC, .VANI, .MANI, .3DO or .3DE files found");
    }
    Ok(failed.iter().map(|(_, _, why)| why.code()).max().unwrap_or(0))
}

/// The last line of an `--out-dir` export: `wrote N of M (K failed)`, with `, cancelled` when it was stopped (the files written stay).
fn export_summary(written: usize, total: usize, failed: usize, cancelled: bool) -> String {
    format!("wrote {written} of {total} ({failed} failed){}", if cancelled { ", cancelled" } else { "" })
}

/// One file of an `--out-dir` export: checks, read, export, write. `Err` is the reason and its kind.
fn export_entry(
    inputs: &Inputs,
    e: &Entry,
    dir: &Path,
    force: bool,
    saved: &[PathBuf],
    taken: &mut std::collections::HashSet<String>,
    folders_checked: &mut std::collections::HashSet<PathBuf>,
) -> std::result::Result<PathBuf, (String, Why)> {
    let input = |m: String| (m, Why::Input);
    let Some(format) = ModelFormat::from_file_name(&e.rel) else {
        return Err(input(format!("not an {MODEL_NAMES} file")));
    };
    if let Some(problem) = shaiya_archive::edit::loose_name_problem(&e.rel) {
        return Err(input(format!("not written: {problem}")));
    }
    if let Some(problem) = out_name_problem(&e.rel) {
        return Err(input(format!("not written: {problem}")));
    }
    let target = json_target(dir, &e.rel);
    if !taken.insert(out_key(&target)) {
        return Err(("another input of this run has the same output name".to_string(), Why::Safety));
    }
    if target.exists() && !force {
        return Err((format!("{} exists; use --force to replace it", target.display()), Why::Safety));
    }
    let bytes = inputs.read(e).map_err(|x| input(x.to_string()))?;
    let source = match &e.origin {
        Origin::Archive => e.name.clone(),
        Origin::Disk(p) => file_name(p),
    };
    let text = match modeljson::export(format, &bytes, &source) {
        Ok(t) => t,
        Err(x) if x.is_bug() => return Err((x.to_string(), Why::Bug)),
        Err(x) => return Err(input(x.to_string())),
    };
    let folder = target.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.to_path_buf());
    if !folders_checked.contains(&folder) {
        // the deepest existing folder is resolved and checked before anything is made: a link that was already there could lead into a client
        match ensure_out_folder(&folder, saved) {
            Ok(None) => {}
            Ok(Some(client)) => return Err((format!("{} is inside the client folder {}", target.display(), client.display()), Why::Safety)),
            Err(x) => return Err((x.to_string(), Why::Safety)),
        }
        folders_checked.insert(folder);
    }
    let bug = Cell::new(false);
    if let Err(x) = write_atomic_checked(&target, text.as_bytes(), &same_bytes_check(text.as_bytes(), &bug)) {
        return Err((x.to_string(), if bug.get() { Why::Bug } else { Why::Safety }));
    }
    Ok(target)
}

fn export_one(ctx: &Ctx, a: &ExportArgs, input: &str, out_file: &Path) -> Res {
    let from = a.from.as_deref();
    let force = a.force;
    let Some(format) = ModelFormat::from_file_name(input) else {
        return Err(Error::format(input, None, format!("not an {MODEL_NAMES} file")).into());
    };
    if !has_ext(&out_file.to_string_lossy(), &["json"]) {
        return Err(Fail::Usage(format!("{}: the output of export must end in .json", out_file.display())));
    }
    // the size is checked before the file is read; the output is checked before anything is read or written
    let (bytes, source) = match from {
        Some(arch) => {
            let archive = open_store(ctx, arch)?;
            check_entry(&archive, input, true, MAX_MODEL_BYTES)?;
            check_output(ctx, out_file, force, &[])?;
            (archive.read(input)?, input.to_string())
        }
        None => {
            let p = resolve(ctx, input)?;
            let len = std::fs::metadata(&p).map_err(|e| Error::io(&p, e))?.len();
            if too_large(len, MAX_MODEL_BYTES) {
                return Err(cap_error(Some(&p), MAX_MODEL_BYTES).into());
            }
            check_output(ctx, out_file, force, &[&p])?;
            let bytes = read_capped(&p, Some(&p), MAX_MODEL_BYTES)?;
            (bytes, file_name(&p))
        }
    };
    let text = modeljson::export(format, &bytes, &source).map_err(|e| json_failure(input, e))?;
    let bug = Cell::new(false);
    if let Err(e) = write_atomic_checked(out_file, text.as_bytes(), &same_bytes_check(text.as_bytes(), &bug)) {
        return Err(if bug.get() { Fail::Bug(e.to_string()) } else { e.into() });
    }
    if ctx.json {
        print_json(&json!({ "input": input, "format": format.extension(), "schema": format.schema(), "out": out_file.display().to_string(), "bytes": text.len() }));
    } else {
        let mut w = out();
        outln!(w, "wrote {}: {} as {}, {} bytes of JSON", out_file.display(), format.label(), format.schema(), num(text.len() as u64));
        let _ = w.flush();
    }
    Ok(0)
}

fn import(ctx: &Ctx, a: &ImportArgs) -> Res {
    if has_ext(&a.input, &["gltf", "glb", "obj", "bvh"]) {
        return gltf_import::run(ctx, a);
    }
    if a.map.is_some() || a.reduce || a.reduce_influences || a.keys.is_some() {
        return Err(Fail::Usage("--map, --reduce, --reduce-influences and --keys are for a glTF import (IN.gltf or IN.glb) only".to_string()));
    }
    let in_path = resolve(ctx, &a.input)?;
    let json_bytes = read_capped(&in_path, Some(&in_path), MAX_JSON_BYTES)?;
    let format = modeljson::format_of(&json_bytes).map_err(|e| json_failure(&a.input, e))?;
    let out_text = a.out.to_string_lossy().into_owned();
    if ModelFormat::from_file_name(&out_text) != Some(format) {
        let asks = match ModelFormat::from_file_name(&out_text) {
            Some(f) => f.label().to_string(),
            None => a.out.extension().map_or_else(|| "no extension".to_string(), |e| format!(".{}", e.to_string_lossy())),
        };
        return Err(Error::format(&a.input, None, format!("holds {}, but {} asks for {asks}", format.schema(), a.out.display())).into());
    }
    let is_ani = format == ModelFormat::Ani;
    match (&a.like, is_ani) {
        (None, true) => return Err(Fail::Usage("--like is required for .ANI: the file it will replace".to_string())),
        (Some(_), false) => return Err(Fail::Usage("--like is for .ANI only".to_string())),
        (None, false) if a.from.is_some() => return Err(Fail::Usage("--from needs --like (it says where the --like file is)".to_string())),
        _ => {}
    }
    // the --like file on disk is also protected from being the output
    let like_path = match (&a.like, &a.from) {
        (Some(l), None) => Some(resolve(ctx, l)?),
        _ => None,
    };
    let mut protected: Vec<&Path> = vec![&in_path];
    if let Some(l) = &like_path {
        protected.push(l);
    }
    check_output(ctx, &a.out, a.force, &protected)?;

    let (_, bytes) = modeljson::import(&json_bytes).map_err(|e| json_failure(&a.input, e))?;
    let out_name = file_name(&a.out);
    let mut rig = None;
    if let (true, Some(like)) = (is_ani, &a.like) {
        let target_bytes = match &like_path {
            Some(p) => read_capped(p, Some(p), MAX_MODEL_BYTES)?,
            None => read_target(ctx, like, a.from.as_deref())?,
        };
        let target = read_ani(like, &target_bytes)?;
        let candidate = read_ani(&out_name, &bytes)?;
        let result = RigResult::new(&candidate, &target);
        if !ctx.json {
            let mut w = out();
            result.print(&mut w, like, &candidate, &target);
            if !result.ok {
                outln!(w, "nothing written");
            }
            let _ = w.flush();
        } else if !result.ok {
            let mut doc = result.json(&a.input, like);
            doc["written"] = json!(false);
            print_json(&doc);
        }
        if !result.ok {
            return Ok(1);
        }
        rig = Some((result, target));
    }
    let bug = Cell::new(false);
    let same = same_bytes_check(&bytes, &bug);
    let check = |temp: &Path| -> Result<()> {
        same(temp)?;
        if let Some((_, target)) = &rig {
            // the file on disk must read as an .ANI that passes again
            let back = std::fs::read(temp).map_err(|e| Error::io(temp, e))?;
            let ok = Ani::parse(&back, &out_name).is_ok_and(|c| passes(&c, target));
            if !ok {
                bug.set(true);
                return Err(Error::Refused("the file written does not pass the rig check".to_string()));
            }
        }
        Ok(())
    };
    if let Err(e) = write_atomic_checked(&a.out, &bytes, &check) {
        return Err(if bug.get() { Fail::Bug(e.to_string()) } else { e.into() });
    }
    if ctx.json {
        let mut doc = match &rig {
            Some((r, _)) => r.json(&a.input, a.like.as_deref().unwrap_or("")),
            None => json!({}),
        };
        doc["input"] = json!(a.input);
        doc["format"] = json!(format.extension());
        doc["out"] = json!(a.out.display().to_string());
        doc["bytes"] = json!(bytes.len());
        doc["written"] = json!(true);
        print_json(&doc);
    } else {
        let mut w = out();
        outln!(w, "wrote {}: {}, {} bytes", a.out.display(), format.label(), num(bytes.len() as u64));
        let _ = w.flush();
    }
    Ok(0)
}

// ---- edit -------------------------------------------------------------------------------------------------------------------------

fn seconds_of(ani: &Ani) -> f32 {
    (ani.end_key as f32 - ani.first_field as f32) / 30.0
}

/// `edit retime`, `edit trim` and `edit copy-track`: the usage checks, the output checks of `import`, the tool, the rig check against IN, the
/// atomic write with a read-back that must read as an .ANI that passes again.
fn edit(ctx: &Ctx, a: &EditArgs) -> Res {
    let (io, name) = match &a.tool {
        EditTool::Retime(x) => (&x.io, "retime"),
        EditTool::Trim(x) => (&x.io, "trim"),
        EditTool::CopyTrack(x) => (&x.io, "copy-track"),
    };
    // arguments first (exit 2), before anything is read
    match &a.tool {
        EditTool::Retime(x) => {
            if let Some(f) = x.factor {
                if !f.is_finite() || !(animedit::RETIME_MIN..=animedit::RETIME_MAX).contains(&f) {
                    return Err(Fail::Usage(format!("--factor must be a number from {} to {}", animedit::RETIME_MIN, animedit::RETIME_MAX)));
                }
            }
            if x.seconds.is_some_and(|s| !s.is_finite() || s <= 0.0) {
                return Err(Fail::Usage("--seconds must be a number above 0".to_string()));
            }
        }
        EditTool::Trim(x) => {
            if x.start >= x.end {
                return Err(Fail::Usage("--start must be below --end".to_string()));
            }
        }
        EditTool::CopyTrack(x) => {
            if !x.rotation && !x.translation {
                return Err(Fail::Usage("give --rotation, --translation or both".to_string()));
            }
        }
    }
    let ani_only = |n: &str| if has_ext(n, &["ani"]) { Ok(()) } else { Err(Error::format(n, None, "not an .ANI file")) };
    ani_only(&io.input)?;
    let in_path = match &io.from {
        None => Some(resolve(ctx, &io.input)?),
        Some(_) => None,
    };
    let src_name = if let EditTool::CopyTrack(x) = &a.tool { Some(x.source.as_str()) } else { None };
    if let Some(s) = src_name {
        ani_only(s)?;
    }
    let src_path = match (src_name, &io.from) {
        (Some(s), None) => Some(resolve(ctx, s)?),
        _ => None,
    };
    let protected: Vec<&Path> = in_path.iter().chain(src_path.iter()).map(|p| p.as_path()).collect();
    check_output(ctx, &io.out, io.force, &protected)?;

    let input_bytes = read_target(ctx, &io.input, io.from.as_deref())?;
    let input = read_ani(&io.input, &input_bytes)?;
    let made = match &a.tool {
        EditTool::Retime(x) => {
            let factor = match (x.factor, x.seconds) {
                (Some(f), _) => Ok(f),
                (None, Some(s)) => animedit::factor_for_seconds(&input, s),
                (None, None) => unreachable!("clap requires one of them"),
            };
            match (factor, x.seconds) {
                // the factor that --seconds asks for is a usage error when it is outside the range --factor may have (exit 2, like --factor)
                (Ok(f), Some(s)) if x.factor.is_none() && !(animedit::RETIME_MIN..=animedit::RETIME_MAX).contains(&f) => {
                    return Err(Fail::Usage(format!(
                        "--seconds {s} makes {} {:.3} times as long: the factor must be from {} to {}",
                        io.input,
                        f,
                        animedit::RETIME_MIN,
                        animedit::RETIME_MAX
                    )));
                }
                (factor, _) => factor.and_then(|f| animedit::retime(&input, f)),
            }
        }
        EditTool::Trim(x) => animedit::trim(&input, x.start, x.end),
        EditTool::CopyTrack(x) => {
            let src_bytes = read_target(ctx, &x.source, io.from.as_deref())?;
            let source = read_ani(&x.source, &src_bytes)?;
            let bones: Vec<usize> = if x.all { (0..input.bones.len()).collect() } else { x.bones.clone() };
            if let Some(b) = bones.iter().find(|&&b| b >= input.bones.len()) {
                return Err(Fail::Usage(format!("bone {b} does not exist: {} has {} bones", io.input, input.bones.len())));
            }
            animedit::copy_tracks(&input, &source, &bones, Tracks { rotation: x.rotation, translation: x.translation })
        }
    };
    let out_ani = match made {
        Ok(o) => o,
        Err(e) => {
            let why = match e {
                shaiya_clientdata::Error::Format { what, .. } => what,
                other => other.to_string(),
            };
            if ctx.json {
                print_json(&json!({"input": io.input, "tool": name, "written": false, "error": why}));
            } else {
                eprintln!("{}: {why}", io.input);
                let mut w = out();
                outln!(w, "nothing written");
                let _ = w.flush();
            }
            return Ok(1);
        }
    };
    let bytes = out_ani.try_to_bytes().map_err(|e| Fail::Bug(e.to_string()))?;
    if !passes(&out_ani, &input) {
        return Err(Fail::Bug("the edited animation does not pass against its input".to_string()));
    }
    let out_name = file_name(&io.out);
    let bug = Cell::new(false);
    let same = same_bytes_check(&bytes, &bug);
    let check = |temp: &Path| -> Result<()> {
        same(temp)?;
        let back = std::fs::read(temp).map_err(|e| Error::io(temp, e))?;
        if !Ani::parse(&back, &out_name).is_ok_and(|c| passes(&c, &input)) {
            bug.set(true);
            return Err(Error::Refused("the file written does not pass the rig check".to_string()));
        }
        Ok(())
    };
    if let Err(e) = write_atomic_checked(&io.out, &bytes, &check) {
        return Err(if bug.get() { Fail::Bug(e.to_string()) } else { e.into() });
    }
    if ctx.json {
        print_json(&json!({
            "input": io.input,
            "tool": name,
            "out": io.out.display().to_string(),
            "bytes": bytes.len(),
            "written": true,
            "passes": true,
            "end_key": {"before": input.end_key, "after": out_ani.end_key},
        }));
    } else {
        let mut w = out();
        outln!(
            w,
            "wrote {}: {} of {}, end key {} -> {} ({:.2} s -> {:.2} s), {} bytes",
            io.out.display(),
            name,
            io.input,
            input.end_key,
            out_ani.end_key,
            seconds_of(&input),
            seconds_of(&out_ani),
            num(bytes.len() as u64)
        );
        let _ = w.flush();
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Ctx {
        Ctx { data_dir: None, json: false, quiet: true, verbose: 0, progress: crate::progress::Mode::Never }
    }

    #[test]
    fn too_large_is_a_strict_comparison() {
        assert!(!too_large(10, 10) && too_large(11, 10) && !too_large(0, 0));
        assert_eq!(MAX_MODEL_BYTES, 128 << 20);
        assert!(cap_error(None, MAX_MODEL_BYTES).to_string() == "larger than 128 MiB: not read", "{}", cap_error(None, MAX_MODEL_BYTES));
    }

    /// The cap is checked from the length before a byte is read: a small cap refuses a ten-byte file with the message `info` prints after FAILED.
    #[test]
    fn info_refuses_a_file_over_the_size_cap() {
        let dir = std::env::temp_dir().join(format!("shaiya-anim-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("big.ani");
        // the message prints the cap in MiB: a ten-byte test cap shows as 0
        std::fs::write(&f, [0u8; 10]).unwrap();
        assert_eq!(read_capped(&f, None, 10).unwrap().len(), 10);
        let e = read_capped(&f, None, 9).unwrap_err();
        assert_eq!(e.to_string(), "larger than 0 MiB: not read");
        assert_eq!(crate::exit_code_for(e.kind()), 3);
        let named = read_capped(&f, Some(&f), 9).unwrap_err().to_string();
        assert!(named.ends_with("big.ani: larger than 0 MiB: not read"), "{named}");
        assert!(read_capped(&dir.join("missing.ani"), None, 9).is_err());
        assert!(cap_error(None, MAX_JSON_BYTES).to_string() == "larger than 256 MiB: not read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reader that yields more than the cap is refused whatever the file said about its length, and no more than cap + 1 bytes are taken.
    #[test]
    fn read_bounded_stops_at_the_cap_plus_one() {
        struct Endless(u64);
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = (buf.len() as u64).min(self.0) as usize;
                buf[..n].fill(7);
                self.0 -= n as u64;
                Ok(n)
            }
        }
        // cap + 1 bytes are available: refused
        assert!(read_bounded(Endless(11), 10).unwrap().is_none());
        // far more than cap + 1 available: refused after reading cap + 1 only
        let mut r = Endless(u64::MAX / 2);
        let before = r.0;
        assert!(read_bounded(&mut r, 10).unwrap().is_none());
        assert_eq!(before - r.0, 11);
        // exactly the cap and less are kept
        assert_eq!(read_bounded(Endless(10), 10).unwrap().map(|v| v.len()), Some(10));
        assert_eq!(read_bounded(Endless(0), 10).unwrap().map(|v| v.len()), Some(0));
    }

    /// The bug path (exit 5) cannot be provoked end to end: the mapper is called with the self-check error.
    #[test]
    fn a_self_check_error_is_a_bug_and_exits_5() {
        let bug = json_failure("a.ani", JsonError::SelfCheck("x".to_string()));
        assert!(matches!(bug, Fail::Bug(_)));
        assert_eq!(stop(&ctx(), &bug), 5);
        // a problem with the input is an input error (exit 3), also a refused value or a cap
        for e in [JsonError::Schema("x".into()), JsonError::TooLarge("x".into()), JsonError::NotWritable("x".into()), JsonError::NotFinite { at: "k".into() }] {
            let f = json_failure("a.json", e);
            assert!(matches!(f, Fail::Error(_)));
            assert_eq!(stop(&ctx(), &f), 3);
        }
        assert_eq!(stop(&ctx(), &Fail::Usage("x".into())), 2);
    }

    /// A cancelled `--out-dir` export keeps the files it wrote, still gets its summary line (which says it was cancelled), and ends as a cancel.
    #[test]
    fn a_cancelled_out_dir_export_keeps_its_files_and_ends_as_a_cancel() {
        let base = std::env::temp_dir().join(format!("shaiya-anim-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, out) = (base.join("src"), base.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        for n in ["a.ANI", "b.ANI", "c.ANI"] {
            std::fs::write(src.join(n), shaiya_testkit::anim::arm_ani(30)).unwrap();
        }
        let cancel = shaiya_core::Cancel::new();
        let progress = shaiya_core::FnProgress::new({
            let cancel = cancel.clone();
            move |done, _| {
                if done == 1 {
                    cancel.cancel();
                }
            }
        });
        let job = shaiya_core::Job::new(&progress, cancel);
        let c = Ctx { data_dir: Some(base.join("data")), ..ctx() };
        let args = ExportArgs { inputs: vec![src.to_string_lossy().into_owned()], from: None, out: None, out_dir: Some(out.clone()), force: false, format: ExportFormat::Json, anims: Vec::new(), allow_lossy: false, pad_end: false, texture_uri_base: None, drop_translation: false, scale: 1.0 };
        let r = export_dir(&c, &args, &out, &job);
        assert!(matches!(r, Err(Fail::Error(Error::Cancelled))));
        let written = std::fs::read_dir(&out).unwrap().count();
        assert_eq!(written, 1, "exactly the file counted before the cancel is on disk");
        assert_eq!(export_summary(1, 3, 0, true), "wrote 1 of 3 (0 failed), cancelled");
        assert_eq!(export_summary(3, 3, 0, false), "wrote 3 of 3 (0 failed)");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A folder that cannot be listed is reported and the walk goes on. (A path that cannot be listed is simulated by a file where a folder is
    /// expected: the same `read_dir` error path as a permission failure, which cannot be made portably.)
    #[test]
    fn a_folder_that_cannot_be_listed_is_reported_and_the_walk_goes_on() {
        let dir = std::env::temp_dir().join(format!("shaiya-anim-walk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ani"), b"x").unwrap();
        let (mut found, mut failed) = (Vec::new(), Vec::new());
        walk(&dir.join("a.ani"), "", &mut found, &mut failed);
        assert!(found.is_empty());
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].0.ends_with("a.ani"), "{failed:?}");
        walk(&dir, "", &mut found, &mut failed);
        assert_eq!(found.len(), 1);
        assert_eq!(failed.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
