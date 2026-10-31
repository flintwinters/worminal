#include <stdlib.h>
#include <string.h>
#include "st.h"
#include "workspace_launch.h"
#include "restore_format.h"

void
launch_free(Launch *launch)
{
	free(launch->title);
	free(launch->cwd);
	free(launch->line);
	free(launch->output);
	free(launch->windowid);
	for (uint32_t i = 0; i < launch->argc; i++)
		free(launch->args[i]);
	for (uint32_t i = 0; i < launch->envc; i++)
		free(launch->env[i]);
	free(launch->args);
	free(launch->env);
}

int
launch_read(WirePacket *packet, Launch *launch)
{
	memset(launch, 0, sizeof(*launch));
	if (!wire_get_u32(packet, &launch->cols) || !wire_get_u32(packet, &launch->rows) ||
	    !wire_get_u32(packet, &launch->allowalt) || !wire_get_u32(packet, &launch->argc) ||
	    !wire_get_u32(packet, &launch->envc) || launch->cols < 1 || launch->cols > WORKSPACE_MAX_COLS ||
	    launch->rows < 1 || launch->rows > WORKSPACE_MAX_ROWS ||
	    launch->argc > WORKSPACE_MAX_ARGS || launch->envc > WORKSPACE_MAX_ENV)
		return 0;
	launch->title = wire_get_string(packet);
	launch->cwd = wire_get_string(packet);
	launch->line = wire_get_string(packet);
	launch->output = wire_get_string(packet);
	launch->windowid = wire_get_string(packet);
	if (!launch->title || !launch->cwd || !launch->line || !launch->output ||
	    !launch->windowid)
		return 0;
	launch->args = calloc(launch->argc + 1, sizeof(char *));
	launch->env = calloc(launch->envc + 1, sizeof(char *));
	if (!launch->args || !launch->env)
		abort();
	for (uint32_t i = 0; i < launch->argc; i++)
		if (!(launch->args[i] = wire_get_string(packet)))
			return 0;
	for (uint32_t i = 0; i < launch->envc; i++)
		if (!(launch->env[i] = wire_get_string(packet)))
			return 0;
	return packet->pos == packet->len;
}


void
launch_pty(Launch *launch, const char *cwd)
{
	/* An omitted environment means inherit the daemon (not an empty environment),
	 * as required by SSH launches that must keep the remote account settings. */
	extern char **environ;
	char restore_flag[] = RESTORE_ENV_NAME "=1";
	const char restore_prefix[] = RESTORE_ENV_NAME "=";
	char **environment = launch->envc ? launch->env : environ, **restore_env = NULL;
	if (launch->restored) {
		size_t count = 0, used = 0;
		while (environment[count]) count++;
		restore_env = calloc(count + 2, sizeof(char *));
		if (!restore_env) abort();
		for (size_t i = 0; i < count; i++)
			if (strncmp(environment[i], restore_prefix, sizeof(restore_prefix) - 1))
				restore_env[used++] = environment[i];
		restore_env[used] = restore_flag;
	}
	ttysetlaunch(cwd, restore_env ? restore_env : launch->envc ? launch->env : NULL,
	             *launch->windowid ? launch->windowid : NULL);
	ttynew(*launch->line ? launch->line : NULL, "/bin/sh",
	       *launch->output ? launch->output : NULL,
	       launch->argc ? launch->args : NULL);
	ttysetlaunch(NULL, NULL, NULL);
	free(restore_env);
}
