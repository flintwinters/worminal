#!/usr/bin/env python3
"""Build, check, measure, and prove Worminal: run `python3 manage.py --help`."""

from contextlib import chdir
import os
import json
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
os.environ["PYTHONDONTWRITEBYTECODE"] = "1"

try:
    import click
    import typer
except ModuleNotFoundError as error:
    raise SystemExit("Install Typer to use manage.py: python3 -m pip install typer") from error

from tools import timing
from tools.paths import CHECKS, runtime_binary
from tools import build as native_build
from tools.theme import explorer_font
from tools.git_index import tracked_files


ROOT = Path(__file__).resolve().parent
app = typer.Typer(no_args_is_help=True, help="Build and test Worminal.")
MAX_FILE_LINES = 1000
MAX_LINE_LENGTH = 120
# Checks own private virtual displays; none may draw on or inject into the developer's desktop.
DESKTOP_VARIABLES = ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY")


@app.callback()
def root():
    os.chdir(ROOT)
    (CHECKS).mkdir(parents=True, exist_ok=True)
    for name in DESKTOP_VARIABLES:
        os.environ.pop(name, None)


def run(args, *, env=None):
    # Maintenance checks must never read or overwrite the user's recovery recipes.
    private_env = {**(os.environ if env is None else env),
                   "XDG_STATE_HOME": str(CHECKS / "state")}
    with timing.span(" ".join(str(part) for part in args)[:120].replace(sys.executable, "python"), "run"):
        return subprocess.run(args, env=private_env).returncode


def check_file_limits():
    index = subprocess.check_output(["git", "ls-files", "--stage", "-z"], cwd=ROOT)
    problems = []
    for name in tracked_files(index):
        # Dependency lockfiles are generated data; their size follows the resolved graph, not source complexity.
        if name == "Cargo.lock":
            continue
        path = ROOT / name
        if not path.is_file() or path.is_symlink():
            continue  # Skip deleted files and temporary links created by tests.
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except UnicodeDecodeError:
            continue
        limit = MAX_FILE_LINES
        if len(lines) > limit:
            problems.append(f"{path.relative_to(ROOT)}: {len(lines)} lines (limit {limit})")
        for number, line in enumerate(lines, 1):
            if len(line) > MAX_LINE_LENGTH:
                problems.append(f"{path.relative_to(ROOT)}:{number}: {len(line)} characters (limit {MAX_LINE_LENGTH})")
    if problems:
        for problem in problems[:20]:
            typer.echo(problem, err=True)
        if len(problems) > 20:
            typer.echo(f"... and {len(problems) - 20} more", err=True)
        raise typer.Exit(1)


def run_checks(match=None, jobs=None):
    native_build.build_targets("worminal-rust", "worminald", "build/gui-peer.so")
    code = run_native_checks()
    if code:
        return code
    from tools.test_runner import run_python
    code = run_python(jobs=jobs, match=match)
    if code or match:
        return code
    return run([sys.executable, "tests/rust_app.py"]) or run([sys.executable, "tests/rust_browser.py"])


RUST_FILE_LINES = 1000


def run_rust_lint():
    if run(["cargo", "fmt", "--all", "--", "--check"]):
        print("Run `cargo fmt --all` to apply rustfmt (100-column lines).")
        return 1
    tracked = subprocess.check_output(["git", "ls-files", "-z", "*.rs"], cwd=ROOT).split(b"\0")
    long = [(path.decode(), lines) for path in tracked if path and (ROOT / path.decode()).is_file()
            and (lines := (ROOT / path.decode()).read_bytes().count(b"\n")) > RUST_FILE_LINES]
    for path, lines in long:
        print(f"{path}: {lines} lines; split files above {RUST_FILE_LINES} lines by responsibility.")
    if long:
        return 1
    # engine-sys links the C engine archive, so it must exist before Rust code is checked.
    native_build.Builder().target("build/libworminal_engine.a")
    return run(["cargo", "clippy", "--locked", "--quiet", "--workspace", "--all-targets", "--", "-D", "warnings"])


@app.command("check-rust-lint")
def check_rust_lint():
    """Check rustfmt formatting, Rust file length and Clippy (warnings fail) for every crate, test and build script."""
    raise typer.Exit(run_rust_lint())


