#ifndef WORMINAL_BROWSER_TARGET_H
#define WORMINAL_BROWSER_TARGET_H

#define WORKSPACE_BROWSER_HOME "file:///"

/* Caller owns the result. NULL selects Files in the caller cwd; an explicit empty string stays invalid. */
char *workspace_browser_target(const char *);

#endif
