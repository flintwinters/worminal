#!/usr/bin/env python3
"""Build, check, measure, and prove Worminal: run `python3 manage.py --help`."""

from contextlib import chdir
import os
import json
from pathlib import Path
import subprocess
import sys

try:
    import typer
except ModuleNotFoundError as error:
    raise SystemExit("Install Typer to use manage.py: python3 -m pip install typer") from error

from tools import build as native_build
from tools.theme import explorer_font
from tools.icon import generate_icon
from tools.privacy import check_privacy
from tools.git_index import tracked_files


ROOT = Path(__file__).resolve().parent
app = typer.Typer(no_args_is_help=True, help="Build and test Worminal.")
proof_app = typer.Typer(help="Run proofs on private Xvfb displays.")
app.add_typer(proof_app, name="proof")
MAX_FILE_LINES = 1000
MAX_LINE_LENGTH = 120
NATIVE_CHECKS = ("build/scrollback_test", "build/url_test", "build/frame_schedule_test")
# These merged files exceed the limit; prevent further growth until they are split.
LEGACY_FILE_LINES = {"src/terminal/st.c": 3225, "src/x11/x.c": 3417,
                     "tests/test_native.py": 1333}


# Checks own private virtual displays; none may draw on or inject into the developer's desktop.
DESKTOP_VARIABLES = ("DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY")


@app.callback()
def root():
    os.chdir(ROOT)
    for name in DESKTOP_VARIABLES:
        os.environ.pop(name, None)


def run(args, *, env=None):
    return subprocess.run(args, env=env).returncode


def check_file_limits():
    index = subprocess.check_output(["git", "ls-files", "--stage", "-z"], cwd=ROOT)
    problems = []
    for name in tracked_files(index):
        path = ROOT / name
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except UnicodeDecodeError:
            continue
        limit = LEGACY_FILE_LINES.get(path.relative_to(ROOT).as_posix(), MAX_FILE_LINES)
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


def run_checks():
    if check_privacy(ROOT):
        return 1
    native_build.build_targets(
        "worminal-x11", "worminald-x11", "worminald-preview", "build/key_injector",
        "build/placement_wm", "build/compact-worminal",
        "build/overlap_probe",
        "build/border_probe", "build/icon_probe", "build/view_state_test", *NATIVE_CHECKS,
    )
    code = run_native_checks()
    if code:
        return code
    code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-q"])
    return code or preview_checks("x11")


def run_native_checks():
    for binary in NATIVE_CHECKS:
        code = run([binary], env={**os.environ, "ASAN_OPTIONS": "detect_leaks=0"})
        if code:
            return code
    return 0


def run_headless_checks():
    native_build.build_targets(*NATIVE_CHECKS, "worminald-x11", "worminald-preview")
    code = run_native_checks()
    if code:
        return code
    patterns = ("test_build.py", "test_perf_metrics.py", "test_cli.py", "test_workspace.py",
                "test_preview_workspace.py", "test_theme.py", "test_privacy.py", "test_handoff.py")
    for pattern in patterns:
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                    "-p", pattern, "-q"])
        if code:
            return code
    return 0


def run_proof_script(path):
    return run([sys.executable, str(path)])


@app.command()
def build():
    """Compile Worminal with the current Alacritty theme."""
    native_build.build_targets()


@app.command("build-preview")
def build_preview():
    """Build compatibility names for the GTK frontend and v2 service."""
    native_build.build_targets("worminal-preview", "worminald-preview")


@app.command("build-x11")
def build_x11():
    """Build release binaries and the original X11 rollback pair."""
    native_build.build_targets("worminal-x11", "worminald-x11")


def preview_checks(backend):
    native_build.build_targets("worminal-preview", "worminald-preview", "build/preview_test")
    return run([sys.executable, "tests/preview.py", backend])


@app.command("media-fixtures")
def media_fixtures():
    """Regenerate silent browser media fixtures (requires ffmpeg)."""
    raise typer.Exit(run_proof_script("tests/media_fixtures.py"))


@app.command("check-preview")
def check_preview():
    """Check combined terminal/browser behavior on private Xvfb, locally and through SSH fixtures."""
    raise typer.Exit(preview_checks("x11"))


@app.command("setup-preview-wayland")
def setup_preview_wayland():
    """Download/extract an optional Debian Weston runtime under .checks; no system installation."""
    from tools.weston import setup
    setup()


@app.command("prove-preview-regressions")
def prove_preview_regressions():
    """Compile project-local counterfactuals and require critical regression tests to fail."""
    from tools.regressions import prove
    prove()


@app.command("preview-perf")
def preview_perf():
    """Compare original and preview launch, shell/input latency, and helper-inclusive memory."""
    native_build.build_targets("worminal-preview", "worminal-x11", "worminald-x11",
                               "worminald-preview", "build/key_injector")
    raise typer.Exit(run([sys.executable, "tests/frontend_perf.py"]))


