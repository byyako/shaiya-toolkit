# SData files

SData files (`Item/Item.SData`, `Monster/Monster.SData`, ...) hold the game's tables. In most clients they are wrapped in a SEED encryption with a checksum; some clients (the Chinese client in the test set) store them plain. The toolkit can describe, check, decrypt and encrypt them. It does not edit the tables: use an editor of your choice on the decrypted file. (An SData editor is planned.) The wrapper format is in [formats/sdata-wrapper.md](../formats/sdata-wrapper.md).

All commands are in [`shaiya sdata`](../tools/shaiya.md#shaiya-sdata); the Workbench SData tab does the same into a folder you choose.

## Look at them

```
shaiya sdata info Item.SData
shaiya sdata info --from @mymod
shaiya sdata check --from @mymod
```

`info` says for each file: `SEED`, the plaintext size, whether the CRC matches and whether the header padding and tail are zero, or `not encrypted`. `check` is the same but exits 1 when a CRC fails. `--from ARCHIVE [PATTERN]...` reads straight out of a client's archive (default pattern `*.sdata`); folders given as inputs are searched for `*.SData`.

## Decrypt, edit, encrypt

```
shaiya sdata decrypt --from @mymod "Item/Item.SData" -o C:\work\plain\
(edit C:\work\plain\Item\Item.SData with your editor)
shaiya sdata encrypt C:\work\plain -o C:\work\enc\ --like @mymod
```

- `decrypt` writes the plaintext under the same relative name. If the client stores the file plain, it is copied unchanged (`plain, copied`) unless you pass `--skip-plain`.
- `encrypt --like` is the important part. The wrapper around the data has a header padding and a tail that the plaintext does not carry; `--like` takes them from the original (a file, a folder of originals, or an archive such as `@mymod`) so that **an unchanged file encrypts back to the identical bytes**, and a file that was plain stays plain. Without `--like` the header padding and tail are written as zeros, so even an unchanged file can differ from the original in those bytes. Use `--like`.
- A file that fails its CRC on decrypt is reported and skipped (exit 3) so you do not edit corrupted data; `--unchecked` writes it anyway and exits 1.
- End a folder given to `-o` with a backslash (`C:\work\plain\`). When there is exactly one file to write and the `-o` path is not an existing folder, the toolkit treats it as the output *file* name.
- Outputs never overwrite existing files unless you say `--force` (or leave them with `--skip-existing`). Use `--dry-run` to see what would happen.

Then put the encrypted file into the client with a patch or an edit (the paths in `C:\work\enc` mirror the archive):

```
shaiya archive replace @mymod C:\work\enc --in-place
```

or `shaiya patch build -o ps0001.patch --files C:\work\enc`. Both keep a journal you can roll back. See [mod files](mod-files.md).

## Unencrypted clients

Some clients store SData plain. `info` says `not encrypted`, `decrypt` copies, and `encrypt --like` keeps plain originals plain, so the same commands work and you can ignore the difference.

## Tips

- Decrypt only the files you need (`"Item/Item.SData"`), not the whole client, unless you are exploring.
- Re-encrypt the whole edited folder with one `--like @name`; each file finds its own original by path.
- `sdata ... --in-place` replaces a standalone file after copying the original to `NAME.bak`. Do not use it on files that belong to a client archive; extract with `--from`, then put the result back with `archive replace` or a patch.
