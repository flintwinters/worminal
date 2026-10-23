/* Preserve rare service failures even when a detached launch discards stderr. */
#ifndef WORMINAL_SERVICE_DIAGNOSTICS_H
#define WORMINAL_SERVICE_DIAGNOSTICS_H
#include <errno.h>
#include <string.h>
#include <syslog.h>

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
