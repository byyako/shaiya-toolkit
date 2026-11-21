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

use crate::error::{Cur, Result};

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

/// Smallest size of one bone in the file: parent, matrix, two counts.
const BONE_MIN: usize = 4 + 64 + 4 + 4;
const KEY_LIMIT: usize = 10_000_000;

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
    pub fn to_bytes(&self) -> Vec<u8> {
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
        d
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
        assert_eq!(rig_of("Character\\DeathEater\\ANI\\dewr_1_a.ANI").rig, Some(("dewr", "DeathEater")));
        assert_eq!(rig_of("").kind, "");
        for (name, race) in RIGS {
            let path = format!("Character/{race}/ANI/{name}_001_a.ANI");
            assert_eq!(rig_of(&path).rig, Some((name, race)), "{path}");
        }
    }
}
