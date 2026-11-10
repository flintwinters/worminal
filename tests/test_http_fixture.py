"""Private HTTP fixture responses and deterministic cleanup, including exception paths."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ssl
import threading
import unittest
import urllib.request

from http_fixture import serving
from preview import fixture


class HttpFixtureTest(unittest.TestCase):
    def test_http_and_tls_serve_complete_responses_then_join_server_thread(self):
        for tls in (False, True):
            with self.subTest(tls=tls):
                before = set(threading.enumerate())
                with fixture(tls=tls) as url:
                    workers = set(threading.enumerate()) - before
                    context = ssl._create_unverified_context() if tls else None
                    with urllib.request.urlopen(url, timeout=3, context=context) as response:
                        self.assertIn(b'<title>Fixture</title>', response.read())
                self.assertTrue(all(not worker.is_alive() for worker in workers))

    def test_body_exception_still_closes_socket_and_joins_server_thread(self):
        server = ThreadingHTTPServer(('127.0.0.1', 0), BaseHTTPRequestHandler)
        before = set(threading.enumerate())
        with self.assertRaisesRegex(RuntimeError, 'fixture body failed'):
            with serving(server):
                workers = set(threading.enumerate()) - before
                raise RuntimeError('fixture body failed')
        self.assertEqual(server.socket.fileno(), -1)
        self.assertTrue(workers)
        self.assertTrue(all(not worker.is_alive() for worker in workers))

    def test_cleanup_preserves_server_policy_for_inflight_request_handlers(self):
        started, release = threading.Event(), threading.Event()
        payload = b'complete response' * 8192
        received = []
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                started.set()
                if not release.wait(3): return
                self.send_response(200)
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            def log_message(self, *_args): pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.daemon_threads = False
        def request():
            with urllib.request.urlopen(f'http://127.0.0.1:{server.server_port}', timeout=3) as response:
                received.append(response.read())
        reader = threading.Thread(target=request)
        timer = threading.Timer(.05, release.set)
        try:
            with serving(server):
                reader.start()
                self.assertTrue(started.wait(3))
                timer.start()
            reader.join(timeout=3)
            self.assertFalse(reader.is_alive())
            self.assertEqual(received, [payload])
        finally:
            release.set()
            if timer.ident is not None: timer.join(timeout=3)
            if reader.ident is not None: reader.join(timeout=3)


if __name__ == '__main__':
    unittest.main()
