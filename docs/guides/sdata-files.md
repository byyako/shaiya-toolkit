# SData files

SData files (`Item/Item.SData`, `Monster/Monster.SData`, ...) hold the game's tables. In most clients they are wrapped in a SEED encryption with a checksum; some clients (the Chinese client in the test set) store them plain. The toolkit can describe, check, decrypt and encrypt them, and show the Item, Skill, Monster and NpcQuest tables as rows and columns (read only). It does not edit the tables: use an editor of your choice on the decrypted file. (An SData editor is planned.) The wrapper format is in [formats/sdata-wrapper.md](../formats/sdata-wrapper.md).

All commands are in [`shaiya sdata`](../tools/shaiya.md#shaiya-sdata); Shaiya Toolkit's SData tab does the same into a folder you choose.

## Look at them

```
shaiya sdata info Item.SData
shaiya sdata info --from "@mymod"
shaiya sdata check --from "@mymod"
```

`info` says for each file: `SEED`, the plaintext size, whether the CRC matches and whether the header padding and tail are zero, or `not encrypted`. `check` is the same but exits 1 when a CRC fails. `--from ARCHIVE [PATTERN]...` reads straight out of a client's archive (default pattern `*.sdata`); folders given as inputs are searched for `*.SData`.

## Look at a table

```
shaiya sdata table Item.SData --csv items.csv
shaiya sdata table --from "@mymod" Item/Item.SData --json
shaiya sdata table NpcQuest.SData --section merchants --csv npcs.csv
```

`table` reads an SData file (encrypted or plain) as rows and columns and writes them as CSV (UTF-8 with a byte-order mark, so a spreadsheet opens it with the right characters) or, with `--json`, as one JSON document. It works only when a layout reads the whole file to its last byte: the layouts are one per format variant (`Item` with 81, 132 or 134 bytes per record, `Skill` with 3, 9 or 15 levels per entry, `Monster` with a 31 or 36 byte block, and `NpcQuest` with three quest record shapes), and `--layout auto` (the default) uses the one that fits. When none fits, or more than one does, the command exits 3 and says `No table layout fits this file`: the file may be another table, or another episode's version of one of these. Names and descriptions are decoded as code page 949 unless `--code-page` says otherwise (cp1252 and cp1251 for the French and Russian clients). A column that has no known meaning is named `f<offset>`. A layout that was read from a client's own loader is **loader-verified** on the clients named in the output; a layout that only reads the file to its last byte is **FIT-ONLY** (the command says so on stderr and the JSON has `"status": "fit-only"`): its columns are named by position (`f<offset>` for fixed fields, `s0`, `s1`, ... for texts, `l0`, `l1`, ... for lists; only the row numbers such as `bank` and `id` keep a plain name) and its values may be split wrongly. Treat a FIT-ONLY table as a way to look at the data, not as a description of the game's own table. `NpcQuest.SData` holds five tables (`merchants`, `gatekeepers`, `npcs`, `quest_links`, `quests`); the CSV is `quests` unless `--section` names another. A text cell that starts with `=`, `+`, `-` or `@` gets a leading `'` in the CSV so a spreadsheet does not run it as a formula; numbers are written as they are.

In Shaiya Toolkit's SData tab, select a file in the list and press **View table** (or double-click it): a window shows the rows with a filter box, the row count, **Copy rows** and **Export CSV...**, and a code page choice for the names. The Archive tab's preview of an SData file has the same **View table** button. For a file no layout fits, the window says "No table layout fits this file (N bytes); shown as hex" and shows the bytes. Under the toolbar the window says where the layout comes from: "Loader-verified on: <clients>." or, for a FIT-ONLY layout, "FIT-ONLY: this layout reads the whole file, but it was not read from any client's own loader. Columns are named by position; values may be split wrongly." The formats pages say what each layout is and which clients it was checked on: [Item.SData](../formats/item-sdata.md), [Skill.SData](../formats/skill-sdata.md), [Monster.SData](../formats/monster.md), [NpcQuest.SData](../formats/npcquest.md).

## Decrypt, edit, encrypt

```
shaiya sdata decrypt --from "@mymod" "Item/Item.SData" -o C:\work\plain\
(edit C:\work\plain\Item\Item.SData with your editor)
shaiya sdata encrypt C:\work\plain -o C:\work\enc\ --like "@mymod"
```

- `decrypt` writes the plaintext under the same relative name. If the client stores the file plain, it is copied unchanged (`plain, copied`) unless you pass `--skip-plain`.
- `encrypt --like` is the important part. The wrapper around the data has a header padding and a tail that the plaintext does not carry; `--like` takes them from the original (a file, a folder of originals, or an archive such as `@mymod`) so that **an unchanged file encrypts back to the identical bytes**, and a file that was plain stays plain. Without `--like` the header padding and tail are written as zeros, so even an unchanged file can differ from the original in those bytes. Use `--like`.
- A file that fails its CRC on decrypt is reported and skipped (exit 3) so you do not edit corrupted data; `--unchecked` writes it anyway and exits 1.
- End a folder given to `-o` with a backslash (`C:\work\plain\`). When there is exactly one file to write and the `-o` path is not an existing folder, the toolkit treats it as the output *file* name.
- Outputs never overwrite existing files unless you say `--force` (or leave them with `--skip-existing`). Use `--dry-run` to see what would happen.

Then put the encrypted file into the client with a patch or an edit (the paths in `C:\work\enc` mirror the archive):

```
shaiya archive replace "@mymod" C:\work\enc --in-place
```

or `shaiya patch build -o ps0001.patch --files C:\work\enc`. `archive replace --in-place` and `patch apply` keep a journal you can roll back; `patch build` only writes the patch file, so there is nothing to undo until you apply it. See [mod files](mod-files.md).

## Checks before files go in

Before an edit or a build, the toolkit looks at each file and prints what it finds as a warning; nothing is blocked. Shaiya Toolkit lists the same findings in the Add dialog and the Patch builder (and asks once for a file that is SEED-encrypted twice or whose checksum fails: **Add anyway** in the Add dialog, **Build anyway** in the Patch builder, **Publish anyway** in Publish). The full table is in [Checks before files go in](../tools/shaiya.md#checks-before-files-go-in).

- `sdata-double-encrypted`: the SData file was encrypted twice (its decrypted body is SEED-encrypted again). Decrypting it once with `shaiya sdata decrypt` gives the singly encrypted file.
- `sdata-crc-bad`: the SEED checksum fails or the wrapper is damaged.
- `sdata-state-differs`: you are putting a plain SData where the client's copy is encrypted, or the reverse. Some clients store theirs plain, so this is advice only; `sdata encrypt --like "@name"` takes the form from the client (see "Decrypt, edit, encrypt" above).
- `texture-unreadable`: a `.dds` or `.tga` the toolkit's reader cannot open. The game may still read it; open it in an image editor to be sure.
- `empty-file`: the file has no bytes.

## Unencrypted clients

Some clients store SData plain. `info` says `not encrypted`, `decrypt` copies, and `encrypt --like` keeps plain originals plain, so the same commands work and you can ignore the difference.

## Tips

- Decrypt only the files you need (`"Item/Item.SData"`), not the whole client, unless you are exploring.
- Re-encrypt the whole edited folder with one `--like "@name"`; each file finds its own original by path.
- `sdata ... --in-place` replaces a standalone file after copying the original to `NAME.bak`. Do not use it on files that belong to a client archive; extract with `--from`, then put the result back with `archive replace` or a patch.
