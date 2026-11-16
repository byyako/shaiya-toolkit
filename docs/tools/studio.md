# Shaiya Archive & Patch Studio

The desktop program `shaiya-studio`. In this version it is a **file inspector** on the shared GUI shell; the archive browser and the patch tools arrive in the next version.

## Using it

- Open files with **File > Open** (Ctrl+O), **File > Open path...** (Ctrl+Shift+O: type or paste a path), **drag and drop**, or the command line: `shaiya-studio data.sah game.exe`.
- Studio identifies each file (SAH index, SAF data, SData encrypted or plain, patch, signature, `game.exe` build) and shows its Name, Kind, Size and Details in a table, the same text `shaiya info` prints.
- **SHA-256:** press the button in a row, or select rows and use **Compute SHA-256** (all rows when none is selected). Hashing runs in the background with byte progress in the **Activity** panel (Ctrl+J) and can be cancelled there; a cancelled row says "Cancelled" and offers a retry.
- **Copy as text / Copy as JSON** put the selected rows (all rows when none is selected) on the clipboard: handy for bug reports. Never attach game files to a report; the hashes are enough.
- A file that cannot be read appears as an error row (the message is in the Details column and in Activity); there is no banner for per-file problems.
- **Remove from list** drops the selected rows.

## The shell

Menus: File, Edit, View, Tools, Help. **Tools > Command palette** (Ctrl+Shift+P) lists every command. **Edit > Preferences** (Ctrl+,): theme (dark, light, system), UI scale, autosave and backup counts, the font used for Korean and Chinese names, and the column widths. **Help > About** shows the version, the git revision and the third-party notices.

Korean and Chinese names need a font: Studio looks for a system font at start (Malgun Gothic or Microsoft YaHei on Windows, Apple SD Gothic Neo or PingFang on macOS, Noto Sans CJK or a similar font on Linux). Name a font file in Preferences, or set `SHAIYA_TOOLKIT_FONT`, to use another one.

Command line: `shaiya-studio [FILES...] [--data-dir DIR] [--no-settings] [--smoke [REPORT_FILE]]`. `--smoke` opens the real window, renders five frames, writes `ok <adapter> <backend>` (or the error) to the report file and exits with 0 or 1: a quick check that the graphics stack works.

Settings and logs: see [install.md](../install.md#portable-mode-and-where-settings-live).
