/* See LICENSE. Shared parser; every stateful operation receives its terminal explicitly. */

static void
te_csiparse(TermEngine *session)
{
	char *p = csiescseq.buf, *np;
	long int v;
	int sep = ';'; /* colon or semi-colon, but not both */

	csiescseq.narg = 0;
	if (*p == '?') {
		csiescseq.priv = 1;
		p++;
	}

	csiescseq.buf[csiescseq.len] = '\0';
	while (p < csiescseq.buf+csiescseq.len) {
		np = NULL;
		v = strtol(p, &np, 10);
		if (np == p)
			v = 0;
		if (v == LONG_MAX || v == LONG_MIN)
			v = -1;
		csiescseq.arg[csiescseq.narg++] = v;
		p = np;
		if (sep == ';' && *p == ':')
			sep = ':'; /* allow override to colon once */
		if (*p != sep || csiescseq.narg == ESC_ARG_SIZ)
			break;
		p++;
	}
	csiescseq.mode[0] = *p++;
	csiescseq.mode[1] = (p < csiescseq.buf+csiescseq.len) ? *p : '\0';
}

static void
te_tmoveato(TermEngine *session, int x, int y)
{
	te_tmoveto(session, x, y + ((term.c.state & CURSOR_ORIGIN) ? term.top: 0));
}

void
te_tmoveto(TermEngine *session, int x, int y)
{
	int miny, maxy;

	if (term.c.state & CURSOR_ORIGIN) {
		miny = term.top;
		maxy = term.bot;
	} else {
		miny = 0;
		maxy = term.row - 1;
	}
	term.c.state &= ~CURSOR_WRAPNEXT;
	term.c.x = LIMIT(x, 0, term.col-1);
	term.c.y = LIMIT(y, miny, maxy);
}

static void
te_tsetchar(TermEngine *session, Rune u, const Glyph *attr, int x, int y)
{
	static const char *vt100_0[62] = { /* 0x41 - 0x7e */
		"↑", "↓", "→", "←", "█", "▚", "☃", /* A - G */
		0, 0, 0, 0, 0, 0, 0, 0, /* H - O */
		0, 0, 0, 0, 0, 0, 0, 0, /* P - W */
		0, 0, 0, 0, 0, 0, 0, " ", /* X - _ */
		"◆", "▒", "␉", "␌", "␍", "␊", "°", "±", /* ` - g */
		"␤", "␋", "┘", "┐", "┌", "└", "┼", "⎺", /* h - o */
		"⎻", "─", "⎼", "⎽", "├", "┤", "┴", "┬", /* p - w */
		"│", "≤", "≥", "π", "≠", "£", "·", /* x - ~ */
	};

	/*
	 * The table is proudly stolen from rxvt.
	 */
	if (term.trantbl[term.charset] == CS_GRAPHIC0 &&
	   BETWEEN(u, 0x41, 0x7e) && vt100_0[u - 0x41])
		utf8decode(vt100_0[u - 0x41], &u, UTF_SIZ);

	if (term.line[y][x].mode & ATTR_WIDE) {
		if (x+1 < term.col) {
			term.line[y][x+1].u = ' ';
			term.line[y][x+1].mode &= ~ATTR_WDUMMY;
		}
	} else if (term.line[y][x].mode & ATTR_WDUMMY) {
		term.line[y][x-1].u = ' ';
		term.line[y][x-1].mode &= ~ATTR_WIDE;
	}

	term.dirty[y] = 1;
	term.line[y][x] = *attr;
	term.line[y][x].u = u;
}

static void
te_tclearregion(TermEngine *session, int x1, int y1, int x2, int y2)
{
	int x, y, temp;
	Glyph *gp;

	if (x1 > x2)
		temp = x1, x1 = x2, x2 = temp;
	if (y1 > y2)
		temp = y1, y1 = y2, y2 = temp;

	LIMIT(x1, 0, term.col-1);
	LIMIT(x2, 0, term.col-1);
	LIMIT(y1, 0, term.row-1);
	LIMIT(y2, 0, term.row-1);

	for (y = y1; y <= y2; y++) {
		term.dirty[y] = 1;
		for (x = x1; x <= x2; x++) {
			gp = &term.line[y][x];
			if (te_selected(session, x, y))
				te_selclear(session);
			gp->fg = term.c.attr.fg;
			gp->bg = term.c.attr.bg;
			gp->mode = 0;
			gp->u = ' ';
		}
	}
}

