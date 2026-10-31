/* Exact byte oracle for the Rust application target resolver. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "browser_target.h"

int
main(int argc, char **argv)
{
	char *target = workspace_browser_target(argc > 1 ? argv[1] : NULL);
	if (!target) return 1;
	size_t length = strlen(target);
	int okay = fwrite(target, 1, length, stdout) == length;
	free(target);
	return !okay;
}
