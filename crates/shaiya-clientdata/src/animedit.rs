//! Editing tools for `.ANI` animations: retime, trim and copy tracks. Each returns a new [`Ani`] and never touches its input; each result
//! must pass [`passes`] against the animation it was made from (same skeleton, same first field, no validation error), or the tool refuses.
//!
//! Poses are sampled with the toolkit's own rules (see [`crate::anim`]): a rotation track is slerped the short way, a translation track is
//! linear, a track holds its first value before its first key and its last value after its last key. The edits are validated offline against
//! those rules; nothing here says how the game plays a file.

use crate::anim::{passes, rig_check, segment, slerp, validate, Ani, RigDiff, RotKey, Severity, TransKey};
use crate::error::{Error, Result};

/// The smallest and largest factor of [`retime`].
pub const RETIME_MIN: f32 = 0.1;
pub const RETIME_MAX: f32 = 10.0;
/// Most keys the new range of [`retime`] may have.
pub const RETIME_RANGE_MAX: i64 = 1_000_000;
/// Most keys [`retime`] samples in all (tracks times keys of the new range): a bigger job is refused before anything is allocated.
pub const RETIME_SAMPLES_MAX: u64 = 50_000_000;
/// A key that linear (or slerp) interpolation between its neighbours reproduces within this is dropped by [`retime`].
pub const RETIME_TOLERANCE: f32 = 1e-5;
/// Keys one dropped run may span in [`retime`].
const PRUNE_WINDOW: usize = 64;

/// Which tracks [`copy_tracks`] copies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tracks {
    pub rotation: bool,
    pub translation: bool,
}

fn refuse<T>(what: impl Into<String>) -> Result<T> {
    Err(Error::Format { file: "animation edit".into(), what: what.into(), offset: 0 })
}

/// The factor that makes `ani` last `seconds` (key units are 1/30 second): `seconds * 30 / (end key - first field)`.
pub fn factor_for_seconds(ani: &Ani, seconds: f32) -> Result<f32> {
    let span = i64::from(ani.end_key) - i64::from(ani.first_field);
    if !seconds.is_finite() || seconds <= 0.0 {
        return refuse("the length in seconds must be a number above 0");
    }
    if span <= 0 {
        return refuse("the animation has no length to change");
    }
    Ok(seconds * 30.0 / span as f32)
}

fn rot_at(track: &[RotKey], t: f32) -> [f32; 4] {
    match segment(|k| track[k].key, track.len(), t) {
        Err(k) => track[k].q,
        Ok((k, f)) => slerp(track[k].q, track[k + 1].q, f),
    }
}

fn trans_at(track: &[TransKey], t: f32) -> [f32; 3] {
    match segment(|k| track[k].key, track.len(), t) {
        Err(k) => track[k].v,
        Ok((k, f)) => {
            let (a, c) = (track[k].v, track[k + 1].v);
            [a[0] + (c[0] - a[0]) * f, a[1] + (c[1] - a[1]) * f, a[2] + (c[2] - a[2]) * f]
        }
    }
}

fn rot_dist(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let s = if dot < 0.0 { -1.0 } else { 1.0 };
    a.iter().zip(&b).map(|(x, y)| (x - s * y).abs()).fold(0.0, f32::max)
}

fn trans_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}

/// The indices of `dense` (one value per whole key) that stay: the first and the last, and every other value the interpolation of its kept
/// neighbours does not reproduce within [`RETIME_TOLERANCE`].
fn prune<T: Copy>(dense: &[T], lerp: impl Fn(T, T, f32) -> T, dist: impl Fn(T, T) -> f32) -> Vec<usize> {
    let n = dense.len();
    let mut kept = vec![0usize];
    let mut a = 0usize;
    while a + 1 < n {
        let mut best = a + 1;
        let mut b = a + 2;
        while b < n && b <= a + PRUNE_WINDOW {
            let ok = (a + 1..b).all(|j| dist(dense[j], lerp(dense[a], dense[b], (j - a) as f32 / (b - a) as f32)) <= RETIME_TOLERANCE);
            if !ok {
                break;
            }
            best = b;
            b += 1;
        }
        kept.push(best);
        a = best;
    }
    kept
}

