"""Read-only report of whether live Worminal processes run the installed files."""
import os
from pathlib import Path

EXECUTABLES = ("worminal", "worminald")


def processes(prefix="/usr/local"):
    """One record per live process of this user running an executable from `prefix`/bin."""
    installed = {name: Path(prefix, "bin", name) for name in EXECUTABLES}
    records = []
    for process in Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        try:
            if process.stat().st_uid != os.getuid():
                continue
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
                            "current": current and not stale_modules, "stale_modules": stale_modules})
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return sorted(records, key=lambda record: record["pid"])


def status(record):
    if record["current"]:
        return "current"
    return "STALE (owners hand off on their own; if this persists the watch did not fire)"


def report(prefix="/usr/local"):
    records = processes(prefix)
    for record in records:
        print(f"{record['pid']:>8} {record['name']:<10} {status(record)}")
    stale = [record for record in records if not record["current"]]
    print(f"{len(records) - len(stale)} current, {len(stale)} stale of {len(records)} live processes from {prefix}.")
    return records
