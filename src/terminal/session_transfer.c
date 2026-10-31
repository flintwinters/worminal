#define _GNU_SOURCE
/* The snapshot is a local native ABI, shared by the GTK and X11 daemons.
 * Never serialize pointers. Reject incompatible layouts before allocating state. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <sys/syscall.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "st_state.h"
#include "engine_codec.h"
#include "session_transfer.h"
#include WORMINAL_THEME_HEADER

static int
transfer(void *context, void *data, size_t size, int writing)
{
	int fd = *(int *)context;
	char *cursor = data;
	while (size) {
		ssize_t bytes = writing ? write(fd, cursor, size) : read(fd, cursor, size);
		if (bytes < 0 && errno == EINTR) continue;
		if (bytes <= 0) return 0;
		cursor += bytes;
		size -= bytes;
	}
	return 1;
}

int
tsessionexport(TermSession *session, int fd)
{
	if (session->view_only) return 0;
	WorminalTransportState transport = {
		.pending_bytes = session->ttybuflen, .remote_scroll = session->remote_scroll,
		.output_kind = session->output_fd < 0 ? -1 : session->output_fd == 1,
	};
	memcpy(transport.pending, session->ttybuf, sizeof(transport.pending));
	EngineSnapshotIO io = {transfer, &fd};
	return te_export_compat(&session->engine, &transport, &io);
}

TermSession *
tsessionimport(int fd)
{
	EngineSnapshotIO io = {transfer, &fd};
	WorminalTransportState transport = {0};
	TermEngine *engine = te_import_compat(&io, &transport);
	if (!engine) return NULL;
	/* Register only a fully validated engine; failed imports cannot publish half a session. */
	TermSession *previous = tsessioncurrent(), *session = tsessionalloc();
	session->engine = *engine;
	free(engine);
	session->ttybuflen = transport.pending_bytes;
	session->remote_scroll = transport.remote_scroll;
	session->output_fd = transport.output_kind ? transport.output_kind : -2;
	memcpy(session->ttybuf, transport.pending, sizeof(session->ttybuf));
	tsessionuse(previous);
	return session;
}

void
tsessionadopt(TermSession *s, int pty, int output, pid_t child)
{
	s->pty_fd = pty;
	if (s->output_fd == -2) s->output_fd = output; else close(output);
	s->child_pid = child;
}

void
tsessiondisown(TermSession *s)
{
	/* Closing the duplicated master is safe only while the receiver holds it. */
	s->child_pid = 0;
}

/* A migrated shell is not our child. Pin its identity so a later close cannot
 * signal an unrelated process after PID reuse (even if descendants keep the PTY). */
int
tsessionprepare(TermSession *s, pid_t child)
{
	if (child <= 0) { s->child_handle = -1; return 1; }
	int fd = syscall(SYS_pidfd_open, child, 0);
	if (fd < 0 && errno != ESRCH) return 0;
	s->child_handle = fd < 0 ? -1 : fd + 1;
	return 1;
}

pid_t
tsessionlivepid(TermSession *s)
{
	if (s->child_handle < 0) return 0;
	if (s->child_handle > 0) {
		struct pollfd p = {.fd = s->child_handle - 1, .events = POLLIN};
		if (poll(&p, 1, 0) > 0) return 0;
	}
	return s->child_pid;
}

void
tsessionsignal(TermSession *s, int signal)
{
	if (s->child_pid <= 0) return;
	if (s->child_handle > 0)
		syscall(SYS_pidfd_send_signal, s->child_handle - 1, signal, NULL, 0);
	else if (!s->child_handle)
		kill(s->child_pid, signal);
}

void
tsessionstopchild(TermSession *s)
{
	tsessionsignal(s, SIGHUP);
	if (s->child_handle > 0) {
		close(s->child_handle - 1);
		s->child_handle = -1;
	}
}

int
tsessionprivatefds(TermSession *s)
{
	if (s->pty_fd >= 0 && fcntl(s->pty_fd, F_SETFD, FD_CLOEXEC) < 0) return 0;
	return s->output_fd <= 1 || fcntl(s->output_fd, F_SETFD, FD_CLOEXEC) == 0;
}
