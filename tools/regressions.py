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
    service_support = [*(f"src/{source}" for source in build.COMMON), "src/workspace/workspace_handoff.c"]
    # Restore the blocking service write path; the reused PTY regression must fail.
    service = directory / "service.c"
    service.write_text((ROOT / "src/workspace/workspace_service.c").read_text().replace(
        "ttyrawhook = service_input;", "ttyrawhook = NULL;"))
    daemon = directory / "worminald"
    builder.program(daemon, [*service_support, str(service)], variant="blocked-proof")
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_workspace.py",
               "-k", "blocked_pty", "-q"]
    env = {**os.environ, "WORMINAL_TEST_WORKSPACE_DAEMON": str(daemon)}
    cases = [("blocked PTY", command, env, b"TimeoutError: timed out")]
    stale = directory / "stale-resize.c"
    stale.write_text((ROOT / "src/workspace/workspace_service.c").read_text().replace(
        "if (tab->controller != client) break;", "/* Missing ownership check. */"))
    preview = directory / "worminald-preview"
    builder.program(preview, [*service_support, str(stale)],
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
    builder.program(ordered_daemon, [*service_support, str(ordering)],
                    variant="ordering-proof", flags=["-DWORMINAL_PREVIEW_PROTOCOL"])
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_preview_workspace.py",
               "-k", "input_reply_frame", "-q"]
    env = {**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(ordered_daemon)}
    cases.append(("input reply/mode ordering", command, env, b"mode catalog preceded its corresponding frame"))
    current_parser = (ROOT / "src/terminal/st.c").read_text()

    def parser_case(name, source, expected):
        if source == current_parser:
            raise ValueError(f"Parser counterfactual no longer removes a fix: {name}")
        parser = directory / f"{name}-st.c"
        parser.write_text(source)
        test = directory / f"{name}-test.c"
        test.write_text((ROOT / "tests/scrollback.c").read_text().replace(
            '../src/terminal/st.c', str(parser)))
        binary = directory / name
        builder.program(binary, [str(test), "src/terminal/session_transfer.c"], variant=f"{name}-proof",
                        flags=["-Itests", "-ffunction-sections", "-fdata-sections"],
                        libs=["-Wl,--gc-sections", "-lm"])
        cases.append((name, [str(binary)], os.environ, expected))

    parser_case("read-capacity", current_parser.replace("sizeof(session->ttybuf)-buflen", "LEN(buf)-buflen"),
                b"ttyread() == strlen(batch)")
    stale_cursor = current_parser.replace("if (cursor.state & CURSOR_WRAPNEXT) cursor.x = oldcol;", "")
    stale_cursor = stale_cursor.replace("if (saved.state & CURSOR_WRAPNEXT) saved.x = oldcol;", "")
    stale_cursor = stale_cursor.replace("MODBIT(term.c.state, cursorend, CURSOR_WRAPNEXT);",
        "MODBIT(term.c.state, cursorend || (cursor.state & CURSOR_WRAPNEXT && term.c.x == col - 1), "
        "CURSOR_WRAPNEXT);")
    stale_cursor = stale_cursor.replace("MODBIT(session->saved[0].state, savedend, CURSOR_WRAPNEXT);",
        "MODBIT(session->saved[0].state, savedend || (saved.state & CURSOR_WRAPNEXT && savedx == col - 1), "
        "CURSOR_WRAPNEXT);")
    parser_case("pending-wrap", stale_cursor, b"term.line[3 / width][3 % width].u == 'd'")
    for index, (name, command, env, expected) in enumerate(cases):
        with (directory / f"failure-{index}.log").open("wb") as log:
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, timeout=15)
        log_data = (directory / f"failure-{index}.log").read_bytes()
        if result.returncode == 0 or expected not in log_data:
            raise ValueError(f"Regression did not detect the removed fix: {name}")
        print(f"Regression proves fix: {name} (counterfactual fails; logs in .checks/regressions)")
