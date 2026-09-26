#ifndef WORMINAL_URL_H
#define WORMINAL_URL_H

#include "st.h"

typedef struct {
	int start_x, start_y, end_x, end_y;
} UrlSpan;

/* Return an owned HTTP(S) URL under a visible cell and its painted span. */
char *urlat(Line *lines, int rows, int cols, int x, int y, UrlSpan *span);
void urlopen(const char *url);

#endif
