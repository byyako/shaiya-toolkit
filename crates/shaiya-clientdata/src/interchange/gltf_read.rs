//! The smallest glTF reader the export's own check needs: a `.gltf` JSON with its buffer, or a `.glb`, and the accessor layer (component
//! types, strides, bounds checked before anything is allocated). No sparse accessors, no external files, no images: only what the toolkit
//! itself writes is read.

use serde_json::Value;

pub const GLB_MAGIC: u32 = 0x4654_6C67;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// A parsed file: the JSON and the one binary buffer.
#[derive(Clone, Debug)]
pub struct Gltf {
    pub json: Value,
    pub bin: Vec<u8>,
}

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
        Ok(Gltf { json, bin: bin.to_vec() })
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

    /// Reads accessor `idx`: bounds are checked against the view and the buffer before the values are allocated.
    pub fn accessor(&self, idx: usize) -> Result<Accessor, String> {
        let a = self.array("accessors").get(idx).ok_or_else(|| format!("accessor {idx} does not exist"))?;
        if a.get("sparse").is_some() {
            return Err(format!("accessor {idx} is sparse (not read here)"));
        }
        let ct = a.get("componentType").and_then(Value::as_u64).ok_or_else(|| format!("accessor {idx} has no componentType"))?;
        let csize = component_size(ct).ok_or_else(|| format!("accessor {idx} has the component type {ct}"))?;
        let width = a.get("type").and_then(Value::as_str).and_then(type_width).ok_or_else(|| format!("accessor {idx} has a bad type"))?;
        let count = a.get("count").and_then(Value::as_u64).ok_or_else(|| format!("accessor {idx} has no count"))? as usize;
        let view_idx = a.get("bufferView").and_then(Value::as_u64).ok_or_else(|| format!("accessor {idx} has no bufferView"))? as usize;
        let view = self.array("bufferViews").get(view_idx).ok_or_else(|| format!("accessor {idx}: bufferView {view_idx} does not exist"))?;
        let get = |v: &Value, k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as usize;
        let (voff, vlen) = (get(view, "byteOffset"), get(view, "byteLength"));
        if voff.checked_add(vlen).is_none_or(|e| e > self.bin.len()) {
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
                let b = &self.bin[at..at + csize];
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
}
