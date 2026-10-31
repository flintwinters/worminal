/* Stream oracle for Rust interoperability with the existing v2 C encoder and decoder. */
#include <stdio.h>
#include <unistd.h>
#include <string.h>
#include "wire.h"
#include "workspace_socket.h"

int
main(int argc, char **argv)
{
	WirePacket packet;
	if (argc == 2 && !strcmp(argv[1], "--socket")) {
		int fd = workspace_connect();
		if (fd < 0 || !wire_write(fd, WIRE_SERVICE_INFO, 0, NULL, 0) || !wire_read(fd, &packet)) return 1;
		uint32_t identity;
		int okay = packet.type == WIRE_SERVICE_INFO && wire_get_u32(&packet, &identity) && packet.pos == packet.len;
		if (okay) printf("%u\n", identity);
		wire_packet_free(&packet);
		close(fd);
		return !okay;
	}
	while (wire_read(STDIN_FILENO, &packet)) {
		WireBuffer encoded = {0};
		wire_append_packet(&encoded, packet.type, packet.tab, packet.data, packet.len);
		int okay = fwrite(encoded.data, 1, encoded.len, stdout) == encoded.len;
		wire_buffer_free(&encoded);
		wire_packet_free(&packet);
		if (!okay) return 1;
	}
	return ferror(stdout) != 0;
}
