//! The glTF reader: a `.gltf` JSON with its buffers, or a `.glb`, and the accessor layer (component types, strides, bounds checked before
//! anything is allocated). The export's own check reads what the toolkit wrote through [`Gltf::from_parts`] and [`Gltf::from_glb`]; the import
//! goes through [`Gltf::open`], which is strict: buffers only from a data uri or a file in the same folder (never a URL, a path outside it
//! or a link out of it), every size capped before it is allocated, `extensionsRequired` must be empty, and an accessor that is sparse or
//! normalized is refused. Nothing here reads an image or writes a file.

use super::RefusalKind;
use serde_json::Value;
use std::path::Path;

pub const GLB_MAGIC: u32 = 0x4654_6C67;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// The largest buffer, `.glb` or `.gltf` the import reads (the size cap of a model file).
pub const MAX_BUFFER_BYTES: usize = 128 << 20;
/// The most values (elements times components) one accessor may hold; a bigger one is refused before anything is allocated.
pub const MAX_ACCESSOR_VALUES: usize = 1 << 23;
/// The most nodes, accessors, views and animations an imported file may list.
pub const MAX_LIST: usize = 1 << 20;

/// A parsed file: the JSON, the first binary buffer and any further buffers.
#[derive(Clone, Debug)]
pub struct Gltf {
    pub json: Value,
    /// Buffer 0 (the `.glb`'s BIN chunk, or the `.bin` of a `.gltf`).
    pub bin: Vec<u8>,
    /// Buffers 1 and up (only a file made by another program has any).
    pub more: Vec<Vec<u8>>,
}

/// Why a file could not be opened: a short kind for counting and the words.
pub type ReadError = (RefusalKind, String);

/// An accessor's values, flattened (`count * width` numbers).
#[derive(Clone, Debug)]
pub struct Accessor {
    pub component_type: u32,
    pub count: usize,
    pub width: usize,
    pub data: Vec<f64>,
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub fn component_size(t: u64) -> Option<usize> {
    match t {
        5120 | 5121 => Some(1),
        5122 | 5123 => Some(2),
        5125 | 5126 => Some(4),
        _ => None,
    }
}

pub fn type_width(t: &str) -> Option<usize> {
    match t {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" | "MAT2" => Some(4),
        "MAT3" => Some(9),
        "MAT4" => Some(16),
        _ => None,
    }
}

impl Gltf {
    /// A `.gltf` text and the bytes of its buffer.
    pub fn from_parts(json: &[u8], bin: &[u8]) -> Result<Gltf, String> {
        let json: Value = serde_json::from_slice(json).map_err(|e| format!("the JSON does not parse: {e}"))?;
        Ok(Gltf { json, bin: bin.to_vec(), more: Vec::new() })
    }

    fn buffer(&self, i: usize) -> Option<&[u8]> {
        if i == 0 {
            Some(&self.bin)
        } else {
            self.more.get(i - 1).map(Vec::as_slice)
        }
    }

    /// A `.glb`: the header, the JSON chunk and at most one BIN chunk, every length 4-aligned and the total exact.
    pub fn from_glb(d: &[u8]) -> Result<Gltf, String> {
        if d.len() < 12 || u32_at(d, 0) != Some(GLB_MAGIC) {
            return Err("not a .glb (the magic is wrong)".to_string());
        }
        if u32_at(d, 4) != Some(2) {
            return Err("the .glb version is not 2".to_string());
        }
        if u32_at(d, 8).map(|n| n as usize) != Some(d.len()) {
            return Err("the .glb length field does not match the file".to_string());
        }
        let mut at = 12;
        let mut json = None;
        let mut bin = Vec::new();
        let mut first = true;
        while at < d.len() {
            let (Some(len), Some(kind)) = (u32_at(d, at), u32_at(d, at + 4)) else { return Err("a chunk header is cut off".to_string()) };
            let len = len as usize;
            if !len.is_multiple_of(4) {
                return Err("a chunk length is not a multiple of 4".to_string());
            }
            let body = d.get(at + 8..).and_then(|r| r.get(..len)).ok_or_else(|| "a chunk runs past the end of the file".to_string())?;
            match (first, kind) {
                (true, CHUNK_JSON) => json = Some(body),
                (false, CHUNK_BIN) if bin.is_empty() => bin = body.to_vec(),
                (false, CHUNK_BIN) => return Err("more than one BIN chunk".to_string()),
                (true, _) => return Err("the first chunk is not JSON".to_string()),
                (false, _) => {}
            }
            first = false;
            at += 8 + len;
        }
        let json = json.ok_or_else(|| "no JSON chunk".to_string())?;
        Gltf::from_parts(json, &bin)
    }

