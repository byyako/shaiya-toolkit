//! Wavefront OBJ as a lossy view of a static mesh: `.3DO`, `.3DE` and (at rest, bones ignored) `.3DC` go out, a `.3DO` or `.3DE` comes in.
//!
//! The mapping is the one mirror of [`super::xform`] (`v x y -z`, `vn nx ny -nz`, `f a/a/a c/c/c b/b/b`), with `vt u 1-v` (OBJ puts the
//! texture origin at the bottom left, the game's files at the top left). One `vt` and one `vn` per vertex, so the vertex numbers are the
//! file's. A texture is a name only: `usemtl <name>` and a `.mtl` that holds `newmtl` lines and nothing else, plus the raw bytes of the
//! stored name as `# texture-hex <hex>`. The import never opens the `.mtl` (this module does no file access at all: it reads the bytes it is
//! given), takes triangles and quads, negative indices, unifies identical `(v, vt, vn)` corners into vertices, computes missing normals
//! (and says so) and refuses a missing texture coordinate, a second material and more than 65,535 vertices. Every cap is checked before the
//! memory it guards is used.

use super::import::{computed_normals, ImportError, ImportKind, ImportReport, Imported, MAX_TEXTURE, MAX_VERTICES};
use super::mesh::{c3dc_mesh, scene_of_3de, scene_of_3do, Collect, Mesh3d};
use super::xform::{s_point, s_tri};
use super::{GltfOptions, InterchangeError, RefusalKind, Report};
use crate::error::decode_cp949;
use crate::mesh3dc::C3dc;
use crate::mesh3de::{Mesh3de, Vertex3de};
use crate::mesh3do::{Mesh3do, Vertex3do};
use std::collections::HashMap;

/// The first line of every OBJ the toolkit writes.
pub const OBJ_HEADER: &str = "# shaiya-toolkit obj v1";
/// The most bytes of an OBJ the import reads.
pub const MAX_OBJ_BYTES: usize = 64 << 20;
/// The most `v`, `vt` or `vn` lines the import keeps (each).
pub const MAX_ELEMENTS: usize = 1 << 21;
/// The most faces (of 3 or 4 corners) the import keeps.
pub const MAX_FACES: usize = 1 << 20;
/// The most refusals listed; the rest are counted.
const LIST_CAP: usize = 200;

/// What to export.
#[derive(Clone, Debug)]
pub enum ObjInput {
    Mesh3do(Mesh3do),
    Mesh3de(Mesh3de),
    /// A `.3DC` at rest: the vertices as stored, the bones and weights left out.
    Mesh3dc(C3dc),
}

#[derive(Clone, Debug, Default)]
pub struct ObjOptions {
    /// The archive path or file name of the input (never a folder path); names the object and goes into the header.
    pub source: String,
    /// The file name of the `.mtl` written beside the `.obj` (the `mtllib` line); empty when none is made.
    pub mtl_name: String,
}

#[derive(Clone, Debug)]
pub struct ObjOutput {
    pub obj: Vec<u8>,
    /// The `.mtl` (names only), when a mesh names a texture.
    pub mtl: Option<Vec<u8>>,
    pub report: Report,
}

fn stem_of(source: &str, fallback: &str) -> String {
    let file = source.rsplit(['/', '\\']).next().unwrap_or(source);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    if stem.is_empty() {
        fallback.to_string()
    } else {
        stem.to_string()
    }
}

/// A name that stays one word on a line: whitespace and control characters become `_`.
fn word(s: &str) -> String {
    s.chars().map(|c| if c.is_whitespace() || c.is_control() { '_' } else { c }).collect()
}

/// A name that stays on one line (spaces kept).
fn line_text(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { '_' } else { c }).collect()
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()).collect()
}

/// The texture name text of a stored name: cp949 up to the first NUL.
fn texture_text(raw: &[u8]) -> String {
    decode_cp949(raw.split(|&b| b == 0).next().unwrap_or(&[]))
}

