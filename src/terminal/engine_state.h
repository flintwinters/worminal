/* Terminal-owned state: no descriptors, children, windows, or active-instance globals. */
#ifndef WORMINAL_ENGINE_STATE_H
#define WORMINAL_ENGINE_STATE_H
#include <stdio.h>
#include "st.h"
#include "engine.h"
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


typedef struct { Line *lines; size_t cap, count; int col, x; } Reflow;
typedef struct EngineEventNode EngineEventNode;
struct WorminalEngine {
	Term term;
	Selection sel;
	CSIEscape csiescseq;
	STREscape strescseq;
	int altscreen_allowed;
	/* OSC 8 URI table indexed by Glyph.link; slot 0 and reclaimed slots are NULL. */
	char **links;
	uint16_t link_count;
	size_t link_bytes;
	int64_t now, render_sync_deadline;
	int render_sync_expired;
	TCursor saved[2];
	EngineEventNode *events, *events_tail;
	/* Only stack-scoped selection contexts borrow caller cells; parser instances keep these zero. */
	const WorminalCell *selection_cells;
	int selection_only;
};
typedef struct WorminalEngine TermEngine;
void *te_alloc(size_t);
void *te_realloc(void *, size_t);
char *te_strdup(const char *);
void te_fail(const char *, ...) __attribute__((noreturn));
void te_setsync(TermEngine *, int);
int64_t te_renderdelay(TermEngine *, int64_t);
void te_freegrid(TermEngine *);
void te_freeevents(TermEngine *);
void te_links_free(TermEngine *);
void te_diagnostic(TermEngine *, const char *, ...);
void te_emit(TermEngine *, uint32_t, int32_t, uint32_t, const void *, size_t);
int te_dimensions(int32_t, int32_t);
#endif
