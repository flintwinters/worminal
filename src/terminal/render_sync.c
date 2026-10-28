#define _GNU_SOURCE
#include <time.h>
#include "st_state.h"
#include "render_sync.h"

static int64_t
sync_now(void)
{
	struct timespec now;
	clock_gettime(CLOCK_MONOTONIC, &now);
	return (int64_t)now.tv_sec * 1000000000 + now.tv_nsec;
}

/* DEC mode 2026 delimits an application's complete redraw, not a PTY read:
 * https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md
 * Repeated begins cannot renew the deadline. After timeout ignore begins until
 * an end/reset, so a broken application cannot indefinitely suppress painting.
 * This transient render gate is deliberately not part of native state snapshots;
 * a new owner starts with a complete resynchronization of the imported cells. */
void
tsessionsetsync(TermSession *session, int set)
{
	if (!set) {
		session->render_sync_deadline = 0;
		session->render_sync_expired = 0;
	} else if (!session->render_sync_deadline && !session->render_sync_expired) {
		session->render_sync_deadline = sync_now() + 150000000;
	}
}

long
tsessionrenderdelay(TermSession *session)
{
	if (!session->render_sync_deadline) return 0;
	int64_t remaining = session->render_sync_deadline - sync_now();
	if (remaining > 0) return remaining;
	session->render_sync_deadline = 0;
	session->render_sync_expired = 1;
	return 0;
}
