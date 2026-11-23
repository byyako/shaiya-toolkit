# Building motion (`.MANI`)

`.MANI` files hold motion parameters for a placed building model: how it swings, orbits, spins or moves back and forth. On US EP4.5 they are all in the `Entity/MAni` folder of the archive. **Every meaning on this page is inferred** from the values and was not checked in the game; the toolkit knows the sizes and the order of the fields, not what the game does with them. Which models use a `.MANI` is not read here.

A `.MANI` is exactly 108 bytes, with no counts and no header, little-endian. A file of any other size is refused ("a .MANI is 108 bytes").

## Fields as read

| Offset | Name (the toolkit's) | Type | Meaning (inferred) |
|---|---|---|---|
| 0 | `tick_ms` | i32 | the motion steps once per this many milliseconds |
| 4 | `swing_on` | i32 | rocking about `swing_axis` (0 or 1) |
| 8 | `swing_axis` | f32 x 3 | axis of the rocking |
| 20 | `swing_limit` | f32 | radians; the direction flips at this angle |
| 24 | `swing_step` | f32 | radians per step |
| 28 | `swing_unused` | f32 | no use known |
| 32 | `swing_pause_ms` | i32 | wait at each end of the rocking, in milliseconds |
| 36 | `orbit_on` | i32 | circular offset about `orbit_axis` (0 or 1) |
| 40 | `orbit_axis` | f32 x 3 | axis of the circular offset |
| 52 | `orbit_step` | f32 | radians per step |
| 56 | `orbit_radius` | f32 | world units |
| 60 | `spin_on` | i32 | continuous rotation about `spin_axis` (0 or 1) |
| 64 | `spin_axis` | f32 x 3 | axis of the rotation |
| 76 | `spin_step` | f32 | radians per step |
| 80 | `move_on` | u16 | back-and-forth movement along `move_dir` (0 or 1) |
| 82 | `move_pause_ms` | u16 | wait at each end of the movement, in milliseconds |
| 84 | `move_dir` | f32 x 3 | direction of the movement |
| 96 | `move_speed` | f32 | start distance per step |
| 100 | `move_accel` | f32 | speed factor per step |
| 104 | `move_distance` | f32 | end point = `move_dir` times this |

The 20 fields cover the 108 bytes with no gap (a test checks it). The file is kept as its 108 raw bytes with typed views, so `Mani::to_bytes` gives back the bytes `Mani::parse` read whatever the float bits are. No program writes a `.MANI` file.

## Two writer generations

Two kinds of file exist in the archive; the toolkit shows both and interprets neither. In one, the integer 500 (bytes `f4 01 00 00`, which as a float is a denormal) sits at offset 104 and the bytes at 80 to 83 are zero. In the other, offsets 80 and 82 hold two `u16` and offset 104 holds a float. `Mani::distance_as_integer` gives the integer when the bits at 104 read as a denormal float.

## What the toolkit does with it

| Program | Use |
|---|---|
| `shaiya-clientdata` `mani` | parses the 108 bytes, writes them back in memory and gives every field with its offset, name, type and inferred meaning |

In Shaiya Toolkit's Archive tab a `.MANI` is shown as a table of its 20 fields (Offset, Field, Meaning, Value), under the header "Motion parameters for a placed building model (108 bytes). Field meanings are inferred and were not checked in the game." Every meaning is labelled inferred, and the stored bits of a value are on hover. The field at offset 104 is also shown as an integer when its bits read as a float are a denormal (the first of the two writer generations), and a file of the integer generation (the integer at offset 104 with zero bytes at 80 to 83) gets a note saying so (shown, not interpreted); a file of the other generation gets no note. A file that is not 108 bytes shows the error and the bytes as hex. `shaiya anim info` prints one line per file and, with `--json`, every field with its `offset`, `name`, `value`, `raw` (the stored bytes in hex), `meaning` and `"inferred": true`; `shaiya anim validate` and `compare` read `.ANI` files only.

## JSON (export and import)

The toolkit's library can turn a `.MANI` into a JSON text (`shaiya-toolkit-mani-v1`) and that text back into the file. An unedited file comes back byte for byte (US EP4.5: all 23 `.MANI` files; validated offline; the eight other matrix clients have their index only and were not read). The JSON goes in through the same Replace and `archive replace` paths as any file: nothing is written into a client folder. Every meaning of a field is inferred and was not checked in the game.

| Key | Content |
|---|---|
| `format` | `shaiya-toolkit-mani-v1`; any other name is refused, and a newer version (`-v2`) is refused as "made by a newer toolkit" |
| `source` | label only (ignored on import) |
| `inferred` | label: every meaning is inferred (ignored on import) |
| `fields` | exactly 20 objects, in the order of the field table above |
| `fields[i].offset`, `name`, `type` | must equal the table (`i32`, `u16`, `f32` or `f32x3`) |
| `fields[i].meaning` | label only (ignored on import) |
| `fields[i].value` | a whole number for `i32` and `u16`, a number for `f32`, three numbers for `f32x3` |

- There is no `columns` key. A float is written in its shortest form; NaN, infinity or a value that has no such number is written as `"bits:XXXXXXXX"`. The integer generation's value at offset 104 is a plain tiny number (for example `7e-43`) that comes back as the same bits.
- A field out of order or of another type, a value of another type, a count other than 20, unknown keys and a newer format are refused.

## Verified on

US EP4.5 (patch 108), read only: all 23 `.MANI` files are 108 bytes, read and write back byte for byte. 12 are of the first kind (the integer 500 at offset 104 and zero bytes at 80 to 83) and 11 of the second (two `u16` at 80: `(0, 500)` in 8 files and `(1, 0)` in 3). The opt-in test `every_mani_of_the_us_client_reads_and_writes_back` measures these numbers.

## Other episodes

The eight other matrix clients have their index only: their `.MANI` files are counted and every one the nine indexes list has a length of 108 bytes (see the [version matrix](../version-matrix.md)); no contents were read, so whether their fields mean the same is not checked.
