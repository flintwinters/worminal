/* Shared installation layout for lazily loaded frontend and browser modules. */
#ifndef WORMINAL_MODULES_H
#define WORMINAL_MODULES_H
#include <gmodule.h>

static inline char *
module_path(const char *name, const char *build_name)
{
	char *executable = g_file_read_link("/proc/self/exe", NULL);
	if (!executable) return NULL;
	char *directory = g_path_get_dirname(executable);
	g_free(executable);
	char *path = g_build_filename(directory, "..", "lib", "worminal", name, NULL);
	if (!g_file_test(path, G_FILE_TEST_EXISTS)) {
		g_free(path);
		path = g_build_filename(directory, "build", build_name ? build_name : name, NULL);
	}
	g_free(directory);
	return path;
}
#endif
