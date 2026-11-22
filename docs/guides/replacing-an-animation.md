# Replacing an animation

For people who have made an animation of their own (in another tool) and want to put it in place of an existing `.ANI` file of a client. The toolkit does not make or edit animations: it finds the file to replace, checks your file against it, puts it in with the usual backup and journal, and lets you look at the result. You need a client folder of your own and a saved name for it (`shaiya clients add`, see [mod files](mod-files.md)); work on a copy while learning, and close the game before any step that changes the client.

Everything the toolkit says about animations is **validated offline**: the checks, the preview and the playback were run on test data and on the files of one client, never judged in the game, apart from the single sentence in [what was seen in the game](#what-was-seen-in-the-game). The format is in [formats/ani.md](../formats/ani.md).

## 1. What can be replaced

An existing `.ANI` file, at the same archive path: the new file takes the old one's place in the archive. The toolkit does not create new animation slots and does not write animation files by itself.

## 2. Find the target

Pick the file by name. Character animations are named `<rig>_<number>_<name>.ANI` in `Character/<race>/ANI`; the rig is the first part of the name (`huwm` is the human male Fighter), and **each rig is its own target**: the skeletons of different rigs differ, and on US EP4.5 almost every file of a rig has reference matrices of its own.

```
shaiya anim info --from "@mymod" "Character/Human/ANI/huwm_*"
```

`info` prints one line per file: the kind and rig of an archive path ("Character rig huwm (Human)"), the bone count, the end key and seconds, the number of keys and the validation findings. In Shaiya Toolkit's Archive tab, select the `.ANI` to see the same facts and play it on a mesh the toolkit finds by name (section 6). A file that does not read to its last byte is shown with the reader's error and its bytes as hex, never half-read.

## 3. The skeleton rule

An `.ANI` file holds, for each bone of a skeleton, a reference matrix and the keys that move it. It has no bone names: a mesh is matched to its bones by **index**. So a replacement has to keep the **skeleton** of the file it replaces:

- the same **bone count**,
- the same **order and parents** of the bones,
- the same **reference matrices**, bit for bit (64 bytes per bone). On US EP4.5 almost every file of a rig has its own, so copy them from the exact file you replace, not from another file of the same rig.

Keys (the motion) may change as you like. Separately, the toolkit also asks for the same first field as the file it replaces (inferred to be the start key; not checked in the game). It is not part of the skeleton rule, and the toolkit reports it on its own line.

## 4. Check your file

```
shaiya anim compare my.ANI "Character/Human/ANI/huwm_117_x.ANI" --from "@mymod"
```

Exit 0 means the file passes: same skeleton, same first field and no validation error. It prints `same skeleton as ...` or each difference (bone count, parents, matrices), the first field on its own line, your file's validation errors, and `passes` or `does not pass` (exit 1). Exit 3 means a file did not read or the target is not in the archive. `shaiya anim validate my.ANI` checks the file alone (exit 1 when it has an error). With `--json` the result is machine readable ([shaiya anim](../tools/shaiya.md#shaiya-anim)).

Passing is a result of the toolkit's own checks. It does not say that the game accepts the file.

## 5. Put it in

**In Shaiya Toolkit.** In the Archive tab, right-click the file, choose **Replace with file...** and pick yours. The toolkit compares it with the copy already in the client first; if the skeleton differs it asks (**Add anyway** or **Cancel**) before queueing anything, and lists the finding (`anim-rig-differs`). A different first field, a file that does not read, and validation errors only warn. Nothing is written until you press **Apply to client...** in the Archive tab's pending panel; it asks first and writes a journal.

**On the command line.**

```
shaiya archive replace "@mymod" my.ANI --as "Character/Human/ANI/huwm_117_x.ANI" --in-place
```

The same checks run, print a warning for each finding and never change the exit code. `--in-place` writes a journal; without it you must name `-o` for a new pair of files. See [mod files](mod-files.md).

A patch build (`shaiya patch build`, the Patch builder) has no copy of the client to compare with, so it checks that the file reads and has no validation error, but cannot run the skeleton check. Run `anim compare` yourself before building.

## 6. Look at it

Select the file in the Archive tab, in a client that holds your replacement. The player shows the animation posed on a mesh, with **Play/Pause**, a slider over the keys, **Loop**, **Bones** (the skeleton drawn as lines) and a **Mesh** choice: the default parts, a single mesh, or the skeleton alone. Time runs at 30 key units per second. The meshes (`.3DC` files) are found by name in the sibling `3DC` folder and are offered only when every bone they use exists in the animation; without one the skeleton is drawn. The picture is an approximation of the game's playback, validated offline.

![The animation player on the synthetic demo arm, paused at key 30](../assets/screenshots/toolkit-anim-preview.png)

## 7. Undo

Every change you apply is journaled, except with `--no-journal` or Compact, which make none. In Shaiya Toolkit use **Undo last change** or **Apply & history > Roll back...**; on the command line:

```
shaiya patch rollback "@mymod"
```

puts `data.sah` and `data.saf` back byte for byte. See [undo and recovery](undo-and-recovery.md) for what can block a rollback.

## 8. Share it

Queue the replacement in the Archive tab and press **Make patch...**, or use `shaiya patch build`, then sign and publish as in [make a patch](make-a-patch.md). Players apply the patch the usual way and can roll it back.

## What was seen in the game

Seen in the game (US EP4.5): basic playback of one replaced victory emote on the `huwm` rig (the human male Fighter); everything else is validated offline only.

## Other episodes

The toolkit treats every client the same: it lists, extracts and replaces animation files on any client. It previews and checks a file when, and only when, the file reads to the last byte. The contents of the animation files were read only on US EP4.5 (patch 108): all 3,509 `.ANI` files read to the last byte. The eight other test clients have their index only (files counted, none read), so whether their files read is not checked; if one does not, the toolkit says so and shows the bytes.
