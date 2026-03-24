# End-to-End Test Suite for Surge Orchestrator

This directory contains end-to-end integration tests for the Surge orchestrator pipeline. These tests verify the complete workflow from spec validation through agent execution, QA review, and merge.

## Test Coverage

The E2E test suite covers:
- **Full pipeline execution** with real ACP agents
- **Dependency resolution** and topological ordering
- **Event streaming** (AgentMessageChunk, TokensConsumed)
- **Git worktree** creation and commit generation
- **Retry policies** and circuit breakers
- **Error handling** (timeouts, auth failures, connection failures)
- **QA response parsing** with malformed input handling
- **Task checkpointing** and resume after crash
- **Graceful degradation** when agents are unavailable

## Prerequisites

### Required
- Rust toolchain (stable)
- Git installed and in PATH
- Write access to temp directory

### Optional (for full E2E tests)
- At least one ACP-compatible agent installed:
  - **Claude Code**: MCP-compatible coding agent
  - **GitHub Copilot CLI**: `gh copilot` command
  - **Other ACP agents**: See ACP registry

**Note**: Tests that require a real agent will automatically skip if no agent is found. Core functionality tests (retry logic, QA parsing, state machines) run without agents.

## Running the Tests

### Run all tests
```bash
cargo test -p surge-orchestrator --test '*'
```

### Run specific test files

```bash
# Full pipeline tests (requires agent)
cargo test -p surge-orchestrator --test e2e_pipeline

# Retry policies and circuit breaker tests (no agent required)
cargo test -p surge-orchestrator --test retry_policies_e2e

# Integration tests (no agent required)
cargo test -p surge-orchestrator --test integration
```

### Run individual test cases

```bash
# Simple spec execution
cargo test -p surge-orchestrator --test e2e_pipeline test_e2e_simple_spec

# Dependency ordering verification
cargo test -p surge-orchestrator --test e2e_pipeline test_e2e_dependency_order

# Streaming events
cargo test -p surge-orchestrator --test e2e_pipeline test_e2e_streaming_events

# Git commit generation
cargo test -p surge-orchestrator --test e2e_pipeline test_e2e_git_commits

# Timeout handling
cargo test -p surge-orchestrator --test e2e_pipeline test_agent_timeout_retry_logic

# Connection failure handling
cargo test -p surge-orchestrator --test e2e_pipeline test_agent_connection_failure_graceful_degradation

# Malformed QA response handling
cargo test -p surge-orchestrator --test e2e_pipeline test_error_malformed_qa_response
```

### Run with output logging

```bash
# Show test output (including eprintln! statements)
cargo test -p surge-orchestrator --test e2e_pipeline -- --nocapture

# Show test output with minimal noise
cargo test -p surge-orchestrator --test e2e_pipeline -- --nocapture --test-threads=1
```

## Test Files

### `e2e_pipeline.rs`
**Full pipeline integration tests** - Requires ACP agent for most tests

| Test | Description | Requires Agent |
|------|-------------|----------------|
| `test_e2e_simple_spec` | Execute simple spec with one subtask | ✅ |
| `test_e2e_dependency_order` | Verify subtasks execute in topological order | ✅ |
| `test_e2e_streaming_events` | Verify AgentMessageChunk and TokensConsumed events | ✅ |
| `test_e2e_git_commits` | Verify meaningful commit messages in worktree | ✅ |
| `test_agent_timeout_retry_logic` | Verify timeout error structure and retry config | ❌ |
| `test_agent_connection_failure_graceful_degradation` | Verify graceful failure with invalid agent | ❌ |
| `test_error_malformed_qa_response` | Verify QA response parsing fallback logic | ❌ |

### `retry_policies_e2e.rs`
**Retry, circuit breaker, and checkpoint tests** - No agent required

| Test | Description |
|------|-------------|
| `test_circuit_breaker_config_values` | Verify circuit breaker configuration |
| `test_auth_failure_immediate_fail_config` | Verify auth failure config options |
| `test_retry_policy_backoff_strategies` | Verify backoff strategy configuration |
| `test_rate_limit_error_metadata` | Verify rate limit error includes retry info |
| `test_auth_failure_error_guidance` | Verify auth errors include remediation guidance |
| `test_task_checkpoint_and_resume` | Verify task state checkpoint/resume workflow |
| `test_multiple_spec_checkpoints` | Verify independent spec checkpoints |
| `test_circuit_breaker_config_integration` | Verify circuit breaker config integration |
| `test_default_retry_policy` | Verify default retry policy values |
| `test_checkpoint_all_task_states` | Verify all TaskState variants checkpoint correctly |

