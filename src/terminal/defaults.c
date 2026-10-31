/* Defaults shared by every terminal owner and painting frontend. */
#include <wchar.h>
#include "st.h"
#include "workspace/process_defaults.h"

char *utmp, *scroll;
char *stty_args = PROCESS_STTY;
int allowaltscreen = 1;
/* Micro recognizes Ctrl+End with this terminfo name. Keep the shared Home/End
 * sequences consistent with it; see the navigation and Micro regression tests. */
char *termname = PROCESS_TERM_NAME;
