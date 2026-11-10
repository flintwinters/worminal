/* See LICENSE. Shared parser; every stateful operation receives its terminal explicitly. */

static void
te_osc_color_response(TermEngine *session, int num, int index, int is_osc4)
{
    (void)index;
    te_emit(session, WORMINAL_PALETTE_QUERY, num, is_osc4, NULL, 0);
}

static void
te_strhandle(TermEngine *session)
{
	char *p = NULL, *dec;
	int j, narg, par;
	const struct { int idx; char *str; } osc_table[] = {
		{ defaultfg, "foreground" },
		{ defaultbg, "background" },
		{ defaultcs, "cursor" }
	};

	term.esc &= ~(ESC_STR_END|ESC_STR);
	te_strparse(session);
	par = (narg = strescseq.narg) ? atoi(strescseq.args[0]) : 0;

	switch (strescseq.type) {
	case ']': /* OSC -- Operating System Command */
		switch (par) {
		case 0:
			if (narg > 1) {
				te_xsettitle(session, strescseq.args[1]);
				te_xseticontitle(session, strescseq.args[1]);
			}
			return;
		case 1:
			if (narg > 1)
				te_xseticontitle(session, strescseq.args[1]);
			return;
		case 2:
			if (narg > 1)
				te_xsettitle(session, strescseq.args[1]);
			return;
		case 8: /* hyperlink: OSC 8 ; params ; URI */
			te_oschyperlink(session);
			return;
		case 52: /* manipulate selection data */
			if (narg > 2) {
				dec = base64dec(strescseq.args[2]);
				if (dec) {
					te_xsetsel(session, dec);
					te_xclipcopy(session);
				} else {
					te_diagnostic(session, "erresc: invalid base64\n");
				}
			}
			return;
		case 10: /* set dynamic VT100 text foreground color */
		case 11: /* set dynamic VT100 text background color */
		case 12: /* set dynamic text cursor color */
			if (narg < 2)
				break;
			p = strescseq.args[1];
			if ((j = par - 10) < 0 || j >= LEN(osc_table))
				break; /* shouldn't be possible */

			if (!strcmp(p, "?")) {
				te_osc_color_response(session, par, osc_table[j].idx, 0);
			} else if (te_xsetcolorname(session, osc_table[j].idx, p)) {
				te_diagnostic(session, "erresc: invalid %s color: %s\n",
				        osc_table[j].str, p);
			} else {
				te_tfulldirt(session);
			}
			return;
		case 4: /* color set */
			if (narg < 3)
				break;
			p = strescseq.args[2];
			/* FALLTHROUGH */
		case 104: /* color reset */
			j = (narg > 1) ? atoi(strescseq.args[1]) : -1;

			if (p && !strcmp(p, "?")) {
				te_osc_color_response(session, j, 0, 1);
			} else if (te_xsetcolorname(session, j, p)) {
				if (par == 104 && narg <= 1) {
					te_xloadcols(session);
					return; /* color reset without parameter */
				}
				te_diagnostic(session, "erresc: invalid color j=%d, p=%s\n",
				        j, p ? p : "(null)");
			} else {
				/*
				 * TODO if defaultbg color is changed, borders
				 * are dirty
				 */
				te_tfulldirt(session);
			}
			return;
		case 110: /* reset dynamic VT100 text foreground color */
		case 111: /* reset dynamic VT100 text background color */
		case 112: /* reset dynamic text cursor color */
			if (narg != 1)
				break;
			if ((j = par - 110) < 0 || j >= LEN(osc_table))
				break; /* shouldn't be possible */
			if (te_xsetcolorname(session, osc_table[j].idx, NULL)) {
				te_diagnostic(session, "erresc: %s color not found\n", osc_table[j].str);
			} else {
				te_tfulldirt(session);
			}
			return;
		}
		break;
	case 'k': /* old title set compatibility */
		te_xsettitle(session, strescseq.args[0]);
		return;
	case 'P': /* DCS -- Device Control String */
	case '_': /* APC -- Application Program Command */
	case '^': /* PM -- Privacy Message */
		return;
	}

	te_diagnostic(session, "erresc: unknown str ");
	te_strdump(session);
}

