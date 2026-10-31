/* Preserve rare service failures even when a detached launch discards stderr. */
#ifndef WORMINAL_SERVICE_DIAGNOSTICS_H
#define WORMINAL_SERVICE_DIAGNOSTICS_H
#include <errno.h>
#include <signal.h>
#include <string.h>
#include <syslog.h>

static void
service_stop_signals(void (*handler)(int), sigset_t *blocked)
{
	struct sigaction stop = {.sa_handler = handler};
	sigemptyset(&stop.sa_mask);
	sigemptyset(blocked);
	int signals[] = {SIGTERM, SIGINT, SIGHUP};
	for (size_t i = 0; i < sizeof(signals) / sizeof(signals[0]); i++) {
		sigaction(signals[i], &stop, NULL);
		sigaddset(blocked, signals[i]);
	}
}

static int
service_failure(const char *operation)
{
	int error = errno;
	syslog(LOG_ERR, "%s failed: %s (errno %d)", operation, strerror(error), error);
	return 1;
}

static int
service_diagnostics(int (*run)(void))
{
	openlog(WORKSPACE_DAEMON, LOG_PID, LOG_USER);
	syslog(LOG_NOTICE, "Workspace service started (protocol %d)", WIRE_VERSION);
	int result = run();
	syslog(result ? LOG_ERR : LOG_NOTICE, "Workspace service exited with status %d", result);
	closelog();
	return result;
}
#endif
