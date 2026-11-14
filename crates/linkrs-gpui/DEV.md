# linkrs-gpui Developer Guide

Native desktop client for Linkrs, built on GPUI. This crate is intentionally
**not** a member of the root workspace: it is its own workspace consuming
`../gpui-kit` through a local path (nested-workspace inheritance plus a
tree-sitter `links` conflict make a single-workspace setup unresolvable).
Always build and test with the manifest flag or from inside this directory:

```sh
cargo build --manifest-path crates/linkrs-gpui/Cargo.toml
cargo test --manifest-path crates/linkrs-gpui/Cargo.toml
# or: cd crates/linkrs-gpui && cargo build
```

Plain `cargo build` from the repository root does **not** cover this crate.

## Prerequisites

- Rust 1.97+ (see `rust-version` in `Cargo.toml`).
- The `crates/gpui-kit` submodule, initialized before the first build:

```sh
git submodule update --init crates/linkrs-gpui
# or initialize everything at once:
git submodule update --init --recursive
```

Local builds never fetch git; all GPUI sources resolve to submodule checkouts.

- A C toolchain plus `pkg-config` (`cc`, `pkg-config` / `pkgconf`).
- System libraries for font rendering and Linux windowing:
  `fontconfig`, `freetype2`, `libpng`, `libxcb`, `libxkbcommon`,
  `libxkbcommon-x11` (D-Bus client libraries are additionally needed at link
  time for secret storage; most desktop installs already ship them).

Per-distro install commands (require root):

```sh
# Arch Linux
sudo pacman -S --needed base-devel pkgconf fontconfig freetype2 libxcb libxkbcommon libxkbcommon-x11

# Debian / Ubuntu
sudo apt install build-essential pkg-config libfontconfig-dev libfreetype-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev libdbus-1-dev

# Fedora
sudo dnf install gcc pkgconf fontconfig-devel freetype-devel \
  libxkbcommon-devel libxkbcommon-x11-devel libxcb-devel dbus-devel
```

Package names drift across releases; the rule is: the linker needs
`fontconfig`, `freetype`, `xcb`, `xkbcommon`, `xkbcommon-x11`, and `dbus-1`,
while `pkg-config` must be able to resolve `fontconfig.pc`. Install them
through the system package manager only; vendoring libraries into a
user-local prefix is not supported.

## Running

The client needs a reachable Linkrs server; sign in with host, port and
credentials in the login view. A running Wayland/X11 session is required for
the window; on headless machines only `cargo build` and `cargo test` are
meaningful (unit tests cover services, statement builders, catalogue
consistency and graph extraction without opening a window).

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `Package fontconfig was not found` from `yeslogic-fontconfig-sys` | Missing system fontconfig | Install system packages (see above) or use the sysroot fallback |
| `rust-lld: error: unable to find library -lxcb` (or `-lxkbcommon`, `-lxkbcommon-x11`) | Missing X11/windowing dev libraries | Same as above |
| `could not find gpui-kit` / path dependency errors | Submodule not initialized | `git submodule update --init --recursive` |
| Binary starts but windows stay blank | No Wayland/X11 session | Run inside a desktop session or nested compositor |
| `libfontconfig.so.1: cannot open shared object file` at startup | System libraries installed but loader cache stale | Re-login or run `sudo ldconfig`, then retry |
