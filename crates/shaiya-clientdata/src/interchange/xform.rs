//! The one mirror (`S = diag(1, 1, -1)`) between the game's left-handed space and glTF, and the small matrix and quaternion helpers the
//! exporters share. Matrices are the game's: 4 x 4, row-major, row vectors (`v' = v * M`), translation in the last row.
//!
//! The 16 floats of a row-major row-vector matrix are the column-major storage of the transposed (column-vector) matrix, so after
//! the conjugation [`s_matrix`] an array goes into a glTF `matrix` or `inverseBindMatrices` accessor without transposition.

use crate::anim::{quat_matrix, Mat4};

/// A point or translation: `(x, y, -z)`.
pub fn s_point(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[1], -p[2]]
}

/// A quaternion `(x, y, z, w)` of a rotation, seen through the mirror: `(-x, -y, z, w)` (`S R(a, t) S = R(S a, -t)`).
pub fn s_quat(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], q[2], q[3]]
}

/// `S * M * S`: the elements `[0][2]`, `[1][2]`, `[2][0]`, `[2][1]`, `[2][3]` and `[3][2]` change sign.
pub fn s_matrix(m: &Mat4) -> Mat4 {
    let mut o = *m;
    for i in [2, 6, 8, 9, 11, 14] {
        o[i] = -o[i];
    }
    o
}

/// A triangle seen through the mirror keeps its outside by swapping two corners: `(a, c, b)`.
pub fn s_tri<T: Copy>(t: [T; 3]) -> [T; 3] {
    [t[0], t[2], t[1]]
}

/// The quaternion `(x, y, z, w)` whose [`quat_matrix`] is the rotation matrix `m` (its upper 3 x 3, row-vector form), unit length. The
/// matrix must be a rotation (no scale, no shear, no reflection).
pub fn quat_of_matrix(m: &Mat4) -> [f32; 4] {
    let (m00, m01, m02, m10, m11, m12, m20, m21, m22) = (m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (1.0 + trace).sqrt() * 2.0;
        [(m12 - m21) / s, (m20 - m02) / s, (m01 - m10) / s, s / 4.0]
    } else if m00 >= m11 && m00 >= m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [s / 4.0, (m01 + m10) / s, (m02 + m20) / s, (m12 - m21) / s]
    } else if m11 >= m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [(m01 + m10) / s, s / 4.0, (m12 + m21) / s, (m20 - m02) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, s / 4.0, (m01 - m10) / s]
    };
    normalized(q).unwrap_or([0.0, 0.0, 0.0, 1.0])
}

/// `q` scaled to length 1; `None` for a non-finite or zero quaternion.
pub fn normalized(q: [f32; 4]) -> Option<[f32; 4]> {
    if q.iter().any(|c| !c.is_finite()) {
        return None;
    }
    let n = q.iter().map(|c| f64::from(*c) * f64::from(*c)).sum::<f64>().sqrt();
    if n < 1e-12 {
        return None;
    }
    Some(q.map(|c| (f64::from(c) / n) as f32))
}

/// Makes a track of quaternions hemisphere-continuous: a key is negated when its dot product with the one before is negative (what the game's
/// short-way slerp does anyway, and what viewers that blend component by component need).
pub fn make_continuous(qs: &mut [[f32; 4]]) {
    for k in 1..qs.len() {
        let dot: f32 = qs[k - 1].iter().zip(&qs[k]).map(|(a, b)| a * b).sum();
        if dot < 0.0 {
            qs[k] = qs[k].map(|c| -c);
        }
    }
}

/// A matrix split into scale, rotation and translation, in the game's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub t: [f32; 3],
    pub q: [f32; 4],
    pub s: [f32; 3],
}

impl Trs {
    pub const IDENTITY: Trs = Trs { t: [0.0; 3], q: [0.0, 0.0, 0.0, 1.0], s: [1.0; 3] };

    /// True when a scale differs from 1 by more than 1e-6 in any axis (the threshold the writer uses for `scale`).
    pub fn has_scale(&self) -> bool {
        self.s.iter().any(|s| (s - 1.0).abs() > 1e-6)
    }

    /// The row-vector matrix `Scale * Rotation * Translation` (a point is scaled, rotated, then moved).
    pub fn matrix(&self) -> Mat4 {
        let mut m = quat_matrix(self.q);
        for r in 0..3 {
            for c in 0..3 {
                m[r * 4 + c] *= self.s[r];
            }
        }
        m[12..15].copy_from_slice(&self.t);
        m
    }

