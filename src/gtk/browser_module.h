#ifndef WORMINAL_BROWSER_MODULE_H
#define WORMINAL_BROWSER_MODULE_H

#include <gmodule.h>
#include "preview.h"

#define WORMINAL_BROWSER_ABI 3
#define WORMINAL_STRINGIFY_(value) #value
#define WORMINAL_STRINGIFY(value) WORMINAL_STRINGIFY_(value)
#define WORMINAL_BROWSER_MODULE "browser-v" WORMINAL_STRINGIFY(WORMINAL_BROWSER_ABI) ".so"

/* The engine never resolves frontend symbols through the global linker scope. */
typedef struct {
	void (*title)(Page *, const char *);
	void (*navigate)(Page *, const char *);
	void (*timing)(const char *);
	void (*open)(App *, const char *);
	void (*close)(Page *);
	void (*select)(Page *);
} BrowserHost;

typedef struct {
	Browser *(*create)(App *, const char *);
	GtkWidget *(*widget)(Browser *);
	GtkWidget *(*view)(Browser *);
	void (*bind)(Browser *, Page *, const char *);
	gboolean (*shortcut)(Page *, guint, guint);
	void (*zoom)(Browser *, double);
	void (*destroy)(Browser *);
	void (*initialize)(const BrowserHost *);
} BrowserModule;

#endif
