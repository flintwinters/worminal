"""Private OpenSSH substitute. A test-only SOCKS server for a locally unresolvable hostname."""
import socket
import sys

path = sys.argv[sys.argv.index("-D") + 1]
assert sys.argv[-2:] == ["--", "tester@proxy-host"]
with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
    server.bind(path)
    server.listen()
    while True:
        connection, _ = server.accept()
        with connection, connection.makefile("rb") as stream:
            connection.settimeout(5)
            try:
                header = stream.read(2)
                if not header:
                    continue
                assert header[0] == 5 and 0 in stream.read(header[1])
                connection.sendall(b"\x05\x00")
                assert stream.read(4) == b"\x05\x01\x00\x03"
                assert stream.read(stream.read(1)[0]) == b"proxy-test.invalid"
                assert stream.read(2) == b"\x00\x50"
                connection.sendall(b"\x05\x00\x00\x01\x00\x00\x00\x00\x00\x00")
                request = stream.readline()
                if not request:
                    continue
                _, target, _ = request.split()
                while stream.readline().strip():
                    pass
                body = b"<title>Via SSH " + target + b"</title>"
                connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n"
                                   + f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
            except (BrokenPipeError, ConnectionResetError, TimeoutError):
                continue
