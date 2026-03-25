# Gate State Persistence and Resume Testing Guide

This guide provides manual testing scenarios for verifying gate state persistence and resume behavior after pipeline restart.

## Overview

The gate persistence feature ensures that pipeline gates maintain their state across restarts:
- **GATE_STATE.json** tracks when a gate was triggered and any decisions made
- **Timeout tracking** persists across restarts (countdown continues from original trigger time)
- **Decision persistence** allows approval/rejection after restart
- **Multiple restarts** are supported before making a decision

## Prerequisites

1. Build the project:
   ```bash
   cargo build --workspace
   ```

2. Create a test spec directory:
   ```bash
   mkdir -p .auto-claude/specs/test-gate-resume
   ```

3. Configure a gate in your test spec's `surge.toml`:
   ```toml
   [pipeline.gates]
   after_plan = true
   ```

## Test Scenario 1: Basic Persistence Across Restart

**Goal:** Verify gate state persists when pipeline is killed and restarted.

### Steps

1. **Start pipeline with gate enabled:**
   ```bash
   cargo run -p surge-cli -- run test-gate-resume
   ```

2. **Wait for gate to trigger:**
   - Pipeline should pause at the planning gate
   - CLI should display: "⏸️ Gate awaiting approval"

3. **Verify GATE_STATE.json created:**
   ```bash
   cat .auto-claude/specs/test-gate-resume/GATE_STATE.json
   ```

   Expected output:
   ```json
   {
     "phase": "Planning",
     "triggered_at": 1234567890,
     "decision": null,
     "decided_at": null
   }
   ```

   ✅ Verify:
   - File exists
   - `triggered_at` is a Unix timestamp (non-zero)
   - `decision` is null
   - `decided_at` is null

4. **Kill the pipeline process:**
   - Press `Ctrl+C` to terminate the pipeline
   - Or use task manager to kill the process

5. **Verify GATE_STATE.json still exists:**
   ```bash
   cat .auto-claude/specs/test-gate-resume/GATE_STATE.json
   ```

   ✅ Verify:
   - File still exists (not deleted on crash)
   - Contents unchanged from step 3

6. **Restart the pipeline:**
   ```bash
   cargo run -p surge-cli -- run test-gate-resume
   ```

7. **Verify gate state restored:**
   - Pipeline should immediately recognize the gate is still pending
   - Should display: "⏸️ Gate awaiting approval"
   - Should NOT re-trigger the gate (no new timestamp)

8. **Approve the gate:**
   - When prompted, select `[a] Approve`
   - Enter optional feedback: "Approved after restart"

9. **Verify decision persisted:**
   ```bash
   cat .auto-claude/specs/test-gate-resume/GATE_STATE.json
   ```

   Expected output:
   ```json
   {
     "phase": "Planning",
     "triggered_at": 1234567890,
     "decision": {
       "Approved": {
         "feedback": "Approved after restart"
       }
     },
     "decided_at": 1234567900
   }
   ```

   ✅ Verify:
   - `triggered_at` matches original timestamp
   - `decision` contains approval
   - `decided_at` is set

### Success Criteria
- ✅ GATE_STATE.json persists across restart
- ✅ Original trigger timestamp preserved
- ✅ Gate remains paused after restart
- ✅ Approval after restart works correctly
- ✅ Pipeline continues after approval

---

## Test Scenario 2: Timeout Tracking Across Restart

**Goal:** Verify timeout countdown continues across restart using original trigger time.

### Steps

1. **Configure gate with short timeout:**
   Create a test program that uses `GateManager::with_timeout()`:
   ```rust
   let manager = GateManager::with_timeout(
       config,
       specs_dir,
       Duration::from_secs(60), // 60 second timeout
   );
   ```

2. **Trigger gate and note time:**
   - Start pipeline
   - Gate triggers at time T0
   - Note the `triggered_at` timestamp from GATE_STATE.json

3. **Wait 30 seconds:**
   - Let pipeline run for 30 seconds (half the timeout)

