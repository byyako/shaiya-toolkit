# Dungeon floors (`.dg`)

A dungeon map's `.wld` ([World file](wld.md)) names a `.dg` file in `world/dungeon/`. The `.dg` holds the dungeon's geometry as an octree of textured, lightmapped meshes with a collision mesh per node. Its octree replaces the terrain of a field.

All numbers are little-endian. `box` is `f32` min x, y, z and max x, y, z (24 bytes). A name is a 256-byte buffer, NUL-terminated.

## Layout as read

```
box bounds
i32 nTex; name texture[nTex]       texture names, files in Entity/Texture
i32 nLightmaps                     the lightmaps are <dg name>_L0.dds ... next to the .dg
i32 hasRoot; Node                  (when hasRoot is not 0)

Node:
  f32 centre[3]; box; box
  i32 nGroups; nGroups x { i32 textureIndex; i32 nMesh; Mesh[nMesh] }
     Mesh   = i32 lightmapIndex; i32 nVert; Vert[nVert]; i32 nFace; Face[nFace]
     Vert   = f32 pos[3], f32 normal[3], u32 colour, f32 uv[2], f32 lightmapUv[2]    (44 bytes)
     Face   = u16 x 3                                                                (6 bytes)
  i32 hasCollision; when set: i32 nVert; f32 pos[3] x nVert; i32 nFace; Face[nFace]
  8 x { i32 hasChild; Node (when hasChild is not 0) }
```

The reader limits the octree depth to 40 levels, and the file must end at the last byte.

## Map size rule

The server size of a dungeon map is the larger of the `.dg` bounds in x and z, rounded up to a multiple of 8. This held for 46 of the 50 dungeon maps that have a `.svmap`. Maps 3, 27, 48 and 49 use 1024 instead.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `Dg` | parses it, flattens it into draw batches and a collision mesh, and gives the suggested map size |
| Shaiya SVMap Editor | the dungeon floor in 2D, the 3D view, ground height queries, and the generator for blocked cells (from the collision mesh). The rule `dg-missing` reports a dungeon whose `.dg` is not in `world/dungeon`. |

The file is only read.

## Verified on

US EP4.5: the 44 `.dg` files of the tested client's archive parse to the last byte. A second, older copy of the archive holds 51 files, and all of them parse.

## Other episodes

Other episodes are unverified. A file that does not parse shows no floor, and the editor says why.
