#ifndef WORMINAL_WINDOWS_H
#define WORMINAL_WINDOWS_H
#include <glib.h>

typedef gboolean (*WindowLaunch)(int, char **, char **, const char *, int);
/* Forward launches to the local GTK owner, including the caller's stdout fd. */
int windows_forward(int, char **, int *);
guint windows_listen(int, WindowLaunch);
#endif
