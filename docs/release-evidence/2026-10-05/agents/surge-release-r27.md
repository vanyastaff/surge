# R27 Observability audit

## Fixed, awaiting execution evidence
- Store::default_path and ArtifactStore::default_path ignored SURGE_HOME, so clean isolated diagnostics/analytics could access actual user usage.db and artifact blobs. Both now use existing surge_core::home::surge_home_dir; error variants retained, docs updated.
- Regression tests added beside each implementation. Child process with temporary explicit SURGE_HOME asserts the exact expected path. Parent environment untouched. Exact test output is checked to prevent a zero-test false pass.
- R11 independently confirmed defect and approved plan.
- git diff --check passed. Cargo intentionally deferred to coordinator; tests not yet executed. No commits.

## Concrete findings forwarded to coordinator
- crates/surge-daemon/src/main.rs: server task logs run_with_supervisor error and cancels shutdown (~442), but enclosing runtime returns 0u8 (~462) unconditionally. Binding failure can terminate daemon with a successful exit code, hiding failure from launchers.
- crates/surge-daemon/src/server.rs: McpProbe branch (~1162) uses SurgeConfig::discover().unwrap_or_default(). Malformed configuration silently becomes zero configured MCP servers, yielding an empty diagnostic result rather than reporting configuration failure.

## Scope and limitations
Read-only scans focused daemon startup/shutdown and notifications; notification failures generally emit tracing warn/error. R16 owns token redaction. No external services exercised. Final release observability remains dependent on execution evidence and daemon findings resolution.
