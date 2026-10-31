/* Shared bounded GTK main-context pumping for native widget checks. */
#ifndef WORMINAL_GTK_WAIT_H
#define WORMINAL_GTK_WAIT_H
#ifndef GTK_WAIT_DIAGNOSTICS
#define GTK_WAIT_DIAGNOSTICS() ((void)0)
#endif
#define WAIT(condition) do { \
	gint64 deadline = g_get_monotonic_time() + 10000000; \
	while (!(condition) && g_get_monotonic_time() < deadline) { \
		while (g_main_context_iteration(NULL, FALSE)) {} \
		g_usleep(1000); \
	} \
	if (!(condition)) { GTK_WAIT_DIAGNOSTICS(); g_error("Timed out: %s (line %d)", #condition, __LINE__); } \
} while (0)
#endif
