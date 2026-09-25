#!/usr/bin/env python3
"""Build, check, measure, and prove Worminal: run `python3 manage.py --help`."""

import os
from pathlib import Path
import subprocess
import sys

try:
    import typer
except ModuleNotFoundError as error:
    raise SystemExit("Install Typer to use manage.py: python3 -m pip install typer") from error

from tools.icon import generate_icon
from tools.theme import generate_theme


ROOT = Path(__file__).resolve().parent
app = typer.Typer(no_args_is_help=True, help="Build and test Worminal.")
proof_app = typer.Typer(help="Run proofs on private Xvfb displays.")
app.add_typer(proof_app, name="proof")
MAX_FILE_LINES = 1000
MAX_LINE_LENGTH = 120
# These inherited C files exceed the limit; prevent further growth until they are split.
LEGACY_FILE_LINES = {"src/terminal/st.c": 3092, "src/x11/x.c": 3364}


@app.callback()
def root():
    os.chdir(ROOT)


def run(args, *, env=None):
    return subprocess.run(args, env=env).returncode


def make(*targets):
    code = run(["make", "-s", *targets])
    if code:
        raise typer.Exit(code)


def prepare(*targets):
    try:
        generate_theme()
    except (OSError, ValueError) as error:
        typer.echo(f"Theme error: {error}", err=True)
        raise typer.Exit(1) from error
    make()
    if targets:
        make(*targets)


def check_file_limits():
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).split(b"\0")
    problems = []
    for raw in paths:
        if not raw:
            continue
        path = ROOT / os.fsdecode(raw)
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
    prepare(".checks/key_injector", ".checks/placement_wm",
            ".checks/compact-worminal", ".checks/overlap_probe",
            ".checks/border_probe", ".checks/icon_probe",
            ".checks/view_state_test", ".checks/scrollback_test", ".checks/url_test")
    code = run([".checks/scrollback_test"],
               env={**os.environ, "ASAN_OPTIONS": "detect_leaks=0"})
    if code:
        return code
    code = run([".checks/url_test"],
               env={**os.environ, "ASAN_OPTIONS": "detect_leaks=0"})
    if code:
        return code
    return run([sys.executable, "-m", "unittest", "discover", "-s", "tests", "-q"])


def run_proof_script(path):
    return run([sys.executable, str(path)])


@app.command()
def build():
    """Compile Worminal with the current Alacritty theme."""
    prepare()


@app.command()
def check():
    """Run the local native and private-display regression suite."""
    raise typer.Exit(run_checks())


@app.command()
def lint():
    """Check file limits and run Clang static analysis (requires clang-tidy)."""
    check_file_limits()
    prepare("lint")


@app.command()
def clean():
    """Remove generated binaries and headers."""
    raise typer.Exit(run(["make", "-s", "clean"]))


@app.command()
def icon():
    """Regenerate the embedded icon from assets/worminal.svg."""
    try:
        generate_icon()
    except (OSError, ImportError, subprocess.CalledProcessError) as error:
        typer.echo(f"Icon error: {error}", err=True)
        raise typer.Exit(1) from error


@app.command()
def latency():
    """Measure launch to usable input with /bin/cat on private Xvfb."""
    prepare(".checks/key_injector")
    raise typer.Exit(run([sys.executable, "tests/latency.py"]))


@app.command("latency-shell")
def latency_shell():
    """Measure when the interactive shell receives early input."""
    prepare(".checks/key_injector")
    raise typer.Exit(run([sys.executable, "tests/latency.py", "--shell"]))


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
    prepare(".checks/view_state_test")
    raise typer.Exit(run_proof_script(ROOT / "tests/proof_plasma.py"))


@proof_app.command()
def cinnamon():
    """Prove shared views and tabs under remote Cinnamon on private Xvfb."""
    prepare(".checks/view_state_test")
    raise typer.Exit(run_proof_script(ROOT / "tests/proof_cinnamon.py"))


if __name__ == "__main__":
    try:
        app()
    except KeyboardInterrupt:
        raise SystemExit(130)
