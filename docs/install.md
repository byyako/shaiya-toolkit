# Install

Download from the [releases page](https://github.com/byyako/shaiya-toolkit/releases). No release has been published yet; the first one comes with the archive and patch tools.

## Which zip

Every release has one zip per tool per platform, plus an everything zip:

| Zip | Contents |
|---|---|
| `shaiya-toolkit-<version>-<platform>.zip` | all programs |
| `shaiya-<version>-<platform>.zip` | the `shaiya` command line only |
| `shaiya-workbench-<version>-<platform>.zip` | Shaiya Workbench only |

`<platform>` is `windows-x86_64`, `linux-x86_64` or `macos-universal`. Each zip holds a folder named like the zip with the program(s), `README.txt`, `LICENSE.txt` and `THIRD-PARTY-NOTICES.txt`. Unzip it anywhere; there is no installer. The GUI programs can run side by side and start each other from their Tools menu when they sit in the same folder.

## Check the download

`SHA256SUMS.txt` on the release page lists `<sha256>  <file>` for every zip. Compare:

- Windows (PowerShell): `Get-FileHash .\shaiya-0.1.0-windows-x86_64.zip -Algorithm SHA256`
- Linux: `sha256sum -c SHA256SUMS.txt --ignore-missing`
- macOS: `shasum -a 256 -c SHA256SUMS.txt --ignore-missing`

Releases built on a public repository also carry a build provenance attestation that can be verified with `gh attestation verify <zip> --repo byyako/shaiya-toolkit`.

## Platform notes

- **Windows.** The programs are not code signed. SmartScreen may show "Windows protected your PC" the first time: choose More info, then Run anyway. The `shaiya` program is a console program: open a terminal in its folder and run `shaiya --help`.
- **macOS.** The GUI programs are app bundles, ad-hoc signed and not notarised. If macOS says the developer is unidentified, right-click the app and choose Open, or run `xattr -dr com.apple.quarantine "Shaiya Workbench.app"`. The macOS GUI is **untested** until a tester confirms it.
- **Linux.** The GUI programs need a desktop with X11 or Wayland and a Vulkan or OpenGL driver. If a file is not executable: `chmod +x shaiya shaiya-workbench`.

## Portable mode and where settings live

By default settings, logs, recovery files and backups live in the user's configuration folder (`%APPDATA%\shaiya-toolkit` on Windows, `~/Library/Application Support/shaiya-toolkit` on macOS, `~/.config/shaiya-toolkit` or `$XDG_CONFIG_HOME/shaiya-toolkit` on Linux). Put an empty file named `portable.txt` next to the program to keep them in a `shaiya-toolkit-data` folder beside it instead. `SHAIYA_TOOLKIT_HOME` names any folder, and the GUI flag `--data-dir DIR` does the same for one run.

## Build from source

```
cargo build --release
```

with stock Rust (`rustup`, stable). The programs are in `target/release`.
