/* See LICENSE. Shared parser; every stateful operation receives its terminal explicitly. */

static Glyph
selection_cell(TermEngine *session, int x, int y)
{
	if (session->selection_only) {
		WorminalCell cell = session->selection_cells[(size_t)y * term.col + x];
		return (Glyph){.u = cell.rune, .mode = cell.attributes, .fg = cell.foreground, .bg = cell.background};
	}
	return te_tline(session, y)[x];
}

static void
selection_dirty(TermEngine *session, int first, int last)
{
	/* Borrowed selection contexts have no damage grid. Keep their special handling
	 * in selection operations, outside the parser's per-character damage path. */
	if (!session->selection_only) te_tsetdirt(session, first, last);
}

void
te_selinit(TermEngine *session)
{
	sel.mode = SEL_IDLE;
	sel.snap = 0;
	sel.ob.x = -1;
}

static int
te_tlinelen(TermEngine *session, int y)
{
	int i = term.col;
	if (selection_cell(session, i - 1, y).mode & ATTR_WRAP)
		return i;

	while (i > 0 && selection_cell(session, i - 1, y).u == ' ')
		--i;

	return i;
}

void
te_selstart(TermEngine *session, int col, int row, int snap)
{
	te_selclear(session);
	sel.mode = SEL_EMPTY;
	sel.type = SEL_REGULAR;
	sel.alt = IS_SET(MODE_ALTSCREEN);
	sel.snap = snap;
	sel.oe.x = sel.ob.x = col;
	sel.oe.y = sel.ob.y = row;
	te_selnormalize(session);

	if (sel.snap != 0)
		sel.mode = SEL_READY;
	selection_dirty(session, sel.nb.y, sel.ne.y);
}

void
te_selextend(TermEngine *session, int col, int row, int type, int done)
{
	int oldey, oldex, oldsby, oldsey, oldtype;

	if (sel.mode == SEL_IDLE)
		return;
	if (done && sel.mode == SEL_EMPTY) {
		te_selclear(session);
		return;
	}

	oldey = sel.oe.y;
	oldex = sel.oe.x;
	oldsby = sel.nb.y;
	oldsey = sel.ne.y;
	oldtype = sel.type;

	sel.oe.x = col;
	sel.oe.y = row;
	te_selnormalize(session);
	sel.type = type;

	if (oldey != sel.oe.y || oldex != sel.oe.x || oldtype != sel.type || sel.mode == SEL_EMPTY)
		selection_dirty(session, MIN(sel.nb.y, oldsby), MAX(sel.ne.y, oldsey));

	sel.mode = done ? SEL_IDLE : SEL_READY;
}

static void
te_selnormalize(TermEngine *session)
{
	int i;

	if (sel.type == SEL_REGULAR && sel.ob.y != sel.oe.y) {
		sel.nb.x = sel.ob.y < sel.oe.y ? sel.ob.x : sel.oe.x;
		sel.ne.x = sel.ob.y < sel.oe.y ? sel.oe.x : sel.ob.x;
	} else {
		sel.nb.x = MIN(sel.ob.x, sel.oe.x);
		sel.ne.x = MAX(sel.ob.x, sel.oe.x);
	}
	sel.nb.y = MIN(sel.ob.y, sel.oe.y);
	sel.ne.y = MAX(sel.ob.y, sel.oe.y);

	te_selsnap(session, &sel.nb.x, &sel.nb.y, -1);
	te_selsnap(session, &sel.ne.x, &sel.ne.y, +1);

	/* expand selection over line breaks */
	if (sel.type == SEL_RECTANGULAR)
		return;
	i = te_tlinelen(session, sel.nb.y);
	if (i < sel.nb.x)
		sel.nb.x = i;
	if (te_tlinelen(session, sel.ne.y) <= sel.ne.x)
		sel.ne.x = term.col - 1;
}

