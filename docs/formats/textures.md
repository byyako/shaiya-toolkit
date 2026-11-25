# Textures (DDS and TGA)

JPG and PNG pictures are read too: see [JPG, PNG and OGG](jpg-png-ogg.md).

The toolkit reads the client's pictures and writes two kinds of its own for new maps.

## What is read

`shaiya_clientdata::texture::decode` picks the decoder from the first bytes and returns RGBA8 pixels:

| Magic | Format | Supported |
|---|---|---|
| `DDS ` | DirectDraw Surface | DXT1 to DXT5 (DXT2 and DXT4 included) and uncompressed bit-mask formats, the first mip level |
| `BM` | bitmap | 24 and 32 bit. Some files the client calls `.dds` are really bitmaps. |
| anything else | Targa | uncompressed and run-length true colour, 24 and 32 bit |

A size of 0 or above 16,384 is refused. A texture that is missing or cannot be decoded is shown as a magenta and black checker.

DXT2 and DXT4 (the premultiplied-alpha variants) are read as DXT3 and DXT5 blocks, and the colour is then divided by the alpha (rounded; alpha 0 gives 0,0,0,0), so every reader of the toolkit gets straight-alpha RGBA. `Item/DDS/reflection.dds` of the US EP4.5 client is a DXT2 file and previews. A DDS with any other four-character code is refused; the file checks before files go in report such a file as `texture-unreadable` and say the game may still accept it (see [Checks before files go in](../tools/shaiya.md#checks-before-files-go-in)).

## What is written

Only Add a map writes pictures, to the staging folder. Both follow the files of the EP4.5 client.

### Field chunk textures

A field is cut into 1024 x 1024 unit chunks: `size / 1024` per side (at least 1). For each chunk at row r and column c the client loads, from `world/Field/<id>/`:

| File | Content |
|---|---|
| `<id>_<r><c>_L.dds` | a light map: 1024 x 1024, DXT1, 11 mip levels, 699,192 bytes |
| `<id>_<r><c>_A<k>.dds` | a mask for terrain texture k: 256 x 256, DXT5, 9 mip levels, 87,536 bytes. The alpha channel is where texture k lies, and `A0` is the opaque base. |

The header is the 128 bytes the client's own files use: the magic, 124, flags `0x21007`, the size, the mip count, the pixel format with the four-character code, and caps `0x401008`. A map of 2048 units has 2 x 2 chunks (`00`, `01`, `10`, `11`). The client shows a popup and stops loading when a light map is missing. A missing mask does not stop it.

A blank field gets a light map shaded from its heights and plain masks that follow its texture slots. A copy gets its source's files renamed to the new id, and a generated light map for any chunk whose source has none. A dungeon has no `Field` folder.

### The minimap

`interface/MiniMap/<id>.tga` is a 512 x 512 picture: 24-bit uncompressed Targa, stored bottom-up, with the 26-byte footer (`TRUEVISION-XFILE`) that the client's own minimaps have. The editor shades it from the heights: a hillshade with the light from the north-west, a green to sand tint by height, and blue under -0.4. The top row is the north edge. It is written only when the box is ticked.

## Which texture a model uses

A `.3DC` mesh names no texture (the name is empty in 2,429 of the 2,431 `.3DC` files of `us-108`). The textures of a model come from the list files [`.MLT`](mlt.md) (character parts), [`.MON`](mon.md) (monsters, NPCs, vehicles, strip NPCs) and [`.ITM`](itm.md) (item `.3DO` models); a `.3DE` effect mesh uses the texture of its own name from `Effect/DDS`. The toolkit's texture index (`shaiya_clientdata::texindex`) reads `Character/*/*.MLT`, `Monster/*.MON`, `Npc/*.MON`, `Vehicle/*.mon`, `Strip/*.MON` and `Item/*.ITM` (any letter case) and answers, for one mesh, its textures in record order; the first is the default.

| Step | Rule |
|---|---|
| Own folder | `<folder of the list>/3DC/<name>`, `/3DO/<name>` or `/DDS/<name>`, letter case ignored |
| Elsewhere | else the same bare name in another `3DC`, `3DO` or `DDS` folder (the first by sorted path); the index says so ("found elsewhere") |
| Missing | else the name is reported as missing; a name is never guessed. A mesh no list names has no texture and is drawn grey. |

The alpha channel is described by the `.MLT` flag only: 0 is transparency and 1 is a glow; `.MON` and `.ITM` say nothing about it, and a texture of those lists is "unknown".

### In the Toolkit

The Archive tab draws a `.3DC` and the parts of an `.ANI` with the first texture their list names, a `.3DO` with its `.ITM` texture and a `.3DE` with its own texture from `Effect/DDS`. A texture whose `.MLT` flag is 1 is drawn with its alpha ignored (the list says the alpha is a glow: inferred); every other texture is cut out where its alpha is under half. A mesh no list names, or whose texture is not in the archive, is drawn grey, and a weak line says why. An `.ANI` decodes each distinct texture once. The record in "texture NAME from LIST (record N)" counts from 0. The picture is the toolkit's own drawing, not the game's renderer: the view is orthographic and the texture is read with the nearest texel in animations and with a bilinear blend in a still picture.

On `us-108` (US EP4.5), read from the client files and measured by the opt-in test `the_us_client_texture_index_numbers` (validated offline):

| List | Pairs | Mesh in own folder | Texture in own folder | Mesh elsewhere | Texture elsewhere | Mesh nowhere | Texture nowhere |
|---|---|---|---|---|---|---|---|
| `.MLT` (112 files) | 2,176 | 2,151 | 2,087 | 0 | 0 | 25 | 89 |
| `.MON` (16 files) | 2,005 | 1,753 | 1,745 | 182 | 83 | 70 | 177 |
| `.ITM` (17 files) | 1,540 | 1,540 | 1,540 | 0 | 0 | 0 | 0 |

2,237 meshes have at least one texture pair whose texture exists. Whether the client searches other folders the same way is not traced; the "elsewhere" step is the toolkit's own rule.

## Verified on

US EP4.5. The headers of the generated light map and masks are byte-identical to files of the client, and a map with id 110 made this way loaded on the client. The 2 x 2 chunk layout of a blank 2048 field and the orientation of the minimap have not been checked in the game.

## Other episodes

Other episodes are unverified. The wizard note says the files are made for the EP4.5 client layout. A minimap is written only on request.
