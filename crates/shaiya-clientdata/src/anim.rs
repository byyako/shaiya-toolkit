//! `.ANI` skeletal animations (`Character/<race>/ANI`, `Monster/ANI`, ...). Layout as observed in client files (all little-endian, no header):
//!
//! ```text
//! i32 first field (inferred: start key); i32 end key; u16 bone count
//! per bone: i32 parent (-1 for a root); f32 reference matrix[16];
//!           i32 nRot,   nRot   x { i32 key; f32 x, y, z, w }   (rotation keys)
//!           i32 nTrans, nTrans x { i32 key; f32 x, y, z }      (translation keys)
//! ```
//!
//! There are no bone names: a mesh is matched to the bones by index. Key times are in units of 1/30 second.
//!
//! Floats are kept exactly as stored (`f32::from_bits`, written with `to_bits`, so NaN payloads survive), which is what lets
//! [`Ani::to_bytes`] give back the bytes [`Ani::parse`] read. A file is read only when it ends exactly after the last bone.

use crate::error::{Cur, Error, Result};

/// A rotation key: a quaternion `(x, y, z, w)` at `key`.
#[derive(Clone, Debug, PartialEq)]
pub struct RotKey {
    pub key: i32,
    pub q: [f32; 4],
}

/// A translation key.
#[derive(Clone, Debug, PartialEq)]
pub struct TransKey {
    pub key: i32,
    pub v: [f32; 3],
}

/// One bone: its parent, its reference matrix (64 bytes, row-major) and its two key tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct AniBone {
    pub parent: i32,
    pub matrix: [f32; 16],
    pub rot: Vec<RotKey>,
    pub trans: Vec<TransKey>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ani {
    /// Inferred to be the start key (-1 in some files, 0 in all others on US EP4.5); never called part of the skeleton.
    pub first_field: i32,
    pub end_key: i32,
    pub bones: Vec<AniBone>,
}

/// The bone count a file's first ten bytes say (two `i32` fields, then the `u16` count), without reading the rest; `None` when `head` is
/// shorter. The file may still fail [`Ani::parse`].
pub fn bone_count_in_header(head: &[u8]) -> Option<usize> {
    head.get(8..10).map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
}

/// Smallest size of one bone in the file: parent, matrix, two counts.
const BONE_MIN: usize = 4 + 64 + 4 + 4;
pub(crate) const KEY_LIMIT: usize = 10_000_000;

/// True when a track of `n` keys is one the reader accepts (at most [`KEY_LIMIT`]).
pub(crate) fn track_fits(n: usize) -> bool {
    n <= KEY_LIMIT
}

impl Ani {
    /// Reads a whole `.ANI`; anything left over, or anything missing, is an error.
    pub fn parse(data: &[u8], file: &str) -> Result<Ani> {
        let mut r = Cur::new(data, file);
        let first_field = r.i32("first field")?;
        let end_key = r.i32("end key")?;
        let n = r.u16("bone count")? as usize;
        if n.saturating_mul(BONE_MIN) > r.left() {
            return Err(r.fail(format!("bone count {n} does not fit in the file")));
        }
        let mut bones = Vec::with_capacity(n);
        for _ in 0..n {
            let parent = r.i32("parent")?;
            let mut matrix = [0f32; 16];
            for m in matrix.iter_mut() {
                *m = r.f32("reference matrix")?;
            }
            let nrot = r.count(20, KEY_LIMIT, "rotation key count")?;
            let mut rot = Vec::with_capacity(nrot);
            for _ in 0..nrot {
                let key = r.i32("rotation key")?;
                rot.push(RotKey { key, q: [r.f32("rotation")?, r.f32("rotation")?, r.f32("rotation")?, r.f32("rotation")?] });
            }
            let ntrans = r.count(16, KEY_LIMIT, "translation key count")?;
            let mut trans = Vec::with_capacity(ntrans);
            for _ in 0..ntrans {
                let key = r.i32("translation key")?;
                trans.push(TransKey { key, v: r.f3("translation")? });
            }
            bones.push(AniBone { parent, matrix, rot, trans });
        }
        r.end("animation")?;
        Ok(Ani { first_field, end_key, bones })
    }

