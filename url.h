#ifndef WORMINAL_URL_H
#define WORMINAL_URL_H

#include "st.h"

/* Return an owned HTTP(S) URL under a visible cell, or NULL. */
char *urlat(Line *lines, int rows, int cols, int x, int y);
void urlopen(const char *url);

#endif
