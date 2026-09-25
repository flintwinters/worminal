#ifndef WORMINAL_SESSION_VIEW_H
#define WORMINAL_SESSION_VIEW_H

#include "st.h"

typedef struct {
	int cols, rows;
	int cursor_x, cursor_y;
	int scroll, alternate;
} SessionFrame;

SessionFrame tsessionframe(TermSession *);
Line tsessionviewline(TermSession *, int);
int tsessionrowdirty(TermSession *, int);
void tsessioncleandirty(TermSession *);
void tsessionimportframe(TermSession *, const SessionFrame *);
void tsessionimportrow(TermSession *, int, const Glyph *, int);

#endif