    /// The bytes of this animation; for a parsed file, exactly the bytes that were read.
    ///
    /// Panics when the animation cannot be written (more than 65,535 bones, a track longer than the reader accepts, or bytes that do not read back), which no parsed file can
    /// hold; use [`Ani::try_to_bytes`] for an animation built in memory.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.try_to_bytes().expect("animation cannot be written")
    }

    /// Like [`Ani::to_bytes`], but refuses an animation whose counts do not fit the file (never writes wrong bytes): more than 65,535 bones, a
    /// track longer than the reader accepts, or bytes that [`Ani::parse`] does not read back.
    pub fn try_to_bytes(&self) -> Result<Vec<u8>> {
        let too_big = |what: String| Error::Format { file: "animation".into(), what, offset: 0 };
        if self.bones.len() > u16::MAX as usize {
            return Err(too_big(format!("{} bones do not fit the 16-bit bone count", self.bones.len())));
        }
        for (i, b) in self.bones.iter().enumerate() {
            if !track_fits(b.rot.len()) || !track_fits(b.trans.len()) {
                return Err(too_big(format!("bone {i}: a track is longer than the {KEY_LIMIT} keys the reader accepts")));
            }
        }
        let (nr, nt) = self.key_counts();
        let mut d = Vec::with_capacity(10 + self.bones.len() * BONE_MIN + nr * 20 + nt * 16);
        d.extend_from_slice(&self.first_field.to_le_bytes());
        d.extend_from_slice(&self.end_key.to_le_bytes());
        d.extend_from_slice(&(self.bones.len() as u16).to_le_bytes());
        for b in &self.bones {
            d.extend_from_slice(&b.parent.to_le_bytes());
            for m in b.matrix {
                d.extend_from_slice(&m.to_bits().to_le_bytes());
            }
            d.extend_from_slice(&(b.rot.len() as i32).to_le_bytes());
            for k in &b.rot {
                d.extend_from_slice(&k.key.to_le_bytes());
                for c in k.q {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
            d.extend_from_slice(&(b.trans.len() as i32).to_le_bytes());
            for k in &b.trans {
                d.extend_from_slice(&k.key.to_le_bytes());
                for c in k.v {
                    d.extend_from_slice(&c.to_bits().to_le_bytes());
                }
            }
        }
        Ani::parse(&d, "animation").map_err(|e| too_big(format!("the bytes do not read back: {e}")))?;
        Ok(d)
    }

    /// The highest key of any track of any bone.
    pub fn last_key(&self) -> Option<i32> {
        self.bones.iter().flat_map(|b| b.rot.iter().map(|k| k.key).chain(b.trans.iter().map(|k| k.key))).max()
    }

    /// (rotation keys, translation keys) over all bones.
    pub fn key_counts(&self) -> (usize, usize) {
        (self.bones.iter().map(|b| b.rot.len()).sum(), self.bones.iter().map(|b| b.trans.len()).sum())
    }
}

// ---- validation -----------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// The codes of [`AniIssue`].
pub mod issue {
    // errors
    pub const PARENT_ORDER: &str = "parent-order";
    pub const KEYS_NOT_INCREASING: &str = "keys-not-increasing";
    pub const END_BEFORE_LAST_KEY: &str = "end-before-last-key";
    pub const KEY_NOT_FINITE: &str = "key-not-finite";
    // warnings
    pub const KEY_BEFORE_START: &str = "key-before-start";
    pub const TRACK_STARTS_LATE: &str = "track-starts-late";
    pub const KEYS_CLOSE: &str = "keys-close";
    pub const QUAT_NOT_UNIT: &str = "quat-not-unit";
    pub const MATRIX_NOT_FINITE: &str = "matrix-not-finite";
}

/// One finding of [`validate`]: one per (code, bone).
#[derive(Clone, Debug, PartialEq)]
pub struct AniIssue {
    pub code: &'static str,
    pub severity: Severity,
    pub bone: Option<usize>,
    pub message: String,
}

/// Checks an animation that already reads. Errors make a file unsafe to offer as a replacement; warnings are unusual but present in
/// files the game ships. A negative key is not an error: the first field says where a file starts.
pub fn validate(a: &Ani) -> Vec<AniIssue> {
    use issue::*;
    let mut out = Vec::new();
    let mut add = |code: &'static str, severity: Severity, bone: Option<usize>, message: String| out.push(AniIssue { code, severity, bone, message });

    for (i, b) in a.bones.iter().enumerate() {
        if b.parent != -1 && !(b.parent >= 0 && (b.parent as usize) < i) {
            add(PARENT_ORDER, Severity::Error, Some(i), format!("bone {i} names parent {}: a parent must be -1 or a bone before it", b.parent));
        }
        let rot: Vec<i32> = b.rot.iter().map(|k| k.key).collect();
        let trans: Vec<i32> = b.trans.iter().map(|k| k.key).collect();
        if !strictly_increasing(&rot) || !strictly_increasing(&trans) {
            add(KEYS_NOT_INCREASING, Severity::Error, Some(i), format!("bone {i}: the keys of a track must increase"));
        }
        if b.rot.iter().any(|k| k.q.iter().any(|c| !c.is_finite())) || b.trans.iter().any(|k| k.v.iter().any(|c| !c.is_finite())) {
            add(KEY_NOT_FINITE, Severity::Error, Some(i), format!("bone {i}: a key value is NaN or infinite"));
        }
        if rot.iter().chain(&trans).any(|&k| k < a.first_field) {
            add(KEY_BEFORE_START, Severity::Warning, Some(i), format!("bone {i}: a key is below the first field ({})", a.first_field));
        }
        if rot.first().is_some_and(|&k| k > 0) || trans.first().is_some_and(|&k| k > 0) {
            add(TRACK_STARTS_LATE, Severity::Warning, Some(i), format!("bone {i}: a track starts after key 0"));
        }
        if close(&rot) || close(&trans) {
            add(KEYS_CLOSE, Severity::Warning, Some(i), format!("bone {i}: two neighbouring keys are less than 2 apart"));
        }
        let off = |q: &[f32; 4]| q.iter().all(|c| c.is_finite()) && ((q.iter().map(|c| (*c as f64) * (*c as f64)).sum::<f64>().sqrt()) - 1.0).abs() > 1e-3;
        if b.rot.iter().any(|k| off(&k.q)) {
            add(QUAT_NOT_UNIT, Severity::Warning, Some(i), format!("bone {i}: a rotation key is not a unit quaternion"));
        }
        if b.matrix.iter().any(|c| !c.is_finite()) {
            add(MATRIX_NOT_FINITE, Severity::Warning, Some(i), format!("bone {i}: the reference matrix holds NaN or infinity"));
        }
    }
    if let Some(last) = a.last_key() {
        if a.end_key < last {
            add(END_BEFORE_LAST_KEY, Severity::Error, None, format!("the end key {} is below the last key {last}", a.end_key));
        }
    }
    out
}

fn strictly_increasing(k: &[i32]) -> bool {
    k.windows(2).all(|w| w[0] < w[1])
}

fn close(k: &[i32]) -> bool {
    k.windows(2).any(|w| w[0] < w[1] && (w[1] as i64 - w[0] as i64) < 2)
}

// ---- the rig check ----------------------------------------------------------------------------------------------------------------

/// One difference between a candidate and the animation it would replace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RigDiff {
    BoneCount { candidate: usize, target: usize },
    Parent { bone: usize, candidate: i32, target: i32 },
    /// The 64 bytes of the reference matrix differ in at least one bit.
    Matrix { bone: usize },
    /// The first field differs (inferred to be the start key; not part of the skeleton).
    FirstField { candidate: i32, target: i32 },
}

impl RigDiff {
    /// True for the differences that change the skeleton: the bone count, a parent, a reference matrix.
    pub fn is_skeleton(&self) -> bool {
        !matches!(self, RigDiff::FirstField { .. })
    }
}

/// The differences between `candidate` and `target`: bone count, parents and reference matrices (as raw bits; over the bones both have),
/// then the first field. Empty means the same skeleton and the same first field.
pub fn rig_check(candidate: &Ani, target: &Ani) -> Vec<RigDiff> {
    let mut out = Vec::new();
    if candidate.bones.len() != target.bones.len() {
        out.push(RigDiff::BoneCount { candidate: candidate.bones.len(), target: target.bones.len() });
    }
    for (i, (c, t)) in candidate.bones.iter().zip(&target.bones).enumerate() {
        if c.parent != t.parent {
            out.push(RigDiff::Parent { bone: i, candidate: c.parent, target: t.parent });
        }
        if c.matrix.iter().zip(&t.matrix).any(|(x, y)| x.to_bits() != y.to_bits()) {
            out.push(RigDiff::Matrix { bone: i });
        }
    }
    if candidate.first_field != target.first_field {
        out.push(RigDiff::FirstField { candidate: candidate.first_field, target: target.first_field });
    }
    out
}

/// True when no difference changes the skeleton (the first field may still differ).
pub fn same_skeleton(diffs: &[RigDiff]) -> bool {
    diffs.iter().all(|d| !d.is_skeleton())
}

/// A candidate passes when it has the same skeleton and the same first field as `target` and no validation error.
pub fn passes(candidate: &Ani, target: &Ani) -> bool {
    rig_check(candidate, target).is_empty() && !validate(candidate).iter().any(|i| i.severity == Severity::Error)
}

