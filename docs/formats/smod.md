# Models (`.smod`)

`.smod` files are the static models that a `.wld` ([World file](wld.md)) places as buildings (`Entity/Building`), shapes (`Entity/Shape`) and trees (`Entity/Tree`).

All numbers are little-endian. A string is `i32 length` and that many bytes.

## Layout as read

```
f32 centre[3]
i32 unknown                      0 in the files checked
f32 min[3], max[3]               bounding box
i32 nModules
  Module:  string texture,       a .tga name; the cleaned archive stores .dds
           i32 nVert, Vert[nVert],   Vert = f32 pos[3], f32 normal[3], u32 colour, f32 u, v   (36 bytes)
           i32 nFace, Face[nFace]    Face = u16 x 3                                            (6 bytes)
f32 box2 min[3], max[3]          inside min and max, not always equal
i32 nExtra                       a flag in practice: when not 0, one collision mesh follows
  Extra:   i32 nPoint, f32[3] x nPoint, i32 nFace, Face[nFace]     untextured, likely collision
```

The colour of every vertex is `0xFFFFFFFF` in the files seen. The first 16 bytes look like a bounding sphere, but the radius is 0 in the files and the client works out its own. The file must end at the last byte.

## Orientation of a placed model

A placement's `dir` is the model's forward axis (+z) and `up` its +y axis. The model's +x is `up x dir`, which gives a left-handed frame. A map yaw of a gives `dir = (sin a, 0, cos a)`. Both vectors are unit length and perpendicular in every EP4 map, and there is no scale.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `Smod` | parses it; `placement_basis` builds the rotation of a placement |
| Shaiya SVMap Editor | the box used to pick a placement (turned with it), the 3D view, and the Models window, which draws a textured picture of a model on the CPU. A model the client cannot read gets a small stand-in box. |

The file is only read.

## Verified on

US EP4.5: 1,355 of 1,355 models parse to the last byte in the original check, and every textured module of the 1,219 buildings, shapes and trees in the client finds and decodes its texture.

## Other episodes

Other episodes are unverified. A model that does not read is drawn as a box and the editor never writes a model.
