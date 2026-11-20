//! Texture decoding to RGBA8: DDS (DXT1 to DXT5, DXT2/DXT4 with the colour un-premultiplied, and uncompressed bit-mask formats, first mip
//! level), TGA (uncompressed and RLE true-colour, 24/32 bit) and BMP (24/32 bit, some "dds" files of the client are really bitmaps). No
//! dependencies.

use crate::error::{Cur, Result};

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row 0 = top.
    pub rgba: Vec<u8>,
}

impl Image {
    /// Average colour of the visible pixels (alpha > 0), used to tint the 2D terrain; mid grey for an empty image.
    pub fn average_colour(&self) -> [u8; 3] {
        let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
        for p in self.rgba.as_chunks::<4>().0.iter() {
            if p[3] > 0 {
                r += p[0] as u64;
                g += p[1] as u64;
                b += p[2] as u64;
                n += 1;
            }
        }
        if n == 0 {
            return [128, 128, 128];
        }
        [(r / n) as u8, (g / n) as u8, (b / n) as u8]
    }

    /// The stand-in for a texture that is missing or cannot be decoded: a magenta / black checker.
    pub fn checker(size: u32) -> Image {
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let on = ((x / (size / 4).max(1)) + (y / (size / 4).max(1))).is_multiple_of(2);
                rgba.extend_from_slice(if on { &[255, 0, 255, 255] } else { &[0, 0, 0, 255] });
            }
        }
        Image { width: size, height: size, rgba }
    }
}

fn limits(r: &Cur, w: u32, h: u32) -> Result<()> {
    if w == 0 || h == 0 || w > 16_384 || h > 16_384 {
        return Err(r.fail(format!("image size {w}x{h} is out of range")));
    }
    Ok(())
}

/// Decode by magic number: `DDS `, `BM`, otherwise TGA.
pub fn decode(data: &[u8], file: &str) -> Result<Image> {
    if data.starts_with(b"DDS ") {
        decode_dds(data, file)
    } else if data.starts_with(b"BM") {
        decode_bmp(data, file)
    } else {
        decode_tga(data, file)
    }
}

// ---------------------------------------------------------------- DDS

fn rgb565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 31) as u32;
    let g = ((v >> 5) & 63) as u32;
    let b = (v & 31) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

/// One 4x4 colour block into `out` (16 pixels RGBA). `four_colour`: DXT3/5 always use the 4-colour mode.
fn colour_block(b: &[u8], four_colour: bool, out: &mut [[u8; 4]; 16]) {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |a: u8, b: u8, wa: u32, wb: u32, d: u32| ((a as u32 * wa + b as u32 * wb) / d) as u8;
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || four_colour {
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 2, 1, 3);
            pal[3][i] = mix(p0[i], p1[i], 1, 2, 3);
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 1, 1, 2);
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = pal[((bits >> (2 * i)) & 3) as usize];
    }
}

fn dxt_alpha5(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7u32 {
            pal[1 + i as usize] = ((7 - i) * a0 + i * a1) / 7;
        }
    } else {
        for i in 1..5u32 {
            pal[1 + i as usize] = ((5 - i) * a0 + i * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &v) in b[2..8].iter().enumerate() {
        bits |= (v as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (3 * i)) & 7) as usize] as u8;
    }
    out
}

