# Install

Download from the [releases page](https://github.com/byyako/shaiya-toolkit/releases).

## Which zip

Every release has two zips per platform:

| Zip | Contents |
|---|---|
| `shaiya-toolkit-<version>-<platform>.zip` | the two desktop programs, Shaiya Toolkit and Shaiya SVMap Editor, and the `shaiya` command line |
| `shaiya-<version>-<platform>.zip` | the `shaiya` command line only |

`<platform>` is `windows-x86_64`, `linux-x86_64` or `macos-universal`. Each zip holds a folder named like the zip with the program(s), `README.txt`, `LICENSE.txt` and `THIRD-PARTY-NOTICES.txt`. Unzip it anywhere; there is no installer. The all-in-one zip holds two GUI programs: Shaiya Toolkit (`shaiya-toolkit`) and Shaiya SVMap Editor (`shaiya-svmap-editor`), for server map files. Further GUI programs will start each other from their Tools menu when they sit in the same folder.

## Check the download

`SHA256SUMS.txt` on the release page lists `<sha256>  <file>` for every zip. Compare:

- Windows (PowerShell): `Get-FileHash .\shaiya-0.2.1-windows-x86_64.zip -Algorithm SHA256`
- Linux: `sha256sum -c SHA256SUMS.txt --ignore-missing`
- macOS: `shasum -a 256 -c SHA256SUMS.txt --ignore-missing`

Releases built on a public repository also carry a build provenance attestation that can be verified with `gh attestation verify <zip> --repo byyako/shaiya-toolkit`.

## Platform notes

- **Windows.** The programs are not code signed. SmartScreen may show "Windows protected your PC" the first time: choose More info, then Run anyway. The `shaiya` program is a console program: open a terminal in its folder and run `shaiya --help`.
- **macOS.** The GUI programs are app bundles, ad-hoc signed and not notarised. If macOS says the developer is unidentified, right-click the app and choose Open, or run `xattr -dr com.apple.quarantine "Shaiya Toolkit.app"`. The macOS GUI is **untested** until a tester confirms it. The same goes for the Linux build of Shaiya SVMap Editor; the editor was tested on Windows.
- **Linux.** The GUI programs need a desktop with X11 or Wayland and a Vulkan or OpenGL driver. If a file is not executable: `chmod +x shaiya shaiya-toolkit shaiya-svmap-editor`.

## Portable mode and where settings live

By default settings, logs, recovery files and the saved client folders live in the user's configuration folder (`%APPDATA%\shaiya-toolkit` on Windows, `~/Library/Application Support/shaiya-toolkit` on macOS, `~/.config/shaiya-toolkit` or `$XDG_CONFIG_HOME/shaiya-toolkit` on Linux). Put an empty file named `portable.txt` next to the program to keep them in a `shaiya-toolkit-data` folder beside it instead. `SHAIYA_TOOLKIT_HOME` names any folder, and the flag `--data-dir DIR` does the same for one run, on the `shaiya` command line (`shaiya --data-dir DIR clients list`), on Shaiya Toolkit and on Shaiya SVMap Editor alike.

Shaiya SVMap Editor keeps its settings, log, recovery files and backups of saved maps in a `shaiya-svmap-editor` folder inside that data root. It does not read Shaiya Toolkit's saved client folders.

**Backups of your game files are not kept there.** Journals, and with them everything needed to roll back a change, live **inside the client folder** you changed, in `shaiya-toolkit-backups/`, next to `data.sah.bak` (a copy of the previous index). They go with the folder when you copy, move or delete it, and they are not removed when you uninstall or delete the toolkit. See [undo and recovery](guides/undo-and-recovery.md).

## Build from source

```
cargo build --release
```

with stock Rust (`rustup`, stable). The programs are in `target/release`.

On Linux the GUI needs development packages (the names are those CI installs on Ubuntu):

```
sudo apt-get install -y libxkbcommon-dev libwayland-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libx11-dev pkg-config
```

Windows and macOS need nothing besides Rust.

Next: [getting started in five minutes](getting-started.md).