    pub fn array(&self, key: &str) -> &[Value] {
        self.json.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
    }

    /// Reads accessor `idx`: bounds are checked against the view and the buffer before the values are allocated. A sparse or normalized
    /// accessor, one without a view, and one over [`MAX_ACCESSOR_VALUES`] values are errors.
    pub fn accessor(&self, idx: usize) -> Result<Accessor, String> {
        let a = self.array("accessors").get(idx).ok_or_else(|| format!("accessor {idx} does not exist"))?;
        if a.get("sparse").is_some() {
            return Err(format!("accessor {idx} is sparse (not read)"));
        }
        if a.get("normalized").and_then(Value::as_bool) == Some(true) {
            return Err(format!("accessor {idx} is normalized (integer values read as fractions are not read)"));
        }
        let ct = a.get("componentType").and_then(Value::as_u64).ok_or_else(|| format!("accessor {idx} has no componentType"))?;
        let csize = component_size(ct).ok_or_else(|| format!("accessor {idx} has the component type {ct}"))?;
        let width = a.get("type").and_then(Value::as_str).and_then(type_width).ok_or_else(|| format!("accessor {idx} has a bad type"))?;
        let count = a.get("count").and_then(Value::as_u64).ok_or_else(|| format!("accessor {idx} has no count"))?;
        let count = usize::try_from(count).map_err(|_| format!("accessor {idx} has too many elements"))?;
        if count.checked_mul(width).is_none_or(|n| n > MAX_ACCESSOR_VALUES) {
            return Err(format!("accessor {idx} holds more than {MAX_ACCESSOR_VALUES} values"));
        }
        let view_idx = a.get("bufferView").and_then(Value::as_u64).ok_or_else(|| format!("accessor {idx} has no bufferView"))? as usize;
        let view = self.array("bufferViews").get(view_idx).ok_or_else(|| format!("accessor {idx}: bufferView {view_idx} does not exist"))?;
        let get = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
        let buffer_idx = view.get("buffer").and_then(Value::as_u64).unwrap_or(0) as usize;
        let buf = self.buffer(buffer_idx).ok_or_else(|| format!("bufferView {view_idx} names buffer {buffer_idx}, which does not exist"))?;
        let (voff, vlen) = (get(view, "byteOffset"), get(view, "byteLength"));
        if voff.checked_add(vlen).is_none_or(|e| e > buf.len()) {
            return Err(format!("bufferView {view_idx} runs past the buffer"));
        }
        let elem = csize * width;
        let stride = view.get("byteStride").and_then(Value::as_u64).map_or(elem, |s| s as usize);
        if stride < elem {
            return Err(format!("accessor {idx}: the stride is smaller than one element"));
        }
        let aoff = get(a, "byteOffset");
        if count == 0 {
            return Err(format!("accessor {idx} has count 0"));
        }
        let last = (count - 1).checked_mul(stride).and_then(|n| n.checked_add(aoff)).and_then(|n| n.checked_add(elem));
        if last.is_none_or(|e| e > vlen) {
            return Err(format!("accessor {idx} runs past its bufferView"));
        }
        let base = voff + aoff;
        let mut data = Vec::with_capacity(count * width);
        for e in 0..count {
            for k in 0..width {
                let at = base + e * stride + k * csize;
                let b = &buf[at..at + csize];
                data.push(match ct {
                    5120 => f64::from(b[0] as i8),
                    5121 => f64::from(b[0]),
                    5122 => f64::from(i16::from_le_bytes([b[0], b[1]])),
                    5123 => f64::from(u16::from_le_bytes([b[0], b[1]])),
                    5125 => f64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    _ => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                });
            }
        }
        Ok(Accessor { component_type: ct as u32, count, width, data })
    }

