"""Read-only report of whether live Worminal processes run the installed files."""
import os
from pathlib import Path

EXECUTABLES = ("worminal", "worminald")


def _sockets():
    """Abstract unix socket inode -> name, from the kernel table."""
    names = {}
    for line in Path("/proc/net/unix").read_text().splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 8 and fields[7].startswith("@worminal-workspace-"):
            names[fields[6]] = fields[7]
    return names


def _protocol(pid, sockets):
    """Daemon protocol from the workspace socket it listens on; v1 processes are never touched implicitly."""
    for entry in Path(f"/proc/{pid}/fd").iterdir():
        try:
            target = os.readlink(entry)
        except OSError:
            continue
        if target.startswith("socket:[") and (name := sockets.get(target[8:-1])):
            return "v2" if name.startswith("@worminal-workspace-v2-") else "v1"
    return None


def processes(prefix="/usr/local"):
    """One record per live process of this user running an executable from `prefix`/bin."""
    installed = {name: Path(prefix, "bin", name) for name in EXECUTABLES}
    sockets, records = _sockets(), []
    for process in Path("/proc").iterdir():
        if not process.name.isdecimal() or process.stat().st_uid != os.getuid():
            continue
        try:
            link = os.readlink(process / "exe")
            path = Path(link.removesuffix(" (deleted)"))
            if path != installed.get(path.name):
                continue
            maps = (process / "maps").read_text().splitlines()
            stale_modules = sorted({line.split()[-2] for line in maps
                                    if f"{prefix}/lib/worminal/" in line and line.endswith("(deleted)")})
            current = path.exists() and os.stat(process / "exe").st_ino == path.stat().st_ino
            records.append({"pid": int(process.name), "name": path.name, "args": (process / "cmdline").read_bytes()
                            .replace(b"\0", b" ").decode(errors="replace").strip(),
                            "protocol": _protocol(process.name, sockets) if path.name == "worminald" else None,
                            "current": current and not stale_modules, "stale_modules": stale_modules})
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return sorted(records, key=lambda record: record["pid"])


def status(record):
    if record["current"]:
        return "current"
    if record["protocol"] == "v1":
        return "STALE (v1: left running by policy, never migrated implicitly)"
    return "STALE (v2 owners hand off on their own; if this persists the watch did not fire)"


def report(prefix="/usr/local"):
    records = processes(prefix)
    for record in records:
        print(f"{record['pid']:>8} {record['name']:<10} {record['protocol'] or '':<3} {status(record)}")
    stale = [record for record in records if not record["current"]]
    print(f"{len(records) - len(stale)} current, {len(stale)} stale of {len(records)} live processes from {prefix}.")
    return records
