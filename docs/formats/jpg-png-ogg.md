# Pictures and sounds of other kinds (`.jpg`, `.png`, `.ogg`)

## JPG

22 `.jpg` files are in US EP4.5: 15 in `interface/` and 7 in `Lensflare/` (validated on file contents). 15 are baseline (3 components, 1024 x 768 or 1280 x 1024) and 7 are progressive (128 x 128 or 256 x 256, one or three components). The toolkit decodes baseline and progressive files with the `zune-jpeg` library to RGBA; a picture with one component gets its grey in all three colour channels. A file larger than 64 Mi pixels is refused before decoding (`a_picture_over_64_mi_pixels_is_refused_by_the_pixel_cap_before_decoding`, a JPEG and a PNG of 10000 x 10000).

| Fact | Evidence |
|---|---|
| All 22 decode, with the size their header says | validated on file contents (US EP4.5): `every_jpg_of_the_us_client_decodes` (the counts and sizes in this section are measured and printed by that test; the decoding and the header sizes are asserted) |
| Grey replicated, progressive decodes like baseline, damaged files are errors | `a_grey_jpg_gets_the_same_value_in_all_colour_channels`, `a_progressive_jpg_decodes_like_the_baseline_one`, `a_damaged_picture_is_an_error_and_never_panics` (synthetic files) |

## PNG

No PNG is in US EP4.5 (measured offline from its index, not asserted by a test). The toolkit decodes any PNG colour type and depth to RGBA with the `png` library (16 bit reduced to 8, indexed colour expanded; test `png_made_by_png_encode_decodes_back`).

## OGG

No `.ogg` file is in US EP4.5 or any of the other matrix clients (measured offline from the indexes, not asserted by a test). The toolkit reads the Vorbis identification header of the first page and the granule position of the last page, and shows "Ogg Vorbis, N channels, R Hz, S s (not played)". Nothing is decoded and nothing is played (test `ogg_info_reads_channels_rate_and_duration`).

## What the toolkit does with them

| Program | Use |
|---|---|
| `shaiya-clientdata` `texture`, `ogg` | `decode_jpg`, `decode_png`, `picture_info`, `ogg_info` |
| `shaiya info` | names each file with its header facts (test `a_jpg_and_a_png_are_described_by_their_headers`, `an_ogg_is_described_by_its_vorbis_header`) |
| Shaiya Toolkit, Archive tab | a JPG or PNG is shown like any picture (test `sounds_and_pictures_are_previewed`) |

Nothing writes these files. `cargo xtask guard` refuses a tracked `.ogg`.
