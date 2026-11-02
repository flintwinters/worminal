"""Prove critical fixes by compiling local counterfactuals, never editing working sources."""

import os
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tomllib

from tools import build

ROOT = Path(__file__).resolve().parent.parent


def rust_fixture(directory, name, component="daemon", *, tests=False):
    source = ROOT / "rust" / component
    target = directory / name
    shutil.copytree(source / "src", target / "src", dirs_exist_ok=True)
    if (source / "build.rs").exists():
        build_script = (source / "build.rs").read_text().replace('env::var_os("CARGO_MANIFEST_DIR")',
            f'Some(std::ffi::OsString::from({json.dumps(str(source))}))')
        (target / "build.rs").write_text(build_script)
    manifest = tomllib.loads((source / "Cargo.toml").read_text())
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    package = f"worminal-{name}-proof"
    text = f'[workspace]\n[package]\nname={json.dumps(package)}\n'
    text += f'version={json.dumps(manifest["package"]["version"])}\n'
    text += f'edition={json.dumps(workspace["workspace"]["package"]["edition"])}\n'
    text += f'[lib]\nname="worminal_{component.replace("-", "_")}"\n'
    for section in ("dependencies", "build-dependencies", "dev-dependencies"):
        if section not in manifest:
            continue
        text += f'[{section}]\n'
        for name, dependency in manifest[section].items():
            if isinstance(dependency, str):
                dependency = {"version": dependency}
            elif "path" in dependency:
                dependency = {**dependency, "path": str((source / dependency["path"]).resolve())}
            elif dependency.get("workspace"):
                shared = workspace["workspace"]["dependencies"][name]
                dependency = {**({"version": shared} if isinstance(shared, str) else shared),
                              **{key: value for key, value in dependency.items() if key != "workspace"}}
            fields = ", ".join(f"{key}={json.dumps(value)}" for key, value in dependency.items())
            text += f'{name} = {{{fields}}}\n'
    (target / "Cargo.toml").write_text(text)
    if tests:
        shutil.copytree(source / "tests", target / "tests", dirs_exist_ok=True)
    return target, package


def rust_build(directory, name, command):
    with (directory / f"{name}-build.log").open("wb") as log:
        subprocess.run(command, cwd=ROOT, stdout=log, stderr=log, check=True, timeout=120)


def rust_frame_counterfactual(directory):
    target, _ = rust_fixture(directory, "rust-frame-queue")
    mutant = target / "src/frames.rs"
    original = mutant.read_text()
    changed = original.replace("output.append(&plan.bytes)?;", "let queued = output.append(&plan.bytes);")
    changed = changed.replace("core(engine.clean())\n    }", "core(engine.clean())?; queued\n    }")
    if changed == original or "let queued" not in changed or "queued\n" not in changed:
        raise ValueError("Rust frame counterfactual no longer removes queue publication ordering")
    mutant.write_text(changed)
    command = ["cargo", "test", "--manifest-path", str(target / "Cargo.toml"),
               "--target-dir", str(ROOT / "build/cargo"), "--offline", "--quiet",
               "queue_rejection_preserves_damage_and_the_last_delivered_baseline", "--lib"]
    rust_build(directory, "rust-frame", [*command, "--no-run"])
    return ("Rust frame queue publication", command, os.environ,
            b"frames::tests::queue_rejection_preserves_damage_and_the_last_delivered_baseline --- FAILED")


def rust_viewport_counterfactual(directory):
    target, _ = rust_fixture(directory, "rust-viewport-publication", "frontend", tests=True)
    mutant = target / "src/viewport.rs"
    source = mutant.read_text()
    operation = "self.pending = Some(Pending {"
    if source.count(operation) != 1:
        raise ValueError("Rust viewport counterfactual no longer removes frame publication ordering")
    mutant.write_text(source.replace(operation, "self.frame.columns = columns as i32;\n                " + operation))
    test = "frames_publish_only_on_finish_and_malformed_or_interrupted_deltas_preserve_the_visible_frame"
    command = ["cargo", "test", "--manifest-path", str(target / "Cargo.toml"),
               "--target-dir", str(ROOT / "build/cargo"), "--offline", "--quiet", "--test", "viewport", test]
    rust_build(directory, "rust-viewport", [*command, "--no-run"])
    return "Rust viewport publication", command, os.environ, test.encode()


