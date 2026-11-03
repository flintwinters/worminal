/* Identical harness for the preserved parser and extracted-engine C adapters. */
#include <assert.h>
#include <locale.h>
#include <sys/resource.h>
#include <time.h>
#ifndef WORMINAL_ORACLE_SOURCE
#define WORMINAL_ORACLE_SOURCE "../src/terminal/st.c"
#endif
#include WORMINAL_ORACLE_SOURCE
#include "session_view.h"

static void hex(const char *text, size_t bytes)
{
	for (size_t i = 0; i < bytes; i++) printf("%02x", (unsigned char)text[i]);
	putchar('\n');
}
void xsetmode(int set, unsigned int flags) { printf("mode %d %u\n", set, flags); }
void xsetpointermotion(int set) { printf("pointer %d\n", set); }
void xsettitle(char *text) { printf("title "); if (text) hex(text, strlen(text)); else puts("null"); }
void xseticontitle(char *text) { printf("icon "); if (text) hex(text, strlen(text)); else puts("null"); }
void xbell(void) { puts("bell"); }
int xsetcursor(int cursor) { printf("cursor %d\n", cursor); return 0; }
void xloadcols(void) { puts("palette reset"); }
int xgetcolor(int index, unsigned char *r, unsigned char *g, unsigned char *b)
{
	*r = 0x12; *g = 0x34; *b = 0x56;
	return index < 0 || index > 259;
}
int xsetcolorname(int index, const char *name)
{
	printf("palette %d %s\n", index, name ? name : "null");
	return index < 0 || index > 259;
}
void xsetsel(char *text) { printf("clipboard "); hex(text, strlen(text)); free(text); }
void xclipcopy(void) { puts("copy"); }
static void reply(const char *text, size_t bytes, int echo)
{
	printf("reply %d ", echo); hex(text, bytes);
}
static void capture(void)
{
	SessionFrame frame = tsessionframe(tsessioncurrent());
	printf("frame %d %d %d %d %d %d\n", frame.cols, frame.rows, frame.cursor_x,
	       frame.cursor_y, frame.scroll, frame.alternate);
	for (int y = 0; y < frame.rows; y++) {
		Line cells = tsessionviewline(tsessioncurrent(), y);
		printf("row %d dirty=%d", y, tsessionrowdirty(tsessioncurrent(), y));
		for (int x = 0; x < frame.cols; x++)
			printf(" %x:%x:%x:%x", cells[x].u, cells[x].mode, cells[x].fg, cells[x].bg);
		putchar('\n');
	}
	tsessioncleandirty(tsessioncurrent());
}

static void benchmark(void)
{
	struct timespec start, end;
	struct rusage usage;
	assert(clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &start) == 0);
	TermSession *owner = tsessionnew(80, 24);
	const char *line = "0123456789 abcdefghijklmnopqrstuvwxyz ABCDEFGHIJKLMNOPQRSTUVWXYZ "
	                   "\342\202\254\344\270\255 \033[31mred\033[0m\r\n";
	for (int i = 0; i < 12000; i++) {
		assert(twrite(line, strlen(line), 0) == (int)strlen(line));
		if (i % 2000 == 1999) tresize(i % 4000 == 1999 ? 100 : 80, 24);
	}
	assert(clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &end) == 0);
	assert(getrusage(RUSAGE_SELF, &usage) == 0);
	uint64_t checksum = 0;
	SessionFrame frame = tsessionframe(owner);
	for (int y = 0; y < frame.rows; y++) {
		Line cells = tsessionviewline(owner, y);
		for (int x = 0; x < frame.cols; x++) checksum = checksum * 31 + cells[x].u;
	}
	long long cpu = (end.tv_sec - start.tv_sec) * 1000000000LL + end.tv_nsec - start.tv_nsec;
	printf("{\"cpu_ns\":%lld,\"rss_kib\":%ld,\"checksum\":%llu}\n", cpu, usage.ru_maxrss,
	       (unsigned long long)checksum);
	tsessionremove(owner);
}

int main(int argc, char **argv)
{
	assert(setlocale(LC_CTYPE, "C.UTF-8"));
	if (argc == 2 && !strcmp(argv[1], "--bench")) { benchmark(); return 0; }
	ttywritehook = reply;
	TermSession *original = tsessionnew(12, 4);
	const char *text = "abc\342\202\254\344\270\255\033[31m RED\033[0m\r\nsecond\r\nthird\r\nfourth\r\nfifth";
	/* Feed byte chunks while retaining incomplete UTF-8 exactly as a PTY adapter does. */
	char pending[16]; int buffered = 0;
	for (size_t i = 0; i < strlen(text); i++) {
		pending[buffered++] = text[i];
		int consumed = twrite(pending, buffered, 0);
		buffered -= consumed; memmove(pending, pending + consumed, buffered);
	}
	assert(!buffered); capture();
	Arg scroll = {.i = 2}; kscrollup(&scroll); capture();
	selstart(0, 0, SNAP_LINE); selextend(3, 1, SEL_REGULAR, 1);
	char *selection = getsel(); printf("selection "); hex(selection, strlen(selection)); free(selection);
	tresize(7, 5); capture();
	tresize(15, 3); capture();
	const char *effects = "\033]2;title\007\033]52;c;Y29weQ==\007\033[5n\033[6n\033[c"
	                      "\033]4;1;#123456\007\033]4;1;?\007\033]10;?\007\007\033[?1003h\033[3 q";
	assert(twrite(effects, strlen(effects), 0) == (int)strlen(effects)); capture();
	const char *alternate = "\033[?1049hALT\033[?1049lEND";
	assert(twrite(alternate, strlen(alternate), 0) == (int)strlen(alternate)); capture();
	const char *malformed = "bad:\300\257\355\240\200\364\220\200\200";
	assert(twrite(malformed, strlen(malformed), 0) == (int)strlen(malformed)); capture();
	tsessionremove(original);
	return 0;
}
