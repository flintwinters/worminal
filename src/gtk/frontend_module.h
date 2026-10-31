/* Versioned, container-free entrypoint shared by C modules and the Rust launcher. */
#ifndef WORMINAL_FRONTEND_MODULE_H
#define WORMINAL_FRONTEND_MODULE_H
#define WORMINAL_FRONTEND_SYMBOL "worminal_frontend_v1"
#define WORMINAL_FRONTEND_FILE "frontend.so"
typedef int (*WorminalFrontendEntry)(int, char **, int);
int worminal_frontend_v1(int, char **, int);
#endif
