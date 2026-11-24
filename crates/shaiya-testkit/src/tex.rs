//! Byte builders for `.DDS` textures, written from the documented header without using `shaiya-clientdata`: an independent writer.
//! Synthetic data only (generated patterns, never game art).

fn header(w: u32, h: u32, pf_flags: u32, fourcc: &[u8; 4], bits: u32, masks: [u32; 4]) -> Vec<u8> {
    let mut v = b"DDS ".to_vec();
    let put = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    put(&mut v, 124);
    put(&mut v, 0x1007);
    put(&mut v, h);
    put(&mut v, w);
    for _ in 0..14 {
        put(&mut v, 0);
    }
    put(&mut v, 32);
    put(&mut v, pf_flags);
    v.extend_from_slice(fourcc);
    put(&mut v, bits);
    for m in masks {
        put(&mut v, m);
    }
    for _ in 0..5 {
        put(&mut v, 0);
    }
    v
}

/// An uncompressed 32-bit `.DDS` with an alpha channel; `pixel(x, y)` is `[r, g, b, a]`, row 0 is the top row.
pub fn dds_rgba(w: u32, h: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let mut d = header(w, h, 0x41, &[0; 4], 32, [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b, a] = pixel(x, y);
            d.extend_from_slice(&[b, g, r, a]);
        }
    }
    d
}

/// A DXT1 `.DDS` whose every 4 x 4 block is the solid colour `colour` (RGB565); `w` and `h` are multiples of 4.
pub fn dds_dxt1_solid(w: u32, h: u32, colour: u16) -> Vec<u8> {
    assert!(w.is_multiple_of(4) && h.is_multiple_of(4));
    let mut d = header(w, h, 4, b"DXT1", 0, [0; 4]);
    for _ in 0..(w / 4) * (h / 4) {
        d.extend_from_slice(&colour.to_le_bytes());
        d.extend_from_slice(&colour.to_le_bytes());
        d.extend_from_slice(&[0, 0, 0, 0]);
    }
    d
}

/// A checker of `cell`-pixel squares in two colours (opaque), `size` pixels a side.
pub fn checker(size: u32, cell: u32, a: [u8; 3], b: [u8; 3]) -> Vec<u8> {
    dds_rgba(size, size, |x, y| {
        let c = if ((x / cell) + (y / cell)).is_multiple_of(2) { a } else { b };
        [c[0], c[1], c[2], 255]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_headers_are_128_bytes_and_the_pixels_follow() {
        let d = dds_rgba(4, 2, |x, y| [x as u8, y as u8, 7, 200]);
        assert_eq!((d.len(), &d[..4]), (128 + 4 * 2 * 4, &b"DDS "[..]));
        assert_eq!(&d[128..132], &[7, 0, 0, 200], "B, G, R, A of the first pixel");
        let x = dds_dxt1_solid(8, 4, 0xF800);
        assert_eq!(x.len(), 128 + 2 * 8);
        assert_eq!(&x[84..88], b"DXT1");
    }
}
