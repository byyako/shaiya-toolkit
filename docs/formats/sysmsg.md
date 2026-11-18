# `sysmsg-uni.txt` and map names

`sysmsg-uni.txt` is the client's table of system messages. It sits at the top of the client's data (the archive root). The toolkit reads it for text, and the editor uses it for map names.

## Layout

- UTF-16, little-endian, with a byte order mark. An odd trailing byte is an error.
- One message per line: the id as decimal digits, blanks or TABs (the original lines have two TABs), then the text in double quotes.
- The text is what lies between the first and the last quote on the line, so a quote inside the text is kept. A line that has no quote after the id, or whose id is not followed only by blanks before the quote, is skipped.

```
5068		"D-Water Land"
```

## Map names

The client shows a map's name from a table in its program, which gives a sysmsg id for each map id. The editor mirrors that table (`shaiya_clientdata::map_name_msg`). A map with no entry, or whose text is missing or empty, shows as `Map <id>`.

| Map ids | sysmsg id |
|---|---|
| 0 | 5068 |
| 1 | 5069 |
| 2 | 5070 |
| 3 to 4 | 5071 |
| 5 to 6 | 5072 |
| 7 to 8 | 5073 |
| 9 to 11 | 5096 |
| 12 to 14 | 5097 |
| 15 to 17 | 5098 |
| 18 | 5074 |
| 19 | 5099 |
| 20 | 5100 |
| 21 to 22 | 5101 |
| 23 to 24 | 5102 |
| 25 | 5103 |
| 26 to 27 | 5104 |
| 28 | 5105 |
| 29 | 5106 |
| 30 | 5107 |
| 31 | 5108 |
| 32 | 5109 |
| 33 | 5110 |
| 34 | 5111 |
| 35 | 5112 |
| 36 | 5113 |
| 42 | 5419 |
| 43 | 5375 |
| 44 | 5376 |
| 45 | 5378 |
| 46 | 5379 |
| 47 | 5380 |
| 48 to 49 | 5405 |
| 50 | 5406 |
| 51 to 52 | 5407 |
| 53 to 54 | 5408 |
| 55 to 56 | 5409 |
| 57 | 5410 |
| 58 | 5411 |
| 59 | 5412 |
| 60 | 5413 |
| 61 | 5414 |
| 62 | 5415 |
| 63 | 5416 |
| 68 | 5425 |
| 69 | 5426 |
| 70 | 5427 |
| 102 to 105 and 108 | 5417 |

The ids 37 to 41, 64 to 67, 71 to 101, 106 and 107 have no entry. There are 47 distinct sysmsg ids covering 67 map ids.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `SysMsg` | reads the messages |
| Shaiya SVMap Editor and `shaiya svmap` | map names in lists, tooltips and CSV files, and the info rule `map-no-name`. The names are never used to decide a write. A new map's name cannot be added by data: the table lives in the client's program, so the wizard writes the name you chose into a note file instead. |

The file is only read.

## Verified on

US EP4.5 patch 108. The version matrix row for map-name ids finds text for all 47 ids on every client that has the file, except cn-ep4, which lacks three (44 of 47).

## Other episodes

The table is the EP4.5 client's. On another client the names and `map-no-name` may be wrong. When the client's `sysmsg-uni.txt` lacks some of the 47 ids, the editor and `shaiya svmap` print a note that says how many. Without a client the map number is shown.
