/* Exercise terminal history without opening an X window. */
#include <assert.h>

#include "../st.c"

unsigned int defaultfg = 258;
unsigned int defaultbg = 259;
unsigned int tabspaces = 8;
wchar_t *worddelimiters = L" ";

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

	/* Existing history survives a width change with blank new cells. */
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
	TermSession second = {0};
	first->ttybuf[0] = 'a';
	first->ttybuflen = 1;
	csiescseq.narg = 1;
	session = &second;
	iofd = 1;
	selinit();
	tnew(3, 2);
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
	session = &second;
	assert(term.line[0][0].u == 'S' && term.c.x == 2);
	assert(second.saved[0].x == 2 && csiescseq.narg == 2);
	return 0;
}
