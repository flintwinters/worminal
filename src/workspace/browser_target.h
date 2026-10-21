#ifndef WORMINAL_BROWSER_TARGET_H
#define WORMINAL_BROWSER_TARGET_H

#define WORKSPACE_BROWSER_SCHEME "worminal"
#define WORKSPACE_BROWSER_HOME WORKSPACE_BROWSER_SCHEME ":///files"

/* Caller owns the result. NULL selects home; an explicit empty string stays invalid. */
char *workspace_browser_target(const char *);

#endif