// ---- pose math --------------------------------------------------------------------------------------------------------------------
//
// The toolkit's own statement of the math (the preview is an approximation of the game's playback, validated offline):
//
// Matrices are 4 x 4, row-major `m[r * 4 + c]`, row vectors (`v' = v * M`), translation in `m[12..15]`, `A * B` = "A, then B".
//
// - A quaternion `q = (x, y, z, w)` is the rotation (row-vector form) with row 0 = `(1 - 2(yy + zz), 2(xy + zw), 2(xz - yw), 0)`, row 1 =
//   `(2(xy - zw), 1 - 2(xx + zz), 2(yz + xw), 0)`, row 2 = `(2(xz + yw), 2(yz - xw), 1 - 2(xx + yy), 0)`, row 3 = `(0, 0, 0, 1)`.
// - `Bind[i]` is the reference matrix of bone `i`. `Local[i] = Bind[i]` for a root (parent -1), else `Bind[i] * inverse(Bind[parent])`
//   (general 4 x 4 inverse; a singular or non-finite matrix gives all NaN).
// - The pose at time `t` (key units), bones in file order: start from `L = Local[i]`; with rotation keys the upper 3 x 3 of `L` becomes the
//   rotation of the sampled quaternion (row 3 kept); with translation keys `L[12..15]` becomes the sampled vector. Sampling is plain:
//   before the first key the first key's value, at or after the last key the last, else the keys `k`, `k + 1` with `key[k] <= t <
//   key[k + 1]` and `f = (t - key[k]) / (key[k + 1] - key[k])`; rotations are slerped the short way (the second quaternion is negated
//   when the dot product is negative; a normalised linear blend when the angle is under 1e-4), translations are linear.
//   `World[i] = L` for a root, else `L * World[parent]`.
// - Skinning a `.3DC` vertex lives in `render::skin`: `M = w * (B[b1] * World[b1]) + (1 - w) * (B[b2] * World[b2])`, `pos' = (pos, 1) * M`.

/// A 4 x 4 matrix, row-major, for row vectors.
pub type Mat4 = [f32; 16];

pub const IDENTITY: Mat4 = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `a * b` ("a, then b"), row-major.
pub fn mat_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    o
}

/// The general 4 x 4 inverse (Gauss-Jordan in f64). A singular or non-finite matrix gives sixteen NaN.
pub fn mat_inverse(m: &Mat4) -> Mat4 {
    let nan = [f32::NAN; 16];
    if m.iter().any(|c| !c.is_finite()) {
        return nan;
    }
    let mut a = [[0f64; 8]; 4];
    for r in 0..4 {
        for c in 0..4 {
            a[r][c] = m[r * 4 + c] as f64;
        }
        a[r][4 + r] = 1.0;
    }
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs())).unwrap_or(col);
        if a[pivot][col] == 0.0 || !a[pivot][col].is_finite() {
            return nan;
        }
        a.swap(col, pivot);
        let d = a[col][col];
        for v in a[col].iter_mut() {
            *v /= d;
        }
        for r in 0..4 {
            if r != col {
                let f = a[r][col];
                if f != 0.0 {
                    let pivot_row = a[col];
                    for (v, p) in a[r].iter_mut().zip(pivot_row) {
                        *v -= f * p;
                    }
                }
            }
        }
    }
    let mut o = [0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = a[r][4 + c] as f32;
        }
    }
    if o.iter().any(|c| !c.is_finite()) {
        return nan;
    }
    o
}

/// The rotation matrix (row-vector form) of the quaternion `(x, y, z, w)`.
pub fn quat_matrix(q: [f32; 4]) -> Mat4 {
    let [x, y, z, w] = q;
    [
        1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + z * w), 2.0 * (x * z - y * w), 0.0,
        2.0 * (x * y - z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + x * w), 0.0,
        2.0 * (x * z + y * w), 2.0 * (y * z - x * w), 1.0 - 2.0 * (x * x + y * y), 0.0,
        0.0, 0.0, 0.0, 1.0,
    ]
}

/// `Local[i]` of every bone: the reference matrix for a root, else `Bind[i] * inverse(Bind[parent])`. A parent that is not an earlier bone is
/// treated as no parent (such a file fails [`validate`]).
pub fn locals(ani: &Ani) -> Vec<Mat4> {
    (0..ani.bones.len())
        .map(|i| {
            let b = &ani.bones[i];
            match usize::try_from(b.parent).ok().filter(|&p| p < i) {
                Some(p) => mat_mul(&b.matrix, &mat_inverse(&ani.bones[p].matrix)),
                None => b.matrix,
            }
        })
        .collect()
}

pub(crate) fn slerp(a: [f32; 4], b: [f32; 4], f: f32) -> [f32; 4] {
    let mut dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let mut b = b;
    if dot < 0.0 {
        b = b.map(|c| -c);
        dot = -dot;
    }
    let angle = dot.clamp(-1.0, 1.0).acos();
    let mut q = [0f32; 4];
    if angle < 1e-4 {
        for i in 0..4 {
            q[i] = a[i] + (b[i] - a[i]) * f;
        }
        let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
        return if n > 0.0 { q.map(|c| c / n) } else { a };
    }
    let s = angle.sin();
    let (wa, wb) = (((1.0 - f) * angle).sin() / s, (f * angle).sin() / s);
    for i in 0..4 {
        q[i] = a[i] * wa + b[i] * wb;
    }
    q
}

/// Where `t` falls in a track of `n >= 1` keys: `Ok((k, f))` between keys `k` and `k + 1`, `Err(k)` when key `k` holds (before the first
/// key, at or after the last, or two equal keys).
pub(crate) fn segment(key: impl Fn(usize) -> i32, n: usize, t: f32) -> std::result::Result<(usize, f32), usize> {
    if t <= key(0) as f32 {
        return Err(0);
    }
    if t >= key(n - 1) as f32 {
        return Err(n - 1);
    }
    let (mut lo, mut hi) = (0usize, n - 1);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if key(mid) as f32 <= t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (a, b) = (key(lo) as f32, key(hi) as f32);
    if b > a {
        Ok((lo, (t - a) / (b - a)))
    } else {
        Err(lo)
    }
}

/// The pose at time `t` (key units): `World[i]` of every bone, bones in file order. `locals` is [`locals`]`(ani)` (computed once per animation).
pub fn pose(ani: &Ani, locals: &[Mat4], t: f32) -> Vec<Mat4> {
    let mut world: Vec<Mat4> = Vec::with_capacity(ani.bones.len());
    for (i, b) in ani.bones.iter().enumerate() {
        let mut l = locals.get(i).copied().unwrap_or(IDENTITY);
        if !b.rot.is_empty() {
            let q = match segment(|k| b.rot[k].key, b.rot.len(), t) {
                Err(k) => b.rot[k].q,
                Ok((k, f)) => slerp(b.rot[k].q, b.rot[k + 1].q, f),
            };
            let r = quat_matrix(q);
            for row in 0..3 {
                for col in 0..3 {
                    l[row * 4 + col] = r[row * 4 + col];
                }
            }
        }
        if !b.trans.is_empty() {
            let v = match segment(|k| b.trans[k].key, b.trans.len(), t) {
                Err(k) => b.trans[k].v,
                Ok((k, f)) => {
                    let (a, c) = (b.trans[k].v, b.trans[k + 1].v);
                    [a[0] + (c[0] - a[0]) * f, a[1] + (c[1] - a[1]) * f, a[2] + (c[2] - a[2]) * f]
                }
            };
            l[12..15].copy_from_slice(&v);
        }
        let w = match usize::try_from(b.parent).ok().filter(|&p| p < i) {
            Some(p) => mat_mul(&l, &world[p]),
            None => l,
        };
        world.push(w);
    }
    world
}

// ---- the pose difference ----------------------------------------------------------------------------------------------------------