static void
te_tdeletechar(TermEngine *session, int n)
{
	int dst, src, size;
	Glyph *line;

	LIMIT(n, 0, term.col - term.c.x);

	dst = term.c.x;
	src = term.c.x + n;
	size = term.col - src;
	line = term.line[term.c.y];

	memmove(&line[dst], &line[src], size * sizeof(Glyph));
	te_tclearregion(session, term.col-n, term.c.y, term.col-1, term.c.y);
}

static void
te_tinsertblank(TermEngine *session, int n)
{
	int dst, src, size;
	Glyph *line;

	LIMIT(n, 0, term.col - term.c.x);

	dst = term.c.x + n;
	src = term.c.x;
	size = term.col - dst;
	line = term.line[term.c.y];

	memmove(&line[dst], &line[src], size * sizeof(Glyph));
	te_tclearregion(session, src, term.c.y, dst - 1, term.c.y);
}

static void
te_tinsertblankline(TermEngine *session, int n)
{
	if (BETWEEN(term.c.y, term.top, term.bot))
		te_tscrolldown(session, term.c.y, n);
}

static void
te_tdeleteline(TermEngine *session, int n)
{
	if (BETWEEN(term.c.y, term.top, term.bot))
		te_tscrollup(session, term.c.y, n);
}

static int32_t
te_tdefcolor(TermEngine *session, const int *attr, int *npar, int l)
{
	int32_t idx = -1;
	uint r, g, b;

	switch (attr[*npar + 1]) {
	case 2: /* direct color in RGB space */
		if (*npar + 4 >= l) {
			te_diagnostic(session,
				"erresc(38): Incorrect number of parameters (%d)\n",
				*npar);
			break;
		}
		r = attr[*npar + 2];
		g = attr[*npar + 3];
		b = attr[*npar + 4];
		*npar += 4;
		if (!BETWEEN(r, 0, 255) || !BETWEEN(g, 0, 255) || !BETWEEN(b, 0, 255))
			te_diagnostic(session, "erresc: bad rgb color (%u,%u,%u)\n",
				r, g, b);
		else
			idx = TRUECOLOR(r, g, b);
		break;
	case 5: /* indexed color */
		if (*npar + 2 >= l) {
			te_diagnostic(session,
				"erresc(38): Incorrect number of parameters (%d)\n",
				*npar);
			break;
		}
		*npar += 2;
		if (!BETWEEN(attr[*npar], 0, 255))
			te_diagnostic(session, "erresc: bad fgcolor %d\n", attr[*npar]);
		else
			idx = attr[*npar];
		break;
	case 0: /* implemented defined (only foreground) */
	case 1: /* transparent */
	case 3: /* direct color in CMY space */
	case 4: /* direct color in CMYK space */
	default:
		te_diagnostic(session,
		        "erresc(38): gfx attr %d unknown\n", attr[*npar]);
		break;
	}

	return idx;
}

