# Worminal is based on st. See LICENSE for copyright and license details.
.POSIX:

include config.mk

SRC = st.c x.c
OBJ = $(SRC:.c=.o)

all: worminal

.c.o:
	$(CC) $(STCFLAGS) -c $<

st.o: config.h .checks/theme.h st.h win.h
x.o: arg.h config.h icon.h .checks/theme.h st.h win.h

.checks/theme.h: tools/theme.py
	python3 -c 'from tools.theme import generate_theme; generate_theme()'

$(OBJ): config.mk

worminal: $(OBJ)
	$(CC) -o $@ $(OBJ) $(STLDFLAGS)

.checks/key_injector: tests/key_injector.c
	mkdir -p .checks
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11 xtst`

.checks/placement_wm: tests/placement_wm.c
	mkdir -p .checks
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/scrollback_test: tests/scrollback.c st.c st.h win.h .checks/theme.h
	mkdir -p .checks
	$(CC) -O1 -g -fsanitize=address,undefined -ffunction-sections -fdata-sections $(STCPPFLAGS) -Wl,--gc-sections -o $@ $< -lm

.checks/compact_theme.h: tests/fixtures/alacritty/compact.toml tools/theme.py
	python3 -c 'from pathlib import Path; from tools.theme import generate_theme; generate_theme(Path("tests/fixtures/alacritty/compact.toml"), Path(".checks/compact_theme.h"))'

.checks/compact-worminal: st.c x.c st.h win.h config.h icon.h .checks/compact_theme.h
	$(CC) $(STCFLAGS) '-DWORMINAL_THEME_HEADER=".checks/compact_theme.h"' -o $@ st.c x.c $(STLDFLAGS)

.checks/overlap_probe: tests/overlap_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/border_probe: tests/border_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/icon_probe: tests/icon_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/view_state_test: tests/view_state.c x.c config.h icon.h .checks/theme.h
	$(CC) $(STCFLAGS) -ffunction-sections -fdata-sections -Wl,--gc-sections -o $@ $< $(STLDFLAGS) `$(PKG_CONFIG) --libs xtst`

clean:
	rm -f worminal $(OBJ) .checks/theme.h .checks/key_injector .checks/placement_wm .checks/scrollback_test .checks/compact_theme.h .checks/compact-worminal .checks/overlap_probe .checks/border_probe .checks/icon_probe .checks/view_state_test

install: worminal
	mkdir -p $(DESTDIR)$(PREFIX)/bin
	cp -f worminal $(DESTDIR)$(PREFIX)/bin/worminal
	chmod 755 $(DESTDIR)$(PREFIX)/bin/worminal
	mkdir -p $(DESTDIR)$(MANPREFIX)/man1
	sed "s/VERSION/$(VERSION)/g" < worminal.1 > $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	chmod 644 $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	mkdir -p $(DESTDIR)$(PREFIX)/share/applications
	cp -f worminal.desktop $(DESTDIR)$(PREFIX)/share/applications/worminal.desktop
	chmod 644 $(DESTDIR)$(PREFIX)/share/applications/worminal.desktop
	mkdir -p $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps
	cp -f worminal.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/worminal.svg
	chmod 644 $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/worminal.svg

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/worminal
	rm -f $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	rm -f $(DESTDIR)$(PREFIX)/share/applications/worminal.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/worminal.svg

.PHONY: all clean install uninstall
