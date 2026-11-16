//! `cargo xtask icons`: draws the generated tool icons (a flat rounded square per tool with one geometric mark) and writes
//! `assets/icons/<id>.png` (256), `<id>-512.png`, `<id>.ico` (PNG-compressed entries 16, 32, 48, 256) and `<id>-64.rgba` (window icon,
//! raw RGBA). No image crate: a small rasteriser, a PNG writer on miniz_oxide, and the ICO container by hand. The author may replace
//! the results with drawn icons later.

use std::path::Path;

/// One icon: its id (file stem), background colour and mark.
pub struct Icon {
    pub id: &'static str,
    pub bg: [u8; 3],
    pub mark: Mark,
}

#[derive(Clone, Copy)]
pub enum Mark {
    /// A terminal prompt: a chevron and an underscore.
    Chevron,
    /// An open box: a square with a lid line.
    Box,
}

pub const ICONS: &[Icon] = &[
    Icon { id: "shaiya-toolkit", bg: [0x3b, 0x3f, 0x9e], mark: Mark::Chevron },
    Icon { id: "shaiya", bg: [0x16, 0x69, 0x7a], mark: Mark::Chevron },
    Icon { id: "shaiya-workbench", bg: [0xb5, 0x54, 0x1c], mark: Mark::Box },
];

const BIG: usize = 512;

fn smooth(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Distance from a point to a segment.
fn seg_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py, ax, ay, bx, by) = (p.0, p.1, a.0, a.1, b.0, b.1);
    let (dx, dy) = (bx - ax, by - ay);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Signed distance to a rounded rectangle centred at (cx, cy) with half sizes (hx, hy) and corner radius r (negative inside).
fn round_rect(p: (f32, f32), c: (f32, f32), h: (f32, f32), r: f32) -> f32 {
    let (qx, qy) = ((p.0 - c.0).abs() - h.0 + r, (p.1 - c.1).abs() - h.1 + r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// The mark as a list of (a, b) segments with a common stroke width.
fn segments(mark: Mark) -> (Vec<((f32, f32), (f32, f32))>, f32) {
    match mark {
        Mark::Chevron => (vec![((165.0, 165.0), (270.0, 256.0)), ((270.0, 256.0), (165.0, 347.0)), ((300.0, 352.0), (375.0, 352.0))], 44.0),
        Mark::Box => {
            let (l, r, t, b) = (150.0, 362.0, 160.0, 372.0);
            (vec![((l, t), (r, t)), ((r, t), (r, b)), ((r, b), (l, b)), ((l, b), (l, t)), ((l, 236.0), (r, 236.0))], 34.0)
        }
    }
}

/// RGBA, premultiplied-free, `BIG` x `BIG`.
pub fn draw(icon: &Icon) -> Vec<u8> {
    let (segs, width) = segments(icon.mark);
    let mut px = vec![0u8; BIG * BIG * 4];
    let c = BIG as f32 / 2.0;
    for y in 0..BIG {
        for x in 0..BIG {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let bg_alpha = 1.0 - smooth(-0.75, 0.75, round_rect(p, (c, c), (c - 16.0, c - 16.0), 112.0));
            if bg_alpha <= 0.0 {
                continue;
            }
            let d = segs.iter().map(|&(a, b)| seg_dist(p, a, b)).fold(f32::MAX, f32::min) - width / 2.0;
            let mark_a = 1.0 - smooth(-0.75, 0.75, d);
            let mut rgb = [0f32; 3];
            for i in 0..3 {
                rgb[i] = icon.bg[i] as f32 * (1.0 - mark_a) + 255.0 * mark_a;
            }
            let o = (y * BIG + x) * 4;
            px[o] = rgb[0].round() as u8;
            px[o + 1] = rgb[1].round() as u8;
            px[o + 2] = rgb[2].round() as u8;
            px[o + 3] = (bg_alpha * 255.0).round() as u8;
        }
    }
    px
}

/// Box-filter downsample of a `BIG` x `BIG` RGBA image to `n` x `n` (alpha-weighted).
pub fn scale(big: &[u8], n: usize) -> Vec<u8> {
    let f = BIG / n;
    assert!(f * n == BIG, "n must divide {BIG}");
    let mut out = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, 0f32);
            for dy in 0..f {
                for dx in 0..f {
                    let o = ((y * f + dy) * BIG + x * f + dx) * 4;
                    let al = big[o + 3] as f32;
                    r += big[o] as f32 * al;
                    g += big[o + 1] as f32 * al;
                    b += big[o + 2] as f32 * al;
                    a += al;
                }
            }
            let o = (y * n + x) * 4;
            if a > 0.0 {
                out[o] = (r / a).round() as u8;
                out[o + 1] = (g / a).round() as u8;
                out[o + 2] = (b / a).round() as u8;
            }
            out[o + 3] = (a / (f * f) as f32).round() as u8;
        }
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// A PNG (8-bit RGBA, filter 0, zlib by miniz_oxide).
pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// An ICO container holding PNG-compressed entries (`(size, png bytes)`; 256 is stored as 0).
pub fn ico(entries: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * entries.len();
    for (size, data) in entries {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, data) in entries {
        out.extend_from_slice(data);
    }
    out
}

/// Writes every icon into `dir` (created when missing). Returns the file names written.
pub fn write_all(dir: &Path) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(dir)?;
    let mut written = Vec::new();
    let mut put = |name: String, bytes: Vec<u8>| -> std::io::Result<()> {
        std::fs::write(dir.join(&name), bytes)?;
        written.push(name);
        Ok(())
    };
    for icon in ICONS {
        let big = draw(icon);
        let at = |n: usize| scale(&big, n);
        put(format!("{}-512.png", icon.id), png(512, 512, &big))?;
        put(format!("{}.png", icon.id), png(256, 256, &at(256)))?;
        let entries: Vec<(u32, Vec<u8>)> = [16u32, 32, 48, 256].iter().map(|&n| (n, png(n, n, &at_any(&big, n as usize)))).collect();
        put(format!("{}.ico", icon.id), ico(&entries))?;
        put(format!("{}-64.rgba", icon.id), at_any(&big, 64))?;
    }
    Ok(written)
}