static void
te_tsetattr(TermEngine *session, const int *attr, int l)
{
	int i;
	int32_t idx;

	for (i = 0; i < l; i++) {
		switch (attr[i]) {
		case 0:
			term.c.attr.mode &= ~(
				ATTR_BOLD       |
				ATTR_FAINT      |
				ATTR_ITALIC     |
				ATTR_UNDERLINE  |
				ATTR_BLINK      |
				ATTR_REVERSE    |
				ATTR_INVISIBLE  |
				ATTR_STRUCK     );
			term.c.attr.fg = defaultfg;
			term.c.attr.bg = defaultbg;
			break;
		case 1:
			term.c.attr.mode |= ATTR_BOLD;
			break;
		case 2:
			term.c.attr.mode |= ATTR_FAINT;
			break;
		case 3:
			term.c.attr.mode |= ATTR_ITALIC;
			break;
		case 4:
			term.c.attr.mode |= ATTR_UNDERLINE;
			break;
		case 5: /* slow blink */
			/* FALLTHROUGH */
		case 6: /* rapid blink */
			term.c.attr.mode |= ATTR_BLINK;
			break;
		case 7:
			term.c.attr.mode |= ATTR_REVERSE;
			break;
		case 8:
			term.c.attr.mode |= ATTR_INVISIBLE;
			break;
		case 9:
			term.c.attr.mode |= ATTR_STRUCK;
			break;
		case 22:
			term.c.attr.mode &= ~(ATTR_BOLD | ATTR_FAINT);
			break;
		case 23:
			term.c.attr.mode &= ~ATTR_ITALIC;
			break;
		case 24:
			term.c.attr.mode &= ~ATTR_UNDERLINE;
			break;
		case 25:
			term.c.attr.mode &= ~ATTR_BLINK;
			break;
		case 27:
			term.c.attr.mode &= ~ATTR_REVERSE;
			break;
		case 28:
			term.c.attr.mode &= ~ATTR_INVISIBLE;
			break;
		case 29:
			term.c.attr.mode &= ~ATTR_STRUCK;
			break;
		case 38:
			if ((idx = te_tdefcolor(session, attr, &i, l)) >= 0)
				term.c.attr.fg = idx;
			break;
		case 39: /* set foreground color to default */
			term.c.attr.fg = defaultfg;
			break;
		case 48:
			if ((idx = te_tdefcolor(session, attr, &i, l)) >= 0)
				term.c.attr.bg = idx;
			break;
		case 49: /* set background color to default */
			term.c.attr.bg = defaultbg;
			break;
		case 58:
			/* This starts a sequence to change the color of
			 * "underline" pixels. We don't support that and
			 * instead eat up a following "5;n" or "2;r;g;b". */
			te_tdefcolor(session, attr, &i, l);
			break;
		default:
			if (BETWEEN(attr[i], 30, 37)) {
				term.c.attr.fg = attr[i] - 30;
			} else if (BETWEEN(attr[i], 40, 47)) {
				term.c.attr.bg = attr[i] - 40;
			} else if (BETWEEN(attr[i], 90, 97)) {
				term.c.attr.fg = attr[i] - 90 + 8;
			} else if (BETWEEN(attr[i], 100, 107)) {
				term.c.attr.bg = attr[i] - 100 + 8;
			} else {
				te_diagnostic(session,
					"erresc(default): gfx attr %d unknown\n",
					attr[i]);
				te_csidump(session);
			}
			break;
		}
	}
}

void
te_tsetscroll(TermEngine *session, int t, int b)
{
	int temp;

	LIMIT(t, 0, term.row-1);
	LIMIT(b, 0, term.row-1);
	if (t > b) {
		temp = t;
		t = b;
		b = temp;
	}
	term.top = t;
	term.bot = b;
}

