/* Caller-time synchronized redraw gate: no clock access or event-loop policy in the engine. */
#include <limits.h>
#include "engine_state.h"

/* DEC 2026: repeated begins cannot renew a deadline. Expiration blocks begins until end/reset.
 * https://github.com/contour-terminal/vt-extensions/blob/master/synchronized-output.md
 * This transient gate is excluded from native snapshots: imports require a complete resynchronization. */
void
te_setsync(TermEngine *engine, int set)
{
	if (!set) {
		engine->render_sync_deadline = 0;
		engine->render_sync_expired = 0;
	} else if (!engine->render_sync_deadline && !engine->render_sync_expired) {
		engine->render_sync_deadline = engine->now > INT64_MAX - 150000000 ?
		                              INT64_MAX : engine->now + 150000000;
	}
}

int64_t
te_renderdelay(TermEngine *engine, int64_t now)
{
	engine->now = now;
	if (!engine->render_sync_deadline) return 0;
	if (engine->render_sync_deadline > now) return engine->render_sync_deadline - now;
	engine->render_sync_deadline = 0;
	engine->render_sync_expired = 1;
	return 0;
}
