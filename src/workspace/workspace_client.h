#ifndef WORMINAL_WORKSPACE_CLIENT_H
#define WORMINAL_WORKSPACE_CLIENT_H

#include <sys/types.h>

int workspace_client_open_process(const char *master, pid_t *child);
int workspace_client_open(const char *master);
int workspace_browser_command(const char *url);

#endif
