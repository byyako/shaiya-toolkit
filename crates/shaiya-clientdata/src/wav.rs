//! RIFF/WAVE sound files: header facts, decoding to `f32` and a linear resampler. Read-only, no dependencies.
//!
//! What is read: PCM 8 bit (unsigned), PCM 16 bit (signed) and IMA/DVI ADPCM (4 bit), mono or stereo. Every other format tag is refused by
//! [`decode`] with `unsupported format (tag N, B bits)`; [`parse_header`] still reports its facts. A file over [`WAV_CAP`] is refused before
//! anything is allocated. Chunks are walked with the odd-size pad byte, unknown chunks are skipped and `data` may come before or after `fmt `.

use crate::error::{Cur, Error, Result};

/// The largest file read or decoded (64 MiB). The client's longest sound file is under 20 MB.
pub const WAV_CAP: usize = 64 << 20;

const WHAT: &str = "WAV file";

/// Format tag of integer PCM.
pub const TAG_PCM: u16 = 1;
/// Format tag of IMA/DVI ADPCM.
pub const TAG_IMA_ADPCM: u16 = 17;

/// The facts of the `fmt ` and `data` chunks.
#[derive(Clone, Debug, PartialEq)]
pub struct WavInfo {
    /// The format tag (1 = PCM, 17 = IMA ADPCM).
    pub tag: u16,
    pub channels: u16,
    pub rate: u32,
    /// Bits per sample as stored (4 for ADPCM).
    pub bits: u16,
    /// The block alignment as stored.
    pub block_align: u16,
    /// The size of the `data` chunk in bytes.
    pub data_len: u32,
    /// Where the `data` chunk's bytes start in the file.
    pub data_offset: usize,
    /// Playing time: sample frames / rate (from the byte rate for a tag that is not decoded).
    pub duration_s: f64,
}

impl WavInfo {
    /// Whether [`decode`] reads this format.
    pub fn is_supported(&self) -> bool {
        matches!((self.tag, self.bits), (TAG_PCM, 8 | 16) | (TAG_IMA_ADPCM, 4)) && (1..=2).contains(&self.channels)
    }

    /// "PCM 16 bit" or "IMA ADPCM 4 bit" or "format tag 3, 32 bits".
    pub fn format_name(&self) -> String {
        match self.tag {
            TAG_PCM => format!("PCM {} bit", self.bits),
            TAG_IMA_ADPCM => format!("IMA ADPCM {} bit", self.bits),
            t => format!("format tag {t}, {} bits", self.bits),
        }
    }
}

/// Decoded sound: interleaved samples in -1..1.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn duration_s(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }

    /// The frame at the fractional frame position `pos` (linear between the two frames around it; the last frame is held), written to `out`
    /// (one value per channel). `out` is left alone when there are no frames.
    pub fn lerp_frame(&self, pos: f64, out: &mut [f32]) {
        let ch = self.channels.max(1) as usize;
        let n = self.frames();
        if n == 0 || !pos.is_finite() {
            return;
        }
        let pos = pos.max(0.0);
        let i = pos.floor() as usize;
        if i + 1 >= n {
            for (c, o) in out.iter_mut().enumerate().take(ch) {
                *o = self.samples[(n - 1) * ch + c];
            }
            return;
        }
        let t = (pos - i as f64) as f32;
        for (c, o) in out.iter_mut().enumerate().take(ch) {
            let (a, b) = (self.samples[i * ch + c], self.samples[(i + 1) * ch + c]);
            *o = a + (b - a) * t;
        }
    }
}

fn fail(what: impl Into<String>, offset: usize) -> Error {
    Error::Format { file: WHAT.to_string(), what: what.into(), offset }
}

