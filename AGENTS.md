# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface features; measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. Its C source handles the window, terminal state, and PTY directly. It creates an unmapped window and starts the shell while fonts load, then maps the finished window. `config.h` is the single configuration source, `config.mk` selects the default `-O3` build, and `manage.py` checks and measures latency on a private Xvfb display. The upstream source and license remain in the repository.

Current job: verify launch and input on a real desktop against the repeatable Xvfb probes, measure memory, then address the remaining font and shell startup costs.
