/* See LICENSE for license details. */
#define _GNU_SOURCE
#include <errno.h>
#include <math.h>
#include <limits.h>
#include <stdint.h>
#include <locale.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
#include <libgen.h>
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <X11/cursorfont.h>
#include <X11/keysym.h>
#include <X11/Xft/Xft.h>
#include <X11/XKBlib.h>

char *argv0;
#include "arg.h"
#include "st.h"
#include "win.h"
#include "session_view.h"
#include "url.h"
#include "wire.h"
#include "workspace_client.h"

/* types used in config.h */
typedef struct {
	uint mod;
	KeySym keysym;
	void (*func)(const Arg *);
	const Arg arg;
} Shortcut;

typedef struct {
	uint mod;
	uint button;
	void (*func)(const Arg *);
	const Arg arg;
	uint  release;
} MouseShortcut;

typedef struct {
	KeySym k;
	uint mask;
	char *s;
	/* three-valued logic variables: 0 indifferent, 1 on, -1 off */
	signed char appkey;    /* application keypad */
	signed char appcursor; /* application cursor */
} Key;

/* X modifiers */
#define XK_ANY_MOD    UINT_MAX
#define XK_NO_MOD     0
#define XK_SWITCH_MOD (1<<13|1<<14)

/* function definitions used in config.h */
static void clipcopy(const Arg *);
static void clippaste(const Arg *);
static void numlock(const Arg *);
static void selpaste(const Arg *);
static void zoom(const Arg *);
static void zoomabs(const Arg *);
static void zoomreset(const Arg *);
static void ttysend(const Arg *);

/* config.h for applying patches and the configuration. */
#include "config.h"
#include "icon.h"

/* XEMBED messages */
#define XEMBED_FOCUS_IN  4
#define XEMBED_FOCUS_OUT 5

/* macros */
#define IS_SET(flag)		((current_window.mode & (flag)) != 0)
#define TRUERED(x)		(((x) & 0xff0000) >> 8)
#define TRUEGREEN(x)		(((x) & 0xff00))
#define TRUEBLUE(x)		(((x) & 0xff) << 8)

typedef XftDraw *Draw;
typedef XftColor Color;
typedef XftGlyphFontSpec GlyphFontSpec;

/* Purely graphic info */
typedef struct {
	int tw, th; /* tty width and height */
	int w, h; /* window width and height */
	int ch; /* char height */
	int cw; /* char width  */
	int mode; /* window state/mode flags */
	int cursor; /* cursor style */
} TermWindow;

typedef struct {
	Display *dpy;
	Colormap cmap;
	Window win;
	Drawable buf;
	GlyphFontSpec *specbuf; /* font spec buffer used for rendering */
	int spec_cols;
	Atom xembed, wmdeletewin, netwmname, netwmiconname, netwmpid;
	struct {
		XIC xic;
		XPoint spot;
		XVaNestedList spotlist;
	} ime;
	Draw draw;
	Visual *vis;
	XSetWindowAttributes attrs;
	int scr;
	int isfixed; /* is fixed geometry? */
	int l, t; /* left and top offset */
	int gm; /* geometry mask */
} XWindow;

typedef struct {
	Atom xtarget;
	char *primary, *clipboard;
	struct timespec tclick1;
	struct timespec tclick2;
} XSelection;

/* Font structure */
#define Font Font_
typedef struct {
	int height;
	int width;
	int ascent;
	int descent;
	int badslant;
	int badweight;
	short lbearing;
	short rbearing;
	XftFont *match;
	FcFontSet *set;
	FcPattern *pattern;
} Font;

/* Drawing Context */
typedef struct {
	Color *col;
	unsigned char *colloaded;
	size_t collen;
	Font font, bfont, ifont, ibfont;
	GC gc;
	Color border;
} DC;

static inline ushort sixd_to_16bit(int);
static int xmakeglyphfontspecs(XftGlyphFontSpec *, const Glyph *, int, int, int);
static void xdrawglyphfontspecs(const XftGlyphFontSpec *, Glyph, int, int, int, int);
static void xdrawglyph(Glyph, int, int);
static void xclear(int, int, int, int);
static int xgeommasktogravity(int);
static int ximopen(Display *);
static void ximinstantiate(Display *, XPointer, XPointer);
static void ximdestroy(XIM, XPointer, XPointer);
static int xicdestroy(XIC, XPointer, XPointer);
static void xinit(int, int);
static void xinitview(int, int, int, int);
static void xactivate(void);
static void cresize(int, int);
static void xresize(int, int);
static void xhints(void);
static void xidentityhints(void);
static int xloadcolor(int, const char *, Color *);
static Color *xcolor(int);
static int xloadfont(Font *, FcPattern *);
static Font *xgetfont(ushort);
static void xloadfonts(const char *, double);
static void xunloadfont(Font *);
static void xunloadfonts(void);
static void xsetenv(void);
static void xseturgency(int);
static int evcol(XEvent *);
static int evrow(XEvent *);

static void expose(XEvent *);
static void visibility(XEvent *);
static void unmap(XEvent *);
static void destroy(XEvent *);
static void kpress(XEvent *);
static void cmessage(XEvent *);
static void resize(XEvent *);
static void focus(XEvent *);
static uint buttonmask(uint);
static int mouseaction(XEvent *, uint);
static void brelease(XEvent *);
static void bpress(XEvent *);
static void bmotion(XEvent *);
static void leave(XEvent *);
static void propnotify(XEvent *);
static void selnotify(XEvent *);
static void selclear_(XEvent *);
static void selrequest(XEvent *);
static void setsel(char *, Time);
static void mousesel(XEvent *, int);
static void mousereport(XEvent *);
static char *kmap(KeySym, uint);
static int match(uint, uint);

static void run(void);
static void usage(void);

static void (*handler[LASTEvent])(XEvent *) = {
	[KeyPress] = kpress,
	[ClientMessage] = cmessage,
	[ConfigureNotify] = resize,
	[VisibilityNotify] = visibility,
	[UnmapNotify] = unmap,
	[DestroyNotify] = destroy,
	[Expose] = expose,
	[FocusIn] = focus,
	[FocusOut] = focus,
	[MotionNotify] = bmotion,
	[LeaveNotify] = leave,
	[ButtonPress] = bpress,
	[ButtonRelease] = brelease,
/*
 * Uncomment if you want the selection to disappear when you select something
 * different in another window.
 */
/*	[SelectionClear] = selclear_, */
	[SelectionNotify] = selnotify,
/*
 * PropertyNotify is only turned on when there is some INCR transfer happening
 * for the selection retrieval.
 */
	[PropertyNotify] = propnotify,
	[SelectionRequest] = selrequest,
};

/* Font Ring Cache */
enum {
	FRC_NORMAL,
	FRC_ITALIC,
	FRC_BOLD,
	FRC_ITALICBOLD
};

typedef struct {
	XftFont *font;
	int flags;
	Rune unicodep;
} Fontcache;

/* A view owns its window, drawing state, font cache, and selection. */
typedef struct XView {
	DC dc;
	XWindow xw;
	XSelection xsel;
	TermWindow win;
	TermSession *terminal;
	Fontcache *fallback_fonts;
	int fallback_len, fallback_cap;
	char *font_name;
	FcPattern *style_pattern;
	double font_size, default_font_size;
	uint buttons;
	char *url_click;
	int url_col, url_row;
	UrlSpan hover;
	int hover_active, hover_inside, hover_col, hover_row;
	int live;
	struct XView *next;
} XView;

typedef struct Tab {
	uint32_t id;
	TermSession *terminal;
	char *title;
	char *initial_title;
	char *directory;
	char **colors;
	int mode, cursor;
	struct Tab *next;
} Tab;

static XView primaryview;
static XView *views = &primaryview;
static XView *view = &primaryview;
static Tab *tabs, *lasttab;
static Display *shared_display;
/* One XIM serves the display; each view has its own XIC. Opening a second
 * XIM on the same display fails with the KDE input method in practice. */
static XIM sharedxim;
static int ximwaiting;
static XView *
xviewfor(Window window)
{
	XView *candidate;

	for (candidate = views; candidate; candidate = candidate->next)
		if (candidate->xw.win == window)
			return candidate;
	return NULL;
}
#define dc (view->dc)
#define xw (view->xw)
#define xsel (view->xsel)
#define current_window (view->win)
#define buttons (view->buttons)
#define frc (view->fallback_fonts)
#define frclen (view->fallback_len)
#define frccap (view->fallback_cap)
#define usedfont (view->font_name)
#define stylepattern (view->style_pattern)
#define usedfontsize (view->font_size)
#define defaultfontsize (view->default_font_size)

static void
xsetview(XView *chosen)
{
	view = chosen;
	if (chosen->terminal)
		tsessionuse(chosen->terminal);
}

static Tab *
xtabfor(TermSession *terminal)
{
	Tab *tab;
	for (tab = tabs; tab; tab = tab->next)
		if (tab->terminal == terminal)
			return tab;
	return NULL;
}

static Tab *
xtabnew(TermSession *terminal, const char *title, const char *cwd)
{
	Tab *tab = xmalloc(sizeof(*tab));
	char *directory = cwd ? xstrdup(cwd) : getcwd(NULL, 0);
	memset(tab, 0, sizeof(*tab));
	tab->terminal = terminal;
	tab->title = xstrdup(title ? title : "Worminal");
	tab->initial_title = xstrdup(title ? title : "Worminal");
	tab->directory = directory ? directory : xstrdup("?");
	tab->cursor = cursorshape;
	if (lasttab)
		lasttab->next = tab;
	else
		tabs = tab;
	lasttab = tab;
	return tab;
}

static char *opt_class = NULL;
static char **opt_cmd  = NULL;
static char *opt_embed = NULL;
static char *opt_font  = NULL;
static char *opt_io    = NULL;
static char *opt_line  = NULL;
static char *opt_name  = NULL;
static char *opt_title = NULL;
static int workspacefd = -1;
static const char *master_target;
static XView *pending_view;
static int pending_new_tab;
static int launch_argc;
static const char *requested_cwd;
static char **requested_env;
static int requested_geometry;
static int requested_cols, requested_rows, requested_x, requested_y, requested_gm;
static int requested_fixed;
static int requested_allowalt = 1;
static char launch_windowid[32];
extern char **environ;
static Window initializing_window;
static int initializing_window_gone;
static int (*previous_xerror)(Display *, XErrorEvent *);
static Tab *xtabfor(TermSession *);
static void xdrawtabs(void);
static void xrefreshtabs(void);
static Tab *xfirsttab(void);
static int xtabwidth(Tab *);
static void xselecttab(Tab *);
static Tab *xtabslot(int);
static void xclosetab(Tab *);
static void xaddview(void);
static XView *xlivefor(TermSession *);
static void workspace_send_launch(uint32_t, const char *, int, int);
static void workspace_message(WirePacket *);
static void workspace_focus(void);

static int
xinitialerror(Display *display, XErrorEvent *error)
{
	if (error->resourceid == initializing_window &&
	    (error->error_code == BadWindow || error->error_code == BadDrawable)) {
		initializing_window_gone = 1;
		return 0;
	}
	if (previous_xerror)
		return previous_xerror(display, error);
	die("unexpected X error during window initialization: %d\n",
	    error->error_code);
	return 0;
}

static void
workspace_send_launch(uint32_t type, const char *directory, int columns, int lines)
{
	WireBuffer buffer = {0};
	char *cwd = directory ? NULL : getcwd(NULL, 0);
	char windowid[32];
	uint32_t argc = type == WIRE_HELLO ? launch_argc : 0;
	uint32_t envc = 0;
	if (!master_target)
		for (char **entry = environ; *entry; entry++)
			envc++;
	snprintf(windowid, sizeof(windowid), "%lu", xw.win);
	wire_put_u32(&buffer, columns);
	wire_put_u32(&buffer, lines);
	wire_put_u32(&buffer, allowaltscreen);
	wire_put_u32(&buffer, argc);
	wire_put_u32(&buffer, envc);
	wire_put_string(&buffer, type == WIRE_HELLO ? opt_title : "Worminal");
	wire_put_string(&buffer, directory ? directory : master_target ? "" : cwd);
	wire_put_string(&buffer, type == WIRE_HELLO ? opt_line : NULL);
	wire_put_string(&buffer, type == WIRE_HELLO ? opt_io : NULL);
	wire_put_string(&buffer, master_target ? NULL : windowid);
	for (uint32_t i = 0; i < argc; i++)
		wire_put_string(&buffer, opt_cmd[i]);
	if (!master_target)
		for (char **entry = environ; *entry; entry++)
			wire_put_string(&buffer, *entry);
	if (!wire_write(workspacefd, type, 0, buffer.data, buffer.len))
		die("workspace connection closed during launch\n");
	wire_buffer_free(&buffer);
	free(cwd);
}

static void
workspace_focus(void)
{
	Tab *tab = xtabfor(view->terminal);
	WireBuffer buffer = {0};
	if (!tab || !tab->id)
		return;
	wire_put_u32(&buffer, MAX(1, current_window.tw / current_window.cw));
	wire_put_u32(&buffer, MAX(1, current_window.th / current_window.ch));
	wire_write(workspacefd, WIRE_FOCUS, tab->id, buffer.data, buffer.len);
	wire_buffer_free(&buffer);
}

