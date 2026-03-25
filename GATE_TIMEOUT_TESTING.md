# Gate Timeout Testing Guide

## Overview

This document describes how to test gate timeout behavior in the Surge pipeline system. Gate timeouts allow automatic handling of gates when no human response is received within a configured time period.

## Current Implementation Status

### ✅ Implemented Features

1. **Timeout Tracking**
   - Gates track when they were triggered via `GATE_STATE.json`
   - Elapsed time is calculated from the `triggered_at` timestamp

2. **Auto-Abort Behavior**
   - When a gate times out, the pipeline returns `PipelineResult::Failed`
   - Timeout includes elapsed time information
   - All gate phases support timeout (after_spec, after_plan, after_qa)

3. **Decision Prevention**
   - Recording any decision (Approved, Rejected, Aborted) prevents timeout
   - Decisions are persisted to `DECISION.json` and include decision timestamp

4. **Programmatic Configuration**
   - Timeout can be configured via `GateManager::with_timeout(config, specs_dir, duration)`
   - No timeout is the default (gates pause indefinitely)

### ❌ Not Yet Implemented

1. **Auto-Approve Behavior**
   - The spec mentions "auto-abort or auto-approve based on config"
   - Currently only auto-abort is implemented
   - Auto-approve would require a configuration option to choose the timeout behavior

2. **surge.toml Configuration**
   - Timeout cannot be configured via surge.toml yet
   - Would require adding fields like:
     ```toml
     [pipeline.gates]
     timeout_secs = 3600  # 1 hour
     timeout_action = "abort"  # or "approve"
     ```

## Automated Testing

### Running the Test Suite

The comprehensive timeout test suite is located in:
```
crates/surge-orchestrator/tests/gate_timeout_e2e.rs
```

Run all timeout tests:
```bash
cargo test --test gate_timeout_e2e -p surge-orchestrator
```

### Test Coverage

The test suite includes 8 comprehensive tests:

1. **test_gate_timeout_basic**
   - Verifies gate doesn't timeout before configured duration
   - Verifies gate returns Timeout action after configured duration
   - Validates elapsed time information

2. **test_gate_decision_prevents_timeout**
   - Verifies approval decision prevents timeout
   - Tests that decisions persist correctly

3. **test_gate_timeout_different_phases**
   - Tests timeout at after_spec gate
   - Tests timeout at after_plan gate
   - Tests timeout at after_qa gate

4. **test_gate_timeout_different_durations**
   - Tests short timeouts (1-2 seconds)
   - Tests longer timeouts (5+ seconds)
   - Verifies gates don't timeout before configured duration

5. **test_gate_timeout_state_persistence**
   - Verifies GATE_STATE.json creation and structure
   - Tests timeout calculation from persisted state
   - Validates phase information in state

6. **test_gate_without_timeout**
   - Verifies gates without timeout never timeout
   - Tests indefinite pause behavior

7. **test_gate_rejection_prevents_timeout**
   - Verifies rejection decision prevents timeout
   - Tests feedback persistence

8. **test_gate_abort_prevents_timeout**
   - Verifies abort decision prevents timeout
   - Tests abort reason persistence

## Manual Testing

### Prerequisites

Since surge.toml configuration is not yet implemented, manual testing requires code changes to create a GateManager with timeout.

### Test Scenario 1: Basic Timeout (Auto-Abort)

**Objective:** Verify gate times out and pipeline fails after configured duration.

**Setup:**
1. Create a test spec with `after_plan` gate enabled
2. Modify orchestrator to create GateManager with 10-second timeout:
   ```rust
   let gate_manager = GateManager::with_timeout(
       config.pipeline.gates.clone(),
       specs_dir.clone(),
       Duration::from_secs(10),
   );
   ```

**Steps:**
1. Start pipeline: `cargo run -p surge-cli -- pipeline run <spec-id>`
2. Wait for pipeline to reach planning gate
3. Observe gate pause message with reason
4. **Do not approve or reject** - wait 10+ seconds
5. Observe pipeline failure with timeout message

**Expected Results:**
- Pipeline pauses at planning gate
- After 10 seconds, pipeline fails with message: "gate timed out after 10 seconds"
- GATE_STATE.json exists in spec directory with triggered_at timestamp
- No DECISION.json file (no decision was made)

### Test Scenario 2: Decision Prevents Timeout

**Objective:** Verify making a decision before timeout prevents timeout.

**Setup:**
Same as Test Scenario 1

**Steps:**
1. Start pipeline: `cargo run -p surge-cli -- pipeline run <spec-id>`
2. Wait for pipeline to reach planning gate (paused)
3. Wait 5 seconds (half the timeout period)
4. Approve gate via CLI or UI
5. Observe pipeline continues to execution phase

**Expected Results:**
- Pipeline pauses at planning gate
- Approval at 5 seconds prevents timeout
- Pipeline continues normally to execution phase
- DECISION.json exists with approval decision
- GATE_STATE.json includes decision and decided_at timestamp

### Test Scenario 3: Multiple Gates with Timeout

**Objective:** Verify timeout works correctly across multiple gate phases.

**Setup:**
1. Enable all gates in surge.toml:
   ```toml
   [pipeline.gates]
   after_spec = true
   after_plan = true
   after_qa = true
   ```
2. Configure GateManager with 15-second timeout

**Steps:**
1. Start pipeline
2. Approve after_spec gate immediately
3. Wait for after_plan gate and approve immediately
4. Wait for after_qa gate
5. **Do not approve** - wait 15+ seconds
6. Observe pipeline timeout at QA gate

