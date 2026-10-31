/* Public-boundary invariants; linked without any application, PTY, clock, or GUI adapters. */
#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "engine.h"
#include "engine_codec.h"

static void
feed(WorminalEngine *engine, const char *text, int64_t now)
{
	size_t bytes = strlen(text), consumed;
	while (bytes) {
		assert(worminal_engine_feed(engine, (const uint8_t *)text, bytes, now, &consumed) == WORMINAL_OK);
		assert(consumed && consumed <= bytes);
		text += consumed;
		bytes -= consumed;
	}
}

static void
events_and_isolation(void)
{
	WorminalEngine *first, *second;
	assert(worminal_engine_create(8, 3, &first) == WORMINAL_OK);
	assert(worminal_engine_create(5, 2, &second) == WORMINAL_OK);
	size_t used = 0;
	const uint8_t input[] = "A\033]2;first\007B";
	assert(worminal_engine_feed(first, input, sizeof(input) - 1, 1, &used) == WORMINAL_OK);
	assert(used == sizeof(input) - 2); /* Title is applied before the following B is parsed. */
	size_t ignored;
	assert(worminal_engine_feed(first, (const uint8_t *)"B", 1, 1, &ignored) == WORMINAL_PENDING);
	WorminalTransportState transport = {0};
	assert(worminal_engine_export(first, &transport, NULL, 0, &ignored) == WORMINAL_PENDING);
	WorminalEvent event;
	uint8_t payload[16];
	assert(worminal_engine_event(first, &event, payload, 1) == WORMINAL_CAPACITY);
	assert(event.type == WORMINAL_TITLE && event.bytes == 5);
	assert(worminal_engine_event(first, &event, payload, sizeof(payload)) == WORMINAL_OK);
	assert(!memcmp(payload, "first", 5));
	assert(worminal_engine_event(first, &event, payload, sizeof(payload)) == WORMINAL_EMPTY);
	feed(second, "xyz", 1);
	feed(first, "B", 1);
	WorminalCell cells[8];
	int32_t dirty;
	assert(worminal_engine_row(first, 0, cells, 8, &used, &dirty) == WORMINAL_OK);
	assert(cells[0].rune == 'A' && cells[1].rune == 'B');
	assert(worminal_engine_row(second, 0, cells, 8, &used, &dirty) == WORMINAL_OK);
	assert(cells[0].rune == 'x' && cells[2].rune == 'z');
	assert(worminal_engine_row(first, 0, cells, 1, &used, &dirty) == WORMINAL_CAPACITY && used == 8);
	assert(worminal_engine_row(first, 3, cells, 8, &used, &dirty) == WORMINAL_INVALID);
	worminal_engine_destroy(first);
	worminal_engine_destroy(second);
}

static void
sync_and_utf8(void)
{
	WorminalEngine *engine;
	assert(worminal_engine_create(8, 3, &engine) == WORMINAL_OK);
	size_t used;
	const uint8_t partial[] = {'a', 0xe2, 0x82};
	assert(worminal_engine_feed(engine, partial, sizeof(partial), 10, &used) == WORMINAL_OK && used == 1);
	const uint8_t complete[] = {0xe2, 0x82, 0xac};
	assert(worminal_engine_feed(engine, complete, sizeof(complete), 10, &used) == WORMINAL_OK && used == 3);
	feed(engine, "\033[?2026h", 100);
	WorminalFrame frame;
	assert(worminal_engine_frame(engine, 100, &frame) == WORMINAL_OK);
	assert(frame.redraw_deadline == 150000100);
	assert(worminal_engine_clean(engine) == WORMINAL_PENDING);
	feed(engine, "\033[?2026h", 1000);
	assert(worminal_engine_frame(engine, 1000, &frame) == WORMINAL_OK && frame.redraw_deadline == 150000100);
	assert(worminal_engine_frame(engine, 150000100, &frame) == WORMINAL_OK && !frame.redraw_deadline);
	feed(engine, "\033[?2026h", 150000101);
	assert(worminal_engine_frame(engine, 150000101, &frame) == WORMINAL_OK && !frame.redraw_deadline);
	feed(engine, "\033[?2026l\033[?2026h", 150000102);
	assert(worminal_engine_frame(engine, 150000102, &frame) == WORMINAL_OK && frame.redraw_deadline);
	assert(worminal_engine_frame(engine, 1, &frame) == WORMINAL_INVALID);
	worminal_engine_destroy(engine);
}

