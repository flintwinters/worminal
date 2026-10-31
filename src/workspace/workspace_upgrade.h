/* Same-workspace upgrades reuse the transactional PTY transfer protocol. */
#ifndef WORMINAL_WORKSPACE_UPGRADE_H
#define WORMINAL_WORKSPACE_UPGRADE_H
#include "service_state.h"
int workspace_upgrade_watch(void);
int workspace_upgrade_check(int, int, Tab *, void (*)(Tab *), uint32_t);
int workspace_upgrade_receive(Tab **, Tab **, uint32_t *, int);
#endif
