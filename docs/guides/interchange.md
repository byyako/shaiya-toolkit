# Taking models and animations to other programs (glTF)

For people who want to look at a Shaiya model or animation in a program that opens glTF 2.0 files (Blender and other viewers), or to start work from a game model there. The toolkit writes glTF, and reads a glTF back into a new game file (section 5). Everything below is **validated offline** on test files and on the files of one client; the game was not involved.

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

## 5. Bringing a glTF back in

`shaiya anim import IN.gltf -o OUT.ani` (or `IN.glb`, and `-o` ending in `.3dc`, `.3do` or `.3de`) and the Toolkit's **Import glTF...** read a glTF 2.0 file and build a **new** game file. They never change an existing file, and a file made by another program (Blender, for example) works when its bones are named as below. Everything here is **validated offline**: how the game plays an imported file was not tested.

```
shaiya anim import walk.glb -o walk.ani --like "Character/Human/ANI/huwm_000_normal.ANI" --from "@us"
shaiya anim import walk.gltf -o walk.ani --like old.ani --keys exact
shaiya anim import torso.glb -o torso.3dc --like "Character/Human/3DC/huwm_torso001.3DC" --from "@us" --reduce-influences
shaiya anim import crate.glb -o crate.3do
```

- **What is made, and the like file.** The extension of `-o` says which game file is made. An `.ANI` and a `.3DC` need `--like`, the file the new one will replace (for a `.3DC`, the `.ANI` of its rig will do): without it the command stops with exit 2. A `.3DO` or `.3DE` takes no like file (exit 2 if one is given). An `.ANI` takes its skeleton (bone count, parents, reference matrices) and its first field from the like file, never from the glTF, and is written only when it passes the same check as `shaiya anim import` of a JSON (same skeleton, same first field, no validation error); otherwise exit 1 and nothing is written. A `.3DC` takes its matrices from the glTF's skin; it is written only when its bone count is the like file's and no vertex names a bone the rig does not have.
- **Which node is which bone.** A node named `Bone_NNN` is bone NNN, with any number of digits (`Bone_5` and `Bone_005` are the same bone). A node with another name needs `--map FILE` (lines of `node name = bone index`; blank lines and lines starting with `#` are skipped). A bone of the like file without a node, a node named for a bone the like file does not have, two nodes for one bone, and a bone whose parent differs from the like file's are refused, with the list.
- **Animation.** The file must hold exactly one animation. It is sampled at every whole key (30 per second), from its first time to its last, by glTF's own rules for `LINEAR`, `STEP` and `CUBICSPLINE`; times that are not multiples of 1/30 second (Blender at 24 frames per second, for example) are sampled at the whole keys and the command says so. A bone gets a rotation or translation track only when it moves, or when its value differs from the like file's own; a bone that stays where the like file has it gets none, so an imported file can have fewer tracks than the file it came from. `--keys exact` instead keeps the file's own keys (each time must be a multiple of 1/30 second, to within a microsecond and the precision of a 32-bit time, and the channel must be `LINEAR`). `--reduce` drops the keys that the keys around them reproduce within 1e-5. The result is **the same pose, not the same bytes**: a quaternion may come back with its sign changed, and keys may differ.
- **Skin (`.3DC`).** A game vertex holds two bones and one weight. A vertex with more than two influences is refused (the reason names the first vertex and how many there are); `--reduce-influences` keeps the two largest, renormalises them and says how many vertices changed and the largest weight dropped. A vertex that has one bone takes the like file's second bone and weight when the like file's vertex of the same number poses the same, which is what lets a `.3DC` the toolkit exported come back **byte for byte** with that file as the like file. The texture name comes from the material's `extras` (`texture_hex`, as the toolkit writes it) or else the material's name; a picture in the glTF is never read.
- **Static meshes (`.3DO`, `.3DE`).** The file must hold one mesh and no skin. The mesh node's own placement is applied to its vertices (and says so); a placement that mirrors the mesh is refused. A `.3DO` needs normals: when the file has none, they are computed from the faces and the command says so. No frames are written.
- **What is refused (exit 3, with the reason naming the node, bone or vertex).** A scale on a bone that the game would drop (the game replaces a bone's 3 x 3 by a rotation; a scale equal to the like file's own is kept), morph targets and morph-weight animation, animation of a node that is not a bone, more than one animation, a node with two parents or a cycle of nodes, a skinned mesh whose node has a placement, more than two meshes or more than one primitive per mesh, a mode other than triangles, more than 65,535 vertices, more than 256 bones, NaN or infinity anywhere, weights that do not add up to 1, sparse accessors, normalized or quantized (non-float) attributes, a required extension (compression, quantization or any other), a buffer that is a URL or lies outside the folder of the `.gltf` (only a `data:` uri or a file next to it is read; a link that leads out of the folder is refused too), a texture name over 255 bytes, a like file that is a cloak, and a file that is truncated, malformed or asks for more than the file holds.
- **Where it stops, and what it writes.** The output rules are those of the JSON import: a new file (an existing one only with `--force`), never inside a client folder, never one of the inputs, written beside and read back before it is renamed. Exit 0 written, 1 an `.ANI` that does not pass (nothing written), 2 a bad argument, 3 a refused or unreadable file, 4 the output rules, 5 a bug.

In Shaiya Toolkit's Archive tab, right-click an `.ANI`, `.3DC`, `.3DO` or `.3DE` file and choose **Import glTF...** (also in the command palette). The row you clicked is the like file (an `.ANI` or `.3DC`) or names the kind (a `.3DO` or `.3DE`). Choose the glTF file (and a map file if you need one), tick the options, press **Import**: a background task reads and checks it, and the window shows what it made, or every reason it refused. **Save as new file...** writes it to a new file outside every client folder (the name must end in the kind's extension, nothing that exists is replaced; the file is read back and checked again before it is renamed); put it into a client with Replace.

## 6. What was checked

Checked: the Khronos glTF validator 2.0.0-dev.3.10, run offline, reports no errors on the toolkit's output for its synthetic test files and for one US EP4.5 rig; not checked: Blender or any other program opening the files.

Before it writes, the toolkit also checks every file it makes (structure, ranges, finite numbers, weights, quaternions) and poses the exported skin the way glTF defines it at ten times, comparing it with its own renderer; a failure is exit 5 and nothing is written.

The import: a `.3DC` the toolkit exported comes back byte for byte (test `a_3dc_round_trips_through_gltf_to_the_same_bytes`), an `.ANI` comes back with the same poses (`an_ani_round_trips_with_keys_exact_to_the_same_poses`), and every refusal above has a test that names its reason (`each_refusal_names_its_reason`, `sampler_rules_step_cubic_and_24_fps_resampling`, `malformed_files_are_errors_not_panics`). On US EP4.5 the opt-in tests measured: the first animation of each of the 16 character rigs and their 96 default parts come back (each `.ANI` passes against its source with `--keys exact` and with every whole key, with the largest pose difference 1.1e-6; each of the 96 `.3DC` files byte for byte), and all 3,462 `.ANI` files that the export writes come back with `--keys exact` and pose alike within a relative 1.2e-5 (`us_client_gltf_round_trips`, `every_exportable_us_ani_imports_back_with_keys_exact`). Blender's own files were not tried.

The tests that stand behind these sentences are in `crates/shaiya-clientdata/tests/gltf_export.rs`, `crates/shaiya-clientdata/tests/gltf_import.rs`, `crates/shaiya-clientdata/tests/gltf_optin.rs`, `crates/shaiya-cli/tests/gltf.rs` and `crates/shaiya-toolkit/tests/toolkit.rs`.
