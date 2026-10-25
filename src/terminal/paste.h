#ifndef WORMINAL_PASTE_H
#define WORMINAL_PASTE_H

#include <stddef.h>

#define PASTE_BEGIN "\033[200~"
#define PASTE_END "\033[201~"

typedef struct {
	int bracketed, previous_cr;
} PasteFilter;

/* Bracketed paste carries text, not Enter keystrokes: preserve its line endings.
 * Match Alacritty's paste handling: ESC can close the envelope and ETX can abort
 * it in shells. Ordinary paste folds CRLF/LF into one Enter, even across chunks.
 * Reference: alacritty/alacritty, alacritty/src/event.rs, ActionContext::paste. */
static inline size_t
paste_filter(PasteFilter *state, char *bytes, size_t length)
{
	size_t output = 0;
	for (size_t i = 0; i < length; i++) {
		char c = bytes[i];
		if (state->bracketed && (c == '\033' || c == '\003')) continue;
		if (!state->bracketed && c == '\n') {
			int paired = state->previous_cr;
			state->previous_cr = 0;
			if (paired) continue;
			c = '\r';
		} else state->previous_cr = c == '\r';
		bytes[output++] = c;
	}
	return output;
}

#endif
