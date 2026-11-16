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

- `version` is **0** in most clients and **1** in at least one (Fawkes v68). A higher version is reported as unsupported by `shaiya info`.
- `file_count` must equal the number of file entries in the tree; a mismatch is an error.
- The offsets are into `data.saf`. A reader checks every entry against the size of `data.saf` before reading.
- The parser is bounds-checked: counts are checked against the bytes that remain, the tree depth is limited to 64, names to 4096 bytes. A damaged index gives an error naming the byte offset, never a panic.
- The free list and the trailer are kept as they are (`Index::free`, `Index::trailer`), so writing an unchanged index back is **byte-identical**.

## Names

- Names are stored in **code page 949** (Korean) or ASCII. The toolkit keeps names as raw bytes (`Vec<u8>`); it decodes them only to show them, and never re-encodes a displayed name when writing.
- Paths shown and typed use `/` as the separator and are matched case-insensitively.

## How the client finds a file

Every directory is a map from a hash to an entry. The hash is **djb2 over the lower-cased name** (`h = 0x1505; h = h * 33 + c`, ASCII `A`-`Z` folded to lower case, characters taken as signed bytes). Consequences:

- Lookups are case-insensitive.
- If two entries in one directory have the same name (in any case), **the later entry wins**; the earlier one is never read. `Index::put` therefore replaces in place instead of adding a second entry, and `Index::dedupe` can drop the shadowed ones.
- The client never reads the free list; only an updater uses it.

## Dead space and compaction

Replacing or removing a file leaves its old bytes in `data.saf`. `shaiya_archive::compact` rewrites `data.saf` with only the bytes the index uses (in their old order), drops shadowed duplicates and the free list, and points the index at the new offsets.

It is crash safe: both new files are written as `*.compact` and synced, a commit marker `data.compact-commit` is created, and the two files are renamed over the originals. `finish_compact` (run it before touching an archive) completes a committed compaction or discards an uncommitted one, so the index and `data.saf` always match. It needs free disk space for one more copy of the live data, keeps the previous index as `data.sah.bak`, and can be cancelled: a cancelled compaction removes its temporary files and leaves the archive as it was.

## Reading without loading the SAF

`Archive` opens the SAF read-only with shared access (a running game does not block reading) and never loads it whole: entries are read by offset, and large entries stream in chunks of at most 1 MiB.
