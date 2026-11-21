//! A PNG encoder (filter 0, zlib by miniz_oxide): enough for a screenshot, no image crate. The `--shot` flag of every program and the
//! SVMap Editor's exports use it.

/// Encodes 8-bit RGBA pixels (`w * h * 4` bytes, rows top to bottom) as a PNG.
pub fn png_encode(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
    for row in rgba.chunks(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&raw, 6);
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

/// The pixels of an egui screenshot as RGBA bytes.
pub fn rgba_of(img: &egui::ColorImage) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(img.pixels.len() * 4);
    for px in &img.pixels {
        rgba.extend_from_slice(&px.to_array());
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_signature_and_chunks() {
        let png = png_encode(2, 2, &[255u8; 16]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert!(png.windows(4).any(|w| w == b"IDAT"));
        assert!(png.ends_with(&[0xAE, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn the_pixels_of_a_screenshot_are_rgba_in_row_order() {
        let mut img = egui::ColorImage::filled([2, 1], egui::Color32::from_rgb(1, 2, 3));
        img.pixels[1] = egui::Color32::from_rgb(9, 8, 7);
        assert_eq!(rgba_of(&img), vec![1, 2, 3, 255, 9, 8, 7, 255]);
    }
}