@app.command("check-preview-keys")
def check_preview_keys():
    """Check keyboard, mouse, clipboard, and real editor input through GTK."""
    native_build.build_targets("worminal-preview", "worminald-preview")
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                          "-p", "test_keys.py", "-q"], env={**os.environ, "WORMINAL_FRONTEND": "worminal-preview"}))


@app.command("check-preview-wayland")
def check_preview_wayland():
    """Require native Wayland on headless Weston with DISPLAY unset."""
    raise typer.Exit(preview_checks("wayland"))


@app.command("check-preview-qtile")
def check_preview_qtile():
    """Check the combined frontend under private Qtile."""
    raise typer.Exit(preview_checks("qtile"))


@app.command()
def check():
    """Run the local native and private-display regression suite."""
    raise typer.Exit(run_checks())


@app.command("check-privacy")
def privacy(history: bool = typer.Option(False, "--history", help="Also scan reachable Git history."),
            staged: bool = typer.Option(False, "--staged", help="Scan index content instead of working files."),
            identities: bool = typer.Option(False, "--identities", help="Require anonymous commit identities."),
            message_file: Path = typer.Option(None, "--message-file", help="Also scan a commit message."),
            repository: Path = typer.Option(ROOT, "--repository", help="Repository or initialized submodule to scan.")):
    """Scan tracked content for common private data; redact matched values."""
    repository = repository.resolve()
    message_file = message_file.resolve() if message_file else None
    with chdir(repository):
        raise typer.Exit(int(check_privacy(repository, history, staged, identities, message_file)))


@app.command("privacy-setup")
def privacy_setup():
    """Install tracked privacy hooks and set an anonymous repository-local identity."""
    for name, value in (("core.hooksPath", ".githooks"), ("user.name", "Worminal contributor"),
                        ("user.email", "anonymous")):
        code = run(["git", "config", "--local", name, value])
        if code:
            raise typer.Exit(code)
    typer.echo("Privacy hooks and anonymous identity installed for this checkout.")


@app.command("check-keys")
def check_keys(legacy: bool = typer.Option(False, "--legacy", help="Check the rollback X11 frontend.")):
    """Check keys, mouse input, and clipboard bytes on private Xvfb displays."""
    native_build.build_targets(*(("worminal-x11", "worminald-x11") if legacy else ()))
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                          "-p", "test_keys.py", "-q"],
                          env={**os.environ, "WORMINAL_FRONTEND": "worminal-x11" if legacy else "worminal"}))


@app.command("check-handoff")
def check_handoff(baseline: Path = typer.Option(
        None, exists=True, dir_okay=False, help="Also transfer both ways with a saved v2 daemon binary.")):
    """Check native state snapshots and live transfers between both daemon protocols."""
    native_build.build_targets("build/scrollback_test", "worminald-x11", "worminald-preview")
    code = run(["build/scrollback_test"], env={**os.environ, "ASAN_OPTIONS": "detect_leaks=0"})
    if not code:
        env = dict(os.environ)
        if baseline:
            env["WORMINAL_TEST_HANDOFF_BASELINE"] = str(baseline.resolve())
        code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                    "-p", "test_handoff.py", "-q"], env=env)
    raise typer.Exit(code)


@app.command("check-headless")
def check_headless():
    """Run native and service checks that need no X11 display."""
    raise typer.Exit(run_headless_checks())


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
    """Check build freshness, installation, and cleanup without building the app."""
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                          "-p", "test_build.py", "-q"]))


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
    native_build.build_targets("worminal", "worminald", "worminald-preview")
    raise typer.Exit(run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                         "-p", "test_preview_workspace.py", "-k", "browser_command", "-q"]))


@app.command("explorer-build")
def explorer_build():
    """Build file-explorer/dist/index.html (run npm ci in the submodule first)."""
    explorer_command("build")


@app.command("explorer-check")
def explorer_check():
    """Check explorer types, directory behavior, DOM interactions, and standalone output."""
    code = run([sys.executable, "-m", "unittest", "discover", "-s", "tests",
                "-p", "test_privacy.py", "-q"])
    if code:
        raise typer.Exit(code)
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


@app.command()
def uninstall(prefix: Path = typer.Option(None, help="Prefix used for installation.")):
    """Remove installed files; supports PREFIX, MANPREFIX, and DESTDIR."""
    native_build.uninstall(prefix)


@app.command()
def icon():
    """Regenerate the embedded icon from assets/worminal.svg."""
    try:
        generate_icon()
    except (OSError, ImportError, subprocess.CalledProcessError) as error:
        typer.echo(f"Icon error: {error}", err=True)
        raise typer.Exit(1) from error


@app.command()
def latency(forwarded: bool = typer.Option(
        False, "--forwarded", help="Measure another window in a running GTK owner.")):
    """Measure launch to usable input with /bin/cat on private Xvfb."""
    native_build.build_targets("build/key_injector")
    raise typer.Exit(run([sys.executable, "tests/latency.py", *(["--forwarded"] if forwarded else [])]))


