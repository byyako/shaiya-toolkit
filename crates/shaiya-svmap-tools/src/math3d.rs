//! The little vector and matrix math the 3D view needs (no `glam`: a handful of functions, so no new dependency).
//!
//! World: x east, y up, z north (the server's axes). The camera is left-handed like Direct3D (looking north, +x is on the
//! right), clip depth 0..1 like wgpu. Matrices are column-major: `m[column][row]`, so they can be handed to WGSL as they are.

pub type V3 = [f32; 3];
pub type Mat4 = [[f32; 4]; 4];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}

pub fn norm(a: V3) -> V3 {
    let l = len(a);
    if l < 1e-12 {
        [0.0, 1.0, 0.0]
    } else {
        scale(a, 1.0 / l)
    }
}

/// `a * b` (apply `b` first).
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut o = [[0.0f32; 4]; 4];
    for (c, oc) in o.iter_mut().enumerate() {
        for (r, v) in oc.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    o
}

/// The view matrix of a camera at `eye` looking along `forward` (unit), `up` = world up.
pub fn look_dir(eye: V3, forward: V3, up: V3) -> Mat4 {
    let f = norm(forward);
    let r = norm(cross(up, f));
    let u = cross(f, r);
    [[r[0], u[0], f[0], 0.0], [r[1], u[1], f[1], 0.0], [r[2], u[2], f[2], 0.0], [-dot(r, eye), -dot(u, eye), -dot(f, eye), 1.0]]
}

/// Left-handed perspective, depth 0 (near) to 1 (far); `fov_y` in radians.
pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let t = 1.0 / (fov_y * 0.5).tan();
    let q = far / (far - near);
    [[t / aspect, 0.0, 0.0, 0.0], [0.0, t, 0.0, 0.0], [0.0, 0.0, q, 1.0], [0.0, 0.0, -near * q, 0.0]]
}

/// Clip-space position of a world point (x, y, z, w).
pub fn project(m: &Mat4, p: V3) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for (r, v) in o.iter_mut().enumerate() {
        *v = m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    }
    o
}

/// Six frustum planes (left, right, bottom, top, near, far) as (nx, ny, nz, d) with the inside where `n.p + d >= 0`.
pub fn frustum(m: &Mat4) -> [[f32; 4]; 6] {
    let row = |r: usize| [m[0][r], m[1][r], m[2][r], m[3][r]];
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    let comb = |a: [f32; 4], b: [f32; 4], s: f32| [a[0] + s * b[0], a[1] + s * b[1], a[2] + s * b[2], a[3] + s * b[3]];
    [comb(r3, r0, 1.0), comb(r3, r0, -1.0), comb(r3, r1, 1.0), comb(r3, r1, -1.0), r2, comb(r3, r2, -1.0)]
}

