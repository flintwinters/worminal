#ifndef WORMINAL_BROWSER_TARGET_H
#define WORMINAL_BROWSER_TARGET_H

#define WORKSPACE_BROWSER_SCHEME "worminal"
#define WORKSPACE_BROWSER_HOME "about:blank"

/* Existing paths beat bare hosts; ./ marks missing local paths. Caller owns the result.
 * NULL selects a blank page internally; explicit empty strings stay invalid. */
char *workspace_browser_target(const char *);

#endif
