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
#include "session_transfer.h"
#include WORMINAL_THEME_HEADER

typedef struct {
	uint32_t magic, version, bytes, glyph_bytes, history;
	int row, col, histlen, scr;
	TCursor c, saved[2];
	Selection selection;
	CSIEscape csi;
	int ocx, ocy, top, bot, mode, esc;
	char trantbl[4];
	int charset, icharset;
	Rune lastc;
	int altscreen_allowed, ttybuflen, remote_scroll, output_kind;
	char ttybuf[BUFSIZ];
	char str_type;
	uint32_t str_len;
} Snapshot;

static int
transfer(int fd, void *data, size_t size, int writing)
{
	char *p = data;
	while (size) {
		ssize_t n = writing ? write(fd, p, size) : read(fd, p, size);
		if (n < 0 && errno == EINTR) continue;
		if (n <= 0) return 0;
		p += n;
		size -= n;
	}
	return 1;
}

int
tsessionexport(TermSession *s, int fd)
{
	Term *t = &s->term;
	if (s->strescseq.len > 1024 * 1024 ||
	    (size_t)t->col * (t->row * 2 + (size_t)t->histlen) > 16000000) return 0;
	Snapshot h = {
		.magic = 0x57545331, .version = 1, .bytes = sizeof(Snapshot),
		.glyph_bytes = sizeof(Glyph), .history = theme_history_size,
		.row = t->row, .col = t->col, .histlen = t->histlen, .scr = t->scr,
		.c = t->c, .selection = s->sel, .csi = s->csiescseq,
		.ocx = t->ocx, .ocy = t->ocy, .top = t->top, .bot = t->bot,
		.mode = t->mode, .esc = t->esc, .charset = t->charset, .icharset = t->icharset,
		.lastc = t->lastc, .altscreen_allowed = s->altscreen_allowed,
		.ttybuflen = s->ttybuflen, .remote_scroll = s->remote_scroll,
		.output_kind = s->output_fd < 0 ? -1 : s->output_fd == 1,
		.str_type = s->strescseq.type, .str_len = s->strescseq.len,
	};
	memcpy(h.saved, s->saved, sizeof(h.saved));
	memcpy(h.trantbl, t->trantbl, sizeof(h.trantbl));
	memcpy(h.ttybuf, s->ttybuf, sizeof(h.ttybuf));
	if (!transfer(fd, &h, sizeof(h), 1) ||
	    !transfer(fd, t->tabs, t->col * sizeof(int), 1) ||
	    !transfer(fd, s->strescseq.buf, h.str_len, 1)) return 0;
	for (int i = 0; i < t->row * 2 + t->histlen; i++) {
		Line row = i < t->row ? t->line[i] : i < 2 * t->row ? t->alt[i - t->row] :
		    t->hist[(t->histi - t->histlen + i - 2 * t->row + theme_history_size) % theme_history_size];
		if (!transfer(fd, row, t->col * sizeof(Glyph), 1)) return 0;
	}
	return 1;
}

TermSession *
tsessionimport(int fd)
{
	Snapshot h;
	TermSession *s, *previous = tsessioncurrent();
	if (!transfer(fd, &h, sizeof(h), 0) || h.magic != 0x57545331 || h.version != 1 ||
	    h.bytes != sizeof(h) || h.glyph_bytes != sizeof(Glyph) ||
	    h.row < 1 || h.row > 4096 || h.col < 1 || h.col > 4096 ||
	    (size_t)h.col * (h.row * 2 + (size_t)MAX(h.histlen, 0)) > 16000000 ||
	    h.histlen < 0 || h.histlen > theme_history_size || h.histlen > (int)h.history ||
	    h.scr < 0 || h.scr > h.histlen || h.top < 0 || h.bot >= h.row || h.bot < h.top ||
	    h.c.x < 0 || h.c.x >= h.col || h.c.y < 0 || h.c.y >= h.row ||
	    h.charset < 0 || h.charset > 3 || h.icharset < 0 || h.icharset > 3 ||
	    h.ttybuflen < 0 || h.ttybuflen > BUFSIZ || h.str_len > 1024 * 1024 ||
	    (h.output_kind < -1 || h.output_kind > 1) || h.csi.len > ESC_BUF_SIZ || h.csi.narg < 0 || h.csi.narg > ESC_ARG_SIZ)
		return NULL;
	s = tsessionnew(h.col, h.row);
	Term *t = &s->term;
	t->c = h.c; s->sel = h.selection; s->csiescseq = h.csi;
	t->ocx = h.ocx; t->ocy = h.ocy; t->top = h.top; t->bot = h.bot;
	t->mode = h.mode; t->esc = h.esc; t->charset = h.charset; t->icharset = h.icharset;
	t->lastc = h.lastc; s->altscreen_allowed = h.altscreen_allowed;
	s->ttybuflen = h.ttybuflen; s->remote_scroll = h.remote_scroll;
	s->output_fd = h.output_kind ? h.output_kind : -2;
	memcpy(s->saved, h.saved, sizeof(h.saved));
	memcpy(t->trantbl, h.trantbl, sizeof(h.trantbl));
	memcpy(s->ttybuf, h.ttybuf, sizeof(h.ttybuf));
	s->strescseq.type = h.str_type;
	s->strescseq.len = h.str_len;
	s->strescseq.siz = MAX(h.str_len + 1, STR_BUF_SIZ);
	s->strescseq.buf = xrealloc(s->strescseq.buf, s->strescseq.siz);
	if (!transfer(fd, t->tabs, t->col * sizeof(int), 0) ||
	    !transfer(fd, s->strescseq.buf, h.str_len, 0)) goto fail;
	s->strescseq.buf[h.str_len] = '\0';
	if (h.histlen) {
		t->hist = calloc(theme_history_size, sizeof(Line));
		if (!t->hist) goto fail;
	}
	for (int i = 0; i < t->row * 2 + h.histlen; i++) {
		Line row;
		if (i < t->row) row = t->line[i];
		else if (i < t->row * 2) row = t->alt[i - t->row];
		else row = t->hist[i - 2 * t->row] = xmalloc(t->col * sizeof(Glyph));
		if (!transfer(fd, row, t->col * sizeof(Glyph), 0)) goto fail;
	}
	t->histlen = h.histlen; t->histi = theme_history_size ? h.histlen % theme_history_size : 0;
	t->scr = h.scr;
	for (int i = 0; i < t->row; i++) t->dirty[i] = 1;
	tsessionuse(previous);
	return s;
fail:
	tsessionremove(s);
	tsessionuse(previous);
	return NULL;
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
