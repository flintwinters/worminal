/* Draw into two real X windows through independent Worminal view contexts. */
#define _GNU_SOURCE
#include <X11/extensions/XTest.h>
#define main worminal_main
#include "../x.c"
#undef main

void
die(const char *format, ...)
{
	fputs(format, stderr);
	exit(1);
}

void
ttywrite(const char *data, size_t len, int echo)
{
	die("unexpected PTY focus report\n");
}

void
tsessionuse(TermSession *chosen)
{
	/* This fixture checks view routing without constructing a PTY session. */
}

TermSession *tsessioncurrent(void) { return view->terminal; }

char *
xstrdup(const char *value)
{
	char *copy = strdup(value);
	if (!copy)
		die("proof allocation failed\n");
	return copy;
}

void tresize(int cols, int rows) { die("unexpected terminal resize\n"); }
void ttyresize(int width, int height) { die("unexpected PTY resize\n"); }
void redraw(void) { die("unexpected terminal redraw\n"); }

void *
xmalloc(size_t size)
{
	void *result = malloc(size);
	if (!result)
		die("proof allocation failed\n");
	return result;
}

void *
xrealloc(void *pointer, size_t size)
{
	void *result = realloc(pointer, size);
	if (!result)
		die("proof allocation failed\n");
	return result;
}

static void
setup_view(XView *target, Display *display, int x, unsigned long color)
{
	XWMHints hints = {.flags = InputHint, .input = True};

	view = target;
	target->live = 1;
	xw.dpy = display;
	xw.scr = DefaultScreen(display);
	xw.vis = DefaultVisual(display, xw.scr);
	xw.cmap = DefaultColormap(display, xw.scr);
	xw.win = XCreateSimpleWindow(display, DefaultRootWindow(display),
	                             x + 300, 300, 100, 60, 0, 0, 0);
	if (!getenv("WORMINAL_PROOF_MANAGED_VIEWS")) {
		XSetWindowAttributes attributes = {.override_redirect = True};
		XChangeWindowAttributes(display, xw.win, CWOverrideRedirect, &attributes);
	}
	XSetWMHints(display, xw.win, &hints);
	XSelectInput(display, xw.win,
	             FocusChangeMask | KeyPressMask | ButtonPressMask);
	current_window.w = 100;
	current_window.h = 60;
	xw.buf = XCreatePixmap(display, xw.win, 100, 60,
	                      DefaultDepth(display, xw.scr));
	dc.gc = XCreateGC(display, xw.win, 0, NULL);
	xloadcolsone();
	dc.border.pixel = WhitePixel(display, xw.scr);
	XSetForeground(display, dc.gc, color);
	XFillRectangle(display, xw.buf, dc.gc, 0, 0, 100, 60);
	XMapWindow(display, xw.win);
}

static void
wait_viewable(Display *display, Window window)
{
	XWindowAttributes attrs;
	struct timespec pause = {.tv_nsec = 10000000};

	for (int attempt = 0; attempt < 500; attempt++) {
		if (XGetWindowAttributes(display, window, &attrs) &&
		    attrs.map_state == IsViewable) {
			/* Managed windows can be briefly unmapped while reparenting. */
			nanosleep(&pause, NULL);
			if (XGetWindowAttributes(display, window, &attrs) &&
			    attrs.map_state == IsViewable)
				return;
		}
		nanosleep(&pause, NULL);
	}
	die("proof window was not mapped by the window manager\n");
}

static void
drain_focus(Display *display)
{
	XEvent event;
	XView *target;

	XSync(display, False);
	while (XPending(display)) {
		XNextEvent(display, &event);
		target = xviewfor(event.xany.window);
		if (target && (event.type == FocusIn || event.type == FocusOut)) {
			view = target;
			focus(&event);
		}
	}
}

static void
focus_view(Display *display, Window window)
{
	Window focused;
	int revert_to;
	char command[160];
	struct timespec pause = {.tv_nsec = 50000000};

	/* A managed window can still be receiving its initial focus decision
	 * after it becomes viewable. Verify that our requested focus sticks. */
	for (int attempt = 0; attempt < 40; attempt++) {
		if (getenv("WORMINAL_PROOF_MANAGED_VIEWS")) {
			snprintf(command, sizeof(command),
			         "xdotool windowactivate --sync %lu && "
			         "xdotool windowfocus --sync %lu",
			         (unsigned long)window, (unsigned long)window);
			if (system(command) != 0)
				die("could not activate managed view\n");
		} else {
			XSetInputFocus(display, window, RevertToNone, CurrentTime);
		}
		XSync(display, False);
		nanosleep(&pause, NULL);
		XGetInputFocus(display, &focused, &revert_to);
		drain_focus(display);
		if (focused == window)
			return;
	}
	fprintf(stderr, "view did not retain X focus: want=%lu got=%lu\n",
	        (unsigned long)window, (unsigned long)focused);
	exit(1);
}

