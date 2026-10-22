"""Attribute diagnostic CPU samples to executable mappings before visible content."""
from collections import Counter
from pathlib import Path
import struct


def collect(pids, directory, cutoff):
    result = []
    for pid in sorted(pids):
        path = directory / f"{pid}.samples"
        if not path.exists():
            continue
        try:
            maps = Path(f"/proc/{pid}/maps").read_text()
            name = Path(f"/proc/{pid}/comm").read_text().strip()
        except (FileNotFoundError, ProcessLookupError):
            continue
        (directory / f"{pid}.maps").write_text(maps)
        mappings = []
        for line in maps.splitlines():
            fields = line.split(maxsplit=5)
            if "x" not in fields[1]:
                continue
            start, end = (int(value, 16) for value in fields[0].split("-"))
            mappings.append((start, end, int(fields[2], 16), fields[5] if len(fields) == 6 else "[anonymous]"))
        data = path.read_bytes()
        count = min(struct.unpack_from("=Q", data)[0], (len(data) - 8) // 16)
        modules, addresses = Counter(), Counter()
        for timestamp, pc in struct.iter_unpack("=QQ", data[8:8 + count * 16]):
            if timestamp > cutoff or not pc:
                continue
            for start, end, offset, module in mappings:
                if start <= pc < end:
                    modules[module] += 1
                    addresses[module, pc - start + offset] += 1
                    break
        result.append({"pid": pid, "name": name, "samples": sum(modules.values()),
                       "libraries": [{"path": module, "samples": count} for module, count in modules.most_common()],
                       "addresses": [{"path": module, "offset": hex(offset), "samples": count}
                                     for (module, offset), count in addresses.most_common(40)]})
    if not result or not any(item["samples"] for item in result):
        raise RuntimeError("CPU sampler produced no samples")
    return result