def rust_activation_counterfactual(directory):
    target, package = rust_fixture(directory, "rust-activation", "frontend")
    mutant = target / "src/graphics_ffi.rs"
    source = mutant.read_text()
    operation = "if window.display().type_().is_a(x11) {"
    if source.count(operation) != 1:
        raise ValueError("Rust activation counterfactual no longer removes fresh X11 timestamps")
    mutant.write_text(source.replace(operation, "if false {"))
    rust_build(directory, "rust-activation", ["cargo", "build", "--manifest-path", str(target / "Cargo.toml"),
               "--target-dir", str(ROOT / "build/cargo"), "--offline", "--quiet", "--release", "--bin", package])
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests",
               "-p", "test_window_activation.py", "-q"]
    env = {**os.environ, "WORMINAL_FRONTEND": str(ROOT / "build/cargo/release" / package)}
    return "Rust activation timestamp", command, env, b"Explicit launch did not refresh its server timestamp"


def rust_handoff_counterfactual(directory, name, filename, before, after, test, pattern="test_handoff.py"):
    slug = name.lower().replace(" ", "-")
    target, package = rust_fixture(directory, slug)
    mutant = target / "src" / filename
    original = mutant.read_text()
    if original.count(before) != 1:
        raise ValueError(f"Rust handoff counterfactual no longer removes its ownership invariant: {name}")
    mutant.write_text(original.replace(before, after))
    rust_build(directory, slug, ["cargo", "build", "--manifest-path", str(target / "Cargo.toml"),
               "--target-dir", str(ROOT / "build/cargo"), "--offline", "--quiet"])
    binary = ROOT / "build/cargo/debug" / package
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", pattern,
               "-k", test, "-q"]
    return (name, command, {**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(binary),
                           "WORMINAL_TEST_RUST_DAEMON": "1"}, test.encode())