static void
te_tsetmode(TermEngine *session, int priv, int set, const int *args, int narg)
{
	int alt; const int *lim;

	for (lim = args + narg; args < lim; ++args) {
		if (priv) {
			switch (*args) {
			case 1: /* DECCKM -- Cursor key */
				te_xsetmode(session, set, MODE_APPCURSOR);
				break;
			case 5: /* DECSCNM -- Reverse video */
				te_xsetmode(session, set, MODE_REVERSE);
				break;
			case 6: /* DECOM -- Origin */
				MODBIT(term.c.state, set, CURSOR_ORIGIN);
				te_tmoveato(session, 0, 0);
				break;
			case 7: /* DECAWM -- Auto wrap */
				MODBIT(term.mode, set, MODE_WRAP);
				break;
			case 0:  /* Error (IGNORED) */
			case 2:  /* DECANM -- ANSI/VT52 (IGNORED) */
			case 3:  /* DECCOLM -- Column  (IGNORED) */
			case 4:  /* DECSCLM -- Scroll (IGNORED) */
			case 8:  /* DECARM -- Auto repeat (IGNORED) */
			case 18: /* DECPFF -- Printer feed (IGNORED) */
			case 19: /* DECPEX -- Printer extent (IGNORED) */
			case 42: /* DECNRCM -- National characters (IGNORED) */
			case 12: /* att610 -- Start blinking cursor (IGNORED) */
				break;
			case 25: /* DECTCEM -- Text Cursor Enable Mode */
				te_xsetmode(session, !set, MODE_HIDE);
				break;
			case 9:    /* X10 mouse compatibility mode */
				te_xsetpointermotion(session, 0);
				te_xsetmode(session, 0, MODE_MOUSE);
				te_xsetmode(session, set, MODE_MOUSEX10);
				break;
			case 1000: /* 1000: report button press */
				te_xsetpointermotion(session, 0);
				te_xsetmode(session, 0, MODE_MOUSE);
				te_xsetmode(session, set, MODE_MOUSEBTN);
				break;
			case 1002: /* 1002: report motion on button press */
				te_xsetpointermotion(session, 0);
				te_xsetmode(session, 0, MODE_MOUSE);
				te_xsetmode(session, set, MODE_MOUSEMOTION);
				break;
			case 1003: /* 1003: enable all mouse motions */
				te_xsetpointermotion(session, set);
				te_xsetmode(session, 0, MODE_MOUSE);
				te_xsetmode(session, set, MODE_MOUSEMANY);
				break;
			case 1004: /* 1004: send focus events to tty */
				te_xsetmode(session, set, MODE_FOCUS);
				break;
			case 1006: /* 1006: extended reporting mode */
				te_xsetmode(session, set, MODE_MOUSESGR);
				break;
			case 1007: /* xterm alternate scroll: wheel sends cursor keys */
				te_xsetmode(session, set, MODE_ALTSCROLL);
				break;
			case 1034: /* 1034: enable 8-bit mode for keyboard input */
				te_xsetmode(session, set, MODE_8BIT);
				break;
			case 1049: /* swap screen & set/restore cursor as xterm */
				if (!session->altscreen_allowed)
					break;
				te_tcursor(session, (set) ? CURSOR_SAVE : CURSOR_LOAD);
				/* FALLTHROUGH */
			case 47: /* swap screen buffer */
			case 1047: /* swap screen buffer */
				if (!session->altscreen_allowed)
					break;
				alt = IS_SET(MODE_ALTSCREEN);
				if (alt) {
					te_tclearregion(session, 0, 0, term.col-1,
							term.row-1);
				}
				if (set ^ alt) /* set is always 1 or 0 */
					te_tswapscreen(session);
				if (*args != 1049)
					break;
				/* FALLTHROUGH */
			case 1048: /* save/restore cursor (like DECSC/DECRC) */
				te_tcursor(session, (set) ? CURSOR_SAVE : CURSOR_LOAD);
				break;
			case 2004: /* 2004: bracketed paste mode */
				te_xsetmode(session, set, MODE_BRCKTPASTE);
				break;
			case 2026: /* Synchronized application redraw. */
				te_setsync(session, set);
				break;
			/* Not implemented mouse modes. See comments there. */
			case 1001: /* mouse highlight mode; can hang the
				      terminal by design when implemented. */
			case 1005: /* UTF-8 mouse mode; will confuse
				      applications not supporting UTF-8
				      and luit. */
			case 1015: /* urxvt mangled mouse mode; incompatible
				      and can be mistaken for other control
				      codes. */
				break;
			default:
				te_diagnostic(session,
					"erresc: unknown private set/reset mode %d\n",
					*args);
				break;
			}
		} else {
			switch (*args) {
			case 0:  /* Error (IGNORED) */
				break;
			case 2:
				te_xsetmode(session, set, MODE_KBDLOCK);
				break;
			case 4:  /* IRM -- Insertion-replacement */
				MODBIT(term.mode, set, MODE_INSERT);
				break;
			case 12: /* SRM -- Send/Receive */
				MODBIT(term.mode, !set, MODE_ECHO);
				break;
			case 20: /* LNM -- Linefeed/new line */
				MODBIT(term.mode, set, MODE_CRLF);
				break;
			default:
				te_diagnostic(session,
					"erresc: unknown set/reset mode %d\n",
					*args);
				break;
			}
		}
	}
}

