#ifndef WORMINAL_WORKSPACE_SOCKET_H
#define WORMINAL_WORKSPACE_SOCKET_H

#include <stddef.h>

int workspace_daemon_path(char *, size_t);
int workspace_connect(void);
int workspace_ensure(void);
int workspace_listen(void);

#endif
