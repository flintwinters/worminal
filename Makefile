# Worminal is based on st. See LICENSE for copyright and license details.
# GNU make maps the source hierarchy into .checks/obj.

include config.mk

COMMON_SRC = src/terminal/st.c src/terminal/session_view.c \
		src/workspace/wire.c src/workspace/workspace_socket.c
CLIENT_SRC = $(COMMON_SRC) src/workspace/workspace_client.c src/x11/x.c src/x11/url.c
SERVER_SRC = $(COMMON_SRC) src/workspace/workspace_service.c
SRC = $(CLIENT_SRC) src/workspace/workspace_service.c
OBJ = $(patsubst src/%.c,.checks/obj/%.o,$(SRC))
CLIENT_OBJ = $(patsubst src/%.c,.checks/obj/%.o,$(CLIENT_SRC))
SERVER_OBJ = $(patsubst src/%.c,.checks/obj/%.o,$(SERVER_SRC))
CLANG_TIDY = clang-tidy
COMPACT_THEME = tests/fixtures/alacritty/compact.toml
COMPACT_HEADER = .checks/compact_theme.h

all: worminal worminald

.checks/obj/%.o: src/%.c
	mkdir -p $(dir $@)
	$(CC) $(STCFLAGS) -c -o $@ $<

.checks/obj/terminal/st.o: src/config.h .checks/theme.h src/terminal/st.h src/terminal/st_state.h src/terminal/win.h
.checks/obj/x11/x.o: src/arg.h src/config.h src/x11/icon.h .checks/theme.h \
		src/terminal/st.h src/terminal/win.h src/x11/url.h
.checks/obj/x11/url.o: src/x11/url.h src/terminal/st.h
.checks/obj/terminal/session_view.o: src/terminal/session_view.h src/terminal/st_state.h src/terminal/st.h
.checks/obj/workspace/workspace_service.o: .checks/theme.h src/terminal/session_view.h \
		src/workspace/wire.h src/workspace/workspace_socket.h

.checks/theme.h: tools/theme.py
	python3 -c 'from tools.theme import generate_theme; generate_theme()'

$(OBJ): config.mk

worminal: $(CLIENT_OBJ)
	$(CC) -o $@ $(CLIENT_OBJ) $(STLDFLAGS)

worminald: $(SERVER_OBJ)
	$(CC) -o $@ $(SERVER_OBJ) $(STLDFLAGS)

lint: .checks/theme.h
	$(CLANG_TIDY) -quiet $(SRC) -- $(INCS) $(STCPPFLAGS) $(CPPFLAGS) $(CFLAGS)

.checks/key_injector: tests/key_injector.c
	mkdir -p .checks
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11 xtst`

.checks/placement_wm: tests/placement_wm.c
	mkdir -p .checks
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/scrollback_test: tests/scrollback.c src/terminal/st.c src/terminal/st.h \
		src/terminal/win.h src/config.h .checks/theme.h
	mkdir -p .checks
	$(CC) -O1 -g -fsanitize=address,undefined -ffunction-sections -fdata-sections \
		$(STCPPFLAGS) -Wl,--gc-sections -o $@ $< -lm

.checks/url_test: tests/url.c src/x11/url.c src/x11/url.h src/terminal/st.h
	mkdir -p .checks
	$(CC) -O1 -g -fsanitize=address,undefined $(STCPPFLAGS) -o $@ tests/url.c src/x11/url.c

$(COMPACT_HEADER): $(COMPACT_THEME) tools/theme.py
	python3 -c 'from tools.theme import generate_theme; generate_theme("$(COMPACT_THEME)", "$(COMPACT_HEADER)")'

.checks/compact-worminal: $(CLIENT_SRC) src/x11/url.h src/terminal/st.h src/terminal/win.h \
		src/config.h src/x11/icon.h $(COMPACT_HEADER) .checks/worminald
	$(CC) $(STCFLAGS) '-DWORMINAL_THEME_HEADER="$(COMPACT_HEADER)"' -o $@ \
		$(CLIENT_SRC) $(STLDFLAGS)

.checks/worminald: $(SERVER_SRC) $(COMPACT_HEADER)
	$(CC) $(STCFLAGS) '-DWORMINAL_THEME_HEADER="$(COMPACT_HEADER)"' -o $@ \
		$(SERVER_SRC) $(STLDFLAGS)

.checks/overlap_probe: tests/overlap_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/border_probe: tests/border_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/icon_probe: tests/icon_probe.c
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11`

.checks/view_state_test: tests/view_state.c src/x11/x.c src/x11/url.c \
		src/workspace/wire.c src/workspace/workspace_socket.c src/workspace/workspace_client.c \
		src/config.h src/x11/icon.h .checks/theme.h
	$(CC) $(STCFLAGS) -ffunction-sections -fdata-sections -Wl,--gc-sections \
		-o $@ $< src/x11/url.c src/workspace/wire.c src/workspace/workspace_socket.c \
		src/workspace/workspace_client.c $(STLDFLAGS) `$(PKG_CONFIG) --libs xtst`

clean:
	rm -f worminal worminald .checks/theme.h .checks/key_injector .checks/placement_wm \
		.checks/scrollback_test $(COMPACT_HEADER) .checks/compact-worminal .checks/worminald \
		.checks/overlap_probe .checks/border_probe .checks/icon_probe .checks/view_state_test .checks/url_test
	rm -rf .checks/obj

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
