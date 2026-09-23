# Worminal

Worminal is a deliberately small terminal. Favor cold launch to usable input and low memory over interface features; measure both before adding complexity.

The app is an X11-native fork of st 0.9.3. Its C source handles the window, terminal state, and PTY directly. `config.h` is the single configuration source, `config.mk` selects the default `-O3` build, and `manage.py` is the root build and check entrypoint. The upstream source and license remain in the repository.

Current job: verify launch and keyboard input on a real desktop, measure cold startup and proportional memory, then address the largest observed cost.