**Expected Results:**
- First two gates (spec, plan) approve successfully
- QA gate times out after 15 seconds
- Pipeline fails with timeout message
- GATE_STATE.json shows QaReview phase

### Test Scenario 4: Timeout with Rejection

**Objective:** Verify rejection before timeout prevents timeout.

**Setup:**
Same as Test Scenario 1

**Steps:**
1. Start pipeline
2. Wait for planning gate
3. Wait 5 seconds
4. **Reject** gate with feedback: "Please add more detail to subtask descriptions"
5. Observe pipeline behavior

**Expected Results:**
- Pipeline receives rejection decision
- Timeout is prevented (decision was made)
- Feedback is injected into agent's next prompt
- Phase re-runs with feedback

## Verification Checklist

### Unit Tests
- [x] test_gate_timeout (in gates.rs)
- [x] test_gate_decision_prevents_timeout (in gates.rs)
- [x] test_gate_with_timeout_constructor (in gates.rs)
- [x] test_gate_without_timeout (in gates.rs)

### Integration Tests
- [x] test_gate_timeout_basic
- [x] test_gate_decision_prevents_timeout
- [x] test_gate_timeout_different_phases
- [x] test_gate_timeout_different_durations
- [x] test_gate_timeout_state_persistence
- [x] test_gate_without_timeout
- [x] test_gate_rejection_prevents_timeout
- [x] test_gate_abort_prevents_timeout

### Manual Verification
- [ ] Basic timeout causes pipeline failure (auto-abort)
- [ ] Timeout message includes elapsed time
- [ ] Decision before timeout prevents timeout
- [ ] Timeout works at all gate phases
- [ ] GATE_STATE.json created with correct structure
- [ ] GATE_STATE.json persists across restarts

## Implementation Gaps and Future Work

### Priority 1: surge.toml Configuration
Add timeout configuration to surge.toml:
```toml
[pipeline.gates]
after_spec = true
after_plan = true
after_qa = true
timeout_secs = 3600        # Optional: timeout in seconds (default: no timeout)
timeout_action = "abort"    # Optional: "abort" or "approve" (default: "abort")
```

Implementation required:
1. Add `timeout_secs` and `timeout_action` fields to `GateConfig` struct
2. Update `GateManager::from_config()` to read timeout settings
3. Update pipeline.rs to use timeout from config
4. Add validation for timeout_secs > 0
5. Add validation for timeout_action enum

### Priority 2: Auto-Approve Behavior
Currently only auto-abort is implemented. Add auto-approve option:

1. Add `TimeoutAction` enum to config:
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize)]
   pub enum TimeoutAction {
       Abort,   // Current behavior: fail pipeline
       Approve, // New: auto-approve and continue
   }
   ```

2. Update `GateManager` to track timeout action:
   ```rust
   pub struct GateManager {
       config: GateConfig,
       specs_dir: PathBuf,
       timeout_secs: Option<u64>,
       timeout_action: TimeoutAction,  // New field
   }
   ```

3. Update timeout handling in pipeline.rs:
   ```rust
   GateAction::Timeout { elapsed } => {
       match gate_manager.timeout_action() {
           TimeoutAction::Abort => {
               // Current behavior: fail pipeline
               return PipelineResult::Failed { ... };
           }
           TimeoutAction::Approve => {
               // New behavior: auto-approve and continue
               info!("Gate auto-approved after timeout");
               // Continue to next phase
           }
       }
   }
   ```

### Priority 3: Timeout Notifications
Add notifications when gates are approaching timeout:

1. Emit warning events at 50%, 75%, 90% of timeout period
2. CLI displays countdown timer for active gates
3. UI shows timeout progress bar on gate approval screen

## Troubleshooting

### Issue: Timeout not triggering
**Cause:** GateManager created without timeout
**Solution:** Ensure GateManager is created with `with_timeout()` constructor

### Issue: Timeout triggers too early/late
**Cause:** Clock skew or system time changes
**Solution:** Check system clock, verify GATE_STATE.json triggered_at timestamp

### Issue: Decision doesn't prevent timeout
**Cause:** DECISION.json not written or not in correct location
**Solution:**
- Verify DECISION.json exists in `.auto-claude/specs/{spec-id}/DECISION.json`
- Check file permissions
- Verify JSON format matches GateDecision schema

### Issue: Tests fail on slow systems
**Cause:** Timing-sensitive tests may fail if system is under load
**Solution:**
- Increase timeout durations in tests
- Run tests with `--test-threads=1` to avoid contention
- Use longer sleep periods in manual tests

## References

- Implementation: `crates/surge-orchestrator/src/gates.rs`
- Pipeline handling: `crates/surge-orchestrator/src/pipeline.rs`
- Config types: `crates/surge-core/src/config.rs`
- Integration tests: `crates/surge-orchestrator/tests/gate_timeout_e2e.rs`
- Unit tests: `crates/surge-orchestrator/src/gates.rs#L664-L745`

## Success Criteria

The gate timeout feature is considered fully tested when:

- [x] All unit tests pass (4/4 tests in gates.rs)
- [x] All integration tests pass (8/8 tests in gate_timeout_e2e.rs)
- [ ] Manual testing confirms auto-abort behavior works as expected
- [ ] Documentation explains current limitations (no auto-approve, no surge.toml config)
- [ ] Implementation gaps are documented for future work

**Current Status:** ✅ Automated testing complete, manual verification pending (requires surge.toml configuration support)
