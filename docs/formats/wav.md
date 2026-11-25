# Sounds (`.wav`)

Every sound file in the client archive is a RIFF/WAVE file. No other sound format is in any of the nine matrix clients (measured offline from the index of each client for this release, not asserted by a test: only `.wav` files are listed, no mp3, ogg or wma).

All numbers are little-endian.

## Layout as read

```
"RIFF"  u32 size  "WAVE"                  the size field is not checked
chunks, each:  4-byte id, u32 size, size bytes, one pad byte when size is odd
  "fmt "   format tag, channels, sample rate, byte rate, block alignment, bits   (16, 18, 20 or 40 bytes, or more)
  "data"   the sound
  anything else (cue, PAD, scot, LIST, fact ...) is skipped
```

`data` may come before or after `fmt `. A `data` chunk whose size runs past the end of the file is refused. A file over 64 MiB is refused before anything is allocated. A sound that would decode to more than 64 Mi numbers (an ADPCM file grows eight times, a PCM 8 bit file four times in memory) or whose sample rate is below 1,000 or above 384,000 Hz is refused by `decode` before the output is allocated, and `resample_linear` refuses a result over the same size or a rate above 384,000 (test `a_decoded_size_or_a_rate_beyond_the_limits_is_refused_before_allocating`).

| Field | Evidence |
|---|---|
| Chunk walk with the pad byte, `fmt ` of 16, 18 and 20 bytes, other chunks skipped | validated on file contents (US EP4.5): `every_wav_of_the_us_client_reads_its_header` reads all 1,019 headers; unit tests `wav_skips_unknown_chunks_and_their_pad_byte`, `wav_fmt_of_18_and_40_bytes_reads` |
| Format tag 1 (PCM) in 1,018 files, tag 17 (IMA ADPCM, 4 bit) in 1 file | validated on file contents (US EP4.5): the same test |
| PCM 8 bit is unsigned, 16 bit is signed | the standard WAVE convention (not measured on the client's files, and not tested in the game: decoding a file without an error cannot show a sign); `wav_pcm8_unsigned_converts_to_minus_1_to_1` tests the conversion |
| IMA ADPCM block layout (4 bytes of header per channel, then 4-bit codes, groups of 4 bytes per channel, low nibble first) | the standard layout; checked on the one US EP4.5 file (it decodes to the length its header says) and by `ima_adpcm_decodes_a_reference_block` |

## What is read and what is refused

- Read: PCM 8 bit, PCM 16 bit and IMA ADPCM, mono or stereo. They decode to 32-bit floats between -1 and 1.
- Refused by the decoder: any other format tag (the message is `unsupported format (tag N, B bits)`, test `an_unsupported_tag_is_refused_with_its_number`) and more than two channels. The header facts of such a file are still read.
- A partial last frame or ADPCM block is dropped, not read (`a_partial_last_frame_or_block_is_dropped_not_read`).
- A damaged or truncated file is an error and never a panic (`malformed_and_truncated_files_are_errors_and_never_panic`).

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `wav` | `parse_header` (header facts), `decode`, `resample_linear` |
| `shaiya info` | names the file with its format, channels, rate and length (test `a_wav_is_described_by_its_header_whatever_its_name`) |
| Shaiya Toolkit, Archive tab | shows the header line and plays the sound, see [the Toolkit page](../tools/toolkit.md) |

Nothing writes a `.wav` file. `cargo xtask guard` refuses a tracked file with this extension.

## Verified on

US EP4.5 (patch 108), read only: all 1,019 `.wav` files read and decode (the test asserts that all of them do). The numbers below are measured and printed by the opt-in test `every_wav_of_the_us_client_reads_its_header`, not asserted: 4,434 seconds of sound in all; of the 1,018 PCM files, 10 are 8 bit and 1,008 are 16 bit; 34 are 44,100 Hz stereo (the music).

## Other episodes

The eight other matrix clients have their index only: their `.wav` files are counted (see the [version matrix](../version-matrix.md)), none was read. A file of another format tag there is refused with its tag number rather than guessed.
