/* Private terminal state shared by the parser and the session transport. */
#ifndef WORMINAL_ST_STATE_H
#define WORMINAL_ST_STATE_H

#include <stdio.h>
#include "st.h"

#define UTF_SIZ 4
#define ESC_BUF_SIZ (128 * UTF_SIZ)
#define ESC_ARG_SIZ 16
#define STR_BUF_SIZ ESC_BUF_SIZ
#define STR_ARG_SIZ ESC_ARG_SIZ

enum term_mode {
	MODE_WRAP = 1 << 0,
	MODE_INSERT = 1 << 1,
	MODE_ALTSCREEN = 1 << 2,
	MODE_CRLF = 1 << 3,
	MODE_ECHO = 1 << 4,
	MODE_PRINT = 1 << 5,
	MODE_UTF8 = 1 << 6,
};

typedef struct {
	Glyph attr;
	int x, y;
	char state;
} TCursor;

typedef struct {
	int mode, type, snap;
	/* Normalized begin/end, followed by original begin/end. */
	struct { int x, y; } nb, ne, ob, oe;
	int alt;
} Selection;

typedef struct {
	int row, col;
	Line *line, *alt, *hist;
	int histlen, histi, scr;
	int *dirty;
	TCursor c;
	int ocx, ocy, top, bot, mode, esc;
	char trantbl[4];
	int charset, icharset;
	int *tabs;
	Rune lastc;
} Term;

typedef struct {
	char buf[ESC_BUF_SIZ];
	size_t len;
	char priv;
	int arg[ESC_ARG_SIZ], narg;
	char mode[2];
} CSIEscape;

typedef struct {
	char type;
	char *buf;
	size_t siz, len;
	char *args[STR_ARG_SIZ];
	int narg;
} STREscape;

struct TermSession {
	Term term;
	Selection sel;
	CSIEscape csiescseq;
	STREscape strescseq;
	int output_fd, pty_fd;
	pid_t child_pid;
	int altscreen_allowed;
	char ttybuf[BUFSIZ];
	int ttybuflen;
	int remote_scroll;
	TCursor saved[2];
	struct TermSession *next;
};

Line tlineat(int, int);

#endif