static void
workspace_write(const char *bytes, size_t length, int echo)
{
	Tab *tab = xtabfor(tsessioncurrent());
	(void)echo;
	if (!view->live)
		xactivate();
	if (tab && tab->id)
		wire_write(workspacefd, WIRE_INPUT, tab->id, bytes, length);
}

static void
workspace_scroll(int up, int amount)
{
	Tab *tab = xtabfor(tsessioncurrent());
	WireBuffer buffer = {0};
	if (!tab || !tab->id)
		return;
	wire_put_u32(&buffer, up);
	wire_put_u32(&buffer, amount);
	wire_write(workspacefd, WIRE_SCROLL, tab->id, buffer.data, buffer.len);
	wire_buffer_free(&buffer);
}

static void
xstartupevent(const char *event, int value)
{
	if (getenv("WORMINAL_TRACE_STARTUP"))
		fprintf(stderr, "worminal-startup %s %d\n", event, value);
}

static void
xstartuptime(const char *event)
{
	struct timespec now;

	if (!getenv("WORMINAL_TRACE_TIMING"))
		return;
	clock_gettime(CLOCK_MONOTONIC, &now);
	fprintf(stderr, "worminal-timing %s %lld\n", event,
	        (long long)now.tv_sec * 1000000000LL + now.tv_nsec);
}

void
clipcopy(const Arg *dummy)
{
	Atom clipboard;

	free(xsel.clipboard);
	xsel.clipboard = NULL;

	if (xsel.primary != NULL) {
		xsel.clipboard = xstrdup(xsel.primary);
		clipboard = XInternAtom(xw.dpy, "CLIPBOARD", 0);
		XSetSelectionOwner(xw.dpy, clipboard, xw.win, CurrentTime);
	}
}

void
clippaste(const Arg *dummy)
{
	Atom clipboard;

	clipboard = XInternAtom(xw.dpy, "CLIPBOARD", 0);
	XConvertSelection(xw.dpy, clipboard, xsel.xtarget, clipboard,
			xw.win, CurrentTime);
}

void
selpaste(const Arg *dummy)
{
	XConvertSelection(xw.dpy, XA_PRIMARY, xsel.xtarget, XA_PRIMARY,
			xw.win, CurrentTime);
}

void
numlock(const Arg *dummy)
{
	current_window.mode ^= MODE_NUMLOCK;
}

void
zoom(const Arg *arg)
{
	Arg larg;

	larg.f = usedfontsize + arg->f;
	zoomabs(&larg);
}

void
zoomabs(const Arg *arg)
{
	xunloadfonts();
	xloadfonts(usedfont, arg->f);
	cresize(0, 0);
	redraw();
	xhints();
}

void
zoomreset(const Arg *arg)
{
	Arg larg;

	if (defaultfontsize > 0) {
		larg.f = defaultfontsize;
		zoomabs(&larg);
	}
}

void
ttysend(const Arg *arg)
{
	ttywrite(arg->s, strlen(arg->s), 1);
}

int
evcol(XEvent *e)
{
	int x = e->xbutton.x - borderpx;
	LIMIT(x, 0, current_window.tw - 1);
	return x / current_window.cw;
}

int
evrow(XEvent *e)
{
	int y = e->xbutton.y - borderpx - current_window.ch;
	LIMIT(y, 0, current_window.th - 1);
	return y / current_window.ch;
}

void
mousesel(XEvent *e, int done)
{
	int type, seltype = SEL_REGULAR;
	uint state = e->xbutton.state & ~(Button1Mask | forcemousemod);

	for (type = 1; type < LEN(selmasks); ++type) {
		if (match(selmasks[type], state)) {
			seltype = type;
			break;
		}
	}
	selextend(evcol(e), evrow(e), seltype, done);
	if (done)
		setsel(getsel(), e->xbutton.time);
}

void
mousereport(XEvent *e)
{
	int len, btn, code;
	int x = evcol(e), y = evrow(e);
	int state = e->xbutton.state;
	char buf[40];
	static int ox, oy;

	if (e->type == MotionNotify) {
		if (x == ox && y == oy)
			return;
		if (!IS_SET(MODE_MOUSEMOTION) && !IS_SET(MODE_MOUSEMANY))
			return;
		/* MODE_MOUSEMOTION: no reporting if no button is pressed */
		if (IS_SET(MODE_MOUSEMOTION) && buttons == 0)
			return;
		/* Set btn to lowest-numbered pressed button, or 12 if no
		 * buttons are pressed. */
		for (btn = 1; btn <= 11 && !(buttons & (1<<(btn-1))); btn++)
			;
		code = 32;
	} else {
		btn = e->xbutton.button;
		/* Only buttons 1 through 11 can be encoded */
		if (btn < 1 || btn > 11)
			return;
		if (e->type == ButtonRelease) {
			/* MODE_MOUSEX10: no button release reporting */
			if (IS_SET(MODE_MOUSEX10))
				return;
			/* Don't send release events for the scroll wheel */
			if (btn == 4 || btn == 5)
				return;
		}
		code = 0;
	}

	ox = x;
	oy = y;

	/* Encode btn into code. If no button is pressed for a motion event in
	 * MODE_MOUSEMANY, then encode it as a release. */
	if ((!IS_SET(MODE_MOUSESGR) && e->type == ButtonRelease) || btn == 12)
		code += 3;
	else if (btn >= 8)
		code += 128 + btn - 8;
	else if (btn >= 4)
		code += 64 + btn - 4;
	else
		code += btn - 1;

	if (!IS_SET(MODE_MOUSEX10)) {
		code += ((state & ShiftMask  ) ?  4 : 0)
		      + ((state & Mod1Mask   ) ?  8 : 0) /* meta key: alt */
		      + ((state & ControlMask) ? 16 : 0);
	}

	if (IS_SET(MODE_MOUSESGR)) {
		len = snprintf(buf, sizeof(buf), "\033[<%d;%d;%d%c",
				code, x+1, y+1,
				e->type == ButtonRelease ? 'm' : 'M');
	} else if (x < 223 && y < 223) {
		len = snprintf(buf, sizeof(buf), "\033[M%c%c%c",
				32+code, 32+x+1, 32+y+1);
	} else {
		return;
	}

	ttywrite(buf, len, 0);
}

uint
buttonmask(uint button)
{
	return button == Button1 ? Button1Mask
	     : button == Button2 ? Button2Mask
	     : button == Button3 ? Button3Mask
	     : button == Button4 ? Button4Mask
	     : button == Button5 ? Button5Mask
	     : 0;
}

int
mouseaction(XEvent *e, uint release)
{
	MouseShortcut *ms;

	/* ignore Button<N>mask for Button<N> - it's set on release */
	uint state = e->xbutton.state & ~buttonmask(e->xbutton.button);

	for (ms = mshortcuts; ms < mshortcuts + LEN(mshortcuts); ms++) {
		if (ms->release == release &&
		    ms->button == e->xbutton.button &&
		    (match(ms->mod, state) ||  /* exact or forced */
		     match(ms->mod, state & ~forcemousemod))) {
			ms->func(&(ms->arg));
			return 1;
		}
	}

	return 0;
}

static char *
xurlat(int col, int row, UrlSpan *span)
{
	SessionFrame frame = tsessionframe(view->terminal);
	Line *lines = xmalloc(frame.rows * sizeof(*lines));
	char *url;
	for (int y = 0; y < frame.rows; y++)
		lines[y] = tsessionviewline(view->terminal, y);
	url = urlat(lines, frame.rows, frame.cols, col, row, span);
	free(lines);
	return url;
}

static void
xclearhover(void)
{
	if (!view->hover_active)
		return;
	tsetdirt(view->hover.start_y, view->hover.end_y);
	view->hover_active = 0;
}

static void
xupdatehover(void)
{
	UrlSpan span;
	char *url = view->hover_inside ? xurlat(view->hover_col, view->hover_row, &span) : NULL;
	if (url && view->hover_active &&
	    span.start_x == view->hover.start_x && span.start_y == view->hover.start_y &&
	    span.end_x == view->hover.end_x && span.end_y == view->hover.end_y) {
		free(url);
		return;
	}
	xclearhover();
	if (url) {
		view->hover = span;
		view->hover_active = 1;
		tsetdirt(span.start_y, span.end_y);
		free(url);
	}
}

void
bpress(XEvent *e)
{
	if (e->xbutton.y >= borderpx &&
	    e->xbutton.y < borderpx + current_window.ch) {
		if (e->xbutton.button == Button1) {
			Tab *tab;
			int x = borderpx, width;
			for (tab = xfirsttab(); tab; tab = tab->next) {
				width = xtabwidth(tab);
				if (e->xbutton.x >= x &&
				    e->xbutton.x < MIN(x + width, current_window.w - borderpx)) {
					xselecttab(tab);
					break;
				}
				x += width;
				if (x >= current_window.w - borderpx)
					break;
			}
		}
		return;
	}
	xactivate();
	int btn = e->xbutton.button;
	char *key;
	struct timespec now;
	int snap;

	if (1 <= btn && btn <= 11)
		buttons |= 1 << (btn-1);
	if (btn == Button1 && (e->xbutton.state & ControlMask) &&
	    !(e->xbutton.state & ShiftMask) &&
	    (view->url_click = xurlat(evcol(e), evrow(e), NULL))) {
		view->url_col = evcol(e);
		view->url_row = evrow(e);
		return;
	}

	if (IS_SET(MODE_MOUSE) && !(e->xbutton.state & forcemousemod)) {
		mousereport(e);
		return;
	}
	/* With mouse reporting off, xterm alternate scroll maps the wheel to
	 * cursor keys only on the alternate screen. */
	if (IS_SET(MODE_ALTSCROLL) && tisaltscr() &&
	    !(e->xbutton.state & forcemousemod) &&
	    (btn == Button4 || btn == Button5)) {
		key = kmap(btn == Button4 ? XK_Up : XK_Down, 0);
		if (key)
			ttywrite(key, strlen(key), 1);
		return;
	}

	if (mouseaction(e, 0))
		return;

	if (btn == Button1) {
		/*
		 * If the user clicks below predefined timeouts specific
		 * snapping behaviour is exposed.
		 */
		clock_gettime(CLOCK_MONOTONIC, &now);
		if (TIMEDIFF(now, xsel.tclick2) <= tripleclicktimeout) {
			snap = SNAP_LINE;
		} else if (TIMEDIFF(now, xsel.tclick1) <= doubleclicktimeout) {
			snap = SNAP_WORD;
		} else {
			snap = 0;
		}
		xsel.tclick2 = xsel.tclick1;
		xsel.tclick1 = now;

		selstart(evcol(e), evrow(e), snap);
	}
}

void
propnotify(XEvent *e)
{
	XPropertyEvent *xpev;
	Atom clipboard = XInternAtom(xw.dpy, "CLIPBOARD", 0);

	xpev = &e->xproperty;
	if (xpev->state == PropertyNewValue &&
			(xpev->atom == XA_PRIMARY ||
			 xpev->atom == clipboard)) {
		selnotify(e);
	}
}

void
selnotify(XEvent *e)
{
	ulong nitems, ofs, rem;
	int format;
	uchar *data, *last, *repl;
	Atom type, incratom, property = None;

	incratom = XInternAtom(xw.dpy, "INCR", 0);

	ofs = 0;
	if (e->type == SelectionNotify)
		property = e->xselection.property;
	else if (e->type == PropertyNotify)
		property = e->xproperty.atom;

	if (property == None)
		return;

	do {
		if (XGetWindowProperty(xw.dpy, xw.win, property, ofs,
					BUFSIZ/4, False, AnyPropertyType,
					&type, &format, &nitems, &rem,
					&data)) {
			fprintf(stderr, "Clipboard allocation failed\n");
			return;
		}

		if (e->type == PropertyNotify && nitems == 0 && rem == 0) {
			/*
			 * If there is some PropertyNotify with no data, then
			 * this is the signal of the selection owner that all
			 * data has been transferred. We won't need to receive
			 * PropertyNotify events anymore.
			 */
			MODBIT(xw.attrs.event_mask, 0, PropertyChangeMask);
			XChangeWindowAttributes(xw.dpy, xw.win, CWEventMask,
					&xw.attrs);
		}

		if (type == incratom) {
			/*
			 * Activate the PropertyNotify events so we receive
			 * when the selection owner does send us the next
			 * chunk of data.
			 */
			MODBIT(xw.attrs.event_mask, 1, PropertyChangeMask);
			XChangeWindowAttributes(xw.dpy, xw.win, CWEventMask,
					&xw.attrs);

			/*
			 * Deleting the property is the transfer start signal.
			 */
			XDeleteProperty(xw.dpy, xw.win, (int)property);
			continue;
		}

		/*
		 * As seen in getsel:
		 * Line endings are inconsistent in the terminal and GUI world
		 * copy and pasting. When receiving some selection data,
		 * replace all '\n' with '\r'.
		 * FIXME: Fix the computer world.
		 */
		repl = data;
		last = data + nitems * format / 8;
		while ((repl = memchr(repl, '\n', last - repl))) {
			*repl++ = '\r';
		}

		if (IS_SET(MODE_BRCKTPASTE) && ofs == 0)
			ttywrite("\033[200~", 6, 0);
		ttywrite((char *)data, nitems * format / 8, 1);
		if (IS_SET(MODE_BRCKTPASTE) && rem == 0)
			ttywrite("\033[201~", 6, 0);
		XFree(data);
		/* number of 32-bit chunks returned */
		ofs += nitems * format / 32;
	} while (rem > 0);

	/*
	 * Deleting the property again tells the selection owner to send the
	 * next data chunk in the property.
	 */
	XDeleteProperty(xw.dpy, xw.win, (int)property);
}