def prove(match=None):
    from tools.reference import module
    historical = module()
    directory = ROOT / ".checks/regressions"
    directory.mkdir(parents=True, exist_ok=True)
    builder = historical.Builder()
    service_support = [f"src/{source}" for source in build.SERVER if source != "workspace/workspace_service.c"]
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
        "if (tab->input_frame || (tab->controller && !tab->screen.len)) {", "if (0) {").replace(
        "if (tab->pending_frame) flush_frame(tab);", "; /* Missing pre-catalog frame flush. */"))
    ordered_daemon = directory / "unordered-preview"
    builder.program(ordered_daemon, [*service_support, str(ordering)],
                    variant="ordering-proof", flags=["-DWORMINAL_PREVIEW_PROTOCOL"])
    command = [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_preview_workspace.py",
               "-k", "input_reply_frame", "-q"]
    env = {**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(ordered_daemon)}
    cases.append(("input reply/mode ordering", command, env, b"mode catalog preceded its corresponding frame"))
    # A parser-backed replica must fail the no-local-history import invariant.
    view_header = directory / "parser-view.h"
    view_header.write_text((ROOT / "tests/session_view_checks.h").read_text().replace(
        "view = tsessionviewnew(8, 3)", "view = tsessionnew(8, 3)"))
    view_test = directory / "parser-view.c"
    view_test.write_text((ROOT / "tests/scrollback.c").read_text().replace(
        "../src/terminal/st.c", str(ROOT / "src/terminal/st.c")).replace(
        '"session_view_checks.h"', f'"{view_header}"'))
    view_binary = directory / "parser-view"
    session_support = ["src/terminal/defaults.c", *(f"src/{source}" for source in build.SESSION_SUPPORT)]
    builder.program(view_binary, [str(view_test), *session_support],
                    variant="parser-view-proof", flags=["-Itests", "-ffunction-sections", "-fdata-sections"],
                    libs=["-Wl,--gc-sections", "-lm"])
    cases.append(("replica does not reflow", [str(view_binary)], os.environ,
                  b"view->engine.term.histlen == 0 && view->engine.term.hist == NULL"))
    crop = directory / "uncropped-view.c"
    crop.write_text((ROOT / "src/terminal/session_view.c").read_text().replace(
        "if (term->line[y][cols - 1].mode & ATTR_WIDE) term->line[y][cols - 1] = blank;", ""))
    crop_binary = directory / "uncropped-view"
    support = [str(crop) if source.endswith("session_view.c") else source for source in session_support]
    builder.program(crop_binary, ["tests/scrollback.c", *support], variant="crop-view-proof",
                    flags=["-Itests", "-ffunction-sections", "-fdata-sections"], libs=["-Wl,--gc-sections", "-lm"])
    cases.append(("replica crops wide cells", [str(crop_binary)], os.environ,
                  b"view->engine.term.line[1][2].u == ' '"))
    current_parser = (ROOT / "src/terminal/st.c").read_text()

    def parser_case(name, source, expected, path="src/terminal/st.c"):
        if source == (ROOT / path).read_text():
            raise ValueError(f"Parser counterfactual no longer removes a fix: {name}")
        parser = directory / f"{name}-st.c"
        parser.write_text(source)
        test = directory / f"{name}-test.c"
        support = session_support
        if path.endswith(".inc"):
            unit = directory / f"{name}-parser.c"
            unit.write_text((ROOT / "src/terminal/engine_parser.c").read_text().replace(
                f'"{Path(path).name}"', f'"{parser}"'))
            support = [str(unit) if item.endswith("engine_parser.c") else item for item in session_support]
            test_source = (ROOT / "tests/scrollback.c").read_text().replace(
                '../src/terminal/st.c', str(ROOT / "src/terminal/st.c"))
        else:
            test_source = (ROOT / "tests/scrollback.c").read_text().replace('../src/terminal/st.c', str(parser))
        test.write_text(test_source)
        binary = directory / name
        builder.program(binary, [str(test), *support],
                        variant=f"{name}-proof",
                        flags=["-Itests", "-ffunction-sections", "-fdata-sections"],
                        libs=["-Wl,--gc-sections", "-lm"])
        cases.append((name, [str(binary)], os.environ, expected))

    parser_case("read-capacity", current_parser.replace("sizeof(session->ttybuf)-buflen", "LEN(buf)-buflen"),
                b"ttyread() == strlen(batch)")
    reflow = (ROOT / "src/terminal/engine_reflow.inc").read_text()
    stale_cursor = reflow.replace("if (cursor.state & CURSOR_WRAPNEXT) cursor.x = oldcol;", "")
    stale_cursor = stale_cursor.replace("if (saved.state & CURSOR_WRAPNEXT) saved.x = oldcol;", "")
    stale_cursor = stale_cursor.replace("MODBIT(term.c.state, cursorend, CURSOR_WRAPNEXT);",
        "MODBIT(term.c.state, cursorend || (cursor.state & CURSOR_WRAPNEXT && term.c.x == col - 1), "
        "CURSOR_WRAPNEXT);")
    stale_cursor = stale_cursor.replace("MODBIT(session->saved[0].state, savedend, CURSOR_WRAPNEXT);",
        "MODBIT(session->saved[0].state, savedend || (saved.state & CURSOR_WRAPNEXT && savedx == col - 1), "
        "CURSOR_WRAPNEXT);")
    parser_case("pending-wrap", stale_cursor, b"term.line[3 / width][3 % width].u == 'd'",
                "src/terminal/engine_reflow.inc")

    def engine_case(name, path, original, replacement, expected, *, sanitize=False):
        source = (ROOT / path).read_text()
        mutant = source.replace(original, replacement)
        if mutant == source:
            raise ValueError(f"Engine counterfactual no longer removes a fix: {name}")
        stem = name.replace(" ", "-")
        unit = directory / f"{stem}.c"
        unit.write_text(mutant)
        if path.endswith(".inc"):
            include = unit
            unit = directory / f"{stem}-parser.c"
            unit.write_text((ROOT / "src/terminal/engine_parser.c").read_text().replace(
                f'"{Path(path).name}"', f'"{include}"'))
            path = "src/terminal/engine_parser.c"
        sources = [str(unit) if f"src/{item}" == path else f"src/{item}" for item in build.ENGINE]
        binary = ROOT / "build/regressions" / stem
        flags = ["-O1", "-g", "-fsanitize=address,undefined"] if sanitize else []
        builder.program(binary, ["tests/engine.c", *sources], variant=f"{stem}-proof",
                        flags=flags, libs=["-lm", *flags])
        cases.append((name, [str(binary)], os.environ, expected))

    engine_case("engine effect ordering", "src/terminal/engine.c",
                "if (engine->events) return WORMINAL_PENDING;", "",
                b"WORMINAL_PENDING")
    engine_case("engine redraw damage retention", "src/terminal/engine.c",
                "if (engine->render_sync_deadline) return WORMINAL_PENDING;", "",
                b"worminal_engine_clean(engine) == WORMINAL_PENDING")
    engine_case("engine pending CSI validation", "src/terminal/engine_snapshot.c",
                "(h.esc & ESC_CSI) && h.csi.len >= ESC_BUF_SIZ - 1", "0",
                b"worminal_engine_import(data, bytes, &copy, &imported) == WORMINAL_INVALID")
    engine_case("engine selection validation", "src/terminal/engine_snapshot.c",
                " || !valid_selection(&h)", "",
                b"worminal_engine_import(data, bytes, &copy, &imported) == WORMINAL_INVALID")
    engine_case("engine trimmed selection bounds", "src/terminal/engine_selection.inc",
                "last < 0 || ", "", b"AddressSanitizer:", sanitize=True)
    engine_case("engine borrowed selection ownership", "src/terminal/engine_selection.inc",
                "if (!session->selection_only) te_tsetdirt", "te_tsetdirt", b"AddressSanitizer:", sanitize=True)
    # Restore fork/exec at the SSH launch boundary: registered GUI hooks must be detected.
    client = directory / "fork-client.c"
    source = (ROOT / "src/workspace/workspace_client.c").read_text()
    unsafe = source.replace(
        'if (!error) error = posix_spawnp(&child, "ssh", &actions, NULL, args, environ);',
        '''if (!error) {
            child = fork();
            if (child < 0) error = errno;
            if (child == 0) {
                dup2(sockets[1], 0); dup2(sockets[1], 1);
                close(sockets[0]); close(sockets[1]);
                execvp("ssh", args); _exit(127);
            }
        }''')
    if unsafe == source:
        raise ValueError("SSH counterfactual no longer removes the spawn fix")
    client.write_text(unsafe)
    binary = directory / "fork-client"
    builder.program(binary, ["tests/workspace_client.c", str(client)],
                    variant="fork-client-proof", libs=["-pthread"])
    cases.append(("SSH bypasses GUI fork hooks", [str(binary)], os.environ, b"fork_hooks == 0"))
    # Without the executable watch, an atomic install must leave the old daemon running.
    upgrade_source = directory / "no-upgrade.c"
    upgrade_source.write_text((ROOT / "src/workspace/workspace_service.c").read_text().replace(
        "int upgrade_watch = workspace_upgrade_watch();", "int upgrade_watch = -1;"))
    upgrade_binary = directory / "no-upgrade-daemon"
    builder.program(upgrade_binary, [*service_support, str(upgrade_source)],
                    variant="upgrade-proof", flags=["-DWORMINAL_PREVIEW_PROTOCOL"])
    cases.append(("automatic daemon upgrade",
                  [sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", "test_upgrade.py",
                   "-k", "empty_workspace", "-q"],
                  {**os.environ, "WORMINAL_TEST_UPGRADE_CANDIDATE": str(upgrade_binary)}, b"TimeoutExpired"))
    # The old one-module cache cannot give new tabs updated code.
    cached_loader = directory / "cached-browser-loader.c"
    cached_loader.write_text((ROOT / "src/gtk/browser_loader.c").read_text().replace(
        "load_generation(App *app)\n{", "load_generation(App *app)\n{\n\tif (latest) return latest;"))
    cached_check = directory / "cached-browser-generations.c"
    cached_check.write_text((ROOT / "tests/browser_generations.c").read_text().replace(
        "../src/gtk/browser_loader.c", str(cached_loader)))
    cached_binary = directory / "cached-browser-generations"
    browser_packages = (*build.PREVIEW_PACKAGES, "gmodule-2.0")
    builder.program(cached_binary, [str(cached_check)], variant="browser-generation-proof",
                    flags=["-Itests", "-Isrc/gtk", *build.pkg_flags("--cflags", browser_packages)],
                    libs=build.pkg_flags("--libs", browser_packages))
    cases.append(("browser module generations",
                  [sys.executable, "-m", "unittest", "discover", "-s", "tests",
                   "-p", "test_browser_generations.py", "-q"],
                  {**os.environ, "WORMINAL_TEST_BROWSER_GENERATIONS_EXECUTABLE": str(cached_binary)},
                  b"updated != original"))
    if match is None or match.casefold() in "Rust frame queue publication".casefold():
        cases.append(rust_frame_counterfactual(directory))
    if match is None or match.casefold() in "Rust viewport publication".casefold():
        cases.append(rust_viewport_counterfactual(directory))
    if match is None or match.casefold() in "Rust activation timestamp".casefold():
        cases.append(rust_activation_counterfactual(directory))
    ownership_cases = (
        ("Rust commit ownership", "handoff.rs", "let commit = channel.commit()?;",
         "let commit = channel.commit()?; channel.acknowledgement()?;",
         "test_commit_delivery_disowns_even_when_receiver_acknowledgement_is_lost"),
        ("Rust staged ownership", "process.rs",
         "let flags = checked(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) })?;",
         "process.signal(libc::SIGHUP)?;\n        " +
         "let flags = checked(unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) })?;",
         "test_rejected_descriptor_layout_never_signals_staged_shells"),
        ("Rust receiver isolation", "handoff.rs", "process::ensure_receiver_session()?;",
         "/* Missing receiver session isolation. */", "test_live_round_trips_merge_without_replacing_shells"),
    )
    for name, filename, before, after, test in ownership_cases:
        if match is None or match.casefold() in name.casefold():
            cases.append(rust_handoff_counterfactual(directory, name, filename, before, after, test))
    upgrade_cases = (
        ("Rust automatic upgrade watch", "service.rs", "if self.try_upgrade() {", "if false {",
         "test_empty_workspace_replaces_without_resurrecting_tabs"),
        ("Rust donor checkpoint ownership", "service.rs", "if self.try_upgrade() {",
         "if self.try_upgrade() { self.save();", "test_replacement_preserves_gapped_ids_browser_and_live_shells"),
        ("Rust legacy donor pause", "handoff.rs", "retiring.signal(libc::SIGSTOP)", "retiring.signal(0)",
         "test_bootstrap_legacy_handoff_preserves_ids_and_enables_checkpoints"),
        ("Rust receiver cancellation", "handoff.rs", "if signals.shutdown_requested()? {", "if false {",
         "test_stopping_receiver_rejects_before_ready_and_preserves_donor"),
        ("Rust receiver setup before commit", "reactor.rs", "if fd < 0 {",
         "let fd = if fd < 0 { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) } else { fd };\n"
         "            if fd < 0 {", "test_receiver_setup_failure_retains_donor_before_commit"),
    )
    for name, filename, before, after, test in upgrade_cases:
        if match is None or match.casefold() in name.casefold():
            cases.append(rust_handoff_counterfactual(directory, name, filename, before, after, test, "test_upgrade.py"))
    selected = [case for case in cases if match is None or match.casefold() in case[0].casefold()]
    if not selected:
        raise ValueError(f"No regression proof matches: {match}")
    for index, (name, command, env, expected) in enumerate(selected):
        with (directory / f"failure-{index}.log").open("wb") as log:
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=log, timeout=15)
        log_data = (directory / f"failure-{index}.log").read_bytes()
        if result.returncode == 0 or expected not in log_data:
            raise ValueError(f"Regression did not detect the removed fix: {name}")
        print(f"Regression proves fix: {name} (counterfactual fails; logs in .checks/regressions)")