static int image_error_code;

static int
image_error(Display *display, XErrorEvent *error)
{
	(void)display;
	image_error_code = error->error_code;
	return 0;
}

static unsigned long
interior_pixel(Display *display, Window window)
{
	XImage *image;
	XWindowAttributes attrs;
	unsigned long pixel;
	int (*previous)(Display *, XErrorEvent *);
	struct timespec pause = {.tv_nsec = 10000000};

	for (int attempt = 0; attempt < 200; attempt++) {
		if (!XGetWindowAttributes(display, window, &attrs) ||
		    attrs.map_state != IsViewable) {
			nanosleep(&pause, NULL);
			continue;
		}
		/* Reparenting can race a GetImage request after map-state query. */
		XSync(display, False);
		image_error_code = 0;
		previous = XSetErrorHandler(image_error);
		image = XGetImage(display, window, 5, 5, 1, 1,
		                  AllPlanes, ZPixmap);
		XSync(display, False);
		XSetErrorHandler(previous);
		if (image_error_code && image_error_code != BadMatch)
			die("unexpected X error while reading view pixels\n");
		if (image_error_code) {
			if (image)
				XDestroyImage(image);
			nanosleep(&pause, NULL);
			continue;
		}
		if (image) {
			pixel = XGetPixel(image, 0, 0);
			XDestroyImage(image);
			return pixel;
		}
		nanosleep(&pause, NULL);
	}
	die("could not read view pixels\n");
	return 0;
}

