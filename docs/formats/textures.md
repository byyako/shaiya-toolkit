# Textures (DDS and TGA)

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

## Verified on

US EP4.5. The headers of the generated light map and masks are byte-identical to files of the client, and a map with id 110 made this way loaded on the client. The 2 x 2 chunk layout of a blank 2048 field and the orientation of the minimap have not been checked in the game.

## Other episodes

Other episodes are unverified. The wizard note says the files are made for the EP4.5 client layout. A minimap is written only on request.