/// What a finished edit must be: it passes against its input, or the reasons are the refusal.
fn finish(out: Ani, input: &Ani) -> Result<Ani> {
    if passes(&out, input) {
        return Ok(out);
    }
    let mut why: Vec<String> = rig_check(&out, input).iter().map(|d| format!("{d:?}")).collect();
    why.extend(validate(&out).into_iter().filter(|i| i.severity == Severity::Error).map(|i| i.message));
    why.truncate(5);
    refuse(format!("the result does not pass against its input: {}", why.join("; ")))
}

/// Makes the animation `factor` times as long: the new end key is `first + round((end - first) * factor)`; every bone that has a track gets
/// keys at every whole key of the new range, sampled from the old pose at `first + (k - first) / factor` on its own track, and the keys that
/// linear (rotation: slerp) interpolation reproduces within 1e-5 are dropped again. A bone without a track stays without one.
///
/// Refused: a factor that is not a finite number in 0.1 to 10; an animation with no length; a result under one key or over
/// [`RETIME_RANGE_MAX`] keys, or a job over [`RETIME_SAMPLES_MAX`] samples.
pub fn retime(ani: &Ani, factor: f32) -> Result<Ani> {
    if !factor.is_finite() || !(RETIME_MIN..=RETIME_MAX).contains(&factor) {
        return refuse(format!("the factor must be a number from {RETIME_MIN} to {RETIME_MAX}"));
    }
    let first = ani.first_field;
    let span = i64::from(ani.end_key) - i64::from(first);
    if span <= 0 {
        return refuse("the animation has no length to change");
    }
    let new_span = (span as f64 * f64::from(factor)).round() as i64;
    if new_span < 1 {
        return refuse("the result would be shorter than one key");
    }
    if new_span > RETIME_RANGE_MAX {
        return refuse(format!("the result would have {new_span} keys; the most is {RETIME_RANGE_MAX}"));
    }
    let Some(new_end) = i32::try_from(i64::from(first) + new_span).ok() else { return refuse("the new end key does not fit") };
    let tracks = ani.bones.iter().map(|b| u64::from(!b.rot.is_empty()) + u64::from(!b.trans.is_empty())).sum::<u64>();
    if tracks.saturating_mul(new_span as u64 + 1) > RETIME_SAMPLES_MAX {
        return refuse("the job would sample too many keys; retime a shorter animation");
    }
    let n = new_span as usize + 1;
    let old_time = |i: usize| first as f32 + i as f32 / factor;
    let mut out = ani.clone();
    out.end_key = new_end;
    for (b, old) in out.bones.iter_mut().zip(&ani.bones) {
        if !old.rot.is_empty() {
            let dense: Vec<[f32; 4]> = (0..n).map(|i| rot_at(&old.rot, old_time(i))).collect();
            let kept = prune(&dense, slerp, rot_dist);
            b.rot = kept.into_iter().map(|i| RotKey { key: first + i as i32, q: dense[i] }).collect();
        }
        if !old.trans.is_empty() {
            let dense: Vec<[f32; 3]> = (0..n).map(|i| trans_at(&old.trans, old_time(i))).collect();
            let lerp = |a: [f32; 3], c: [f32; 3], f: f32| [a[0] + (c[0] - a[0]) * f, a[1] + (c[1] - a[1]) * f, a[2] + (c[2] - a[2]) * f];
            let kept = prune(&dense, lerp, trans_dist);
            b.trans = kept.into_iter().map(|i| TransKey { key: first + i as i32, v: dense[i] }).collect();
        }
    }
    finish(out, ani)
}

