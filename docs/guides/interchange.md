# Taking models and animations to other programs (glTF)

For people who want to look at a Shaiya model or animation in a program that opens glTF 2.0 files (Blender and other viewers), or to start work from a game model there. The toolkit writes glTF; it does not read it back yet. Everything below is **validated offline** on test files and on the files of one client; the game was not involved.

**glTF, OBJ and BVH are a lossy view of the game file: the game file's own round trip (JSON) stays byte for byte; an import builds a new file and runs the same checks as `shaiya anim import` before it writes.** Use [the JSON export](replacing-an-animation.md) when you need the file's exact values; use glTF when you want to look at it elsewhere.

## 1. What goes out

| Game file | glTF holds |
|---|---|
| `.3DC` skinned mesh | the mesh, its skin (the bones and the file's own matrices), its material; with `--anim` one animation per `.ANI` you name |
| `.3DC` cloak (no bones) | a static mesh without a skin; an animation cannot be attached |
| `.ANI` animation | the skeleton at its reference pose and the animation: no mesh |
| `.3DO`, `.3DE` | the mesh at rest (the vertex frames are not written; a note says how many) |

A `.gltf` file comes with a `.bin` beside it that holds the numbers; a `.glb` is one file that holds both. Nothing is written into a client folder, and nothing that exists is replaced without `--force` (the same rules as the JSON export).

## 2. What the mapping does, in words

- **Axes.** The game's space is left-handed; glTF's is right-handed. The toolkit mirrors the front-to-back axis once (`z` becomes `-z`) in positions, normals, rotations and bone matrices, and swaps two corners of every triangle so the outside stays outside. Nothing else is mirrored.
- **Time.** Keys are 1/30 second. The first time in the file is the smaller of the animation's first field and its smallest key (some files start at -1 or -4, and glTF has no negative times); the file records it, and the animation's end key too. glTF files carry no frame rate: if a program asks, set its scene to 30 frames per second (a hint, not checked here).
- **Bones.** One node per bone in file order, named `Bone_000`, `Bone_001`, ... (the game's bones have no names; the number is the link back). A skin needs one common root: when the bones have none (a mesh without an animation, or a skeleton with several roots) the toolkit adds a node `Shaiya_Root` after the bones.
- **Skin.** The game gives each vertex two bones and one weight; glTF gets two joints and two weights. A vertex whose two bones are the same bone, or whose weight is exactly 0 or 1, lists one joint. The 16-bit `pad` field of each vertex goes into a custom attribute `_SHAIYA_PAD` so nothing of the vertex is lost.
- **Rest pose.** For a mesh the bones rest where the mesh's own matrices put them, so the mesh opens undeformed; the animation then carries the complete motion of every bone. A bone that no vertex uses may keep an identity rest when its matrix is not a plain placement.
- **Quaternions** are made continuous (the game blends the short way round; other programs blend component by component and would spin the long way otherwise).
- **Textures are names only.** A material is named with the texture name of the file and keeps the raw bytes as hex in `extras`; no picture is written or embedded, and there is no `images` list unless you give `--texture-uri-base DIR`, which writes only `DIR/<name>.png` as a reference (a file that does not exist there makes viewers complain; convert the game's pictures yourself).
- **Names and paths.** A file holds the archive path or the file name of its input and the toolkit's version, never a folder of your machine.

## 3. What cannot go out

The toolkit refuses a file rather than writing something that plays differently, and names the bone or vertex:

- a vertex, normal, texture coordinate or matrix with NaN or an infinity (the reason says `vertex 287 holds NaN or an infinity in its normal x`),
- a bone whose placement is sheared (its rows not at right angles) or mirrored (negative determinant),
- keys that do not increase,
- a weight outside 0 to 1, or a vertex that names a bone the skeleton does not have,
- animations that do not share one skeleton, or an animation attached to a cloak,
- a mesh without vertices or faces, or a face that names a vertex that does not exist.

`--allow-lossy` projects a sheared, mirrored or NaN placement onto the nearest placement, and drops the keys that do not increase; the output says how many items it changed. A bone no vertex uses, whose matrix is not a plain placement, is not a refusal.

On US EP4.5 the opt-in test `every_us_client_ani_and_3dc_exports_or_is_refused_by_kind` writes 5,864 of the 5,940 `.ANI` and `.3DC` files and refuses the other 76, each with a reason by kind (see [the format pages](../formats/README.md) for the files themselves).

## 4. How to do it

```
shaiya anim export "Character/Human/3DC/huwm_torso001.3DC" --from "@us" --format glb --anim "Character/Human/ANI/huwm_000_normal.ANI" -o torso.glb
shaiya anim export "Character/Human/ANI/huwm_000_normal.ANI" --from "@us" --format gltf -o walk.gltf
shaiya anim export "Monster/ANI/" --from "@us" --format gltf --out-dir gltf-out
```

`--format gltf` writes `walk.gltf` and `walk.bin`; `--format glb` writes one file. Exit 0 written, 2 a bad argument (an output name that does not fit the format), 3 the file cannot be written as glTF (the reason is printed), 4 the output exists, is inside a client folder or is the input, 5 a bug. With `--out-dir` every file is written as `<archive path>.gltf` and `.bin` (or `.glb`) under the folder.

In Shaiya Toolkit's Archive tab, right-click an `.ANI`, `.3DC`, `.3DO` or `.3DE` file and choose **Export as glTF...** (also in the command palette). Pick a `.gltf` or `.glb` name outside every client folder. An `.ANI` goes out together with the one mesh the Mesh box shows when you have chosen one; with the default parts or the skeleton chosen it goes out alone. The file is written by a background task and checked first.

## 5. What was checked

Checked: the Khronos glTF validator 2.0.0-dev.3.10, run offline, reports no errors on the toolkit's output for its synthetic test files and for one US EP4.5 rig; not checked: Blender or any other program opening the files.

Before it writes, the toolkit also checks every file it makes (structure, ranges, finite numbers, weights, quaternions) and poses the exported skin the way glTF defines it at ten times, comparing it with its own renderer; a failure is exit 5 and nothing is written.

The tests that stand behind these sentences are in `crates/shaiya-clientdata/tests/gltf_export.rs`, `crates/shaiya-clientdata/tests/gltf_optin.rs` and `crates/shaiya-cli/tests/gltf.rs`.
