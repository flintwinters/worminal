/* Validated launch requests; shared by ordinary and restored shells. */
#ifndef WORMINAL_WORKSPACE_LAUNCH_H
#define WORMINAL_WORKSPACE_LAUNCH_H
#include "wire.h"
typedef struct {
	uint32_t cols, rows, allowalt, argc, envc;
	int restored;
	char *title, *cwd, *line, *output, *windowid;
	char **args, **env;
} Launch;

void launch_free(Launch *);
int launch_read(WirePacket *, Launch *);
void launch_pty(Launch *, const char *);
#endif
