# Worminal

Worminal is a small desktop terminal. Prioritize the path from launch to usable shell input, compact text rows, and familiar Alacritty colors over interface chrome.

The app is Tauri 2 with a Rust PTY backend and a static xterm.js frontend. Rust starts one shell on a worker after app setup; early input and resize requests queue until it is ready, and initial output is buffered until the webview attaches. The frontend owns rendering, resize, and the persisted line height setting. Theme colors come from Alacritty TOML files and their imports.

Current job: measure launch to usable input on a real desktop, then optimize the remaining delay before adding features.