int
te_selected(TermEngine *session, int x, int y)
{
	if (sel.mode == SEL_EMPTY || sel.ob.x == -1 ||
			sel.alt != IS_SET(MODE_ALTSCREEN))
		return 0;

	if (sel.type == SEL_RECTANGULAR)
		return BETWEEN(y, sel.nb.y, sel.ne.y)
		    && BETWEEN(x, sel.nb.x, sel.ne.x);

	return BETWEEN(y, sel.nb.y, sel.ne.y)
	    && (y != sel.nb.y || x >= sel.nb.x)
	    && (y != sel.ne.y || x <= sel.ne.x);
}

static void
te_selsnap(TermEngine *session, int *x, int *y, int direction)
{
	int newx, newy, xt, yt;
	int delim, prevdelim;
	Glyph gp, prevgp;

	switch (sel.snap) {
	case SNAP_WORD:
		/*
		 * Snap around if the word wraps around at the end or
		 * beginning of a line.
		 */
		prevgp = selection_cell(session, *x, *y);
		prevdelim = ISDELIM(prevgp.u);
		for (;;) {
			newx = *x + direction;
			newy = *y;
			if (!BETWEEN(newx, 0, term.col - 1)) {
				newy += direction;
				newx = (newx + term.col) % term.col;
				if (!BETWEEN(newy, 0, term.row - 1))
					break;

				if (direction > 0)
					yt = *y, xt = *x;
				else
					yt = newy, xt = newx;
				if (!(selection_cell(session, xt, yt).mode & ATTR_WRAP))
					break;
			}

			if (newx >= te_tlinelen(session, newy))
				break;

			gp = selection_cell(session, newx, newy);
			delim = ISDELIM(gp.u);
			if (!(gp.mode & ATTR_WDUMMY) && (delim != prevdelim
					|| (delim && gp.u != prevgp.u)))
				break;

			*x = newx;
			*y = newy;
			prevgp = gp;
			prevdelim = delim;
		}
		break;
	case SNAP_LINE:
		/*
		 * Snap around if the the previous line or the current one
		 * has set ATTR_WRAP at its end. Then the whole next or
		 * previous line will be selected.
		 */
		*x = (direction < 0) ? 0 : term.col - 1;
		if (direction < 0) {
			for (; *y > 0; *y += direction) {
				if (!(selection_cell(session, term.col-1, *y-1).mode
						& ATTR_WRAP)) {
					break;
				}
			}
		} else if (direction > 0) {
			for (; *y < term.row-1; *y += direction) {
				if (!(selection_cell(session, term.col-1, *y).mode
						& ATTR_WRAP)) {
					break;
				}
			}
		}
		break;
	}
}

char *
te_getsel(TermEngine *session)
{
	char *str, *ptr;
	int y, bufsize, lastx, linelen, first, last;

	if (sel.ob.x == -1)
		return NULL;

	bufsize = (term.col+1) * (sel.ne.y-sel.nb.y+1) * UTF_SIZ;
	ptr = str = te_alloc(bufsize);

	/* append every set & selected glyph to the selection */
	for (y = sel.nb.y; y <= sel.ne.y; y++) {
		if ((linelen = te_tlinelen(session, y)) == 0) {
			*ptr++ = '\n';
			continue;
		}

		if (sel.type == SEL_RECTANGULAR) {
			first = sel.nb.x;
			lastx = sel.ne.x;
		} else {
			first = sel.nb.y == y ? sel.nb.x : 0;
			lastx = (sel.ne.y == y) ? sel.ne.x : term.col-1;
		}
		last = MIN(lastx, linelen-1);
		while (last >= first && selection_cell(session, last, y).u == ' ')
			--last;

		for (int x = first; x <= last; ++x) {
			Glyph cell = selection_cell(session, x, y);
			if (cell.mode & ATTR_WDUMMY)
				continue;

			ptr += utf8encode(cell.u, ptr);
		}

		/* Clipboard text uses LF for hard line breaks and joins soft wraps.
		 * Paste handling preserves text in bracketed mode; ordinary paste
		 * converts line endings to Enter keystrokes (see paste.h). */
		/* Trimming an all-space selected prefix can leave last == -1. No glyph
		 * exists there; treating it as a hard break also avoids a read before row 0. */
		if ((y < sel.ne.y || lastx >= linelen) &&
		    (last < 0 || !(selection_cell(session, last, y).mode & ATTR_WRAP) || sel.type == SEL_RECTANGULAR))
			*ptr++ = '\n';
	}
	*ptr = 0;
	return str;
}

