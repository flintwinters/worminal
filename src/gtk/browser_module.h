#ifndef WORMINAL_BROWSER_MODULE_H
#define WORMINAL_BROWSER_MODULE_H

#include <gmodule.h>
#include "preview.h"

#define WORMINAL_BROWSER_ABI 3
/* Shared framework-owned profile; module-private contexts and Browser structs stay separate. */
#define WORMINAL_BROWSER_PROFILE_KEY "worminal-browser-profile-v1"
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

/* Optional ABI-compatible generation lifecycle hook. A retired module must keep
 * live views usable and may release its context once no views remain. */
G_MODULE_EXPORT void worminal_browser_set_retired_v1(gboolean);

#endif