    /// The same placement seen through the mirror.
    pub fn mirrored(&self) -> Trs {
        Trs { t: s_point(self.t), q: s_quat(self.q), s: self.s }
    }
}

/// Why a matrix is not a scale, a rotation and a translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotTrs {
    /// A NaN or an infinity.
    NotFinite,
    /// The last column is not `(0, 0, 0, 1)`.
    NotAffine,
    /// A row of zero length.
    Singular,
    /// The rows of the 3 x 3 are not at right angles.
    Shear,
    /// A negative determinant after the rows are scaled to length 1.
    Reflection,
}

impl NotTrs {
    pub fn reason(self) -> &'static str {
        match self {
            NotTrs::NotFinite => "the matrix holds a NaN or an infinity",
            NotTrs::NotAffine => "the last column of the matrix is not (0, 0, 0, 1)",
            NotTrs::Singular => "a row of the matrix's 3 x 3 has length 0",
            NotTrs::Shear => "the rows of the matrix are not at right angles (shear)",
            NotTrs::Reflection => "the matrix mirrors (negative determinant)",
        }
    }
}

fn dot3(a: &[f32], b: &[f32]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Splits a row-vector matrix `S * R * T`: row `i` of the 3 x 3 has length `s_i`, the rows divided by it are the rotation. A shear, a
/// reflection, a non-finite or non-affine matrix is refused.
pub fn decompose(m: &Mat4) -> Result<Trs, NotTrs> {
    if m.iter().any(|c| !c.is_finite()) {
        return Err(NotTrs::NotFinite);
    }
    if m[3].abs() > 1e-6 || m[7].abs() > 1e-6 || m[11].abs() > 1e-6 || (m[15] - 1.0).abs() > 1e-6 {
        return Err(NotTrs::NotAffine);
    }
    let rows = [&m[0..3], &m[4..7], &m[8..11]];
    let s = [dot3(rows[0], rows[0]).sqrt(), dot3(rows[1], rows[1]).sqrt(), dot3(rows[2], rows[2]).sqrt()];
    if s.iter().any(|s| *s < 1e-12) {
        return Err(NotTrs::Singular);
    }
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        if (dot3(rows[i], rows[j]) / (s[i] * s[j])).abs() > 1e-3 {
            return Err(NotTrs::Shear);
        }
    }
    let mut r = crate::anim::IDENTITY;
    for i in 0..3 {
        for c in 0..3 {
            r[i * 4 + c] = rows[i][c] / s[i];
        }
    }
    let det = r[0] * (r[5] * r[10] - r[6] * r[9]) - r[1] * (r[4] * r[10] - r[6] * r[8]) + r[2] * (r[4] * r[9] - r[5] * r[8]);
    if det < 0.0 {
        return Err(NotTrs::Reflection);
    }
    Ok(Trs { t: [m[12], m[13], m[14]], q: quat_of_matrix(&r), s })
}