/// Most strided keys [`diff`] samples: a longer range is sampled with a stride so that no more than this many poses are computed per
/// animation (the end key is always added, so at most one more).
pub const DIFF_SAMPLES_MAX: i64 = 100_000;
/// The work budget of [`diff`]: sampled keys times compared bones stay at or under this (about 2e7 bone poses) by a larger stride, so a file
/// with many bones is sampled at fewer keys (never fewer than 2 strided keys plus the end key).
pub const DIFF_WORK_MAX: i64 = 20_000_000;
/// A bone differs when its world rotation turns by more than this many degrees at some sampled key (below it is float noise).
pub const DIFF_ANGLE_EPSILON_DEG: f32 = 1e-3;
/// A bone differs when its world position moves by more than this many units at some sampled key.
pub const DIFF_OFFSET_EPSILON: f32 = 1e-4;

/// How one bone's world pose differs between two animations over the sampled keys.
#[derive(Clone, Debug, PartialEq)]
pub struct BoneDiff {
    pub bone: usize,
    /// The largest turn between the two world rotations at any sampled key, in degrees (infinite when a pose is not finite on one side only).
    pub max_angle_deg: f32,
    /// The largest distance between the two world positions at any sampled key, in file units (infinite as above).
    pub max_offset: f32,
    /// Rotation plus translation keys of the bone in the first and in the second file.
    pub keys: (usize, usize),
}

/// What differs between two animations: the skeleton and first field ([`rig_check`] with the first as candidate), the end keys and the
/// bones whose poses differ, largest turn first.
#[derive(Clone, Debug, PartialEq)]
pub struct AniDiff {
    pub skeleton: Vec<RigDiff>,
    pub end_key: (i32, i32),
    pub bones: Vec<BoneDiff>,
}

impl AniDiff {
    /// True when nothing differs: the same skeleton and first field, the same end key and the same poses at every sampled key.
    pub fn is_identical(&self) -> bool {
        self.skeleton.is_empty() && self.end_key.0 == self.end_key.1 && self.bones.is_empty()
    }
}

fn unit_rows(m: &Mat4) -> [[f32; 3]; 3] {
    let mut r = [[0f32; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        let v = [m[i * 4], m[i * 4 + 1], m[i * 4 + 2]];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        *row = if n > 0.0 { v.map(|c| c / n) } else { v };
    }
    r
}

/// The turn between the rotations of two world matrices (rows normalised, so a scale does not count) in degrees, and the distance between
/// their positions. Equal bits give (0, 0); a non-finite value gives infinity.
fn pose_distance(a: &Mat4, b: &Mat4) -> (f32, f32) {
    if a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()) {
        return (0.0, 0.0);
    }
    if !a.iter().chain(b).all(|v| v.is_finite()) {
        return (f32::INFINITY, f32::INFINITY);
    }
    let (ra, rb) = (unit_rows(a), unit_rows(b));
    // M = Ra * Rb^T, in f64 so a tiny turn is not lost to rounding
    let mut m = [[0f64; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| f64::from(ra[i][k]) * f64::from(rb[j][k])).sum();
        }
    }
    let axis = [m[1][2] - m[2][1], m[2][0] - m[0][2], m[0][1] - m[1][0]];
    let sin2 = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let cos2 = m[0][0] + m[1][1] + m[2][2] - 1.0;
    let angle = sin2.atan2(cos2).to_degrees() as f32;
    let d = [a[12] - b[12], a[13] - b[13], a[14] - b[14]];
    (angle, (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt())
}

/// Compares two animations: [`rig_check`] (`a` as candidate), both end keys, and for every bone both files have the world pose from [`pose`]
/// at every integer key from the smaller first field (or 0) to the larger end key (with a stride over [`DIFF_SAMPLES_MAX`] keys, and over
/// [`DIFF_WORK_MAX`] keys times bones; the end key is always sampled). A bone is
/// listed when its pose differs by more than [`DIFF_ANGLE_EPSILON_DEG`] or [`DIFF_OFFSET_EPSILON`] at some sampled key, largest turn first
/// (then largest offset, then bone index). Swapping the files swaps the pairs and keeps every number.
pub fn diff(a: &Ani, b: &Ani) -> AniDiff {
    diff_with(a, b, &mut || false).expect("a diff that is never cancelled ends")
}

/// [`diff`] with a cancel check once per sampled key: `cancel` returning true ends it with `None`.
pub fn diff_with(a: &Ani, b: &Ani, cancel: &mut dyn FnMut() -> bool) -> Option<AniDiff> {
    let skeleton = rig_check(a, b);
    let start = i64::from(a.first_field.min(b.first_field).min(0));
    let end = i64::from(a.end_key.max(b.end_key)).max(start);
    let n = a.bones.len().min(b.bones.len());
    let samples = DIFF_SAMPLES_MAX.min((DIFF_WORK_MAX / (n.max(1) as i64)).max(2));
    let stride = ((end - start + 1) / samples).max(0) + 1;
    let (la, lb) = (locals(a), locals(b));
    let mut max: Vec<(f32, f32)> = vec![(0.0, 0.0); n];
    let mut key = start;
    loop {
        if cancel() {
            return None;
        }
        let (pa, pb) = (pose(a, &la, key as f32), pose(b, &lb, key as f32));
        for (i, m) in max.iter_mut().enumerate() {
            let (angle, offset) = pose_distance(&pa[i], &pb[i]);
            m.0 = m.0.max(angle);
            m.1 = m.1.max(offset);
        }
        if key >= end {
            break;
        }
        key = (key + stride).min(end);
    }
    let count = |b: &AniBone| b.rot.len() + b.trans.len();
    let mut bones: Vec<BoneDiff> = max
        .into_iter()
        .enumerate()
        .filter(|(_, (angle, offset))| *angle > DIFF_ANGLE_EPSILON_DEG || *offset > DIFF_OFFSET_EPSILON)
        .map(|(bone, (max_angle_deg, max_offset))| BoneDiff { bone, max_angle_deg, max_offset, keys: (count(&a.bones[bone]), count(&b.bones[bone])) })
        .collect();
    bones.sort_by(|x, y| y.max_angle_deg.total_cmp(&x.max_angle_deg).then(y.max_offset.total_cmp(&x.max_offset)).then(x.bone.cmp(&y.bone)));
    Some(AniDiff { skeleton, end_key: (a.end_key, b.end_key), bones })
}

// ---- kinds and rigs -----------------------------------------------------------------------------------------------------------------

/// The 16 character rigs: the file-name prefix and the race folder it lives in.
pub const RIGS: [(&str, &str); 16] = [
    ("humf", "Human"),
    ("humm", "Human"),
    ("huwf", "Human"),
    ("huwm", "Human"),
    ("elmr", "Elf"),
    ("elmm", "Elf"),
    ("elwr", "Elf"),
    ("elwm", "Elf"),
    ("vimr", "Vile"),
    ("vimm", "Vile"),
    ("viwr", "Vile"),
    ("viwm", "Vile"),
    ("demf", "DeathEater"),
    ("demr", "DeathEater"),
    ("dewf", "DeathEater"),
    ("dewr", "DeathEater"),
];

