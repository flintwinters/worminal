# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface features; measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. Its C source handles the window, terminal state, PTY, and bounded main-screen scrollback directly. It maps a provisional input-capable window early, lets the window manager place it, and starts the shell while fonts load. `manage.py build` resolves Alacritty TOML colors, font size, line spacing, and scrollback settings into a generated C header, so launch reads no config files. Compact rows repaint backgrounds before glyphs so descenders can overlap adjacent rows. `config.mk` selects `-O3`, and `manage.py` checks and measures latency on a private Xvfb display. The upstream source and license remain in the repository.

Current job: establish repeatable proofs for shared tabs across windows while preserving native scrollback and early input, then decide whether the design meets Worminal's launch and memory goals.
