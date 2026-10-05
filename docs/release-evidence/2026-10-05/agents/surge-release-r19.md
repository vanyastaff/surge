# R19 platform compatibility audit

Scope: CLI/daemon release archives; desktop remains optional/in-development. Read-only audit; no repository edits and no parallel cargo builds.

## Findings

1. P1 Windows runtime capability differs from advertised four-target distribution. `crates/surge-persistence/src/work_items/start_preparation/secure_lock.rs:40–47` requires RestrictedPrivateInputs for every task; lines 75–80 reject that requirement on Windows. `start_preparation.rs:154,207` calls this boundary, so durable Task Start preparation fails safely on Windows. This is explicit unsupported behavior, not an ACL bug to bypass. Document capability limitation or complete a secure Windows implementation before claiming full workflow parity.
2. P1 Windows MCP executable writer dispatch cannot establish process identity. `surge-acp/src/process_evidence.rs:25–32` returns unavailable on non-Linux/non-macOS. `surge-orchestrator/src/engine/writer_coverage.rs:57–62` propagates observe_container failure; `surge-mcp/src/connection.rs:414` invokes child_started. Windows ordinary MCP-backed workflow therefore has a fail-closed limitation beyond archive --version smoke. Recovery liveness is Unknown on Windows. ACP direct session opening tolerates absent container (`bridge/worker.rs:559–566`); don't incorrectly claim all ACP operations are unavailable.
3. Native release evidence remains external: release.yml configures Windows2022, macOS15 Intel/ARM and Ubuntu24.04 builds; this local macOS inspection doesn't establish those builds. `--version` smoke proves loader/start only, not daemon named pipes or execution.
4. GNU Linux archive is dynamic and oldest validated environment is Ubuntu24.04; docs already clearly state glibc/OpenSSL dependencies, Alpine/older distro unvalidated. macOS15 minimum tested rather than universal older macOS compatibility is also disclosed.
5. Desktop excluded from macOS/Windows test suites; workspace clippy still builds desktop on those runners. Desktop Metal/native libraries may affect CI, but archive only contains surge-cli/surge-daemon. No evidence to expand release scope to desktop.

## Checks

- Read release workflow, CI matrix, release packaging target set, getting-started supported platform text, secure preparation platform gates, process-evidence and observer call sites.
- Initial `python3 -m unittest discover -s scripts -p test_release.py -v` failed because system python3=3.9.6 has no tomllib; scripts explicitly require3.11+. This is environment prerequisite, not packaging failure.
- Re-ran packaging contract with `/opt/homebrew/bin/python3.12` (see coordinator message for result).
- No native Linux/Windows build, no GitHub runner availability claim, no browser/device applicability (native app).

Acceptance: platform support documentation must name Windows task/private input/MCP recovery limitations and release verdict must require actual native workflow artifacts if claiming four-target binary readiness.
