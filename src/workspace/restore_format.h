/* Recovery recipe format, shared by the C and Rust application implementations. */
#ifndef WORMINAL_RESTORE_FORMAT_H
#define WORMINAL_RESTORE_FORMAT_H
#include <limits.h>
#define RESTORE_VERSION 1u
#define RESTORE_LIMIT (8u * 1024u * 1024u)
#define RESTORE_TABS 4096u
#define RESTORE_URL_LIMIT 65536u
#define RESTORE_ENV_NAME "WORMINAL_RESTORE"
enum restore_limits { RESTORE_DIRECTORY_LIMIT = PATH_MAX };
#endif