static void
input_modes(void)
{
	WorminalEngine *engine;
	assert(worminal_engine_create(8, 3, &engine) == WORMINAL_OK);
	feed(engine, "\033[12l\033[20h", 1);
	assert(worminal_engine_input(engine, (const uint8_t *)"a\r", 2, 1, 2) == WORMINAL_OK);
	WorminalEvent event;
	uint8_t payload[8];
	assert(worminal_engine_event(engine, &event, payload, sizeof(payload)) == WORMINAL_OK);
	assert(event.type == WORMINAL_WRITE && event.bytes == 3 && !memcmp(payload, "a\r\n", 3));
	WorminalCell cells[8];
	size_t count;
	int32_t dirty;
	assert(worminal_engine_row(engine, 0, cells, 8, &count, &dirty) == WORMINAL_OK);
	assert(cells[0].rune == 'a');
	worminal_engine_destroy(engine);
}

static void
malformed_input(void)
{
	WorminalEngine *engine;
	assert(worminal_engine_create(13, 4, &engine) == WORMINAL_OK);
	uint32_t random = 0x5eed;
	uint8_t pending[64], payload[8192];
	size_t buffered = 0;
	for (int chunk = 0; chunk < 64; chunk++) {
		for (int i = 0; i < 31; i++) {
			random = random * 1664525u + 1013904223u;
			pending[buffered++] = random >> 24;
		}
		while (buffered) {
			size_t consumed;
			assert(worminal_engine_feed(engine, pending, buffered, chunk + 1, &consumed) == WORMINAL_OK);
			assert(consumed <= buffered);
			buffered -= consumed;
			memmove(pending, pending + consumed, buffered);
			WorminalEvent event;
			int status;
			while ((status = worminal_engine_event(engine, &event, payload, sizeof(payload))) != WORMINAL_EMPTY)
				assert(status == WORMINAL_OK);
			if (!consumed) break;
		}
		assert(buffered < 4);
		assert(worminal_engine_resize(engine, 7 + chunk % 13, 2 + chunk % 5) == WORMINAL_OK);
	}
	worminal_engine_destroy(engine);
}

static void
snapshot_validation(void)
{
	WorminalEngine *engine, *copy;
	assert(worminal_engine_create(8, 3, &engine) == WORMINAL_OK);
	feed(engine, "one\r\ntwo\r\nthree\r\nfour\033[31", 1);
	WorminalTransportState transport = {.pending_bytes = 2, .output_kind = 1};
	transport.pending[0] = 0xe2; transport.pending[1] = 0x82;
	size_t bytes;
	assert(worminal_engine_export(engine, &transport, NULL, 0, &bytes) == WORMINAL_CAPACITY);
	uint8_t *data = malloc(bytes);
	assert(data && worminal_engine_export(engine, &transport, data, bytes, &bytes) == WORMINAL_OK);
	WorminalTransportState imported;
	assert(worminal_engine_import(data, bytes, &copy, &imported) == WORMINAL_OK);
	assert(!memcmp(&transport, &imported, sizeof(transport)));
	feed(engine, "mX", 2); feed(copy, "mX", 2);
	WorminalCell before[8], after[8];
	int32_t dirty;
	size_t count;
	for (int row = 0; row < 3; row++) {
		assert(worminal_engine_row(engine, row, before, 8, &count, &dirty) == WORMINAL_OK);
		assert(worminal_engine_row(copy, row, after, 8, &count, &dirty) == WORMINAL_OK);
		assert(!memcmp(before, after, sizeof(before)));
	}
	worminal_engine_destroy(copy);
	/* No truncated import can publish a partial instance or overwrite application transport state. */
	const size_t lengths[] = {0, 1, 20, 256, bytes - 1};
	for (size_t i = 0; i < sizeof(lengths) / sizeof(lengths[0]); i++) {
		memset(&imported, 0x5a, sizeof(imported));
		copy = engine;
		assert(worminal_engine_import(data, lengths[i], &copy, &imported) == WORMINAL_INVALID && !copy);
		assert(imported.pending[0] == 0x5a);
	}
	data[0] ^= 1;
	assert(worminal_engine_import(data, bytes, &copy, &imported) == WORMINAL_INVALID && !copy);
	data[0] ^= 1;
	EngineCompatSnapshot *header = (void *)data;
	EngineCompatSnapshot valid = *header;
	header->csi.len = ESC_BUF_SIZ;
	assert(worminal_engine_import(data, bytes, &copy, &imported) == WORMINAL_INVALID && !copy);
	*header = valid;
	header->selection = (Selection){.type = SEL_REGULAR, .ne = {.y = header->row + 1}};
	assert(worminal_engine_import(data, bytes, &copy, &imported) == WORMINAL_INVALID && !copy);
	free(data);
	worminal_engine_destroy(engine);
}

int
main(void)
{
	events_and_isolation();
	sync_and_utf8();
	input_modes();
	malformed_input();
	snapshot_validation();
	return 0;
}
