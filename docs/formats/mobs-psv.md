# Afterglow `mobs.psv`

`mobs.psv` is an export of the monster definitions of the Afterglow server. The original server keeps its monsters in a database, so it has no such file. The editor can read it to check monster ids.

## Layout as read

A text file with one row per line. Fields are separated by `|`: the row number, the monster id, a name field and more fields. Only the second field is read, as a decimal number. A line whose second field is not a number is ignored.

```
1|0|3F3F|0
2|1|D0A1|38
```

The setting for the export in Preferences (or `--afterglow-defs` of `shaiya svmap check`) is the file itself or a folder that holds it directly, in `defs-export` or in `data/defs-export`.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-svmap-tools` `server_data` | `parse_mob_ids` returns the set of ids |
| Shaiya SVMap Editor and `shaiya svmap check --server-data --afterglow-defs` | the rule `mob-unknown-server` reports a monster id of a map that is not in the export. It runs only when an export is set. |

The file is only read.

## Verified on

Afterglow's export as used by the author. The id column is the only part checked.

## Other episodes

Other server builds have no such export. Without it, `mob-unknown-server` does not run and there is no note.
