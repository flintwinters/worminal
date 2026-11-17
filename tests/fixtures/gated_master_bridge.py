"""Private master transport that holds accepted input before the real daemon sees it."""
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import time

native = os.environ['WORMINAL_TEST_GATE_DAEMON']
if sys.argv[1:] != ['--bridge']:
    os.execv(native, [native, *sys.argv[1:]])

sys.path.insert(0, os.environ['WORMINAL_TEST_PYTHON_DIR'])
from workspace_protocol import packet, send, WorkspaceClosed

upstream = socket.socket(fileno=0)
parent, child = socket.socketpair()
with child:
    process = subprocess.Popen([native, '--bridge'], stdin=child, stdout=child)

def downstream():
    try:
        while data := parent.recv(16384):
            upstream.sendall(data)
    except OSError:
        pass
    finally:
        try:
            upstream.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass

threading.Thread(target=downstream, daemon=True).start()
try:
    while True:
        kind, tab, body = packet(upstream, version=2)
        if kind == 30 and (drop := os.environ.get('WORMINAL_TEST_DROP_FENCE')):
            # Drop exactly the original owner's first fence; replacement/reconnect bridges
            # use --bridge-existing and bypass this wrapper. No input is held or replayed.
            Path(drop).touch()
            break
        if kind == 3:  # WIRE_INPUT; retain exact packet bytes until the fixture releases backpressure.
            Path(os.environ['WORMINAL_TEST_INPUT_WAITING']).touch()
            while not Path(os.environ['WORMINAL_TEST_INPUT_GATE']).exists():
                time.sleep(.01)
        send(parent, kind, tab, body, version=2)
except (OSError, WorkspaceClosed):
    pass
finally:
    upstream.close()
    parent.close()
    if process.poll() is None:
        process.terminate()
    process.wait(timeout=5)
