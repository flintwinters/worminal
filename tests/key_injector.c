/* Wait for the benchmark window on a private X display, then send one key. */
#define _POSIX_C_SOURCE 200809L
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/keysym.h>
#include <X11/extensions/XTest.h>

int
main(int argc, char **argv)
{
	Display *display;
	XEvent event;
	Window root;
	KeyCode key;
	XTextProperty title;
	struct timespec now;

	if ((argc != 2 && argc != 3) || !(display = XOpenDisplay(NULL)))
		return 1;
	root = DefaultRootWindow(display);
	XSelectInput(display, root, SubstructureNotifyMask);
	XSync(display, False);
	puts("ready");
	fflush(stdout);

	for (;;) {
		XNextEvent(display, &event);
		if (event.type != MapNotify ||
		    !XGetWMName(display, event.xmap.window, &title))
			continue;
		if (title.nitems != strlen(argv[1]) ||
		    memcmp(title.value, argv[1], title.nitems) != 0) {
			XFree(title.value);
			continue;
		}
		XFree(title.value);
		break;
	}

	key = XKeysymToKeycode(display, XK_z);
	if (!key)
		return 2;
	XSetInputFocus(display, event.xmap.window, RevertToParent, CurrentTime);
	XTestFakeKeyEvent(display, key, True, CurrentTime);
	XTestFakeKeyEvent(display, key, False, CurrentTime);
	if (argc == 3) {
		key = XKeysymToKeycode(display, XK_Return);
		if (!key)
			return 2;
		XTestFakeKeyEvent(display, key, True, CurrentTime);
		XTestFakeKeyEvent(display, key, False, CurrentTime);
	}
	XSync(display, False);
	clock_gettime(CLOCK_MONOTONIC, &now);
	printf("captured %lld\n",
	       (long long)now.tv_sec * 1000000000LL + now.tv_nsec);
	fflush(stdout);
	XCloseDisplay(display);
	return 0;
}
