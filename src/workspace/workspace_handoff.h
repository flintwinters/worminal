#ifndef WORMINAL_WORKSPACE_HANDOFF_H
#define WORMINAL_WORKSPACE_HANDOFF_H
#include "service_state.h"
#include "handoff_format.h"
int handoff_command(const char *, int);
int handin_listen(const char *);
int handin_accept(int, Tab **, Tab **, uint32_t *);
int handoff_send(const char *, Tab *, void (*)(Tab *));
int handoff_upgrade_send(const char *, Tab *, void (*)(Tab *), uint32_t);
int handin_upgrade_accept(int, Tab **, Tab **, uint32_t *);
int handin_mapped_accept(int, Tab **, Tab **, uint32_t *, const uint32_t *, uint32_t, uint32_t, pid_t);
#endif
