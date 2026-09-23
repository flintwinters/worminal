# Worminal is based on st. See LICENSE for copyright and license details.
.POSIX:

include config.mk

SRC = st.c x.c
OBJ = $(SRC:.c=.o)

all: worminal

.c.o:
	$(CC) $(STCFLAGS) -c $<

st.o: config.h .checks/theme.h st.h win.h
x.o: arg.h config.h .checks/theme.h st.h win.h

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

clean:
	rm -f worminal $(OBJ) .checks/theme.h .checks/key_injector .checks/placement_wm .checks/scrollback_test

install: worminal
	mkdir -p $(DESTDIR)$(PREFIX)/bin
	cp -f worminal $(DESTDIR)$(PREFIX)/bin/worminal
	chmod 755 $(DESTDIR)$(PREFIX)/bin/worminal
	mkdir -p $(DESTDIR)$(MANPREFIX)/man1
	sed "s/VERSION/$(VERSION)/g" < worminal.1 > $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	chmod 644 $(DESTDIR)$(MANPREFIX)/man1/worminal.1
	tic -sx st.info

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/worminal
	rm -f $(DESTDIR)$(MANPREFIX)/man1/worminal.1

.PHONY: all clean install uninstall