void
xclipcopy(void)
{
	clipcopy(NULL);
}

void
selclear_(XEvent *e)
{
	selclear();
}

void
selrequest(XEvent *e)
{
	XSelectionRequestEvent *xsre;
	XSelectionEvent xev;
	Atom xa_targets, string, clipboard;
	char *seltext;

	xsre = (XSelectionRequestEvent *) e;
	xev.type = SelectionNotify;
	xev.requestor = xsre->requestor;
	xev.selection = xsre->selection;
	xev.target = xsre->target;
	xev.time = xsre->time;
	if (xsre->property == None)
		xsre->property = xsre->target;

	/* reject */
	xev.property = None;

	xa_targets = XInternAtom(xw.dpy, "TARGETS", 0);
	if (xsre->target == xa_targets) {
		/* respond with the supported type */
		string = xsel.xtarget;
		XChangeProperty(xsre->display, xsre->requestor, xsre->property,
				XA_ATOM, 32, PropModeReplace,
				(uchar *) &string, 1);
		xev.property = xsre->property;
	} else if (xsre->target == xsel.xtarget || xsre->target == XA_STRING) {
		/*
		 * xith XA_STRING non ascii characters may be incorrect in the
		 * requestor. It is not our problem, use utf8.
		 */
		clipboard = XInternAtom(xw.dpy, "CLIPBOARD", 0);
		if (xsre->selection == XA_PRIMARY) {
			seltext = xsel.primary;
		} else if (xsre->selection == clipboard) {
			seltext = xsel.clipboard;
		} else {
			fprintf(stderr,
				"Unhandled clipboard selection 0x%lx\n",
				xsre->selection);
			return;
		}
		if (seltext != NULL) {
			XChangeProperty(xsre->display, xsre->requestor,
					xsre->property, xsre->target,
					8, PropModeReplace,
					(uchar *)seltext, strlen(seltext));
			xev.property = xsre->property;
		}
	}

	/* all done, send a notification to the listener */
	if (!XSendEvent(xsre->display, xsre->requestor, 1, 0, (XEvent *) &xev))
		fprintf(stderr, "Error sending SelectionNotify event\n");
}

void
setsel(char *str, Time t)
{
	if (!str)
		return;

	free(xsel.primary);
	xsel.primary = str;

	XSetSelectionOwner(xw.dpy, XA_PRIMARY, xw.win, t);
	if (XGetSelectionOwner(xw.dpy, XA_PRIMARY) != xw.win)
		selclear();
}

void
xsetsel(char *str)
{
	setsel(str, CurrentTime);
}

void
brelease(XEvent *e)
{
	int btn = e->xbutton.button;

	if (1 <= btn && btn <= 11)
		buttons &= ~(1 << (btn-1));
	if (btn == Button1 && view->url_click) {
		if (evcol(e) == view->url_col && evrow(e) == view->url_row)
			urlopen(view->url_click);
		free(view->url_click);
		view->url_click = NULL;
		return;
	}

	if (IS_SET(MODE_MOUSE) && !(e->xbutton.state & forcemousemod)) {
		mousereport(e);
		return;
	}

	if (mouseaction(e, 1))
		return;
	if (btn == Button1)
		mousesel(e, 1);
}

void
bmotion(XEvent *e)
{
	view->hover_inside = e->xmotion.x >= borderpx &&
			e->xmotion.x < borderpx + current_window.tw &&
			e->xmotion.y >= borderpx + current_window.ch &&
			e->xmotion.y < borderpx + current_window.ch + current_window.th;
	if (view->hover_inside) {
		view->hover_col = evcol(e);
		view->hover_row = evrow(e);
	}
	xupdatehover();
	if (view->url_click)
		return;
	if (IS_SET(MODE_MOUSE) && !(e->xbutton.state & forcemousemod)) {
		mousereport(e);
		return;
	}

	mousesel(e, 0);
}

static void
leave(XEvent *e)
{
	(void)e;
	view->hover_inside = 0;
	xclearhover();
}

void
cresize(int width, int height)
{
	int col, row;

	if (width != 0)
		current_window.w = width;
	if (height != 0)
		current_window.h = height;

	col = (current_window.w - 2 * borderpx) / current_window.cw;
	row = (current_window.h - 2 * borderpx - current_window.ch) / current_window.ch;
	col = MAX(1, col);
	row = MAX(1, row);

	/* Service frames own the grid size. Resizing it here could invalidate rows
	 * still arriving from an earlier frame. */
	if (workspacefd < 0)
		tresize(col, row);
	xresize(col, row);
	if (workspacefd >= 0)
		workspace_focus();
	else
		ttyresize(current_window.tw, current_window.th);
}

void
xresize(int col, int row)
{
	current_window.tw = col * current_window.cw;
	current_window.th = row * current_window.ch;

	XFreePixmap(xw.dpy, xw.buf);
	xw.buf = XCreatePixmap(xw.dpy, xw.win, current_window.w, current_window.h,
			DefaultDepth(xw.dpy, xw.scr));
	XftDrawChange(xw.draw, xw.buf);
	xclear(0, 0, current_window.w, current_window.h);

	/* An older frame may still need a wider glyph buffer. */
	if (xw.spec_cols < col) {
		xw.spec_cols = col;
		xw.specbuf = xrealloc(xw.specbuf, xw.spec_cols * sizeof(GlyphFontSpec));
	}
}

ushort
sixd_to_16bit(int x)
{
	return x == 0 ? 0 : 0x3737 + 0x2828 * x;
}

int
xloadcolor(int i, const char *name, Color *ncolor)
{
	XRenderColor color = { .alpha = 0xffff };
	int loaded;
	Tab *tab = xtabfor(tsessioncurrent());
	if (!name && tab && tab->colors && i < MAX(LEN(colorname), 256))
		name = tab->colors[i];

	if (!name && BETWEEN(i, 16, 255) && !colorname[i]) { /* 256 color */
		if (i < 6*6*6+16) { /* same colors as xterm */
			color.red   = sixd_to_16bit( ((i-16)/36)%6 );
			color.green = sixd_to_16bit( ((i-16)/6) %6 );
			color.blue  = sixd_to_16bit( ((i-16)/1) %6 );
		} else { /* greyscale */
			color.red = 0x0808 + 0x0a0a * (i - (6*6*6+16));
			color.green = color.blue = color.red;
		}
		loaded = XftColorAllocValue(xw.dpy, xw.vis,
		                            xw.cmap, &color, ncolor);
	} else {
		if (!name)
			name = colorname[i];
		loaded = XftColorAllocName(xw.dpy, xw.vis, xw.cmap,
				name, ncolor);
	}
	if (loaded)
		xstartupevent(i == defaultbg ? "background" : "color", i);
	return loaded;
}

static Color *
xcolor(int i)
{
	if (!dc.colloaded[i]) {
		if (!xloadcolor(i, NULL, &dc.col[i]))
			die("could not allocate color %d\n", i);
		dc.colloaded[i] = 1;
	}
	return &dc.col[i];
}

static void
xloadcolsone(void)
{
	size_t i;

	if (dc.col) {
		for (i = 0; i < dc.collen; ++i)
			if (dc.colloaded[i])
				XftColorFree(xw.dpy, xw.vis, xw.cmap, &dc.col[i]);
		memset(dc.colloaded, 0, dc.collen);
	} else {
		dc.collen = MAX(LEN(colorname), 256);
		dc.col = xmalloc(dc.collen * sizeof(Color));
		dc.colloaded = xmalloc(dc.collen);
		memset(dc.colloaded, 0, dc.collen);
	}

	/* The X window needs only its background before mapping. Other colors
	 * are allocated by xcolor when terminal output uses them. */
	xcolor(defaultbg);
}

void
xloadcols(void)
{
	XView *previous = view, *candidate;
	TermSession *terminal = tsessioncurrent();

	for (candidate = views; candidate; candidate = candidate->next)
		if (candidate->terminal == terminal) {
			xsetview(candidate);
			xloadcolsone();
		}
	xsetview(previous);
	tsessionuse(terminal);
}

int
xgetcolor(int x, unsigned char *r, unsigned char *g, unsigned char *b)
{
	Color *color;
	Color temporary;
	int hidden = xtabfor(tsessioncurrent()) && !xlivefor(tsessioncurrent());

	if (!BETWEEN(x, 0, dc.collen - 1))
		return 1;
	if (hidden) {
		if (!xloadcolor(x, NULL, &temporary))
			return 1;
		color = &temporary;
	} else {
		color = xcolor(x);
	}
	*r = color->color.red >> 8;
	*g = color->color.green >> 8;
	*b = color->color.blue >> 8;
	if (hidden)
		XftColorFree(xw.dpy, xw.vis, xw.cmap, &temporary);

	return 0;
}

static int
xsetcolornameone(int x, const char *name)
{
	Color ncolor;

	if (!BETWEEN(x, 0, dc.collen - 1))
		return 1;

	if (!xloadcolor(x, name, &ncolor))
		return 1;

	if (dc.colloaded[x])
		XftColorFree(xw.dpy, xw.vis, xw.cmap, &dc.col[x]);
	dc.col[x] = ncolor;
	dc.colloaded[x] = 1;

	return 0;
}

int
xsetcolorname(int x, const char *name)
{
	XView *previous = view, *candidate;
	TermSession *terminal = tsessioncurrent();
	Tab *tab = xtabfor(terminal);
	int failed = 0;
	Color validated;
	if (tab && BETWEEN(x, 0, MAX(LEN(colorname), 256) - 1)) {
		if (!xloadcolor(x, name, &validated))
			return 1;
		XftColorFree(xw.dpy, xw.vis, xw.cmap, &validated);
		if (!tab->colors) {
			tab->colors = xmalloc(MAX(LEN(colorname), 256) * sizeof(char *));
			memset(tab->colors, 0, MAX(LEN(colorname), 256) * sizeof(char *));
		}
		free(tab->colors[x]);
		tab->colors[x] = name ? xstrdup(name) : NULL;
	}

	for (candidate = views; candidate; candidate = candidate->next)
		if (candidate->terminal == terminal) {
			xsetview(candidate);
			failed |= xsetcolornameone(x, name);
		}
	xsetview(previous);
	tsessionuse(terminal);
	return failed;
}

/*
 * Absolute coordinates.
 */
void
xclear(int x1, int y1, int x2, int y2)
{
	XftDrawRect(xw.draw,
			xcolor(IS_SET(MODE_REVERSE)? defaultfg : defaultbg),
			x1, y1, x2-x1, y2-y1);
}

void
xhints(void)
{
	XSizeHints *sizeh;

	sizeh = XAllocSizeHints();

	sizeh->flags = PSize | PResizeInc | PBaseSize | PMinSize;
	sizeh->height = current_window.h;
	sizeh->width = current_window.w;
	sizeh->height_inc = current_window.ch;
	sizeh->width_inc = current_window.cw;
	sizeh->base_height = 2 * borderpx + current_window.ch;
	sizeh->base_width = 2 * borderpx;
	sizeh->min_height = 2 * current_window.ch + 2 * borderpx;
	sizeh->min_width = current_window.cw + 2 * borderpx;
	if (xw.isfixed) {
		sizeh->flags |= PMaxSize;
		sizeh->min_width = sizeh->max_width = current_window.w;
		sizeh->min_height = sizeh->max_height = current_window.h;
	}
	if (xw.gm & (XValue|YValue)) {
		sizeh->flags |= USPosition | PWinGravity;
		sizeh->x = xw.l;
		sizeh->y = xw.t;
		sizeh->win_gravity = xgeommasktogravity(xw.gm);
	}

	XSetWMNormalHints(xw.dpy, xw.win, sizeh);
	XFree(sizeh);
}

void
xidentityhints(void)
{
	XClassHint class = {opt_name ? opt_name : "worminal",
	                    opt_class ? opt_class : "Worminal"};
	XWMHints wm = {.flags = InputHint, .input = 1};

	XSetClassHint(xw.dpy, xw.win, &class);
	XSetWMHints(xw.dpy, xw.win, &wm);
}

int
xgeommasktogravity(int mask)
{
	switch (mask & (XNegative|YNegative)) {
	case 0:
		return NorthWestGravity;
	case XNegative:
		return NorthEastGravity;
	case YNegative:
		return SouthWestGravity;
	}

	return SouthEastGravity;
}

