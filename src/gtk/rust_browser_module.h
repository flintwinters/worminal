#ifndef WORMINAL_RUST_BROWSER_MODULE_H
#define WORMINAL_RUST_BROWSER_MODULE_H
#include <stdint.h>

/* Incompatible with the historical Page/App browser ABI. All operations run on
 * the GTK owner thread. Strings are borrowed only for the duration of a call.
 * Widget/profile pointers are framework GObjects, never Rust allocations.
 * A view owns its callback until destroy returns; no callback follows destroy.
 * Code stays resident for WebKit's asynchronous callbacks after retirement. */
#define WORMINAL_RUST_BROWSER_ABI 2
#define WORMINAL_RUST_BROWSER_FILE "browser-rust-v2.so"
enum { WORMINAL_BROWSER_TITLE = 1, WORMINAL_BROWSER_NAVIGATE,
       WORMINAL_BROWSER_OPEN, WORMINAL_BROWSER_CLOSE };
typedef void (*WorminalBrowserEvent)(void *, uint32_t, const char *);
typedef struct {
    void *(*profile)(const char *, const char *); /* returns a full GObject reference */
    void *(*create)(void *, const char *, WorminalBrowserEvent, void *);
    void *(*widget)(void *); /* borrowed GtkWidget reference */
    void (*focus)(void *);
    void (*navigate)(void *, const char *);
    int32_t (*shortcut)(void *, uint32_t, uint32_t);
    void (*zoom)(void *, double);
    void (*background)(void *, double, double, double, double);
    void (*destroy)(void *);
    void (*retire)(uint32_t); /* zero reactivates a restored compatible generation */
} WorminalRustBrowser;
const WorminalRustBrowser *worminal_rust_browser_module(uint32_t);
/* Additive private update hook: null input exports cookies; nonnull input imports them.
 * Reply data is borrowed only during the callback. Zero return means unsupported/error before callback. */
typedef void (*WorminalBrowserProfileReply)(void *, int32_t, const uint8_t *, uint64_t);
typedef int32_t (*WorminalBrowserProfileTransfer)(void *, const uint8_t *, uint64_t,
                                                WorminalBrowserProfileReply, void *);
int32_t worminal_browser_profile_transfer_v1(void *, const uint8_t *, uint64_t,
                                            WorminalBrowserProfileReply, void *);

#endif
