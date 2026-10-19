"""Prove critical fixes by compiling local counterfactuals, never editing working sources."""

import os
from pathlib import Path
import subprocess
import sys

from tools import build

ROOT = Path(__file__).resolve().parent.parent


def prove():
    directory = ROOT / ".checks/regressions"
    directory.mkdir(parents=True, exist_ok=True)
    builder = build.Builder()
    # Restore the blocking service write path; the reused PTY regression must fail.
    service = directory / "service.c"
    service.write_text((ROOT / "src/workspace/workspace_service.c").read_text().replace(
        "ttyrawhook = service_input;", "ttyrawhook = NULL;"))
    daemon = directory / "worminald"
    builder.program(daemon, [*(f"src/{source}" for source in build.COMMON), str(service)], variant="blocked-proof")
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_workspace.py",
               "-k", "blocked_pty", "-q"]
    env = {**os.environ, "WORMINAL_TEST_WORKSPACE_DAEMON": str(daemon)}
    cases = [("blocked PTY", command, env, b"TimeoutError: timed out")]
    stale = directory / "stale-resize.c"
    stale.write_text((ROOT / "src/workspace/workspace_service.c").read_text().replace(
        "if (tab->controller != client) break;", "/* Missing ownership check. */"))
    preview = directory / "worminald-preview"
    builder.program(preview, [*(f"src/{source}" for source in build.COMMON), str(stale)],
                    variant="resize-proof", flags=["-DWORMINAL_PREVIEW_PROTOCOL"])
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_preview_workspace.py",
               "-k", "stale_resize", "-q"]
    env = {**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(preview)}
    cases.append(("stale resize ownership", command, env, b"stale resize released the new controller"))
    ordering = directory / "mode-ordering.c"
    ordering.write_text((ROOT / "src/workspace/workspace_service.c").read_text().replace(
        "if (tab->input_frame) {", "if (0) {").replace(
        "if (tab->pending_frame) flush_frame(tab);", "/* Missing pre-catalog frame flush. */"))
    ordered_daemon = directory / "unordered-preview"
    builder.program(ordered_daemon, [*(f"src/{source}" for source in build.COMMON), str(ordering)],
                    variant="ordering-proof", flags=["-DWORMINAL_PREVIEW_PROTOCOL"])
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_preview_workspace.py",
               "-k", "input_reply_frame", "-q"]
    env = {**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(ordered_daemon)}
    cases.append(("input reply/mode ordering", command, env, b"mode catalog preceded its corresponding frame"))
    # Restore sizeof(pointer) as the read capacity in the native parser test.
    parser = directory / "st.c"
    parser.write_text((ROOT / "src/terminal/st.c").read_text().replace(
        "sizeof(session->ttybuf)-buflen", "LEN(buf)-buflen"))
    test = directory / "scrollback.c"
    test.write_text((ROOT / "tests/scrollback.c").read_text().replace(
        '../src/terminal/st.c', str(parser)))
    binary = directory / "read-capacity"
    builder.program(binary, [str(test)], variant="read-proof", flags=["-ffunction-sections", "-fdata-sections"],
                    libs=["-Wl,--gc-sections", "-lm"])
    cases.append(("PTY buffer capacity", [str(binary)], os.environ, b"ttyread() == strlen(batch)"))
    for index, (name, command, env, expected) in enumerate(cases):
        with (directory / f"failure-{index}.log").open("wb") as log:
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, timeout=15)
        log_data = (directory / f"failure-{index}.log").read_bytes()
        if result.returncode == 0 or expected not in log_data:
            raise ValueError(f"Regression did not detect the removed fix: {name}")
        print(f"Regression proves fix: {name} (counterfactual fails; logs in .checks/regressions)")