/// Writes `input` as OBJ (and a `.mtl` of names). A file that cannot be written (NaN or an infinity in a vertex, a face naming a missing
/// vertex, no vertices) is refused with the vertex; nothing is returned then.
pub fn export_obj(input: ObjInput, opts: &ObjOptions) -> Result<ObjOutput, InterchangeError> {
    let mut c = Collect::new(false);
    let gopts = GltfOptions { source: opts.source.clone(), ..GltfOptions::default() };
    let (meshes, mut notes) = match &input {
        ObjInput::Mesh3do(m) => {
            let s = scene_of_3do(m, &mut c, &gopts);
            (s.meshes, s.notes)
        }
        ObjInput::Mesh3de(m) => {
            let s = scene_of_3de(m, &mut c, &gopts);
            let mut notes = s.notes;
            let odd = m.vertices.iter().filter(|v| v.flag != -1).count();
            if odd > 0 {
                notes.push(format!("{odd} vertices have a flag other than -1; the flag is not written"));
            }
            (s.meshes, notes)
        }
        ObjInput::Mesh3dc(m) => {
            let stem = stem_of(&opts.source, "mesh");
            let many = m.meshes.len() > 1;
            let meshes: Vec<Mesh3d> = m.meshes.iter().enumerate().map(|(i, x)| c3dc_mesh(x, &mut c, &if many { format!("{stem}_{i}") } else { stem.clone() }, false, 0)).collect();
            (meshes, vec!["a .3DC is written as it lies at rest: the bones and the weights are left out (the glTF export carries them)".to_string()])
        }
    };
    if c.failed() {
        return Err(InterchangeError::Refused { format: "OBJ", list: c.refused, more: c.more });
    }
    let (obj, mtl) = write_obj(&meshes, opts);
    if !meshes.is_empty() {
        notes.push("OBJ holds no bones, no animation and no texture picture: a texture is a name only".to_string());
    }
    let report = Report { notes, meshes: meshes.len(), vertices: meshes.iter().map(|m| m.positions.len()).sum(), ..Report::default() };
    Ok(ObjOutput { obj: obj.into_bytes(), mtl: mtl.map(String::into_bytes), report })
}

/// The OBJ text and the `.mtl` text (when a texture is named) of meshes already in the mirrored space.
fn write_obj(meshes: &[Mesh3d], opts: &ObjOptions) -> (String, Option<String>) {
    let mut names: Vec<String> = Vec::new();
    for m in meshes {
        let t = line_text(texture_text(&m.texture).trim());
        if !t.is_empty() && !names.contains(&t) {
            names.push(t);
        }
    }
    let mut o = String::new();
    o.push_str(OBJ_HEADER);
    o.push('\n');
    if !opts.source.is_empty() {
        o.push_str(&format!("# source {}\n", line_text(opts.source.rsplit(['/', '\\']).next().unwrap_or(&opts.source))));
    }
    o.push_str("# a lossy view of the game file: z is mirrored (x y -z), a triangle swaps two corners, v is 1 - v; a texture is a name only\n");
    if !names.is_empty() && !opts.mtl_name.is_empty() {
        o.push_str(&format!("mtllib {}\n", word(&opts.mtl_name)));
    }
    let mut base = 1usize;
    for m in meshes {
        o.push_str(&format!("o {}\n", word(&m.name)));
        if !m.texture.is_empty() {
            o.push_str(&format!("# texture-hex {}\n", hex_of(&m.texture)));
        }
        for p in &m.positions {
            o.push_str(&format!("v {} {} {}\n", p[0], p[1], p[2]));
        }
        // 1 - v in f64 gives the stored f32 back for every v from about 1e-9 up (a v of -0 comes back as +0 and a smaller one may round to its
        // neighbour; the texture coordinates of the US EP4.5 meshes are checked to the bit by the opt-in test)
        for uv in &m.uvs {
            o.push_str(&format!("vt {} {}\n", uv[0], 1.0f64 - f64::from(uv[1])));
        }
        if let Some(ns) = &m.normals {
            for n in ns {
                o.push_str(&format!("vn {} {} {}\n", n[0], n[1], n[2]));
            }
        }
        let t = line_text(texture_text(&m.texture).trim());
        if !t.is_empty() {
            o.push_str(&format!("usemtl {t}\n"));
        }
        let has_n = m.normals.is_some();
        for tri in &m.triangles {
            o.push('f');
            for &i in tri {
                let i = base + usize::from(i);
                if has_n {
                    o.push_str(&format!(" {i}/{i}/{i}"));
                } else {
                    o.push_str(&format!(" {i}/{i}"));
                }
            }
            o.push('\n');
        }
        base += m.positions.len();
    }
    let mtl = (!names.is_empty()).then(|| {
        let mut s = format!("{OBJ_HEADER}\n# materials by name only: no texture file is named or written\n");
        for n in &names {
            s.push_str(&format!("newmtl {n}\n"));
        }
        s
    });
    (o, mtl)
}