def run_native_checks():
    from tools.engine_checks import check
    code = check()
    return (code or run_rust_lint() or run_rust_workspace_checks() or run_rust_process_checks()
            or run_rust_frontend_tests())


def run_headless_checks():
    native_build.build_targets("worminald")
    code = run_native_checks()
    if code:
        return code
    patterns = ("test_build.py", "test_perf_metrics.py", "test_cli.py", "test_workspace.py",
                "test_preview_workspace.py", "test_agent_api.py", "test_remote_install.py", "test_remote_upgrade.py", "test_theme.py", "test_config_sync.py", "test_running.py", "test_update_flow.py",
                "test_handoff.py", "test_restore.py", "test_upgrade.py")
    for pattern in patterns:
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                    "-p", pattern, "-q"])
        if code:
            return code
    return 0


@app.command()
def build():
    """Compile Worminal; appearance is read at runtime from worminal.yaml or Alacritty config."""
    native_build.build_targets()


@app.command("check-engine")
def check_engine():
    """Check sanitized engine boundaries and the Rust owning wrapper."""
    from tools.engine_checks import check
    raise typer.Exit(check())


def run_rust_workspace_checks():
    return run(["cargo", "test", "--locked", "--quiet", "-p", "worminal-protocol", "-p", "worminal-workspace"])


@app.command("check-rust-workspace")
def check_rust_workspace():
    """Check Rust framing, launch requests, recovery recipes and local transport."""
    raise typer.Exit(run_rust_workspace_checks())


@app.command("daemon-perf")
def daemon_perf(label: str = typer.Option(..., help="Filename label for retained measurements."),
                trials: int = typer.Option(5, min=1),
                compare: str = typer.Option(None, help="Git revision with daemon-perf for alternating trials.")):
    """Measure release daemon queue, catalog and frame CPU costs without a display."""
    from tools.daemon_perf import measure
    measure(label, trials, compare)


@app.command("network-perf")
def network_perf(label: str = typer.Option(..., help="Filename label for retained measurements."),
                 trials: int = typer.Option(3, min=1),
                 compare: str = typer.Option(None, help="Git baseline for identical service transport workloads.")):
    """Measure sparse traffic and modeled congested frame age; excludes SSH, RTT and presentation."""
    from tools.daemon_perf import measure
    measure(label, trials, compare, network=True)


@app.command("check-rust-process")
def check_rust_process():
    """Check headless Rust PTYs, child identities and real zsh restoration on project-local fixtures."""
    raise typer.Exit(run_rust_process_checks())


def run_rust_process_checks():
    native_build.Builder().target("build/libworminal_engine.a")
    return run(["cargo", "test", "--locked", "--quiet", "-p", "worminal-daemon"])


def build_rust_daemon():
    return native_build.Builder().rust_artifact("worminal-daemon")


def run_rust_frontend_tests():
    return run(["cargo", "test", "--locked", "--quiet", "-p", "worminal-frontend", "--lib", "--tests"])


@app.command("check-rust-frontend")
def check_rust_frontend():
    """Check imported Rust viewports, frame publication and frontend state."""
    native_build.Builder().target("build/libworminal_engine.a")
    raise typer.Exit(run_rust_frontend_tests())


@app.command("build-rust-app")
def build_rust_app():
    """Build the native Rust terminal/browser application with its lazy frontend and browser modules."""
    native_build.build_targets("worminal-rust")
    typer.echo("Built build/bin/worminal-rust; native terminal/SSH/browser. Installation unchanged.")


@app.command("check-test-runtime")
def check_test_runtime(prove: bool = typer.Option(False, help="Also require failure with owner cleanup removed.")):
    """Check private display/service shutdown and copied-runtime cleanup, including exception paths."""
    native_build.build_targets("worminald")
    code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                "-p", "test_runtime.py", "-q"])
    if not code and prove:
        code = run([sys.executable, "tests/test_runtime.py", "--prove"])
    raise typer.Exit(code)


@app.command("check-rust-app")
def check_rust_app(module_only: bool = typer.Option(
        False, help="Run only launcher/module and GUI broker checks on private displays.")):
    """Check the actual Rust app: private shell input, clipboard, tabs and surviving-session reattachment."""
    native_build.build_targets("worminal", "worminal-rust", "build/gui-peer.so")
    arguments = (["-m", "unittest", "discover", "-s", "tests", "-p", "rust_module.py", "-q"]
                 if module_only else ["tests/rust_app.py"])
    raise typer.Exit(run([sys.executable, *arguments]))