static void
te_csihandle(TermEngine *session)
{
	char buf[40];
	int len;

	switch (csiescseq.mode[0]) {
	default:
	unknown:
		te_diagnostic(session, "erresc: unknown csi ");
		te_csidump(session);
		/* te_fail(""); */
		break;
	case '@': /* ICH -- Insert <n> blank char */
		DEFAULT(csiescseq.arg[0], 1);
		te_tinsertblank(session, csiescseq.arg[0]);
		break;
	case 'A': /* CUU -- Cursor <n> Up */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveto(session, term.c.x, term.c.y-csiescseq.arg[0]);
		break;
	case 'B': /* CUD -- Cursor <n> Down */
	case 'e': /* VPR --Cursor <n> Down */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveto(session, term.c.x, term.c.y+csiescseq.arg[0]);
		break;
	case 'i': /* MC -- Media Copy */
		switch (csiescseq.arg[0]) {
		case 0:
			te_tdump(session);
			break;
		case 1:
			te_tdumpline(session, term.c.y);
			break;
		case 2:
			te_tdumpsel(session);
			break;
		case 4:
			term.mode &= ~MODE_PRINT;
			break;
		case 5:
			term.mode |= MODE_PRINT;
			break;
		}
		break;
	case 'c': /* DA -- Device Attributes */
		if (csiescseq.arg[0] == 0)
			te_ttywrite(session, vtiden, strlen(vtiden), 0);
		break;
	case 'b': /* REP -- if last char is printable print it <n> more times */
		LIMIT(csiescseq.arg[0], 1, 65535);
		if (term.lastc)
			while (csiescseq.arg[0]-- > 0)
				te_tputc(session, term.lastc);
		break;
	case 'C': /* CUF -- Cursor <n> Forward */
	case 'a': /* HPR -- Cursor <n> Forward */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveto(session, term.c.x+csiescseq.arg[0], term.c.y);
		break;
	case 'D': /* CUB -- Cursor <n> Backward */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveto(session, term.c.x-csiescseq.arg[0], term.c.y);
		break;
	case 'E': /* CNL -- Cursor <n> Down and first col */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveto(session, 0, term.c.y+csiescseq.arg[0]);
		break;
	case 'F': /* CPL -- Cursor <n> Up and first col */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveto(session, 0, term.c.y-csiescseq.arg[0]);
		break;
	case 'g': /* TBC -- Tabulation clear */
		switch (csiescseq.arg[0]) {
		case 0: /* clear current tab stop */
			term.tabs[term.c.x] = 0;
			break;
		case 3: /* clear all the tabs */
			memset(term.tabs, 0, term.col * sizeof(*term.tabs));
			break;
		default:
			goto unknown;
		}
		break;
	case 'G': /* CHA -- Move to <col> */
	case '`': /* HPA */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveto(session, csiescseq.arg[0]-1, term.c.y);
		break;
	case 'H': /* CUP -- Move to <row> <col> */
	case 'f': /* HVP */
		DEFAULT(csiescseq.arg[0], 1);
		DEFAULT(csiescseq.arg[1], 1);
		te_tmoveato(session, csiescseq.arg[1]-1, csiescseq.arg[0]-1);
		break;
	case 'I': /* CHT -- Cursor Forward Tabulation <n> tab stops */
		DEFAULT(csiescseq.arg[0], 1);
		te_tputtab(session, csiescseq.arg[0]);
		break;
	case 'J': /* ED -- Clear screen */
		switch (csiescseq.arg[0]) {
		case 0: /* below */
			te_tclearregion(session, term.c.x, term.c.y, term.col-1, term.c.y);
			if (term.c.y < term.row-1) {
				te_tclearregion(session, 0, term.c.y+1, term.col-1,
						term.row-1);
			}
			break;
		case 1: /* above */
			if (term.c.y > 0)
				te_tclearregion(session, 0, 0, term.col-1, term.c.y-1);
			te_tclearregion(session, 0, term.c.y, term.c.x, term.c.y);
			break;
		case 2: /* all */
			te_tclearregion(session, 0, 0, term.col-1, term.row-1);
			break;
		default:
			goto unknown;
		}
		break;
	case 'K': /* EL -- Clear line */
		switch (csiescseq.arg[0]) {
		case 0: /* right */
			te_tclearregion(session, term.c.x, term.c.y, term.col-1,
					term.c.y);
			break;
		case 1: /* left */
			te_tclearregion(session, 0, term.c.y, term.c.x, term.c.y);
			break;
		case 2: /* all */
			te_tclearregion(session, 0, term.c.y, term.col-1, term.c.y);
			break;
		}
		break;
	case 'S': /* SU -- Scroll <n> line up */
		if (csiescseq.priv) break;
		DEFAULT(csiescseq.arg[0], 1);
		te_tscrollup(session, term.top, csiescseq.arg[0]);
		break;
	case 'T': /* SD -- Scroll <n> line down */
		DEFAULT(csiescseq.arg[0], 1);
		te_tscrolldown(session, term.top, csiescseq.arg[0]);
		break;
	case 'L': /* IL -- Insert <n> blank lines */
		DEFAULT(csiescseq.arg[0], 1);
		te_tinsertblankline(session, csiescseq.arg[0]);
		break;
	case 'l': /* RM -- Reset Mode */
		te_tsetmode(session, csiescseq.priv, 0, csiescseq.arg, csiescseq.narg);
		break;
	case 'M': /* DL -- Delete <n> lines */
		DEFAULT(csiescseq.arg[0], 1);
		te_tdeleteline(session, csiescseq.arg[0]);
		break;
	case 'X': /* ECH -- Erase <n> char */
		DEFAULT(csiescseq.arg[0], 1);
		te_tclearregion(session, term.c.x, term.c.y,
				term.c.x + csiescseq.arg[0] - 1, term.c.y);
		break;
	case 'P': /* DCH -- Delete <n> char */
		DEFAULT(csiescseq.arg[0], 1);
		te_tdeletechar(session, csiescseq.arg[0]);
		break;
	case 'Z': /* CBT -- Cursor Backward Tabulation <n> tab stops */
		DEFAULT(csiescseq.arg[0], 1);
		te_tputtab(session, -csiescseq.arg[0]);
		break;
	case 'd': /* VPA -- Move to <row> */
		DEFAULT(csiescseq.arg[0], 1);
		te_tmoveato(session, term.c.x, csiescseq.arg[0]-1);
		break;
	case 'h': /* SM -- Set terminal mode */
		te_tsetmode(session, csiescseq.priv, 1, csiescseq.arg, csiescseq.narg);
		break;
	case 'm': /* SGR -- Terminal attribute (color) */
		te_tsetattr(session, csiescseq.arg, csiescseq.narg);
		break;
	case 'n': /* DSR -- Device Status Report */
		switch (csiescseq.arg[0]) {
		case 5: /* Status Report "OK" `0n` */
			te_ttywrite(session, "\033[0n", sizeof("\033[0n") - 1, 0);
			break;
		case 6: /* Report Cursor Position (CPR) "<row>;<column>R" */
			len = snprintf(buf, sizeof(buf), "\033[%i;%iR",
			               term.c.y+1, term.c.x+1);
			te_ttywrite(session, buf, len, 0);
			break;
		default:
			goto unknown;
		}
		break;
	case 'r': /* DECSTBM -- Set Scrolling Region */
		if (csiescseq.priv) {
			goto unknown;
		} else {
			DEFAULT(csiescseq.arg[0], 1);
			DEFAULT(csiescseq.arg[1], term.row);
			te_tsetscroll(session, csiescseq.arg[0]-1, csiescseq.arg[1]-1);
			te_tmoveato(session, 0, 0);
		}
		break;
	case 's': /* DECSC -- Save cursor position (ANSI.SYS) */
		te_tcursor(session, CURSOR_SAVE);
		break;
	case 'u': /* DECRC -- Restore cursor position (ANSI.SYS) */
		if (csiescseq.priv) {
			goto unknown;
		} else {
			te_tcursor(session, CURSOR_LOAD);
		}
		break;
	case ' ':
		switch (csiescseq.mode[1]) {
		case 'q': /* DECSCUSR -- Set Cursor Style */
			if (te_xsetcursor(session, csiescseq.arg[0]))
				goto unknown;
			break;
		default:
			goto unknown;
		}
		break;
	}
}

static void
te_csidump(TermEngine *session)
{
	size_t i;
	uint c;

	te_diagnostic(session, "ESC[");
	for (i = 0; i < csiescseq.len; i++) {
		c = csiescseq.buf[i] & 0xff;
		if (isprint(c)) {
			te_diagnostic(session, "%c", c);
		} else if (c == '\n') {
			te_diagnostic(session, "(\\n)");
		} else if (c == '\r') {
			te_diagnostic(session, "(\\r)");
		} else if (c == 0x1b) {
			te_diagnostic(session, "(\\e)");
		} else {
			te_diagnostic(session, "(%02x)", c);
		}
	}
	te_diagnostic(session, "%c", '\n');
}

static void
te_csireset(TermEngine *session)
{
	memset(&csiescseq, 0, sizeof(csiescseq));
}