// ---- the import ---------------------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Refs {
    list: Vec<super::Refusal>,
    more: usize,
}

impl Refs {
    fn add(&mut self, kind: RefusalKind, what: impl Into<String>, reason: impl Into<String>) {
        if self.list.len() < LIST_CAP {
            self.list.push(super::Refusal { kind, what: what.into(), reason: reason.into() });
        } else {
            self.more += 1;
        }
    }

    fn failed(&self) -> bool {
        !self.list.is_empty()
    }

    fn done(self) -> ImportError {
        ImportError { list: self.list, more: self.more }
    }
}

/// One corner of a face: indices into the lists, 0-based.
#[derive(Clone, Copy)]
struct Corner {
    v: u32,
    vt: Option<u32>,
    vn: Option<u32>,
}

/// A face, kept small: the lists are capped at [`MAX_ELEMENTS`], so an index fits 32 bits.
struct Face {
    corners: [Corner; 4],
    n: usize,
    /// The line number, for the messages.
    line: u32,
    /// Which `usemtl` name was active (an index into the names, `None` before any).
    material: Option<u32>,
}

/// An index of an OBJ list of `len` entries so far: 1-based, or negative from the end.
fn resolve(token: &str, len: usize) -> Result<usize, String> {
    let i: i64 = token.parse().map_err(|_| format!("{token:?} is not an index"))?;
    let r = if i > 0 {
        usize::try_from(i - 1).ok()
    } else if i < 0 {
        len.checked_sub(usize::try_from(i.unsigned_abs()).unwrap_or(usize::MAX))
    } else {
        None
    };
    match r {
        Some(k) if k < len => Ok(k),
        _ => Err(format!("the index {i} names no entry (the list holds {len} so far)")),
    }
}

fn floats<const N: usize>(rest: &[&str], min: usize) -> Option<[f64; N]> {
    if rest.len() < min {
        return None;
    }
    let mut out = [0f64; N];
    for (k, o) in out.iter_mut().enumerate() {
        *o = rest.get(k)?.parse().ok()?;
    }
    Some(out)
}

