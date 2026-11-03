/* Private parser operations. Public consumers use engine.h. */
#ifndef WORMINAL_ENGINE_PARSER_H
#define WORMINAL_ENGINE_PARSER_H
#include "engine_state.h"
#include "win.h"
void te_selinit(TermEngine *session);
void te_selstart(TermEngine *session, int col, int row, int snap);
void te_selextend(TermEngine *session, int col, int row, int type, int done);
int te_selected(TermEngine *session, int x, int y);
char * te_getsel(TermEngine *session);
void te_selclear(TermEngine *session);
int te_tattrset(TermEngine *session, int attr);
void te_tsetdirt(TermEngine *session, int top, int bot);
void te_tsetdirtattr(TermEngine *session, int attr);
void te_tfulldirt(TermEngine *session);
void te_tcursor(TermEngine *session, int mode);
void te_treset(TermEngine *session);
void te_tnew(TermEngine *session, int col, int row);
Line te_tline(TermEngine *session, int y);
Line te_tlineat(TermEngine *session, int y, int scroll);
void te_kscrollup(TermEngine *session, const Arg *arg);
void te_kscrolldown(TermEngine *session, const Arg *arg);
void te_tswapscreen(TermEngine *session);
int te_tisaltscr(TermEngine *session);
void te_tscrollup(TermEngine *session, int orig, int n);
void te_tmoveto(TermEngine *session, int x, int y);
void te_tsetscroll(TermEngine *session, int t, int b);
void te_toggleprinter(TermEngine *session, const Arg *arg);
void te_printscreen(TermEngine *session, const Arg *arg);
void te_printsel(TermEngine *session, const Arg *arg);
int te_twrite(TermEngine *session, const char *buf, int buflen, int show_ctrl);
void te_tresize(TermEngine *session, int col, int row);
void te_resettitle(TermEngine *session);
void te_xsetmode(TermEngine *, int, unsigned int);
void te_xsetpointermotion(TermEngine *, int);
void te_xsettitle(TermEngine *, char *);
void te_xseticontitle(TermEngine *, char *);
void te_xbell(TermEngine *);
int te_xsetcursor(TermEngine *, int);
void te_xloadcols(TermEngine *);
int te_xsetcolorname(TermEngine *, int, const char *);
int te_xgetcolor(TermEngine *, int, unsigned char *, unsigned char *, unsigned char *);
void te_xsetsel(TermEngine *, char *);
void te_xclipcopy(TermEngine *);
void te_ttywrite(TermEngine *, const char *, size_t, int);
void te_tprinter(TermEngine *, char *, size_t);

/* Arbitrary sizes */
#define UTF_INVALID   0xFFFD

/* macros */
#define IS_SET(flag)		((term.mode & (flag)) != 0)
#define ISCONTROLC0(c)		(BETWEEN(c, 0, 0x1f) || (c) == 0x7f)
#define ISCONTROLC1(c)		(BETWEEN(c, 0x80, 0x9f))
#define ISCONTROL(c)		(ISCONTROLC0(c) || ISCONTROLC1(c))
#define ISDELIM(u)		(u && wcschr(worddelimiters, u))

enum cursor_movement {
	CURSOR_SAVE,
	CURSOR_LOAD
};

enum cursor_state {
	CURSOR_DEFAULT  = 0,
	CURSOR_WRAPNEXT = 1,
	CURSOR_ORIGIN   = 2
};

enum charset {
	CS_GRAPHIC0,
	CS_GRAPHIC1,
	CS_UK,
	CS_USA,
	CS_MULTI,
	CS_GER,
	CS_FIN
};

enum escape_state {
	ESC_START      = 1,
	ESC_CSI        = 2,
	ESC_STR        = 4,  /* DCS, OSC, PM, APC */
	ESC_ALTCHARSET = 8,
	ESC_STR_END    = 16, /* a final string was encountered */
	ESC_TEST       = 32, /* Enter in test mode */
	ESC_UTF8       = 64,
};


#endif
