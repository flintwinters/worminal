/* Private terminal state shared by the parser and the session transport. */
#ifndef WORMINAL_ST_STATE_H
#define WORMINAL_ST_STATE_H

#include <stdio.h>
#include "st.h"
#include "engine_state.h"

struct TermSession {
	TermEngine engine;
	int output_fd, pty_fd;
	pid_t child_pid;
	int child_handle; /* Imported pidfd + 1; zero for a child, -1 after exit. */
	int view_only;
	char ttybuf[BUFSIZ];
	int ttybuflen;
	int remote_scroll;
	struct TermSession *next;
};

TermSession *tsessionalloc(void);
void tsessionfreegrid(TermSession *);
void tsessionviewresize(TermSession *, int, int);
Line tlineat(int, int);

#endif
