# Server `Map.ini` and `MapWar.ini`

`Map.ini` lists the maps the game server loads and sets per-map options. `MapWar.ini` has the same layout for the war server's maps. Both live in the server data folder, the parent of the `Map` folder that holds the `.svmap` files.

## Layout

The file is cp949 text with CRLF line ends and Korean comments (the original `Map.ini` has 408 lines).

- `//` starts a comment that runs to the end of the line.
- A `/* ... */` block at the top holds a sample with `MapCount = 9`. It is a comment and is not read.
- `[NAME]` starts a section. `key = value` lines belong to the section above them. Section and key names are matched without regard to case. If a key is written twice, the last one wins.

| Section | Keys |
|---|---|
| `[BASE]` | `MapCount`, and four start positions |
| `[DEFAULT_SET_ZONE]` | the keys below, as the defaults for every zone |
| `[SET_ZONE_n]` | the settings of map n |

Keys of a zone as the editor shows them: `MapType` (`F` field, `D` dungeon), `MapName`; the instance keys `CreateType` (`S`, `F`, `P`, `G`, `R` or `H`), `CreateTime`, `CreateMinUser`, `EnterMaxUser`, `ExpireTime`, `WarType`; the three rebirth positions `RebirthMapPos`, `RebirthMapPos1`, `RebirthMapPos2`, each `map, x, y, z`; and the weather keys `WeatherState`, `WeatherRate`, `WeatherPower`, `WeatherDelay`, `WeatherNoneDelay`. A rebirth position with map -1, or with x, y and z all 0, is ignored by the server. The units of `ExpireTime` and the meaning of every `CreateType` letter are not known to the toolkit.

A key that a zone leaves out is assumed to take its value from `[DEFAULT_SET_ZONE]`. The original files point that way. It is not confirmed.

## Which maps are loaded

- `Map.ini`: the server loads the map ids 0 to `MapCount` - 1. In the original server a missing section is fine, but a missing `<id>.svmap` for any id below `MapCount` stops the server from starting.
- `MapWar.ini`: zone n is map n, and the war maps start at 100 (`[SET_ZONE_100]` and up). Its `MapCount` is the number of war maps, the highest id minus 99. Adding zone 109 gives `MapCount = 10`.
- Afterglow skips an id that has no map file. It accepts ids up to 255 except 200, and does not import `MapWar.ini`.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-svmap-tools` `MapIni` | a line-preserving reader. Every line is kept as raw bytes with its own ending, and every value keeps its byte range, so unchanged lines write back byte for byte. |
| Shaiya SVMap Editor | the Map.ini editor and Add a map change a value in place (only its own bytes, a `//` comment after it stays), remove a key line, or append a new key after the last key of its zone. A new zone goes at the end of the file, with `MapCount` raised to what the file needs. A new zone never lowers `MapCount`. |
| `shaiya svmap check --server-data` | reads both files for the rules `mapini-no-svmap`, `svmap-not-loaded`, `mapini-rebirth-outside` and `portal-dest-no-map` |

The editor writes only plain ASCII values on one line, with no `//` or `/*` inside. A file without `[BASE] MapCount` is not edited. A save writes a temporary file, reads it back, keeps a copy of the old file (`.bak` once, and a timestamped copy in the editor's data folder), and renames last. It refuses when the file changed on disk since it was read.

## Verified on

The original server files (US EP4.5 server, `Map.ini` with `MapCount` 68 and `MapWar.ini` for the maps 100 to 108) read and write back byte for byte. Afterglow's behaviour for gaps comes from its import script. A new map made with Add a map (id 110) imported on Afterglow.

## Other episodes

Other server builds are unverified. The editor offers a Target server setting (original or Afterglow) that changes how a missing map file is reported (an error for the original server, info for Afterglow). An edited `Map.ini` has not been checked live on a running server.
