/* See LICENSE for license details. */
#ifndef WORMINAL_ST_H
#define WORMINAL_ST_H
#include <stddef.h>
#include "engine.h"

#include <stdint.h>
#include <sys/select.h>
#include <sys/types.h>

/* macros */
#ifndef MIN
#define MIN(a, b)		((a) < (b) ? (a) : (b))
#endif
#ifndef MAX
#define MAX(a, b)		((a) < (b) ? (b) : (a))
#endif
#define LEN(a)			(sizeof(a) / sizeof(a)[0])
#define BETWEEN(x, a, b)	((a) <= (x) && (x) <= (b))
#define DIVCEIL(n, d)		(((n) + ((d) - 1)) / (d))
#define DEFAULT(a, b)		(a) = (a) ? (a) : (b)
#define LIMIT(x, a, b)		(x) = (x) < (a) ? (a) : (x) > (b) ? (b) : (x)
#define ATTRCMP(a, b)		((a).mode != (b).mode || (a).fg != (b).fg || \
				(a).bg != (b).bg)
#define TIMEDIFF(t1, t2)	((t1.tv_sec-t2.tv_sec)*1000 + \
				(t1.tv_nsec-t2.tv_nsec)/1E6)
#define MODBIT(x, set, bit)	((set) ? ((x) |= (bit)) : ((x) &= ~(bit)))

#ifndef WORMINAL_THEME_HEADER
#define WORMINAL_THEME_HEADER "build/theme.h"
#endif

#define TRUECOLOR(r,g,b)	(WORMINAL_TRUECOLOR_FLAG | (r) << 16 | (g) << 8 | (b))
#define IS_TRUECOL(x)		(WORMINAL_TRUECOLOR_FLAG & (x))

typedef unsigned char uchar;
typedef unsigned int uint;
typedef unsigned long ulong;
typedef unsigned short ushort;

typedef uint_least32_t Rune;

#define Glyph Glyph_
typedef struct {
	Rune u;           /* character code */
	ushort mode;      /* attribute flags */
	uint32_t fg;      /* foreground  */
	uint32_t bg;      /* background  */
} Glyph;

typedef Glyph *Line;

typedef union {
	int i;
	uint ui;
	float f;
	const void *v;
	const char *s;
} Arg;

typedef struct TermSession TermSession;
TermSession *tsessioncurrent(void);
TermSession *tsessionnew(int, int);
TermSession *tsessionnext(TermSession *);
void tsessionuse(TermSession *);
int tsessionfd(TermSession *);
pid_t tsessionpid(TermSession *);
void tsessionreap(void);
int tsessionexitstatus(void);
void tsessionhangupall(void);
void tsessionstop(TermSession *);
void tsessionremove(TermSession *);
void tsessionallowalt(TermSession *, int);

void die(const char *, ...) __attribute__((noreturn));
void redraw(void);
void draw(void);
void kscrollup(const Arg *);
void kscrolldown(const Arg *);
int tisaltscr(void);

void tlegacyprint(char *, size_t);
void tlegacyraw(const char *, size_t);
void tlegacyinput(const char *, size_t, int);
void printscreen(const Arg *);
void printsel(const Arg *);
void sendbreak(const Arg *);
void toggleprinter(const Arg *);

int tattrset(int);
void tnew(int, int);
void tresize(int, int);
void tsetdirt(int, int);
void tsetdirtattr(int);
void ttyhangup(void);
void ttysetlaunch(const char *, char **, const char *);
int ttynew(const char *, char *, const char *, char **);
size_t ttyread(void);
void ttyresize(int, int);
void ttywrite(const char *, size_t, int);
extern void (*ttyrawhook)(const char *, size_t);
extern void (*ttywritehook)(const char *, size_t, int);
extern void (*tscrollhook)(int, int);
extern void (*tprinterhook)(const char *, size_t);

void resettitle(void);

void selclear(void);
void selinit(void);
void selstart(int, int, int);
void selextend(int, int, int, int);
int selected(int, int);
char *getsel(void);

size_t utf8encode(Rune, char *);

void *xmalloc(size_t);
void *xrealloc(void *, size_t);
char *xstrdup(const char *);

/* config.h globals */
extern char *utmp;
extern char *scroll;
extern char *stty_args;
extern char *vtiden;
extern wchar_t *worddelimiters;
extern int allowaltscreen;
extern int allowwindowops;
extern char *termname;
extern unsigned int tabspaces;
extern unsigned int defaultfg;
extern unsigned int defaultbg;
extern unsigned int defaultcs;
#endif
