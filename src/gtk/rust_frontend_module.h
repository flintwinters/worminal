#ifndef WORMINAL_RUST_FRONTEND_MODULE_H
#define WORMINAL_RUST_FRONTEND_MODULE_H
#include <stdint.h>
/* Incompatible with the legacy mutable-argv frontend ABI. Arguments are borrowed
 * until return; the listener is transferred on entry, including error returns.
 * -1 means no listener (local help/version). No Rust containers cross this ABI.
 * The launcher keeps code resident for framework callbacks through process exit. */
#define WORMINAL_RUST_FRONTEND_SYMBOL "worminal_rust_frontend_v1"
#define WORMINAL_RUST_FRONTEND_FILE "frontend-rust-v1.so"
typedef int32_t (*WorminalRustFrontendEntry)(int32_t, const char *const *, int32_t);
int32_t worminal_rust_frontend_v1(int32_t, const char *const *, int32_t);
#endif
