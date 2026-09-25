# Worminal is based on st. See LICENSE for copyright and license details.
.POSIX:

include config.mk

SRC = st.c x.c session_view.c wire.c workspace_socket.c workspace_client.c workspace_service.c
COMMON_OBJ = st.o session_view.o wire.o workspace_socket.o
OBJ = $(SRC:.c=.o)
CLANG_TIDY = clang-tidy
COMPACT_THEME = tests/fixtures/alacritty/compact.toml
COMPACT_HEADER = .checks/compact_theme.h

all: worminal worminald

.c.o:
	$(CC) $(STCFLAGS) -c $<

st.o: config.h .checks/theme.h st.h st_state.h win.h
x.o: arg.h config.h icon.h .checks/theme.h st.h win.h
session_view.o: session_view.h st_state.h st.h
workspace_service.o: .checks/theme.h session_view.h wire.h workspace_socket.h

.checks/theme.h: tools/theme.py
	python3 -c 'from tools.theme import generate_theme; generate_theme()'

$(OBJ): config.mk

worminal: $(COMMON_OBJ) workspace_client.o x.o
	$(CC) -o $@ $(COMMON_OBJ) workspace_client.o x.o $(STLDFLAGS)

worminald: $(COMMON_OBJ) workspace_service.o
	$(CC) -o $@ $(COMMON_OBJ) workspace_service.o $(STLDFLAGS)

lint: .checks/theme.h
	$(CLANG_TIDY) -quiet $(SRC) -- $(INCS) $(STCPPFLAGS) $(CPPFLAGS) $(CFLAGS)

.checks/key_injector: tests/key_injector.c
	mkdir -p .checks
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11 xtst`

.checks/placement_wm: tests/placement_wm.c
	mkdir -p .checks
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/scrollback_test: tests/scrollback.c st.c st.h win.h .checks/theme.h
	mkdir -p .checks
	$(CC) -O1 -g -fsanitize=address,undefined -ffunction-sections -fdata-sections \
		$(STCPPFLAGS) -Wl,--gc-sections -o $@ $< -lm

$(COMPACT_HEADER): $(COMPACT_THEME) tools/theme.py
	python3 -c 'from tools.theme import generate_theme; generate_theme("$(COMPACT_THEME)", "$(COMPACT_HEADER)")'

.checks/compact-worminal: st.c x.c session_view.c wire.c workspace_socket.c workspace_client.c \
		st.h win.h config.h icon.h $(COMPACT_HEADER) .checks/worminald
	$(CC) $(STCFLAGS) '-DWORMINAL_THEME_HEADER="$(COMPACT_HEADER)"' -o $@ \
		st.c x.c session_view.c wire.c workspace_socket.c workspace_client.c $(STLDFLAGS)

.checks/worminald: st.c session_view.c wire.c workspace_socket.c workspace_service.c $(COMPACT_HEADER)
	$(CC) $(STCFLAGS) '-DWORMINAL_THEME_HEADER="$(COMPACT_HEADER)"' -o $@ \
		st.c session_view.c wire.c workspace_socket.c workspace_service.c $(STLDFLAGS)

.checks/overlap_probe: tests/overlap_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/border_probe: tests/border_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/icon_probe: tests/icon_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/view_state_test: tests/view_state.c x.c wire.c workspace_socket.c workspace_client.c \
		config.h icon.h .checks/theme.h
	$(CC) $(STCFLAGS) -ffunction-sections -fdata-sections -Wl,--gc-sections \
		-o $@ $< wire.c workspace_socket.c workspace_client.c $(STLDFLAGS) `$(PKG_CONFIG) --libs xtst`

clean:
	rm -f worminal worminald $(OBJ) .checks/theme.h .checks/key_injector .checks/placement_wm \
		.checks/scrollback_test $(COMPACT_HEADER) .checks/compact-worminal .checks/worminald \
		.checks/overlap_probe .checks/border_probe .checks/icon_probe .checks/view_state_test

install: worminal worminald
	mkdir -p $(DESTDIR)$(PREFIX)/bin
	cp -f worminal $(DESTDIR)$(PREFIX)/bin/worminal
	chmod 755 $(DESTDIR)$(PREFIX)/bin/worminal
	cp -f worminald $(DESTDIR)$(PREFIX)/bin/worminald
	chmod 755 $(DESTDIR)$(PREFIX)/bin/worminald
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
	rm -f $(DESTDIR)$(PREFIX)/bin/worminald
	rm -f $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	rm -f $(DESTDIR)$(PREFIX)/share/applications/worminal.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/worminal.svg

.PHONY: all clean install lint uninstall
