# Worminal

Worminal is a small desktop terminal. Prioritize the path from launch to usable shell input, compact text rows, and familiar Alacritty colors over interface chrome.

The app is Tauri 2 with a Rust PTY backend and a static xterm.js frontend. Rust starts one shell during app setup and buffers initial output until the webview attaches. The frontend owns rendering, resize, and the persisted line height setting. Theme colors come from Alacritty TOML files and their imports.

Current job: complete and verify the first vertical slice, then measure startup and input latency before adding features.
