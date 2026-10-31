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

void
tsessionsetsync(TermSession *session, int set)
{
	session->engine.now = sync_now();
	te_setsync(&session->engine, set);
}

long
tsessionrenderdelay(TermSession *session)
{
	return te_renderdelay(&session->engine, sync_now());
}

int64_t
tsessionnow(void)
{
	return sync_now();
}
