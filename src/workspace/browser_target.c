#define _GNU_SOURCE
#include <arpa/inet.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/stat.h>
#include <unistd.h>
#include "browser_target.h"

static int
alpha(unsigned char c)
{
	return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
}

/* Without a scheme, only a recognizable authority is a web target. Existing
 * paths win below; ./ disambiguates a missing dotted filename from a hostname. */
static int
web_authority(const char *text)
{
	const char *end = text + strcspn(text, "/?#");
	const char *host_end = end;
	if (*text == '[') {
		const char *close = memchr(text, ']', end - text);
		if (!close) return 0;
		char *host = strndup(text + 1, close - text - 1);
		struct in6_addr address;
		int valid = host && inet_pton(AF_INET6, host, &address) == 1;
		free(host);
		if (!valid) return 0;
		host_end = close + 1;
	} else {
		const char *colon = memchr(text, ':', end - text);
		if (colon) host_end = colon;
		int dotted = 0;
		for (const char *p = text; p < host_end; p++) {
			if (*p == '.') {
				if (p == text || p[-1] == '.' || p[-1] == '-') return 0;
				dotted = 1;
			} else if (!(alpha(*p) || (*p >= '0' && *p <= '9') ||
			             (*p == '-' && p > text && p[-1] != '.'))) return 0;
		}
		if (host_end == text || host_end[-1] == '-' ||
		    (!dotted && !((host_end - text) == 9 && !strncasecmp(text, "localhost", 9)))) return 0;
	}
	if (host_end == end) return 1;
	if (*host_end++ != ':' || host_end == end) return 0;
	for (; host_end < end; host_end++) if (*host_end < '0' || *host_end > '9') return 0;
	return 1;
}

char *
workspace_browser_target(const char *text)
{
	if (!text) return strdup(WORKSPACE_BROWSER_HOME);
	if (!*text) return strdup(text);
	const unsigned char *scheme = (const unsigned char *)text;
	int web = web_authority(text);
	if (!web && alpha(*scheme)) {
		for (scheme++; alpha(*scheme) || (*scheme >= '0' && *scheme <= '9') ||
		     *scheme == '+' || *scheme == '-' || *scheme == '.'; scheme++) {}
		if (*scheme == ':') return strdup(text);
	}
	/* Resolve in the command's cwd, before a service with another cwd owns the tab.
	 * Real paths also collapse ./ and ../; a missing file still gets a useful file URL. */
	char *path = realpath(text, NULL);
	struct stat info;
	if (!path && web && lstat(text, &info) < 0 && (errno == ENOENT || errno == ENOTDIR)) {
		char *url = NULL;
		if (asprintf(&url, "https://%s", text) < 0) return NULL;
		return url;
	}
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
	char *url = malloc(12 + 3 * length);
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