/// What an archive path says about an animation: its kind (the first folder as stored) and, for a character animation, its rig.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RigInfo {
    pub kind: String,
    /// (rig prefix, race folder) from [`RIGS`].
    pub rig: Option<(&'static str, &'static str)>,
}

/// Names the kind and the rig of `Character/<race>/ANI/<rig>_...` by the path only (any letter case); the rig check never uses this.
pub fn rig_of(archive_path: &str) -> RigInfo {
    let segs: Vec<&str> = archive_path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let kind = segs.first().map(|s| s.to_string()).unwrap_or_default();
    let mut rig = None;
    if segs.len() == 4 && segs[0].eq_ignore_ascii_case("character") && segs[2].eq_ignore_ascii_case("ani") {
        let prefix = segs[3].split('_').next().unwrap_or("");
        if segs[3].contains('_') {
            rig = RIGS.iter().find(|(name, _)| name.eq_ignore_ascii_case(prefix)).copied();
        }
    }
    RigInfo { kind, rig }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shaiya_testkit::anim::{arm_ani, identity_bits, translation_bits, ani_bytes, BoneSpec};

    const Q_ID: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn bone(parent: i32, matrix: [u32; 16]) -> BoneSpec {
        BoneSpec { parent, matrix, rot: Vec::new(), trans: Vec::new() }
    }

    fn parse(d: &[u8]) -> Result<Ani> {
        Ani::parse(d, "t.ani")
    }

    fn codes(a: &Ani) -> Vec<&'static str> {
        validate(a).iter().filter(|i| i.severity == Severity::Error).map(|i| i.code).collect()
    }

    #[test]
    fn parses_the_arm_and_writes_it_back_byte_for_byte() {
        let d = arm_ani(30);
        let a = parse(&d).unwrap();
        assert_eq!((a.first_field, a.end_key, a.bones.len()), (0, 30, 3));
        assert_eq!(a.bones.iter().map(|b| b.parent).collect::<Vec<_>>(), vec![-1, 0, 1]);
        assert_eq!(a.bones[1].matrix[13], 1.0);
        assert_eq!(a.bones[2].matrix[13], 2.0);
        assert_eq!(a.key_counts(), (2, 0));
        assert_eq!(a.last_key(), Some(30));
        assert_eq!(a.to_bytes(), d);
    }

    #[test]
    fn every_truncation_and_one_more_byte_are_errors() {
        let d = arm_ani(30);
        let mut longer = d.clone();
        longer.push(0);
        assert!(parse(&longer).is_err());
        for n in 0..d.len() {
            assert!(parse(&d[..n]).is_err(), "prefix of {n} bytes");
        }
    }

    #[test]
    fn nan_bits_survive_the_round_trip() {
        let mut m = identity_bits();
        m[3] = 0xFFC0_0000;
        m[7] = 0x7FA0_0001;
        let d = ani_bytes(0, 0, &[bone(-1, m)]);
        let a = parse(&d).unwrap();
        assert!(a.bones[0].matrix[3].is_nan() && a.bones[0].matrix[7].is_nan());
        assert_eq!(a.bones[0].matrix[3].to_bits(), 0xFFC0_0000);
        assert_eq!(a.bones[0].matrix[7].to_bits(), 0x7FA0_0001);
        assert_eq!(a.to_bytes(), d);
    }

    #[test]
    fn track_fits_refuses_more_than_key_limit() {
        assert!(track_fits(0) && track_fits(KEY_LIMIT));
        assert!(!track_fits(KEY_LIMIT + 1) && !track_fits(usize::MAX));
    }

    #[test]
    fn an_animation_with_too_many_bones_is_refused_not_truncated() {
        let bone = AniBone { parent: -1, matrix: [0.0; 16], rot: Vec::new(), trans: Vec::new() };
        let mut a = Ani { first_field: 0, end_key: 0, bones: vec![bone; u16::MAX as usize] };
        assert_eq!(parse(&a.try_to_bytes().unwrap()).unwrap().bones.len(), u16::MAX as usize);
        a.bones.push(a.bones[0].clone());
        let e = a.try_to_bytes().unwrap_err().to_string();
        assert!(e.contains("65536 bones"), "{e}");
    }

    #[test]
    fn a_count_that_cannot_fit_is_refused_without_allocating() {
        let mut d = ani_bytes(0, 0, &[bone(-1, identity_bits())]);
        // the rotation count sits after first field, end key, bone count, parent and the matrix
        let at = 4 + 4 + 2 + 4 + 64;
        d[at..at + 4].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(parse(&d).is_err());
        d[at..at + 4].copy_from_slice(&(-1i32).to_le_bytes());
        assert!(parse(&d).is_err());
        // a bone count that cannot fit
        let mut e = ani_bytes(0, 0, &[]);
        e[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(parse(&e).is_err());
    }

    fn one(rot: &[(i32, [f32; 4])], trans: &[(i32, [f32; 3])]) -> Ani {
        parse(&ani_bytes(0, 100, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: rot.to_vec(), trans: trans.to_vec() }])).unwrap()
    }

    #[test]
    fn validate_finds_each_error_code() {
        // parent-order: a parent that is not before the bone, and one that is below -1
        let a = parse(&ani_bytes(0, 0, &[bone(-1, identity_bits()), bone(1, identity_bits())])).unwrap();
        assert_eq!(codes(&a), vec!["parent-order"]);
        let a = parse(&ani_bytes(0, 0, &[bone(-2, identity_bits())])).unwrap();
        assert_eq!(codes(&a), vec!["parent-order"]);
        let a = parse(&ani_bytes(0, 0, &[bone(5, identity_bits())])).unwrap();
        assert_eq!(codes(&a), vec!["parent-order"]);
        // keys-not-increasing, in either track
        assert_eq!(codes(&one(&[(10, Q_ID), (10, Q_ID)], &[])), vec!["keys-not-increasing"]);
        assert_eq!(codes(&one(&[], &[(10, [0.0; 3]), (5, [0.0; 3])])), vec!["keys-not-increasing"]);
        // end-before-last-key
        let a = parse(&ani_bytes(0, 5, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![(0, Q_ID), (9, Q_ID)], trans: vec![] }])).unwrap();
        assert_eq!(codes(&a), vec!["end-before-last-key"]);
        assert_eq!(validate(&a)[0].bone, None);
        // key-not-finite (and quat-not-unit is not reported for it)
        assert_eq!(codes(&one(&[(0, [f32::NAN, 0.0, 0.0, 1.0])], &[])), vec!["key-not-finite"]);
        assert_eq!(codes(&one(&[], &[(0, [f32::INFINITY, 0.0, 0.0])])), vec!["key-not-finite"]);
        for i in validate(&one(&[(0, [f32::NAN, 0.0, 0.0, 1.0])], &[])) {
            assert_eq!(i.severity, Severity::Error);
        }
    }

    #[test]
    fn validate_finds_each_warning_code() {
        let warn = |a: &Ani| -> Vec<&'static str> {
            let v = validate(a);
            assert!(v.iter().all(|i| i.severity == Severity::Warning), "{v:?}");
            v.iter().map(|i| i.code).collect()
        };
        assert_eq!(warn(&one(&[(5, Q_ID), (20, Q_ID)], &[])), vec!["track-starts-late"]);
        assert_eq!(warn(&one(&[(0, Q_ID), (1, Q_ID)], &[])), vec!["keys-close"]);
        assert_eq!(warn(&one(&[(0, [0.0, 0.0, 0.0, 2.0])], &[])), vec!["quat-not-unit"]);
        let mut m = identity_bits();
        m[0] = 0xFFC0_0000;
        let a = parse(&ani_bytes(0, 0, &[bone(-1, m)])).unwrap();
        assert_eq!(warn(&a), vec!["matrix-not-finite"]);
        // a key below the first field
        let a = parse(&ani_bytes(3, 100, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![(2, Q_ID), (50, Q_ID)], trans: vec![] }])).unwrap();
        assert_eq!(warn(&a), vec!["key-before-start", "track-starts-late"]);
        // one issue per code and bone, however many tracks and keys offend
        let a = one(&[(5, Q_ID), (6, Q_ID), (7, Q_ID)], &[(5, [0.0; 3]), (6, [0.0; 3])]);
        assert_eq!(warn(&a), vec!["track-starts-late", "keys-close"]);
    }

    #[test]
    fn the_stock_like_arm_has_no_issue() {
        assert!(validate(&parse(&arm_ani(30)).unwrap()).is_empty());
        assert!(validate(&parse(&arm_ani(2)).unwrap()).is_empty());
    }

    #[test]
    fn a_track_starting_at_minus_one_with_first_field_minus_one_has_no_issue() {
        let d = ani_bytes(-1, 30, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![(-1, Q_ID), (30, Q_ID)], trans: vec![(-1, [0.0; 3]), (30, [1.0, 0.0, 0.0])] }]);
        let a = parse(&d).unwrap();
        assert!(validate(&a).is_empty());
        assert_eq!(a.last_key(), Some(30));
    }

    #[test]
    fn a_key_below_the_first_field_warns() {
        let d = ani_bytes(0, 30, &[BoneSpec { parent: -1, matrix: identity_bits(), rot: vec![(-1, Q_ID), (30, Q_ID)], trans: vec![] }]);
        let v = validate(&parse(&d).unwrap());
        assert_eq!(v.iter().map(|i| i.code).collect::<Vec<_>>(), vec!["key-before-start"]);
        assert_eq!(v[0].severity, Severity::Warning);
        assert_eq!(v[0].bone, Some(0));
    }

    #[test]
    fn rig_check_is_empty_for_the_same_skeleton_with_other_keys() {
        let a = parse(&arm_ani(30)).unwrap();
        let mut b = parse(&arm_ani(60)).unwrap();
        b.bones[2].rot.push(RotKey { key: 7, q: Q_ID });
        assert!(rig_check(&a, &b).is_empty());
        assert!(rig_check(&a, &a).is_empty());
        assert!(same_skeleton(&rig_check(&a, &b)));
    }

    #[test]
    fn rig_check_names_first_field_bone_count_parent_and_matrix() {
        let a = parse(&arm_ani(30)).unwrap();
        let mut c = a.clone();
        c.first_field = -1;
        let d = rig_check(&c, &a);
        assert_eq!(d, vec![RigDiff::FirstField { candidate: -1, target: 0 }]);
        assert!(!d[0].is_skeleton());
        assert!(same_skeleton(&d));
        let mut c = a.clone();
        c.bones.pop();
        assert_eq!(rig_check(&c, &a), vec![RigDiff::BoneCount { candidate: 2, target: 3 }]);
        assert_eq!(rig_check(&a, &c), vec![RigDiff::BoneCount { candidate: 3, target: 2 }]);
        let mut c = a.clone();
        c.bones[2].parent = 0;
        assert_eq!(rig_check(&c, &a), vec![RigDiff::Parent { bone: 2, candidate: 0, target: 1 }]);
        assert!(!same_skeleton(&rig_check(&c, &a)));
        // one bit of one matrix element
        let mut c = a.clone();
        c.bones[1].matrix[13] = f32::from_bits(c.bones[1].matrix[13].to_bits() ^ 1);
        assert_eq!(rig_check(&c, &a), vec![RigDiff::Matrix { bone: 1 }]);
        // the same NaN bits are equal, other NaN bits are not
        let mut m = identity_bits();
        m[5] = 0xFFC0_0000;
        let n1 = parse(&ani_bytes(0, 0, &[bone(-1, m)])).unwrap();
        assert!(rig_check(&n1, &n1.clone()).is_empty());
        m[5] = 0x7FC0_0000;
        let n2 = parse(&ani_bytes(0, 0, &[bone(-1, m)])).unwrap();
        assert_eq!(rig_check(&n2, &n1), vec![RigDiff::Matrix { bone: 0 }]);
        // diffs are reported in the order: count, then per bone parent and matrix, then first field
        let mut c = a.clone();
        c.bones.pop();
        c.bones[0].matrix[0] = 2.0;
        c.first_field = 4;
        assert_eq!(
            rig_check(&c, &a),
            vec![RigDiff::BoneCount { candidate: 2, target: 3 }, RigDiff::Matrix { bone: 0 }, RigDiff::FirstField { candidate: 4, target: 0 }]
        );
    }

    #[test]
    fn passes_needs_same_skeleton_same_first_field_and_no_error() {
        let target = parse(&arm_ani(30)).unwrap();
        assert!(passes(&parse(&arm_ani(90)).unwrap(), &target));
        // a different first field
        let mut c = parse(&arm_ani(90)).unwrap();
        c.first_field = -1;
        assert!(!passes(&c, &target));
        // a different skeleton
        let mut c = parse(&arm_ani(90)).unwrap();
        c.bones[1].matrix[12] = 5.0;
        assert!(!passes(&c, &target));
        // an error in the candidate
        let mut c = parse(&arm_ani(90)).unwrap();
        c.end_key = 10;
        assert!(!passes(&c, &target));
        // a warning alone does not stop it
        let mut c = parse(&arm_ani(90)).unwrap();
        c.bones[0].rot.push(RotKey { key: 91, q: [0.0, 0.0, 0.0, 2.0] });
        c.end_key = 91;
        assert!(passes(&c, &target));
        // the skeleton of translation helpers: a pure translation arm still matches itself
        let t = parse(&ani_bytes(0, 0, &[bone(-1, translation_bits(1.0, 2.0, 3.0))])).unwrap();
        assert!(passes(&t, &t.clone()));
    }

    #[test]
    fn rig_of_names_the_16_rigs_and_kinds() {
        let r = rig_of("Character/Human/ANI/huwm_117_x.ANI");
        assert_eq!((r.kind.as_str(), r.rig), ("Character", Some(("huwm", "Human"))));
        let r = rig_of("character/human/ani/HUWM_117_x.ani");
        assert_eq!((r.kind.as_str(), r.rig), ("character", Some(("huwm", "Human"))));
        let r = rig_of("Monster/ANI/Mob_X_Att1.ANI");
        assert_eq!((r.kind.as_str(), r.rig), ("Monster", None));
        assert_eq!(rig_of("Character/Human/ANI/xxxx_1.ANI").rig, None);
        assert_eq!(rig_of("Character/Human/ANI/huwm.ANI").rig, None);
        assert_eq!(rig_of("Character/Human/ANI/huwmx_1.ANI").rig, None);
        assert_eq!(rig_of(r"Character\DeathEater\ANI\dewr_1_a.ANI").rig, Some(("dewr", "DeathEater")));
        assert_eq!(rig_of("").kind, "");
        for (name, race) in RIGS {
            let path = format!("Character/{race}/ANI/{name}_001_a.ANI");
            assert_eq!(rig_of(&path).rig, Some((name, race)), "{path}");
        }
    }
}

