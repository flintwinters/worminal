/* See LICENSE. Shared parser; every stateful operation receives its terminal explicitly. */

static void
te_tresizeplain(TermEngine *session, int col, int row)
{
	int i;
	int minrow = MIN(row, term.row);
	int mincol = MIN(col, term.col);
	int scr, wrapnext = col == term.col ? term.c.state & CURSOR_WRAPNEXT : 0;
	int *bp;
	TCursor c;

	if (col < 1 || row < 1) {
		te_diagnostic(session,
		        "tresize: error resizing to %dx%d\n", col, row);
		return;
	}

	if (term.scr)
		te_selclear(session);
	/* Slide the screen to keep the cursor visible; retain displaced main rows. */
	for (i = 0; i <= term.c.y - row; i++) {
		if (te_history_size && !IS_SET(MODE_ALTSCREEN))
			free(te_thistorypush(session, term.line[i]));
		else
			free(term.line[i]);
		free(term.alt[i]);
	}
	/* ensure that both src and dst are not NULL */
	if (i > 0) {
		memmove(term.line, term.line + i, row * sizeof(Line));
		memmove(term.alt, term.alt + i, row * sizeof(Line));
	}
	for (i += row; i < term.row; i++) {
		free(term.line[i]);
		free(term.alt[i]);
	}

	/* resize to new height */
	term.line = te_realloc(term.line, row * sizeof(Line));
	term.alt  = te_realloc(term.alt,  row * sizeof(Line));
	term.dirty = te_realloc(term.dirty, row * sizeof(*term.dirty));
	term.tabs = te_realloc(term.tabs, col * sizeof(*term.tabs));

	/* resize each row to new width, zero-pad if needed */
	if (col != term.col) {
		Glyph blank = { .u = ' ', .fg = defaultfg, .bg = defaultbg };
		for (i = 0; i < te_history_size && term.hist; i++) {
			if (!term.hist[i])
				continue;
			term.hist[i] = te_realloc(term.hist[i], col * sizeof(Glyph));
			for (int j = term.col; j < col; j++)
				term.hist[i][j] = blank;
		}
	}
	for (i = 0; i < minrow; i++) {
		term.line[i] = te_realloc(term.line[i], col * sizeof(Glyph));
		term.alt[i]  = te_realloc(term.alt[i],  col * sizeof(Glyph));
	}

	/* allocate any new rows */
	for (/* i = minrow */; i < row; i++) {
		term.line[i] = te_alloc(col * sizeof(Glyph));
		term.alt[i] = te_alloc(col * sizeof(Glyph));
	}
	if (col > term.col) {
		bp = term.tabs + term.col;

		memset(bp, 0, sizeof(*term.tabs) * (col - term.col));
		while (--bp > term.tabs && !*bp)
			/* nothing */ ;
		for (bp += tabspaces; bp < term.tabs + col; bp += tabspaces)
			*bp = 1;
	}
	/* update terminal size */
	term.col = col;
	term.row = row;
	/* reset scrolling region */
	te_tsetscroll(session, 0, row-1);
	/* make use of the LIMIT in tmoveto */
	te_tmoveto(session, term.c.x, term.c.y);
	term.c.state |= wrapnext;
	/* Clearing both screens (it makes dirty all lines) */
	c = term.c;
	scr = term.scr;
	for (i = 0; i < 2; i++) {
		if (mincol < col && 0 < minrow) {
			te_tclearregion(session, mincol, 0, col - 1, minrow - 1);
		}
		if (0 < col && minrow < row) {
			te_tclearregion(session, 0, minrow, col - 1, row - 1);
		}
		te_tswapscreen(session);
		te_tcursor(session, CURSOR_LOAD);
	}
	term.c = c;
	term.scr = scr;
	if (scr)
		te_tfulldirt(session);
}

static Line
te_treflowline(TermEngine *session, Reflow *out)
{
	return out->lines[(out->count - 1) % out->cap];
}

