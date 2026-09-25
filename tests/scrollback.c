/* Exercise terminal history without opening an X window. */
#include <assert.h>

#include "../st.c"

unsigned int defaultfg = 258;
unsigned int defaultbg = 259;
unsigned int defaultcs = 257;
unsigned int tabspaces = 8;
wchar_t *worddelimiters = L" ";
int allowaltscreen = 1;
int allowwindowops = 0;
char *vtiden = "\033[?6c";

void xsetmode(int set, unsigned int flags) {}
void xsetpointermotion(int set) {}
void xsettitle(char *title) {}
void xseticontitle(char *title) {}
void xbell(void) {}
int xsetcursor(int cursor) { return 0; }
void xloadcols(void) {}
int xgetcolor(int index, unsigned char *red, unsigned char *green, unsigned char *blue) { return 1; }
int xsetcolorname(int index, const char *name) { return 0; }
void xsetsel(char *selection) {}
void xclipcopy(void) {}

int
main(void)
{
	Arg one = { .i = 1 }, page = { .i = -1 };
	int i, before;

	selinit();
	tnew(4, 3);
	assert(term.hist == NULL);
	for (i = 0; i < 3; i++)
		term.line[i][0].u = 'A' + i;

	tscrollup(0, 1);
	assert(term.histlen == 1 && term.scr == 0);
	kscrollup(&one);
	assert(term.scr == 1 && tline(0)[0].u == 'A');
	assert(tline(1)[0].u == 'B');

	/* New output keeps the viewed row in place. */
	term.line[2][0].u = 'D';
	tscrollup(0, 1);
	assert(term.scr == 2 && tline(0)[0].u == 'A');
	assert(tline(1)[0].u == 'B');

	/* Selection and copy read the viewport, not the live screen. */
	selstart(0, 0, 0);
	selextend(0, 1, SEL_REGULAR, 0);
	selextend(0, 1, SEL_REGULAR, 1);
	char *copy = getsel();
	assert(strcmp(copy, "A\nB") == 0);
	free(copy);

	kscrolldown(&page);
	assert(term.scr == 0 && tline(0)[0].u == 'C');
	kscrollup(&page);
	assert(term.scr == 2);

	/* A scrolling subregion does not become shell scrollback. */
	before = term.histlen;
	tsetscroll(1, 2);
	tscrollup(1, 1);
	assert(term.histlen == before);
	tsetscroll(0, 2);

	/* Alternate-screen output must never enter command history. */
	before = term.histlen;
	tswapscreen();
	assert(term.scr == 0);
	tscrollup(0, 1);
	kscrollup(&one);
	assert(term.histlen == before && term.scr == 0);
	tswapscreen();

	/* Existing hard-broken history stays in scrollback after widening. */
	tresize(6, 3);
	kscrollup(&one);
	assert(tline(0)[0].u == 'B');
	assert(tline(0)[5].u == ' ');
	tresize(6, 2);
	assert(term.scr == 1 && tline(0)[0].u == 'B');
	term.c.y = 1;
	tresize(6, 1);
	assert(term.scr == 2 && tline(0)[0].u == 'B');
	kscrolldown(&page);

	/* The ring stays bounded and retains the most recently evicted row. */
	for (i = 0; i <= theme_history_size; i++) {
		term.line[0][0].u = 'Z';
		tscrollup(0, 1);
	}
	assert(term.histlen == theme_history_size);
	kscrollup(&one);
	assert(tline(0)[0].u == 'Z');

	/* A frozen view keeps its old frame while the shared screen advances.
	 * Activation reads the current screen or retained history independently. */
	term.line[0][0].u = 'X';
	Glyph frozen = tlineat(0, 0)[0];
	tscrollup(0, 1);
	term.line[0][0].u = 'Q';
	assert(frozen.u == 'X');
	assert(tlineat(0, 0)[0].u == 'Q');
	assert(tlineat(0, 1)[0].u == 'X');
	assert(tlineat(0, theme_history_size + 1)[0].u == 'Z');

	/* The focused view may change the shared PTY geometry while a
	 * full-screen program uses the alternate screen. */
	term.scr = 0;
	tswapscreen();
	term.line[0][0].u = 'V';
	tresize(8, 3);
	assert(IS_SET(MODE_ALTSCREEN) && term.col == 8 && term.row == 3);
	assert(term.line[0][0].u == 'V');
	tresize(4, 1);
	assert(IS_SET(MODE_ALTSCREEN) && term.col == 4 && term.row == 1);
	assert(term.line[0][0].u == 'V');
	tswapscreen();
	assert(term.line[0][0].u == 'Q');
	assert(tlineat(0, 1)[0].u == 'X');

	/* Tab parser and history state must survive work on another tab. */
	TermSession *first = session;
	first->ttybuf[0] = 'a';
	first->ttybuflen = 1;
	csiescseq.narg = 1;
	TermSession *second = tsessionnew(3, 2);
	term.line[0][0].u = 'S';
	term.c.x = 2;
	tcursor(CURSOR_SAVE);
	csiescseq.narg = 2;
	assert(term.histlen == 0 && term.line[0][0].u == 'S');
	session = first;
	assert(term.line[0][0].u == 'Q');
	assert(tlineat(0, 1)[0].u == 'X');
	assert(first->ttybuflen == 1 && first->ttybuf[0] == 'a');
	assert(csiescseq.narg == 1);
	session = second;
	assert(term.line[0][0].u == 'S' && term.c.x == 2);
	assert(second->saved[0].x == 2 && csiescseq.narg == 2);

	/* The active view's resize must reach the same PTY as its screen. */
	int master, slave;
	struct winsize size;
	assert(openpty(&master, &slave, NULL, NULL, NULL) == 0);
	cmdfd = master;
	tresize(11, 4);
	ttyresize(110, 40);
	assert(ioctl(slave, TIOCGWINSZ, &size) == 0);
	assert(size.ws_col == 11 && size.ws_row == 4);
	assert(size.ws_xpixel == 110 && size.ws_ypixel == 40);
	close(master);
	close(slave);

	/* Interleaved reads cannot mix the two tabs' UTF-8 parser buffers. */
	int firstmaster, firstslave, secondmaster, secondslave;
	assert(openpty(&firstmaster, &firstslave, NULL, NULL, NULL) == 0);
	assert(openpty(&secondmaster, &secondslave, NULL, NULL, NULL) == 0);
	cmdfd = firstmaster;
	tmoveto(0, 0);
	TermSession *third = tsessionnew(3, 2);
	cmdfd = secondmaster;
	assert(tsessionnext(NULL) == &primarysession &&
	       tsessionnext(&primarysession) == second &&
	       tsessionnext(second) == third);
	assert(tsessionfd(third) == secondmaster);
	assert(write(firstslave, "\xc3", 1) == 1);
	tsessionuse(second);
	assert(ttyread() == 1 && second->ttybuflen == 1);
	assert(write(secondslave, "B", 1) == 1);
	tsessionuse(third);
	assert(ttyread() == 1 && term.line[0][0].u == 'B');
	assert(write(firstslave, "\xa9", 1) == 1);
	tsessionuse(second);
	assert(ttyread() == 1 && term.line[0][0].u == 0xe9);
	assert(second->ttybuflen == 0);
	close(firstmaster);
	close(firstslave);
	close(secondmaster);
	close(secondslave);

	/* Wrapped text must survive both narrower and wider grids, including
	 * the cursor position used by the next character. */
	tsessionnew(4, 3);
	twrite("abcdefghij", 10, 0);
	tresize(5, 3);
	assert(term.hist == NULL);
	assert(term.line[0][0].u == 'a' && term.line[0][4].u == 'e');
	assert(term.line[1][0].u == 'f' && term.line[1][4].u == 'j');
	twrite("k", 1, 0);
	assert(term.line[2][0].u == 'k');
	tresize(3, 3);
	assert(term.histlen == 1 && term.hist[0][0].u == 'a');
	assert(term.line[0][0].u == 'd' && term.line[0][2].u == 'f');
	assert(term.line[1][0].u == 'g' && term.line[1][2].u == 'i');
	assert(term.line[2][0].u == 'j' && term.line[2][1].u == 'k');
	tresize(6, 3);
	assert(term.line[0][0].u == 'a' && term.line[0][5].u == 'f');
	assert(term.line[1][0].u == 'g' && term.line[1][4].u == 'k');

	/* Hard breaks stay hard, and a wide glyph stays whole at a new edge. */
	tsessionnew(4, 3);
	twrite("ab\r\ncd", 6, 0);
	tresize(6, 3);
	assert(term.line[0][0].u == 'a' && term.line[0][2].u == ' ');
	assert(term.line[1][0].u == 'c');
	tsessionnew(4, 2);
	term.line[0][0].u = 'A';
	term.line[0][1] = (Glyph){ .u = 0x754c, .mode = ATTR_WIDE };
	term.line[0][2] = (Glyph){ .mode = ATTR_WDUMMY };
	term.line[0][3].u = 'B';
	term.c.x = 3;
	tresize(3, 2);
	assert(term.line[0][1].u == 0x754c &&
	       term.line[0][2].mode & ATTR_WDUMMY);
	assert(term.line[1][0].u == 'B' && term.c.y == 1);
	tresize(1, 4);
	tresize(3, 4);
	assert(term.line[0][1].u == 0x754c &&
	       term.line[0][1].mode & ATTR_WIDE &&
	       term.line[0][2].mode & ATTR_WDUMMY);

	/* st marks the wide lead cell, rather than its dummy, at a soft wrap. */
	tsessionnew(4, 2);
	term.line[0][0].u = 'A';
	term.line[0][1].u = 'B';
	term.line[0][2] = (Glyph){ .u = 0x754c,
	                            .mode = ATTR_WIDE | ATTR_WRAP };
	term.line[0][3] = (Glyph){ .mode = ATTR_WDUMMY };
	term.line[1][0].u = 'C';
	term.c.x = 1;
	term.c.y = 1;
	tresize(5, 2);
	assert(term.line[0][4].u == 'C');
	assert(term.line[0][2].mode & ATTR_WIDE);
	assert(term.line[0][3].mode & ATTR_WDUMMY);

	/* A scrolled viewport keeps its place as wraps change. */
	tsessionnew(4, 3);
	twrite("abcdefghijklmnop", 16, 0);
	kscrollup(&one);
	assert(term.scr == 1 && tline(0)[0].u == 'a');
	tresize(5, 3);
	assert(term.scr == 1 && tline(0)[0].u == 'a');
	assert(term.histlen == 1);

	/* A wrap into an empty cursor row still leaves room for the next input. */
	tsessionnew(4, 3);
	twrite("abcd", 4, 0);
	term.line[0][3].mode |= ATTR_WRAP;
	term.c.x = 0;
	term.c.y = 1;
	term.c.state &= ~CURSOR_WRAPNEXT;
	tresize(2, 3);
	assert(term.c.x == 0 && term.c.y == 2);
	twrite("e", 1, 0);
	assert(term.line[2][0].u == 'e');

	/* Expansion of a full history ring still keeps only the configured limit. */
	tsessionnew(4, 2);
	for (i = 0; i <= theme_history_size; i++) {
		term.line[0][0].u = 'q';
		term.line[0][1].u = 'r';
		term.line[0][2].u = 's';
		term.line[0][3].u = 't';
		term.line[0][3].mode |= ATTR_WRAP;
		tscrollup(0, 1);
	}
	tresize(2, 2);
	assert(term.histlen == theme_history_size);
	assert(term.hist[(term.histi - 1 + theme_history_size) % theme_history_size][0].u == 's');
	return 0;
}
