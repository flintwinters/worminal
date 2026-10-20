#ifndef WORMINAL_TAB_LABEL_H
#define WORMINAL_TAB_LABEL_H
#include <string.h>

/* Both frontends name terminal tabs after the last working-directory component. */
static inline const char *
tab_label(const char *directory, int *length)
{
	size_t end = strlen(directory), start;
	while (end > 1 && directory[end - 1] == '/') end--;
	if (end == 1 && directory[0] == '/') { *length = 1; return directory; }
	start = end;
	while (start > 0 && directory[start - 1] != '/') start--;
	*length = end - start;
	return directory + start;
}
#endif
