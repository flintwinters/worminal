#include <assert.h>
#include <stdlib.h>
#include <string.h>
#include "../url.h"

static void
row(Glyph *cells, int width, const char *text, int wraps)
{
	size_t length = strlen(text);
	for (int x = 0; x < width; x++) {
		cells[x].u = x < length ? (unsigned char)text[x] : ' ';
		cells[x].mode = 0;
	}
	if (wraps)
		cells[width - 1].mode = ATTR_WRAP;
}

static void
expect(Line *lines, int rows, int cols, int x, int y, const char *expected)
{
	char *actual = urlat(lines, rows, cols, x, y, NULL);
	assert((actual == NULL) == (expected == NULL));
	if (expected)
		assert(strcmp(actual, expected) == 0);
	free(actual);
}

int
main(void)
{
	Glyph cells[3][32] = {0};
	Line lines[] = {cells[0], cells[1], cells[2]};
	row(lines[0], 32, "Visit (https://example.org/a).", 0);
	expect(lines, 3, 32, 15, 0, "https://example.org/a");
	expect(lines, 3, 32, 29, 0, NULL);
	expect(lines, 3, 32, 0, 0, NULL);
	row(lines[0], 32, "https://site.org/wiki/A_(B)", 0);
	expect(lines, 3, 32, 23, 0, "https://site.org/wiki/A_(B)");
	row(lines[0], 32, "http://", 0);
	expect(lines, 3, 32, 3, 0, NULL);
	row(lines[0], 32, "https://example.org/very/long/pa", 1);
	row(lines[1], 32, "th-continued?q=1&v=2", 0);
	expect(lines, 3, 32, 6, 0, "https://example.org/very/long/path-continued?q=1&v=2");
	expect(lines, 3, 32, 5, 1, "https://example.org/very/long/path-continued?q=1&v=2");
	UrlSpan span;
	char *wrapped = urlat(lines, 3, 32, 5, 1, &span);
	assert(wrapped != NULL);
	assert(span.start_x == 0 && span.start_y == 0);
	assert(span.end_x == 19 && span.end_y == 1);
	free(wrapped);
	row(lines[2], 32, "ftp://example.org", 0);
	expect(lines, 3, 32, 8, 2, NULL);
	return 0;
}