int
xloadfont(Font *f, FcPattern *pattern)
{
	FcPattern *configured;
	FcPattern *match;
	FcResult result;
	XGlyphInfo extents;
	int wantattr, haveattr;

	/*
	 * Manually configure instead of calling XftMatchFont
	 * so that we can use the configured pattern for
	 * "missing glyph" lookups.
	 */
	configured = FcPatternDuplicate(pattern);
	if (!configured)
		return 1;

	FcConfigSubstitute(NULL, configured, FcMatchPattern);
	XftDefaultSubstitute(xw.dpy, xw.scr, configured);

	match = FcFontMatch(NULL, configured, &result);
	if (!match) {
		FcPatternDestroy(configured);
		return 1;
	}

	if (!(f->match = XftFontOpenPattern(xw.dpy, match))) {
		FcPatternDestroy(configured);
		FcPatternDestroy(match);
		return 1;
	}

	if ((XftPatternGetInteger(pattern, "slant", 0, &wantattr) ==
	    XftResultMatch)) {
		/*
		 * Check if xft was unable to find a font with the appropriate
		 * slant but gave us one anyway. Try to mitigate.
		 */
		if ((XftPatternGetInteger(f->match->pattern, "slant", 0,
		    &haveattr) != XftResultMatch) || haveattr < wantattr) {
			f->badslant = 1;
			fputs("font slant does not match\n", stderr);
		}
	}

	if ((XftPatternGetInteger(pattern, "weight", 0, &wantattr) ==
	    XftResultMatch)) {
		if ((XftPatternGetInteger(f->match->pattern, "weight", 0,
		    &haveattr) != XftResultMatch) || haveattr != wantattr) {
			f->badweight = 1;
			fputs("font weight does not match\n", stderr);
		}
	}

	XftTextExtentsUtf8(xw.dpy, f->match,
		(const FcChar8 *) ascii_printable,
		strlen(ascii_printable), &extents);

	f->set = NULL;
	f->pattern = configured;

	f->ascent = f->match->ascent;
	f->descent = f->match->descent;
	f->lbearing = 0;
	f->rbearing = f->match->max_advance_width;

	f->height = f->ascent + f->descent;
	f->width = DIVCEIL(extents.xOff, strlen(ascii_printable));

	return 0;
}

void
xloadfonts(const char *fontstr, double fontsize)
{
	FcPattern *pattern;
	double fontval;

	if (fontstr[0] == '-')
		pattern = XftXlfdParse(fontstr, False, False);
	else
		pattern = FcNameParse((const FcChar8 *)fontstr);

	if (!pattern)
		die("can't open font %s\n", fontstr);

	/* The configured Alacritty point size sets the base font before cell spacing. */
	if (fontsize == 0 && theme_font_size > 0) {
		FcPatternDel(pattern, FC_PIXEL_SIZE);
		FcPatternDel(pattern, FC_SIZE);
		FcPatternAddDouble(pattern, FC_SIZE, theme_font_size);
	}

	if (fontsize > 1) {
		FcPatternDel(pattern, FC_PIXEL_SIZE);
		FcPatternDel(pattern, FC_SIZE);
		FcPatternAddDouble(pattern, FC_PIXEL_SIZE, (double)fontsize);
		usedfontsize = fontsize;
	} else {
		if (FcPatternGetDouble(pattern, FC_PIXEL_SIZE, 0, &fontval) ==
				FcResultMatch) {
			usedfontsize = fontval;
		} else if (FcPatternGetDouble(pattern, FC_SIZE, 0, &fontval) ==
				FcResultMatch) {
			usedfontsize = -1;
		} else {
			/*
			 * Default font size is 12, if none given. This is to
			 * have a known usedfontsize value.
			 */
			FcPatternAddDouble(pattern, FC_PIXEL_SIZE, 12);
			usedfontsize = 12;
		}
		defaultfontsize = usedfontsize;
	}

	if (xloadfont(&dc.font, pattern))
		die("can't open font %s\n", fontstr);

	if (usedfontsize < 0) {
		FcPatternGetDouble(dc.font.match->pattern,
		                   FC_PIXEL_SIZE, 0, &fontval);
		usedfontsize = fontval;
		if (fontsize == 0)
			defaultfontsize = fontval;
	}

	/* Setting character width and height. */
	current_window.cw = ceilf(dc.font.width * cwscale);
	current_window.ch = MAX(1, (int)ceilf(dc.font.height * chscale) + theme_font_offset_y);

	/* Preserve the original pattern so styled fonts can be opened when a
	 * styled glyph is first drawn, after the window has mapped. */
	stylepattern = pattern;
}

static Font *
xgetfont(ushort mode)
{
	Font *font;
	FcPattern *pattern;

	if ((mode & (ATTR_ITALIC | ATTR_BOLD)) ==
			(ATTR_ITALIC | ATTR_BOLD))
		font = &dc.ibfont;
	else if (mode & ATTR_ITALIC)
		font = &dc.ifont;
	else if (mode & ATTR_BOLD)
		font = &dc.bfont;
	else
		return &dc.font;

	if (font->match)
		return font;

	pattern = FcPatternDuplicate(stylepattern);
	if (!pattern)
		die("could not duplicate font pattern\n");
	FcPatternDel(pattern, FC_SLANT);
	FcPatternAddInteger(pattern, FC_SLANT,
			(mode & ATTR_ITALIC) ? FC_SLANT_ITALIC : FC_SLANT_ROMAN);
	if (mode & ATTR_BOLD) {
		FcPatternDel(pattern, FC_WEIGHT);
		FcPatternAddInteger(pattern, FC_WEIGHT, FC_WEIGHT_BOLD);
	}
	if (xloadfont(font, pattern))
		die("can't open styled font %s\n", usedfont);
	FcPatternDestroy(pattern);
	xstartupevent("style", mode & (ATTR_ITALIC | ATTR_BOLD));
	return font;
}

void
xunloadfont(Font *f)
{
	if (!f->match)
		return;
	XftFontClose(xw.dpy, f->match);
	FcPatternDestroy(f->pattern);
	if (f->set)
		FcFontSetDestroy(f->set);
	memset(f, 0, sizeof(*f));
}

void
xunloadfonts(void)
{
	/* Free the loaded fonts in the font cache.  */
	while (frclen > 0)
		XftFontClose(xw.dpy, frc[--frclen].font);

	xunloadfont(&dc.font);
	xunloadfont(&dc.bfont);
	xunloadfont(&dc.ifont);
	xunloadfont(&dc.ibfont);
	FcPatternDestroy(stylepattern);
	stylepattern = NULL;
}

int
ximopen(Display *dpy)
{
	XIMCallback imdestroy = { .client_data = NULL,
	                         .callback = ximdestroy };
	XICCallback icdestroy = { .client_data = (XPointer)view,
	                         .callback = xicdestroy };

	if (!sharedxim) {
		sharedxim = XOpenIM(dpy, NULL, NULL, NULL);
		if (!sharedxim)
			return 0;

		if (XSetIMValues(sharedxim, XNDestroyCallback, &imdestroy, NULL))
			fprintf(stderr, "XSetIMValues: "
			                "Could not set XNDestroyCallback.\n");
	}

	if (!xw.ime.spotlist)
		xw.ime.spotlist = XVaCreateNestedList(0, XNSpotLocation,
		                                      &xw.ime.spot, NULL);

	if (xw.ime.xic == NULL) {
		xw.ime.xic = XCreateIC(sharedxim, XNInputStyle,
		                       XIMPreeditNothing | XIMStatusNothing,
		                       XNClientWindow, xw.win,
		                       XNDestroyCallback, &icdestroy,
		                       NULL);
	}
	if (xw.ime.xic == NULL)
		fprintf(stderr, "XCreateIC: Could not create input context.\n");

	return xw.ime.xic != NULL;
}

void
ximinstantiate(Display *dpy, XPointer client, XPointer call)
{
	XView *previous = view;
	XView *candidate;
	int ready = 1;

	for (candidate = views; candidate; candidate = candidate->next) {
		xsetview(candidate);
		ready &= ximopen(dpy);
	}
	if (ready) {
		XUnregisterIMInstantiateCallback(xw.dpy, NULL, NULL, NULL,
		                                 ximinstantiate, client);
		ximwaiting = 0;
	}
	xsetview(previous);
}

void
ximdestroy(XIM xim, XPointer client, XPointer call)
{
	XView *previous = view;
	XView *candidate;

	sharedxim = NULL;
	for (candidate = views; candidate; candidate = candidate->next) {
		xsetview(candidate);
		xw.ime.xic = NULL;
		if (xw.ime.spotlist)
			XFree(xw.ime.spotlist);
		xw.ime.spotlist = NULL;
	}
	if (!ximwaiting) {
		XRegisterIMInstantiateCallback(xw.dpy, NULL, NULL, NULL,
		                               ximinstantiate, NULL);
		ximwaiting = 1;
	}
	xsetview(previous);
}

int
xicdestroy(XIC xim, XPointer client, XPointer call)
{
	XView *previous = view;
	xsetview((XView *)client);
	xw.ime.xic = NULL;
	xsetview(previous);
	return 1;
}

void
xinitview(int cols, int rows, int first, int spawnpty)
{
	XGCValues gcvalues;
	XWindowChanges changes;
	Cursor cursor;
	Window parent, root;
	pid_t thispid = getpid();
	XColor xmousefg, xmousebg;
	XRenderColor bordergray = {.red = 0x9999, .green = 0x9999,
	                           .blue = 0x9999, .alpha = 0xffff};
	unsigned int geometrymask;

	if (first) {
		if (!(xw.dpy = XOpenDisplay(NULL)))
			die("can't open display\n");
		shared_display = xw.dpy;
		xstartuptime("display");
	} else {
		xw.dpy = shared_display;
	}
	xw.scr = XDefaultScreen(xw.dpy);
	xw.vis = XDefaultVisual(xw.dpy, xw.scr);
	xw.cmap = XDefaultColormap(xw.dpy, xw.scr);
	root = XRootWindow(xw.dpy, xw.scr);
	if (!(opt_embed && (parent = strtol(opt_embed, NULL, 0))))
		parent = root;

	/* The unmapped window supplies WINDOWID while the shell and font setup
	 * proceed together. Give the PTY its initial cell size before exec. */
	xw.attrs.background_pixel = BlackPixel(xw.dpy, xw.scr);
	xw.attrs.border_pixel = xw.attrs.background_pixel;
	xw.attrs.bit_gravity = NorthWestGravity;
	xw.attrs.event_mask = FocusChangeMask | KeyPressMask | KeyReleaseMask
		| ExposureMask | VisibilityChangeMask | StructureNotifyMask
		| PointerMotionMask | LeaveWindowMask | ButtonPressMask | ButtonReleaseMask;
	xw.attrs.colormap = xw.cmap;
	xw.win = XCreateWindow(xw.dpy, root, xw.l, xw.t,
			cols * 8 + 2 * borderpx, (rows + 1) * 16 + 2 * borderpx,
			0, XDefaultDepth(xw.dpy, xw.scr), InputOutput,
			xw.vis, CWBackPixel | CWBorderPixel | CWBitGravity
			| CWEventMask | CWColormap, &xw.attrs);
	initializing_window = xw.win;
	initializing_window_gone = 0;
	previous_xerror = XSetErrorHandler(xinitialerror);
	if (parent != root)
		XReparentWindow(xw.dpy, xw.win, parent, xw.l, xw.t);
	if (spawnpty) {
		xsetenv();
		ttynew(opt_line, shell, opt_io, opt_cmd);
		ttysetlaunch(NULL, NULL, NULL);
		if (first)
			xstartuptime("pty");
	} else if (workspacefd >= 0 && (first || pending_view == view)) {
		workspace_send_launch(first ? WIRE_HELLO : WIRE_NEW, requested_cwd, cols, rows);
	}

	/* Mapping this placeholder lets X queue early keys while font setup runs.
	 * Final font metrics replace its provisional pixel geometry below. */
	xw.xembed = XInternAtom(xw.dpy, "_XEMBED", False);
	xw.wmdeletewin = XInternAtom(xw.dpy, "WM_DELETE_WINDOW", False);
	xw.netwmname = XInternAtom(xw.dpy, "_NET_WM_NAME", False);
	xw.netwmiconname = XInternAtom(xw.dpy, "_NET_WM_ICON_NAME", False);
	XChangeProperty(xw.dpy, xw.win, XInternAtom(xw.dpy, "_NET_WM_ICON", False),
			XA_CARDINAL, 32, PropModeReplace,
			(const unsigned char *)worminal_icon,
			sizeof(worminal_icon) / sizeof(worminal_icon[0]));
	XSetWMProtocols(xw.dpy, xw.win, &xw.wmdeletewin, 1);
	xw.netwmpid = XInternAtom(xw.dpy, "_NET_WM_PID", False);
	XChangeProperty(xw.dpy, xw.win, xw.netwmpid, XA_CARDINAL, 32,
			PropModeReplace, (uchar *)&thispid, 1);
	current_window.mode = MODE_NUMLOCK;
	resettitle();
	xidentityhints();
	if (spawnpty || workspacefd >= 0) {
		XMapWindow(xw.dpy, xw.win);
		XFlush(xw.dpy);
		if (first)
			xstartuptime("map-request");
	}

	/* font */
	if (!FcInit())
		die("could not init fontconfig.\n");
	if (first)
		xstartuptime("fontconfig");

	usedfont = (opt_font == NULL)? font : opt_font;
	xloadfonts(usedfont, 0);
	if (first)
		xstartuptime("font");

	/* colors */
	xloadcolsone();
	if (!XftColorAllocValue(xw.dpy, xw.vis, xw.cmap, &bordergray, &dc.border))
		die("could not allocate border color\n");

	/* adjust fixed window geometry */
	current_window.w = 2 * borderpx + cols * current_window.cw;
	current_window.h = 2 * borderpx + (rows + 1) * current_window.ch;
	if (xw.gm & XNegative)
		xw.l += DisplayWidth(xw.dpy, xw.scr) - current_window.w - 2;
	if (xw.gm & YNegative)
		xw.t += DisplayHeight(xw.dpy, xw.scr) - current_window.h - 2;

	/* Keep the window manager's placement unless -g supplied that axis. */
	changes.x = xw.l;
	changes.y = xw.t;
	changes.width = current_window.w;
	changes.height = current_window.h;
	geometrymask = CWWidth | CWHeight;
	if (xw.gm & XValue)
		geometrymask |= CWX;
	if (xw.gm & YValue)
		geometrymask |= CWY;
	XConfigureWindow(xw.dpy, xw.win, geometrymask, &changes);
	XSetWindowBackground(xw.dpy, xw.win, dc.col[defaultbg].pixel);

	memset(&gcvalues, 0, sizeof(gcvalues));
	gcvalues.graphics_exposures = False;
	dc.gc = XCreateGC(xw.dpy, root, GCGraphicsExposures,
			&gcvalues);
	xw.buf = XCreatePixmap(xw.dpy, root, current_window.w, current_window.h,
			DefaultDepth(xw.dpy, xw.scr));
	XSetForeground(xw.dpy, dc.gc, dc.col[defaultbg].pixel);
	XFillRectangle(xw.dpy, xw.buf, dc.gc, 0, 0, current_window.w, current_window.h);

	/* font spec buffer */
	xw.specbuf = xmalloc(cols * sizeof(GlyphFontSpec));
	xw.spec_cols = cols;

	/* Xft rendering context */
	xw.draw = XftDrawCreate(xw.dpy, xw.buf, xw.vis, xw.cmap);

	/* input methods */
	if (!ximopen(xw.dpy)) {
		XRegisterIMInstantiateCallback(xw.dpy, NULL, NULL, NULL,
	                                       ximinstantiate, NULL);
		ximwaiting = 1;
	}

	/* white cursor, black outline */
	cursor = XCreateFontCursor(xw.dpy, mouseshape);
	XDefineCursor(xw.dpy, xw.win, cursor);

	if (XParseColor(xw.dpy, xw.cmap, colorname[mousefg], &xmousefg) == 0) {
		xmousefg.red   = 0xffff;
		xmousefg.green = 0xffff;
		xmousefg.blue  = 0xffff;
	}

	if (XParseColor(xw.dpy, xw.cmap, colorname[mousebg], &xmousebg) == 0) {
		xmousebg.red   = 0x0000;
		xmousebg.green = 0x0000;
		xmousebg.blue  = 0x0000;
	}

	XRecolorCursor(xw.dpy, cursor, &xmousefg, &xmousebg);

	xhints();

	clock_gettime(CLOCK_MONOTONIC, &xsel.tclick1);
	clock_gettime(CLOCK_MONOTONIC, &xsel.tclick2);
	xsel.primary = NULL;
	xsel.clipboard = NULL;
	xsel.xtarget = XInternAtom(xw.dpy, "UTF8_STRING", 0);
	if (xsel.xtarget == None)
		xsel.xtarget = XA_STRING;
	if (!spawnpty && workspacefd < 0) {
		XMapWindow(xw.dpy, xw.win);
		XFlush(xw.dpy);
	}
	/* A provisional window can be destroyed while fonts load. Drain any
	 * pending BadWindow before restoring the normal X error policy. */
	XSync(xw.dpy, False);
	XSetErrorHandler(previous_xerror);
	initializing_window = None;
	if (initializing_window_gone) {
		signal(SIGCHLD, SIG_IGN);
		ttyhangup();
		exit(0);
	}
}