/// Keeps the keys `start` to `end` and moves them to the first field: needs `first field <= start < end <= end key`. Every track is sampled at
/// `start` and at `end`, the keys between stay, and all of them move by `first field - start`; the new end key is `first field + end - start`.
pub fn trim(ani: &Ani, start: i32, end: i32) -> Result<Ani> {
    let first = ani.first_field;
    if !(first <= start && start < end && end <= ani.end_key) {
        return refuse(format!("the part to keep must satisfy {first} <= start < end <= {} (start {start}, end {end})", ani.end_key));
    }
    let shift = i64::from(first) - i64::from(start);
    let moved = |k: i32| -> Result<i32> { i32::try_from(i64::from(k) + shift).or_else(|_| refuse("a key does not fit after moving it")) };
    let mut out = ani.clone();
    out.end_key = moved(end)?;
    for (o, b) in out.bones.iter_mut().zip(&ani.bones) {
        if !b.rot.is_empty() {
            let mut v = vec![RotKey { key: moved(start)?, q: rot_at(&b.rot, start as f32) }];
            for k in b.rot.iter().filter(|k| k.key > start && k.key < end) {
                v.push(RotKey { key: moved(k.key)?, q: k.q });
            }
            v.push(RotKey { key: moved(end)?, q: rot_at(&b.rot, end as f32) });
            o.rot = v;
        }
        if !b.trans.is_empty() {
            let mut v = vec![TransKey { key: moved(start)?, v: trans_at(&b.trans, start as f32) }];
            for k in b.trans.iter().filter(|k| k.key > start && k.key < end) {
                v.push(TransKey { key: moved(k.key)?, v: k.v });
            }
            v.push(TransKey { key: moved(end)?, v: trans_at(&b.trans, end as f32) });
            o.trans = v;
        }
    }
    finish(out, ani)
}