void
te_selclear(TermEngine *session)
{
	if (sel.ob.x == -1)
		return;
	sel.mode = SEL_IDLE;
	sel.ob.x = -1;
	selection_dirty(session, sel.nb.y, sel.ne.y);
}

int
te_tattrset(TermEngine *session, int attr)
{
	int i, j;

	for (i = 0; i < term.row-1; i++) {
		for (j = 0; j < term.col-1; j++) {
			if (term.line[i][j].mode & attr)
				return 1;
		}
	}

	return 0;
}

void
te_tsetdirt(TermEngine *session, int top, int bot)
{
	int i;

	if (term.row <= 0)
		return;

	LIMIT(top, 0, term.row-1);
	LIMIT(bot, 0, term.row-1);

	for (i = top; i <= bot; i++)
		term.dirty[i] = 1;
}

void
te_tsetdirtattr(TermEngine *session, int attr)
{
	int i, j;

	for (i = 0; i < term.row-1; i++) {
		for (j = 0; j < term.col-1; j++) {
			if (term.line[i][j].mode & attr) {
				te_tsetdirt(session, i, i);
				break;
			}
		}
	}
}

void
te_tfulldirt(TermEngine *session)
{
	te_tsetdirt(session, 0, term.row-1);
}

void
te_tcursor(TermEngine *session, int mode)
{
	TCursor *c = session->saved;
	int alt = IS_SET(MODE_ALTSCREEN);

	if (mode == CURSOR_SAVE) {
		c[alt] = term.c;
	} else if (mode == CURSOR_LOAD) {
		term.c = c[alt];
		te_tmoveto(session, c[alt].x, c[alt].y);
	}
}

void
te_treset(TermEngine *session)
{
	uint i;
	te_setsync(session, 0);

	term.c = (TCursor){{
		.mode = ATTR_NULL,
		.fg = defaultfg,
		.bg = defaultbg
	}, .x = 0, .y = 0, .state = CURSOR_DEFAULT};

	memset(term.tabs, 0, term.col * sizeof(*term.tabs));
	for (i = tabspaces; i < term.col; i += tabspaces)
		term.tabs[i] = 1;
	term.top = 0;
	term.bot = term.row - 1;
	term.mode = MODE_WRAP|MODE_UTF8;
	memset(term.trantbl, CS_USA, sizeof(term.trantbl));
	term.charset = 0;

	for (i = 0; i < 2; i++) {
		te_tmoveto(session, 0, 0);
		te_tcursor(session, CURSOR_SAVE);
		te_tclearregion(session, 0, 0, term.col-1, term.row-1);
		te_tswapscreen(session);
	}
}

void
te_tnew(TermEngine *session, int col, int row)
{
	term = (Term){ .c = { .attr = { .fg = defaultfg, .bg = defaultbg } } };
	te_tresize(session, col, row);
	te_treset(session);
}

Line
te_tline(TermEngine *session, int y)
{
	return te_tlineat(session, y, term.scr);
}

Line
te_tlineat(TermEngine *session, int y, int scroll)
{
	LIMIT(scroll, 0, term.histlen);
	if (IS_SET(MODE_ALTSCREEN))
		scroll = 0;
	if (y < scroll) {
		if (!term.hist || te_history_size <= 0)
			te_fail("invalid scrollback viewport\n");
		return term.hist[(term.histi - scroll + y + te_history_size)
		                 % te_history_size];
	}
	return term.line[y - scroll];
}

static Line
te_thistorypush(TermEngine *session, Line line)
{
	Line recycled;

	if (te_history_size <= 0)
		te_fail("history is disabled\n");
	if (!term.hist) {
		term.hist = te_alloc(te_history_size * sizeof(Line));
		memset(term.hist, 0, te_history_size * sizeof(Line));
	}
	recycled = term.hist[term.histi];
	term.hist[term.histi] = line;
	term.histi = (term.histi + 1) % te_history_size;
	if (term.histlen < te_history_size)
		term.histlen++;
	if (term.scr)
		term.scr = MIN(term.scr + 1, term.histlen);
	return recycled;
}

