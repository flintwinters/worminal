# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface features;
measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. A per-user `worminald` service owns ordered tabs, PTYs, terminal state,
and bounded main-screen scrollback; X11 processes connect locally or through `--master user@host` over SSH. A tab has
one live painting view and can run with no windows. Each window maps early, then loads fonts while the service starts
its shell. `manage.py build` compiles Alacritty colors, font size, line spacing, and scrollback into a generated C
header. `src/terminal` owns terminal state, `src/x11` paints views, and `src/workspace` connects views to the service.
Compact rows repaint backgrounds before glyphs. `config.mk` selects `-O3`; `manage.py` checks and measures latency
on private Xvfb. The upstream source and license remain in the repository.

Current job: measure service and window memory, then compare launch and memory against the earlier in-process owner.