/// Reads the header facts: the chunk walk, the `fmt ` chunk (16, 18, 20, 40 bytes or any size from 16) and the `data` chunk.
pub fn parse_header(bytes: &[u8]) -> Result<WavInfo> {
    if bytes.len() > WAV_CAP {
        return Err(fail(format!("file is {} bytes, over the {} MiB limit", bytes.len(), WAV_CAP >> 20), 0));
    }
    let mut c = Cur::new(bytes, WHAT);
    if !c.bytes(4, "RIFF tag").is_ok_and(|b| b == b"RIFF") {
        return Err(fail("not a RIFF file", 0));
    }
    c.u32("RIFF size")?;
    if !c.bytes(4, "form type").is_ok_and(|b| b == b"WAVE") {
        return Err(fail("not a WAVE file", 8));
    }
    let mut fmt: Option<(u16, u16, u32, u32, u16, u16)> = None;
    let mut data: Option<(usize, u32)> = None;
    while c.left() >= 8 && (fmt.is_none() || data.is_none()) {
        let id: [u8; 4] = c.bytes(4, "chunk id")?.try_into().expect("4 bytes");
        let size = c.u32("chunk size")?;
        let body = c.p;
        match &id {
            b"fmt " => {
                if fmt.is_none() {
                    if size < 16 {
                        return Err(fail(format!("fmt chunk is {size} bytes, needs 16"), body));
                    }
                    if size as usize > c.left() {
                        return Err(fail(format!("fmt chunk of {size} bytes runs past the end of the file"), body));
                    }
                    let tag = c.u16("format tag")?;
                    let channels = c.u16("channels")?;
                    let rate = c.u32("sample rate")?;
                    let byte_rate = c.u32("byte rate")?;
                    let block_align = c.u16("block alignment")?;
                    let bits = c.u16("bits per sample")?;
                    fmt = Some((tag, channels, rate, byte_rate, block_align, bits));
                }
            }
            b"data" if data.is_none() => {
                if size as usize > c.left() {
                    return Err(fail(format!("data chunk of {size} bytes runs past the end of the file ({} bytes left)", c.left()), body));
                }
                data = Some((body, size));
            }
            _ => {}
        }
        // next chunk: the body and its pad byte when the size is odd
        let next = body as u64 + size as u64 + (size & 1) as u64;
        if next > bytes.len() as u64 {
            break;
        }
        c.p = next as usize;
    }
    let (tag, channels, rate, byte_rate, block_align, bits) = fmt.ok_or_else(|| fail("no fmt chunk", bytes.len()))?;
    let (data_offset, data_len) = data.ok_or_else(|| fail("no data chunk", bytes.len()))?;
    if channels == 0 {
        return Err(fail("0 channels", 0));
    }
    if rate == 0 {
        return Err(fail("sample rate 0", 0));
    }
    let bits = if tag == TAG_IMA_ADPCM && bits == 0 { 4 } else { bits };
    let supported = matches!((tag, bits), (TAG_PCM, 8 | 16) | (TAG_IMA_ADPCM, 4)) && channels <= 2;
    let duration_s = if supported {
        let frames = if tag == TAG_PCM { data_len as u64 / (channels as u64 * (bits as u64 / 8)) } else { adpcm_frames(data_len as usize, block_align as usize, channels as usize).unwrap_or(0) as u64 };
        frames as f64 / rate as f64
    } else if byte_rate > 0 {
        data_len as f64 / byte_rate as f64
    } else {
        0.0
    };
    Ok(WavInfo { tag, channels, rate, bits, block_align, data_len, data_offset, duration_s })
}

// ---------------------------------------------------------------- IMA ADPCM

/// The IMA ADPCM step sizes.
pub const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307,
    337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358,
    5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
/// How the step index moves for the low three bits of a code.
pub const IMA_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Sample frames in one block of `len` bytes (a header of 4 bytes per channel, then 4-bit codes); `None` when the block is too short for its
/// header.
fn block_frames(len: usize, channels: usize) -> Option<usize> {
    let head = 4 * channels;
    if len < head {
        return None;
    }
    // codes come in groups of 4 bytes (8 samples) per channel; a short last group still holds its whole bytes
    Some(1 + (len - head) * 2 / channels)
}

fn adpcm_frames(data_len: usize, block_align: usize, channels: usize) -> Option<usize> {
    if block_align < 4 * channels {
        return None;
    }
    let (full, rest) = (data_len / block_align, data_len % block_align);
    let mut n = full * block_frames(block_align, channels)?;
    if let Some(f) = block_frames(rest, channels) {
        n += f;
    }
    Some(n)
}

fn ima_step(pred: &mut i32, index: &mut i32, code: u8) -> i32 {
    let step = IMA_STEPS[*index as usize];
    let mut diff = step >> 3;
    if code & 1 != 0 {
        diff += step >> 2;
    }
    if code & 2 != 0 {
        diff += step >> 1;
    }
    if code & 4 != 0 {
        diff += step;
    }
    if code & 8 != 0 {
        diff = -diff;
    }
    *pred = (*pred + diff).clamp(-32768, 32767);
    *index = (*index + IMA_INDEX[(code & 7) as usize]).clamp(0, 88);
    *pred
}

