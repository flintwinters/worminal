#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "url.h"

#define URL_LIMIT 8192

static int
urlchar(Rune rune)
{
	return rune > 32 && rune < 127 && rune != '<' && rune != '>' &&
	       rune != '"' && rune != '\'' && rune != '`';
}

static int
urlprefix(const char *text, size_t length)
{
	return (length > 7 && !memcmp(text, "http://", 7) && urlchar(text[7])) ||
	       (length > 8 && !memcmp(text, "https://", 8) && urlchar(text[8]));
}

static int
unmatched_closer(const char *text, size_t start, size_t end, char open, char close)
{
	int balance = 0;
	for (size_t i = start; i < end; i++) {
		balance += text[i] == open;
		balance -= text[i] == close;
	}
	return balance < 0;
}

char *
urlat(Line *lines, int rows, int cols, int x, int y, UrlSpan *span)
{
	char text[URL_LIMIT];
	int positions[URL_LIMIT];
	size_t length = 0, clicked = URL_LIMIT, start, end;
	int first, last;

	if (!lines || rows < 1 || cols < 1 || x < 0 || x >= cols || y < 0 || y >= rows)
		return NULL;
	first = y;
	while (first > 0 && lines[first - 1][cols - 1].mode & ATTR_WRAP)
		first--;
	last = y;
	while (last + 1 < rows && lines[last][cols - 1].mode & ATTR_WRAP)
		last++;
	for (int row = first; row <= last; row++) {
		int width = cols;
		if (!(lines[row][cols - 1].mode & ATTR_WRAP))
			while (width > 0 && lines[row][width - 1].u == ' ')
				width--;
		for (int col = 0; col < width; col++) {
			Glyph cell = lines[row][col];
			if (row == y && col == x)
				clicked = length;
			if (cell.mode & ATTR_WDUMMY)
				continue;
			if (length == URL_LIMIT - 1)
				return NULL;
			positions[length] = row * cols + col;
			text[length++] = urlchar(cell.u) ? cell.u : ' ';
		}
		if (row == y && x >= width)
			return NULL;
	}
	if (clicked >= length || text[clicked] == ' ')
		return NULL;
	start = clicked;
	while (start > 0 && text[start - 1] != ' ')
		start--;
	end = clicked + 1;
	while (end < length && text[end] != ' ')
		end++;
	/* A URL may follow punctuation such as an opening parenthesis. */
	while (start < end && !urlprefix(text + start, end - start))
		start++;
	if (start == end || clicked < start)
		return NULL;
	while (end > start) {
		char tail = text[end - 1];
		if (strchr(".,;:!?", tail) ||
		    (tail == ')' && unmatched_closer(text, start, end, '(', ')')) ||
		    (tail == ']' && unmatched_closer(text, start, end, '[', ']')) ||
		    (tail == '}' && unmatched_closer(text, start, end, '{', '}')))
			end--;
		else
			break;
	}
	if (clicked >= end || end - start > URL_LIMIT - 1)
		return NULL;
	char *result = malloc(end - start + 1);
	if (!result)
		return NULL;
	memcpy(result, text + start, end - start);
	result[end - start] = 0;
	if (span) {
		span->start_x = positions[start] % cols;
		span->start_y = positions[start] / cols;
		span->end_x = positions[end - 1] % cols;
		span->end_y = positions[end - 1] / cols;
	}
	return result;
}

void
urlopen(const char *url)
{
	pid_t child = fork();
	if (child != 0)
		return;
	/* The GUI process ignores SIGCHLD; the detached helper must not keep its
	 * X11 or workspace descriptors open while a browser is running. */
	setsid();
	signal(SIGCHLD, SIG_DFL);
	for (int fd = 3; fd < 1024; fd++)
		close(fd);
	execlp("xdg-open", "xdg-open", url, (char *)NULL);
	_exit(127);
}
