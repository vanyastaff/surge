# Role 04: clean installation

Scope: read-only audit of README, docs/getting-started.md, release.yml, scripts/release.py, init.rs, daemon.rs, cli_init_test.rs, examples_smoke.rs; no Cargo builds or production configuration changes.

## Confirmed findings

1. README Quick Start builds the workspace, then invokes bare `surge` in another project without installing target/debug/surge on PATH. A clean source build does not install a binary on PATH. Fresh-user instructions fail at this transition. It also requires the optional desktop dependencies although release artifacts contain CLI/daemon only. Owner: technical writer. Acceptance: show source command paths or install both binaries first; link optional desktop build separately.
2. docs/getting-started.md correctly explains paired release binaries and PATH installation, but all onboarding and smoke examples following archive installation use `cargo run` and repository examples. The archive contains exactly README.md, two licenses and two executables; no Cargo.toml or examples. Archive users need direct `surge init --default`, `surge project describe`, and an inline terminal-only graph or clearly marked source-checkout-only smoke instructions. Owner: technical writer. Acceptance: complete archive-user command sequence independent of a source checkout.

## Evidence

- `python3 -m unittest discover -s scripts -p test_release.py`: fails (24 failure subcases across 5 tests), because system Python lacks tomllib. Packaging script declares Python 3.11+; CI selects 3.12, so this is an environment prerequisite rather than a script defect.
- `python3.12 -m unittest discover -s scripts -p test_release.py`: PASS, 5 tests, 14.180s. Tests cover four archive formats, independent checksum verification, bad/duplicate/missing release sets, malformed files, missing daemon, and tag validation. Duplicate zip fixture warning intentional.
- `ls -l target/debug/surge target/release/surge target/debug/surge-daemon target/release/surge-daemon`: all four missing.
- scripts/release.py requires both surge and surge-daemon nonempty regular files; validates exact five archive members and Unix executable permissions. Release matrix extracts archives and invokes both --version natively.
- daemon_binary_path chooses sibling surge-daemon before PATH. Documentation requirement to keep both together agrees with code.
- init --default validates an existing config and returns without changing it; fresh init uses onboarding::default_config and config.save. Existing regression tests cli_init_test include config validity and byte-identical idempotence, but not executed in this read-only role.
- examples_smoke onboarding integration creates temporary project/HOME/SURGE_HOME, runs init, deterministic project describe, terminal-only flow and checks replay terminal==completed. Suitable for coordinator's final test run.

## Remaining evidence gaps

No real native archive or compiled CLI exists yet; --version, fresh init, idempotent init, deterministic project describe, direct terminal flow and daemon terminal flow remain runtime-unverified. Final release must run these against newly built extracted archive with isolated SURGE_HOME and temporary project. Native Windows/Linux/Intel macOS results require respective CI runners; this Apple Silicon host cannot certify them.

No repository files modified.
