/* Application process defaults shared during the C/Rust service transition. */
#ifndef WORMINAL_PROCESS_DEFAULTS_H
#define WORMINAL_PROCESS_DEFAULTS_H
#define PROCESS_TERM_NAME "xterm-256color"
#define PROCESS_SHELL "/bin/sh"
#define PROCESS_STTY "stty raw pass8 nl -echo -iexten -cstopb 38400"
#endif
