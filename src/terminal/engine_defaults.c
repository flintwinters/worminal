/* Terminal defaults shared by the standalone engine and legacy adapters. */
#include <wchar.h>
#include "st.h"

/* Identification returned for DA and DECID queries. */
char *vtiden = "\033[?6c";
wchar_t *worddelimiters = L" ";
/* Match the advertised terminfo tab width (it#8); application PTYs disable kernel tab expansion. */
unsigned int tabspaces = 8;
unsigned int defaultfg = 258, defaultbg = 259, defaultcs = 256;
