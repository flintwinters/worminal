/* Deadline boundaries must not add a second interval to a partial output burst. */
#include <assert.h>
#include "../src/workspace/frame_schedule.h"

int
main(void)
{
	struct timespec last = {.tv_sec = 42, .tv_nsec = 990000000};
	assert(frame_delay_ns((struct timespec){0}, last) == 0);
	assert(frame_delay_ns(last, last) == 16000000);
	assert(frame_delay_ns(last, (struct timespec){42, 995000000}) == 11000000);
	assert(frame_delay_ns(last, (struct timespec){43, 0}) == 6000000);
	assert(frame_delay_ns(last, (struct timespec){43, 5999999}) == 1);
	assert(frame_delay_ns(last, (struct timespec){43, 6000000}) == 0);
	assert(frame_delay_ns(last, (struct timespec){43, 7000000}) == 0);
	assert(frame_delay_ns(last, (struct timespec){44, 0}) == 0);
	return 0;
}
