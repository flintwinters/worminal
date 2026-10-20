#ifndef WORMINAL_WORKSPACE_HANDOFF_H
#define WORMINAL_WORKSPACE_HANDOFF_H
#include "service_state.h"
int handoff_command(const char *, int);
int handin_listen(const char *);
int handin_accept(int, Tab **, Tab **, uint32_t *);
int handoff_send(const char *, Tab *, void (*)(Tab *));
#endif