@app.command("check-notifications")
def check_notifications():
    """Check OSC 99 delivery and click activation on a private session bus and X11 desktop."""
    native_build.build_targets("worminal-rust", "worminald")
    raise typer.Exit(run([sys.executable, "tests/test_notifications.py", "-q"]))


@app.command("check-rust-browser")
def check_rust_browser():
    """Check real Rust browser views, lazy loading, owner forwarding and local-document security privately."""
    native_build.build_targets("worminal-rust")
    raise typer.Exit(run([sys.executable, "tests/rust_browser.py"]))


@app.command("build-rust-daemon")
def build_rust_daemon_command():
    """Build the Rust service artifact without publishing or replacing installed daemons."""
    typer.echo(build_rust_daemon())


@app.command("check-agent-api")
def check_agent_api():
    """Check headless agent discovery, screen reads and keystrokes on a private workspace."""
    native_build.Builder().target("build/libworminal_engine.a")
    code = run(["cargo", "test", "--locked", "--quiet", "-p", "worminal-protocol",
                "-p", "worminal-daemon", "-p", "worminal-engine", "--lib"])
    if not code:
        code = run(["cargo", "test", "--locked", "--quiet", "-p", "worminal-workspace", "--test", "devices"])
    if not code:
        # Registry publication shares the recovery writer; these existing boundary checks need no C oracle.
        code = run(["cargo", "test", "--locked", "--quiet", "-p", "worminal-workspace",
                    "--test", "state", "invalid_"])
    if not code:
        code = run(["cargo", "test", "--locked", "--quiet", "-p", "worminal-engine", "--test", "selection"])
    if not code:
        candidate = build_rust_daemon()
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                    "-p", "test_agent_api.py", "-q"],
                   env={**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(candidate)})
    raise typer.Exit(code)


@app.command("check-remote-install")
def check_remote_install():
    """Check native remote-install bootstrap against private mock-SSH targets, without root or live hosts."""
    candidate = build_rust_daemon()
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_remote_install.py", "-q"],
                        env={**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(candidate)}))


@app.command("check-remote-upgrade")
def check_remote_upgrade():
    """Check private mock-SSH upgrades, publication rollback and live daemon session preservation."""
    candidate = build_rust_daemon()
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_remote_upgrade.py", "-q"],
                        env={**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(candidate)}))


@app.command("check-rust-daemon")
def check_rust_daemon():
    """Check the Rust service with Python clients, ownership, output and recovery fixtures."""
    native_build.build_targets("worminald")
    candidate = build_rust_daemon()
    code = run_rust_process_checks()
    for pattern in ("test_preview_workspace.py", "test_restore.py", "test_workspace.py"):
        if code:
            break
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", pattern, "-q"],
                   env={**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(candidate),
                        "WORMINAL_TEST_WORKSPACE_DAEMON": str(candidate),
                        "WORMINAL_TEST_BRIDGE_VERSION": "2"})
    raise typer.Exit(code)


@app.command("inventory")
def inventory():
    """Record installed hashes, live owners, windows and PTYs without copies or process changes."""
    from tools.migration import inventory
    inventory()


@app.command("running")
def running(prefix: Path = typer.Option("/usr/local", help="Installation prefix whose live processes to classify.")):
    """Report which live Worminal processes run the installed files; v1 daemons are listed, never changed."""
    from tools import running as live
    live.report(str(prefix))


@app.command("media-fixtures")
def media_fixtures():
    """Regenerate silent browser media fixtures (requires ffmpeg)."""
    raise typer.Exit(run([sys.executable, "tests/media_fixtures.py"]))


@app.command("setup-wayland")
def setup_wayland():
    """Download/extract a project-local Debian Weston runtime; no system installation."""
    from tools.weston import setup
    setup()


@app.command("setup-audio")
def setup_audio():
    """Prepare private browser-test audio without installing system packages; headless hosts skip it."""
    from tools.audio import setup
    setup()


@app.command("setup-check-runtime")
def setup_check_runtime():
    """Prepare everything the full suite needs: explorer build, project-local audio and Weston; idempotent."""
    if native_build.headless():
        return
    from tools import explorer
    explorer.prepare()
    from tools.audio import setup as setup_audio
    from tools import weston
    setup_audio()
    try:
        weston.command(os.environ.copy())
    except ValueError:
        weston.setup()


