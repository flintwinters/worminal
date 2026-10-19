/* Defaults shared by every terminal owner and painting frontend. */
#include <wchar.h>
#include "st.h"

char *utmp, *scroll;
char *stty_args = "stty raw pass8 nl -echo -iexten -cstopb 38400";
/* Identification returned for DA and DECID queries. */
char *vtiden = "\033[?6c";
wchar_t *worddelimiters = L" ";
int allowaltscreen = 1;
/* Micro recognizes Ctrl+End with this terminfo name. Keep the shared Home/End
 * sequences consistent with it; see the navigation and Micro regression tests. */
char *termname = "xterm-256color";
/* Match the advertised terminfo tab width (it#8); kernel tab expansion must be off. */
unsigned int tabspaces = 8;
unsigned int defaultfg = 258, defaultbg = 259, defaultcs = 256;
