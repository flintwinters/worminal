/* One deadline policy for both sleeping and emitting batched PTY output. */
#ifndef WORMINAL_FRAME_SCHEDULE_H
#define WORMINAL_FRAME_SCHEDULE_H
#include <stdint.h>
#include <time.h>

static inline long
frame_delay_ns(struct timespec last, struct timespec now)
{
	const int64_t interval = 16000000;
	if (!last.tv_sec && !last.tv_nsec) return 0;
	int64_t elapsed = (int64_t)(now.tv_sec - last.tv_sec) * 1000000000 + now.tv_nsec - last.tv_nsec;
	return elapsed >= interval ? 0 : interval - elapsed;
}
#endif