@app.command("prove-regressions")
def prove_regressions(match: str = typer.Option(None, help="Only run matching regression proofs.")):
    """Rebuild with fixes removed and check that the tests fail."""
    from tools.regressions import prove
    prove(match)


@app.command()
def check(match: str = typer.Option(None, help="Run matching Python checks; full suite is the default."),
          jobs: int = typer.Option(None, min=1, show_default="available CPUs // 2 + 1",
                                   help="Independent Python workers; --jobs 1 runs serially.")):
    """Run the local native and private-display regression suite."""
    raise typer.Exit(run_checks(match, jobs))


@app.command("check-python")
def check_python(jobs: int = typer.Option(None, min=1, show_default="available CPUs // 2 + 1"),
                 match: str = typer.Option(None),
                 module: list[str] = typer.Option(None, help="Top-level module; repeat to select several.")):
    """Run Python checks against existing build artifacts; use check to build prerequisites first."""
    from tools.test_runner import run_python
    raise typer.Exit(run_python(jobs=jobs, match=match, modules=module or ()))


@app.command("check-keys")
def check_keys():
    """Check keys, mouse input, and clipboard bytes on private Xvfb displays."""
    native_build.build_targets()
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_keys.py", "-q"]))


@app.command("check-upgrade")
def check_upgrade():
    """Verify automatic replacements, rollback, legacy bootstrap, and live tab identities privately."""
    native_build.build_targets("worminald")
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_upgrade.py", "-q"]))


@app.command("upgrade")
def upgrade_device(target: str = typer.Argument(..., help="Existing device's SSH host or user@host."),
                   source_prefix: Path = typer.Option(Path("/usr/local"), help="Current installed source prefix.")):
    """Copy this device's installed Worminal release to an existing compatible device over SSH."""
    from tools.remote_upgrade import upgrade
    try:
        receipt = upgrade(target, source_prefix)
    except (OSError, ValueError, RuntimeError) as error:
        typer.echo(str(error), err=True)
        raise typer.Exit(1) from error
    typer.echo(json.dumps(receipt))
    if not receipt["active"]:
        raise typer.Exit(1)


@app.command("upgrade-daemon")
def upgrade_daemon(executable: Path = typer.Option(
        Path("/usr/local/bin/worminald"), exists=True, dir_okay=False, help="Installed release daemon.")):
    """Activate the installed v2 daemon while preserving a pre-automatic daemon's live tabs."""
    from tools.daemon_upgrade import upgrade
    upgrade(executable)


@app.command("check-handoff")
def check_handoff():
    """Check native state snapshots and live transfers between current daemons."""
    native_build.build_targets("worminald")
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_handoff.py", "-q"]))


@app.command("check-restore")
def check_restore():
    """Check tab/directory recovery across daemon shutdowns and crashes, including real zsh startup."""
    native_build.build_targets("worminald")
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_restore.py", "-q"]))


@app.command("check-headless")
def check_headless():
    """Run native and service checks that need no X11 display."""
    raise typer.Exit(run_headless_checks())


@app.command("check-bridge")
def check_bridge():
    """Verify input and output stay independent under SSH bridge congestion on v2."""
    native_build.build_targets("worminald")
    for version, daemon in (("2", "worminald"),):
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                    "-p", "test_workspace.py", "-k", "bridge_congestion", "-q"],
                   env={**os.environ, "WORMINAL_TEST_WORKSPACE_DAEMON": daemon,
                        "WORMINAL_TEST_BRIDGE_VERSION": version})
        if code:
            raise typer.Exit(code)


@app.command("check-master-startup")
def check_master_startup(
    executable: Path = typer.Option(None, help="Executable to check; defaults to project build."),
    master: str = typer.Option(None, help="Use real SSH with a private remote workspace."),
):
    """Check native --master startup and input on private Xvfb, using an isolated workspace."""
    native_build.build_targets()
    env = dict(os.environ)
    if executable:
        env["WORMINAL_TEST_EXECUTABLE"] = str(executable.resolve())
    if master:
        env["WORMINAL_TEST_REAL_MASTER"] = master
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_master_startup.py", "-q"], env=env))