fn decode_adpcm(block: &[u8], channels: usize, out: &mut Vec<f32>) {
    let frames = match block_frames(block.len(), channels) {
        Some(f) => f,
        None => return,
    };
    let mut pred = [0i32; 2];
    let mut index = [0i32; 2];
    let mut chans: [Vec<i16>; 2] = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    for ch in 0..channels {
        pred[ch] = i16::from_le_bytes([block[ch * 4], block[ch * 4 + 1]]) as i32;
        index[ch] = (block[ch * 4 + 2] as i32).min(88);
        chans[ch].push(pred[ch] as i16);
    }
    let codes = &block[4 * channels..];
    // groups of 4 bytes per channel, channels interleaved group by group; low nibble first
    for group in codes.chunks(4 * channels) {
        for ch in 0..channels {
            let Some(part) = group.get(ch * 4..) else { break };
            for &b in part.iter().take(4) {
                for code in [b & 15, b >> 4] {
                    let v = ima_step(&mut pred[ch], &mut index[ch], code);
                    chans[ch].push(v as i16);
                }
            }
        }
    }
    let n = chans[..channels].iter().map(Vec::len).min().unwrap_or(0);
    for i in 0..n {
        for ch in chans.iter().take(channels) {
            out.push(ch[i] as f32 / 32768.0);
        }
    }
}

/// Decodes the sound: PCM 8 (unsigned) and 16 (signed), IMA ADPCM, mono or stereo. The file size cap is checked before anything is allocated.
pub fn decode(bytes: &[u8]) -> Result<Pcm> {
    let info = parse_header(bytes)?;
    if !info.is_supported() {
        let what = if matches!((info.tag, info.bits), (TAG_PCM, 8 | 16) | (TAG_IMA_ADPCM, 4)) {
            format!("unsupported channel count {}", info.channels)
        } else {
            format!("unsupported format (tag {}, {} bits)", info.tag, info.bits)
        };
        return Err(fail(what, info.data_offset));
    }
    let data = &bytes[info.data_offset..info.data_offset + info.data_len as usize];
    let ch = info.channels as usize;
    let mut samples: Vec<f32>;
    match (info.tag, info.bits) {
        (TAG_PCM, 8) => {
            samples = Vec::with_capacity(data.len() / ch * ch);
            samples.extend(data[..data.len() / ch * ch].iter().map(|&b| (b as f32 - 128.0) / 128.0));
        }
        (TAG_PCM, _) => {
            let n = data.len() / (2 * ch) * ch;
            samples = Vec::with_capacity(n);
            samples.extend(data.as_chunks::<2>().0.iter().take(n).map(|b| i16::from_le_bytes(*b) as f32 / 32768.0));
        }
        _ => {
            let align = info.block_align as usize;
            if align < 4 * ch {
                return Err(fail(format!("ADPCM block alignment {align} is too small for {ch} channels"), info.data_offset));
            }
            samples = Vec::with_capacity(adpcm_frames(data.len(), align, ch).unwrap_or(0) * ch);
            for block in data.chunks(align) {
                decode_adpcm(block, ch, &mut samples);
            }
        }
    }
    Ok(Pcm { rate: info.rate, channels: info.channels, samples })
}

