# Worminal is based on st. See LICENSE for copyright and license details.
# GNU make maps the source hierarchy into build/obj.

include config.mk

COMMON_SRC = src/terminal/st.c src/terminal/session_view.c \
		src/workspace/wire.c src/workspace/workspace_socket.c
CLIENT_SRC = $(COMMON_SRC) src/workspace/workspace_client.c src/x11/x.c src/x11/url.c
SERVER_SRC = $(COMMON_SRC) src/workspace/workspace_service.c
SRC = $(CLIENT_SRC) src/workspace/workspace_service.c
OBJ = $(patsubst src/%.c,build/obj/%.o,$(SRC))
CLIENT_OBJ = $(patsubst src/%.c,build/obj/%.o,$(CLIENT_SRC))
SERVER_OBJ = $(patsubst src/%.c,build/obj/%.o,$(SERVER_SRC))
CLANG_TIDY = clang-tidy
COMPACT_THEME = tests/fixtures/alacritty/compact.toml
COMPACT_HEADER = build/compact_theme.h

all: worminal worminald

build/obj/%.o: src/%.c
	mkdir -p $(dir $@)
	$(CC) $(STCFLAGS) -c -o $@ $<

build/obj/terminal/st.o: src/config.h build/theme.h src/terminal/st.h src/terminal/st_state.h src/terminal/win.h
build/obj/x11/x.o: src/arg.h src/config.h src/x11/icon.h build/theme.h \
		src/terminal/st.h src/terminal/win.h src/x11/url.h
build/obj/x11/url.o: src/x11/url.h src/terminal/st.h
build/obj/terminal/session_view.o: src/terminal/session_view.h src/terminal/st_state.h src/terminal/st.h
build/obj/workspace/workspace_service.o: build/theme.h src/terminal/session_view.h \
		src/workspace/wire.h src/workspace/workspace_socket.h

build/theme.h: tools/theme.py
	python3 -c 'from tools.theme import generate_theme; generate_theme()'

$(OBJ): config.mk

worminal: $(CLIENT_OBJ)
	$(CC) -o $@ $(CLIENT_OBJ) $(STLDFLAGS)

worminald: $(SERVER_OBJ)
	$(CC) -o $@ $(SERVER_OBJ) $(STLDFLAGS)

lint: build/theme.h
	$(CLANG_TIDY) -quiet $(SRC) -- $(INCS) $(STCPPFLAGS) $(CPPFLAGS) $(CFLAGS)

build/key_injector: tests/key_injector.c
	mkdir -p build
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11 xtst`

build/placement_wm: tests/placement_wm.c
	mkdir -p build
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

build/scrollback_test: tests/scrollback.c src/terminal/st.c src/terminal/st.h \
		src/terminal/win.h src/config.h build/theme.h
	mkdir -p build
	$(CC) -O1 -g -fsanitize=address,undefined -ffunction-sections -fdata-sections \
		$(STCPPFLAGS) -Wl,--gc-sections -o $@ $< -lm

build/url_test: tests/url.c src/x11/url.c src/x11/url.h src/terminal/st.h
	mkdir -p build
	$(CC) -O1 -g -fsanitize=address,undefined $(STCPPFLAGS) -o $@ tests/url.c src/x11/url.c

$(COMPACT_HEADER): $(COMPACT_THEME) tools/theme.py
	python3 -c 'from tools.theme import generate_theme; generate_theme("$(COMPACT_THEME)", "$(COMPACT_HEADER)")'

build/compact-worminal: $(CLIENT_SRC) src/x11/url.h src/terminal/st.h src/terminal/win.h \
		src/config.h src/x11/icon.h $(COMPACT_HEADER) build/worminald
	$(CC) $(STCFLAGS) '-DWORMINAL_THEME_HEADER="$(COMPACT_HEADER)"' -o $@ \
		$(CLIENT_SRC) $(STLDFLAGS)

build/worminald: $(SERVER_SRC) $(COMPACT_HEADER)
	$(CC) $(STCFLAGS) '-DWORMINAL_THEME_HEADER="$(COMPACT_HEADER)"' -o $@ \
		$(SERVER_SRC) $(STLDFLAGS)

build/overlap_probe: tests/overlap_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

build/border_probe: tests/border_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

build/icon_probe: tests/icon_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

build/view_state_test: tests/view_state.c src/x11/x.c src/x11/url.c \
		src/workspace/wire.c src/workspace/workspace_socket.c src/workspace/workspace_client.c \
		src/config.h src/x11/icon.h build/theme.h
	$(CC) $(STCFLAGS) -ffunction-sections -fdata-sections -Wl,--gc-sections \
		-o $@ $< src/x11/url.c src/workspace/wire.c src/workspace/workspace_socket.c \
		src/workspace/workspace_client.c $(STLDFLAGS) `$(PKG_CONFIG) --libs xtst`

clean:
	rm -f worminal worminald build/theme.h build/key_injector build/placement_wm \
		build/scrollback_test $(COMPACT_HEADER) build/compact-worminal build/worminald \
		build/overlap_probe build/border_probe build/icon_probe build/view_state_test build/url_test
	rm -rf build/obj

install: worminal worminald
	mkdir -p $(DESTDIR)$(PREFIX)/bin
	cp -f worminal $(DESTDIR)$(PREFIX)/bin/worminal
	chmod 755 $(DESTDIR)$(PREFIX)/bin/worminal
	cp -f worminald $(DESTDIR)$(PREFIX)/bin/worminald
	chmod 755 $(DESTDIR)$(PREFIX)/bin/worminald
	mkdir -p $(DESTDIR)$(MANPREFIX)/man1
	sed "s/VERSION/$(VERSION)/g" < docs/man/worminal.1 > $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	chmod 644 $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	mkdir -p $(DESTDIR)$(PREFIX)/share/applications
	cp -f assets/worminal.desktop $(DESTDIR)$(PREFIX)/share/applications/worminal.desktop
	chmod 644 $(DESTDIR)$(PREFIX)/share/applications/worminal.desktop
	mkdir -p $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps
	cp -f assets/worminal.svg $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/worminal.svg
	chmod 644 $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/worminal.svg

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/worminal
	rm -f $(DESTDIR)$(PREFIX)/bin/worminald
	rm -f $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	rm -f $(DESTDIR)$(PREFIX)/share/applications/worminal.desktop
	rm -f $(DESTDIR)$(PREFIX)/share/icons/hicolor/scalable/apps/worminal.svg

.PHONY: all clean install lint uninstall