    /// Opens a `.gltf` (JSON text) or a `.glb` for import. `resolve` is asked for the bytes of a buffer whose `uri` is a plain file name
    /// (already checked by [`buffer_name`] to name a file next to the `.gltf`; it must read nothing else and stop at
    /// [`MAX_BUFFER_BYTES`]); a data uri is decoded here. Every refusal names its kind.
    pub fn open(bytes: &[u8], resolve: &mut dyn FnMut(&str) -> Result<Vec<u8>, String>) -> Result<Gltf, ReadError> {
        let format = |m: String| (RefusalKind::Format, m);
        if bytes.len() > MAX_BUFFER_BYTES {
            return Err(format(format!("the file is over {} MiB", MAX_BUFFER_BYTES >> 20)));
        }
        let is_glb = u32_at(bytes, 0) == Some(GLB_MAGIC);
        let mut g = if is_glb { Gltf::from_glb(bytes).map_err(format)? } else { Gltf::from_parts(bytes, &[]).map_err(format)? };
        if !g.json.is_object() {
            return Err(format("the JSON is not an object".to_string()));
        }
        let version = g.json.get("asset").and_then(|a| a.get("version")).and_then(Value::as_str).unwrap_or("");
        if !version.starts_with("2.") {
            return Err(format(format!("asset.version is {version:?}, not 2.0")));
        }
        if let Some(req) = g.json.get("extensionsRequired") {
            let Some(list) = req.as_array() else { return Err(format("extensionsRequired is not a list".to_string())) };
            let list: Vec<String> = list.iter().map(|x| x.as_str().unwrap_or("?").to_string()).collect();
            if !list.is_empty() {
                let what = if list.iter().any(|e| e.contains("draco") || e.contains("meshopt") || e.contains("quantization")) { "compressed or quantized data" } else { "extensions the import does not know" };
                return Err((RefusalKind::Extension, format!("the file requires {} ({what}); the import supports no extension", list.join(", "))));
            }
        }
        for key in ["nodes", "accessors", "bufferViews", "buffers", "meshes", "skins", "animations", "materials"] {
            if g.json.get(key).and_then(Value::as_array).is_some_and(|a| a.len() > MAX_LIST) {
                return Err(format(format!("{key} lists more than {MAX_LIST} entries")));
            }
        }
        let specs: Vec<Value> = g.array("buffers").to_vec();
        let mut total = 0usize;
        let mut loaded: Vec<Vec<u8>> = Vec::with_capacity(specs.len());
        for (i, b) in specs.iter().enumerate() {
            let len = b.get("byteLength").and_then(Value::as_u64).ok_or_else(|| format(format!("buffer {i} has no byteLength")))?;
            let len = usize::try_from(len).ok().filter(|l| *l > 0 && *l <= MAX_BUFFER_BYTES).ok_or_else(|| (RefusalKind::Buffer, format!("buffer {i} has the length {len}: a buffer holds 1 to {MAX_BUFFER_BYTES} bytes")))?;
            total = total.saturating_add(len);
            if total > MAX_BUFFER_BYTES * 2 {
                return Err((RefusalKind::Buffer, "the buffers are over 256 MiB together".to_string()));
            }
            let mut data = match b.get("uri").and_then(Value::as_str) {
                None if i == 0 && is_glb => std::mem::take(&mut g.bin),
                None => return Err((RefusalKind::Buffer, format!("buffer {i} has no uri and is not the .glb's own buffer"))),
                Some(uri) => match data_uri(uri, len)? {
                    Some(d) => d,
                    None => {
                        let name = buffer_name(uri)?;
                        resolve(&name).map_err(|m| (RefusalKind::Buffer, format!("buffer {i} ({name}): {m}")))?
                    }
                },
            };
            if data.len() < len {
                return Err((RefusalKind::Buffer, format!("buffer {i} holds {} bytes but its byteLength says {len}", data.len())));
            }
            data.truncate(len);
            loaded.push(data);
        }
        g.bin = if loaded.is_empty() { Vec::new() } else { loaded.remove(0) };
        g.more = loaded;
        Ok(g)
    }
}

/// A `data:` uri (base64) as bytes, `None` when `uri` is not one. `expect` is the buffer's byteLength: a longer body is refused before it is
/// decoded.
fn data_uri(uri: &str, expect: usize) -> Result<Option<Vec<u8>>, ReadError> {
    if !uri.get(..5).is_some_and(|h| h.eq_ignore_ascii_case("data:")) {
        return Ok(None);
    }
    let bad = |m: &str| Err((RefusalKind::Buffer, m.to_string()));
    let Some((meta, body)) = uri[5..].split_once(',') else { return bad("a data uri has no comma") };
    if !meta.to_ascii_lowercase().ends_with(";base64") {
        return bad("a data uri that is not base64 is not read");
    }
    // 4 characters hold 3 bytes
    if body.len() / 4 * 3 > expect + 3 {
        return bad("a data uri holds more bytes than the buffer's byteLength");
    }
    let mut out = Vec::with_capacity(expect + 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut pad = false;
    for c in body.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => {
                pad = true;
                continue;
            }
            _ => return bad("a data uri holds a character that is not base64"),
        };
        if pad {
            return bad("a data uri has base64 digits after its padding");
        }
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(Some(out))
}

/// The file name a buffer's `uri` asks for, or why it is refused: a URL, or a path outside the folder (an absolute path, a drive, `..`, a
/// separator, a stream name) is never read.
pub fn buffer_name(uri: &str) -> Result<String, ReadError> {
    let refuse = |m: String| Err((RefusalKind::Buffer, m));
    let outside = || format!("the buffer's uri {uri:?} is a path outside the file's folder (only a file next to it is read)");
    if uri.is_empty() {
        return refuse("the buffer's uri is empty".to_string());
    }
    let b = uri.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && !uri.contains("://") {
        return refuse(outside());
    }
    let scheme = uri.split_once(':').is_some_and(|(s, _)| s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) && s.bytes().all(|c| c.is_ascii_alphanumeric() || b"+-.".contains(&c)));
    if scheme || uri.starts_with("//") {
        return refuse(format!("the buffer's uri {uri:?} is a URL (nothing is fetched from the network)"));
    }
    let mut name = Vec::with_capacity(uri.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            match uri.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => name.push(v),
                None => return refuse(format!("the buffer's uri {uri:?} has a bad % escape")),
            }
            i += 3;
        } else {
            name.push(b[i]);
            i += 1;
        }
    }
    let Ok(name) = String::from_utf8(name) else { return refuse(format!("the buffer's uri {uri:?} is not UTF-8 text")) };
    if name.is_empty() || name.contains(['/', '\\', '\0', ':']) || name.chars().all(|c| c == '.') {
        return refuse(outside());
    }
    Ok(name)
}

