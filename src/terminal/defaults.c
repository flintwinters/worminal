/* Defaults shared by every terminal owner and painting frontend. */
#include <wchar.h>
#include "st.h"

char *utmp, *scroll;
char *stty_args = "stty raw pass8 nl -echo -iexten -cstopb 38400";
int allowaltscreen = 1;
/* Micro recognizes Ctrl+End with this terminfo name. Keep the shared Home/End
 * sequences consistent with it; see the navigation and Micro regression tests. */
char *termname = "xterm-256color";