/// Copies the named tracks of the named bones from `src` into a copy of `dst`. `src` and `dst` must have the same skeleton and first field
/// ([`rig_check`]`(src, dst)` empty), else the refusal lists the differences. A copied track replaces the whole track (a bone whose source
/// track is empty ends without one). The end key becomes the larger of `dst`'s and the last copied key.
pub fn copy_tracks(dst: &Ani, src: &Ani, bones: &[usize], what: Tracks) -> Result<Ani> {
    if !what.rotation && !what.translation {
        return refuse("choose rotation, translation or both");
    }
    if bones.is_empty() {
        return refuse("name at least one bone");
    }
    let diffs = rig_check(src, dst);
    if !diffs.is_empty() {
        let list: Vec<String> = diffs
            .iter()
            .take(5)
            .map(|d| match d {
                RigDiff::BoneCount { candidate, target } => format!("bone count {candidate} against {target}"),
                RigDiff::Parent { bone, candidate, target } => format!("bone {bone} has parent {candidate} against {target}"),
                RigDiff::Matrix { bone } => format!("bone {bone} has another reference matrix"),
                RigDiff::FirstField { candidate, target } => format!("first field {candidate} against {target}"),
            })
            .collect();
        return refuse(format!("the two animations do not have the same skeleton: {}", list.join("; ")));
    }
    if let Some(&b) = bones.iter().find(|&&b| b >= dst.bones.len()) {
        return refuse(format!("bone {b} does not exist: the animation has {} bones", dst.bones.len()));
    }
    let mut out = dst.clone();
    let mut last = out.end_key;
    for &b in bones {
        if what.rotation {
            out.bones[b].rot = src.bones[b].rot.clone();
            last = last.max(src.bones[b].rot.last().map_or(last, |k| k.key));
        }
        if what.translation {
            out.bones[b].trans = src.bones[b].trans.clone();
            last = last.max(src.bones[b].trans.last().map_or(last, |k| k.key));
        }
    }
    out.end_key = last;
    finish(out, dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{locals, pose};
    use shaiya_testkit::anim::{ani_bytes, translation_bits, BoneSpec};

    fn spec(parent: i32, y: f32, rot: Vec<(i32, [f32; 4])>, trans: Vec<(i32, [f32; 3])>) -> BoneSpec {
        BoneSpec { parent, matrix: translation_bits(0.0, y, 0.0), rot, trans }
    }

    fn turn(deg: f32) -> [f32; 4] {
        let (s, c) = (deg.to_radians() * 0.5).sin_cos();
        [0.0, 0.0, s, c]
    }

    fn build(first: i32, end: i32, bones: &[BoneSpec]) -> Ani {
        Ani::parse(&ani_bytes(first, end, bones), "t.ani").unwrap()
    }

    /// Bone 0 without tracks, bone 1 turned 0 to 80 degrees over 30 keys, bone 2 moving through a corner at key 10.
    fn sample() -> Ani {
        build(
            0,
            30,
            &[
                spec(-1, 0.0, Vec::new(), Vec::new()),
                spec(0, 1.0, vec![(0, turn(0.0)), (30, turn(80.0))], Vec::new()),
                spec(1, 2.0, Vec::new(), vec![(0, [0.0, 0.0, 0.0]), (10, [1.0, 0.0, 0.0]), (30, [1.0, 2.0, 0.0])]),
            ],
        )
    }

    fn same_poses(a: &Ani, ta: f32, b: &Ani, tb: f32) -> bool {
        let (pa, pb) = (pose(a, &locals(a), ta), pose(b, &locals(b), tb));
        pa.iter().zip(&pb).all(|(x, y)| x.iter().zip(y).all(|(p, q)| (p - q).abs() <= 1e-5))
    }

    #[test]
    fn retime_by_2_doubles_the_end_key_and_poses_match_at_scaled_times() {
        let a = sample();
        let r = retime(&a, 2.0).unwrap();
        assert_eq!((r.first_field, r.end_key), (0, 60));
        for t in 0..=60 {
            assert!(same_poses(&r, t as f32, &a, t as f32 / 2.0), "key {t}");
        }
        // the straight runs are kept as their end points only
        assert_eq!(r.bones[2].trans.iter().map(|k| k.key).collect::<Vec<_>>(), vec![0, 20, 60]);
        assert_eq!(r.bones[1].rot.iter().map(|k| k.key).collect::<Vec<_>>(), vec![0, 60]);
        assert!(r.bones[0].rot.is_empty() && r.bones[0].trans.is_empty(), "a bone without a track stays without one");
        assert!(r.bones[1].trans.is_empty() && r.bones[2].rot.is_empty(), "a track that was not there is not made");
    }

    #[test]
    fn retime_by_half_resamples_without_colliding_keys() {
        let zig = |i: i32| [[0.0, 9.0, 1.0, 9.0, 5.0][i as usize], 0.0, 0.0];
        let a = build(0, 4, &[spec(-1, 0.0, Vec::new(), (0..=4).map(|i| (i, zig(i))).collect())]);
        let r = retime(&a, 0.5).unwrap();
        assert_eq!(r.end_key, 2);
        let keys: Vec<i32> = r.bones[0].trans.iter().map(|k| k.key).collect();
        assert_eq!(keys, vec![0, 1, 2], "one key per whole key of the new range, none twice");
        assert!(validate(&r).iter().all(|i| i.severity != Severity::Error));
        for t in 0..=2 {
            assert!(same_poses(&r, t as f32, &a, t as f32 * 2.0), "key {t}");
        }
    }

    #[test]
    fn trim_keeps_the_inner_poses_and_moves_them_to_the_first_field() {
        let a = sample();
        let r = trim(&a, 5, 20).unwrap();
        assert_eq!((r.first_field, r.end_key), (0, 15));
        for k in 0..=15 {
            assert!(same_poses(&r, k as f32, &a, (k + 5) as f32), "key {k}");
        }
        // the corner key at 10 stays as an inner key, moved by -5
        assert_eq!(r.bones[2].trans.iter().map(|k| k.key).collect::<Vec<_>>(), vec![0, 5, 15]);
        // a first field of -1 stays the first field
        let b = build(-1, 30, &[spec(-1, 0.0, Vec::new(), vec![(-1, [0.0; 3]), (30, [3.0, 0.0, 0.0])])]);
        let r = trim(&b, 10, 20).unwrap();
        assert_eq!((r.first_field, r.end_key), (-1, 9));
        assert_eq!(r.bones[0].trans.iter().map(|k| k.key).collect::<Vec<_>>(), vec![-1, 9]);
    }

    #[test]
    fn trim_outside_the_animation_is_refused() {
        let a = sample();
        for (s, e) in [(-1, 10), (5, 31), (10, 10), (20, 10), (0, 0)] {
            assert!(trim(&a, s, e).is_err(), "{s}..{e}");
        }
        assert!(trim(&a, 0, 30).is_ok() && trim(&a, 29, 30).is_ok());
    }

    #[test]
    fn copy_track_copies_only_the_named_bones_and_tracks() {
        let skeleton = |rot1: Vec<(i32, [f32; 4])>, trans1: Vec<(i32, [f32; 3])>, trans2: Vec<(i32, [f32; 3])>| {
            build(0, 30, &[spec(-1, 0.0, Vec::new(), Vec::new()), spec(0, 1.0, rot1, trans1), spec(1, 2.0, vec![(0, turn(10.0)), (30, turn(20.0))], trans2)])
        };
        let dst = skeleton(vec![(0, turn(0.0)), (30, turn(10.0))], vec![(0, [0.0; 3]), (30, [0.0, 1.0, 0.0])], vec![(0, [0.0; 3])]);
        let src = skeleton(vec![(0, turn(45.0)), (15, turn(50.0)), (30, turn(90.0))], vec![(0, [5.0; 3]), (30, [6.0; 3])], vec![(0, [9.0; 3]), (30, [8.0; 3])]);
        let r = copy_tracks(&dst, &src, &[1], Tracks { rotation: true, translation: false }).unwrap();
        assert_eq!(r.bones[1].rot, src.bones[1].rot, "the named bone's rotation comes from the source");
        assert_eq!(r.bones[1].trans, dst.bones[1].trans, "its translation does not");
        assert_eq!(r.bones[2], dst.bones[2], "another bone is untouched");
        assert_eq!(r.bones[0], dst.bones[0]);
        let r = copy_tracks(&dst, &src, &[1, 2], Tracks { rotation: false, translation: true }).unwrap();
        assert_eq!((r.bones[1].trans.clone(), r.bones[2].trans.clone()), (src.bones[1].trans.clone(), src.bones[2].trans.clone()));
        assert_eq!((r.bones[1].rot.clone(), r.bones[2].rot.clone()), (dst.bones[1].rot.clone(), dst.bones[2].rot.clone()));
        // nothing named, nothing chosen, a bone that is not there
        assert!(copy_tracks(&dst, &src, &[], Tracks { rotation: true, translation: true }).is_err());
        assert!(copy_tracks(&dst, &src, &[1], Tracks { rotation: false, translation: false }).is_err());
        assert!(copy_tracks(&dst, &src, &[3], Tracks { rotation: true, translation: true }).is_err());
    }

    #[test]
    fn copy_track_between_two_skeletons_is_refused() {
        let a = sample();
        let mut other = sample();
        other.bones[2].matrix[13] += 1.0;
        let e = copy_tracks(&a, &other, &[1], Tracks { rotation: true, translation: true }).unwrap_err().to_string();
        assert!(e.contains("not have the same skeleton") && e.contains("bone 2"), "{e}");
        let fewer = build(0, 30, &[spec(-1, 0.0, Vec::new(), Vec::new())]);
        assert!(copy_tracks(&a, &fewer, &[0], Tracks { rotation: true, translation: true }).is_err());
        let mut late = sample();
        late.first_field = -1;
        assert!(copy_tracks(&a, &late, &[1], Tracks { rotation: true, translation: true }).is_err(), "the first field must match too");
    }

    #[test]
    fn every_edit_passes_against_its_input() {
        let a = sample();
        for r in [retime(&a, 2.0).unwrap(), retime(&a, 0.5).unwrap(), retime(&a, 1.0).unwrap(), trim(&a, 3, 17).unwrap(), copy_tracks(&a, &a, &[1, 2], Tracks { rotation: true, translation: true }).unwrap()] {
            assert!(passes(&r, &a));
            let bytes = r.try_to_bytes().unwrap();
            assert_eq!(Ani::parse(&bytes, "t.ani").unwrap(), r, "the result writes and reads back");
        }
        // the input is not changed
        assert_eq!(a, sample());
    }

    #[test]
    fn a_zero_or_nonfinite_factor_is_refused() {
        let a = sample();
        for f in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.09, 10.5] {
            assert!(retime(&a, f).is_err(), "{f}");
        }
        assert!(retime(&a, 0.1).is_ok() && retime(&a, 10.0).is_ok());
        // an animation with no length, and a result over the key cap
        assert!(retime(&build(0, 0, &[spec(-1, 0.0, Vec::new(), Vec::new())]), 2.0).is_err());
        assert!(retime(&build(0, 20_000_000, &[spec(-1, 0.0, Vec::new(), Vec::new())]), 10.0).is_err());
        assert!(factor_for_seconds(&a, 0.0).is_err() && factor_for_seconds(&a, f32::NAN).is_err());
        assert_eq!(factor_for_seconds(&a, 2.0).unwrap(), 2.0);
    }
}
