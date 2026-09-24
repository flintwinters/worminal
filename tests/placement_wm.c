/* Minimal X11 window manager for placement regression checks on Xvfb. */
#include <stdio.h>
#include <stdlib.h>
#include <X11/Xlib.h>

int
main(void)
{
	Display *display = XOpenDisplay(NULL);
	Window root;
	XEvent event;
	XWindowChanges changes;

	if (!display)
		return 1;
	root = DefaultRootWindow(display);
	XSelectInput(display, root, SubstructureRedirectMask);
	XSync(display, False);
	puts("ready");
	fflush(stdout);

	for (;;) {
		XNextEvent(display, &event);
		if (event.type == MapRequest) {
			if (getenv("WORMINAL_CLOSE_ON_MAP")) {
				XDestroyWindow(display, event.xmaprequest.window);
				XSync(display, False);
				puts("closed");
				fflush(stdout);
				continue;
			}
			XMoveWindow(display, event.xmaprequest.window, 100, 80);
			XMapWindow(display, event.xmaprequest.window);
			XSync(display, False);
			puts("mapped");
			fflush(stdout);
		} else if (event.type == ConfigureRequest) {
			changes.x = event.xconfigurerequest.x;
			changes.y = event.xconfigurerequest.y;
			changes.width = event.xconfigurerequest.width;
			changes.height = event.xconfigurerequest.height;
			changes.border_width = event.xconfigurerequest.border_width;
			changes.sibling = event.xconfigurerequest.above;
			changes.stack_mode = event.xconfigurerequest.detail;
			XConfigureWindow(display, event.xconfigurerequest.window,
			                 event.xconfigurerequest.value_mask, &changes);
			XSync(display, False);
			puts("configured");
			fflush(stdout);
		}
	}
}
