# Server `Obelisk.ini`

`Obelisk.ini` is a line-based text file in the server data folder (next to `Map.ini`). It places obelisks on maps, among other settings. The toolkit reads only the zones and the obelisk positions.

## Layout

- A line that does not start with `[` (after blanks) is ignored. That covers comments that start with `//` or `--` and every value line.
- A line that starts with `[` is split at the characters `,` `=` `(` `)` `[` `]` `{` `}`, blanks and tabs. The first piece is the head, matched without regard to case.
- `[zone_N]` starts the zone of map N.
- `[Obelisk_K] = (x, y, z), ...` adds obelisk K at (x, y, z) to the current zone. The pieces after the first three numbers are not read.
- A head that starts with `Portal_` or `bothmob_` ends the current zone, so an `Obelisk_` line after it belongs to no zone and is skipped.
- A number that does not parse is 0.

Example:

```
[zone_0]
[Obelisk_001] = (155.10,39.43,190.38),(52, 252, 152),0
```

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-svmap-tools` `server_data` | `parse_obelisks` returns the zone, number and position of each obelisk |
| Shaiya SVMap Editor and `shaiya svmap check --server-data` | the rule `obelisk-pos` reports an obelisk outside its zone's map or on a blocked cell |

The file is never written.

## Verified on

The original server's `Obelisk.ini`: map 0 has seven obelisks and maps 18 and 30 one each, nine in all. The sample `[Portal_` line adds none.

## Other episodes

Other server builds are unverified. A missing or unreadable file only turns off `obelisk-pos`, with one note.