int
main(void)
{
	const struct { const char *path, *label; } cases[] = {
		{"/home/felix/projects/worminal/", "worminal"},
		{"/home/felix/projects/.checks", ".checks"},
		{"/", "/"},
	};
	Display *display = XOpenDisplay(NULL);
	XView first = {0}, second = {0};
	unsigned long black, white;
	Window first_window, second_window;
	Window previous_focus;
	int revert_to;
	XColor gray = {.red = 0x7777, .green = 0x7777, .blue = 0x7777};
	for (size_t i = 0; i < LEN(cases); i++) {
		int length;
		const char *label = xtablabel(cases[i].path, &length);
		if (length != strlen(cases[i].label) ||
		    strncmp(label, cases[i].label, length) != 0)
			die("tab label is not the final directory segment\n");
	}

	if (!display)
		return 2;
	XGetInputFocus(display, &previous_focus, &revert_to);
	black = BlackPixel(display, DefaultScreen(display));
	white = WhitePixel(display, DefaultScreen(display));
	if (!XAllocColor(display, DefaultColormap(display, DefaultScreen(display)), &gray))
		return 2;
	setup_view(&first, display, 0, black);
	first_window = xw.win;
	setup_view(&second, display, 150, white);
	second_window = xw.win;
	wait_viewable(display, first_window);
	wait_viewable(display, second_window);
	view = &first;
	xfinishdraw();
	view = &second;
	xfinishdraw();
	views = &first;
	first.next = &second;
	if (xviewfor(first_window) != &first ||
	    xviewfor(second_window) != &second)
		die("X events did not resolve to their view\n");
	XSync(display, False);
	if (interior_pixel(display, first_window) != black ||
	    interior_pixel(display, second_window) != white)
		die("view drawing leaked between windows\n");
	view = &first;
	XSetForeground(display, dc.gc, gray.pixel);
	XFillRectangle(display, xw.buf, dc.gc, 0, 0, 100, 60);
	xfinishdraw();
	XSync(display, False);
	if (interior_pixel(display, first_window) != gray.pixel ||
	    interior_pixel(display, second_window) != white)
		die("view redraw affected the other window\n");

	focus_view(display, first_window);
	view = &first;
	if (!(current_window.mode & MODE_FOCUSED))
		die("first view did not receive focus\n");
	focus_view(display, second_window);
	view = &first;
	if (current_window.mode & MODE_FOCUSED)
		die("first view kept focus\n");
	view = &second;
	if (!(current_window.mode & MODE_FOCUSED))
		die("second view did not receive focus\n");

	KeyCode key = XKeysymToKeycode(display, XK_z);
	XEvent event;
	int keys = 0, clicks = 0;
	XTestFakeKeyEvent(display, key, True, CurrentTime);
	XTestFakeKeyEvent(display, key, False, CurrentTime);
	XRaiseWindow(display, second_window);
	struct timespec settle = {.tv_nsec = 150000000};
	nanosleep(&settle, NULL);
	int pointer_x, pointer_y;
	Window pointer_child;
	XTranslateCoordinates(display, second_window, DefaultRootWindow(display),
	                      5, 5, &pointer_x, &pointer_y, &pointer_child);
	XTestFakeMotionEvent(display, DefaultScreen(display), pointer_x, pointer_y,
	                     CurrentTime);
	XSync(display, False);
	XTestFakeButtonEvent(display, 1, True, CurrentTime);
	XTestFakeButtonEvent(display, 1, False, CurrentTime);
	XTestFakeButtonEvent(display, 1, True, CurrentTime);
	XTestFakeButtonEvent(display, 1, False, CurrentTime);
	XSync(display, False);
	while (XPending(display)) {
		XNextEvent(display, &event);
		if (event.type == KeyPress || event.type == ButtonPress) {
			if (xviewfor(event.xany.window) != &second)
				die("input reached the wrong view\n");
			keys += event.type == KeyPress;
			clicks += event.type == ButtonPress;
		}
	}
	if (keys < 1 || clicks < 1) {
		Window root, child;
		int rx, ry, wx, wy;
		unsigned int mask;
		XQueryPointer(display, second_window, &root, &child,
		              &rx, &ry, &wx, &wy, &mask);
		fprintf(stderr, "focused view missed input: keys=%d clicks=%d pointer=%d,%d child=%lu\n",
		        keys, clicks, wx, wy, (unsigned long)child);
		return 1;
	}

	/* XIM callbacks must update their own view even when another view is current. */
	view = &first;
	xw.ime.xic = (XIC)1;
	view = &second;
	xw.ime.xic = (XIC)2;
	xicdestroy(NULL, (XPointer)&first, NULL);
	view = &first;
	if (xw.ime.xic != NULL)
		die("XIM callback updated the wrong view\n");
	view = &second;
	if (xw.ime.xic != (XIC)2)
		die("XIM callback changed another view\n");
	xw.ime.xic = NULL;
	setlocale(LC_CTYPE, "");
	XSetLocaleModifiers("");
	view = &first;
	int first_im = ximopen(display);
	int first_ic = xw.ime.xic != NULL;
	view = &second;
	int second_im = ximopen(display);
	int second_ic = xw.ime.xic != NULL;
	if (first_im != second_im || first_ic != second_ic ||
	    (getenv("WORMINAL_PROOF_REQUIRE_IM") && !first_ic)) {
		fprintf(stderr, "input method contexts: first=%d/%d second=%d/%d\n",
		        first_im, first_ic, second_im, second_ic);
		return 1;
	}

	/* Reloading one view's fonts must leave the other view drawable. */
	if (!FcInit())
		die("fontconfig did not initialize\n");
	view = &first;
	usedfont = font;
	xloadfonts(usedfont, 0);
	view = &second;
	usedfont = font;
	xloadfonts(usedfont, 0);
	FcPattern *second_pattern = stylepattern;
	XftFont *second_font = dc.font.match;
	view = &first;
	xunloadfonts();
	xloadfonts(usedfont, 0);
	view = &second;
	if (stylepattern != second_pattern || dc.font.match != second_font)
		die("font resources leaked between views\n");
	Glyph glyph = {.u = 'A', .fg = defaultfg, .bg = defaultbg};
	XftGlyphFontSpec spec;
	if (xmakeglyphfontspecs(&spec, &glyph, 1, 0, 0) != 1 ||
	    spec.font != second_font)
		die("second view could not shape text after first reloaded fonts\n");

	/* A tab's OSC palette changes reach every selected view. */
	first.terminal = second.terminal = (TermSession *)1;
	view = &first;
	if (xsetcolorname(1, "#ff0000"))
		die("tab color change failed\n");
	if (dc.col[1].color.red < 0xf000) {
		fprintf(stderr, "first red=%u\n", dc.col[1].color.red);
		return 1;
	}
	view = &second;
	if (dc.col[1].color.red < 0xf000)
		die("tab color did not reach both views\n");
	second.terminal = (TermSession *)2;
	view = &first;
	if (xsetcolorname(1, "#00ff00"))
		die("isolated tab color change failed\n");
	if (dc.col[1].color.green < 0xf000)
		die("tab color did not change first view\n");
	view = &second;
	if (dc.col[1].color.green != 0)
		die("tab color leaked across sessions\n");
	if (previous_focus != None && previous_focus != PointerRoot)
		XSetInputFocus(display, previous_focus, revert_to, CurrentTime);
	XSync(display, False);
	XCloseDisplay(display);
	return 0;
}
