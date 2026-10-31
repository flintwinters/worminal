/* Native snapshot invariants; included in the sanitizer-backed terminal suite. */
#include <sys/mman.h>
#include "../src/terminal/session_transfer.h"

static void
check_session_transfer(void)
{
	TermSession *previous = tsessioncurrent(), *original = tsessionnew(12, 4), *copy;
	int fd = memfd_create("worminal-terminal-test", MFD_CLOEXEC);
	assert(fd >= 0);
	const char *lines = "first\r\nsecond\r\nthird\r\nfourth\r\nfifth";
	twrite(lines, strlen(lines), 0);
	original->engine.term.line[0][0].mode |= ATTR_WRAP;
	original->engine.term.tabs[3] = 1;
	original->engine.saved[0].x = 3;
	original->engine.saved[1].y = 2;
	original->ttybuf[0] = '\xe2'; original->ttybuf[1] = '\x82'; original->ttybuflen = 2;
	original->engine.term.scr = 1;
	assert(original->engine.term.histlen > 0);
	/* A partial OSC must complete correctly after the transfer. */
	const char *osc = "\033]2;unfinished";
	twrite(osc, strlen(osc), 0);
	assert(tsessionexport(original, fd));
	assert(lseek(fd, 0, SEEK_SET) == 0);
	copy = tsessionimport(fd);
	assert(copy && copy->pty_fd == -1 && copy->child_pid == 0);
	assert(copy->engine.term.histlen == original->engine.term.histlen && copy->engine.term.scr == 1);
	assert(copy->engine.saved[0].x == 3 && copy->engine.saved[1].y == 2);
	assert(copy->ttybuflen == 2 && !memcmp(copy->ttybuf, original->ttybuf, 2));
	assert(copy->engine.strescseq.len == original->engine.strescseq.len);
	assert(!memcmp(copy->engine.strescseq.buf, original->engine.strescseq.buf, original->engine.strescseq.len));
	assert(!memcmp(copy->engine.term.tabs, original->engine.term.tabs, 12 * sizeof(int)));
	for (int i = 0; i < 4; i++) {
		assert(!memcmp(copy->engine.term.line[i], original->engine.term.line[i], 12 * sizeof(Glyph)));
		assert(!memcmp(copy->engine.term.alt[i], original->engine.term.alt[i], 12 * sizeof(Glyph)));
	}
	tsessionuse(copy);
	twrite(" title\007", 7, 0);
	assert(copy->engine.term.esc == 0);
	/* Reject an incomplete payload without registering or signalling a child. */
	assert(ftruncate(fd, 20) == 0 && lseek(fd, 0, SEEK_SET) == 0);
	assert(!tsessionimport(fd));
	/* Round-trip alternate screen and an incomplete CSI without losing main history. */
	assert(ftruncate(fd, 0) == 0 && lseek(fd, 0, SEEK_SET) == 0);
	tsessionuse(copy);
	const char *alternate = "\033[?1049hALT\033[31";
	twrite(alternate, strlen(alternate), 0);
	assert(tsessionexport(copy, fd));
	off_t size = lseek(fd, 0, SEEK_CUR);
	assert(lseek(fd, 0, SEEK_SET) == 0);
	TermSession *altcopy = tsessionimport(fd);
	assert(altcopy && (altcopy->engine.term.mode & MODE_ALTSCREEN));
	assert(altcopy->engine.term.histlen == copy->engine.term.histlen);
	tsessionuse(altcopy);
	twrite("mX", 2, 0);
	assert(altcopy->engine.term.c.attr.fg == 1 && altcopy->engine.term.esc == 0);
	assert(!memcmp(altcopy->engine.term.alt[0], copy->engine.term.alt[0], 12 * sizeof(Glyph)));
	assert(ftruncate(fd, size - 1) == 0 && lseek(fd, 0, SEEK_SET) == 0);
	assert(!tsessionimport(fd));
	tsessionremove(altcopy);
	close(fd);
	tsessionremove(copy); tsessionremove(original); tsessionuse(previous);
}
