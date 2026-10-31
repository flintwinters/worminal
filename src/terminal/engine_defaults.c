/* Terminal defaults shared by the standalone engine and legacy adapters. */
#include <wchar.h>
#include "st.h"

/* Identification returned for DA and DECID queries. */
char *vtiden = "\033[?6c";
wchar_t *worddelimiters = L" ";
/* Match the advertised terminfo tab width (it#8); application PTYs disable kernel tab expansion. */
unsigned int tabspaces = 8;
unsigned int defaultfg = WORMINAL_DEFAULT_FOREGROUND, defaultbg = WORMINAL_DEFAULT_BACKGROUND;
unsigned int defaultcs = WORMINAL_DEFAULT_CURSOR;
