#ifndef WORMINAL_BROWSER_MODULE_H
#define WORMINAL_BROWSER_MODULE_H

#include <gmodule.h>
#include "preview.h"

#define WORMINAL_BROWSER_ABI 1

typedef struct {
	Browser *(*create)(App *);
	GtkWidget *(*widget)(Browser *);
	GtkWidget *(*view)(Browser *);
	void (*bind)(Browser *, Page *, const char *);
	gboolean (*shortcut)(Page *, guint, guint);
	void (*zoom)(Browser *, double);
	void (*destroy)(Browser *);
} BrowserModule;

#endif