/// Like [`scale`] for sizes that do not divide 512 (48): resample from the 256 image with a box filter on fractional bounds.
fn at_any(big: &[u8], n: usize) -> Vec<u8> {
    if BIG % n == 0 {
        return scale(big, n);
    }
    let mut out = vec![0u8; n * n * 4];
    let s = BIG as f32 / n as f32;
    for y in 0..n {
        for x in 0..n {
            let (x0, x1) = ((x as f32 * s) as usize, (((x + 1) as f32 * s).ceil() as usize).min(BIG));
            let (y0, y1) = ((y as f32 * s) as usize, (((y + 1) as f32 * s).ceil() as usize).min(BIG));
            let (mut r, mut g, mut b, mut a, mut cnt) = (0f32, 0f32, 0f32, 0f32, 0f32);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let o = (yy * BIG + xx) * 4;
                    let al = big[o + 3] as f32;
                    r += big[o] as f32 * al;
                    g += big[o + 1] as f32 * al;
                    b += big[o + 2] as f32 * al;
                    a += al;
                    cnt += 1.0;
                }
            }
            let o = (y * n + x) * 4;
            if a > 0.0 {
                out[o] = (r / a).round() as u8;
                out[o + 1] = (g / a).round() as u8;
                out[o + 2] = (b / a).round() as u8;
            }
            out[o + 3] = (a / cnt).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_are_transparent_and_the_centre_is_drawn() {
        let big = draw(&ICONS[2]);
        assert_eq!(big[3], 0, "the corner is outside the rounded square");
        let centre = (256 * BIG + 256) * 4;
        assert_eq!(big[centre + 3], 255);
        // a background pixel keeps the colour of the tool
        let bg = (120 * BIG + 60) * 4;
        assert_eq!(&big[bg..bg + 3], &ICONS[2].bg);
    }

    #[test]
    fn png_and_ico_containers_are_well_formed() {
        let img = vec![255u8; 4 * 4 * 4];
        let p = png(4, 4, &img);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(p.ends_with(&[0xAE, 0x42, 0x60, 0x82]));
        let i = ico(&[(16, p.clone()), (256, p.clone())]);
        assert_eq!(&i[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(i[6], 16);
        assert_eq!(i[22], 0, "256 is stored as 0");
        let first = u32::from_le_bytes(i[18..22].try_into().unwrap()) as usize;
        assert_eq!(first, 6 + 32);
        assert_eq!(&i[first..first + 8], &p[..8]);
    }

    #[test]
    fn scaling_keeps_the_size_contract() {
        let big = draw(&ICONS[0]);
        assert_eq!(scale(&big, 64).len(), 64 * 64 * 4);
        assert_eq!(at_any(&big, 48).len(), 48 * 48 * 4);
    }
}
