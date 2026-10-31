/* Legacy ambient-session API; the parser itself has no ambient session or application callbacks. */
#include "engine_parser.h"
#define tfulldirt() te_tfulldirt(&session->engine)
#define tcursor(mode) te_tcursor(&session->engine, mode)
#define treset() te_treset(&session->engine)
#define tline(y) te_tline(&session->engine, y)
#define tswapscreen() te_tswapscreen(&session->engine)
#define tscrollup(orig, n) te_tscrollup(&session->engine, orig, n)
#define tmoveto(x, y) te_tmoveto(&session->engine, x, y)
#define tsetscroll(t, b) te_tsetscroll(&session->engine, t, b)
#define twrite(buf, buflen, show_ctrl) tlegacywrite(buf, buflen, show_ctrl)

int tlegacywrite(const char *, int, int);