4. **Kill and restart pipeline:**
   - Kill the process
   - Immediately restart it

5. **Verify timeout not reached yet:**
   - Pipeline should still pause (30 seconds elapsed < 60 second timeout)
   - Gate should await approval

6. **Wait another 35 seconds:**
   - Total time now: 30 + 35 = 65 seconds > 60 second timeout

7. **Verify timeout fires:**
   - Pipeline should detect timeout
   - Should display: "⏱️ Gate timed out (elapsed: 65s)"
   - Pipeline should fail/abort (current behavior)

8. **Verify timeout calculation used original timestamp:**
   ```bash
   # Calculate: current_time - triggered_at should be ~65 seconds
   cat .auto-claude/specs/test-gate-resume/GATE_STATE.json
   ```

### Success Criteria
- ✅ Timeout countdown continues across restart
- ✅ Elapsed time calculated from original trigger time
- ✅ Timeout fires after total elapsed time exceeds limit
- ✅ Restart does not reset timeout clock

---

## Test Scenario 3: Multiple Restarts Before Decision

**Goal:** Verify gate handles multiple restart cycles gracefully.

### Steps

1. **Start pipeline:**
   ```bash
   cargo run -p surge-cli -- run test-gate-resume
   ```

2. **Wait for gate:**
   - Gate triggers and pauses

3. **Kill and restart #1:**
   - Kill process
   - Restart: `cargo run -p surge-cli -- run test-gate-resume`
   - Verify gate still paused

4. **Kill and restart #2:**
   - Kill process again
   - Restart: `cargo run -p surge-cli -- run test-gate-resume`
   - Verify gate still paused

5. **Kill and restart #3:**
   - Kill process again
   - Restart: `cargo run -p surge-cli -- run test-gate-resume`
   - Verify gate still paused

6. **Verify GATE_STATE.json unchanged:**
   ```bash
   cat .auto-claude/specs/test-gate-resume/GATE_STATE.json
   ```

   ✅ Verify:
   - `triggered_at` still matches original time
   - `decision` still null
   - File integrity maintained

7. **Finally approve:**
   - Select `[a] Approve`
   - Pipeline continues

### Success Criteria
- ✅ Multiple restarts supported
- ✅ State remains consistent
- ✅ Original trigger time preserved across all restarts
- ✅ Decision works after multiple restarts

---

## Test Scenario 4: Rejection After Restart

**Goal:** Verify rejection with feedback works after restart.

### Steps

1. **Start pipeline and trigger gate:**
   ```bash
   cargo run -p surge-cli -- run test-gate-resume
   ```

2. **Kill process at gate:**
   - Gate triggers and pauses
   - Kill the process

3. **Restart and reject:**
   ```bash
   cargo run -p surge-cli -- run test-gate-resume
   ```
   - When prompted, select `[r] Reject`
   - Enter feedback: "Plan needs more detail on error handling"

4. **Verify rejection persisted:**
   ```bash
   cat .auto-claude/specs/test-gate-resume/GATE_STATE.json
   ```

   Expected:
   ```json
   {
     "phase": "Planning",
     "triggered_at": 1234567890,
     "decision": {
       "Rejected": {
         "reason": "Needs revision",
         "feedback": "Plan needs more detail on error handling"
       }
     },
     "decided_at": 1234567900
   }
   ```

5. **Verify pipeline handles rejection:**
   - Pipeline should re-run the planning phase
   - Rejection feedback should be injected into agent prompt
   - Check for HUMAN_INPUT.md or feedback in agent context

### Success Criteria
- ✅ Rejection works after restart
- ✅ Rejection feedback persisted
- ✅ Pipeline re-runs phase with feedback

---

## Test Scenario 5: Persistence Across Different Phases

**Goal:** Verify persistence works for all gate phases.

### Test each phase:

1. **After Planning Gate:**
   - Enable: `after_plan = true`
   - Test restart at Planning phase

