#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "browser_target.h"

static int
alpha(unsigned char c)
{
	return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
}

char *
workspace_browser_target(const char *text)
{
	if (!text) return strdup(WORKSPACE_BROWSER_HOME);
	if (!*text) return strdup(text);
	const unsigned char *scheme = (const unsigned char *)text;
	if (alpha(*scheme)) {
		for (scheme++; alpha(*scheme) || (*scheme >= '0' && *scheme <= '9') ||
		     *scheme == '+' || *scheme == '-' || *scheme == '.'; scheme++) {}
		if (*scheme == ':') return strdup(text);
	}
	/* Resolve in the command's cwd, before a service with another cwd owns the tab.
	 * Real paths also collapse ./ and ../; a missing file still gets a useful file URL. */
	char *path = realpath(text, NULL);
	if (!path) {
		if (*text == '/') path = strdup(text);
		else {
			char *cwd = getcwd(NULL, 0);
			if (!cwd) return NULL;
			if (asprintf(&path, "%s/%s", cwd, text) < 0) path = NULL;
			free(cwd);
		}
	}
	if (!path) return NULL;
	size_t length = strlen(path);
	if (length > 65536) { free(path); errno = ENAMETOOLONG; return NULL; }
	char *url = malloc(8 + 3 * length);
	if (!url) { free(path); return NULL; }
	char *out = stpcpy(url, "file://");
	const char hex[] = "0123456789ABCDEF";
	for (const unsigned char *p = (const unsigned char *)path; *p; p++) {
		if (alpha(*p) || (*p >= '0' && *p <= '9') || strchr("/-._~", *p)) *out++ = *p;
		else { *out++ = '%'; *out++ = hex[*p >> 4]; *out++ = hex[*p & 15]; }
	}
	*out = '\0';
	free(path);
	return url;
}
