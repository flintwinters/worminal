"""Shared lifecycle for private HTTP servers used by browser checks and measurements."""
from contextlib import contextmanager
import threading


@contextmanager
def serving(server):
    # serve_forever's default 0.5s poll is a shutdown delay paid by every short fixture.
    # Requests still wake select immediately; retain shutdown, server_close and thread join.
    worker = threading.Thread(target=server.serve_forever, kwargs={'poll_interval': .02}, daemon=True)
    worker.start()
    try:
        yield server
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