2. **After Executing Gate:**
   - Enable: `after_execution = true`  (if supported)
   - Test restart at Executing phase

3. **After QA Gate:**
   - Enable: `after_qa = true`
   - Test restart at QaReview phase

### Success Criteria
- ✅ GATE_STATE.json correctly records phase
- ✅ Persistence works for all phases
- ✅ Decision can be made after restart for any phase

---

## Verification Checklist

After completing all scenarios:

- [ ] GATE_STATE.json is created when gate triggers
- [ ] GATE_STATE.json persists across process crash/restart
- [ ] Original trigger timestamp preserved across restarts
- [ ] Gate remains paused after restart (awaits approval)
- [ ] Approval after restart works correctly
- [ ] Rejection after restart works correctly
- [ ] Timeout tracking uses original trigger time
- [ ] Multiple restarts supported before decision
- [ ] Decision persistence includes all required fields
- [ ] File format is valid JSON and parseable
- [ ] Works across all gate phases

---

## Troubleshooting

### GATE_STATE.json not created
- Verify gate is enabled in surge.toml
- Check that GateManager.trigger_gate() is called
- Check file permissions on specs directory

### State not restored after restart
- Verify GATE_STATE.json still exists
- Check that new GateManager uses same specs_dir
- Verify JSON is valid (use `jq` or JSON validator)

### Timeout not working across restart
- Verify GateManager created with timeout: `with_timeout()`
- Check that triggered_at timestamp is correct
- Calculate elapsed time manually: `current_time - triggered_at`

### Decision not persisted
- Check that record_decision() was called
- Verify DECISION.json was created
- Check GATE_STATE.json for decision field

---

## Implementation Notes

### Current Implementation
✅ **Implemented:**
- GATE_STATE.json creation and persistence
- trigger_gate() records initial timestamp
- load_gate_state() reads persisted state
- record_decision() updates state with decision
- Timeout tracking across restarts
- Decision persistence (Approved, Rejected, Aborted)
- Multiple restart support

### File Structure
```
.auto-claude/specs/{spec-id}/
├── GATE_STATE.json      # Persistent state (survives restarts)
├── DECISION.json        # One-time decision (consumed on read)
├── HUMAN_INPUT.md       # Rejection feedback (if rejected)
└── spec.md              # Original spec
```

### Key Behaviors
1. **GATE_STATE.json** is the source of truth for persistence
2. **DECISION.json** is written by CLI/UI and consumed by orchestrator (one-time read)
3. Timeout countdown uses `triggered_at` from GATE_STATE.json
4. Multiple GateManager instances can read same state (concurrent safe for reads)
5. First instance to read DECISION.json consumes it (writes are not atomic)

---

## Future Enhancements

Potential improvements for gate persistence:

1. **Atomic decision writes** - Use file locking for concurrent writes
2. **Decision history** - Keep log of all decisions (not just latest)
3. **Resume checkpoint** - Save full pipeline state, not just gate state
4. **State validation** - Verify GATE_STATE.json integrity on load
5. **Migration tools** - Handle schema changes in state files

---

## Integration Tests

Automated tests verify this functionality:

```bash
# Run full gate persistence test suite
cargo test --test gate_persistence_e2e -p surge-orchestrator

# Individual tests:
cargo test test_gate_state_persists_across_restart
cargo test test_approval_after_restart
cargo test test_rejection_after_restart
cargo test test_timeout_persists_across_restart
cargo test test_multiple_restarts_before_decision
cargo test test_persistence_all_phases
cargo test test_gate_state_file_format
cargo test test_concurrent_gate_state_access
```

All 8 persistence tests should pass.

---

## Summary

Gate state persistence ensures pipeline gates are resilient to process crashes and restarts. The GATE_STATE.json file provides:
- Persistent record of when gate was triggered
- Timeout tracking across restarts
- Decision persistence (approval/rejection/abort)
- Support for multiple restart cycles

This feature is critical for long-running pipelines and production environments where process interruptions may occur.