/// DXT2 and DXT4 store the colour multiplied by alpha. Back to straight alpha: c = round(c * 255 / a); alpha 0 gives 0,0,0,0.
fn unpremultiply(p: &mut [u8; 4]) {
    let a = p[3] as u32;
    if a == 0 {
        *p = [0; 4];
    } else if a != 255 {
        for c in p.iter_mut().take(3) {
            *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
}

fn mask_shift(mask: u32) -> (u32, u32) {
    if mask == 0 {
        return (0, 0);
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).trailing_ones();
    (shift, bits)
}

fn extract(v: u32, mask: u32) -> u8 {
    let (shift, bits) = mask_shift(mask);
    if bits == 0 {
        return 255;
    }
    let x = (v & mask) >> shift;
    let max = if bits >= 32 { u32::MAX } else { (1u32 << bits) - 1 };
    ((x as u64 * 255 + (max as u64) / 2) / max as u64) as u8
}

pub fn decode_dds(data: &[u8], file: &str) -> Result<Image> {
    let mut r = Cur::new(data, file);
    if r.bytes(4, "magic")? != b"DDS " {
        return Err(r.fail("not a DDS file"));
    }
    let size = r.u32("header size")?;
    if size != 124 {
        return Err(r.fail(format!("DDS header size {size}")));
    }
    r.u32("flags")?;
    let h = r.u32("height")?;
    let w = r.u32("width")?;
    limits(&r, w, h)?;
    r.skip(4 + 4 + 4 + 44, "pitch, depth, mips, reserved")?;
    let pf_size = r.u32("pixel format size")?;
    let pf_flags = r.u32("pixel format flags")?;
    let fourcc = r.bytes(4, "fourcc")?.to_vec();
    let bits = r.u32("bit count")?;
    let (rm, gm, bm, am) = (r.u32("mask")?, r.u32("mask")?, r.u32("mask")?, r.u32("mask")?);
    if pf_size != 32 {
        return Err(r.fail(format!("DDS pixel format size {pf_size}")));
    }
    r.skip(20, "caps")?;
    let (w, h) = (w as usize, h as usize);
    let mut rgba = vec![0u8; w * h * 4];
    if pf_flags & 4 != 0 {
        let block = match &fourcc[..] {
            b"DXT1" => 8,
            b"DXT2" | b"DXT3" | b"DXT4" | b"DXT5" => 16,
            other => return Err(r.fail(format!("unsupported DDS format {:?}", String::from_utf8_lossy(other)))),
        };
        let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
        let blocks = r.bytes(bw * bh * block, "DXT blocks")?;
        for by in 0..bh {
            for bx in 0..bw {
                let b = &blocks[(by * bw + bx) * block..][..block];
                let mut px = [[0u8; 4]; 16];
                match &fourcc[..] {
                    b"DXT1" => colour_block(b, false, &mut px),
                    b"DXT2" | b"DXT3" => {
                        colour_block(&b[8..], true, &mut px);
                        for (i, p) in px.iter_mut().enumerate() {
                            let nib = (b[i / 2] >> (4 * (i % 2))) & 15;
                            p[3] = nib * 17;
                        }
                    }
                    _ => {
                        colour_block(&b[8..], true, &mut px);
                        let a = dxt_alpha5(b);
                        for (p, a) in px.iter_mut().zip(a) {
                            p[3] = a;
                        }
                    }
                }
                if matches!(&fourcc[..], b"DXT2" | b"DXT4") {
                    for p in px.iter_mut() {
                        unpremultiply(p);
                    }
                }
                for (i, p) in px.iter().enumerate() {
                    let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                    if x < w && y < h {
                        rgba[(y * w + x) * 4..][..4].copy_from_slice(p);
                    }
                }
            }
        }
    } else if pf_flags & 0x40 != 0 && matches!(bits, 16 | 24 | 32) {
        let bytes = (bits / 8) as usize;
        let raw = r.bytes(w * h * bytes, "pixels")?;
        let has_alpha = pf_flags & 1 != 0 && am != 0;
        for (i, px) in raw.chunks_exact(bytes).enumerate() {
            let mut v = 0u32;
            for (k, &b) in px.iter().enumerate() {
                v |= (b as u32) << (8 * k);
            }
            rgba[i * 4] = extract(v, rm);
            rgba[i * 4 + 1] = extract(v, gm);
            rgba[i * 4 + 2] = extract(v, bm);
            rgba[i * 4 + 3] = if has_alpha { extract(v, am) } else { 255 };
        }
    } else {
        return Err(r.fail(format!("unsupported DDS pixel format (flags {pf_flags:#x}, {bits} bits)")));
    }
    Ok(Image { width: w as u32, height: h as u32, rgba })
}

// ---------------------------------------------------------------- TGA

pub fn decode_tga(data: &[u8], file: &str) -> Result<Image> {
    let mut r = Cur::new(data, file);
    let id_len = r.u8("id length")? as usize;
    let cmap_type = r.u8("colour map type")?;
    let kind = r.u8("image type")?;
    r.skip(5, "colour map spec")?;
    r.skip(4, "origin")?;
    let w = r.u16("width")? as u32;
    let h = r.u16("height")? as u32;
    let depth = r.u8("pixel depth")?;
    let desc = r.u8("descriptor")?;
    limits(&r, w, h)?;
    if cmap_type != 0 || !matches!(kind, 2 | 10) || !matches!(depth, 24 | 32) {
        return Err(r.fail(format!("unsupported TGA (type {kind}, {depth} bit, colour map {cmap_type})")));
    }
    r.skip(id_len, "image id")?;
    let bpp = (depth / 8) as usize;
    let (w, h) = (w as usize, h as usize);
    let mut px = Vec::with_capacity(w * h * bpp);
    if kind == 2 {
        px.extend_from_slice(r.bytes(w * h * bpp, "pixels")?);
    } else {
        while px.len() < w * h * bpp {
            let head = r.u8("RLE packet")?;
            let n = (head & 0x7f) as usize + 1;
            if head & 0x80 != 0 {
                let one = r.bytes(bpp, "RLE run")?;
                for _ in 0..n {
                    px.extend_from_slice(one);
                }
            } else {
                px.extend_from_slice(r.bytes(n * bpp, "RLE literal")?);
            }
        }
        px.truncate(w * h * bpp);
    }
    let top_first = desc & 0x20 != 0;
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if top_first { y } else { h - 1 - y };
        for x in 0..w {
            let s = &px[(sy * w + x) * bpp..][..bpp];
            let d = &mut rgba[(y * w + x) * 4..][..4];
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = if bpp == 4 { s[3] } else { 255 };
        }
    }
    Ok(Image { width: w as u32, height: h as u32, rgba })
}

// ---------------------------------------------------------------- BMP

pub fn decode_bmp(data: &[u8], file: &str) -> Result<Image> {
    let mut r = Cur::new(data, file);
    if r.bytes(2, "magic")? != b"BM" {
        return Err(r.fail("not a BMP file"));
    }
    r.skip(8, "file header")?;
    let off = r.u32("pixel data offset")? as usize;
    let hs = r.u32("header size")?;
    if hs < 40 {
        return Err(r.fail(format!("BMP header size {hs}")));
    }
    let w = r.i32("width")?;
    let h = r.i32("height")?;
    r.u16("planes")?;
    let bits = r.u16("bit count")?;
    let compression = r.u32("compression")?;
    if w <= 0 || h == 0 || compression != 0 || !matches!(bits, 24 | 32) {
        return Err(r.fail(format!("unsupported BMP ({w}x{h}, {bits} bit, compression {compression})")));
    }
    limits(&r, w as u32, h.unsigned_abs())?;
    let bottom_up = h > 0;
    let (w, h) = (w as usize, h.unsigned_abs() as usize);
    let bpp = (bits / 8) as usize;
    let stride = (w * bpp).div_ceil(4) * 4;
    r.p = off;
    let raw = r.bytes(stride * h, "pixels")?;
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let sy = if bottom_up { h - 1 - y } else { y };
        for x in 0..w {
            let s = &raw[sy * stride + x * bpp..][..bpp];
            let d = &mut rgba[(y * w + x) * 4..][..4];
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = 255;
        }
    }
    Ok(Image { width: w as u32, height: h as u32, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dds_header(w: u32, h: u32, flags: u32, fourcc: &[u8; 4], bits: u32, masks: [u32; 4]) -> Vec<u8> {
        let mut v = b"DDS ".to_vec();
        let mut put = |x: u32| v.extend_from_slice(&x.to_le_bytes());
        put(124);
        put(0x1007);
        put(h);
        put(w);
        for _ in 0..(1 + 1 + 1 + 11) {
            put(0);
        }
        put(32);
        put(flags);
        v.extend_from_slice(fourcc);
        let mut put = |x: u32| v.extend_from_slice(&x.to_le_bytes());
        put(bits);
        for m in masks {
            put(m);
        }
        for _ in 0..5 {
            put(0);
        }
        v
    }

    #[test]
    fn dxt1_block() {
        let mut d = dds_header(4, 4, 4, b"DXT1", 0, [0; 4]);
        // c0 = pure red (0xF800), c1 = pure blue (0x001F); indices: pixel i uses i % 4
        d.extend_from_slice(&[0x00, 0xf8, 0x1f, 0x00, 0xe4, 0xe4, 0xe4, 0xe4]);
        let img = decode_dds(&d, "t").unwrap();
        assert_eq!((img.width, img.height), (4, 4));
        assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&img.rgba[4..8], &[0, 0, 255, 255]);
        assert_eq!(&img.rgba[8..12], &[170, 0, 85, 255]);
        assert_eq!(&img.rgba[12..16], &[85, 0, 170, 255]);
    }

    #[test]
    fn dxt5_alpha_and_truncation() {
        let mut d = dds_header(4, 4, 4, b"DXT5", 0, [0; 4]);
        d.extend_from_slice(&[255, 0, 0, 0, 0, 0, 0, 0]); // alpha0 = 255, all indices 0
        d.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]); // white, white
        let img = decode_dds(&d, "t").unwrap();
        assert_eq!(&img.rgba[0..4], &[255, 255, 255, 255]);
        assert!(decode_dds(&d[..d.len() - 1], "t").is_err());
        assert!(decode_dds(&d[..20], "t").is_err());
    }

    // DXT3 block: alpha of pixel i = i * 17; both colours = c (565).
    fn dxt3_block(c: u16) -> Vec<u8> {
        let mut b = Vec::new();
        for i in 0..8u8 {
            b.push((2 * i) | ((2 * i + 1) << 4));
        }
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b
    }

    // DXT5 block: alpha0 = 200, alpha1 = 100, every index 1 (alpha 100); both colours = c (565).
    fn dxt5_block(c: u16) -> Vec<u8> {
        let mut b = vec![200, 100, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24];
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&c.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b
    }

    #[test]
    fn dxt3_and_dxt5_bytes_are_unchanged_by_dxt2_dxt4_support() {
        // 565 0x07E0 is pure green; expected bytes are written out, not derived from the decoder.
        let mut d = dds_header(4, 4, 4, b"DXT3", 0, [0; 4]);
        d.extend_from_slice(&dxt3_block(0x07e0));
        let img = decode_dds(&d, "t").unwrap();
        for i in 0..16usize {
            assert_eq!(&img.rgba[i * 4..i * 4 + 4], &[0, 255, 0, (i as u8) * 17], "dxt3 pixel {i}");
        }
        let mut d = dds_header(4, 4, 4, b"DXT5", 0, [0; 4]);
        d.extend_from_slice(&dxt5_block(0x07e0));
        let img = decode_dds(&d, "t").unwrap();
        for i in 0..16usize {
            assert_eq!(&img.rgba[i * 4..i * 4 + 4], &[0, 255, 0, 100], "dxt5 pixel {i}");
        }
    }

    #[test]
    fn dxt2_is_dxt3_blocks_with_the_colour_un_premultiplied() {
        // stored 565 0x0400: g = 32 -> (32*255+31)/63 = 130
        let mut d = dds_header(4, 4, 4, b"DXT2", 0, [0; 4]);
        d.extend_from_slice(&dxt3_block(0x0400));
        let img = decode_dds(&d, "t").unwrap();
        assert_eq!(&img.rgba[0..4], &[0, 0, 0, 0], "alpha 0 gives 0,0,0,0");
        assert_eq!(&img.rgba[60..64], &[0, 130, 0, 255], "alpha 255 keeps the colour");
        assert_eq!(&img.rgba[20..24], &[0, 255, 0, 85], "130 * 255 / 85 clamps to 255");
        assert_eq!(&img.rgba[40..44], &[0, 195, 0, 170], "round(130 * 255 / 170)");
        let mut d3 = dds_header(4, 4, 4, b"DXT3", 0, [0; 4]);
        d3.extend_from_slice(&dxt3_block(0x0400));
        let img3 = decode_dds(&d3, "t").unwrap();
        for i in 0..16usize {
            assert_eq!(img.rgba[i * 4 + 3], img3.rgba[i * 4 + 3], "alpha is the DXT3 alpha");
        }
    }

    #[test]
    fn dxt4_is_dxt5_blocks_with_the_colour_un_premultiplied() {
        // stored 565 0x4000: r = 8 -> (8*255+15)/31 = 66; alpha 100 -> (66*255+50)/100 = 168
        let mut d = dds_header(4, 4, 4, b"DXT4", 0, [0; 4]);
        d.extend_from_slice(&dxt5_block(0x4000));
        let img = decode_dds(&d, "t").unwrap();
        assert_eq!(&img.rgba[0..4], &[168, 0, 0, 100]);
        assert_eq!(&img.rgba[60..64], &[168, 0, 0, 100]);
        assert!(decode_dds(&d[..d.len() - 1], "t").is_err());
    }

    #[test]
    fn un_premultiplying_stays_close_to_the_straight_colour() {
        for straight in [0u32, 1, 40, 128, 200, 255] {
            for a in 1..=255u32 {
                let stored = (straight * a + 127) / 255;
                let mut p = [stored as u8, stored as u8, stored as u8, a as u8];
                unpremultiply(&mut p);
                // half a step of the stored value, scaled by 255 / a, plus rounding
                let allowed = (255 / a as i64) / 2 + 1;
                assert!((p[0] as i64 - straight as i64).abs() <= allowed, "straight {straight} alpha {a} -> {}", p[0]);
            }
        }
        let mut p = [9, 9, 9, 0];
        unpremultiply(&mut p);
        assert_eq!(p, [0, 0, 0, 0]);
    }

    #[test]
    fn uncompressed_bgra() {
        let mut d = dds_header(1, 1, 0x41, b"\0\0\0\0", 32, [0xff0000, 0xff00, 0xff, 0xff00_0000]);
        d.extend_from_slice(&[10, 20, 30, 40]); // b, g, r, a
        let img = decode_dds(&d, "t").unwrap();
        assert_eq!(img.rgba, vec![30, 20, 10, 40]);
    }

    #[test]
    fn tga_rle_and_orientation() {
        let mut t = vec![0, 0, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1, 0, 24, 0];
        t.extend_from_slice(&[0x81, 1, 2, 3]); // a run of 2 pixels (b, g, r) = (1, 2, 3)
        let img = decode_tga(&t, "t").unwrap();
        assert_eq!(img.rgba, vec![3, 2, 1, 255, 3, 2, 1, 255]);
        assert!(decode_tga(&t[..t.len() - 1], "t").is_err());
        assert!(decode_tga(&[1, 2, 3], "t").is_err());
    }

    #[test]
    fn average_and_checker() {
        let img = Image { width: 2, height: 1, rgba: vec![10, 20, 30, 255, 30, 40, 50, 255] };
        assert_eq!(img.average_colour(), [20, 30, 40]);
        let c = Image::checker(8);
        assert_eq!(c.rgba.len(), 8 * 8 * 4);
    }
}
