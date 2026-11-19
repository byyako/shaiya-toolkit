# Version.ini and UpdateVersion.ini

Two small text files decide what the original updater downloads: `Version.ini` in the client folder says where a player is, and `UpdateVersion.ini` on the patch server says where the server is. `shaiya patch publish` and Shaiya Toolkit's Publish section write the second one; Shaiya Toolkit reads the first.

**Verified on:** the original US EP4.5 updater. **Other episodes:** not verified. Both files are plain text with a `[Version]` section, so a replacement updater or another episode may read different keys; `--layout flat` and the key list below are plain enough to adapt by hand.

## Version.ini (client folder)

| Key | Meaning |
|---|---|
| `CheckVersion` | The client generation. The server's `CheckVersion` must be the same number; when it differs, the original updater tells the player to reinstall the game. |
| `CurrentVersion` | The number of the last patch the client has applied. |
| `StartUpdate` | `UPDATE_START` while the original updater is patching, `UPDATE_END` after it has finished. A file that still says `UPDATE_START` is the trace of an update that did not finish. |

The toolkit reads these three keys (any letter case, with or without a byte-order mark, ASCII or code page 949 text) and never writes this file. `shaiya clients show` and File > Manage client folders print `CheckVersion` and `CurrentVersion`; the Patch builder suggests `CurrentVersion + 1` as the next patch number.

## UpdateVersion.ini (patch server)

| Key | Meaning |
|---|---|
| `CheckVersion` | Compared with the client's `CheckVersion`; a different number makes the original updater tell players to reinstall. |
| `UpdaterVersion` | The version of the updater the server offers. |
| `PatchFileVersion` | The number of the newest patch on the server. A client at `CurrentVersion` fetches every patch from `CurrentVersion + 1` up to `PatchFileVersion`, in order. |

The original updater reads the file at `<host>/Shaiya/UpdateVersion.ini` and fetches the patches from `<host>/Shaiya/patch/psNNNN.patch`. It only fetches names of the form `psNNNN.patch` and it does not honour `remove.txt` inside a patch ([patch format](patch.md#removals)). It does not check signatures.

A wrong `UpdateVersion.ini` stops every player's update: a `PatchFileVersion` that is too high makes players ask for a patch that is not there, one that is too low hides patches, and a different `CheckVersion` sends everybody to a reinstall.

## What `shaiya patch publish` writes

```
DIR/Shaiya/UpdateVersion.ini
DIR/Shaiya/patch/ps0110.patch
```

(`--layout flat` puts the ini and the patches in `DIR` itself.) A new file is ASCII with CRLF line endings:

```
[Version]
CheckVersion=3
UpdaterVersion=1
PatchFileVersion=110
```

- An existing file is edited line by line: only the value of a key that changes is replaced, a missing key is added after the last line of `[Version]` (with the line ending of that line, CRLF when it has none), and every other line (comments, other sections, other keys, spacing) stays byte for byte. That includes bytes that are not ASCII, such as code page 949 text in a comment: they are neither checked nor converted. Only a new file is pure ASCII.
- A new file needs `--check-version`; `UpdaterVersion` is 1 unless `--updater-version` is given. Set it to the version of the updater your server offers: publishing never writes an updater file.
- Patches are copied to a temporary name in the folder, compared with the original by SHA-256, renamed into place and read back; the ini is written last, so it never points at a patch that is not there yet.
- It refuses (exit 4) a file name that is not `psNNNN.patch` with four digits, a patch that fails the structure checks of `patch verify`, a gap between the existing `PatchFileVersion` and the new numbers, a different file under a name that is already in the folder (a published patch is never replaced), a `CheckVersion` different from the existing one unless `--new-check-version` is given, an existing ini it cannot read, and a destination that is a client folder or inside one, or that has a client folder below it where the files would go (with the default layout, `DIR/Shaiya` or `DIR/Shaiya/patch` holding `data.sah` and `data.saf`, or being a saved client). Everything above is decided before anything is written, so these refusals write nothing.
- A change found while writing stops the publish too (exit 4): a patch file that appeared in the folder since the plan, a source patch that changed, or the ini changed by someone else since the plan (it is looked at again right before it is replaced, after the copies). This can come after some patches were copied; the message then lists the files already copied and says that `UpdateVersion.ini` was not changed. Those files stay (the ini does not name them yet, so no player asks for them), and publishing again finds them as already published. A copied file is never replaced; where the file system allows hard links the refusal is atomic, otherwise a file that appears in the few microseconds between the last check and the rename could be overwritten.
- It warns about a patch that carries removals (the original updater ignores them) and about the [file checks](../tools/shaiya.md#checks-before-files-go-in) of the patches' files; those warnings do not stop it.
- It copies a `psNNNN.patch.sig` found beside a patch along with it.
- `--dry-run` prints the plan and writes nothing.

See [Make a patch](../guides/make-a-patch.md#6-publish).
