/* Check the icon and class exposed to a panel by a mapped terminal window. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <X11/Xatom.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>

int
main(int argc, char **argv)
{
	Display *display;
	Window window;
	Atom actual;
	int format, ok, class_ok;
	unsigned long count, remaining, *pixels;
	unsigned char *data = NULL;
	XClassHint class;

	if (argc != 2 || !(display = XOpenDisplay(NULL)))
		return 2;
	window = strtoul(argv[1], NULL, 10);
	if (!XGetClassHint(display, window, &class)) {
		XCloseDisplay(display);
		return 2;
	}
	class_ok = !strcmp(class.res_name, "worminal") &&
	           !strcmp(class.res_class, "Worminal");
	XFree(class.res_name);
	XFree(class.res_class);
	if (XGetWindowProperty(display, window,
	                       XInternAtom(display, "_NET_WM_ICON", False),
	                       0, 48 * 48 + 2, False, XA_CARDINAL,
	                       &actual, &format, &count, &remaining,
	                       &data) != Success) {
		XCloseDisplay(display);
		return 2;
	}
	pixels = (unsigned long *)data;
	ok = class_ok && actual == XA_CARDINAL && format == 32 &&
	     count == 48 * 48 + 2 && remaining == 0 &&
	     pixels[0] == 48 && pixels[1] == 48 &&
	     /* Xlib sign-extends 32-bit property values to unsigned long on LP64. */
	     (pixels[2 + 24 * 48 + 24] & 0xffffffffUL) == 0xffffc0d4UL;
	if (!ok)
		fprintf(stderr, "window class or _NET_WM_ICON is incorrect: "
		        "class=%d type=%lu format=%d count=%lu remaining=%lu center=%08lx\n",
		        class_ok, actual, format, count, remaining,
		        data && count > 2 + 24 * 48 + 24 ? pixels[2 + 24 * 48 + 24] : 0UL);
	if (data)
		XFree(data);
	XCloseDisplay(display);
	return ok ? 0 : 1;
}