/// Reads `bytes` as a new `.3DO` or `.3DE`. `cancel` is checked every 4,096 lines (the import then refuses with "cancelled"). Nothing is
/// opened: an `mtllib` line is counted and ignored.
pub fn import_obj(bytes: &[u8], kind: ImportKind, cancel: Option<&shaiya_core::job::Cancel>) -> Result<(Imported, ImportReport), ImportError> {
    if !matches!(kind, ImportKind::Mesh3do | ImportKind::Mesh3de) {
        return Err(ImportError::one(RefusalKind::Like, "OBJ", "an OBJ is imported as a .3DO or .3DE: it holds no skin and no keys for a .3DC or .ANI"));
    }
    if bytes.len() > MAX_OBJ_BYTES {
        return Err(ImportError::one(RefusalKind::Limit, "OBJ", format!("{} bytes is over the {MAX_OBJ_BYTES}-byte limit", bytes.len())));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Err(ImportError::one(RefusalKind::Format, "OBJ", "the file is not UTF-8 text"));
    };
    let mut refs = Refs::default();
    let mut report = ImportReport::default();
    let mut vs: Vec<[f32; 3]> = Vec::new();
    let mut vts: Vec<[f64; 2]> = Vec::new();
    let mut vns: Vec<[f32; 3]> = Vec::new();
    let mut faces: Vec<Face> = Vec::new();
    let mut materials: Vec<String> = Vec::new();
    let mut current: Option<usize> = None;
    let mut hexes: Vec<Vec<u8>> = Vec::new();
    let mut ignored: HashMap<String, usize> = HashMap::new();
    'lines: for (n, raw) in text.split('\n').enumerate() {
        let line_no = n + 1;
        if n % 4096 == 0 && cancel.is_some_and(shaiya_core::job::Cancel::is_cancelled) {
            return Err(ImportError::one(RefusalKind::Limit, "OBJ", "cancelled"));
        }
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = line.strip_prefix('#') {
            if let Some(h) = c.trim().strip_prefix("texture-hex ") {
                match unhex(h.trim()) {
                    Some(b) if b.len() <= MAX_TEXTURE => hexes.push(b),
                    Some(b) => refs.add(RefusalKind::Limit, format!("line {line_no}"), format!("the texture name is {} bytes (at most {MAX_TEXTURE})", b.len())),
                    None => refs.add(RefusalKind::Format, format!("line {line_no}"), "texture-hex is not hexadecimal text"),
                }
            }
            continue;
        }
        if line.ends_with('\\') {
            refs.add(RefusalKind::Unsupported, format!("line {line_no}"), "a line that continues on the next one (a trailing backslash) is not read");
            break;
        }
        let mut it = line.split_whitespace();
        let key = it.next().unwrap_or("");
        // only the first few words of a line are collected (a face needs 5 to be refused for having too many, a vertex up to 7): a line of
        // millions of words costs nothing
        let rest: Vec<&str> = it.by_ref().take(if key == "f" { 5 } else { 8 }).collect();
        match key {
            "v" | "vn" | "vt" => {
                let (len, cap) = match key {
                    "v" => (vs.len(), MAX_ELEMENTS),
                    "vt" => (vts.len(), MAX_ELEMENTS),
                    _ => (vns.len(), MAX_ELEMENTS),
                };
                if len >= cap {
                    refs.add(RefusalKind::Limit, format!("line {line_no}"), format!("more than {cap} `{key}` lines"));
                    break 'lines;
                }
                match key {
                    // `v x y z w` (exactly four numbers) is a rational position: a w other than 1 would move the vertex, so it is refused (six or
                    // seven numbers are the colour extension, whose colours are not read)
                    "v" if rest.len() == 4 && !rest[3].parse::<f64>().is_ok_and(|w| (w - 1.0).abs() <= 1e-9) => {
                        refs.add(RefusalKind::Unsupported, format!("line {line_no}"), format!("vertex {} has a fourth number `{}`: only w = 1 is read", vs.len() + 1, rest[3]));
                    }
                    "v" => match floats::<3>(&rest, 3) {
                        Some(p) if p.iter().all(|c| c.is_finite()) => vs.push([p[0] as f32, p[1] as f32, p[2] as f32]),
                        Some(_) => refs.add(RefusalKind::NotFinite, format!("line {line_no}"), format!("vertex {} holds NaN or an infinity", vs.len() + 1)),
                        None => refs.add(RefusalKind::Format, format!("line {line_no}"), "a `v` line needs three numbers"),
                    },
                    "vt" => match floats::<2>(&rest, 2) {
                        Some(t) if t.iter().all(|c| c.is_finite()) => vts.push(t),
                        Some(_) => refs.add(RefusalKind::NotFinite, format!("line {line_no}"), format!("texture coordinate {} holds NaN or an infinity", vts.len() + 1)),
                        None => refs.add(RefusalKind::Format, format!("line {line_no}"), "a `vt` line needs two numbers"),
                    },
                    _ => match floats::<3>(&rest, 3) {
                        Some(p) if p.iter().all(|c| c.is_finite()) => vns.push([p[0] as f32, p[1] as f32, p[2] as f32]),
                        Some(_) => refs.add(RefusalKind::NotFinite, format!("line {line_no}"), format!("normal {} holds NaN or an infinity", vns.len() + 1)),
                        None => refs.add(RefusalKind::Format, format!("line {line_no}"), "a `vn` line needs three numbers"),
                    },
                }
                if refs.list.len() >= LIST_CAP {
                    break 'lines;
                }
            }
            "f" => {
                if faces.len() >= MAX_FACES {
                    refs.add(RefusalKind::Limit, format!("line {line_no}"), format!("more than {MAX_FACES} faces"));
                    break 'lines;
                }
                if rest.len() < 3 {
                    refs.add(RefusalKind::Format, format!("line {line_no}"), "a face needs at least 3 corners");
                    continue;
                }
                if rest.len() > 4 {
                    refs.add(RefusalKind::Unsupported, format!("line {line_no}"), "a face of more than 4 corners: triangulate it in the program that made the file");
                    continue;
                }
                let mut corners = [Corner { v: 0, vt: None, vn: None }; 4];
                let mut ok = true;
                for (k, tok) in rest.iter().enumerate() {
                    let parts: Vec<&str> = tok.split('/').take(4).collect();
                    if parts.len() > 3 || parts[0].is_empty() {
                        refs.add(RefusalKind::Format, format!("line {line_no}"), format!("the corner {tok:?} is not `v`, `v/vt`, `v//vn` or `v/vt/vn`"));
                        ok = false;
                        break;
                    }
                    let get = |s: &str, len: usize, what: &str| -> Result<Option<usize>, String> {
                        if s.is_empty() {
                            Ok(None)
                        } else {
                            resolve(s, len).map(Some).map_err(|m| format!("{what}: {m}"))
                        }
                    };
                    let parsed = resolve(parts[0], vs.len()).map_err(|m| format!("vertex: {m}")).and_then(|v| {
                        let vt = get(parts.get(1).copied().unwrap_or(""), vts.len(), "texture coordinate")?;
                        let vn = get(parts.get(2).copied().unwrap_or(""), vns.len(), "normal")?;
                        Ok(Corner { v: v as u32, vt: vt.map(|x| x as u32), vn: vn.map(|x| x as u32) })
                    });
                    match parsed {
                        Ok(c) => corners[k] = c,
                        Err(m) => {
                            refs.add(RefusalKind::Index, format!("line {line_no}"), m);
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    faces.push(Face { corners, n: rest.len(), line: u32::try_from(line_no).unwrap_or(u32::MAX), material: current.map(|m| m as u32) });
                }
            }
            "usemtl" => {
                let name = line["usemtl".len()..].trim().to_string();
                current = if name.is_empty() {
                    None
                } else {
                    Some(materials.iter().position(|m| *m == name).unwrap_or_else(|| {
                        materials.push(name);
                        materials.len() - 1
                    }))
                };
            }
            // groups and objects are merged into one mesh; the material decides whether that is allowed
            "o" | "g" => {}
            // `mtllib` (never opened), smoothing groups, lines, points, curves and the rest: counted, not read
            other => {
                *ignored.entry(other.to_string()).or_insert(0) += 1;
            }
        }
    }
    if refs.failed() {
        return Err(refs.done());
    }
    if faces.is_empty() {
        return Err(ImportError::one(RefusalKind::Empty, "OBJ", "the file has no faces"));
    }
    // one material for the whole mesh: a game mesh has one texture name
    let mut used: Vec<Option<u32>> = Vec::new();
    for f in &faces {
        if !used.contains(&f.material) {
            used.push(f.material);
        }
    }
    if used.len() > 1 {
        let names: Vec<String> = used.iter().map(|m| m.map_or("(no material)".to_string(), |i| materials[i as usize].clone())).collect();
        return Err(ImportError::one(RefusalKind::Unsupported, "materials", format!("the faces use {} materials ({}); a .3DO or .3DE holds one texture name", used.len(), names.join(", "))));
    }
    if let Some(f) = faces.iter().find(|f| f.corners[..f.n].iter().any(|c| c.vt.is_none())) {
        return Err(ImportError::one(RefusalKind::Format, format!("line {}", f.line), "a corner has no texture coordinate (`vt`); a mesh without them cannot be textured in the game"));
    }
    let all_normals = faces.iter().all(|f| f.corners[..f.n].iter().all(|c| c.vn.is_some()));
    let want_normals = kind == ImportKind::Mesh3do;
    // split into triangles, unify corners into vertices
    let mut ids: HashMap<(usize, usize, usize), u16> = HashMap::new();
    let mut vertices: Vec<(usize, usize, usize)> = Vec::new();
    let mut tris: Vec<[u16; 3]> = Vec::new();
    for f in &faces {
        let c = &f.corners;
        let split: Vec<[usize; 3]> = if f.n == 3 {
            vec![[0, 1, 2]]
        } else {
            let d = |a: usize, b: usize| {
                let (p, q) = (vs[c[a].v as usize], vs[c[b].v as usize]);
                (0..3).map(|i| f64::from(p[i] - q[i]).powi(2)).sum::<f64>()
            };
            if d(0, 2) <= d(1, 3) {
                vec![[0, 1, 2], [0, 2, 3]]
            } else {
                vec![[0, 1, 3], [1, 2, 3]]
            }
        };
        for t in split {
            let mut out = [0u16; 3];
            for (k, &ci) in t.iter().enumerate() {
                let cn = c[ci];
                let key = (cn.v as usize, cn.vt.unwrap_or(0) as usize, if all_normals && want_normals { cn.vn.unwrap_or(0) as usize } else { usize::MAX });
                let id = match ids.get(&key) {
                    Some(&id) => id,
                    None => {
                        if vertices.len() >= MAX_VERTICES {
                            return Err(ImportError::one(RefusalKind::Limit, format!("line {}", f.line), format!("the mesh needs more than {MAX_VERTICES} vertices (one per distinct position, texture coordinate and normal); a game mesh holds at most {MAX_VERTICES}")));
                        }
                        let id = vertices.len() as u16;
                        vertices.push(key);
                        ids.insert(key, id);
                        id
                    }
                };
                out[k] = id;
            }
            // the mirror turns the winding round: back to the game's order
            tris.push(s_tri(out));
        }
    }
    // the vertices in the order of the file's `v` lines (then `vt`, `vn`), not in order of first use: an OBJ the toolkit wrote gives its
    // vertex numbers back
    let mut order: Vec<usize> = (0..vertices.len()).collect();
    order.sort_by_key(|&i| vertices[i]);
    let mut perm = vec![0u16; vertices.len()];
    for (new, &old) in order.iter().enumerate() {
        perm[old] = new as u16;
    }
    for t in &mut tris {
        for i in t.iter_mut() {
            *i = perm[usize::from(*i)];
        }
    }
    let vertices: Vec<(usize, usize, usize)> = order.iter().map(|&i| vertices[i]).collect();
    // the texture: the visible material name, unless the file also carries the raw bytes of the same name
    let name = used[0].map(|i| materials[i as usize].clone());
    let texture = {
        let from_name = |n: &str| -> Result<Vec<u8>, ImportError> {
            let (b, _, bad) = encoding_rs::EUC_KR.encode(n);
            if bad {
                return Err(ImportError::one(RefusalKind::Format, "material", format!("the material name {n:?} has characters the game's text encoding (cp949) cannot hold")));
            }
            if b.len() + 1 > MAX_TEXTURE {
                return Err(ImportError::one(RefusalKind::Limit, "material", format!("the texture name is {} bytes (at most {MAX_TEXTURE})", b.len() + 1)));
            }
            let mut v = b.into_owned();
            v.push(0);
            Ok(v)
        };
        let distinct = hexes.iter().any(|h| *h != hexes[0]);
        if distinct {
            return Err(ImportError::one(RefusalKind::Unsupported, "texture-hex", "the file names more than one texture; a .3DO or .3DE holds one"));
        }
        match (hexes.first(), &name) {
            (Some(h), Some(n)) if texture_text(h).trim() == n.as_str() => h.clone(),
            (Some(h), None) => h.clone(),
            (Some(_), Some(n)) => {
                report.notes.push("a `# texture-hex` line names another texture than usemtl: the material name was used".to_string());
                from_name(n)?
            }
            (None, Some(n)) => from_name(n)?,
            (None, None) => Vec::new(),
        }
    };
    let positions: Vec<[f32; 3]> = vertices.iter().map(|k| s_point(vs[k.0])).collect();
    let uvs: Vec<[f32; 2]> = vertices.iter().map(|k| [vts[k.1][0] as f32, (1.0 - vts[k.1][1]) as f32]).collect();
    report.meshes = 1;
    report.vertices = vertices.len();
    let mut skipped: Vec<(&String, &usize)> = ignored.iter().filter(|(k, _)| k.as_str() != "mtllib").collect();
    skipped.sort();
    for (k, n) in skipped {
        report.notes.push(format!("{n} `{k}` lines were not read"));
    }
    if ignored.contains_key("mtllib") {
        report.notes.push("the .mtl named by mtllib was not opened: the texture is the usemtl name".to_string());
    }
    let imported = if kind == ImportKind::Mesh3de {
        if !vns.is_empty() {
            report.notes.push("a .3DE holds no normals: the file's were left out".to_string());
        }
        Imported::Mesh3de(Mesh3de {
            texture,
            vertices: positions.iter().zip(&uvs).map(|(p, uv)| Vertex3de { pos: *p, flag: -1, uv: *uv }).collect(),
            faces: tris,
            block: None,
        })
    } else {
        let normals: Vec<[f32; 3]> = if all_normals {
            vertices.iter().map(|k| s_point(vns[k.2])).collect()
        } else {
            let have = faces.iter().map(|f| f.corners[..f.n].iter().filter(|c| c.vn.is_some()).count()).sum::<usize>();
            let total = faces.iter().map(|f| f.n).sum::<usize>();
            report.notes.push(format!("{} of {total} corners had no normal (`vn`): the normals were computed from the faces", total - have));
            computed_normals(&positions, &tris)
        };
        Imported::Mesh3do(Mesh3do {
            texture,
            vertices: positions.iter().zip(&normals).zip(&uvs).map(|((p, n), uv)| Vertex3do { pos: *p, normal: *n, uv: *uv }).collect(),
            faces: tris,
            frame_rate: 0,
            frames: Vec::new(),
        })
    };
    Ok((imported, report))
}
