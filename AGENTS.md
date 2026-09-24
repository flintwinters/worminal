# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface features; measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. Its C source handles windows, terminal state, PTYs, and bounded main-screen scrollback directly. It maps a provisional input-capable window early, lets the window manager place it, and starts the shell while fonts load. `manage.py build` resolves Alacritty TOML colors, font size, line spacing, and scrollback settings into a generated C header, so launch reads no config files. Compact rows repaint backgrounds before glyphs so descenders can overlap adjacent rows. An opt-in `-S` prototype lets launchers attach another window to one owner and one terminal session; only the most recently used view repaints PTY output. `config.mk` selects `-O3`, and `manage.py` checks and measures latency on a private Xvfb display. The upstream source and license remain in the repository.

Current job: finish repeatable shared-tab proofs without opening windows on the working desktop; then assess remaining integration gaps and measure launch and memory before adopting the prototype.