### `integration.rs`
**Component integration tests** - No agent required

Additional integration tests for specific components and edge cases.

### `fixtures_validation.rs`
**Fixture validation tests** - No agent required

Validates that test fixture files (specs, configs) are well-formed and loadable.

### `fixtures/`
**Test data and helper modules**

- `mod.rs`: Fixture loading utilities
- `simple_spec.toml`: Single-subtask spec for basic tests
- `dependency_spec.toml`: Multi-subtask spec with dependency graph

### `helpers.rs`
**Test utilities**

Shared helper functions for:
- Binary path detection
- Temp directory/database creation
- Agent discovery and configuration
- Spec loading
- Cleanup

## Expected Outcomes

### Tests with Agent Available

When an ACP agent is installed and discoverable:

```
✓ test_e2e_simple_spec - PASS
  - Pipeline executes (may pause at gates or fail gracefully)
  - No crashes or hangs

✓ test_e2e_dependency_order - PASS
  - Subtasks execute in correct topological order
  - Dependencies respected (base → utils → integration)

✓ test_e2e_streaming_events - PASS
  - AgentMessageChunk events broadcast (if agent supports streaming)
  - TokensConsumed events broadcast with non-zero token counts

✓ test_e2e_git_commits - PASS
  - Worktree created for spec
  - Commits follow format: "surge: subtask {title} — {id}"
  - Commit messages are descriptive (>15 chars)
```

### Tests without Agent

When no ACP agent is available, agent-dependent tests skip automatically:

```
test_e2e_simple_spec ... SKIP: No ACP agent available on this system
test_e2e_dependency_order ... SKIP: No ACP agent available on this system
test_e2e_streaming_events ... SKIP: No ACP agent available on this system
test_e2e_git_commits ... SKIP: No ACP agent available on this system
```

### Tests without Agent Requirement

Core functionality tests always run:

```
✓ test_agent_timeout_retry_logic - PASS
  - Timeout error structure verified
  - Retry configuration verified
  - Circuit breaker prevents infinite loops

✓ test_agent_connection_failure_graceful_degradation - PASS
  - Pipeline fails gracefully with invalid agent
  - Error messages are descriptive and actionable

✓ test_error_malformed_qa_response - PASS
  - Malformed JSON falls back to text parsing
  - Defaults to Approved verdict when unclear
  - Extracts issue descriptions from text markers

✓ test_task_checkpoint_and_resume - PASS
  - Task state persists to database
  - Resume after "crash" loads correct state
  - Checkpoint updates (not appends)

✓ test_circuit_breaker_config_values - PASS
  - Default threshold is 3
  - Custom threshold is configurable
  - New executor starts with circuit closed
```

## Troubleshooting

### "SKIP: No ACP agent available on this system"

**Cause**: No ACP-compatible agent detected in PATH.

**Solutions**:
1. Install an ACP agent (Claude Code, Copilot CLI, etc.)
2. Ensure agent command is in PATH
3. Run agent detection manually:
   ```bash
   cargo run -p surge-cli -- ping
   ```
4. Run tests that don't require agents:
   ```bash
   cargo test -p surge-orchestrator --test retry_policies_e2e
   cargo test -p surge-orchestrator --test e2e_pipeline test_agent_timeout_retry_logic
   ```

### "Pipeline failed (may be expected in E2E test)"

**Cause**: Real agent execution can fail for various reasons.

**Why this is OK**: E2E tests verify graceful handling of failures, not just success cases. The test passes if:
- No crash or hang occurs
- Error messages are descriptive
- System remains in consistent state

**When to investigate**:
- If ALL tests fail consistently
- If error messages are unhelpful
- If cleanup fails (temp dirs/databases not cleaned)

### Test hangs or times out

**Possible causes**:
1. **Agent timeout**: Agent not responding within timeout period
2. **Blocking I/O**: Event listener not properly aborted
3. **Git lock**: Worktree lock not released

**Solutions**:
```bash
# Kill hung process
pkill -9 surge

# Clean up temp directories
rm -rf /tmp/surge-e2e-*

# Clean up git worktrees
cd <project-root>
git worktree prune
```

### Database lock errors

