/* C application adapter. Only this layer invokes application callbacks or selects legacy sessions. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "st_state.h"
#include "engine_parser.h"
#include "render_sync.h"

static void
drain(TermSession *chosen)
{
	if (!chosen->engine.events) return;
	WorminalEvent event;
	size_t capacity = 4096;
	uint8_t *data = xmalloc(capacity + 1);
	for (;;) {
		int status = worminal_engine_event(&chosen->engine, &event, data, capacity);
		if (status == WORMINAL_EMPTY) break;
		if (status == WORMINAL_CAPACITY) {
			capacity = event.bytes;
			data = xrealloc(data, capacity + 1);
			continue;
		}
		data[event.bytes] = 0;
		char *text = event.flags ? NULL : (char *)data;
		switch (event.type) {
		case WORMINAL_REPLY: ttywrite((char *)data, event.bytes, event.flags); break;
		case WORMINAL_WRITE: tlegacyraw((char *)data, event.bytes); break;
		case WORMINAL_TITLE: xsettitle(text); break;
		case WORMINAL_ICON_TITLE: xseticontitle(text); break;
		case WORMINAL_MODE: xsetmode(event.argument, event.flags); break;
		case WORMINAL_POINTER_MOTION: xsetpointermotion(event.argument); break;
		case WORMINAL_CURSOR: xsetcursor(event.argument); break;
		case WORMINAL_PALETTE: xsetcolorname(event.argument, text); break;
		case WORMINAL_PALETTE_RESET: xloadcols(); break;
		case WORMINAL_CLIPBOARD:
			if (allowwindowops) { xsetsel(xstrdup((char *)data)); xclipcopy(); }
			break;
		case WORMINAL_BELL: xbell(); break;
		case WORMINAL_DIAGNOSTIC: fwrite(data, 1, event.bytes, stderr); break;
		case WORMINAL_PRINT: tlegacyprint((char *)data, event.bytes); break;
		case WORMINAL_PALETTE_QUERY: {
			unsigned char red, green, blue;
			int number = event.argument;
			int dynamic[] = {defaultfg, defaultbg, defaultcs};
			int index = event.flags ? number : dynamic[number - 10];
			if (xgetcolor(index, &red, &green, &blue)) break;
			char reply[64];
			int bytes = snprintf(reply, sizeof(reply), "\033]%s%d;rgb:%02x%02x/%02x%02x/%02x%02x\007",
				event.flags ? "4;" : "", number, red, red, green, green, blue, blue);
			if (bytes > 0 && bytes < (int)sizeof(reply)) ttywrite(reply, bytes, 1);
			break;
		}
		}
	}
	free(data);
}

int
tlegacywrite(const char *bytes, int length, int echo)
{
	TermSession *chosen = tsessioncurrent();
	int used = 0;
	chosen->engine.now = tsessionnow();
	drain(chosen);
	while (used < length) {
		int consumed = te_twrite(&chosen->engine, bytes + used, length - used, echo);
		drain(chosen);
		if (!consumed) break;
		used += consumed;
	}
	return used;
}

void
tlegacyinput(const char *bytes, size_t length, int echo)
{
	TermSession *chosen = tsessioncurrent();
	if (worminal_engine_input(&chosen->engine, (const uint8_t *)bytes, length, echo,
							 tsessionnow()) != WORMINAL_OK) die("invalid terminal input state\n");
	drain(chosen);
}

void
kscrollup(const Arg *arg)
{
	if (tscrollhook) tscrollhook(1, arg->i);
	else te_kscrollup(&tsessioncurrent()->engine, arg);
}
void
kscrolldown(const Arg *arg)
{
	if (tscrollhook) tscrollhook(0, arg->i);
	else te_kscrolldown(&tsessioncurrent()->engine, arg);
}
void
tresize(int columns, int rows)
{
	TermSession *chosen = tsessioncurrent();
	if (chosen->view_only) tsessionviewresize(chosen, columns, rows);
	else te_tresize(&chosen->engine, columns, rows);
}

void
selinit(void)
{
	te_selinit(&tsessioncurrent()->engine);
	drain(tsessioncurrent());
}

void
selstart(int col, int row, int snap)
{
	te_selstart(&tsessioncurrent()->engine, col, row, snap);
	drain(tsessioncurrent());
}

void
selextend(int col, int row, int type, int done)
{
	te_selextend(&tsessioncurrent()->engine, col, row, type, done);
	drain(tsessioncurrent());
}

int
selected(int x, int y)
{
	return te_selected(&tsessioncurrent()->engine, x, y);
}

char *
getsel(void)
{
	return te_getsel(&tsessioncurrent()->engine);
}

void
selclear(void)
{
	te_selclear(&tsessioncurrent()->engine);
	drain(tsessioncurrent());
}

int
tattrset(int attr)
{
	return te_tattrset(&tsessioncurrent()->engine, attr);
}

void
tsetdirt(int top, int bot)
{
	te_tsetdirt(&tsessioncurrent()->engine, top, bot);
	drain(tsessioncurrent());
}

void
tsetdirtattr(int attr)
{
	te_tsetdirtattr(&tsessioncurrent()->engine, attr);
	drain(tsessioncurrent());
}

void
tnew(int col, int row)
{
	te_tnew(&tsessioncurrent()->engine, col, row);
	drain(tsessioncurrent());
}

Line
tlineat(int y, int scroll)
{
	return te_tlineat(&tsessioncurrent()->engine, y, scroll);
}

int
tisaltscr(void)
{
	return te_tisaltscr(&tsessioncurrent()->engine);
}

void
toggleprinter(const Arg *arg)
{
	te_toggleprinter(&tsessioncurrent()->engine, arg);
	drain(tsessioncurrent());
}

void
printscreen(const Arg *arg)
{
	te_printscreen(&tsessioncurrent()->engine, arg);
	drain(tsessioncurrent());
}

void
printsel(const Arg *arg)
{
	te_printsel(&tsessioncurrent()->engine, arg);
	drain(tsessioncurrent());
}

void
resettitle(void)
{
	te_resettitle(&tsessioncurrent()->engine);
	drain(tsessioncurrent());
}
