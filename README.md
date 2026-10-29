# RDS-center

A small, offline Remote Desktop (RDP) connection manager for Windows, in Rust.
Two front-ends share the same core and the same vault file:

| App | Folder | UI toolkit |
|---|---|---|
| `RDS-center-egui.exe` | `app-egui` | egui / eframe (glow) |
| `RDS-center-slint.exe` | `app-slint` | Slint (Fluent style) |

Each is a single `.exe`. It only needs DLLs that ship with Windows (the C runtime is linked statically).

## Features

- Connections grouped in folders (`Work/Servers`), with search
- Host, port, user, domain, password, RD Gateway, screen size, all monitors, clipboard/drive sharing, admin session, notes
- Double-click or **Connect** opens the built-in Windows client (`mstsc`) and logs in automatically
- Import existing `.rdp` files
- Encrypted backup, change master password, auto-lock after idle time

## Security

- Everything is in one file: `%LOCALAPPDATA%\RDS-center\vault.rdsc`. No account, no sync, no network.
- The key is derived from the master password with Argon2id (64 MiB, 3 passes). Data is encrypted with XChaCha20-Poly1305, and the file header is authenticated too.
- The master password can't be recovered. Keep a backup and remember the password.
- When you connect, the password is stored for a short time as a session-only Windows credential (`TERMSRV/<host>`) so `mstsc` can log in. A temporary `.rdp` file is written to `%TEMP%`. Both are removed 60 seconds later.

## Build (on Windows)

Install Rust from https://rustup.rs, then run:

```
cargo build --release
```

The executables are in `target\release\`.

Cross-build from Linux: `rustup target add x86_64-pc-windows-gnu`, install mingw-w64, then run `cargo build --release --target x86_64-pc-windows-gnu`.

Run the core tests with `cargo test -p rds-core`.

## egui flicker

The egui app has settings to stop flicker: the glow renderer with vsync, no MSAA, no fade animations, and no continuous repaint. Containers keep fixed ids, so changing the selection does not start a new layout pass. The clear colour is the same as the panel colour. See the comment at the top of `app-egui/src/main.rs`.