/// Whether the box can be seen (false only when it lies entirely outside one plane).
pub fn box_visible(planes: &[[f32; 4]; 6], min: V3, max: V3) -> bool {
    for p in planes {
        // the corner furthest along the plane normal
        let c = [if p[0] >= 0.0 { max[0] } else { min[0] }, if p[1] >= 0.0 { max[1] } else { min[1] }, if p[2] >= 0.0 { max[2] } else { min[2] }];
        if p[0] * c[0] + p[1] * c[1] + p[2] * c[2] + p[3] < 0.0 {
            return false;
        }
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: V3,
    pub dir: V3,
}

impl Ray {
    pub fn at(&self, t: f32) -> V3 {
        add(self.origin, scale(self.dir, t))
    }
}

/// Distance along the ray to the box (slab test), or `None` when it misses or the box is behind. A ray that starts inside
/// returns 0.
pub fn ray_box(ray: &Ray, min: V3, max: V3) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for a in 0..3 {
        if ray.dir[a].abs() < 1e-9 {
            if ray.origin[a] < min[a] || ray.origin[a] > max[a] {
                return None;
            }
        } else {
            let inv = 1.0 / ray.dir[a];
            let (mut a0, mut a1) = ((min[a] - ray.origin[a]) * inv, (max[a] - ray.origin[a]) * inv);
            if a0 > a1 {
                std::mem::swap(&mut a0, &mut a1);
            }
            t0 = t0.max(a0);
            t1 = t1.min(a1);
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

/// Distance along the ray to a vertical cylinder (axis through `base`, from y = base.y to base.y + height).
pub fn ray_cylinder(ray: &Ray, base: V3, radius: f32, height: f32) -> Option<f32> {
    // box prefilter
    ray_box(ray, [base[0] - radius, base[1], base[2] - radius], [base[0] + radius, base[1] + height, base[2] + radius])?;
    let (ox, oz) = (ray.origin[0] - base[0], ray.origin[2] - base[2]);
    let (dx, dz) = (ray.dir[0], ray.dir[2]);
    let a = dx * dx + dz * dz;
    let mut best: Option<f32> = None;
    let mut consider = |t: f32| {
        if t >= 0.0 {
            let y = ray.origin[1] + ray.dir[1] * t;
            if y >= base[1] && y <= base[1] + height && best.is_none_or(|b| t < b) {
                best = Some(t);
            }
        }
    };
    if a > 1e-12 {
        let b = 2.0 * (ox * dx + oz * dz);
        let c = ox * ox + oz * oz - radius * radius;
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let s = disc.sqrt();
            consider((-b - s) / (2.0 * a));
            consider((-b + s) / (2.0 * a));
        }
    }
    // the caps
    if ray.dir[1].abs() > 1e-9 {
        for y in [base[1], base[1] + height] {
            let t = (y - ray.origin[1]) / ray.dir[1];
            if t >= 0.0 {
                let (px, pz) = (ox + dx * t, oz + dz * t);
                if px * px + pz * pz <= radius * radius && best.is_none_or(|b| t < b) {
                    best = Some(t);
                }
            }
        }
    }
    // a ray that starts inside the cylinder
    if best.is_none() && ox * ox + oz * oz <= radius * radius && ray.origin[1] >= base[1] && ray.origin[1] <= base[1] + height {
        best = Some(0.0);
    }
    best
}

/// Distance along the ray to a sphere.
pub fn ray_sphere(ray: &Ray, c: V3, r: f32) -> Option<f32> {
    let oc = sub(ray.origin, c);
    let b = dot(oc, ray.dir);
    let cc = dot(oc, oc) - r * r;
    let disc = b * b - cc;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    let t = -b - s;
    if t >= 0.0 {
        Some(t)
    } else if -b + s >= 0.0 {
        Some(0.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looking_north_puts_east_on_the_right_and_the_axis_in_the_middle() {
        let view = look_dir([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]);
        let proj = perspective(1.0, 1.5, 0.1, 100.0);
        let vp = mul(&proj, &view);
        let c = project(&vp, [0.0, 0.0, 10.0]);
        assert!((c[0] / c[3]).abs() < 1e-5 && (c[1] / c[3]).abs() < 1e-5);
        let e = project(&vp, [3.0, 0.0, 10.0]);
        assert!(e[0] / e[3] > 0.1, "east is on the right");
        let u = project(&vp, [0.0, 3.0, 10.0]);
        assert!(u[1] / u[3] > 0.1, "up is up");
        // depth 0..1
        let near = project(&vp, [0.0, 0.0, 0.1]);
        let far = project(&vp, [0.0, 0.0, 100.0]);
        assert!((near[2] / near[3]).abs() < 1e-4);
        assert!((far[2] / far[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn frustum_culls_boxes_behind_and_to_the_side() {
        let view = look_dir([0.0, 10.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]);
        let vp = mul(&perspective(1.0, 1.0, 0.1, 100.0), &view);
        let f = frustum(&vp);
        assert!(box_visible(&f, [-1.0, 9.0, 20.0], [1.0, 11.0, 22.0]));
        assert!(!box_visible(&f, [-1.0, 9.0, -22.0], [1.0, 11.0, -20.0]));
        assert!(!box_visible(&f, [200.0, 9.0, 20.0], [210.0, 11.0, 22.0]));
        assert!(!box_visible(&f, [-1.0, 9.0, 300.0], [1.0, 11.0, 310.0]));
    }

    #[test]
    fn ray_shapes() {
        let r = Ray { origin: [0.0, 5.0, -10.0], dir: norm([0.0, 0.0, 1.0]) };
        assert!((ray_box(&r, [-1.0, 4.0, 0.0], [1.0, 6.0, 2.0]).unwrap() - 10.0).abs() < 1e-4);
        assert!(ray_box(&r, [5.0, 4.0, 0.0], [6.0, 6.0, 2.0]).is_none());
        assert!((ray_cylinder(&r, [0.0, 4.0, 0.0], 1.0, 3.0).unwrap() - 9.0).abs() < 1e-4);
        assert!(ray_cylinder(&r, [0.0, 8.0, 0.0], 1.0, 3.0).is_none());
        assert!((ray_sphere(&r, [0.0, 5.0, 0.0], 2.0).unwrap() - 8.0).abs() < 1e-4);
        // from above onto the cap
        let d = Ray { origin: [0.2, 20.0, 0.0], dir: [0.0, -1.0, 0.0] };
        assert!((ray_cylinder(&d, [0.0, 0.0, 0.0], 1.0, 3.0).unwrap() - 17.0).abs() < 1e-4);
    }
}
