//! Pictures made from parsed client data, without a GUI: the terrain hillshade. The SVMap Editor and the toolkit's Archive
//! preview both call this code, so a map looks the same in both.

use crate::wld::{Terrain, CORNER_SPACING, HEIGHT_OFFSET, HEIGHT_SCALE};

/// The neutral tint of [`hillshade_grey`].
pub const GREY_TINT: [u8; 3] = [200, 200, 200];

/// Colours the pixels of the corners in `rect` (`[cx0, cz0, cx1, cz1]`, inclusive) of an `n` x `n` hillshade picture from the
/// corner heights. A terrain edit redraws only the corners it touched (and their neighbours) with this; the first picture is the
/// same function over the whole grid, so a partial redraw gives the pixels a full one would. Light comes from the north-west;
/// row 0 of the picture is the north edge.
pub fn shade_rect(heights: &[i16], tex_index: &[u8], n: usize, tints: &[[u8; 3]], rect: [u32; 4], rgba: &mut [u8]) {
    if n == 0 || heights.len() != n * n || rgba.len() != n * n * 4 {
        return;
    }
    let corner = |cx: usize, cz: usize| heights[cz.min(n - 1) * n + cx.min(n - 1)] as f32 * HEIGHT_SCALE + HEIGHT_OFFSET;
    // light: from the north-west, 45 degrees up
    let l = {
        let v = [-0.5f32, 0.7, 0.5];
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / len, v[1] / len, v[2] / len]
    };
    let (x1, z1) = ((rect[2] as usize).min(n - 1), (rect[3] as usize).min(n - 1));
    for cz in (rect[1] as usize).min(n - 1)..=z1 {
        for cx in (rect[0] as usize).min(n - 1)..=x1 {
            let hx = corner(cx + 1, cz) - corner(cx.saturating_sub(1), cz);
            let hz = corner(cx, cz + 1) - corner(cx, cz.saturating_sub(1));
            let span = 2.0 * CORNER_SPACING;
            // surface normal of y = h(x, z): (-dh/dx, 1, -dh/dz)
            let (nx, ny, nz) = (-hx / span, 1.0f32, -hz / span);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let lam = ((nx * l[0] + ny * l[1] + nz * l[2]) / len).max(0.0);
            let shade = 0.30 + 0.85 * lam;
            let ti = tex_index.get(cz * n + cx).copied().unwrap_or(0) as usize;
            let base = tints.get(ti).copied().unwrap_or([128, 128, 128]);
            let o = ((n - 1 - cz) * n + cx) * 4;
            for k in 0..3 {
                rgba[o + k] = (base[k] as f32 * shade).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// The hillshade of a whole terrain in one neutral grey tint: `(width, height, rgba)`, one pixel per corner, north up, opaque.
pub fn hillshade_grey(t: &Terrain) -> (usize, usize, Vec<u8>) {
    let n = t.n;
    let mut rgba = vec![255u8; n * n * 4];
    if n > 0 {
        shade_rect(&t.heights, &t.tex_index, n, &[GREY_TINT], [0, 0, n as u32 - 1, n as u32 - 1], &mut rgba);
    }
    (n, n, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terrain(n: usize, f: impl Fn(usize, usize) -> i16) -> Terrain {
        Terrain { size: (n as u32 - 1) * 2, n, heights: (0..n * n).map(|i| f(i % n, i / n)).collect(), tex_index: vec![0; n * n] }
    }

    #[test]
    fn a_slope_facing_north_west_is_brighter_than_one_facing_south_east() {
        // z grows to the north: height falls towards west and north, so the surface faces north-west
        let nw = terrain(9, |x, z| 9000 + 40 * x as i16 - 40 * z as i16);
        let se = terrain(9, |x, z| 9000 - 40 * x as i16 + 40 * z as i16);
        let (_, _, a) = hillshade_grey(&nw);
        let (_, _, b) = hillshade_grey(&se);
        let mid = (4 * 9 + 4) * 4;
        assert!(a[mid] > b[mid], "{} vs {}", a[mid], b[mid]);
    }

    #[test]
    fn hillshade_grey_is_one_opaque_pixel_per_corner() {
        let t = terrain(5, |_, _| 9000);
        let (w, h, px) = hillshade_grey(&t);
        assert_eq!((w, h, px.len()), (5, 5, 5 * 5 * 4));
        assert!(px.chunks(4).all(|p| p[3] == 255 && p[0] == p[1] && p[1] == p[2]));
    }
}
