#ifndef WORMINAL_RUST_FRONTEND_MODULE_H
#define WORMINAL_RUST_FRONTEND_MODULE_H
#include <stdint.h>
/* Incompatible with the legacy mutable-argv frontend ABI. Arguments are borrowed
 * until return; the listener is transferred on entry, including error returns.
 * -1 means no listener (local help/version). No Rust containers cross this ABI.
 * Entry runs on the main thread. Flags select the owner's profile once: zero
 * retains the stable product profile; CANDIDATE isolates development storage.
 * The launcher keeps code resident for framework callbacks through process exit. */
#define WORMINAL_RUST_FRONTEND_SYMBOL "worminal_rust_frontend_v2"
#define WORMINAL_RUST_FRONTEND_FILE "frontend-rust-v2.so"
#define WORMINAL_RUST_FRONTEND_CANDIDATE 1
/* Optional additive runtime identity hook. Paths and descriptors are borrowed during the call;
 * the frontend duplicates descriptors to pin the exact loaded inodes for update detection. */
#define WORMINAL_RUST_FRONTEND_RUNTIME_SYMBOL "worminal_frontend_runtime_v1"
typedef int32_t (*WorminalRustFrontendRuntimeEntry)(int32_t, int32_t, const char *, const char *);
int32_t worminal_frontend_runtime_v1(int32_t, int32_t, const char *, const char *);
typedef int32_t (*WorminalRustFrontendEntry)(int32_t, const char *const *, int32_t, uint32_t);
int32_t worminal_rust_frontend_v2(int32_t, const char *const *, int32_t, uint32_t);
#endif
