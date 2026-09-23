# Upstream st version and local build settings.
VERSION = 0.9.3
PREFIX = /usr/local
MANPREFIX = $(PREFIX)/share/man
PKG_CONFIG = pkg-config

INCS = `$(PKG_CONFIG) --cflags x11 xft fontconfig freetype2`
LIBS = -lm -lrt -lutil `$(PKG_CONFIG) --libs x11 xft fontconfig freetype2`

# The shipped build is optimized for startup and runtime performance.
CFLAGS = -O3
STCPPFLAGS = -DVERSION=\"$(VERSION)\" -D_XOPEN_SOURCE=600
STCFLAGS = $(INCS) $(STCPPFLAGS) $(CPPFLAGS) $(CFLAGS)
STLDFLAGS = $(LIBS) $(LDFLAGS)
