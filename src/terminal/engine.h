/* Terminal engine C API. The caller owns buffers; no file descriptors, processes or GUI objects cross it. */
#ifndef WORMINAL_ENGINE_H
#define WORMINAL_ENGINE_H
#include <stddef.h>
#include <stdint.h>

enum glyph_attribute {
	ATTR_NULL       = 0,
	ATTR_BOLD       = 1 << 0,
	ATTR_FAINT      = 1 << 1,
	ATTR_ITALIC     = 1 << 2,
	ATTR_UNDERLINE  = 1 << 3,
	ATTR_BLINK      = 1 << 4,
	ATTR_REVERSE    = 1 << 5,
	ATTR_INVISIBLE  = 1 << 6,
	ATTR_STRUCK     = 1 << 7,
	ATTR_WRAP       = 1 << 8,
	ATTR_WIDE       = 1 << 9,
	ATTR_WDUMMY     = 1 << 10,
	ATTR_BOLD_FAINT = ATTR_BOLD | ATTR_FAINT,
};

enum selection_mode {
	SEL_IDLE = 0,
	SEL_EMPTY = 1,
	SEL_READY = 2
};

enum selection_type {
	SEL_REGULAR = 1,
	SEL_RECTANGULAR = 2
};

enum selection_snap {
	SNAP_WORD = 1,
	SNAP_LINE = 2
};

enum win_mode {
	MODE_VISIBLE     = 1 << 0,
	MODE_FOCUSED     = 1 << 1,
	MODE_APPKEYPAD   = 1 << 2,
	MODE_MOUSEBTN    = 1 << 3,
	MODE_MOUSEMOTION = 1 << 4,
	MODE_REVERSE     = 1 << 5,
	MODE_KBDLOCK     = 1 << 6,
	MODE_HIDE        = 1 << 7,
	MODE_APPCURSOR   = 1 << 8,
	MODE_MOUSESGR    = 1 << 9,
	MODE_8BIT        = 1 << 10,
	MODE_BLINK       = 1 << 11,
	MODE_FBLINK      = 1 << 12,
	MODE_FOCUS       = 1 << 13,
	MODE_MOUSEX10    = 1 << 14,
	MODE_MOUSEMANY   = 1 << 15,
	MODE_BRCKTPASTE  = 1 << 16,
	MODE_NUMLOCK     = 1 << 17,
	MODE_ALTSCROLL   = 1 << 18,
	MODE_MOUSE       = MODE_MOUSEBTN|MODE_MOUSEMOTION|MODE_MOUSEX10\
	                  |MODE_MOUSEMANY,
};

typedef struct WorminalEngine WorminalEngine;
typedef struct WorminalSelection WorminalSelection;
enum worminal_default_color {
	WORMINAL_DEFAULT_FOREGROUND = 258,
	WORMINAL_DEFAULT_BACKGROUND = 259,
	WORMINAL_DEFAULT_CURSOR = 256
};
#define WORMINAL_PENDING_BYTES 8192
#define WORMINAL_TRUECOLOR_FLAG (1u << 24)
/* Compatibility fields are supplied by the application, never owned by the terminal engine. */
typedef struct {
	int32_t pending_bytes, remote_scroll, output_kind;
	uint8_t pending[WORMINAL_PENDING_BYTES];
} WorminalTransportState;
typedef struct {
	uint32_t rune, attributes, foreground, background;
} WorminalCell;
typedef struct {
	int32_t columns, rows, cursor_x, cursor_y, scroll, alternate;
	int64_t redraw_deadline;
} WorminalFrame;
typedef struct { int32_t first, end; } WorminalSelectionRange;
typedef struct {
	uint32_t type;
	int32_t argument;
	uint32_t flags;
	size_t bytes;
} WorminalEvent;
enum worminal_status {
	WORMINAL_OK = 0,
	WORMINAL_INVALID = 1,
	WORMINAL_CAPACITY = 2,
	WORMINAL_EMPTY = 3,
	WORMINAL_PENDING = 4
};
enum worminal_event_type {
	WORMINAL_REPLY = 1,
	WORMINAL_TITLE,
	WORMINAL_ICON_TITLE,
	WORMINAL_MODE,
	WORMINAL_POINTER_MOTION,
	WORMINAL_CURSOR,
	WORMINAL_PALETTE,
	WORMINAL_PALETTE_RESET,
	WORMINAL_PALETTE_QUERY,
	WORMINAL_CLIPBOARD,
	WORMINAL_BELL,
	WORMINAL_PRINT,
	WORMINAL_DIAGNOSTIC,
	WORMINAL_WRITE,
	WORMINAL_NOTIFICATION
};
enum worminal_control { WORMINAL_TOGGLE_PRINTER = 0, WORMINAL_PRINT_SCREEN = 1 };

