# myRDCcenter: technical notes

This document is for developers and for users who want to know how the app works. For daily use, read the [README](../README.md).

## Project layout

myRDCcenter is written in Rust. One core library holds all logic. Two apps give it a user interface.

| Folder | Content |
|---|---|
| `core` | Vault, connections, groups, import and export, RDP launch, colors (`myrdc-core`) |
| `app-egui` | `myRDCcenter-egui`, made with egui / eframe (glow renderer) |
| `app-slint` | `myRDCcenter-slint`, made with Slint (Fluent style) |
| `assets` | App icon in SVG, PNG, ICO and RGBA |
| `packaging` | Linux `.desktop` files and the macOS `Info.plist` |
| `docs` | This document and the screenshots |

Both apps use the same vault file. The colors of both apps come from `core/src/theme.rs`. The Slint file `app-slint/ui/app.slint` has a copy of the main colors in its `Theme` global.

## How Connect works

| System | Client | Login |
|---|---|---|
| Windows | Built-in Remote Desktop (`mstsc`) | Automatic. The app stores the password for 60 seconds as a session-only Windows credential (`TERMSRV/<host>`). |
| macOS | **Windows App** (before: Microsoft Remote Desktop) from the App Store | Windows App asks for the password. |
| Linux | FreeRDP (`xfreerdp3`, `sdl-freerdp3` or `xfreerdp`) | Automatic. The app sends the password on stdin, never on the command line. |

On Windows and macOS, the app writes a temporary `.rdp` file. It deletes the file again after 60 seconds.

### Full screen on Windows

For **Full screen**, the `.rdp` file gets the size and the display scale (125 %, 150 %, ...) of the primary monitor:

- `desktopwidth` and `desktopheight`: the pixel size of the monitor
- `desktopscalefactor`: the Windows display scale, rounded to a value that `mstsc` accepts
- `dynamic resolution:i:1`: the session follows changes to the window size and scale

Without these lines, `mstsc` uses the size from `Default.rdp` and a scale of 100 %. Text then looks too small on high-DPI screens. With **Use all monitors**, the app does not set a size, so `mstsc` can use all monitors.

The remote computer must run Windows 8.1 / Server 2012 R2 or later to use the scale.

## Vault file

The vault is one encrypted file:

| System | Location |
|---|---|
| Windows | `%LOCALAPPDATA%\myRDCcenter\vault.rdsc` |
| Linux | `~/.local/share/myRDCcenter/vault.rdsc` |
| macOS | `~/Library/Application Support/myRDCcenter/vault.rdsc` |

To use a different vault file, set the environment variable `MYRDCCENTER_VAULT` to its path. This is useful for a demo vault or a vault on a USB stick.

A vault from the old name "RDS-center" moves to the new location automatically.

## Security

- The key comes from the master password with Argon2id (64 MiB, 3 passes).
- XChaCha20-Poly1305 encrypts the data. The file header is authenticated too.
- The app writes the vault to a temporary file first and then renames it. A crash cannot leave a broken vault.
- The master password cannot be recovered.
- Passwords are wiped from memory when the app no longer needs them.

### Remember on this computer

When this option is on, the app stores the vault key on the computer:

- Windows: protected with DPAPI, so only your Windows login can read it.
- macOS: stored in the Keychain.

Anybody who can log in as you can then open the vault. When you clear the option, the app deletes the stored key.

## Import and export

| File | Content |
|---|---|
| `.rdp` | One connection. UTF-16 and UTF-8 files are supported. Passwords are not imported. |
| `.rdm` | An export with folders. RDP entries, folders and usernames are imported. Other entry types (SSH, web, ...) are skipped. The passwords are encrypted by the program that made the file, so they are not imported. |
| `.rdcx` | A myRDCcenter export. It contains connections, groups and passwords, encrypted with the password of the file. |

The import does not make duplicate connections when you import the same file again.

## Build

1. Install Rust from https://rustup.rs.
2. On Linux, install these packages: `libwayland-dev libxkbcommon-dev libfontconfig1-dev libgl1-mesa-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev`.
3. Run `cargo build --release`.

The apps are in `target/release/`.

To run the tests of the core library, run `cargo test -p myrdc-core`.

## Make a release

1. Change `version` in the root `Cargo.toml`.
2. Commit and push to `main`.
3. Push a tag with the same version, for example `v0.6.4`.

GitHub Actions then builds all files for Windows, Linux and macOS and attaches them to the release. You can also start the **Release** workflow on the Actions tab and type the tag name.

The Windows `.exe` files need only DLLs that come with Windows, because the C runtime is linked statically. The macOS apps are universal builds for Apple Silicon and Intel.

## Make the screenshots

The screenshots in `docs/` use a demo vault, not real data.

1. Make the demo vault:

   ```
   cargo run -p myrdc-core --example demo_vault -- C:\temp\demo.rdsc
   ```

2. Build the apps with the `screenshot` feature:

   ```
   cargo build -p myrdccenter-egui -p myrdccenter-slint --features screenshot
   ```

3. Set these environment variables and start each app:

   | Variable | Value |
   |---|---|
   | `MYRDCCENTER_VAULT` | `C:\temp\demo.rdsc` |
   | `MYRDCCENTER_SELECT` | `SQL server` |
   | `MYRDCCENTER_SCREENSHOT` | `docs\screenshot-egui.png` or `docs\screenshot-slint.png` |

The app opens the demo vault, selects the connection, saves the PNG file and stops. Release builds do not include the `screenshot` feature.

## egui flicker

egui can flicker on Windows with default settings. The egui app uses these settings to stop it:

- glow renderer with vsync and no MSAA
- no fade animations and no continuous repaint
- fixed container ids, so a new selection does not start a new layout pass
- a clear color that is the same as the window background

See the comment at the top of `app-egui/src/main.rs`.
