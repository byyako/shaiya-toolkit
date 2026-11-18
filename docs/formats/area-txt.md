# World area names (`world/<id>.txt`)

`world/<id>.txt` holds the names of the named areas of one map. The client loads it after the map's `.wld` ([World file](wld.md)). Area number i of the `.wld` is named by the line whose first column is i.

## Layout

- UTF-16, little-endian, normally with a byte order mark (`FF FE`) at the start.
- One line per area: the area number as decimal digits, a TAB, then the name. Lines end with CRLF in the original files.
- The numbers need not run without gaps. On map 2 the names cover areas 0 to 66, 87 and 88, and the areas in between are picture captions with no name.
- A line without a TAB, or with a first column that is not a number, is not a name. The toolkit keeps such lines but does not read them.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `text::parse_area_names` | reads the names as the client's reader sees them |
| `shaiya-clientdata` `AreaTxt` | a line-preserving model. Every line keeps its own ending, so a file that nothing touched writes back byte for byte: with or without the byte order mark, CRLF or LF, a missing final newline, an odd trailing byte, lines it does not understand. |
| Shaiya SVMap Editor | reads names for regions and writes a changed name to the staging folder as `data/world/<id>.txt`. A new map gets a copy of its source's names, or a file with only the byte order mark. |

`AreaTxt::set_name` changes one line (it keeps the number text and the line ending) or inserts a new line before the first line with a greater number. Inserting a region renumbers the following lines so each name stays with its area. A name with a TAB or a line break is refused. A file that does not look like UTF-16 text (an odd byte count, or no byte order mark and no `number TAB name` line) is not changed, so UTF-16 is never mixed into a file in another encoding. The only untouched line that can change is a last line without a newline: it gets one when a line is appended after it.

## Verified on

US EP4.5: all `world/*.txt` files of the client write back byte for byte (33 files in the version matrix row). The names read as the client's reader sees them. A `<id>.txt` that holds only a byte order mark has not been tried in the game.

## Other episodes

The line-preserving model keeps whatever it does not understand, so another episode's file is not damaged by a read and write. The format on other clients is unverified. The order of the lines of a new map's file in the game has not been checked.
