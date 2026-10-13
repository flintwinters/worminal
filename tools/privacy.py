"""Conservative, value-redacting privacy checks for tracked content and Git history."""

import re
import subprocess

from rich.console import Console


PATTERNS = {
    "private key": r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----",
    "service credential": r"\b(?:AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9]{20,}|"
                          r"github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}|"
                          r"xox[baprs]-[A-Za-z0-9-]{20,})\b",
    "credential assignment": r"(?i)\b(?:password|api_key|access_token|client_secret)\s*[:=]\s*"
                             r"[\"'][^\"'\s]{8,}[\"']",
    "personal home path": r"/(?:home|Users)/[A-Za-z0-9_.-]+/",
    "private network address": r"\b(?:10\.\d{1,3}|192\.168|172\.(?:1[6-9]|2\d|3[01]))"
                               r"\.\d{1,3}\.\d{1,3}\b",
    "email address": r"\b[A-Za-z0-9_.+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
    "URL credential": r"[a-zA-Z]+://[^\s/:]+:[^\s/@]+@",
}
RULES = {name: re.compile(pattern) for name, pattern in PATTERNS.items()}


def findings(data):
    """Return categories and line numbers without retaining matched values."""
    text = data.decode("utf-8", errors="replace")
    return [(name, text.count("\n", 0, match.start()) + 1)
            for name, pattern in RULES.items() for match in pattern.finditer(text)]


def git(*args):
    return subprocess.check_output(["git", *args])


def check_privacy(root, history=False, staged=False, identities=False, message_file=None):
    console = Console()
    count = 0

    def report(location, data):
        nonlocal count
        for category, line in findings(data):
            console.print(f"{location}:{line}: {category}", markup=False)
            count += 1

    for raw in git("ls-files", "-z").split(b"\0"):
        if not raw:
            continue
        path = root / raw.decode("utf-8", errors="surrogateescape")
        # Upstream license attribution must remain intact.
        if path.name == "LICENSE":
            continue
        data = git("show", f":{path.relative_to(root).as_posix()}") if staged else path.read_bytes()
        report(path.relative_to(root).as_posix(), data)
    if identities:
        for variable in ("GIT_AUTHOR_IDENT", "GIT_COMMITTER_IDENT"):
            if not git("var", variable).startswith(b"Worminal contributor <anonymous> "):
                console.print(f"{variable}: use the anonymous project identity (manage.py privacy-setup).")
                count += 1
    if message_file:
        report("commit message", message_file.read_bytes())
    if history:
        objects = git("rev-list", "--objects", "--all").splitlines()
        for entry in objects:
            oid, _, path = entry.partition(b" ")
            if path == b"LICENSE":
                continue
            if git("cat-file", "-t", oid.decode()).strip() == b"blob":
                report(f"history blob {oid.decode()[:12]}", git("cat-file", "blob", oid.decode()))
        for entry in git("log", "--all", "--format=%H").splitlines():
            oid = entry.decode()
            report(f"commit {oid[:12]}", git("show", "-s", "--format=%an <%ae>%n%cn <%ce>%n%B", oid))
    console.print(f"Privacy check: {count} finding(s).", markup=False)
    return bool(count)
