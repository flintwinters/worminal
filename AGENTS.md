# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface features; measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. Its C source handles windows, terminal state, PTYs, and bounded main-screen scrollback directly. It maps a provisional input-capable window early, lets the window manager place it, and starts the shell while fonts load. `manage.py build` resolves Alacritty TOML colors, font family, size, line spacing, and scrollback settings into a generated C header, so launch reads no config files. Compact rows repaint backgrounds before glyphs so descenders can overlap adjacent rows. One owner serves windows and ordered tabs per executable build, user, and X11 display; older owners keep their tabs after a rebuild until their windows close. Each tab has one live painting view and may have no selected window. `config.mk` selects `-O3`, and `manage.py` checks and measures latency on private Xvfb. The upstream source and license remain in the repository.

Current job: measure launch and memory after the completed shared-tab lifecycle, and use the `manage.py proof` tree on private Xvfb, including remote Cinnamon, while desktops are in use.