@app.command("check-network")
def check_network(daemon: Path = typer.Option(None, help="Compatible v2 daemon binary to check a baseline.")):
    """Check redundant catalogs/rows, stationary scrolling, and complete-frame flushing."""
    if daemon is None:
        native_build.build_targets("worminald")
        daemon = runtime_binary("worminald")
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_preview_workspace.py", "-k", "redundant_catalog",
                         "-k", "frame_transport", "-k", "redraw_rows", "-k", "scroll_at_boundary",
                         "-k", "scrolling_", "-k", "synchronized_redraw", "-k", "input_without_local_echo",
                         "-k", "partially_sent_snapshot", "-k", "row_spans", "-q"],
                        env={**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(daemon.resolve())}))


@app.command("check-window-state")
def check_window_state():
    """Check the public session-bus directory interface on a private bus and display."""
    native_build.build_targets()
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_window_state.py", "-v"]))


@app.command("check-window-activation")
def check_window_activation():
    """Check new-window focus under Cinnamon's Muffin on a private display."""
    native_build.build_targets()
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_window_activation.py", "-v"]))


@app.command("setup-window-activation")
def setup_window_activation():
    """Prepare a project-local Muffin runtime for private activation checks, without system installation."""
    from tools.muffin import setup
    setup()


@app.command()
def lint():
    """Check file limits and run Clang static analysis (requires clang-tidy)."""
    check_file_limits()
    native_build.analyze()


@app.command()
def clean():
    """Remove generated binaries and headers."""
    native_build.clean()


@app.command("check-build")
def check_build():
    """Check build freshness, installation, runtime configuration and cleanup."""
    for pattern in ("test_build.py", "test_theme.py"):
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p", pattern, "-q"])
        if code:
            raise typer.Exit(code)


@app.command("check-install")
def check_install(installed: bool = typer.Option(False, help='Verify published /usr/local artifacts without writing.'),
                  skip_build: bool = typer.Option(False, help='Verify existing artifacts, including transferred builds.')):
    """Stage /usr/local privately; verify hashes, native SSH input, browser and loaded module paths."""
    if not skip_build:
        native_build.build_targets()
    raise typer.Exit(run([sys.executable, "tests/installation.py", *(['--installed'] if installed else [])]))


def explorer_command(command):
    script = ROOT / "file-explorer/manage.py"
    if not script.exists():
        typer.echo("Initialize the file-explorer submodule first: git submodule update --init", err=True)
        raise typer.Exit(1)
    raise typer.Exit(run([sys.executable, str(script), command],
                         env={**os.environ, "WORMINAL_PALETTE": str(ROOT / "assets/palette.json"),
                              "WORMINAL_FONT_JSON": json.dumps(explorer_font())}))


@app.command("check-browser-target")
def check_browser_target():
    """Check browser target detection, path encoding and terminal conversion privately."""
    check_file_limits()
    native_build.build_targets("worminal", "worminald")
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_preview_workspace.py", "-k", "browser_command", "-q"]))


@app.command("explorer-build")
def explorer_build():
    """Build file-explorer/dist/index.html (run npm ci in the submodule first)."""
    explorer_command("build")


@app.command("explorer-check")
def explorer_check():
    """Check explorer types, directory behavior, DOM interactions, and standalone output."""
    check_file_limits()
    explorer_command("check")


@app.command("explorer-lint")
def explorer_lint():
    """Run the explorer's pinned Hyperlinter architecture and style audit."""
    explorer_command("lint")


@app.command()
def install(prefix: Path = typer.Option(None, help="Installation prefix (default: PREFIX or ~/.local)."),
            skip_build: bool = typer.Option(False, help="Publish an existing user build without rebuilding.")):
    """Build and install binaries, manual, desktop launcher, and icon; supports DESTDIR staging."""
    native_build.install(prefix, skip_build=skip_build)


@app.command("auto-update")
def auto_update(timer: str = typer.Option(None, click_type=click.Choice(["install", "remove"]),
                                          help="Manage the daily 03:00 systemd user timer instead of updating.")):
    """Update /usr/local from the tip of main if it changed and passes build and check."""
    from tools import auto_update as updates
    raise typer.Exit(updates.update() if timer is None else updates.schedule(timer == "remove") or 0)


@app.command("update")
def update_checkout(prefix: str = typer.Option("/usr/local", help="Installation prefix."),
                    check: bool = typer.Option(False, help="Also run the full suite before installing.")):
    """Build, install this checkout to /usr/local, and verify installed files; run check separately."""
    from tools.auto_update import publish
    raise typer.Exit(publish(prefix=prefix, checked=check))


