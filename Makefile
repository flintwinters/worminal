# Worminal is based on st. See LICENSE for copyright and license details.
.POSIX:

include config.mk

SRC = st.c x.c
OBJ = $(SRC:.c=.o)

all: worminal

.c.o:
	$(CC) $(STCFLAGS) -c $<

st.o: config.h st.h win.h
x.o: arg.h config.h st.h win.h

$(OBJ): config.mk

worminal: $(OBJ)
	$(CC) -o $@ $(OBJ) $(STLDFLAGS)

.checks/key_injector: tests/key_injector.c
	mkdir -p .checks
	$(CC) -O2 -o $@ $< `$(PKG_CONFIG) --cflags --libs x11 xtst`

clean:
	rm -f worminal $(OBJ) .checks/key_injector

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