static void
te_strparse(TermEngine *session)
{
	int c;
	char *p = strescseq.buf;

	strescseq.narg = 0;
	strescseq.buf[strescseq.len] = '\0';

	if (*p == '\0')
		return;

	while (strescseq.narg < STR_ARG_SIZ) {
		strescseq.args[strescseq.narg++] = p;
		while ((c = *p) != ';' && c != '\0')
			++p;
		if (c == '\0')
			return;
		*p++ = '\0';
	}
}

static void
te_strdump(TermEngine *session)
{
	size_t i;
	uint c;

	te_diagnostic(session, "ESC%c", strescseq.type);
	for (i = 0; i < strescseq.len; i++) {
		c = strescseq.buf[i] & 0xff;
		if (c == '\0') {
			te_diagnostic(session, "%c", '\n');
			return;
		} else if (isprint(c)) {
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
	te_diagnostic(session, "ESC\\\n");
}

static void
te_strreset(TermEngine *session)
{
	strescseq = (STREscape){
		.buf = te_realloc(strescseq.buf, STR_BUF_SIZ),
		.siz = STR_BUF_SIZ,
	};
}

void
te_toggleprinter(TermEngine *session, const Arg *arg)
{
	term.mode ^= MODE_PRINT;
}

void
te_printscreen(TermEngine *session, const Arg *arg)
{
	te_tdump(session);
}

void
te_printsel(TermEngine *session, const Arg *arg)
{
	te_tdumpsel(session);
}

static void
te_tdumpsel(TermEngine *session)
{
	char *ptr;

	if ((ptr = te_getsel(session))) {
		te_tprinter(session, ptr, strlen(ptr));
		free(ptr);
	}
}

static void
te_tdumpline(TermEngine *session, int n)
{
	char buf[UTF_SIZ];
	const Glyph *bp, *end;

	bp = &term.line[n][0];
	end = &bp[MIN(te_tlinelen(session, n), term.col) - 1];
	if (bp != end || bp->u != ' ') {
		for ( ; bp <= end; ++bp)
			te_tprinter(session, buf, utf8encode(bp->u, buf));
	}
	te_tprinter(session, "\n", 1);
}

static void
te_tdump(TermEngine *session)
{
	int i;

	for (i = 0; i < term.row; ++i)
		te_tdumpline(session, i);
}

static void
te_tputtab(TermEngine *session, int n)
{
	uint x = term.c.x;

	if (n > 0) {
		while (x < term.col && n--)
			for (++x; x < term.col && !term.tabs[x]; ++x)
				/* nothing */ ;
	} else if (n < 0) {
		while (x > 0 && n++)
			for (--x; x > 0 && !term.tabs[x]; --x)
				/* nothing */ ;
	}
	term.c.x = LIMIT(x, 0, term.col-1);
}

static void
te_tdefutf8(TermEngine *session, char ascii)
{
	if (ascii == 'G')
		term.mode |= MODE_UTF8;
	else if (ascii == '@')
		term.mode &= ~MODE_UTF8;
}

static void
te_tdeftran(TermEngine *session, char ascii)
{
	static char cs[] = "0B";
	static int vcs[] = {CS_GRAPHIC0, CS_USA};
	char *p;

	if ((p = strchr(cs, ascii)) == NULL) {
		te_diagnostic(session, "esc unhandled charset: ESC ( %c\n", ascii);
	} else {
		term.trantbl[term.icharset] = vcs[p - cs];
	}
}

static void
te_tdectest(TermEngine *session, char c)
{
	int x, y;

	if (c == '8') { /* DEC screen alignment test. */
		for (x = 0; x < term.col; ++x) {
			for (y = 0; y < term.row; ++y)
				te_tsetchar(session, 'E', &term.c.attr, x, y);
		}
	}
}

static void
te_tstrsequence(TermEngine *session, uchar c)
{
	switch (c) {
	case 0x90:   /* DCS -- Device Control String */
		c = 'P';
		break;
	case 0x9f:   /* APC -- Application Program Command */
		c = '_';
		break;
	case 0x9e:   /* PM -- Privacy Message */
		c = '^';
		break;
	case 0x9d:   /* OSC -- Operating System Command */
		c = ']';
		break;
	}
	te_strreset(session);
	strescseq.type = c;
	term.esc |= ESC_STR;
}

static void
te_tcontrolcode(TermEngine *session, uchar ascii)
{
	switch (ascii) {
	case '\t':   /* HT */
		te_tputtab(session, 1);
		return;
	case '\b':   /* BS */
		te_tmoveto(session, term.c.x-1, term.c.y);
		return;
	case '\r':   /* CR */
		te_tmoveto(session, 0, term.c.y);
		return;
	case '\f':   /* LF */
	case '\v':   /* VT */
	case '\n':   /* LF */
		/* go to first col if the mode is set */
		te_tnewline(session, IS_SET(MODE_CRLF));
		return;
	case '\a':   /* BEL */
		if (term.esc & ESC_STR_END) {
			/* backwards compatibility to xterm */
			te_strhandle(session);
		} else {
			te_xbell(session);
		}
		break;
	case '\033': /* ESC */
		te_csireset(session);
		term.esc &= ~(ESC_CSI|ESC_ALTCHARSET|ESC_TEST);
		term.esc |= ESC_START;
		return;
	case '\016': /* SO (LS1 -- Locking shift 1) */
	case '\017': /* SI (LS0 -- Locking shift 0) */
		term.charset = 1 - (ascii - '\016');
		return;
	case '\032': /* SUB */
		te_tsetchar(session, '?', &term.c.attr, term.c.x, term.c.y);
		/* FALLTHROUGH */
	case '\030': /* CAN */
		te_csireset(session);
		break;
	case '\005': /* ENQ (IGNORED) */
	case '\000': /* NUL (IGNORED) */
	case '\021': /* XON (IGNORED) */
	case '\023': /* XOFF (IGNORED) */
	case 0177:   /* DEL (IGNORED) */
		return;
	case 0x80:   /* TODO: PAD */
	case 0x81:   /* TODO: HOP */
	case 0x82:   /* TODO: BPH */
	case 0x83:   /* TODO: NBH */
	case 0x84:   /* TODO: IND */
		break;
	case 0x85:   /* NEL -- Next line */
		te_tnewline(session, 1); /* always go to first col */
		break;
	case 0x86:   /* TODO: SSA */
	case 0x87:   /* TODO: ESA */
		break;
	case 0x88:   /* HTS -- Horizontal tab stop */
		term.tabs[term.c.x] = 1;
		break;
	case 0x89:   /* TODO: HTJ */
	case 0x8a:   /* TODO: VTS */
	case 0x8b:   /* TODO: PLD */
	case 0x8c:   /* TODO: PLU */
	case 0x8d:   /* TODO: RI */
	case 0x8e:   /* TODO: SS2 */
	case 0x8f:   /* TODO: SS3 */
	case 0x91:   /* TODO: PU1 */
	case 0x92:   /* TODO: PU2 */
	case 0x93:   /* TODO: STS */
	case 0x94:   /* TODO: CCH */
	case 0x95:   /* TODO: MW */
	case 0x96:   /* TODO: SPA */
	case 0x97:   /* TODO: EPA */
	case 0x98:   /* TODO: SOS */
	case 0x99:   /* TODO: SGCI */
		break;
	case 0x9a:   /* DECID -- Identify Terminal */
		te_ttywrite(session, vtiden, strlen(vtiden), 0);
		break;
	case 0x9b:   /* TODO: CSI */
	case 0x9c:   /* TODO: ST */
		break;
	case 0x90:   /* DCS -- Device Control String */
	case 0x9d:   /* OSC -- Operating System Command */
	case 0x9e:   /* PM -- Privacy Message */
	case 0x9f:   /* APC -- Application Program Command */
		te_tstrsequence(session, ascii);
		return;
	}
	/* only CAN, SUB, \a and C1 chars interrupt a sequence */
	term.esc &= ~(ESC_STR_END|ESC_STR);
}

static int
te_eschandle(TermEngine *session, uchar ascii)
{
	switch (ascii) {
	case '[':
		term.esc |= ESC_CSI;
		return 0;
	case '#':
		term.esc |= ESC_TEST;
		return 0;
	case '%':
		term.esc |= ESC_UTF8;
		return 0;
	case 'P': /* DCS -- Device Control String */
	case '_': /* APC -- Application Program Command */
	case '^': /* PM -- Privacy Message */
	case ']': /* OSC -- Operating System Command */
	case 'k': /* old title set compatibility */
		te_tstrsequence(session, ascii);
		return 0;
	case 'n': /* LS2 -- Locking shift 2 */
	case 'o': /* LS3 -- Locking shift 3 */
		term.charset = 2 + (ascii - 'n');
		break;
	case '(': /* GZD4 -- set primary charset G0 */
	case ')': /* G1D4 -- set secondary charset G1 */
	case '*': /* G2D4 -- set tertiary charset G2 */
	case '+': /* G3D4 -- set quaternary charset G3 */
		term.icharset = ascii - '(';
		term.esc |= ESC_ALTCHARSET;
		return 0;
	case 'D': /* IND -- Linefeed */
		if (term.c.y == term.bot) {
			te_tscrollup(session, term.top, 1);
		} else {
			te_tmoveto(session, term.c.x, term.c.y+1);
		}
		break;
	case 'E': /* NEL -- Next line */
		te_tnewline(session, 1); /* always go to first col */
		break;
	case 'H': /* HTS -- Horizontal tab stop */
		term.tabs[term.c.x] = 1;
		break;
	case 'M': /* RI -- Reverse index */
		if (term.c.y == term.top) {
			te_tscrolldown(session, term.top, 1);
		} else {
			te_tmoveto(session, term.c.x, term.c.y-1);
		}
		break;
	case 'Z': /* DECID -- Identify Terminal */
		te_ttywrite(session, vtiden, strlen(vtiden), 0);
		break;
	case 'c': /* RIS -- Reset to initial state */
		te_treset(session);
		te_resettitle(session);
		te_xloadcols(session);
		te_xsetmode(session, 0, MODE_HIDE);
		te_xsetmode(session, 0, MODE_BRCKTPASTE);
		te_xsetmode(session, 0, MODE_ALTSCROLL);
		break;
	case '=': /* DECPAM -- Application keypad */
		te_xsetmode(session, 1, MODE_APPKEYPAD);
		break;
	case '>': /* DECPNM -- Normal keypad */
		te_xsetmode(session, 0, MODE_APPKEYPAD);
		break;
	case '7': /* DECSC -- Save Cursor */
		te_tcursor(session, CURSOR_SAVE);
		break;
	case '8': /* DECRC -- Restore Cursor */
		te_tcursor(session, CURSOR_LOAD);
		break;
	case '\\': /* ST -- String Terminator */
		if (term.esc & ESC_STR_END)
			te_strhandle(session);
		break;
	default:
		te_diagnostic(session, "erresc: unknown sequence ESC 0x%02X '%c'\n",
			(uchar) ascii, isprint(ascii)? ascii:'.');
		break;
	}
	return 1;
}

static void
te_tputc(TermEngine *session, Rune u)
{
	char c[UTF_SIZ];
	int control;
	int width, len;
	Glyph *gp;

	control = ISCONTROL(u);
	if (u < 127 || !IS_SET(MODE_UTF8)) {
		c[0] = u;
		width = len = 1;
	} else {
		len = utf8encode(u, c);
		if (!control && (width = wcwidth(u)) == -1)
			width = 1;
	}

	if (IS_SET(MODE_PRINT))
		te_tprinter(session, c, len);

	/*
	 * STR sequence must be checked before anything else
	 * because it uses all following characters until it
	 * receives a ESC, a SUB, a ST or any other C1 control
	 * character.
	 */
	if (term.esc & ESC_STR) {
		if (u == '\a' || u == 030 || u == 032 || u == 033 ||
		   ISCONTROLC1(u)) {
			term.esc &= ~(ESC_START|ESC_STR);
			term.esc |= ESC_STR_END;
			goto check_control_code;
		}

		if (strescseq.len+len >= strescseq.siz) {
			/*
			 * Here is a bug in terminals. If the user never sends
			 * some code to stop the str or esc command, then st
			 * will stop responding. But this is better than
			 * silently failing with unknown characters. At least
			 * then users will report back.
			 *
			 * In the case users ever get fixed, here is the code:
			 */
			/*
			 * term.esc = 0;
			 * te_strhandle(session);
			 */
			if (strescseq.siz > (SIZE_MAX - UTF_SIZ) / 2)
				return;
			strescseq.siz *= 2;
			strescseq.buf = te_realloc(strescseq.buf, strescseq.siz);
		}

		memmove(&strescseq.buf[strescseq.len], c, len);
		strescseq.len += len;
		return;
	}

check_control_code:
	/*
	 * Actions of control codes must be performed as soon they arrive
	 * because they can be embedded inside a control sequence, and
	 * they must not cause conflicts with sequences.
	 */
	if (control) {
		/* in UTF-8 mode ignore handling C1 control characters */
		if (IS_SET(MODE_UTF8) && ISCONTROLC1(u))
			return;
		te_tcontrolcode(session, u);
		/*
		 * control codes are not shown ever
		 */
		if (!term.esc)
			term.lastc = 0;
		return;
	} else if (term.esc & ESC_START) {
		if (term.esc & ESC_CSI) {
			csiescseq.buf[csiescseq.len++] = u;
			if (BETWEEN(u, 0x40, 0x7E)
					|| csiescseq.len >= \
					sizeof(csiescseq.buf)-1) {
				term.esc = 0;
				te_csiparse(session);
				te_csihandle(session);
			}
			return;
		} else if (term.esc & ESC_UTF8) {
			te_tdefutf8(session, u);
		} else if (term.esc & ESC_ALTCHARSET) {
			te_tdeftran(session, u);
		} else if (term.esc & ESC_TEST) {
			te_tdectest(session, u);
		} else {
			if (!te_eschandle(session, u))
				return;
			/* sequence already finished */
		}
		term.esc = 0;
		/*
		 * All characters which form part of a sequence are not
		 * printed
		 */
		return;
	}
	if (te_selected(session, term.c.x, term.c.y))
		te_selclear(session);

	gp = &term.line[term.c.y][term.c.x];
	if (IS_SET(MODE_WRAP) && (term.c.state & CURSOR_WRAPNEXT)) {
		gp->mode |= ATTR_WRAP;
		te_tnewline(session, 1);
		gp = &term.line[term.c.y][term.c.x];
	}

	if (IS_SET(MODE_INSERT) && term.c.x+width < term.col) {
		memmove(gp+width, gp, (term.col - term.c.x - width) * sizeof(Glyph));
		gp->mode &= ~ATTR_WIDE;
	}

	if (term.c.x+width > term.col) {
		if (IS_SET(MODE_WRAP))
			te_tnewline(session, 1);
		else
			te_tmoveto(session, term.col - width, term.c.y);
		gp = &term.line[term.c.y][term.c.x];
	}

	te_tsetchar(session, u, &term.c.attr, term.c.x, term.c.y);
	term.lastc = u;

	if (width == 2) {
		gp->mode |= ATTR_WIDE;
		if (term.c.x+1 < term.col) {
			if (gp[1].mode == ATTR_WIDE && term.c.x+2 < term.col) {
				gp[2].u = ' ';
				gp[2].mode &= ~ATTR_WDUMMY;
			}
			gp[1].u = '\0';
			gp[1].mode = ATTR_WDUMMY;
			gp[1].link = gp->link;
		}
	}
	if (term.c.x+width < term.col) {
		te_tmoveto(session, term.c.x+width, term.c.y);
	} else {
		term.c.state |= CURSOR_WRAPNEXT;
	}
}

int
te_twrite(TermEngine *session, const char *buf, int buflen, int show_ctrl)
{
	int charsize;
	Rune u;
	int n;

	for (n = 0; n < buflen; n += charsize) {
		if (IS_SET(MODE_UTF8)) {
			/* process a complete utf8 char */
			charsize = utf8decode(buf + n, &u, buflen - n);
			if (charsize == 0)
				break;
		} else {
			u = buf[n] & 0xFF;
			charsize = 1;
		}
		if (show_ctrl && ISCONTROL(u)) {
			if (u & 0x80) {
				u &= 0x7f;
				te_tputc(session, '^');
				te_tputc(session, '[');
			} else if (u != '\n' && u != '\r' && u != '\t') {
				u ^= 0x40;
				te_tputc(session, '^');
			}
		}
		te_tputc(session, u);
		if (session->events) return n + charsize;
	}
	return n;
}
