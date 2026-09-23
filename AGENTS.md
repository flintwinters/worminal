# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface features; measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. Its C source handles the window, terminal state, and PTY directly. It maps a provisional input-capable window early, lets the window manager place it, and starts the shell while fonts load. `config.h` holds non-theme settings; `manage.py build` resolves Alacritty TOML colors and imports into a generated C header, so launch reads no theme files. `config.mk` selects `-O3`, and `manage.py` checks and measures latency on a private Xvfb display. The upstream source and license remain in the repository.

Current job: verify theme appearance, early focus, and input capture on a real desktop against the repeatable Xvfb probes, then measure memory and address remaining startup costs.