#[cfg(test)]
mod pose_tests {
    use super::*;
    use shaiya_testkit::anim::{ani_bytes, identity_bits, translation_bits, BoneSpec};

    fn near(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    fn build(end_key: i32, bones: &[BoneSpec]) -> Ani {
        Ani::parse(&ani_bytes(0, end_key, bones), "t.ani").unwrap()
    }

    fn spec(parent: i32, matrix: [u32; 16], rot: Vec<(i32, [f32; 4])>, trans: Vec<(i32, [f32; 3])>) -> BoneSpec {
        BoneSpec { parent, matrix, rot, trans }
    }

    /// The quaternion of a turn of `deg` degrees about the unit axis `(x, y, z)`.
    fn turn(deg: f32, axis: [f32; 3]) -> [f32; 4] {
        let (s, c) = (deg.to_radians() * 0.5).sin_cos();
        [axis[0] * s, axis[1] * s, axis[2] * s, c]
    }

    /// The first row of the rotation of a root bone with rotation keys at time `t`.
    fn root_row0(keys: Vec<(i32, [f32; 4])>, t: f32) -> [f32; 3] {
        let a = build(100, &[spec(-1, identity_bits(), keys, Vec::new())]);
        let w = pose(&a, &locals(&a), t);
        [w[0][0], w[0][1], w[0][2]]
    }

    #[test]
    fn quat_matrix_turns_x_to_minus_z_for_90_degrees_about_y() {
        let m = quat_matrix(turn(90.0, [0.0, 1.0, 0.0]));
        // row vector: (1, 0, 0) * M is row 0
        assert!(near(m[0], 0.0, 1e-6) && near(m[1], 0.0, 1e-6) && near(m[2], -1.0, 1e-6), "{:?}", &m[0..3]);
    }

    #[test]
    fn rotation_halfway_between_keys_is_45_degrees() {
        let r = root_row0(vec![(0, turn(0.0, [0.0, 0.0, 1.0])), (10, turn(90.0, [0.0, 0.0, 1.0]))], 5.0);
        let (s, c) = 45f32.to_radians().sin_cos();
        assert!(near(r[0], c, 1e-5) && near(r[1], s, 1e-5) && near(r[2], 0.0, 1e-5), "{r:?}");
    }

    #[test]
    fn before_the_first_key_and_after_the_last_the_end_keys_hold() {
        let keys = vec![(10, turn(30.0, [0.0, 0.0, 1.0])), (20, turn(90.0, [0.0, 0.0, 1.0]))];
        let (first, last) = (root_row0(keys.clone(), 10.0), root_row0(keys.clone(), 20.0));
        for t in [-5.0, 0.0, 9.9] {
            assert_eq!(root_row0(keys.clone(), t), first, "t = {t}");
        }
        for t in [20.0, 25.0, 1000.0] {
            assert_eq!(root_row0(keys.clone(), t), last, "t = {t}");
        }
        assert!(near(first[1], 30f32.to_radians().sin(), 1e-5) && near(last[1], 1.0, 1e-5));
        // a translation track holds its end keys too
        let a = build(100, &[spec(-1, identity_bits(), Vec::new(), vec![(10, [1.0, 0.0, 0.0]), (20, [3.0, 0.0, 0.0])])]);
        let l = locals(&a);
        assert_eq!(pose(&a, &l, 0.0)[0][12], 1.0);
        assert_eq!(pose(&a, &l, 15.0)[0][12], 2.0);
        assert_eq!(pose(&a, &l, 99.0)[0][12], 3.0);
    }

    #[test]
    fn short_way_slerp_for_opposite_signs() {
        // the second key is the 45 degree turn written with every sign flipped: the short way from 0 to 45 degrees goes through 22.5
        let q = turn(45.0, [0.0, 0.0, 1.0]);
        let r = root_row0(vec![(0, [0.0, 0.0, 0.0, 1.0]), (10, q.map(|c| -c))], 5.0);
        let (s, c) = 22.5f32.to_radians().sin_cos();
        assert!(near(r[0], c, 1e-5) && near(r[1], s, 1e-5), "{r:?}");
    }

    #[test]
    fn a_child_follows_its_parent() {
        let a = build(
            10,
            &[
                spec(-1, identity_bits(), Vec::new(), vec![(0, [0.0, 2.0, 0.0])]),
                spec(0, translation_bits(0.0, 1.0, 0.0), Vec::new(), Vec::new()),
            ],
        );
        let l = locals(&a);
        assert!(near(l[1][13], 1.0, 1e-6), "child local = bind * inverse(parent bind)");
        let w = pose(&a, &l, 0.0);
        assert!(near(w[1][12], 0.0, 1e-6) && near(w[1][13], 3.0, 1e-6) && near(w[1][14], 0.0, 1e-6), "{:?}", &w[1][12..15]);
    }

    #[test]
    fn the_inverse_undoes_a_matrix_and_a_singular_one_is_nan() {
        let m = mat_mul(&quat_matrix(turn(40.0, [0.0, 1.0, 0.0])), &{
            let mut t = IDENTITY;
            t[12..15].copy_from_slice(&[1.0, 2.0, 3.0]);
            t
        });
        let id = mat_mul(&m, &mat_inverse(&m));
        for (i, v) in id.iter().enumerate() {
            assert!(near(*v, IDENTITY[i], 1e-5), "{id:?}");
        }
        assert!(mat_inverse(&[0.0; 16]).iter().all(|c| c.is_nan()));
        let mut bad = IDENTITY;
        bad[3] = f32::NAN;
        assert!(mat_inverse(&bad).iter().all(|c| c.is_nan()));
    }
}

#[cfg(test)]
mod diff_tests {
    use super::*;
    use shaiya_testkit::anim::arm_ani;

