/* Imported frames are already reflowed by the owner; replicas keep one viewport. */
#include "../src/terminal/session_view.h"

static void
check_session_view(void)
{
	TermSession *previous = tsessioncurrent(), *view = tsessionviewnew(8, 3);
	SessionFrame frame = {8, 3, 7, 2, 0, 0};
	tsessionimportframe(view, &frame);
	for (int y = 0; y < 3; y++) {
		Glyph row[8];
		for (int x = 0; x < 8; x++) row[x] = (Glyph){.u = 'A' + y, .fg = defaultfg, .bg = defaultbg};
		row[7].mode = ATTR_WRAP;
		tsessionimportrow(view, y, row, 8);
	}
	Glyph saved[3][8];
	for (int y = 0; y < 3; y++) memcpy(saved[y], view->term.line[y], sizeof(saved[y]));
	tsessioncleandirty(view);
	tsessioncopyrows(view, 1, 0, 2);
	assert(view->term.line[0][0].u == 'B' && view->term.line[1][0].u == 'C');
	assert(view->term.line[0][7].mode & ATTR_WRAP);
	assert(tsessionrowdirty(view, 0) && tsessionrowdirty(view, 1) && !tsessionrowdirty(view, 2));
	tsessioncopyrows(view, 0, 1, 2);
	assert(view->term.line[1][0].u == 'B' && view->term.line[2][0].u == 'C');
	for (int y = 0; y < 3; y++) tsessionimportrow(view, y, saved[y], 8);
	frame = (SessionFrame){4, 3, 3, 2, 2, 0};
	tsessionimportframe(view, &frame);
	assert(view->term.histlen == 0 && view->term.hist == NULL);
	assert(view->term.line[0][0].u == 'A' && view->term.line[2][0].u == 'C');
	assert(view->remote_scroll == 2 && view->term.scr == 0);
	assert(view->term.alt == NULL && view->term.tabs == NULL && view->view_only);
	frame = (SessionFrame){9, 5, 8, 4, 2, 0};
	tsessionimportframe(view, &frame);
	assert(view->term.line[0][0].u == 'A');
	for (int y = 0; y < 5; y++)
		for (int x = y < 3 ? 4 : 0; x < 9; x++) {
			Glyph cell = view->term.line[y][x];
			assert(cell.u == ' ' && cell.mode == 0 && cell.fg == defaultfg && cell.bg == defaultbg);
		}
	Glyph words[9];
	for (int x = 0; x < 9; x++) words[x] = (Glyph){.u = "one two  "[x], .fg = defaultfg, .bg = defaultbg};
	tsessioncleandirty(view);
	tsessionimportrow(view, 0, words, 9);
	assert(tsessionrowdirty(view, 0) && !tsessionrowdirty(view, 1));
	selstart(0, 0, SNAP_WORD);
	char *copy = getsel();
	assert(copy && strcmp(copy, "one") == 0);
	free(copy);
	frame.rows = 7;
	tsessionimportframe(view, &frame);
	copy = getsel();
	assert(copy && strcmp(copy, "one") == 0);
	free(copy);
	frame.alternate = 1; frame.scroll = 0;
	tsessionimportframe(view, &frame);
	assert(!selected(0, 0));
	selstart(0, 0, SNAP_WORD);
	frame.cols = 11; frame.cursor_x = 10;
	tsessionimportframe(view, &frame);
	assert(selected(0, 0));
	frame.alternate = 0;
	tsessionimportframe(view, &frame);
	assert(!selected(0, 0));
	int fd = memfd_create("worminal-view-test", MFD_CLOEXEC);
	assert(fd >= 0 && !tsessionexport(view, fd) && lseek(fd, 0, SEEK_CUR) == 0);
	close(fd);
	/* A clipped wide glyph cannot leave an orphaned half-cell behind. */
	view->term.line[1][2] = (Glyph){.u = 0x4e2d, .mode = ATTR_WIDE, .fg = defaultfg, .bg = defaultbg};
	view->term.line[1][3] = (Glyph){.mode = ATTR_WDUMMY, .fg = defaultfg, .bg = defaultbg};
	frame.cols = 3; frame.cursor_x = 2;
	tsessionimportframe(view, &frame);
	assert(view->term.line[1][2].u == ' ' && !(view->term.line[1][2].mode & ATTR_WIDE));
	frame.cols = 4;
	tsessionimportframe(view, &frame);
	Glyph pair[4];
	for (int x = 0; x < 4; x++) pair[x] = (Glyph){.u = ' ', .fg = defaultfg, .bg = defaultbg};
	pair[2] = (Glyph){.u = 0x4e2d, .mode = ATTR_WIDE, .fg = defaultfg, .bg = defaultbg};
	pair[3] = (Glyph){.mode = ATTR_WDUMMY, .fg = defaultfg, .bg = defaultbg};
	tsessionimportrow(view, 1, pair, 4);
	selstart(2, 1, 0);
	selextend(3, 1, SEL_REGULAR, 0); selextend(3, 1, SEL_REGULAR, 1);
	copy = getsel();
	assert(copy && strcmp(copy, "\xe4\xb8\xad") == 0);
	free(copy);
	TermSession *other = tsessionviewnew(2, 2);
	for (int cycle = 0; cycle < 100; cycle++) {
		frame = (SessionFrame){cycle & 1 ? 400 : 1, cycle & 1 ? 200 : 1, 0, 0, cycle, cycle & 1};
		tsessionimportframe(view, &frame);
		assert(tsessioncurrent() == other);
		assert(view->term.alt == NULL && view->term.hist == NULL && view->term.tabs == NULL);
		assert(view->term.histlen == 0 && view->remote_scroll == cycle);
	}
	tsessionremove(other);
	tsessionremove(view);
	tsessionuse(previous);
}
