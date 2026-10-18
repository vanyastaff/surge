# Stage 1 test mock discovery closure

Initial Stage 1 authoring, shared feature repair count 0/3. No production feature or runtime namespace changes. Existing Stage 0 frozen evidence remains historical and immutable.

Observed: eight fallback expressions across five integration-fixture source files hardcode workspace target/debug/mock_acp_agent. This worktree target/debug is a symlink into the separate primary checkout; setting CARGO_TARGET_DIR for current gates does not change those expressions. The actual stage gate must run a freshly built mock from its own selected target/profile.

Add one test-only included helper, scripts/test-support/mock_acp_binary.rs, and migrate only:

- crates/surge-daemon/tests/fixtures/wire_bridge.rs
- crates/surge-daemon/tests/fixtures/bootstrap_gate_cold.rs
- crates/surge-daemon/tests/quota_recovery_route_test.rs (three callers)
- crates/surge-daemon/tests/owned_flow_mcp_recovery.rs
- crates/surge-orchestrator/tests/engine_acp_permission.rs (two callers)

Use OsString/PathBuf without lossy text conversion or environment mutation. Preserve the existing explicit SURGE_TEST_MOCK_ACP_BINARY override first; then CARGO_BIN_EXE_mock_acp_agent if supplied. Otherwise derive the actual selected profile directory from current_exe when its immediate directory is Cargo's deps directory, and select the sibling mock_acp_agent plus platform EXE_SUFFIX. This avoids guessing debug/release or interpreting a relative Cargo target against each test process's crate cwd. The executable is selected before recipe admission, preserving immutable launch identity.

Only if the test executable cannot identify a Cargo deps/profile layout, reuse the established repository CARGO_TARGET_DIR fallback: absolute values stay absolute, relative values are rooted at the workspace (manifest_dir/../..), and absence uses workspace target. This fallback selects debug because the existing repository test command uses the debug/test profile; normal Cargo integration execution uses the actual-profile path above. An explicitly selected or inferred missing binary fails loudly and never silently falls back to the primary checkout. No main target/debug symlink or shared binary is changed.

No new generic library, dependency, production API or platform sandbox is needed. Include the same helper under a private module in each caller; keep existing command arguments, fixture scripts, source/registry identifiers, assertions and process-settlement checks intact. No path-resolver mirror test is added. Meaningful validation is current-target build plus actual integration admission/ACP/MCP execution: build cargo build --locked -p surge-acp --bin mock_acp_agent under the gate environment; record exact path, bytes digest and build receipt; run existing real integrations and the deliberate outer RED separately. Inspect recorded launch commands where available to ensure the current profile path is used, including the controlled wire fixture before immutable admission. Run strict all-target Clippy in both feature modes and format/diff checks for the helper/caller closure. No paid provider launch, real retained-project mutation, push or merge follows from this harness repair.

Requires focused owner acceptance before edits. Targeted preceding Stage 1 regression suite already passed 5/5 (four original runtime-gate failures corrected without weakening semantic assertions plus the raw-vs-normalized graph guard control); that is not a full affected-suite pass.