@app.command("latency-visible")
def latency_visible(trials: int = typer.Option(10, min=1), keys: int = typer.Option(50, min=1),
                    trace_startup: bool = typer.Option(False, help="Include diagnostic startup milestones."),
                    shell: bool = typer.Option(False, help="Include interactive shell initialization."),
                    startup_resources: bool = typer.Option(False, help="Sample first-ready CPU and peak RSS counters."),
                    saved_tabs: int = typer.Option(0, min=0, help="Seed service-owned tabs before measurement."),
                    varied_labels: bool = typer.Option(False, help="Seed tabs in distinct project-local directories."),
                    bridge_delay_ms: int = typer.Option(None, min=0, help="Use a private delayed SSH bridge fixture."),
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


@app.command("render-perf")
def render_perf(trials: int = typer.Option(10, min=1), frames: int = typer.Option(60, min=1),
                resize: bool = typer.Option(False, help="Measure viewport imports instead of painting."),
                tabs: int = typer.Option(0, min=0, max=4096, help="Measure a strip with N distinct tab labels."),
                backend: str = typer.Option("x11", help="Private display backend: x11 or wayland."),
                compare: Path = typer.Option(None, help="Snapshot with a compatible internal frontend view ABI.")):
    """Compare release-module painting, exact pixels, CPU, and memory."""
    native_build.build_targets("worminal", "build/render_probe")
    options = ["--compare", str(compare)] if compare else []
    if tabs:
        options += ["--tabs", str(tabs)]
    if resize:
        options += ["--resize"]
    raise typer.Exit(run([sys.executable, "tests/render_perf.py", "--trials", str(trials), "--frames", str(frames),
                          "--backend", backend, *options]))


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
    native_build.build_targets("worminald-preview", "build/frame_schedule_test")
    code = run(["build/frame_schedule_test"])
    if code:
        raise typer.Exit(code)
    variants = [("candidate", ROOT / "worminald-preview")]
    if compare:
        daemon = compare.resolve() / "worminald"
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


@app.command("latency-shell")
def latency_shell():
    """Measure when the interactive shell receives early input."""
    native_build.build_targets("build/key_injector")
    raise typer.Exit(run([sys.executable, "tests/latency.py", "--shell"]))


@app.command("master-perf")
def master_perf(host: str):
    """Measure SSH tab-switch traffic; set WORMINAL_MASTER_DAEMON for a test build."""
    raise typer.Exit(run([sys.executable, "tests/master_perf.py", host]))


@app.command("window-perf")
def window_perf():
    """Measure isolated tab repaint on the current X11 display (needs python-xlib)."""
    native_build.build_targets("worminal-x11", "worminald-x11")
    raise typer.Exit(run([sys.executable, "tests/window_perf.py"]))


@app.command("run")
def run_window():
    """Run the already-built Worminal binary."""
    if not (ROOT / "worminal").is_file():
        typer.echo("Build Worminal first with: python3 manage.py build", err=True)
        raise typer.Exit(1)
    os.execv("./worminal", ["./worminal"])


@proof_app.callback(invoke_without_command=True)
def proof(ctx: typer.Context):
    """Run every local and private-display proof when no child is specified."""
    if ctx.invoked_subcommand is not None:
        return
    typer.echo("Proof: check")
    failures = []
    if run_checks():
        failures.append("check")
    # Every proof_*.py script is an isolated display proof with a main entrypoint.
    # Discovery makes new proofs part of this route without another registry.
    for path in sorted((ROOT / "tests").glob("proof_*.py")):
        name = path.stem.removeprefix("proof_")
        typer.echo(f"Proof: {name}")
        if run_proof_script(path):
            failures.append(name)
    if failures:
        typer.echo(f"Failed: {', '.join(failures)}", err=True)
        raise typer.Exit(1)
    typer.echo("All proofs passed.")


@proof_app.command()
def plasma():
    """Prove shared views and tabs under isolated KWin."""
    native_build.build_targets("build/view_state_test")
    raise typer.Exit(run_proof_script(ROOT / "tests/proof_plasma.py"))


@proof_app.command()
def cinnamon():
    """Prove shared views and tabs under remote Cinnamon on private Xvfb."""
    native_build.build_targets("build/view_state_test")
    raise typer.Exit(run_proof_script(ROOT / "tests/proof_cinnamon.py"))


@proof_app.command()
def movement():
    """Check offscreen Alt-dragging under private Muffin."""
    native_build.build_targets("worminal-x11", "worminald-x11")
    raise typer.Exit(run_proof_script(ROOT / "tests/proof_window_move.py"))


@proof_app.command()
def qtile():
    """Check zoom cell alignment under private Qtile."""
    native_build.build_targets("build/compact-worminal")
    raise typer.Exit(run_proof_script(ROOT / "tests/proof_qtile.py"))


if __name__ == "__main__":
    try:
        app()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        typer.echo(f"Command failed: {error}", err=True)
        raise SystemExit(1) from error
    except KeyboardInterrupt:
        raise SystemExit(130)
