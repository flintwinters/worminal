/* Draw into two real X windows through independent Worminal view contexts. */
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

static void
setup_view(XView *target, Display *display, int x, unsigned long color)
{
	XWMHints hints = {.flags = InputHint, .input = True};

	view = target;
	xw.dpy = display;
	xw.scr = DefaultScreen(display);
	xw.vis = DefaultVisual(display, xw.scr);
	xw.cmap = DefaultColormap(display, xw.scr);
	xw.win = XCreateSimpleWindow(display, DefaultRootWindow(display),
	                             x, 0, 16, 16, 0, 0, 0);
	XSetWMHints(display, xw.win, &hints);
	XSelectInput(display, xw.win,
	             FocusChangeMask | KeyPressMask | ButtonPressMask);
	current_window.w = current_window.h = 16;
	xw.buf = XCreatePixmap(display, xw.win, 16, 16,
	                      DefaultDepth(display, xw.scr));
	dc.gc = XCreateGC(display, xw.win, 0, NULL);
	dc.col = calloc(defaultbg + 1, sizeof(Color));
	dc.colloaded = calloc(defaultbg + 1, 1);
	dc.col[defaultbg].pixel = BlackPixel(display, xw.scr);
	dc.colloaded[defaultbg] = 1;
	dc.border.pixel = WhitePixel(display, xw.scr);
	XSetForeground(display, dc.gc, color);
	XFillRectangle(display, xw.buf, dc.gc, 0, 0, 16, 16);
	XMapWindow(display, xw.win);
}

static void
wait_viewable(Display *display, Window window)
{
	XWindowAttributes attrs;
	struct timespec pause = {.tv_nsec = 10000000};

	for (int attempt = 0; attempt < 500; attempt++) {
		if (XGetWindowAttributes(display, window, &attrs) &&
		    attrs.map_state == IsViewable)
			return;
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

static unsigned long
interior_pixel(Display *display, Window window)
{
	XImage *image = XGetImage(display, window, 5, 5, 1, 1,
	                          AllPlanes, ZPixmap);
	unsigned long pixel;

	if (!image)
		die("could not read view pixels\n");
	pixel = XGetPixel(image, 0, 0);
	XDestroyImage(image);
	return pixel;
}

int
main(void)
{
	Display *display = XOpenDisplay(NULL);
	XView first = {0}, second = {0};
	unsigned long black, white;
	Window first_window, second_window;
	Window previous_focus;
	int revert_to;
	XColor gray = {.red = 0x7777, .green = 0x7777, .blue = 0x7777};

	if (!display)
		return 2;
	XGetInputFocus(display, &previous_focus, &revert_to);
	black = BlackPixel(display, DefaultScreen(display));
	white = WhitePixel(display, DefaultScreen(display));
	if (!XAllocColor(display, DefaultColormap(display, DefaultScreen(display)), &gray))
		return 2;
	setup_view(&first, display, 0, black);
	first_window = xw.win;
	setup_view(&second, display, 30, white);
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
	XFillRectangle(display, xw.buf, dc.gc, 0, 0, 16, 16);
	xfinishdraw();
	XSync(display, False);
	if (interior_pixel(display, first_window) != gray.pixel ||
	    interior_pixel(display, second_window) != white)
		die("view redraw affected the other window\n");

	XSetInputFocus(display, first_window, RevertToNone, CurrentTime);
	drain_focus(display);
	view = &first;
	if (!(current_window.mode & MODE_FOCUSED))
		die("first view did not receive focus\n");
	XSetInputFocus(display, second_window, RevertToNone, CurrentTime);
	drain_focus(display);
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
	XWarpPointer(display, None, second_window, 0, 0, 0, 0, 5, 5);
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
	if (keys != 1 || clicks != 1)
		die("focused view missed key or mouse input\n");

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
	if (previous_focus != None && previous_focus != PointerRoot)
		XSetInputFocus(display, previous_focus, revert_to, CurrentTime);
	XSync(display, False);
	XCloseDisplay(display);
	return 0;
}
