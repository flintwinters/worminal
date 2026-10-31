/* See LICENSE for license details. */
#ifndef WORMINAL_WIN_H
#define WORMINAL_WIN_H

#include "engine.h"

enum draw_pass {
	DRAW_BACKGROUND = 1,
	DRAW_FOREGROUND = 2,
};

void xbell(void);
void xclipcopy(void);
void xdrawcursor(int, int, Glyph, int, int, Glyph);
void xdrawline(Line, int, int, int, int);
void xfinishdraw(void);
void xloadcols(void);
int xsetcolorname(int, const char *);
int xgetcolor(int, unsigned char *, unsigned char *, unsigned char *);
void xseticontitle(char *);
void xsettitle(char *);
int xsetcursor(int);
void xsetmode(int, unsigned int);
void xsetpointermotion(int);
void xsetsel(char *);
int xstartdraw(void);
int xoverlap(void);
void xximspot(int, int);
#endif