int worminal_engine_create(int32_t, int32_t, WorminalEngine **);
void worminal_engine_destroy(WorminalEngine *);
/* Stop at effects so callers apply them before parsing later bytes. An incomplete UTF-8 suffix
 * remains with the caller, including during legacy handoff. Drain queued events before feeding. */
int worminal_engine_feed(WorminalEngine *, const uint8_t *, size_t, int64_t, size_t *);
/* Encode input using terminal echo/newline modes; WRITE events contain the resulting PTY bytes. */
int worminal_engine_input(WorminalEngine *, const uint8_t *, size_t, int32_t, int64_t);
int worminal_engine_resize(WorminalEngine *, int32_t, int32_t);
int worminal_engine_scroll(WorminalEngine *, int32_t, int32_t);
int worminal_engine_select(WorminalEngine *, int32_t, int32_t, int32_t, int32_t, int32_t, int32_t);
int worminal_engine_clear_selection(WorminalEngine *);
int worminal_engine_selected(WorminalEngine *, int32_t, int32_t, int32_t *);
int worminal_engine_allow_alternate(WorminalEngine *, int32_t);
int worminal_engine_control(WorminalEngine *, uint32_t);
int worminal_engine_selection(WorminalEngine *, uint8_t *, size_t, size_t *);
/* Snapshot retained history and the active screen, preserving selection and scroll position. */
int worminal_engine_all_text(WorminalEngine *, uint8_t *, size_t, size_t *);
int worminal_engine_frame(WorminalEngine *, int64_t, WorminalFrame *);
int worminal_engine_row(WorminalEngine *, int32_t, WorminalCell *, size_t, size_t *, int32_t *);
/* Per-cell OSC 8 link ids of a visible row (0 = none); same status codes as worminal_engine_row. Ids are
 * meaningful only inside this engine and until its next feed. */
int worminal_engine_row_links(WorminalEngine *, int32_t, uint16_t *, size_t, size_t *);
/* URI for a nonzero link id, or NULL. Owned by the engine; valid until the next feed. */
const char *worminal_engine_link_uri(WorminalEngine *, uint16_t);
int worminal_engine_clean(WorminalEngine *);
/* CAPACITY reports the required payload size and leaves the event queued. */
int worminal_engine_event(WorminalEngine *, WorminalEvent *, uint8_t *, size_t);
int worminal_engine_export(WorminalEngine *, const WorminalTransportState *, uint8_t *, size_t, size_t *);
int worminal_engine_import(const uint8_t *, size_t, WorminalEngine **, WorminalTransportState *);
int worminal_engine_import_prefix(const uint8_t *, size_t, WorminalEngine **, WorminalTransportState *, size_t *);
/* Imported views own cells in the caller. Selection borrows them only during each call;
 * it cannot parse bytes, allocate a terminal grid, reflow, or retain a cell pointer. */
int worminal_selection_create(WorminalSelection **);
void worminal_selection_destroy(WorminalSelection *);
int worminal_selection_configure(WorminalSelection *, const WorminalFrame *);
int worminal_selection_clear(WorminalSelection *);
int worminal_selection_select(WorminalSelection *, const WorminalCell *, size_t,
                             int32_t, int32_t, int32_t, int32_t, int32_t, int32_t);
int worminal_selection_text(WorminalSelection *, const WorminalCell *, size_t, uint8_t *, size_t, size_t *);
/* One half-open column interval per viewport row, empty when first == end. */
int worminal_selection_ranges(WorminalSelection *, WorminalSelectionRange *, size_t, size_t *);
#endif