@app.command()
def uninstall(prefix: Path = typer.Option(None, help="Prefix used for installation.")):
    """Remove installed files; supports PREFIX, MANPREFIX, and DESTDIR."""
    native_build.uninstall(prefix)




@app.command("latency-visible")
def latency_visible(trials: int = typer.Option(10, min=1), keys: int = typer.Option(50, min=1),
                    trace_startup: bool = typer.Option(False, help="Include diagnostic startup milestones."),
                    shell: bool = typer.Option(False, help="Include interactive shell initialization."),
                    startup_resources: bool = typer.Option(False, help="Sample first-ready CPU and peak RSS counters."),
                    saved_tabs: int = typer.Option(0, min=0, help="Seed service-owned tabs before measurement."),
                    varied_labels: bool = typer.Option(False, help="Seed tabs in distinct project-local directories."),
                    bridge_delay_ms: int = typer.Option(None, min=0, help="Use a private delayed SSH bridge fixture."),
                    master: str = typer.Option(None, help="Measure real SSH using an isolated remote workspace."),
                    remote_echo: str = typer.Option(None, help="Path to build/latency_echo on the remote host."),
                    snapshot: Path = typer.Option(None, help="Save current binaries/modules as a comparison baseline."),
                    compare: Path = typer.Option(None, help="Alternate trials with a saved baseline directory.")):
    """Measure real key events to visible echo and fresh/additional window presentation."""
    native_build.build_targets("worminal", "worminald", "build/latency_echo")
    options = (["--snapshot", str(snapshot)] if snapshot else []) + (["--compare", str(compare)] if compare else [])
    if varied_labels:
        options += ["--varied-labels"]
    if startup_resources:
        options += ["--startup-resources"]
    if shell:
        options += ["--shell"]
    if trace_startup:
        options += ["--trace-startup"]
    if bridge_delay_ms is not None:
        options += ["--bridge-delay-ms", str(bridge_delay_ms)]
    if master:
        options += ["--real-master", master, "--remote-echo", remote_echo or ""]
    raise typer.Exit(run([sys.executable, "tests/visible_latency.py", "--trials", str(trials),
                          "--keys", str(keys), "--saved-tabs", str(saved_tabs), *options]))


@app.command("profile-startup")
def profile_startup(shell: bool = typer.Option(False, help="Include interactive shell initialization.")):
    """Trace private-display startup syscalls; diagnostic timings include tracing overhead."""
    native_build.build_targets("worminal", "worminald", "build/latency_echo")
    raise typer.Exit(run([sys.executable, "tests/startup_profile.py", *(["--shell"] if shell else [])]))


@app.command("browser-perf")
def browser_perf(trials: int = typer.Option(5, min=1),
                 snapshot: Path = typer.Option(None), compare: Path = typer.Option(None),
                 cold_open: bool = typer.Option(False, help="Measure fresh process launch directly into a browser."),
                 established_profile: bool = typer.Option(
                     False, help="Initialize profile storage before measured launches."),
                 trace_startup: bool = typer.Option(False, help="Log startup milestones; timings include log I/O."),
                 profile_cpu: bool = typer.Option(
                     False, help="Diagnostically sample startup CPU without perf privileges."),
                 kind: str = typer.Option("all", help="Target: all, http, file, or explorer.")):
    """Measure browser presentation, including direct cold launches, on private Xvfb."""
    native_build.build_targets("worminal", "worminald", "build/latency_echo")
    options = (["--snapshot", str(snapshot)] if snapshot else []) + (["--compare", str(compare)] if compare else [])
    options += ["--kind", kind, *(["--cold-open"] if cold_open else [])]
    if trace_startup:
        options += ["--trace-startup"]
    if established_profile:
        options += ["--established-profile"]
    if profile_cpu:
        native_build.build_targets("build/browser_cpu.so")
        options += ["--profile-cpu"]
    raise typer.Exit(run([sys.executable, "tests/browser_perf.py", "--trials", str(trials), *options]))