    const Q_ID: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn near(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    fn parse(d: &[u8]) -> Result<Ani> {
        Ani::parse(d, "t.ani")
    }

    fn half_arm() -> Ani {
        let a = parse(&arm_ani(30)).unwrap();
        let mut b = a.clone();
        let half = std::f32::consts::FRAC_PI_8;
        b.bones[1].rot[1].q = [0.0, 0.0, half.sin(), half.cos()];
        b
    }

    #[test]
    fn diff_of_an_animation_with_itself_is_identical() {
        let a = parse(&arm_ani(30)).unwrap();
        let d = diff(&a, &a);
        assert!(d.is_identical() && d.bones.is_empty() && d.skeleton.is_empty() && d.end_key == (30, 30));
    }

    #[test]
    fn diff_names_the_turned_bone_and_its_children_largest_first() {
        let a = parse(&arm_ani(30)).unwrap();
        let d = diff(&a, &half_arm());
        assert!(!d.is_identical());
        let named: Vec<usize> = d.bones.iter().map(|b| b.bone).collect();
        assert_eq!(named, vec![2, 1], "bone 0 does not move; the child with the larger offset comes first");
        assert!(near(d.bones[1].max_angle_deg, 45.0, 0.01), "{:?}", d.bones[1]);
        assert!(d.bones[1].max_offset < 1e-4 && d.bones[0].max_offset > 0.1);
        assert_eq!(d.bones[1].keys, (2, 2));
    }

    #[test]
    fn diff_is_symmetric_in_its_numbers() {
        let a = parse(&arm_ani(30)).unwrap();
        let mut b = half_arm();
        b.end_key = 45;
        b.bones[0].rot.push(RotKey { key: 3, q: Q_ID });
        b.first_field = -1;
        let (ab, ba) = (diff(&a, &b), diff(&b, &a));
        assert_eq!(ab.end_key, (30, 45));
        assert_eq!(ba.end_key, (45, 30));
        assert_eq!(ab.bones.len(), ba.bones.len());
        for (x, y) in ab.bones.iter().zip(&ba.bones) {
            assert_eq!((x.bone, x.max_angle_deg.to_bits(), x.max_offset.to_bits()), (y.bone, y.max_angle_deg.to_bits(), y.max_offset.to_bits()));
            assert_eq!(x.keys, (y.keys.1, y.keys.0));
        }
        assert_eq!(ab.skeleton, vec![RigDiff::FirstField { candidate: 0, target: -1 }]);
        assert_eq!(ba.skeleton, vec![RigDiff::FirstField { candidate: -1, target: 0 }]);
    }

    #[test]
    fn diff_lists_a_longer_animation_and_a_changed_skeleton() {
        let a = parse(&arm_ani(30)).unwrap();
        let mut b = a.clone();
        b.end_key = 60;
        let d = diff(&a, &b);
        assert!(!d.is_identical() && d.bones.is_empty() && d.end_key == (30, 60), "same poses, other length");
        let mut c = a.clone();
        c.bones.pop();
        let d = diff(&a, &c);
        assert_eq!(d.skeleton, vec![RigDiff::BoneCount { candidate: 3, target: 2 }]);
        assert!(d.bones.is_empty(), "only the bones both files have are compared");
    }

    /// Bone 1 turns to `end_deg` degrees about z at key 30 (bone 0 carries the translation track `trans`).
    fn turned_to(end_deg: f64, trans: Vec<(i32, [f32; 3])>) -> Ani {
        use shaiya_testkit::anim::{ani_bytes, translation_bits, BoneSpec};
        let q = |deg: f64| {
            let h = deg.to_radians() * 0.5;
            [0.0, 0.0, h.sin() as f32, h.cos() as f32]
        };
        let bone = |parent: i32, y: f32, rot: Vec<(i32, [f32; 4])>, trans: Vec<(i32, [f32; 3])>| BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot, trans };
        parse(&ani_bytes(0, 30, &[bone(-1, 0.0, Vec::new(), trans), bone(0, 0.0, vec![(0, q(0.0)), (30, q(end_deg))], Vec::new())])).unwrap()
    }

