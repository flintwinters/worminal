/* Inspect a compact terminal window on a private Xvfb display. */
#include <stdio.h>
#include <stdlib.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>

int
main(int argc, char **argv)
{
	Display *display;
	XWindowAttributes attrs;
	XImage *image;
	Window window;
	int cw, ch, x, y, row0 = 0, spill = 0;
	unsigned long background;

	if (argc != 2 || !(display = XOpenDisplay(NULL)))
		return 2;
	window = strtoul(argv[1], NULL, 0);
	XSync(display, False);
	if (!XGetWindowAttributes(display, window, &attrs) ||
	    attrs.width < 24 || attrs.height < 10)
		return 2;
	image = XGetImage(display, window, 0, 0, attrs.width, attrs.height,
	                  AllPlanes, ZPixmap);
	if (!image)
		return 2;
	cw = (attrs.width - 4) / 10;
	ch = (attrs.height - 4) / 2;
	background = XGetPixel(image, 2 + 5 * cw, 2 + ch);
	for (x = 2; x < 2 + cw; x++) {
		for (y = 2; y < 2 + ch; y++)
			row0 += XGetPixel(image, x, y) != background;
		for (y = 2 + ch; y < 2 + 2 * ch; y++)
			spill += XGetPixel(image, x, y) != background;
	}
	XDestroyImage(image);
	XCloseDisplay(display);
	printf("glyph=%d below-row=%d cell=%dx%d\n", row0, spill, cw, ch);
	return row0 == 0 ? 2 : spill == 0 ? 1 : 0;
}
