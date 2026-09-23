# Worminal

An early Tauri terminal emulator focused on quick shell input, compact rows, and Alacritty colors.

## Run

Install Node.js, Rust 1.88 or newer, and the [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/). Then:

```sh
npm install
npm run tauri dev
```

The dev command builds the frontend once and serves the built files through Tauri.
Restart it after frontend edits.

Restart an existing `tauri dev` process after installing rustup. The npm launcher
and `manage.py` prefer rustup's Cargo even when an older shell still has Debian's
Rust 1.85 first in `PATH`.

Run `python3 manage.py check` for the frontend build and Rust tests.

The settings button in the upper right adjusts line height from 0.5 to 1.5 and saves it locally. `Ctrl+Shift+L` also opens it.

Worminal reads the first Alacritty TOML config found in the usual XDG or home locations and follows its `[general].import` list. It supports the primary, normal, bright, dim, cursor, and selection color tables. Restart after changing a theme. Other Alacritty settings are outside this initial slice.
