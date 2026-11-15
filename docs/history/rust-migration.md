# Rust migration measurements

Historical cutover record. Current architecture lives in [architecture.md](../architecture.md), and current build,
test and install commands live in [development.md](../development.md). The C comparison tools mentioned below
were removed after the migration; these measurements are retained as evidence, not current acceptance criteria.

The migration's tests passed before installation. The cutover kept the three tabs, three PTYs and all nine running
shell/application process IDs. The later cleanup also passed the full suite, rendering comparisons and installed
SSH/browser checks. Logs are in `build/checks`.

In two HTTP browser trials, Rust used about 5.4–5.5% more total PSS memory than C: 203,402 versus 192,965 KiB for the
first browser, and 265,637 versus 251,782 KiB after opening another. This includes browser helpers, the daemon and
shells. Two trials do not establish a general bound or equal CPU use. Raw results are in
`build/checks/rust-release-browser-perf.json`. Parser checks also showed higher CPU use than the old parser and
recorded measurements in `build/checks/engine-resources.json`.