void
te_kscrollup(TermEngine *session, const Arg *arg)
{
	int n;

	if (IS_SET(MODE_ALTSCREEN) || !term.histlen)
		return;
	n = arg->i < 0 ? term.row + arg->i : arg->i ? arg->i : te_scroll_multiplier;
	LIMIT(n, 0, term.histlen - term.scr);
	if (n) {
		te_selclear(session);
		term.scr += n;
		te_tfulldirt(session);
	}
}

void
te_kscrolldown(TermEngine *session, const Arg *arg)
{
	int n;

	if (IS_SET(MODE_ALTSCREEN) || !term.scr)
		return;
	n = arg->i < 0 ? term.row + arg->i : arg->i ? arg->i : te_scroll_multiplier;
	LIMIT(n, 0, term.scr);
	if (n) {
		te_selclear(session);
		term.scr -= n;
		te_tfulldirt(session);
	}
}

void
te_tswapscreen(TermEngine *session)
{
	Line *tmp = term.line;

	if (term.scr)
		te_selclear(session);
	term.scr = 0;
	term.line = term.alt;
	term.alt = tmp;
	term.mode ^= MODE_ALTSCREEN;
	te_tfulldirt(session);
}

int
te_tisaltscr(TermEngine *session)
{
	return IS_SET(MODE_ALTSCREEN);
}

static void
te_tscrolldown(TermEngine *session, int orig, int n)
{
	int i;
	Line temp;

	LIMIT(n, 0, term.bot-orig+1);

	te_tsetdirt(session, orig, term.bot-n);
	te_tclearregion(session, 0, term.bot-n+1, term.col-1, term.bot);

	for (i = term.bot; i >= orig+n; i--) {
		temp = term.line[i];
		term.line[i] = term.line[i-n];
		term.line[i-n] = temp;
	}

	te_selscroll(session, orig, n);
}

void
te_tscrollup(TermEngine *session, int orig, int n)
{
	int i, j;
	Line temp;

	LIMIT(n, 0, term.bot-orig+1);
	if (!n)
		return;
	/* Only a full main-screen scroll contributes to command history. */
	if (!IS_SET(MODE_ALTSCREEN) && orig == 0 &&
	    term.top == 0 && term.bot == term.row - 1 && te_history_size) {
		if (term.scr)
			te_selclear(session);
		for (j = 0; j < n; j++) {
			temp = te_thistorypush(session, term.line[j]);
			term.line[j] = temp ? temp : te_alloc(term.col * sizeof(Glyph));
		}
		if (term.scr) {
			te_tfulldirt(session);
		}
	}

	te_tclearregion(session, 0, orig, term.col-1, orig+n-1);
	te_tsetdirt(session, orig+n, term.bot);

	for (i = orig; i <= term.bot-n; i++) {
		temp = term.line[i];
		term.line[i] = term.line[i+n];
		term.line[i+n] = temp;
	}

	te_selscroll(session, orig, -n);
}

static void
te_selscroll(TermEngine *session, int orig, int n)
{
	if (sel.ob.x == -1 || sel.alt != IS_SET(MODE_ALTSCREEN))
		return;

	if (BETWEEN(sel.nb.y, orig, term.bot) != BETWEEN(sel.ne.y, orig, term.bot)) {
		te_selclear(session);
	} else if (BETWEEN(sel.nb.y, orig, term.bot)) {
		sel.ob.y += n;
		sel.oe.y += n;
		if (sel.ob.y < term.top || sel.ob.y > term.bot ||
		    sel.oe.y < term.top || sel.oe.y > term.bot) {
			te_selclear(session);
		} else {
			te_selnormalize(session);
		}
	}
}

static void
te_tnewline(TermEngine *session, int first_col)
{
	int y = term.c.y;

	if (y == term.bot) {
		te_tscrollup(session, term.top, 1);
	} else {
		y++;
	}
	te_tmoveto(session, first_col ? 0 : term.c.x, y);
}
