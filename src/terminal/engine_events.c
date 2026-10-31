/* Parser effects are data. Applications drain these in order between feed operations. */
#include <stdlib.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
#include "engine_parser.h"
#include WORMINAL_THEME_HEADER

struct EngineEventNode {
	EngineEventNode *next;
	WorminalEvent event;
	uint8_t *data;
	size_t capacity;
};

void
te_emit(TermEngine *engine, uint32_t type, int32_t argument, uint32_t flags, const void *data, size_t bytes)
{
	EngineEventNode *tail = engine->events_tail;
	if (tail && tail->event.type == type && tail->event.argument == argument &&
		tail->event.flags == flags &&
		(type == WORMINAL_PRINT || type == WORMINAL_DIAGNOSTIC || type == WORMINAL_WRITE)) {
		size_t required = tail->event.bytes + bytes;
		if (required < bytes) te_fail("terminal event size overflow");
		if (required > tail->capacity) {
			tail->capacity = MAX(required, tail->capacity * 2);
			tail->data = te_realloc(tail->data, tail->capacity);
		}
		if (bytes) memcpy(tail->data + tail->event.bytes, data, bytes);
		tail->event.bytes = required;
		return;
	}
	EngineEventNode *node = te_alloc(sizeof(*node));
	node->data = bytes ? te_alloc(bytes) : NULL;
	node->capacity = bytes;
	node->next = NULL;
	node->event = (WorminalEvent){type, argument, flags, bytes};
	if (bytes) memcpy(node->data, data, bytes);
	if (engine->events_tail) engine->events_tail->next = node; else engine->events = node;
	engine->events_tail = node;
}

int
worminal_engine_event(WorminalEngine *engine, WorminalEvent *event, uint8_t *data, size_t capacity)
{
	if (!engine || !event || (!data && capacity)) return WORMINAL_INVALID;
	EngineEventNode *node = engine->events;
	if (!node) return WORMINAL_EMPTY;
	*event = node->event;
	if (capacity < event->bytes) return WORMINAL_CAPACITY;
	if (event->bytes) memcpy(data, node->data, event->bytes);
	engine->events = node->next;
	if (!engine->events) engine->events_tail = NULL;
	free(node->data);
	free(node);
	return WORMINAL_OK;
}

void
te_freeevents(TermEngine *engine)
{
	while (engine->events) {
		EngineEventNode *node = engine->events;
		engine->events = node->next;
		free(node->data);
		free(node);
	}
	engine->events_tail = NULL;
}

static void
string_event(TermEngine *engine, uint32_t type, int32_t argument, const char *text)
{
	te_emit(engine, type, argument, !text, text, text ? strlen(text) : 0);
}

void te_xsettitle(TermEngine *engine, char *title) { string_event(engine, WORMINAL_TITLE, 0, title); }
void te_xseticontitle(TermEngine *engine, char *title) { string_event(engine, WORMINAL_ICON_TITLE, 0, title); }
void te_xsetmode(TermEngine *engine, int set, unsigned int flags)
{
	te_emit(engine, WORMINAL_MODE, set, flags, NULL, 0);
}
void te_xsetpointermotion(TermEngine *engine, int set)
{
	te_emit(engine, WORMINAL_POINTER_MOTION, set, 0, NULL, 0);
}
void te_xbell(TermEngine *engine) { te_emit(engine, WORMINAL_BELL, 0, 0, NULL, 0); }
int te_xsetcursor(TermEngine *engine, int cursor)
{
	if (cursor < 0 || cursor > 7) return 1;
	te_emit(engine, WORMINAL_CURSOR, cursor, 0, NULL, 0);
	return 0;
}
void te_xloadcols(TermEngine *engine) { te_emit(engine, WORMINAL_PALETTE_RESET, 0, 0, NULL, 0); }
int te_xsetcolorname(TermEngine *engine, int index, const char *name)
{
	if (index < 0 || index >= (int)LEN(colorname)) return 1;
	string_event(engine, WORMINAL_PALETTE, index, name);
	return 0;
}
void te_xsetsel(TermEngine *engine, char *selection)
{
	string_event(engine, WORMINAL_CLIPBOARD, 0, selection);
	free(selection);
}
void te_xclipcopy(TermEngine *engine) { (void)engine; }
void te_ttywrite(TermEngine *engine, const char *text, size_t bytes, int echo)
{
	te_emit(engine, WORMINAL_REPLY, 0, echo, text, bytes);
}
void te_tprinter(TermEngine *engine, char *text, size_t bytes)
{
	te_emit(engine, WORMINAL_PRINT, 0, 0, text, bytes);
}

void
te_diagnostic(TermEngine *engine, const char *format, ...)
{
	va_list args, copy;
	va_start(args, format);
	va_copy(copy, args);
	int bytes = vsnprintf(NULL, 0, format, copy);
	va_end(copy);
	if (bytes >= 0) {
		char *text = te_alloc((size_t)bytes + 1);
		vsnprintf(text, (size_t)bytes + 1, format, args);
		te_emit(engine, WORMINAL_DIAGNOSTIC, 0, 0, text, bytes);
		free(text);
	}
	va_end(args);
}