static void
te_treflownew(TermEngine *session, Reflow *out)
{
	size_t slot = out->count % out->cap;
	Glyph blank = { .u = ' ', .fg = defaultfg, .bg = defaultbg };
	int x;

	free(out->lines[slot]);
	out->lines[slot] = te_alloc(out->col * sizeof(Glyph));
	for (x = 0; x < out->col; x++)
		out->lines[slot][x] = blank;
	out->count++;
	out->x = 0;
}

static void
te_treflow(TermEngine *session, int col, int row)
{
	Reflow out = { .col = col };
	size_t first, start, screenrow = 0, cursorrow = 0, savedrow = 0, anchorrow = 0;
	int cursorx = 0, savedx = 0, cursorfound = 0, savedfound = 0;
	int cursorend = 0, savedend = 0;
	int oldcol = term.col, oldrow = term.row, oldhistlen = term.histlen;
	int oldscr = term.scr, last = oldrow - 1, n, i, x, len, wrapped, previouswrap = 0, width;
	TCursor cursor = term.c, saved = session->saved[0];
	Line source;

	if (col <= 0 || row <= 0)
		te_fail("invalid reflow dimensions\n");
	/* Pending wraps denote insertion after the last glyph, not on that glyph. */
	if (cursor.state & CURSOR_WRAPNEXT) cursor.x = oldcol;
	if (saved.state & CURSOR_WRAPNEXT) saved.x = oldcol;
	/* Empty rows below the cursor are space for future output, not history. */
	while (last > cursor.y) {
		for (x = 0; x < oldcol && term.line[last][x].u == ' '; x++)
			;
		if (x != oldcol || term.line[last][oldcol - 1].mode & ATTR_WRAP)
			break;
		last--;
	}
	/* Keep at most the bounded history plus the visible screen while streaming. */
	out.cap = (size_t)te_history_size + row;
	out.lines = te_alloc(out.cap * sizeof(Line));
	memset(out.lines, 0, out.cap * sizeof(Line));
	te_treflownew(session, &out);
	for (n = 0; n < oldhistlen + last + 1; n++) {
		if (n < oldhistlen)
			source = term.hist[(term.histi - oldhistlen + n + te_history_size)
			                   % te_history_size];
		else
			source = term.line[n - oldhistlen];
		if (previouswrap && out.x == col) {
			te_treflowline(session, &out)[col - 1].mode |= ATTR_WRAP;
			te_treflownew(session, &out);
		}
		if (n == oldhistlen)
			screenrow = out.count - 1;
		if (n == oldhistlen - oldscr)
			anchorrow = out.count - 1;
		wrapped = source[oldcol - 1].mode & ATTR_WRAP;
		if (oldcol > 1 && source[oldcol - 1].mode & ATTR_WDUMMY)
			wrapped |= source[oldcol - 2].mode & ATTR_WRAP;
		previouswrap = wrapped;
		len = oldcol;
		if (!wrapped)
			while (len > 0 && source[len - 1].u == ' ')
				len--;
		if (n == oldhistlen + cursor.y)
			len = MAX(len, cursor.x);
		if (n == oldhistlen + saved.y)
			len = MAX(len, saved.x);
		for (i = 0; i < len; i++) {
			if (source[i].mode & ATTR_WDUMMY) {
				if (n == oldhistlen + cursor.y && i == cursor.x) {
					cursorrow = out.count - 1;
					cursorx = MAX(0, out.x - 2);
					cursorfound = 1;
				}
				if (n == oldhistlen + saved.y && i == saved.x) {
					savedrow = out.count - 1;
					savedx = MAX(0, out.x - 2);
					savedfound = 1;
				}
				continue;
			}
			width = (source[i].mode & ATTR_WIDE) && col > 1 ? 2 : 1;
			if (out.x + width > col) {
				te_treflowline(session, &out)[col - 1].mode |= ATTR_WRAP;
				te_treflownew(session, &out);
			}
			if (n == oldhistlen + cursor.y && i == cursor.x) {
				cursorrow = out.count - 1;
				cursorx = out.x;
				cursorfound = 1;
			}
			if (n == oldhistlen + saved.y && i == saved.x) {
				savedrow = out.count - 1;
				savedx = out.x;
				savedfound = 1;
			}
			Glyph glyph = source[i];
			glyph.mode &= ~(ATTR_WRAP | ATTR_WDUMMY | ATTR_WIDE);
			if (source[i].mode & ATTR_WIDE)
				glyph.mode |= ATTR_WIDE;
			te_treflowline(session, &out)[out.x++] = glyph;
			if (width == 2) {
				Glyph dummy = { .mode = ATTR_WDUMMY };
				te_treflowline(session, &out)[out.x++] = dummy;
			}
		}
		if (n == oldhistlen + cursor.y && !cursorfound && cursor.x == len) {
			cursorrow = out.count - 1;
			cursorend = out.x == col;
			cursorx = MIN(out.x, col - 1);
			cursorfound = 1;
		}
		if (n == oldhistlen + saved.y && !savedfound && saved.x == len) {
			savedrow = out.count - 1;
			savedend = out.x == col;
			savedx = MIN(out.x, col - 1);
			savedfound = 1;
		}
		if (!wrapped && n + 1 < oldhistlen + last + 1)
			te_treflownew(session, &out);
	}
	if (!cursorfound) {
		cursorrow = out.count - 1;
		cursorx = MIN(out.x, col - 1);
	}
	if (!savedfound) {
		savedrow = cursorrow;
		savedx = cursorx;
	}
	first = out.count > out.cap ? out.count - out.cap : 0;
	/* Keep the old screen boundary unless new wraps require older rows above it. */
	start = MAX(screenrow, out.count > (size_t)row ? out.count - row : 0);
	if (cursorrow < start)
		start = cursorrow;
	start = MAX(start, first);

	/* Reuse the ordinary resize for the alternate screen, tabs and margins. */
	te_tresizeplain(session, col, row);
	for (i = 0; i < term.row; i++)
		free(term.line[i]);
	free(term.line);
	if (term.hist) {
		for (i = 0; i < te_history_size; i++)
			free(term.hist[i]);
		free(term.hist);
	}
	term.line = te_alloc(row * sizeof(Line));
	term.histlen = MIN((size_t)te_history_size, start - first);
	term.hist = term.histlen ? te_alloc(te_history_size * sizeof(Line)) : NULL;
	if (term.hist)
		memset(term.hist, 0, te_history_size * sizeof(Line));
	term.histi = te_history_size ? term.histlen % te_history_size : 0;
	for (i = 0; i < term.histlen; i++) {
		size_t position = start - term.histlen + i;
		term.hist[i] = out.lines[position % out.cap];
		out.lines[position % out.cap] = NULL;
	}
	for (i = 0; i < row; i++) {
		size_t position = start + i;
		if (position < out.count) {
			term.line[i] = out.lines[position % out.cap];
			out.lines[position % out.cap] = NULL;
		} else {
			te_treflownew(session, &out);
			term.line[i] = out.lines[(out.count - 1) % out.cap];
			out.lines[(out.count - 1) % out.cap] = NULL;
		}
	}
	for (i = 0; i < out.cap; i++)
		free(out.lines[i]);
	free(out.lines);
	term.c = cursor;
	term.c.x = cursorx;
	term.c.y = MIN((size_t)(row - 1), cursorrow - start);
	MODBIT(term.c.state, cursorend, CURSOR_WRAPNEXT);
	session->saved[0] = saved;
	session->saved[0].x = savedx;
	session->saved[0].y = MIN((size_t)(row - 1), savedrow - start);
	MODBIT(session->saved[0].state, savedend, CURSOR_WRAPNEXT);
	term.scr = oldscr ? MIN(term.histlen, start > anchorrow ? start - anchorrow : 0) : 0;
	term.ocx = term.c.x;
	term.ocy = term.c.y;
	te_tfulldirt(session);
}

void
te_tresize(TermEngine *session, int col, int row)
{
	if (col > 0 && row > 0 && term.row > 0 && col != term.col &&
	    !IS_SET(MODE_ALTSCREEN)) {
		if (sel.ob.x != -1)
			te_selclear(session);
		te_treflow(session, col, row);
	} else {
		te_tresizeplain(session, col, row);
	}
}

void
te_resettitle(TermEngine *session)
{
	te_xsettitle(session, NULL);
}