/// Reads the file `name` (from [`buffer_name`]) in `folder` for [`Gltf::open`]: a plain file whose real place is inside `folder` (a link out
/// of it is refused), at most [`MAX_BUFFER_BYTES`].
pub fn read_beside(folder: &Path, name: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let (real, home) = match (std::fs::canonicalize(folder.join(name)), std::fs::canonicalize(folder)) {
        (Ok(r), Ok(h)) => (r, h),
        (Err(e), _) => return Err(format!("cannot be read: {e}")),
        (_, Err(e)) => return Err(format!("the folder cannot be read: {e}")),
    };
    if real.parent() != Some(home.as_path()) {
        return Err("is a link to a file outside the folder".to_string());
    }
    let f = std::fs::File::open(&real).map_err(|e| format!("cannot be read: {e}"))?;
    let meta = f.metadata().map_err(|e| format!("cannot be read: {e}"))?;
    if !meta.is_file() {
        return Err("is not a file".to_string());
    }
    if meta.len() > MAX_BUFFER_BYTES as u64 {
        return Err(format!("is over {} MiB", MAX_BUFFER_BYTES >> 20));
    }
    let mut out = Vec::new();
    f.take(MAX_BUFFER_BYTES as u64 + 1).read_to_end(&mut out).map_err(|e| format!("cannot be read: {e}"))?;
    if out.len() > MAX_BUFFER_BYTES {
        return Err(format!("is over {} MiB", MAX_BUFFER_BYTES >> 20));
    }
    Ok(out)
}