void
xinit(int cols, int rows)
{
	xinitview(cols, rows, 1, workspacefd < 0);
	view->live = 1;
}

static void
xaddview(void)
{
	XView *previous = view;
	XView *source = views;
	XView *created = xmalloc(sizeof(*created));
	int cols, rows;

	memset(created, 0, sizeof(*created));
	cols = MAX(1, (source->win.w - 2 * borderpx) / source->win.cw);
	rows = MAX(1, (source->win.h - 2 * borderpx - source->win.ch) / source->win.ch);
	if (requested_geometry) {
		cols = requested_cols;
		rows = requested_rows;
	}
	created->terminal = previous->terminal;
	created->live = 0;
	created->next = source->next;
	source->next = created;
	pending_view = created;
	pending_new_tab = 1;
	xsetview(created);
	if (requested_geometry) {
		xw.l = requested_x;
		xw.t = requested_y;
		xw.gm = requested_gm;
		xw.isfixed = requested_fixed;
	}
	xinitview(cols, rows, 0, 0);
	xsetview(previous);
	xrefreshtabs();
}

static void
xactivate(void)
{
	XView *candidate;

	if (view->live) {
		workspace_focus();
		return;
	}
	for (candidate = views; candidate; candidate = candidate->next)
		if (candidate->terminal == view->terminal)
			candidate->live = 0;
	view->live = 1;
	cresize(current_window.w, current_window.h);
	redraw();
}

static void
xselecttab(Tab *tab)
{
	if (!tab)
		return;
	if (view->terminal == tab->terminal) {
		workspace_focus();
		return;
	}
	xclearhover();
	view->live = 0;
	view->terminal = tab->terminal;
	tsessionuse(tab->terminal);
	xupdatehover();
	current_window.mode = (current_window.mode & (MODE_VISIBLE | MODE_FOCUSED)) |
	                      tab->mode | MODE_NUMLOCK;
	current_window.cursor = tab->cursor;
	xloadcolsone();
	char *title = xstrdup(tab->title);
	xsettitle(title);
	free(title);
	xactivate();
	xrefreshtabs();
}

static Tab *
xtabslot(int slot)
{
	Tab *tab = tabs;
	while (tab && --slot > 0)
		tab = tab->next;
	return tab;
}

static void
xclosetab(Tab *tab)
{
	if (tab && tab->id)
		wire_write(workspacefd, WIRE_CLOSE, tab->id, NULL, 0);
}

static void
xnewtab(void)
{
	Tab *selected = xtabfor(view->terminal);
	int cols = MAX(1, (current_window.w - 2 * borderpx) / current_window.cw);
	int rows = MAX(1, (current_window.h - 2 * borderpx - current_window.ch) /
	                  current_window.ch);
	pending_view = view;
	pending_new_tab = 1;
	workspace_send_launch(WIRE_NEW, selected ? selected->directory : NULL, cols, rows);
}

static XView *
xlivefor(TermSession *terminal)
{
	XView *candidate;

	for (candidate = views; candidate; candidate = candidate->next)
		if (candidate->terminal == terminal && candidate->live)
			return candidate;
	return NULL;
}

static void
xremoveview(int destroyed)
{
	XView *removed = view;
	XView **slot = &views;
	XView *nextlive = NULL, *candidate;
	size_t i;
	int waslive = removed->live;

	while (*slot && *slot != removed)
		slot = &(*slot)->next;
	if (!*slot)
		die("closing an unknown view\n");
	*slot = removed->next;
	for (candidate = views; candidate; candidate = candidate->next)
		if (candidate->terminal == removed->terminal) {
			nextlive = candidate;
			break;
		}
	if (xw.ime.xic)
		XDestroyIC(xw.ime.xic);
	if (xw.ime.spotlist)
		XFree(xw.ime.spotlist);
	xunloadfonts();
	free(frc);
	for (i = 0; i < dc.collen; i++)
		if (dc.colloaded[i])
			XftColorFree(xw.dpy, xw.vis, xw.cmap, &dc.col[i]);
	XftColorFree(xw.dpy, xw.vis, xw.cmap, &dc.border);
	free(dc.col);
	free(dc.colloaded);
	free(xw.specbuf);
	XftDrawDestroy(xw.draw);
	XFreePixmap(xw.dpy, xw.buf);
	XFreeGC(xw.dpy, dc.gc);
	if (!destroyed)
		XDestroyWindow(xw.dpy, xw.win);
	free(xsel.primary);
	free(xsel.clipboard);
	free(removed->url_click);
	if (removed != &primaryview)
		free(removed);
	if (!views) {
		/* The owner's deliberate last-window shutdown wins over a shell's
		 * SIGHUP exit status, which SIGCHLD otherwise turns into exit(1). */
		signal(SIGCHLD, SIG_IGN);
		tsessionhangupall();
		exit(0);
	}
	xsetview(views);
	if (waslive && nextlive) {
		xsetview(nextlive);
		xactivate();
	}
}

int
xmakeglyphfontspecs(XftGlyphFontSpec *specs, const Glyph *glyphs, int len, int x, int y)
{
	float winx = borderpx + x * current_window.cw, winy = borderpx + (y + 1) * current_window.ch, xp, yp;
	ushort mode, prevmode = USHRT_MAX;
	Font *font = &dc.font;
	int frcflags = FRC_NORMAL;
	float runewidth = current_window.cw;
	Rune rune;
	FT_UInt glyphidx;
	FcResult fcres;
	FcPattern *fcpattern, *fontpattern;
	FcFontSet *fcsets[] = { NULL };
	FcCharSet *fccharset;
	int i, f, numspecs = 0;

	for (i = 0, xp = winx, yp = winy + font->ascent; i < len; ++i) {
		/* Fetch rune and mode for current glyph. */
		rune = glyphs[i].u;
		mode = glyphs[i].mode;

		/* Skip dummy wide-character spacing. */
		if (mode == ATTR_WDUMMY)
			continue;

		/* Determine font for glyph if different from previous glyph. */
		if (prevmode != mode) {
			prevmode = mode;
			font = xgetfont(mode);
			frcflags = FRC_NORMAL;
			runewidth = current_window.cw * ((mode & ATTR_WIDE) ? 2.0f : 1.0f);
			if ((mode & ATTR_ITALIC) && (mode & ATTR_BOLD)) {
				frcflags = FRC_ITALICBOLD;
			} else if (mode & ATTR_ITALIC) {
				frcflags = FRC_ITALIC;
			} else if (mode & ATTR_BOLD) {
				frcflags = FRC_BOLD;
			}
			yp = winy + font->ascent;
		}

		/* Lookup character index with default font. */
		glyphidx = XftCharIndex(xw.dpy, font->match, rune);
		if (glyphidx) {
			specs[numspecs].font = font->match;
			specs[numspecs].glyph = glyphidx;
			specs[numspecs].x = (short)xp;
			specs[numspecs].y = (short)yp;
			xp += runewidth;
			numspecs++;
			continue;
		}

		/* Fallback on font cache, search the font cache for match. */
		for (f = 0; f < frclen; f++) {
			glyphidx = XftCharIndex(xw.dpy, frc[f].font, rune);
			/* Everything correct. */
			if (glyphidx && frc[f].flags == frcflags)
				break;
			/* We got a default font for a not found glyph. */
			if (!glyphidx && frc[f].flags == frcflags
					&& frc[f].unicodep == rune) {
				break;
			}
		}

		/* Nothing was found. Use fontconfig to find matching font. */
		if (f >= frclen) {
			if (!font->set)
				font->set = FcFontSort(0, font->pattern,
				                       1, 0, &fcres);
			fcsets[0] = font->set;

			/*
			 * Nothing was found in the cache. Now use
			 * some dozen of Fontconfig calls to get the
			 * font for one single character.
			 *
			 * Xft and fontconfig are design failures.
			 */
			fcpattern = FcPatternDuplicate(font->pattern);
			fccharset = FcCharSetCreate();

			FcCharSetAddChar(fccharset, rune);
			FcPatternAddCharSet(fcpattern, FC_CHARSET,
					fccharset);
			FcPatternAddBool(fcpattern, FC_SCALABLE, 1);

			FcConfigSubstitute(0, fcpattern,
					FcMatchPattern);
			FcDefaultSubstitute(fcpattern);

			fontpattern = FcFontSetMatch(0, fcsets, 1,
					fcpattern, &fcres);

			/* Allocate memory for the new cache entry. */
			if (frclen >= frccap) {
				frccap += 16;
				frc = xrealloc(frc, frccap * sizeof(Fontcache));
			}

			frc[frclen].font = XftFontOpenPattern(xw.dpy,
					fontpattern);
			if (!frc[frclen].font)
				die("XftFontOpenPattern failed seeking fallback font: %s\n",
					strerror(errno));
			frc[frclen].flags = frcflags;
			frc[frclen].unicodep = rune;

			glyphidx = XftCharIndex(xw.dpy, frc[frclen].font, rune);

			f = frclen;
			frclen++;

			FcPatternDestroy(fcpattern);
			FcCharSetDestroy(fccharset);
		}

		specs[numspecs].font = frc[f].font;
		specs[numspecs].glyph = glyphidx;
		specs[numspecs].x = (short)xp;
		specs[numspecs].y = (short)yp;
		xp += runewidth;
		numspecs++;
	}

	return numspecs;
}

