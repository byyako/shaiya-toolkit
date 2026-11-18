# The archive: data.sah and data.saf

A Shaiya client keeps most of its files in one archive made of two files: `data.sah`, the **index**, and `data.saf`, a flat blob holding the file contents end to end. The `shaiya-archive` crate reads and writes both. All numbers are little-endian.

## Index layout

```
header     char[3] "SAH"; i32 version; u32 file_count; byte[40] zero
directory  str name; u32 n_files; file[n_files]; u32 n_dirs; directory[n_dirs]      (the root's name is empty)
file       str name; i64 saf_offset; u32 length; i32 version                          (version: Unix time the file was packed)
str        i32 length including the NUL; bytes; NUL
free list  u32 count; free_entry[count]                                              (after the root directory; optional)
free_entry str name; i64 saf_offset; u32 length; i32 version                          (same shape as a file entry)
trailer    the remaining bytes (4 bytes in the clients seen)
```

- `version` is **0** in most clients and **1** in at least one (Fawkes v68, same layout as 0). A higher version is reported as unsupported by `shaiya info`.
- `file_count` must equal the number of file entries in the tree; a mismatch is an error.
- The offsets are into `data.saf` and are 64-bit, so a `data.saf` may be larger than 4 GiB (the Fawkes v68 archive ends at 6,645,531,195 bytes); each file's length is 32-bit. A reader checks every entry against the size of `data.saf` before reading, and the toolkit does all offset arithmetic in 64 bits.
- The parser is bounds-checked: counts are checked against the bytes that remain, the tree depth is limited to 64, names to 4096 bytes. A damaged index gives an error naming the byte offset, never a panic.
- The free list and the trailer are kept as they are (`Index::free`, `Index::trailer`), so writing an unchanged index back is **byte-identical**.

## Names

- Names are stored in **code page 949** (Korean) or ASCII; Korean names occur in every region's archive that was tested. The toolkit keeps names as raw bytes (`Vec<u8>`); it decodes them only to show them, and never re-encodes a displayed name when writing.
- Paths shown and typed use `/` as the separator and are matched case-insensitively.

## How the client finds a file

Every directory is a map from a hash to an entry. The hash is **djb2 over the lower-cased name** (`h = 0x1505; h = h * 33 + c`, ASCII `A`-`Z` folded to lower case, characters taken as signed bytes). Consequences:

- Lookups are case-insensitive.
- If two entries in one directory have the same name (in any case), **the later entry wins**; the earlier one is never read. `Index::put` therefore replaces in place instead of adding a second entry, and `Index::dedupe` can drop the shadowed ones.
- The client never reads the free list; only an updater uses it.

## Shadowing

An entry is *shadowed* when a later entry in the same directory has the same lookup hash; the client loads only the later one. The hash is computed over the whole name, so two different names that happen to share a hash shadow each other too (`shaiya archive check` calls this `hash_collision`).

- **Folders shadow as well.** When two sub-directories of one directory have the same (case-folded) name, the later directory wins and **everything in the earlier one is shadowed**, files and sub-folders alike.
- Shadowed entries still occupy space in `data.saf` and are still listed in the index. The toolkit hides them by default (`archive list`), reports each one as a `shadowed` warning in `archive check` and shows them with `--include-shadowed`; `extract --include-shadowed` writes them as `name~shadowed-N.ext`.
- A visible-file count is therefore *files minus shadowed*; the [version matrix](../version-matrix.md) uses that number.
- `compact` and writes to a new pair (`-o`) drop shadowed entries (and the free list); in-place edits leave them alone.

## The free list and dead space

The free list is a list of ranges of `data.saf` that an updater may reuse. Measured on real clients: it is empty in some clients and holds hundreds of entries in others (507, 500 and 630 in the clients tested), the entry lengths vary, and the client itself never reads it. The toolkit preserves it byte for byte on in-place edits and drops it on compaction and on writes to a new pair.

Stock archives are not necessarily packed tightly: a single byte is commonly left between files, and the free list or earlier replacements leave further gaps. `shaiya archive stats` reports the unused bytes (bytes of `data.saf` that no entry references; ranges shared by several entries count once) and the number of gaps.

## Dead space and compaction

Replacing or removing a file leaves its old bytes in `data.saf`. `shaiya_archive::compact` rewrites `data.saf` with only the bytes the index uses (in their old order), drops shadowed duplicates and the free list, and points the index at the new offsets.

It is crash safe: both new files are written as `*.compact` and synced, a commit marker `data.compact-commit` is created, and the two files are renamed over the originals. `finish_compact` (run it before touching an archive) completes a committed compaction or discards an uncommitted one, so the index and `data.saf` always match. It needs free disk space for one more copy of the live data (the CLI's dry run says how much: live bytes plus 256 MB), keeps the previous index as `data.sah.bak`, and can be cancelled: a cancelled compaction removes its temporary files and leaves the archive as it was.

**Compaction ends rollback.** It writes a new `data.saf` and a new index, so the journals made by earlier in-place changes (which restore the old index and cut `data.saf` back) no longer apply; the toolkit reports them as blocked. The old `data.saf` is not kept. See [undo and recovery](../guides/undo-and-recovery.md).

## Reading without loading the SAF

`Archive` opens the SAF read-only with shared access (a running game does not block reading) and never loads it whole: entries are read by offset, and large entries stream in chunks of at most 1 MiB.

## Changing an archive in place

`shaiya_archive::edit::apply_in_place` (used by `shaiya archive add|replace|remove --in-place`, `shaiya patch apply` and Shaiya Toolkit) never overwrites file contents. It appends the new bytes to `data.saf`, syncs, and only then replaces `data.sah` atomically, so at every moment the index in the folder describes valid data. Before that it records a journal and a pending marker in the client folder and, on failure or cancel, cuts `data.saf` back to its old length. Replaced and removed files leave unused bytes that `compact` reclaims. The journal files, the marker and the recovery rules are described in [undo and recovery](../guides/undo-and-recovery.md); they are local backups made by the toolkit and are not part of the archive format.

`shaiya_archive::swap` puts another pair in place of a client's own by four renames recorded in `data.swap-journal`; `pack_folder` builds a new pair from a folder (sorted, names in code page 949); `edit::write_new` writes an edited, compacted copy as a new pair.