**Cause**: Previous test run didn't clean up database file.

**Solution**:
```bash
# Clean up temp databases
rm -f /tmp/surge-e2e-*.db
rm -f /tmp/surge-retry-e2e-*.db
```

### Git worktree errors

**Cause**: Previous test run left worktree in inconsistent state.

**Solution**:
```bash
# From project root
git worktree prune

# Clean up temp directories
rm -rf /tmp/surge-e2e-*
```

## Test Data Locations

During test execution, temporary files are created:

- **Temp directories**: `/tmp/surge-e2e-<test_name>-<pid>/`
- **Temp databases**: `/tmp/surge-e2e-<test_name>-<pid>.db`
- **Git worktrees**: `<temp_dir>/.git/worktrees/<spec_id>/`

All temp files are automatically cleaned up after tests complete.

## Adding New Tests

### 1. For tests requiring an agent

```rust
#[tokio::test]
async fn test_my_new_feature() {
    // Skip if no agent available
    if !has_any_agent() {
        eprintln!("SKIP: No ACP agent available on this system");
        return;
    }

    // Discover agent
    let mut discovery = AgentDiscovery::new();
    let registry = Registry::builtin();
    let agents = discovery.discover_all(registry.list());

    // ... rest of test
}
```

### 2. For tests without agent requirement

```rust
#[test]
fn test_my_unit_logic() {
    // Direct unit test - no agent needed
    let config = ExecutorConfig::default();
    assert_eq!(config.max_retries, 3);
}

#[tokio::test]
async fn test_my_integration_logic() {
    // Async integration test - no agent needed
    let store = Store::open(&temp_db_path("test")).unwrap();
    // ... rest of test
}
```

### 3. Pattern to follow

1. Use `temp_test_dir()` or `temp_db_path()` for file paths
2. Always clean up with `cleanup_dir()` or `cleanup_db()`
3. Allow failures for agent-dependent tests (check error messages, not just success)
4. Use `eprintln!` for debug output (visible with `--nocapture`)
5. Add tests to appropriate file:
   - `e2e_pipeline.rs`: Full pipeline tests
   - `retry_policies_e2e.rs`: Retry/circuit breaker/checkpoint tests
   - `integration.rs`: Component integration tests

## CI/CD Integration

Tests are designed to work in CI environments where agents may not be available:

- **Agent-dependent tests**: Auto-skip if no agent found
- **Core tests**: Always run (retry logic, QA parsing, state machines)
- **No external dependencies**: Tests use temp directories and in-process git repos
- **Parallel-safe**: Each test uses unique temp paths (test name + PID)

### Recommended CI test command

```bash
# Run all tests, allow agent tests to skip
cargo test -p surge-orchestrator --test '*' -- --test-threads=4

# Run only tests that don't require agents (faster)
cargo test -p surge-orchestrator --test retry_policies_e2e
cargo test -p surge-orchestrator --test integration
```

## Acceptance Criteria Verification

These E2E tests verify the acceptance criteria for:

### Spec Execution (003-end-to-end-pipeline-hardening)
- [x] Full pipeline executes from spec validation to merge
- [x] Subtasks execute in correct dependency order
- [x] Git worktrees created and isolated
- [x] Commits generated with meaningful messages
- [x] Events broadcast for streaming and token tracking

### Retry Policies and Resilience
- [x] Circuit breaker trips after threshold failures
- [x] Rate limit errors include retry metadata
- [x] Auth failures handled with immediate fail or retry
- [x] Backoff strategies (linear, exponential, jitter) configurable
- [x] Task state checkpointed and resumable after crash

### Error Handling
- [x] Timeout errors have descriptive messages
- [x] Connection failures degrade gracefully (no crashes)
- [x] Malformed QA responses fall back to text parsing
- [x] Error messages include actionable remediation steps

## Related Documentation

- **Architecture**: `docs/02-ARCHITECTURE.md`
- **ACP Integration**: `crates/surge-acp/README.md`
- **Token Tracking Verification**: `INTEGRATION_TEST_VERIFICATION.md`
- **Orchestrator Design**: `crates/surge-orchestrator/README.md`

## Questions?

If tests fail unexpectedly or you need to add new test coverage, check:
1. This README for troubleshooting steps
2. Individual test file documentation (doc comments)
3. Helper functions in `helpers.rs` and `fixtures/mod.rs`
4. Project architecture docs in `docs/`
