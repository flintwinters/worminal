"""Fake ssh placed first on PATH: accepts only the production bridge arguments for known test hosts."""
import os
import re
import sys
import time


def run(daemon, hosts):
    arguments = sys.argv[1:]
    if not any(arguments == ["-T", "-C", "-oConnectTimeout=10", host, daemon, "--bridge"] for host in hosts):
        sys.exit(64)
    master = os.environ.get("WORMINAL_TEST_REAL_MASTER")
    if master:
        # Forward to a real host; the values reach a remote shell, so accept only plain names.
        remote = os.environ.get("WORMINAL_TEST_REMOTE_DAEMON") or "worminald"
        scope = os.environ.get("WORMINAL_SHARED_SOCKET_SCOPE", "")
        if not re.fullmatch(r"[A-Za-z0-9_./-]+", remote) or not re.fullmatch(r"[A-Za-z0-9_-]+", scope):
            sys.exit(64)
        os.execv("/usr/bin/ssh", ["/usr/bin/ssh", "-T", "-C", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10",
                                  master, f"env WORMINAL_SHARED_SOCKET_SCOPE={scope} {remote} --bridge"])
    delay = os.environ.get("WORMINAL_TEST_BRIDGE_DELAY")
    if delay:
        time.sleep(float(delay))
    test_daemon = os.environ["WORMINAL_TEST_DAEMON"]
    os.execvp(test_daemon, [test_daemon, "--bridge"])