/// Linear resampling to `rate` Hz: the duration stays (the frame count scales by `rate / pcm.rate`, rounded). The same rate returns a copy; a
/// rate of 0 returns a copy too.
pub fn resample_linear(pcm: &Pcm, rate: u32) -> Pcm {
    if rate == 0 || rate == pcm.rate || pcm.frames() == 0 {
        return pcm.clone();
    }
    let ch = pcm.channels.max(1) as usize;
    let n_in = pcm.frames();
    let n_out = ((n_in as u128 * rate as u128 + pcm.rate as u128 / 2) / pcm.rate as u128) as usize;
    let step = pcm.rate as f64 / rate as f64;
    let mut samples = Vec::with_capacity(n_out * ch);
    let mut frame = vec![0f32; ch];
    for i in 0..n_out {
        pcm.lerp_frame(i as f64 * step, &mut frame);
        samples.extend_from_slice(&frame);
    }
    Pcm { rate, channels: pcm.channels, samples }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WAV built from `(id, body)` chunks (pad byte after an odd body).
    pub fn riff(chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = b"WAVE".to_vec();
        for (id, b) in chunks {
            body.extend_from_slice(*id);
            body.extend_from_slice(&(b.len() as u32).to_le_bytes());
            body.extend_from_slice(b);
            if b.len() % 2 == 1 {
                body.push(0);
            }
        }
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    pub fn fmt_chunk(tag: u16, channels: u16, rate: u32, bits: u16, block_align: u16, extra: &[u8]) -> Vec<u8> {
        let mut f = Vec::new();
        f.extend_from_slice(&tag.to_le_bytes());
        f.extend_from_slice(&channels.to_le_bytes());
        f.extend_from_slice(&rate.to_le_bytes());
        let byte_rate = if tag == TAG_PCM { rate * block_align as u32 } else { rate * block_align as u32 / 8 };
        f.extend_from_slice(&byte_rate.to_le_bytes());
        f.extend_from_slice(&block_align.to_le_bytes());
        f.extend_from_slice(&bits.to_le_bytes());
        f.extend_from_slice(extra);
        f
    }

    fn pcm16_mono(rate: u32, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        riff(&[(b"fmt ", fmt_chunk(1, 1, rate, 16, 2, &[])), (b"data", data)])
    }

    #[test]
    fn wav_pcm16_mono_reads_header_and_samples() {
        let wav = pcm16_mono(22050, &[0, 16384, -16384, 32767, -32768]);
        let info = parse_header(&wav).unwrap();
        assert_eq!((info.tag, info.channels, info.rate, info.bits, info.block_align, info.data_len), (1, 1, 22050, 16, 2, 10));
        assert!((info.duration_s - 5.0 / 22050.0).abs() < 1e-12);
        assert_eq!(info.format_name(), "PCM 16 bit");
        let pcm = decode(&wav).unwrap();
        assert_eq!((pcm.rate, pcm.channels), (22050, 1));
        assert_eq!(pcm.samples, vec![0.0, 0.5, -0.5, 32767.0 / 32768.0, -1.0]);
    }

    #[test]
    fn wav_pcm16_stereo_keeps_the_channels_interleaved() {
        let data: Vec<u8> = [100i16, -100, 200, -200].iter().flat_map(|s| s.to_le_bytes()).collect();
        let wav = riff(&[(b"fmt ", fmt_chunk(1, 2, 44100, 16, 4, &[])), (b"data", data)]);
        let pcm = decode(&wav).unwrap();
        assert_eq!(pcm.frames(), 2);
        assert_eq!(pcm.samples[0], 100.0 / 32768.0);
        assert_eq!(pcm.samples[1], -100.0 / 32768.0);
        assert!((parse_header(&wav).unwrap().duration_s - 2.0 / 44100.0).abs() < 1e-12);
    }

    #[test]
    fn wav_pcm8_unsigned_converts_to_minus_1_to_1() {
        let wav = riff(&[(b"fmt ", fmt_chunk(1, 1, 11025, 8, 1, &[])), (b"data", vec![0, 128, 255, 64])]);
        let pcm = decode(&wav).unwrap();
        assert_eq!(pcm.samples, vec![-1.0, 0.0, 127.0 / 128.0, -0.5]);
        assert_eq!(parse_header(&wav).unwrap().format_name(), "PCM 8 bit");
    }

    #[test]
    fn wav_skips_unknown_chunks_and_their_pad_byte() {
        // an odd-sized chunk before `data` (3 bytes + 1 pad), a LIST chunk after it, and `data` before `fmt `
        let odd = (b"scot", vec![1, 2, 3]);
        let data: Vec<u8> = [7i16, 8].iter().flat_map(|s| s.to_le_bytes()).collect();
        let wav = riff(&[odd.clone(), (b"fmt ", fmt_chunk(1, 1, 8000, 16, 2, &[])), (b"cue ", vec![9; 5]), (b"data", data.clone()), (b"LIST", vec![0; 6])]);
        let info = parse_header(&wav).unwrap();
        assert_eq!(info.data_len, 4);
        assert_eq!(decode(&wav).unwrap().samples.len(), 2);
        let first = riff(&[odd, (b"data", data), (b"fmt ", fmt_chunk(1, 1, 8000, 16, 2, &[]))]);
        assert_eq!(parse_header(&first).unwrap().rate, 8000, "data may come before fmt");
        assert_eq!(decode(&first).unwrap().samples.len(), 2);
    }

    #[test]
    fn wav_fmt_of_18_and_40_bytes_reads() {
        let data: Vec<u8> = [1i16, 2, 3].iter().flat_map(|s| s.to_le_bytes()).collect();
        let w18 = riff(&[(b"fmt ", fmt_chunk(1, 1, 22050, 16, 2, &[0, 0])), (b"data", data.clone())]);
        assert_eq!(parse_header(&w18).unwrap().data_len, 6);
        assert_eq!(decode(&w18).unwrap().samples.len(), 3);
        let w40 = riff(&[(b"fmt ", fmt_chunk(1, 1, 22050, 16, 2, &[0u8; 24])), (b"data", data.clone())]);
        assert_eq!(parse_header(&w40).unwrap().tag, 1);
        assert_eq!(decode(&w40).unwrap().samples.len(), 3);
        let w16 = riff(&[(b"fmt ", fmt_chunk(1, 1, 22050, 16, 2, &[])), (b"data", data)]);
        assert_eq!(decode(&w16).unwrap(), decode(&w40).unwrap());
        // fmt of 20 bytes (the ADPCM size) on a PCM file reads too
        let w20 = riff(&[(b"fmt ", fmt_chunk(1, 1, 22050, 16, 2, &[0, 0, 0, 0])), (b"data", vec![0, 0])]);
        assert!(parse_header(&w20).is_ok());
    }

    /// An IMA ADPCM encoder written for the test: it returns the codes and what a decoder must reproduce.
    fn ima_encode(samples: &[i16]) -> (Vec<u8>, Vec<i16>) {
        let mut pred = samples[0] as i32;
        let mut index = 0i32;
        let mut recon = vec![samples[0]];
        let mut nibbles = Vec::new();
        for &s in &samples[1..] {
            let step = IMA_STEPS[index as usize];
            let mut diff = s as i32 - pred;
            let mut code = 0u8;
            if diff < 0 {
                code = 8;
                diff = -diff;
            }
            let mut tmp = step;
            for bit in [4u8, 2, 1] {
                if diff >= tmp {
                    code |= bit;
                    diff -= tmp;
                }
                tmp >>= 1;
            }
            // the decoder's own reconstruction, spelled out again
            let mut delta = step >> 3;
            if code & 4 != 0 {
                delta += step;
            }
            if code & 2 != 0 {
                delta += step >> 1;
            }
            if code & 1 != 0 {
                delta += step >> 2;
            }
            pred = (pred + if code & 8 != 0 { -delta } else { delta }).clamp(-32768, 32767);
            index = (index + [-1, -1, -1, -1, 2, 4, 6, 8][(code & 7) as usize]).clamp(0, 88);
            recon.push(pred as i16);
            nibbles.push(code);
        }
        (nibbles.chunks(2).map(|p| p[0] | (p.get(1).copied().unwrap_or(0) << 4)).collect(), recon)
    }

    #[test]
    fn ima_adpcm_decodes_a_reference_block() {
        // 1 header sample + 16 coded samples = 4 + 8 bytes
        let samples: Vec<i16> = (0..17).map(|i| ((i as f32 * 0.7).sin() * 12000.0) as i16).collect();
        let (codes, recon) = ima_encode(&samples);
        assert_eq!(codes.len(), 8);
        let mut block = samples[0].to_le_bytes().to_vec();
        block.extend([0u8, 0]); // step index 0, reserved
        block.extend(&codes);
        let mut extra = vec![2, 0]; // cbSize
        extra.extend(17u16.to_le_bytes()); // samples per block
        let wav = riff(&[(b"fmt ", fmt_chunk(17, 1, 22050, 4, 12, &extra)), (b"fact", 17u32.to_le_bytes().to_vec()), (b"data", block)]);
        let info = parse_header(&wav).unwrap();
        assert_eq!((info.tag, info.bits, info.block_align), (17, 4, 12));
        assert!((info.duration_s - 17.0 / 22050.0).abs() < 1e-12);
        let pcm = decode(&wav).unwrap();
        let want: Vec<f32> = recon.iter().map(|&v| v as f32 / 32768.0).collect();
        assert_eq!(pcm.samples, want);
        // a hand-computed start: predictor 0, index 0, step 7: code 4 adds 7 + 0 + 0 = 7 (7>>3 = 0, no 1/2 bit, 4 bit adds 7), index +2;
        // step[2] = 9, code 0 adds 9>>3 = 1, index 1
        let hand = vec![0, 0, 0, 0, 0x04];
        let wav = riff(&[(b"fmt ", fmt_chunk(17, 1, 22050, 4, 5, &[2, 0, 3, 0])), (b"data", hand)]);
        let s = decode(&wav).unwrap().samples;
        assert_eq!(s, vec![0.0, 7.0 / 32768.0, 8.0 / 32768.0]);
    }

    #[test]
    fn ima_adpcm_stereo_blocks_interleave_groups_of_four_bytes() {
        let left: Vec<i16> = (0..9).map(|i| i * 300).collect();
        let right: Vec<i16> = (0..9).map(|i| -i * 200).collect();
        let (cl, rl) = ima_encode(&left);
        let (cr, rr) = ima_encode(&right);
        let mut block = Vec::new();
        for s in [left[0], right[0]] {
            block.extend(s.to_le_bytes());
            block.extend([0u8, 0]);
        }
        block.extend(&cl);
        block.extend(&cr);
        let wav = riff(&[(b"fmt ", fmt_chunk(17, 2, 22050, 4, block.len() as u16, &[])), (b"data", block)]);
        let pcm = decode(&wav).unwrap();
        assert_eq!(pcm.frames(), 9);
        for i in 0..9 {
            assert_eq!(pcm.samples[i * 2], rl[i] as f32 / 32768.0);
            assert_eq!(pcm.samples[i * 2 + 1], rr[i] as f32 / 32768.0);
        }
    }

    #[test]
    fn an_unsupported_tag_is_refused_with_its_number() {
        let wav = riff(&[(b"fmt ", fmt_chunk(3, 1, 44100, 32, 4, &[])), (b"data", vec![0; 8])]);
        // the header facts still read
        let info = parse_header(&wav).unwrap();
        assert_eq!((info.tag, info.bits, info.is_supported()), (3, 32, false));
        match decode(&wav) {
            Err(Error::Format { what, .. }) => assert_eq!(what, "unsupported format (tag 3, 32 bits)"),
            other => panic!("{other:?}"),
        }
        let wav = riff(&[(b"fmt ", fmt_chunk(2, 1, 22050, 4, 36, &[])), (b"data", vec![0; 8])]);
        assert!(decode(&wav).unwrap_err().to_string().contains("unsupported format (tag 2, 4 bits)"));
        // 24-bit PCM, 0xFFFE
        let wav = riff(&[(b"fmt ", fmt_chunk(1, 1, 22050, 24, 3, &[])), (b"data", vec![0; 6])]);
        assert!(decode(&wav).unwrap_err().to_string().contains("unsupported format (tag 1, 24 bits)"));
        let wav = riff(&[(b"fmt ", fmt_chunk(0xFFFE, 2, 48000, 16, 4, &[0u8; 24])), (b"data", vec![0; 8])]);
        assert!(decode(&wav).unwrap_err().to_string().contains("unsupported format (tag 65534, 16 bits)"));
    }

    #[test]
    fn a_data_chunk_past_the_end_is_refused() {
        let mut wav = pcm16_mono(22050, &[1, 2, 3, 4]);
        let n = wav.len();
        wav.truncate(n - 2);
        let e = parse_header(&wav).unwrap_err().to_string();
        assert!(e.contains("data chunk of 8 bytes runs past the end"), "{e}");
        assert!(decode(&wav).is_err());
        // a size of 4 GiB - 1 (a streamed file) is refused the same way
        let mut wav = pcm16_mono(22050, &[1, 2]);
        wav[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_header(&wav).is_err());
    }

    #[test]
    fn a_file_over_the_cap_is_refused_before_anything_is_allocated() {
        let mut wav = pcm16_mono(22050, &[1, 2]);
        wav.resize(WAV_CAP + 1, 0);
        let e = parse_header(&wav).unwrap_err().to_string();
        assert!(e.contains("over the 64 MiB limit"), "{e}");
        assert!(decode(&wav).unwrap_err().to_string().contains("64 MiB"));
        // exactly the cap is read (the declared data chunk is small, the file just has a long tail)
        wav.truncate(WAV_CAP);
        assert!(parse_header(&wav).is_ok());
    }

    #[test]
    fn malformed_and_truncated_files_are_errors_and_never_panic() {
        let good = riff(&[(b"fmt ", fmt_chunk(17, 2, 22050, 4, 20, &[2, 0, 9, 0])), (b"data", vec![0x55; 40])]);
        // every prefix of a good file, and every single-byte corruption of its header area
        for n in 0..good.len() {
            let _ = parse_header(&good[..n]);
            let _ = decode(&good[..n]);
        }
        for i in 0..good.len().min(64) {
            for v in [0u8, 1, 0x7f, 0xff] {
                let mut b = good.clone();
                b[i] = v;
                let _ = parse_header(&b);
                let _ = decode(&b);
            }
        }
        assert!(parse_header(b"").is_err());
        assert!(parse_header(b"RIFF").is_err());
        assert!(parse_header(b"RIFX\0\0\0\0WAVE").is_err());
        assert!(parse_header(b"RIFF\0\0\0\0AVI ").is_err());
        // no data chunk, no fmt chunk, fmt too small, zero channels, zero rate, a chunk size that wraps
        assert!(parse_header(&riff(&[(b"fmt ", fmt_chunk(1, 1, 8000, 16, 2, &[]))])).unwrap_err().to_string().contains("no data chunk"));
        assert!(parse_header(&riff(&[(b"data", vec![0; 4])])).unwrap_err().to_string().contains("no fmt chunk"));
        assert!(parse_header(&riff(&[(b"fmt ", vec![0; 8]), (b"data", vec![0; 4])])).is_err());
        assert!(parse_header(&riff(&[(b"fmt ", fmt_chunk(1, 0, 8000, 16, 2, &[])), (b"data", vec![0; 4])])).is_err());
        assert!(parse_header(&riff(&[(b"fmt ", fmt_chunk(1, 1, 0, 16, 2, &[])), (b"data", vec![0; 4])])).is_err());
        let mut wrap = b"RIFF\0\0\0\0WAVEjunk".to_vec();
        wrap.extend(u32::MAX.to_le_bytes());
        wrap.extend([0; 16]);
        assert!(parse_header(&wrap).is_err());
        // ADPCM with a block alignment too small for its header
        let bad = riff(&[(b"fmt ", fmt_chunk(17, 2, 22050, 4, 4, &[])), (b"data", vec![0; 16])]);
        assert!(decode(&bad).is_err());
        // three channels of PCM
        let three = riff(&[(b"fmt ", fmt_chunk(1, 3, 22050, 16, 6, &[])), (b"data", vec![0; 12])]);
        assert!(decode(&three).unwrap_err().to_string().contains("unsupported channel count 3"));
    }

    #[test]
    fn a_partial_last_frame_or_block_is_dropped_not_read() {
        let wav = riff(&[(b"fmt ", fmt_chunk(1, 2, 8000, 16, 4, &[])), (b"data", vec![0, 1, 2, 3, 4, 5])]);
        assert_eq!(decode(&wav).unwrap().frames(), 1);
        let wav = riff(&[(b"fmt ", fmt_chunk(17, 1, 8000, 4, 8, &[])), (b"data", vec![0, 0, 0, 0, 0x11, 0x11, 0x11, 0x11, 1, 0, 0, 0, 0x22])]);
        let pcm = decode(&wav).unwrap();
        // block 1 (8 bytes): 1 + 8 samples, block 2 (a 1-byte tail after its header): 1 + 2 samples
        assert_eq!(pcm.samples.len(), 9 + 3);
    }

    #[test]
    fn resampling_keeps_the_duration() {
        let pcm = Pcm { rate: 22050, channels: 2, samples: (0..22050 * 2).map(|i| (i as f32 * 0.001).sin()).collect() };
        for rate in [8000, 44100, 48000, 96000] {
            let r = resample_linear(&pcm, rate);
            assert_eq!((r.rate, r.channels), (rate, 2));
            assert!((r.duration_s() - pcm.duration_s()).abs() < 1.0 / rate as f64, "{rate}: {} vs {}", r.duration_s(), pcm.duration_s());
            assert_eq!(r.samples.len() % 2, 0);
        }
        assert_eq!(resample_linear(&pcm, 22050), pcm);
        // linear: the midpoint of two frames is their mean, the first frame is kept and the last is held
        let ramp = Pcm { rate: 1000, channels: 1, samples: vec![0.0, 1.0, 0.0] };
        let up = resample_linear(&ramp, 2000);
        assert_eq!(up.samples, vec![0.0, 0.5, 1.0, 0.5, 0.0, 0.0]);
        let empty = Pcm { rate: 1000, channels: 1, samples: vec![] };
        assert!(resample_linear(&empty, 48000).samples.is_empty());
    }
}