    #[test]
    fn diff_lists_a_turn_above_0_001_degrees_and_not_one_below() {
        let a = turned_to(90.0, Vec::new());
        assert!(diff(&a, &turned_to(90.0005, Vec::new())).bones.is_empty(), "a turn of 0.0005 degrees is float noise");
        let d = diff(&a, &turned_to(90.002, Vec::new()));
        assert_eq!(d.bones.iter().map(|b| b.bone).collect::<Vec<_>>(), vec![1], "{d:?}");
        assert!(near(d.bones[0].max_angle_deg, 0.002, 0.0005), "{:?}", d.bones[0]);
        assert_eq!((DIFF_ANGLE_EPSILON_DEG, DIFF_OFFSET_EPSILON), (1e-3, 1e-4));
    }

    #[test]
    fn diff_lists_a_move_above_0_0001_units_and_not_one_below() {
        let track = |x: f32| vec![(0, [0.0, 0.0, 0.0]), (30, [x, 0.0, 0.0])];
        let a = turned_to(0.0, track(1.0));
        assert!(diff(&a, &turned_to(0.0, track(1.00005))).bones.is_empty(), "a move of 0.00005 units is float noise");
        let d = diff(&a, &turned_to(0.0, track(1.0002)));
        let mut named: Vec<usize> = d.bones.iter().map(|b| b.bone).collect();
        named.sort();
        assert_eq!(named, vec![0, 1], "bone 0 moves, and bone 1 is its child");
        assert!(d.bones.iter().all(|b| near(b.max_offset, 0.0002, 0.00005)), "{d:?}");
    }

    #[test]
    fn diff_with_stops_when_cancelled_and_checks_once_per_sample() {
        let a = parse(&arm_ani(30)).unwrap();
        let mut calls = 0;
        assert_eq!(diff_with(&a, &a, &mut || { calls += 1; false }).map(|d| d.is_identical()), Some(true));
        assert_eq!(calls, 31, "one check per whole key 0..=30");
        let mut calls = 0;
        assert!(diff_with(&a, &a, &mut || { calls += 1; calls == 5 }).is_none());
        assert_eq!(calls, 5, "it stops at the first cancel");
    }

    #[test]
    fn diff_samples_at_most_the_key_cap_and_the_bone_work_budget() {
        use shaiya_testkit::anim::{ani_bytes, identity_bits, BoneSpec};
        let build = |end_key: i32, n: usize| {
            let bones: Vec<BoneSpec> = (0..n).map(|_| BoneSpec { parent: -1, matrix: identity_bits(), rot: Vec::new(), trans: Vec::new() }).collect();
            parse(&ani_bytes(0, end_key, &bones)).unwrap()
        };
        // one bone, a very long range: at most DIFF_SAMPLES_MAX + 1 keys (the end key is always added)
        let a = build(300_000, 1);
        let mut calls = 0;
        diff_with(&a, &a, &mut || { calls += 1; false }).unwrap();
        assert!(calls as i64 <= DIFF_SAMPLES_MAX + 1 && calls > 1000, "{calls}");
        // many bones: keys x bones stays under the work budget (plus one end key)
        let bones = 2_000;
        let b = build(100_000, bones);
        let mut calls = 0;
        diff_with(&b, &b, &mut || { calls += 1; false }).unwrap();
        assert!((calls as i64 - 1) * bones as i64 <= DIFF_WORK_MAX && calls > 100, "{calls}");
    }

    #[test]
    fn the_bone_count_is_read_from_the_first_ten_bytes() {
        let bytes = arm_ani(30);
        assert_eq!(bone_count_in_header(&bytes[..10]), Some(3));
        assert_eq!(bone_count_in_header(&bytes), Some(parse(&bytes).unwrap().bones.len()));
        assert_eq!(bone_count_in_header(&bytes[..9]), None);
    }
}
