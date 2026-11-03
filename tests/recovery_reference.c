/* Read and write native C recovery recipes without launching shells or altering a real workspace. */
#include <stdio.h>
#include <string.h>
#include "workspace_restore.h"

static void
text(const char *value)
{
	for (const unsigned char *p = (const unsigned char *)value; *p; p++) printf("%02x", *p);
	putchar('\n');
}

static Tab *
terminal(Client *client, Launch *launch)
{
	(void)client;
	if (!launch->restored || launch->argc) return NULL;
	fputs("T ", stdout); text(launch->cwd);
	return NULL;
}

static Tab *
browser(const char *url)
{
	fputs("B ", stdout); text(url);
	return NULL;
}

int
main(int argc, char **argv)
{
	if (argc == 2 && !strcmp(argv[1], "--read")) {
		workspace_restore_start(terminal, browser);
		return ferror(stdout) != 0;
	}
	if (argc == 4 && !strcmp(argv[1], "--write")) {
		Tab tab = {.directory = argv[2], .url = *argv[3] ? argv[3] : NULL};
		workspace_restore_save(&tab);
		return 0;
	}
	return 2;
}