@app.command("interaction-perf")
def interaction_perf(trials: int = typer.Option(10, min=1), samples: int = typer.Option(20, min=1),
                     pace_ms: float = typer.Option(0, min=0, help="Idle time before each measured interaction."),
                     compare: Path = typer.Option(None, help="Saved latency-visible baseline directory.")):
    """Measure tab ownership, cached switching, and wheel/page scrolling to visible pixels."""
    native_build.build_targets("worminal", "worminald", "build/latency_echo")
    options = ["--compare", str(compare)] if compare else []
    raise typer.Exit(run([sys.executable, "tests/interaction_perf.py", "--trials", str(trials),
                          "--samples", str(samples), "--pace-ms", str(pace_ms), *options]))


@app.command("frame-perf")
def frame_perf(compare: Path = typer.Option(None, help="Saved latency-visible baseline directory."),
               startup: bool = typer.Option(False, help="Measure new-tab output while another tab is writing.")):
    """Measure paced PTY output to complete frames without keyboard-response boosting."""
    native_build.build_targets("worminald")
    variants = [("candidate", runtime_binary("worminald"))]
    if compare:
        daemon = runtime_binary("worminald", prefix=compare.resolve())
        if not daemon.is_file():
            raise typer.BadParameter("Comparison directory must contain worminald")
        variants.insert(0, ("baseline", daemon))
    for label, daemon in variants:
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-p",
                    "test_preview_workspace.py", "-k", "new_tab_output" if startup else "paced_output", "-q"],
                   env={**os.environ, "WORMINAL_TEST_PREVIEW_DAEMON": str(daemon),
                        "WORMINAL_FRAME_PERF_NAME": label})
        if code:
            raise typer.Exit(code)




@app.command("master-perf")
def master_perf(host: str):
    """Compare ordinary SSH with v2 input/frame latency, scrolling, and tab-switch traffic."""
    raise typer.Exit(run([sys.executable, "tests/master_perf.py", host]))


@app.command("master-traffic")
def master_traffic(host: str):
    """Measure SSH -C wire KiB/s of hidden busy tabs on HOST's installed daemon, isolated scope (~1 min)."""
    raise typer.Exit(run([sys.executable, "tests/master_traffic.py", host]))


@app.command("codex-perf")
def codex_perf(host: str, visible: bool = typer.Option(False, help="Observe real GTK pixels on private Xvfb.")):
    """Compare unsubmitted typing in real Codex through ordinary SSH and --master."""
    native_build.build_targets("worminald")
    raise typer.Exit(run([sys.executable, "tests/codex_visible_perf.py" if visible else "tests/codex_perf.py", host]))


@app.command("prove-window-movement")
def movement():
    """Check offscreen Alt-dragging under private Muffin."""
    native_build.build_targets()
    raise typer.Exit(run([sys.executable, "tests/proof_window_move.py"]))


@app.command("timings")
def timings(run: str = typer.Argument(None, help="Run file name or unique part of it; default is the latest."),
            depth: int = typer.Option(4, min=1, help="Levels of the span tree to show."),
            minimum: float = typer.Option(0.5, help="Hide spans faster than this many seconds."),
            list_runs: bool = typer.Option(False, "--list", help="List recorded runs instead."),
            compare: bool = typer.Option(False, "--compare", help="Compare with the previous run of the same command.")):
    """Show where the time of a recorded manage.py run went (every command records its own)."""
    runs = timing.runs()
    if list_runs:
        for path in runs[-30:]:
            spans, roots = timing.load(path)
            typer.echo(f"{path.name}  {max((root['seconds'] for root in roots), default=0):8.1f}s")
        return
    chosen = [path for path in runs if not run or run in path.name]
    if not chosen:
        raise typer.Exit("No recorded runs; run a manage.py command first.")
    latest = chosen[-1]
    typer.echo(timing.render(latest, depth=depth, minimum=minimum))
    if compare:
        command = latest.name.split("-", 2)[2]
        earlier = [path for path in chosen[:-1] if path.name.endswith("-" + command)]
        typer.echo("\n" + (timing.compare(latest, earlier[-1]) if earlier else "No earlier run of this command."))


if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 and not sys.argv[1].startswith("-") else "manage"
    if command != "timings":
        timing.start(command)
    try:
        with timing.span("manage.py " + " ".join(sys.argv[1:]), "command"):
            app()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        typer.echo(f"Command failed: {error}", err=True)
        raise SystemExit(1) from error
    except KeyboardInterrupt:
        raise SystemExit(130)
