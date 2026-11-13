/* Keep the parser in one translation unit so st's per-character operations can still be inlined.
 * Source chunks keep the operations reviewable; all state is passed explicitly. */
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>
#include "engine_parser.h"
#include "engine_context.h"
static int te_tlinelen(TermEngine *session, int y);
static void te_selnormalize(TermEngine *session);
static void te_selsnap(TermEngine *session, int *x, int *y, int direction);
static Line te_thistorypush(TermEngine *session, Line line);
static void te_tscrolldown(TermEngine *session, int orig, int n);
static void te_selscroll(TermEngine *session, int orig, int n);
static void te_tnewline(TermEngine *session, int first_col);
static void te_csiparse(TermEngine *session);
static void te_tmoveato(TermEngine *session, int x, int y);
static void te_tsetchar(TermEngine *session, Rune u, const Glyph *attr, int x, int y);
static void te_tclearregion(TermEngine *session, int x1, int y1, int x2, int y2);
static void te_tdeletechar(TermEngine *session, int n);
static void te_tinsertblank(TermEngine *session, int n);
static void te_tinsertblankline(TermEngine *session, int n);
static void te_tdeleteline(TermEngine *session, int n);
static int32_t te_tdefcolor(TermEngine *session, const int *attr, int *npar, int l);
static void te_tsetattr(TermEngine *session, const int *attr, int l);
static void te_tsetmode(TermEngine *session, int priv, int set, const int *args, int narg);
static void te_csihandle(TermEngine *session);
static void te_csidump(TermEngine *session);
static void te_csireset(TermEngine *session);
static void te_osc_color_response(TermEngine *session, int num, int index, int is_osc4);
static void te_strhandle(TermEngine *session);
static void te_strparse(TermEngine *session);
static void te_strdump(TermEngine *session);
static void te_strreset(TermEngine *session);
static void te_tdumpsel(TermEngine *session);
static void te_tdumpline(TermEngine *session, int n);
static void te_tdump(TermEngine *session);
static void te_tputtab(TermEngine *session, int n);
static void te_tdefutf8(TermEngine *session, char ascii);
static void te_tdeftran(TermEngine *session, char ascii);
static void te_tdectest(TermEngine *session, char c);
static void te_tstrsequence(TermEngine *session, uchar c);
static void te_tcontrolcode(TermEngine *session, uchar ascii);
static int te_eschandle(TermEngine *session, uchar ascii);
static void te_tputc(TermEngine *session, Rune u);
static void te_tresizeplain(TermEngine *session, int col, int row);
static Line te_treflowline(TermEngine *session, Reflow *out);
static void te_treflownew(TermEngine *session, Reflow *out);
static void te_treflow(TermEngine *session, int col, int row);
static size_t utf8decode(const char *, Rune *, size_t);
static Rune utf8decodebyte(char, size_t *);
static char utf8encodebyte(Rune, size_t);
static size_t utf8validate(Rune *, size_t);
static char *base64dec(const char *);
static char base64dec_getc(const char **);
#include "engine_util.inc"
#include "engine_selection.inc"
#include "engine_parse.inc"
#include "engine_links.inc"
#include "engine_strings.inc"
#include "engine_reflow.inc"
