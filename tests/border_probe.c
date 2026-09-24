/* Check the border pixels that the terminal itself presents to X11 clients. */
#include <stdio.h>
#include <stdlib.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>

int
main(int argc, char **argv)
{
	Display *display;
	Window window;
	XWindowAttributes attrs;
	XImage *image;
	XColor gray = {.red = 0x9999, .green = 0x9999, .blue = 0x9999};
	int ok;

	if (argc != 2 || !(display = XOpenDisplay(NULL)))
		return 2;
	window = strtoul(argv[1], NULL, 10);
	if (!XGetWindowAttributes(display, window, &attrs) ||
	    attrs.width < 3 || attrs.height < 3 ||
	    !(image = XGetImage(display, window, 0, 0, 3, 3,
	                        AllPlanes, ZPixmap))) {
		XCloseDisplay(display);
		return 2;
	}
	if (!XAllocColor(display, attrs.colormap, &gray)) {
		XDestroyImage(image);
		XCloseDisplay(display);
		return 2;
	}
	ok = attrs.border_width == 0 &&
	     XGetPixel(image, 0, 0) == gray.pixel &&
	     XGetPixel(image, 1, 0) == gray.pixel &&
	     XGetPixel(image, 0, 1) == gray.pixel &&
	     XGetPixel(image, 1, 1) != gray.pixel;
	XDestroyImage(image);
	XCloseDisplay(display);
	if (!ok)
		fputs("client edge is not a one-pixel #999999 border\n", stderr);
	return ok ? 0 : 1;
}