void
xdrawglyphfontspecs(const XftGlyphFontSpec *specs, Glyph base, int len, int x, int y, int pass)
{
	int charlen = len * ((base.mode & ATTR_WIDE) ? 2 : 1);
	int winx = borderpx + x * current_window.cw, winy = borderpx + (y + 1) * current_window.ch,
	    width = charlen * current_window.cw;
	Color *fg, *bg, *temp, revfg, revbg, truefg, truebg;
	Font *font = xgetfont(base.mode);
	XRenderColor colfg, colbg;
	XRectangle r;
	int overlap = xoverlap();

	/* Fallback on color display for attributes not supported by the font */
	if ((base.mode & ATTR_ITALIC && font->badslant) ||
	    (base.mode & ATTR_BOLD && font->badweight)) {
		base.fg = defaultattr;
	}

	if (IS_TRUECOL(base.fg)) {
		colfg.alpha = 0xffff;
		colfg.red = TRUERED(base.fg);
		colfg.green = TRUEGREEN(base.fg);
		colfg.blue = TRUEBLUE(base.fg);
		XftColorAllocValue(xw.dpy, xw.vis, xw.cmap, &colfg, &truefg);
		fg = &truefg;
	} else {
		fg = xcolor(base.fg);
	}

	if (IS_TRUECOL(base.bg)) {
		colbg.alpha = 0xffff;
		colbg.green = TRUEGREEN(base.bg);
		colbg.red = TRUERED(base.bg);
		colbg.blue = TRUEBLUE(base.bg);
		XftColorAllocValue(xw.dpy, xw.vis, xw.cmap, &colbg, &truebg);
		bg = &truebg;
	} else {
		bg = xcolor(base.bg);
	}

	/* Use bright system colors for bold text when the theme requests it. */
	if (theme_bold_bright && (base.mode & ATTR_BOLD_FAINT) == ATTR_BOLD &&
	    BETWEEN(base.fg, 0, 7))
		fg = xcolor(base.fg + 8);
	else if (theme_bold_bright &&
	         (base.mode & ATTR_BOLD_FAINT) == ATTR_BOLD &&
	         base.fg == defaultfg)
		fg = xcolor(260);

	if (IS_SET(MODE_REVERSE)) {
		if (fg == &dc.col[defaultfg]) {
			fg = xcolor(defaultbg);
		} else {
			colfg.red = ~fg->color.red;
			colfg.green = ~fg->color.green;
			colfg.blue = ~fg->color.blue;
			colfg.alpha = fg->color.alpha;
			XftColorAllocValue(xw.dpy, xw.vis, xw.cmap, &colfg,
					&revfg);
			fg = &revfg;
		}

		if (bg == &dc.col[defaultbg]) {
			bg = xcolor(defaultfg);
		} else {
			colbg.red = ~bg->color.red;
			colbg.green = ~bg->color.green;
			colbg.blue = ~bg->color.blue;
			colbg.alpha = bg->color.alpha;
			XftColorAllocValue(xw.dpy, xw.vis, xw.cmap, &colbg,
					&revbg);
			bg = &revbg;
		}
	}

	if ((base.mode & ATTR_BOLD_FAINT) == ATTR_FAINT) {
		if (BETWEEN(base.fg, 0, 7) && colorname[261 + base.fg]) {
			fg = xcolor(261 + base.fg);
		} else if (base.fg == defaultfg && colorname[269]) {
			fg = xcolor(269);
		} else {
			colfg.red = fg->color.red / 2;
			colfg.green = fg->color.green / 2;
			colfg.blue = fg->color.blue / 2;
			colfg.alpha = fg->color.alpha;
			XftColorAllocValue(xw.dpy, xw.vis, xw.cmap, &colfg, &revfg);
			fg = &revfg;
		}
	}

	if (base.mode & ATTR_REVERSE) {
		temp = fg;
		fg = bg;
		bg = temp;
	}

	if (base.mode & ATTR_BLINK && current_window.mode & MODE_BLINK)
		fg = bg;

	if (base.mode & ATTR_INVISIBLE)
		fg = bg;

	if (pass & DRAW_BACKGROUND) {
		/* Clean borders and each cell background before drawing any glyphs. */
		if (x == 0) {
			xclear(0, (y == 0)? 0 : winy, borderpx,
				winy + current_window.ch +
				((winy + current_window.ch >= borderpx + current_window.th)? current_window.h : 0));
		}
		if (winx + width >= borderpx + current_window.tw) {
			xclear(winx + width, (y == 0)? 0 : winy, current_window.w,
				((winy + current_window.ch >= borderpx + current_window.ch + current_window.th)?
				 current_window.h : (winy + current_window.ch)));
		}
		if (y == 0)
			xclear(winx, 0, winx + width, borderpx);
		if (winy + current_window.ch >= borderpx + current_window.th)
			xclear(winx, winy + current_window.ch, winx + width, current_window.h);
		XftDrawRect(xw.draw, bg, winx, winy, width, current_window.ch);
	}

	if (pass & DRAW_FOREGROUND) {
		/* Keep horizontal run clipping, but let ink cross row boundaries. */
		r.x = 0;
		r.y = 0;
		r.height = overlap ? current_window.h : current_window.ch;
		r.width = width;
		XftDrawSetClipRectangles(xw.draw, winx,
				overlap ? 0 : winy, &r, 1);
		XftDrawGlyphFontSpec(xw.draw, fg, specs, len);
		if (base.mode & ATTR_UNDERLINE)
			XftDrawRect(xw.draw, fg, winx,
					winy + dc.font.ascent * chscale + 1, width, 1);
		if (base.mode & ATTR_STRUCK)
			XftDrawRect(xw.draw, fg, winx,
					winy + 2 * dc.font.ascent * chscale / 3, width, 1);
		XftDrawSetClip(xw.draw, 0);
	}
}

void
xdrawglyph(Glyph g, int x, int y)
{
	int numspecs;
	XftGlyphFontSpec spec;

	numspecs = xmakeglyphfontspecs(&spec, &g, 1, x, y);
	xdrawglyphfontspecs(&spec, g, numspecs, x, y,
			DRAW_BACKGROUND | DRAW_FOREGROUND);
}

void
xdrawcursor(int cx, int cy, Glyph g, int ox, int oy, Glyph og)
{
	Color drawcol;

	/* Compact rows are fully repainted before the cursor is drawn. */
	if (!xoverlap()) {
		if (selected(ox, oy))
			og.mode ^= ATTR_REVERSE;
		xdrawglyph(og, ox, oy);
	}

	if (IS_SET(MODE_HIDE))
		return;

	/*
	 * Select the right color for the right mode.
	 */
	g.mode &= ATTR_BOLD|ATTR_ITALIC|ATTR_UNDERLINE|ATTR_STRUCK|ATTR_WIDE;

	if (IS_SET(MODE_REVERSE)) {
		g.mode |= ATTR_REVERSE;
		g.bg = defaultfg;
		if (selected(cx, cy)) {
			drawcol = *xcolor(defaultcs);
			g.fg = defaultrcs;
		} else {
			drawcol = *xcolor(defaultrcs);
			g.fg = defaultcs;
		}
	} else {
		if (selected(cx, cy)) {
			g.fg = defaultfg;
			g.bg = defaultrcs;
		} else {
			g.fg = defaultbg;
			g.bg = defaultcs;
		}
		drawcol = *xcolor(g.bg);
	}

	/* draw the new one */
	if (IS_SET(MODE_FOCUSED)) {
		switch (current_window.cursor) {
		case 7: /* st extension */
			g.u = 0x2603; /* snowman (U+2603) */
			/* FALLTHROUGH */
		case 0: /* Blinking Block */
		case 1: /* Blinking Block (Default) */
		case 2: /* Steady Block */
			xdrawglyph(g, cx, cy);
			break;
		case 3: /* Blinking Underline */
		case 4: /* Steady Underline */
			XftDrawRect(xw.draw, &drawcol,
					borderpx + cx * current_window.cw,
					borderpx + (cy + 2) * current_window.ch - \
						cursorthickness,
					current_window.cw, cursorthickness);
			break;
		case 5: /* Blinking bar */
		case 6: /* Steady bar */
			XftDrawRect(xw.draw, &drawcol,
					borderpx + cx * current_window.cw,
					borderpx + (cy + 1) * current_window.ch,
					cursorthickness, current_window.ch);
			break;
		}
	} else {
		XftDrawRect(xw.draw, &drawcol,
				borderpx + cx * current_window.cw,
				borderpx + (cy + 1) * current_window.ch,
				current_window.cw - 1, 1);
		XftDrawRect(xw.draw, &drawcol,
				borderpx + cx * current_window.cw,
				borderpx + (cy + 1) * current_window.ch,
				1, current_window.ch - 1);
		XftDrawRect(xw.draw, &drawcol,
				borderpx + (cx + 1) * current_window.cw - 1,
				borderpx + (cy + 1) * current_window.ch,
				1, current_window.ch - 1);
		XftDrawRect(xw.draw, &drawcol,
				borderpx + cx * current_window.cw,
				borderpx + (cy + 2) * current_window.ch - 1,
				current_window.cw, 1);
	}
}

void
xsetenv(void)
{
	snprintf(launch_windowid, sizeof(launch_windowid), "%lu", xw.win);
	ttysetlaunch(requested_cwd, requested_env, launch_windowid);
}

void
xseticontitle(char *p)
{
	XTextProperty prop;
	TermSession *terminal = tsessioncurrent();
	Tab *tab = xtabfor(terminal);
	XView *previous = view, *candidate;
	if (!p || !*p)
		p = tab ? tab->initial_title : opt_title;

	for (candidate = views; candidate; candidate = candidate->next) {
		if (candidate->terminal != terminal)
			continue;
		xsetview(candidate);
		if (Xutf8TextListToTextProperty(xw.dpy, &p, 1, XUTF8StringStyle,
		                                    &prop) == Success) {
			XSetWMIconName(xw.dpy, xw.win, &prop);
			XSetTextProperty(xw.dpy, xw.win, &prop, xw.netwmiconname);
			XFree(prop.value);
		}
	}
	xsetview(previous);
	tsessionuse(terminal);
}

void
xsettitle(char *p)
{
	XTextProperty prop;
	TermSession *terminal = tsessioncurrent();
	Tab *tab = xtabfor(terminal);
	XView *previous = view, *candidate;
	if (!p || !*p)
		p = tab ? tab->initial_title : opt_title;

	if (tab) {
		free(tab->title);
		tab->title = xstrdup(p);
	}
	for (candidate = views; candidate; candidate = candidate->next) {
		if (candidate->terminal != terminal)
			continue;
		xsetview(candidate);
		if (Xutf8TextListToTextProperty(xw.dpy, &p, 1, XUTF8StringStyle,
		                                    &prop) == Success) {
			XSetWMName(xw.dpy, xw.win, &prop);
			XSetTextProperty(xw.dpy, xw.win, &prop, xw.netwmname);
			XFree(prop.value);
		}
	}
	xsetview(previous);
	tsessionuse(terminal);
}

static const char *
xtablabel(const char *directory, int *length)
{
	size_t end = strlen(directory), start;

	while (end > 1 && directory[end - 1] == '/')
		end--;
	if (end == 1 && directory[0] == '/') {
		*length = 1;
		return directory;
	}
	start = end;
	while (start > 0 && directory[start - 1] != '/')
		start--;
	*length = end - start;
	return directory + start;
}

static int
xtabwidth(Tab *tab)
{
	XGlyphInfo extents;
	int length;
	const char *title = xtablabel(tab->directory, &length);
	XftTextExtentsUtf8(xw.dpy, dc.font.match, (const FcChar8 *)title,
	                    length, &extents);
	return extents.xOff + 2 * current_window.cw;
}

static Tab *
xfirsttab(void)
{
	Tab *first = tabs, *tab;
	int width;
	while (first && first->next) {
		width = borderpx;
		for (tab = first; tab; tab = tab->next) {
			width += xtabwidth(tab);
			if (tab->terminal == view->terminal)
				break;
		}
		if (width <= current_window.w - borderpx)
			break;
		first = first->next;
	}
	return first;
}

static void
xdrawtabs(void)
{
	Tab *tab;
	int x = borderpx, right = current_window.w - borderpx;
	if (!tabs || !dc.font.match)
		return;
	int baseline = borderpx + (current_window.ch - dc.font.height) / 2 + dc.font.ascent;
	XRectangle clip = {.x = borderpx, .y = borderpx,
	                   .width = MAX(0, right - borderpx), .height = current_window.ch};
	/* Tab refreshes copy this strip without redrawing the window outline. */
	XftDrawRect(xw.draw, xcolor(defaultbg), borderpx, borderpx,
	            MAX(0, right - borderpx), current_window.ch);
	XftDrawSetClipRectangles(xw.draw, 0, 0, &clip, 1);
	for (tab = xfirsttab(); tab && x < right; tab = tab->next) {
		int width = xtabwidth(tab), selected = tab->terminal == view->terminal;
		int length;
		const char *title = xtablabel(tab->directory, &length);
		Color *ink = xcolor(selected ? defaultbg : defaultfg);
		if (selected)
			XftDrawRect(xw.draw, xcolor(defaultfg), x, borderpx,
			            MIN(width, right - x), current_window.ch);
		XftDrawStringUtf8(xw.draw, ink,
		                  dc.font.match, x + current_window.cw, baseline,
		                  (const FcChar8 *)title, length);
		XftDrawRect(xw.draw, ink, x + current_window.cw,
		            MIN(baseline + 2, borderpx + current_window.ch - 2),
		            MAX(0, width - 2 * current_window.cw), 1);
		x += width;
	}
	XftDrawSetClip(xw.draw, 0);
}

