#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <syslog.h>
#include <unistd.h>
#include "workspace_restore.h"
#include "restore_format.h"

/* Independent of the transport version; filenames isolate v1/v2 and scopes. */
typedef struct RestoreTab RestoreTab;
struct RestoreTab {
	char *directory, *url;
	RestoreTab *next;
};
static char state_path[PATH_MAX];
static int disabled;
static WireBuffer saved;

static int
prepare_path(void)
{
	const char *root = getenv("XDG_STATE_HOME"), *home = getenv("HOME");
	const char *scope = getenv("WORMINAL_SHARED_SOCKET_SCOPE");
	char directory[PATH_MAX], encoded[256];
	if (*state_path) return 1;
	if (!scope) scope = "";
	if (strlen(scope) > (sizeof(encoded) - 1) / 2) return 0;
	for (size_t i = 0; i <= strlen(scope); i++) {
		if (!scope[i]) { encoded[2 * i] = 0; break; }
		snprintf(encoded + 2 * i, 3, "%02x", (unsigned char)scope[i]);
	}
	int n = root && *root == '/' ? snprintf(directory, sizeof(directory), "%s/worminal", root)
	    : snprintf(directory, sizeof(directory), "%s/.local/state/worminal", home ? home : "");
	if (n < 0 || n >= (int)sizeof(directory) || directory[0] != '/') return 0;
	for (char *p = directory + 1; ; p++) {
		if (*p != '/' && *p) continue;
		char c = *p; *p = 0;
		if (mkdir(directory, 0700) < 0 && errno != EEXIST) return 0;
		*p = c;
		if (!c) break;
	}
	n = snprintf(state_path, sizeof(state_path), "%s/tabs-v%d-%s", directory, WIRE_VERSION, encoded);
	if (n < 0 || n >= (int)sizeof(state_path)) { *state_path = 0; return 0; }
	return 1;
}

static void
workspace_restore_free(RestoreTab *tab)
{
	while (tab) {
		RestoreTab *next = tab->next;
		free(tab->directory); free(tab->url); free(tab); tab = next;
	}
}

