/* Local, versioned snapshots; descriptors travel separately through SCM_RIGHTS. */
#ifndef WORMINAL_SESSION_TRANSFER_H
#define WORMINAL_SESSION_TRANSFER_H
#include "st.h"
int tsessionexport(TermSession *, int);
TermSession *tsessionimport(int);
int tsessionprivatefds(TermSession *);
int tsessionprepare(TermSession *, pid_t);
pid_t tsessionlivepid(TermSession *);
void tsessionsignal(TermSession *, int);
void tsessionstopchild(TermSession *);
void tsessionadopt(TermSession *, int, int, pid_t);
void tsessiondisown(TermSession *);
#endif
