# Python check workers

The full check command builds shared artifacts and runs native checks first:

```sh
python3 manage.py check
```

Only reviewed independent Python modules overlap. Each module runs in its own process; cases within a module
remain serial. Shared build/reference checks, unreviewed modules and standalone GUI suites remain serial. The
reviewed set includes private daemon handoff/upgrade checks and private-display window replacement checks, whose
socket scopes, copied binaries and process cleanup are independent. Adding a test module does not automatically
make it parallel. Resource ownership must be reviewed before adding it to `tools/test_runner.py::INDEPENDENT`.

For iteration against existing build artifacts:

```sh
python3 manage.py check-python --jobs 2 --module test_upgrade --module test_window_replacement
python3 manage.py check-python --jobs 2 --module test_privacy --module test_http_fixture
python3 manage.py check-python --jobs 2 --match retained
```

`--module` can be repeated and takes a top-level module name without `.py`. `--match` retains unittest's `-k`
semantics. Without `--module`, discovery still covers all tests, including unreviewed/nested modules in the serial
tail. Every selected case remains scheduled; worker failures propagate as a nonzero exit status after other
scheduled tests finish. Reviewed parallel modules have a 180-second worker timeout; a timed-out worker and its
explicitly tagged private fixtures are retired, with a failed result and preserved logs. The serial tail retains
its existing test timeouts. This command does not build prerequisites or install dependencies. Use the full `check`
command first, or the relevant project build command. Historical C aliases live only for their build owner’s
lifetime; use the full check command for suites requiring those disposable artifacts.

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

A representative Cinnamon run (12 exposed CPUs) passed the same 34 upgrade/handoff/window/privacy tests in
30.32s with one worker and 17.65s with two workers, about 42% less wall time. This is a selected-suite measurement,
not a claim about the full check command. The short-suite sample on the two-CPU assistant host was slower in
parallel; use `--jobs 1` when that suits the selected checks better.
