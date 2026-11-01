#ifndef WORMINAL_PASTE_H
#define WORMINAL_PASTE_H

#include <stddef.h>
#include <stdlib.h>
#include "engine.h"

#define PASTE_BEGIN WORMINAL_PASTE_BEGIN
#define PASTE_END WORMINAL_PASTE_END

typedef WorminalPasteFilter PasteFilter;

/* Legacy callers use the same pure terminal rule as Rust. Invalid state is an internal error. */
static inline size_t
paste_filter(PasteFilter *state, char *bytes, size_t length)
{
	size_t output;
	if (worminal_paste_filter(state, (uint8_t *)bytes, length, &output) != WORMINAL_OK) abort();
	return output;
}

#endif
