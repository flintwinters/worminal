# Testing

Build, test, install and runtime preparation are indexed in [development.md](development.md) and
`python3 manage.py --help`. Tests exercise the current application; historical records live under [history](history/).

## Performance ceilings

`python3 manage.py check-perf` runs the existing warmed release daemon CPU and deterministic network workloads,
then fails if any median exceeds its fixed maximum in `tools/perf_limits.json`. It also runs in the native
phase of full `check` and `check-headless`, and before every `check-python` run, including filtered selections.
Each entrypoint runs the gate once before Python workers start. Reports and raw samples remain in
`build/checks/daemon-perf-check.json`,
`network-perf-check.json` and `perf-check.json`, including failed budget results. These fixed filenames are
replaced on the next run; preserve them before another run when investigating a regression.

The initial CPU ceilings are roughly twice the development host's seven-trial median, rounded upward to allow
CPU-frequency and machine variation. They use thread CPU time per operation, not build/test wall time. Network
ceilings equal the three-trial deterministic baseline: traffic, peak queue size, modeled frame age and tail drain
time. Frame count is informational because fewer frames alone does not prove better performance. Neither suite
measures GUI, real SSH latency or physical presentation.

Limits never update automatically. Investigate failures before changing a ceiling. To review a deliberate change,
run `daemon-perf --label NAME --trials 7` and `network-perf --label NAME --trials 3`, inspect their retained samples,
then explicitly edit the baseline values, maxima and workload hashes with an explanation in the commit. Workload
hash or unit changes, missing/invalid samples and new metrics without a budget decision fail closed. Use
`check-perf --limits PATH` for a reviewed machine-specific policy; ordinary checks always use the checked-in file.

## Python check workers

The full check command builds shared artifacts and runs native checks first:

```sh
python3 manage.py check
```

Only reviewed independent Python modules overlap. Each module runs in its own process; cases within a module
remain serial. Shared build checks, unreviewed modules and standalone GUI suites remain serial. The
reviewed set includes private daemon handoff/upgrade checks and private-display window replacement checks, whose
socket scopes, copied binaries and process cleanup are independent. Adding a test module does not automatically
make it parallel. Resource ownership must be reviewed before adding it to `tools/test_runner.py::INDEPENDENT`.

For iteration against existing build artifacts:

```sh
python3 manage.py check-python --jobs 2 --module test_upgrade --module test_window_replacement
python3 manage.py check-python --jobs 2 --module test_theme --module test_http_fixture
python3 manage.py check-python --jobs 2 --match retained
```

`--module` can be repeated and takes a top-level module name without `.py`. `--match` retains unittest's `-k`
semantics. Without `--module`, discovery still covers all tests, including unreviewed/nested modules in the serial
tail. After the performance gate passes, every selected case remains scheduled; worker failures propagate after other
scheduled tests finish. Reviewed parallel modules have a 180-second worker timeout; a timed-out worker and its
explicitly tagged private fixtures are retired, with a failed result and preserved logs. The serial tail retains
its existing test timeouts. Python checks use existing application artifacts and do not install dependencies.
Use the full `check` command first, or the relevant project build command. The performance gate builds its own
release benchmark prerequisites.

Workers remove inherited desktop display/session-bus variables, use local GIO, and receive separate project-local
XDG state/cache/data/runtime and diagnostic directories. Tests that need graphics create their private Xvfb/Wayland
displays. Workers never share an installed GUI owner or default user workspace. Logs and a JSON summary are retained
under `build/checks/python-tests-*/`; failures are replayed without mixing simultaneous process output.

The default worker count is `floor(available CPUs / 2) + 1`. CPU affinity determines availability where supported;
portable CPU-count APIs provide the fallback, with one CPU assumed if unavailable. Thus 1, 2 and 12 available CPUs
select 1, 2 and 7 workers. Explicit `--jobs N` overrides this formula; `--jobs 1` disables parallelism. Any positive
worker count is accepted, and concurrency is also limited by the number of eligible modules.
Short CPU-heavy suites can become slower because of process startup and CPU contention. Compare the same
selected tests on the target machine. Native compilation is not duplicated across workers. Parallel mode does not
shorten timeout assertions or skip cleanup/coverage to manufacture a speedup.

A representative Cinnamon run (12 exposed CPUs) passed the same 34 upgrade/handoff/window tests in
30.32s with one worker and 17.65s with two workers, about 42% less wall time. This is a selected-suite measurement,
not a claim about the full check command. The short-suite sample on the two-CPU assistant host was slower in
parallel; use `--jobs 1` when that suits the selected checks better.
