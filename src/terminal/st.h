/* See LICENSE for license details. */
#ifndef WORMINAL_ST_H
#define WORMINAL_ST_H
#include <stddef.h>
#include "engine.h"

#include <stdint.h>
#include <wchar.h>

/* macros */
#ifndef MIN
#define MIN(a, b)		((a) < (b) ? (a) : (b))
#endif
#ifndef MAX
#define MAX(a, b)		((a) < (b) ? (b) : (a))
#endif
#define LEN(a)			(sizeof(a) / sizeof(a)[0])
#define BETWEEN(x, a, b)	((a) <= (x) && (x) <= (b))
#define DEFAULT(a, b)		(a) = (a) ? (a) : (b)
#define LIMIT(x, a, b)		(x) = (x) < (a) ? (a) : (x) > (b) ? (b) : (x)
#define MODBIT(x, set, bit)	((set) ? ((x) |= (bit)) : ((x) &= ~(bit)))

#define TRUECOLOR(r,g,b)	(WORMINAL_TRUECOLOR_FLAG | (r) << 16 | (g) << 8 | (b))

typedef unsigned char uchar;
typedef unsigned int uint;
typedef unsigned short ushort;

typedef uint_least32_t Rune;

#define Glyph Glyph_
typedef struct {
	Rune u;           /* character code */
	ushort mode;      /* attribute flags */
	uint16_t link;    /* OSC 8 hyperlink id (0 = none); occupies former padding */
	uint32_t fg;      /* foreground  */
	uint32_t bg;      /* background  */
} Glyph;
_Static_assert(sizeof(Glyph) == 16, "link id must stay in the former padding");

typedef Glyph *Line;

typedef union {
	int i;
	uint ui;
	float f;
	const void *v;
	const char *s;
} Arg;

size_t utf8encode(Rune, char *);

/* Terminal settings. */
extern char *vtiden;
extern wchar_t *worddelimiters;
extern unsigned int tabspaces;
extern unsigned int defaultfg;
extern unsigned int defaultbg;
extern unsigned int defaultcs;
#endif
