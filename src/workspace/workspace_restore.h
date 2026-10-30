/* Durable recipes only: never persist commands, environments, or terminal state. */
#ifndef WORMINAL_WORKSPACE_RESTORE_H
#define WORMINAL_WORKSPACE_RESTORE_H
#include "service_state.h"
#include "workspace_launch.h"
int workspace_restore_start(Tab *(*)(Client *, Launch *), Tab *(*)(const char *));
void workspace_restore_save(Tab *);
#endif