static void
xrefreshtabs(void)
{
	XView *previous = view, *candidate;
	for (candidate = views; candidate; candidate = candidate->next) {
		xsetview(candidate);
		if (!xw.draw)
			continue;
		xdrawtabs();
		XCopyArea(xw.dpy, xw.buf, xw.win, dc.gc, 0, 0,
		          current_window.w, borderpx + current_window.ch, 0, 0);
	}
	xsetview(previous);
}

static Tab *
workspace_find(Tab **head, uint32_t id)
{
	Tab **slot = head;
	while (*slot && (*slot)->id != id)
		slot = &(*slot)->next;
	if (!*slot)
		return NULL;
	Tab *tab = *slot;
	*slot = tab->next;
	tab->next = NULL;
	return tab;
}

static void
workspace_free_tab(Tab *tab)
{
	free(tab->title);
	free(tab->initial_title);
	free(tab->directory);
	if (tab->colors) {
		for (size_t i = 0; i < MAX(LEN(colorname), 256); i++)
			free(tab->colors[i]);
		free(tab->colors);
	}
	tsessionremove(tab->terminal);
	free(tab);
}

static void
workspace_catalog(WirePacket *packet)
{
	uint32_t count, id, mode, cursor, colors, closed, fallback;
	Tab *unseen = tabs;
	XView *saved = view;
	if (!wire_get_u32(packet, &count) || !wire_get_u32(packet, &closed) ||
	    !wire_get_u32(packet, &fallback) || count > 1000)
		die("invalid workspace catalog\n");
	if (!count)
		exit(0);
	tabs = lasttab = NULL;
	for (uint32_t item = 0; item < count; item++) {
		if (!wire_get_u32(packet, &id) || !wire_get_u32(packet, &mode) ||
		    !wire_get_u32(packet, &cursor) || !id)
			die("invalid workspace tab\n");
		char *title = wire_get_string(packet);
		char *directory = wire_get_string(packet);
		if (!title || !directory || !wire_get_u32(packet, &colors) ||
		    colors != LEN(colorname))
			die("invalid workspace metadata\n");
		Tab *tab = workspace_find(&unseen, id);
		if (!tab)
			tab = workspace_find(&unseen, 0);
		if (!tab) {
			TermSession *terminal = tsessionnew(cols, rows);
			tab = xtabnew(terminal, title, directory);
		} else {
			if (lasttab)
				lasttab->next = tab;
			else
				tabs = tab;
			lasttab = tab;
		}
		tab->id = id;
		tab->mode = mode;
		tab->cursor = cursor;
		free(tab->title);
		free(tab->directory);
		tab->title = title;
		tab->directory = directory;
		if (!tab->colors)
			tab->colors = calloc(MAX(LEN(colorname), 256), sizeof(char *));
		for (uint32_t i = 0; i < colors; i++) {
			char *name = wire_get_string(packet);
			if (!name)
				die("invalid workspace palette\n");
			free(tab->colors[i]);
			tab->colors[i] = *name ? name : NULL;
			if (!*name)
				free(name);
		}
	}
	if (packet->pos != packet->len)
		die("invalid workspace catalog length\n");
	for (XView *candidate = views; candidate; candidate = candidate->next) {
		xsetview(candidate);
		Tab *selected = xtabfor(candidate->terminal);
		if (candidate == pending_view && pending_new_tab)
			selected = lasttab;
		if (!selected && fallback)
			for (Tab *old = unseen; old; old = old->next)
				if (old->terminal == candidate->terminal && old->id == closed)
					for (Tab *next = tabs; next; next = next->next)
						if (next->id == fallback)
							selected = next;
		if (!selected)
			selected = tabs;
		if (candidate->terminal != selected->terminal)
			xselecttab(selected);
		else {
			current_window.mode = (current_window.mode & (MODE_VISIBLE | MODE_FOCUSED)) |
			                      selected->mode | MODE_NUMLOCK;
			current_window.cursor = selected->cursor;
			xloadcolsone();
			char *copy = xstrdup(selected->title);
			xsettitle(copy);
			free(copy);
		}
	}
	pending_view = NULL;
	pending_new_tab = 0;
	while (unseen) {
		Tab *next = unseen->next;
		workspace_free_tab(unseen);
		unseen = next;
	}
	xsetview(saved);
	xrefreshtabs();
}

static void
workspace_message(WirePacket *packet)
{
	Tab *tab = NULL;
	uint32_t values[6];
	if (packet->type == WIRE_CATALOG) {
		workspace_catalog(packet);
		return;
	}
	for (tab = tabs; tab && tab->id != packet->tab; tab = tab->next)
		;
	if (!tab)
		return;
	if (packet->type == WIRE_FRAME_FINISH) {
		XView *previous = view;
		for (XView *candidate = views; candidate; candidate = candidate->next)
			if (candidate->terminal == tab->terminal && candidate->hover_inside) {
				xsetview(candidate);
				xupdatehover();
			}
		xsetview(previous);
		return;
	}
	if (packet->type == WIRE_RELEASE) {
		for (XView *candidate = views; candidate; candidate = candidate->next)
			if (candidate->terminal == tab->terminal)
				candidate->live = 0;
		return;
	}
	if (packet->type == WIRE_PRINT) {
		for (size_t done = 0; done < packet->len;) {
			ssize_t count = write(1, packet->data + done, packet->len - done);
			if (count <= 0)
				break;
			done += count;
		}
		return;
	}
	if (packet->type == WIRE_FRAME) {
		for (size_t i = 0; i < LEN(values); i++)
			if (!wire_get_u32(packet, &values[i]))
				die("invalid workspace frame\n");
		if (values[0] < 1 || values[0] > 400 || values[1] < 1 || values[1] > 200 ||
		    values[2] >= values[0] || values[3] >= values[1])
			die("invalid workspace dimensions\n");
		SessionFrame frame = {
			.cols = values[0], .rows = values[1],
			.cursor_x = values[2], .cursor_y = values[3],
			.scroll = values[4], .alternate = values[5]
		};
		XView *previous = view;
		for (XView *candidate = views; candidate; candidate = candidate->next)
			if (candidate->terminal == tab->terminal) {
				xsetview(candidate);
				if (xw.spec_cols < frame.cols) {
					xw.specbuf = xrealloc(xw.specbuf, frame.cols * sizeof(GlyphFontSpec));
					xw.spec_cols = frame.cols;
				}
			}
		xsetview(previous);
		tsessionimportframe(tab->terminal, &frame);
		return;
	}
	if (packet->type == WIRE_CLIPBOARD) {
		char *value = wire_get_string(packet);
		if (!value)
			return;
		for (XView *candidate = views; candidate; candidate = candidate->next)
			if (candidate->terminal == tab->terminal && candidate->live) {
				XView *previous = view;
				xsetview(candidate);
				xsetsel(value);
				xclipcopy();
				xsetview(previous);
				return;
			}
		free(value);
		return;
	}
	if (packet->type == WIRE_ROW) {
		uint32_t row;
		SessionFrame frame = tsessionframe(tab->terminal);
		if (!wire_get_u32(packet, &row) || row >= (uint32_t)frame.rows ||
		    packet->len - packet->pos != (size_t)frame.cols * 16)
			die("invalid workspace row\n");
		Glyph *glyphs = xmalloc(frame.cols * sizeof(*glyphs));
		for (int col = 0; col < frame.cols; col++) {
			uint32_t mode;
			wire_get_u32(packet, &glyphs[col].u);
			wire_get_u32(packet, &mode);
			glyphs[col].mode = mode;
			wire_get_u32(packet, &glyphs[col].fg);
			wire_get_u32(packet, &glyphs[col].bg);
		}
		tsessionimportrow(tab->terminal, row, glyphs, frame.cols);
		free(glyphs);
	}
}

int
xstartdraw(void)
{
	return IS_SET(MODE_VISIBLE);
}

int
xoverlap(void)
{
	return current_window.ch < dc.font.height;
}

void
xdrawline(Line line, int x1, int y1, int x2, int pass)
{
	int i, x, ox, numspecs;
	Glyph base, new;
	XftGlyphFontSpec *specs = xw.specbuf;

	if (pass & DRAW_FOREGROUND) {
		numspecs = xmakeglyphfontspecs(specs, &line[x1], x2 - x1, x1, y1);
	} else {
		/* Background runs need attributes, but no font or glyph lookup. */
		for (x = x1, numspecs = 0; x < x2; x++)
			numspecs += line[x].mode != ATTR_WDUMMY;
	}
	i = ox = 0;
	for (x = x1; x < x2 && i < numspecs; x++) {
		new = line[x];
		if (new.mode == ATTR_WDUMMY)
			continue;
		if (view->hover_active && y1 >= view->hover.start_y && y1 <= view->hover.end_y &&
		    (y1 != view->hover.start_y || x >= view->hover.start_x) &&
		    (y1 != view->hover.end_y || x <= view->hover.end_x))
			new.mode |= ATTR_UNDERLINE;
		if (selected(x, y1))
			new.mode ^= ATTR_REVERSE;
		if (i > 0 && ATTRCMP(base, new)) {
			xdrawglyphfontspecs(specs, base, i, ox, y1, pass);
			specs += i;
			numspecs -= i;
			i = 0;
		}
		if (i == 0) {
			ox = x;
			base = new;
		}
		i++;
	}
	if (i > 0)
		xdrawglyphfontspecs(specs, base, i, ox, y1, pass);
}

void
xfinishdraw(void)
{
	xdrawtabs();
	/* The window manager may discard an X window border; draw the outline
	 * inside the client area after cell backgrounds have been repainted. */
	XSetForeground(xw.dpy, dc.gc, dc.border.pixel);
	XDrawRectangle(xw.dpy, xw.buf, dc.gc, 0, 0, current_window.w - 1, current_window.h - 1);
	XCopyArea(xw.dpy, xw.buf, xw.win, dc.gc, 0, 0, current_window.w,
			current_window.h, 0, 0);
	XSetForeground(xw.dpy, dc.gc,
			xcolor(IS_SET(MODE_REVERSE)?
				defaultfg : defaultbg)->pixel);
}

void
xximspot(int x, int y)
{
	if (xw.ime.xic == NULL)
		return;

	xw.ime.spot.x = borderpx + x * current_window.cw;
	xw.ime.spot.y = borderpx + (y + 2) * current_window.ch;

	XSetICValues(xw.ime.xic, XNPreeditAttributes, xw.ime.spotlist, NULL);
}

void
expose(XEvent *ev)
{
	if (view->live)
		redraw();
	else
		xfinishdraw();
}

void
visibility(XEvent *ev)
{
	XVisibilityEvent *e = &ev->xvisibility;

	MODBIT(current_window.mode, e->state != VisibilityFullyObscured, MODE_VISIBLE);
}

void
unmap(XEvent *ev)
{
	current_window.mode &= ~MODE_VISIBLE;
	view->hover_inside = 0;
	xclearhover();
	Tab *tab = xtabfor(view->terminal);
	if (tab && tab->id && view->live)
		wire_write(workspacefd, WIRE_RELEASE, tab->id, NULL, 0);
}

void
destroy(XEvent *ev)
{
	xremoveview(1);
}

void
xsetpointermotion(int set)
{
	/* Hover needs motion events even when applications disable mouse reporting. */
	(void)set;
}

void
xsetmode(int set, unsigned int flags)
{
	XView *candidate;
	TermSession *terminal = tsessioncurrent();
	Tab *tab = xtabfor(terminal);
	int oldmode = tab ? tab->mode : current_window.mode;
	if (tab)
		MODBIT(tab->mode, set, flags);
	for (candidate = views; candidate; candidate = candidate->next)
		if (candidate->terminal == terminal)
			MODBIT(candidate->win.mode, set, flags);
	if ((flags & MODE_REVERSE) && tab &&
	    ((tab->mode ^ oldmode) & MODE_REVERSE)) {
		candidate = xlivefor(terminal);
		if (candidate) {
			XView *previous = view;
			xsetview(candidate);
			redraw();
			xsetview(previous);
			tsessionuse(terminal);
		}
	}
}

int
xsetcursor(int cursor)
{
	Tab *tab = xtabfor(tsessioncurrent());
	if (!BETWEEN(cursor, 0, 7)) /* 7: st extension */
		return 1;
	if (tab)
		tab->cursor = cursor;
	if (xlivefor(tsessioncurrent()))
		xlivefor(tsessioncurrent())->win.cursor = cursor;
	return 0;
}

void
xseturgency(int add)
{
	XWMHints *h = XGetWMHints(xw.dpy, xw.win);

	MODBIT(h->flags, add, XUrgencyHint);
	XSetWMHints(xw.dpy, xw.win, h);
	XFree(h);
}

void
xbell(void)
{
	XView *live = xlivefor(tsessioncurrent());
	XView *previous = view;
	TermSession *terminal = tsessioncurrent();
	if (!live)
		return;
	xsetview(live);
	if (!(IS_SET(MODE_FOCUSED)))
		xseturgency(1);
	if (bellvolume)
		XkbBell(xw.dpy, xw.win, bellvolume, (Atom)NULL);
	xsetview(previous);
	tsessionuse(terminal);
}