/// The closest scale, rotation and translation to any matrix (`--allow-lossy`): non-finite entries become those of the identity, the rows are
/// made orthogonal in order (Gram-Schmidt), a mirror is undone by turning the third row round, a zero row is replaced by an axis.
pub fn decompose_lossy(m: &Mat4) -> Trs {
    let mut c = *m;
    for (i, v) in c.iter_mut().enumerate() {
        if !v.is_finite() {
            *v = crate::anim::IDENTITY[i];
        }
    }
    let mut rows = [[c[0], c[1], c[2]], [c[4], c[5], c[6]], [c[8], c[9], c[10]]];
    let mut s = [1f32; 3];
    let mut out = [[0f32; 3]; 3];
    for i in 0..3 {
        let mut v = rows[i];
        for o in out.iter().take(i) {
            let d = dot3(&v, o);
            for k in 0..3 {
                v[k] -= d * o[k];
            }
        }
        let n = dot3(&v, &v).sqrt();
        s[i] = dot3(&rows[i], &rows[i]).sqrt().max(1e-6);
        if n < 1e-9 {
            v = [0.0; 3];
            v[i] = 1.0;
        } else {
            v = v.map(|x| x / n);
        }
        out[i] = v;
        rows[i] = v;
    }
    let cross = [out[0][1] * out[1][2] - out[0][2] * out[1][1], out[0][2] * out[1][0] - out[0][0] * out[1][2], out[0][0] * out[1][1] - out[0][1] * out[1][0]];
    if dot3(&cross, &out[2]) < 0.0 {
        out[2] = out[2].map(|x| -x);
    }
    let mut r = crate::anim::IDENTITY;
    for i in 0..3 {
        r[i * 4..i * 4 + 3].copy_from_slice(&out[i]);
    }
    Trs { t: [c[12], c[13], c[14]], q: quat_of_matrix(&r), s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{mat_inverse, mat_mul};

    fn rand_quat(seed: &mut u64) -> [f32; 4] {
        let mut next = || {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((*seed >> 33) as f32 / (1u64 << 31) as f32) * 2.0 - 1.0
        };
        normalized([next(), next(), next(), next() + 0.01]).unwrap()
    }

    fn close(a: &Mat4, b: &Mat4, tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn the_mirror_conjugates_quaternions_and_matrices() {
        let mut seed = 7;
        for _ in 0..200 {
            let q = rand_quat(&mut seed);
            let mut m = quat_matrix(q);
            m[12..15].copy_from_slice(&[0.5, -2.0, 3.0]);
            // S * M * S, spelled out as a product of matrices
            let mut s = crate::anim::IDENTITY;
            s[10] = -1.0;
            assert!(close(&mat_mul(&mat_mul(&s, &m), &s), &s_matrix(&m), 1e-6));
            // the quaternion of the mirrored rotation is (-x, -y, z, w)
            assert!(close(&quat_matrix(s_quat(q)), &s_matrix(&quat_matrix(q)), 1e-6));
            // and the matrix reads back to that quaternion (up to sign)
            let back = quat_of_matrix(&quat_matrix(q));
            let dot: f32 = back.iter().zip(&q).map(|(a, b)| a * b).sum();
            assert!(dot.abs() > 1.0 - 1e-5, "{back:?} vs {q:?}");
        }
        // a point, a normal and a triangle: a clockwise triangle with its normal outward stays outward (counter-clockwise) after the mirror
        let tri = [[0.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]];
        let cross = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]]
        };
        let n = cross(tri[0], tri[1], tri[2]);
        let m = tri.map(s_point);
        let order = s_tri([0usize, 1, 2]);
        let n2 = cross(m[order[0]], m[order[1]], m[order[2]]);
        let n_mirrored = s_point(n);
        assert!(dot3(&n2, &n_mirrored) > 0.0, "the swapped triangle's geometric normal points where the mirrored normal does");
    }

    #[test]
    fn a_row_matrix_copies_into_the_gltf_accessor_without_transposition() {
        // a rotation of 90 degrees about y with a move: M_row = R_row, row 3 = t
        let q = [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2];
        let mut m = quat_matrix(q);
        m[12..15].copy_from_slice(&[1.0, 2.0, 3.0]);
        let g = s_matrix(&m);
        // the accessor holds g as 16 floats; read column-major (what glTF does), the matrix G_col with G_col[r][c] = a[c * 4 + r]
        let col = |r: usize, c: usize| g[c * 4 + r];
        // the column-vector matrix of the mirrored placement: its last column is the mirrored translation
        assert_eq!([col(0, 3), col(1, 3), col(2, 3)], [1.0, 2.0, -3.0]);
        // a point p (game space) goes to p' = p * M; mirrored: S p' = G_col * (S p)
        let p = [0.25f32, -0.5, 2.0];
        let pm = [p[0] * m[0] + p[1] * m[4] + p[2] * m[8] + m[12], p[0] * m[1] + p[1] * m[5] + p[2] * m[9] + m[13], p[0] * m[2] + p[1] * m[6] + p[2] * m[10] + m[14]];
        let sp = s_point(p);
        let via_col: Vec<f32> = (0..3).map(|r| col(r, 0) * sp[0] + col(r, 1) * sp[1] + col(r, 2) * sp[2] + col(r, 3)).collect();
        let want = s_point(pm);
        for i in 0..3 {
            assert!((via_col[i] - want[i]).abs() < 1e-6, "{via_col:?} vs {want:?}");
        }
        // composition: World = L * World_parent (row) is World_col = Wp_col * L_col
        let mut l = quat_matrix([0.0, 0.0, 0.5f32.sqrt(), 0.5f32.sqrt()]);
        l[12..15].copy_from_slice(&[0.0, 1.0, 0.0]);
        let world = mat_mul(&l, &m);
        let (gl, gm, gw) = (s_matrix(&l), s_matrix(&m), s_matrix(&world));
        let colmul = |a: &Mat4, b: &Mat4| {
            // product of the column-vector matrices stored column-major in a and b
            let mut o = [0f32; 16];
            for r in 0..4 {
                for c in 0..4 {
                    o[c * 4 + r] = (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum();
                }
            }
            o
        };
        assert!(close(&colmul(&gm, &gl), &gw, 1e-6));
    }

    #[test]
    fn local_decomposition_and_its_refusals() {
        let mut seed = 99;
        // rotation, rotation + translation
        let q = rand_quat(&mut seed);
        let mut m = quat_matrix(q);
        m[12..15].copy_from_slice(&[1.0, 2.0, 3.0]);
        let t = decompose(&m).unwrap();
        assert!(!t.has_scale());
        assert!(close(&t.matrix(), &m, 1e-5));
        // uniform scale and non-uniform scale
        for s in [[2.0, 2.0, 2.0], [1.0, 3.0, 0.5]] {
            let want = Trs { t: [0.0, 1.0, 0.0], q, s };
            let t = decompose(&want.matrix()).unwrap();
            assert!(t.has_scale());
            assert!(close(&t.matrix(), &want.matrix(), 1e-5));
        }
        // shear
        let mut sh = crate::anim::IDENTITY;
        sh[1] = 0.5;
        sh[0] = 1.0;
        assert_eq!(decompose(&sh), Err(NotTrs::Shear));
        // reflection
        let mut refl = crate::anim::IDENTITY;
        refl[0] = -1.0;
        assert_eq!(decompose(&refl), Err(NotTrs::Reflection));
        // NaN, a zero row, a projective column
        let mut nan = crate::anim::IDENTITY;
        nan[5] = f32::NAN;
        assert_eq!(decompose(&nan), Err(NotTrs::NotFinite));
        let mut zero = crate::anim::IDENTITY;
        zero[10] = 0.0;
        assert_eq!(decompose(&zero), Err(NotTrs::Singular));
        let mut proj = crate::anim::IDENTITY;
        proj[3] = 0.1;
        assert_eq!(decompose(&proj), Err(NotTrs::NotAffine));
        // the lossy projection always gives a rotation
        for bad in [sh, refl, nan, zero, proj] {
            let t = decompose_lossy(&bad);
            assert!(t.q.iter().all(|c| c.is_finite()));
            assert!(decompose(&t.matrix()).is_ok(), "the projection of {bad:?} decomposes");
        }
        // an inverse of a rotation + translation decomposes (what the exporter does with a 3DC's matrices)
        assert!(decompose(&mat_inverse(&m)).is_ok());
    }

    #[test]
    fn continuity_matches_the_game_slerp() {
        let mut seed = 3;
        let mut keys: Vec<[f32; 4]> = (0..6).map(|_| rand_quat(&mut seed)).collect();
        // force sign flips: the same rotations, half of them in the other hemisphere
        for (i, k) in keys.iter_mut().enumerate() {
            if i % 2 == 1 {
                *k = k.map(|c| -c);
            }
        }
        let mut cont = keys.clone();
        make_continuous(&mut cont);
        for k in 1..cont.len() {
            let dot: f32 = cont[k - 1].iter().zip(&cont[k]).map(|(a, b)| a * b).sum();
            assert!(dot >= 0.0);
        }
        // the game slerps the short way between the stored keys; a plain slerp of the continuous keys is the same rotation at 50 times
        let ani = |qs: &[[f32; 4]]| crate::anim::Ani {
            first_field: 0,
            end_key: 100,
            bones: vec![crate::anim::AniBone {
                parent: -1,
                matrix: crate::anim::IDENTITY,
                rot: qs.iter().enumerate().map(|(i, q)| crate::anim::RotKey { key: i as i32 * 10, q: *q }).collect(),
                trans: vec![],
            }],
        };
        let (a, b) = (ani(&keys), ani(&cont));
        let (la, lb) = (crate::anim::locals(&a), crate::anim::locals(&b));
        for i in 0..50 {
            let t = i as f32 * 1.0;
            let (pa, pb) = (crate::anim::pose(&a, &la, t), crate::anim::pose(&b, &lb, t));
            assert!(close(&pa[0], &pb[0], 1e-5), "t={t}");
        }
    }
}