static RestoreTab *
workspace_restore_read(void)
{
	RestoreTab *head = NULL, **tail = &head;
	struct stat st;
	uint32_t version, count;
	if (!prepare_path()) { disabled = 1; return NULL; }
	int fd = open(state_path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
	if (fd < 0 && errno == ENOENT) return NULL;
	if (fd < 0) goto invalid;
	if (fstat(fd, &st) < 0 || !S_ISREG(st.st_mode) || st.st_size < 8 || st.st_size > RESTORE_LIMIT)
		goto invalid;
	saved.data = malloc(st.st_size); saved.len = saved.cap = st.st_size;
	if (!saved.data) abort();
	for (size_t pos = 0; pos < saved.len;) {
		ssize_t n = read(fd, saved.data + pos, saved.len - pos);
		if (n < 0 && errno == EINTR) continue;
		if (n <= 0) goto invalid;
		pos += n;
	}
	close(fd); fd = -1;
	WirePacket packet = {.data = saved.data, .len = saved.len};
	if (!wire_get_u32(&packet, &version) || version != RESTORE_VERSION ||
	    !wire_get_u32(&packet, &count) || count > RESTORE_TABS) goto invalid;
	for (uint32_t i = 0; i < count; i++) {
		RestoreTab *tab = calloc(1, sizeof(*tab));
		if (!tab) abort();
		*tail = tab; tail = &tab->next;
		size_t position = packet.pos;
		tab->directory = wire_get_string(&packet); tab->url = wire_get_string(&packet);
		if (!tab->directory || !tab->url || strlen(tab->directory) >= RESTORE_DIRECTORY_LIMIT ||
		    strlen(tab->url) > RESTORE_URL_LIMIT ||
		    packet.pos - position != 8 + strlen(tab->directory) + strlen(tab->url) ||
		    (*tab->directory && *tab->directory != '/') ||
		    (!*tab->directory && !*tab->url)) goto invalid;
#ifndef WORMINAL_PREVIEW_PROTOCOL
		if (*tab->url) goto invalid;
#endif
	}
	if (packet.pos != packet.len) goto invalid;
	return head;
invalid:
	if (fd >= 0) close(fd);
	workspace_restore_free(head);
	wire_buffer_free(&saved); saved = (WireBuffer){0};
	/* Preserve unrecognized/damaged recovery files rather than overwriting them. */
	disabled = 1;
	syslog(LOG_ERR, "Tab recovery unavailable; preserving %s", state_path);
	return NULL;
}

void
workspace_restore_save(Tab *tabs)
{
	WireBuffer buffer = {0};
	uint32_t count = 0;
	char temporary[PATH_MAX];
	if (disabled || (!tabs && !saved.len) || !prepare_path()) return;
	for (Tab *tab = tabs; tab; tab = tab->next) count++;
	if (count > RESTORE_TABS) return;
	wire_put_u32(&buffer, RESTORE_VERSION); wire_put_u32(&buffer, count);
	for (Tab *tab = tabs; tab; tab = tab->next) {
		const char *directory = *tab->directory ? tab->directory : getenv("HOME");
		wire_put_string(&buffer, directory && *directory == '/' ? directory : "/");
		wire_put_string(&buffer, tab->url);
	}
	if (buffer.len > RESTORE_LIMIT || (saved.len == buffer.len && !memcmp(saved.data, buffer.data, buffer.len)))
		goto done;
	if (snprintf(temporary, sizeof(temporary), "%s.XXXXXX", state_path) >= (int)sizeof(temporary)) goto done;
	int fd = mkostemp(temporary, O_CLOEXEC), okay = fd >= 0;
	for (size_t pos = 0; okay && pos < buffer.len;) {
		ssize_t n = write(fd, buffer.data + pos, buffer.len - pos);
		if (n < 0 && errno == EINTR) continue;
		if (n <= 0) { okay = 0; break; }
		pos += n;
	}
	if (okay && fsync(fd) < 0) okay = 0;
	if (fd >= 0 && close(fd) < 0) okay = 0;
	if (okay && rename(temporary, state_path) < 0) okay = 0;
	if (okay) {
		/* Persist the rename as well as the contents across power loss. */
		char *slash = strrchr(temporary, '/'); *slash = 0;
		int parent = open(temporary, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
		if (parent >= 0) { fsync(parent); close(parent); }
		wire_buffer_free(&saved); saved = buffer; return;
	}
	unlink(temporary);
	syslog(LOG_ERR, "Could not save tab recovery: %s", strerror(errno));
done:
	wire_buffer_free(&buffer);
}

int
workspace_restore_start(Tab *(*start)(Client *, Launch *), Tab *(*browser)(const char *))
{
	(void)browser;
	RestoreTab *recipes = workspace_restore_read();
	for (RestoreTab *item = recipes; item; item = item->next) {
#ifdef WORMINAL_PREVIEW_PROTOCOL
		if (*item->url) { browser(item->url); continue; }
#endif
		/* A missing saved directory falls back without dropping the tab. Never
		 * replay a saved command: a recipe always starts the default shell. */
		const char *cwd = item->directory;
		struct stat directory;
		if (stat(cwd, &directory) < 0 || !S_ISDIR(directory.st_mode) || access(cwd, X_OK) < 0)
			cwd = getenv("HOME");
		if (!cwd || stat(cwd, &directory) < 0 || !S_ISDIR(directory.st_mode) || access(cwd, X_OK) < 0)
			cwd = "/";
		Launch launch = {.cols = 80, .rows = 24, .allowalt = 1, .restored = 1,
		    .cwd = (char *)cwd, .title = "", .line = "", .output = "", .windowid = ""};
		start(NULL, &launch);
	}
	int restored = recipes != NULL;
	workspace_restore_free(recipes);
	return restored;
}
