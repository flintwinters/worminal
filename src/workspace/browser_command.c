/* The launcher and daemon CLI send the same calling-PTY conversion request. */
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <unistd.h>
#include "browser_target.h"
#include "wire.h"
#include "workspace_client.h"
#include "workspace_socket.h"

int
workspace_browser_command(const char *url)
{
	char *target = workspace_browser_target(url);
	if (!target) { perror("Cannot resolve browser target"); return 1; }
	int fd = workspace_connect();
	if (fd < 0) { free(target); fputs("No preview workspace service is running.\n", stderr); return 1; }
	/* Converting this PTY hangs up its shell. Let the short-lived command receive
	 * its result and exit normally even if it is in the foreground process group. */
	signal(SIGHUP, SIG_IGN);
	struct timeval timeout = {.tv_sec = 3};
	setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
	WireBuffer buffer = {0};
	wire_put_string(&buffer, target);
	free(target);
	int okay = wire_write(fd, WIRE_BROWSER_REPLACE, 0, buffer.data, buffer.len);
	wire_buffer_free(&buffer);
	WirePacket packet;
	if (okay && wire_read(fd, &packet) > 0) {
		okay = packet.type == WIRE_COMMAND_DONE;
		if (!okay) {
			char *error = packet.type == WIRE_ERROR ? wire_get_string(&packet) : NULL;
			fprintf(stderr, "%s\n", error ? error : "Preview service rejected the browser command.");
			free(error);
		}
		wire_packet_free(&packet);
	} else {
		fputs("Preview service did not reply; an older service may need restarting after its sessions end.\n", stderr);
		okay = 0;
	}
	close(fd);
	return okay ? 0 : 1;
}