void
focus(XEvent *ev)
{
	XFocusChangeEvent *e = &ev->xfocus;

	if (e->mode == NotifyGrab)
		return;

	if (ev->type == FocusIn) {
		xactivate();
		if (xw.ime.xic)
			XSetICFocus(xw.ime.xic);
		current_window.mode |= MODE_FOCUSED;
		xseturgency(0);
		if (IS_SET(MODE_FOCUS))
			ttywrite("\033[I", 3, 0);
	} else {
		if (xw.ime.xic)
			XUnsetICFocus(xw.ime.xic);
		current_window.mode &= ~MODE_FOCUSED;
		if (IS_SET(MODE_FOCUS))
			ttywrite("\033[O", 3, 0);
	}
}

int
match(uint mask, uint state)
{
	return mask == XK_ANY_MOD || mask == (state & ~ignoremod);
}

char*
kmap(KeySym k, uint state)
{
	Key *kp;
	int i;

	/* Check for mapped keys out of X11 function keys. */
	for (i = 0; i < LEN(mappedkeys); i++) {
		if (mappedkeys[i] == k)
			break;
	}
	if (i == LEN(mappedkeys)) {
		if ((k & 0xFFFF) < 0xFD00)
			return NULL;
	}

	for (kp = key; kp < key + LEN(key); kp++) {
		if (kp->k != k)
			continue;

		if (!match(kp->mask, state))
			continue;

		if (IS_SET(MODE_APPKEYPAD) ? kp->appkey < 0 : kp->appkey > 0)
			continue;
		if (IS_SET(MODE_NUMLOCK) && kp->appkey == 2)
			continue;

		if (IS_SET(MODE_APPCURSOR) ? kp->appcursor < 0 : kp->appcursor > 0)
			continue;

		return kp->s;
	}

	return NULL;
}

void
kpress(XEvent *ev)
{
	xactivate();
	static int firstkey = 1;
	XKeyEvent *e = &ev->xkey;
	KeySym ksym = NoSymbol;
	char buf[64], *customkey;
	int len;
	Rune c;
	Status status;
	Shortcut *bp;

	if (IS_SET(MODE_KBDLOCK))
		return;
	if (firstkey) {
		firstkey = 0;
		xstartuptime("key");
	}

	if (xw.ime.xic) {
		len = XmbLookupString(xw.ime.xic, e, buf, sizeof buf, &ksym, &status);
		if (status == XBufferOverflow)
			return;
	} else {
		len = XLookupString(e, buf, sizeof buf, &ksym, NULL);
	}
	if (e->state & ControlMask) {
		Tab *tab, *selected = xtabfor(view->terminal), *previous = NULL;
		switch (ksym) {
		case XK_t: case XK_T:
			xnewtab(); return;
		case XK_n: case XK_N: {
			char *old_title = opt_title, *old_line = opt_line, *old_io = opt_io;
			char **old_cmd = opt_cmd;
			opt_title = "Worminal";
			opt_line = opt_io = NULL;
			opt_cmd = NULL;
			xaddview();
			opt_title = old_title;
			opt_line = old_line;
			opt_io = old_io;
			opt_cmd = old_cmd;
			return;
		}
		case XK_w: case XK_W:
			xclosetab(selected); return;
		case XK_Tab: case XK_ISO_Left_Tab:
			if (e->state & ShiftMask || ksym == XK_ISO_Left_Tab) {
				for (tab = tabs; tab && tab != selected; tab = tab->next)
					previous = tab;
				xselecttab(previous ? previous : lasttab);
			} else {
				xselecttab(selected->next ? selected->next : tabs);
			}
			return;
		default:
			if (BETWEEN(ksym, XK_1, XK_9)) {
				tab = ksym == XK_9 ? lasttab : xtabslot(ksym - XK_0);
				xselecttab(tab ? tab : lasttab);
				return;
			}
		}
	}
	if (e->state & Mod1Mask && BETWEEN(ksym, XK_0, XK_9)) {
		xselecttab(xtabslot(ksym == XK_0 ? 10 : ksym - XK_0));
		return;
	}
	/* 1. shortcuts */
	for (bp = shortcuts; bp < shortcuts + LEN(shortcuts); bp++) {
		if (ksym == bp->keysym && match(bp->mod, e->state)) {
			bp->func(&(bp->arg));
			return;
		}
	}

	/* 2. custom keys from config.h */
	if ((customkey = kmap(ksym, e->state))) {
		ttywrite(customkey, strlen(customkey), 1);
		return;
	}

	/* 3. composed string from input method */
	if (len == 0)
		return;
	if (len == 1 && e->state & Mod1Mask) {
		if (IS_SET(MODE_8BIT)) {
			if (*buf < 0177) {
				c = *buf | 0x80;
				len = utf8encode(c, buf);
			}
		} else {
			buf[1] = buf[0];
			buf[0] = '\033';
			len = 2;
		}
	}
	ttywrite(buf, len, 1);
}

void
cmessage(XEvent *e)
{
	/*
	 * See xembed specs
	 *  http://standards.freedesktop.org/xembed-spec/xembed-spec-latest.html
	 */
	if (e->xclient.message_type == xw.xembed && e->xclient.format == 32) {
		if (e->xclient.data.l[1] == XEMBED_FOCUS_IN) {
			current_window.mode |= MODE_FOCUSED;
			xseturgency(0);
		} else if (e->xclient.data.l[1] == XEMBED_FOCUS_OUT) {
			current_window.mode &= ~MODE_FOCUSED;
		}
	} else if (e->xclient.data.l[0] == xw.wmdeletewin) {
		xremoveview(0);
	}
}

void
resize(XEvent *e)
{
	if (e->xconfigure.width == current_window.w && e->xconfigure.height == current_window.h)
		return;
	if (view->live) {
		cresize(e->xconfigure.width, e->xconfigure.height);
	} else {
		Pixmap old = xw.buf;
		int oldw = current_window.w, oldh = current_window.h;
		current_window.w = e->xconfigure.width;
		current_window.h = e->xconfigure.height;
		xw.buf = XCreatePixmap(xw.dpy, xw.win, current_window.w,
		                       current_window.h, DefaultDepth(xw.dpy, xw.scr));
		XSetForeground(xw.dpy, dc.gc, dc.col[defaultbg].pixel);
		XFillRectangle(xw.dpy, xw.buf, dc.gc, 0, 0,
		               current_window.w, current_window.h);
		XCopyArea(xw.dpy, old, xw.buf, dc.gc, 0, 0,
		          MIN(oldw, current_window.w), MIN(oldh, current_window.h), 0, 0);
		XFreePixmap(xw.dpy, old);
		XftDrawChange(xw.draw, xw.buf);
	}
}

void
run(void)
{
	XEvent ev;
	XWindowAttributes attrs;
	XView *candidate;
	fd_set rfd;
	int xfd = XConnectionNumber(xw.dpy), maxfd, xev, drawing, ptyevent;
	struct timespec seltv, *tv, now, lastblink, trigger;
	double timeout;

	/* Waiting for window mapping */
	do {
		XNextEvent(xw.dpy, &ev);
		/*
		 * This XFilterEvent call is required because of XOpenIM. It
		 * does filter out the key event and some client message for
		 * the input method too.
		 */
		if (XFilterEvent(&ev, None))
			continue;
	} while (ev.type != MapNotify);
	xstartupevent("mapped", 0);
	xstartuptime("map-observed");

	/* Mapping precedes the final resize; query the actual server geometry. */
	if (!XGetWindowAttributes(xw.dpy, xw.win, &attrs))
		die("could not read window geometry\n");
	cresize(attrs.width, attrs.height);
	xstartuptime("resize");

	for (timeout = -1, drawing = 0, lastblink = (struct timespec){0};;) {
		FD_ZERO(&rfd);
		maxfd = MAX(xfd, workspacefd);
		FD_SET(xfd, &rfd);
		FD_SET(workspacefd, &rfd);

		if (XPending(xw.dpy))
			timeout = 0;  /* existing events might not set xfd */

		seltv.tv_sec = timeout / 1E3;
		seltv.tv_nsec = 1E6 * (timeout - 1E3 * seltv.tv_sec);
		tv = timeout >= 0 ? &seltv : NULL;

		if (pselect(maxfd+1,
		            &rfd, NULL, NULL, tv, NULL) < 0) {
			if (errno == EINTR)
				continue;
			die("select failed: %s\n", strerror(errno));
		}
		clock_gettime(CLOCK_MONOTONIC, &now);

		ptyevent = 0;
		if (FD_ISSET(workspacefd, &rfd)) {
			WirePacket packet;
			if (!wire_read(workspacefd, &packet))
				die("workspace connection closed\n");
			workspace_message(&packet);
			wire_packet_free(&packet);
			ptyevent = 1;
		}

		xev = 0;
		while (XPending(xw.dpy)) {
			xev = 1;
			XNextEvent(xw.dpy, &ev);
			XView *target = xviewfor(ev.xany.window);
			if (target)
				xsetview(target);
			if (XFilterEvent(&ev, None))
				continue;
			if (!target)
				continue;
			if (handler[ev.type])
				(handler[ev.type])(&ev);
		}

		/*
		 * To reduce flicker and tearing, when new content or event
		 * triggers drawing, we first wait a bit to ensure we got
		 * everything, and if nothing new arrives - we draw.
		 * We start with trying to wait minlatency ms. If more content
		 * arrives sooner, we retry with shorter and shorter periods,
		 * and eventually draw even without idle after maxlatency ms.
		 * Typically this results in low latency while interacting,
		 * maximum latency intervals during `cat huge.txt`, and perfect
		 * sync with periodic updates from animations/key-repeats/etc.
		 */
		if (ptyevent || xev) {
			if (!drawing) {
				trigger = now;
				drawing = 1;
			}
			timeout = (maxlatency - TIMEDIFF(now, trigger)) \
			          / maxlatency * minlatency;
			if (timeout > 0)
				continue;  /* we have time, try to find idle */
		}

		/* idle detected or maxlatency exhausted -> draw */
		timeout = -1;
		if (blinktimeout && tattrset(ATTR_BLINK)) {
			timeout = blinktimeout - TIMEDIFF(now, lastblink);
			if (timeout <= 0) {
				if (-timeout > blinktimeout) /* start visible */
					current_window.mode |= MODE_BLINK;
				current_window.mode ^= MODE_BLINK;
				tsetdirtattr(ATTR_BLINK);
				lastblink = now;
				timeout = blinktimeout;
			}
		}

		for (candidate = views; candidate; candidate = candidate->next)
			if (candidate->live) {
				xsetview(candidate);
				draw();
			}
		XFlush(xw.dpy);
		drawing = 0;
	}
}

void
usage(void)
{
	die("usage: %s [-aiv] [-c class] [-f font] [-g geometry]"
	    " [-n name] [-o file]\n"
	    "          [-T title] [-t title] [-w windowid]"
	    " [[-e] command [args ...]]\n"
	    "       %s [-aiv] [-c class] [-f font] [-g geometry]"
	    " [-n name] [-o file]\n"
	    "          [-T title] [-t title] [-w windowid] -l line"
	    " [stty_args ...]\n", argv0, argv0);
}

int
main(int argc, char *argv[])
{
	xstartuptime("main");
	if (argc >= 3 && strcmp(argv[1], "--master") == 0) {
		master_target = argv[2];
		memmove(argv + 1, argv + 3, (argc - 2) * sizeof(*argv));
		argc -= 2;
	}
	xw.l = xw.t = 0;
	xw.isfixed = False;
	xsetcursor(cursorshape);

	ARGBEGIN {
	case 'a':
		allowaltscreen = 0;
		break;
	case 'c':
		opt_class = EARGF(usage());
		break;
	case 'e':
		if (argc > 0)
			--argc, ++argv;
		goto run;
	case 'f':
		opt_font = EARGF(usage());
		break;
	case 'g':
		xw.gm = XParseGeometry(EARGF(usage()),
				&xw.l, &xw.t, &cols, &rows);
		break;
	case 'i':
		xw.isfixed = 1;
		break;
	case 'o':
		opt_io = EARGF(usage());
		break;
	case 'l':
		opt_line = EARGF(usage());
		break;
	case 'n':
		opt_name = EARGF(usage());
		break;
	case 't':
	case 'T':
		opt_title = EARGF(usage());
		break;
	case 'w':
		opt_embed = EARGF(usage());
		break;
	case 'v':
		die("%s " VERSION "\n", argv0);
		break;
	default:
		usage();
	} ARGEND;

run:
	if (argc > 0) /* eat all remaining arguments */
		opt_cmd = argv;
	launch_argc = argc;

	if (!opt_title)
		opt_title = (opt_line || !opt_cmd) ? "Worminal" : opt_cmd[0];

	setlocale(LC_CTYPE, "");
	XSetLocaleModifiers("");
	cols = MAX(cols, 1);
	rows = MAX(rows, 1);
	workspacefd = workspace_client_open(master_target);
	if (workspacefd < 0)
		die("could not connect to Worminal workspace: %s\n", strerror(errno));
	xstartuptime("workspace");
	ttywritehook = workspace_write;
	tscrollhook = workspace_scroll;
	tnew(cols, rows);
	view->terminal = tsessioncurrent();
	tsessionallowalt(view->terminal, allowaltscreen);
	xtabnew(view->terminal, opt_title, NULL);
	pending_view = view;
	pending_new_tab = 1;
	xinit(cols, rows);
	selinit();
	run();

	return 0;
}
