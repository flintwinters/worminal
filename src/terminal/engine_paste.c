/* Shared terminal paste semantics; the application owns clipboard requests and transport. */
#include "engine.h"

int
worminal_paste_filter(WorminalPasteFilter *state, uint8_t *bytes, size_t length, size_t *written)
{
	if (!state || !written || (!bytes && length) || (state->bracketed != 0 && state->bracketed != 1) ||
	    (state->previous_cr != 0 && state->previous_cr != 1)) return WORMINAL_INVALID;
	size_t output = 0;
	/* Bracketed paste carries text, not Enter keystrokes. ESC can close the envelope
	 * and ETX can abort it in shells; ordinary paste folds CRLF/LF across chunks.
	 * Reference: alacritty/alacritty, alacritty/src/event.rs, ActionContext::paste. */
	for (size_t i = 0; i < length; i++) {
		uint8_t c = bytes[i];
		if (state->bracketed && (c == '\033' || c == '\003')) continue;
		if (!state->bracketed && c == '\n') {
			int paired = state->previous_cr;
			state->previous_cr = 0;
			if (paired) continue;
			c = '\r';
		} else state->previous_cr = c == '\r';
		bytes[output++] = c;
	}
	*written = output;
	return WORMINAL_OK;
}
