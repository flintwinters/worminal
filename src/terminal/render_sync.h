/* A bounded rendering gate; parsing and PTY input continue while it is held. */
#ifndef WORMINAL_RENDER_SYNC_H
#define WORMINAL_RENDER_SYNC_H
#include "st.h"
void tsessionsetsync(TermSession *, int);
long tsessionrenderdelay(TermSession *);
#endif
