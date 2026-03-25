# Manual UI Gate Approval E2E Testing Guide

This guide provides step-by-step instructions for manually testing the UI gate approval flow end-to-end.

## Prerequisites

1. Rust toolchain installed
2. Surge project built (`cargo build --workspace`)
3. At least one ACP agent available on your system (Claude Code, Copilot CLI, or Zed Agent)
4. A test Surge project directory

## Test Setup

### 1. Create Test Project

```bash
# Create a test directory
mkdir -p /tmp/surge-gate-test
cd /tmp/surge-gate-test

# Initialize git repository
git init
git config user.name "Test User"
git config user.email "test@surge.local"
echo "# Test Project" > README.md
git add README.md
git commit -m "Initial commit"
```

### 2. Create Test Spec

Create `.auto-claude/specs/test-gate-001/spec.md`:

```bash
mkdir -p .auto-claude/specs/test-gate-001
cat > .auto-claude/specs/test-gate-001/spec.md << 'EOF'
# Test Gate Approval

Test spec to verify gate approval flow in UI.

## Requirements

- Create a simple hello world function in Rust
- Add basic error handling
- Add unit tests

## Acceptance Criteria

- Function prints "Hello, World!"
- Tests pass
- Code follows Rust best practices
EOF
```

### 3. Configure Gates in surge.toml

Create `surge.toml` with gates enabled:

```toml
[project]
name = "surge-gate-test"

[pipeline.gates]
after_spec = false
after_plan = true        # Enable planning gate
after_each_subtask = false
after_qa = true          # Enable QA gate

[agents]
default = "claude-code"  # Or your available agent

[[agents.pool]]
id = "claude-code"
command = "claude-code"  # Or your agent command
```

## Test Scenario 1: Approval Flow (After Planning Gate)

This test verifies the complete approval flow at the planning gate.

### Step 1: Start Surge UI

```bash
# From surge repository root
cargo run -p surge-ui
```

**Expected:** UI opens showing welcome screen or project dashboard.

### Step 2: Open Test Project

1. Click "Open Project" or use File menu
2. Navigate to `/tmp/surge-gate-test`
3. Select the directory

**Expected:** Project loads, dashboard displays with test project info.

### Step 3: Run Spec with Gate

1. Navigate to "Spec Explorer" screen (sidebar icon or keyboard shortcut)
2. Click on "test-gate-001" spec
3. Click "Run Spec" button

**Alternative via CLI (in separate terminal):**
```bash
cd /tmp/surge-gate-test
surge run test-gate-001
```

**Expected:** Pipeline starts executing, shows "Planning" phase.

### Step 4: Wait for Planning Gate

The pipeline should pause after the planning phase completes.

**Expected:**
- Pipeline status changes to "Paused" or "Awaiting Approval"
- Notification appears: "Gate approval required for test-gate-001"
- Gate approval button or link appears in UI

### Step 5: Navigate to Gate Approval Screen

**Option A:** Click the notification or approval button from dashboard

**Option B:** Navigate manually:
1. Click on the task in task list
2. Task detail shows "Gate: After Planning" status
3. Click "Review Gate" or "Approve" button
4. Gate approval screen opens

**Expected:**
- Gate approval screen displays
- Screen shows "Gate Approval" title
- Task ID badge: "test-gate-001"
- Gate type badge: "Planning" or "after_plan"
- Description: "Review plan before execution begins"

### Step 6: Verify Plan Diff Display

The gate approval screen should display three tabs:
- **Plan Diff** (active by default)
- **Code Changes**
- **QA Results**

**Verify Plan Diff Tab:**
1. Tab should be highlighted/active
2. Plan diff section should show:
   - Category rows (e.g., "Complexity", "Estimated Subtasks", "Files Changed")
   - Before/After comparison boxes
   - Visual differentiation (colors, borders)
   - Example data or actual plan data

**Expected:**
- Plan diff renders correctly
- Before/After boxes display side-by-side
- Text is readable and properly formatted
- No layout issues or overlapping elements

### Step 7: Review Other Tabs

**Code Changes Tab:**
1. Click "Code Changes" tab
2. Should show list of files to be modified/created
3. Each file shows:
   - File path
   - Status badge (A=Added, M=Modified, D=Deleted)
   - Lines added/removed count
4. Summary stats at bottom

**QA Results Tab:**
1. Click "QA Results" tab
2. Should show list of QA checks (if applicable)
3. Each check shows:
   - Check name
   - Status icon (✓ or ✗)
   - Pass/Fail badge
   - Optional message

**Expected:**
- Tab switching works smoothly
- All panels render correctly
- Demo data displays if no real data available

### Step 8: Click Approve Button

1. Return to Plan Diff tab (or stay on any tab)
2. Locate "Approve" button at bottom of screen
3. Click "Approve" button

**Alternative:** Use keyboard shortcut `Ctrl+Enter` (if implemented)

**Expected:**
- Button click registers (visual feedback)
- Confirmation message appears briefly
- Screen transitions back to dashboard or task detail
- Gate decision file written to `.surge/gates/test-gate-001.json`

### Step 9: Verify Pipeline Resumes

1. Return to dashboard or task detail view
2. Monitor pipeline status

**Expected:**
- Pipeline status changes from "Paused" to "Executing"
- Execution phase begins
- Agent starts writing code
- No errors or failures

### Step 10: Verify Decision File

In a terminal, verify the decision file:

```bash
cd /tmp/surge-gate-test
cat .surge/gates/test-gate-001.json
```

**Expected output:**
```json
{"task_id":"test-gate-001","approved":true,"timestamp":"1234567890"}
```

**Expected:**
- File exists
- Contains task_id, approved=true, and timestamp
- JSON is valid and parseable

## Test Scenario 2: Rejection Flow (With Feedback)

This test verifies the rejection flow with structured feedback.

### Step 1-5: Same as Scenario 1

Follow steps 1-5 from Scenario 1 to reach the gate approval screen.

### Step 6: Click Reject Button

1. Locate "Reject" button at bottom of screen
2. Click "Reject" button

**Expected:**
- Rejection feedback input form appears
- Text area for entering feedback
- Description: "Provide feedback for why this gate was rejected"
- Cancel and Confirm buttons visible

### Step 7: Enter Rejection Feedback

1. Click in the feedback text area
2. Type rejection feedback:

```
The plan complexity is too high. Please simplify the approach by:
1. Breaking down large subtasks into smaller chunks
2. Reducing the number of files to modify
3. Consider using existing utility functions instead of creating new ones
```

**Expected:**
- Text input works correctly
- Characters appear as typed
- Text wraps properly in text area
- Backspace/delete works

### Step 8: Confirm Rejection

1. Click "Confirm Rejection" button

**Expected:**
- Feedback is saved
- HUMAN_INPUT.md file created in spec directory
- Screen returns to dashboard
- Pipeline status shows "Rejected" or "Failed"

### Step 9: Verify Feedback File

```bash
cd /tmp/surge-gate-test
cat .auto-claude/specs/test-gate-001/HUMAN_INPUT.md
```

**Expected output:**
```markdown
# Gate Rejection Feedback

**Task ID:** test-gate-001
**Timestamp:** 2026-03-24 12:34:56 UTC

## Feedback

The plan complexity is too high. Please simplify the approach by:
1. Breaking down large subtasks into smaller chunks
2. Reducing the number of files to modify
3. Consider using existing utility functions instead of creating new ones

## Instructions

Please address the feedback above and re-run this phase of the pipeline.
```

### Step 10: Verify Pipeline Behavior

**Expected:**
- Pipeline should abort or transition to failed state
- Task shows "Rejected" status
- When pipeline is re-run, feedback should be injected into agent prompt

## Test Scenario 3: QA Gate Approval

This test verifies gate approval after the QA phase.

### Step 1-2: Same as Scenario 1

Set up project and start UI.

### Step 3: Let Pipeline Run to QA Gate

1. Start spec execution
2. Approve planning gate (or disable it in surge.toml)
3. Let execution phase complete
4. Let QA phase complete

**Expected:**
- Pipeline pauses after QA phase
- "QA Review Gate" notification appears

### Step 4: Navigate to QA Gate Approval

1. Click notification or navigate to gate approval screen
2. Screen shows "Gate Type: QA Review"

**Expected:**
- Gate type badge shows "QaReview"
- Description mentions quality assurance review

### Step 5: Review QA Results Tab

1. Click "QA Results" tab
2. Review QA check results:
   - ✓ Build Success
   - ✓ Tests Pass
   - ✓ Clippy Warnings
   - ✗ Code Coverage (example failure)

**Expected:**
- QA results display correctly
- Pass/fail badges are color-coded (green/red)
- Failure messages are shown
- Summary shows overall status

### Step 6: Approve or Reject Based on QA

**If QA looks good:** Click "Approve"
- Pipeline proceeds to merge phase

**If QA has issues:** Click "Reject" and provide feedback
- Pipeline aborts or re-runs with feedback

## Test Scenario 4: Cancel Rejection

This test verifies the cancel button works during rejection.

### Step 1-5: Reach gate approval screen

### Step 6: Click Reject Button

Rejection feedback form appears.

### Step 7: Enter Some Feedback

Type a few characters in the feedback field.

### Step 8: Click Cancel Button

**Expected:**
- Feedback input form disappears
- Returns to normal approval view
- Feedback text is cleared
- No files written
- Can still approve or reject again

## Keyboard Shortcuts to Test

| Shortcut | Action | Expected |
|----------|--------|----------|
| `Ctrl+Enter` | Quick approve | Approves gate without clicking button |
| `Esc` | Cancel/Close | Closes feedback input or returns to dashboard |
| `Tab` | Navigate | Cycles through tabs and buttons |

## Edge Cases to Test

### Empty Rejection Feedback

1. Click Reject
2. Leave feedback field empty
3. Click Confirm Rejection

**Expected:** Validation error or warning (feedback should be required)

### Multiple Rapid Clicks

1. Click Approve button multiple times rapidly

**Expected:** Only one decision written, no duplicate files or errors

### Navigate Away During Approval

1. Click Approve
2. Immediately click dashboard or another screen

**Expected:** Decision still written, navigation works, no errors

## Cleanup

After testing:

```bash
rm -rf /tmp/surge-gate-test
```

## Troubleshooting

### Gate approval screen doesn't appear

- Check that `after_plan = true` in surge.toml
- Verify pipeline actually reached planning phase
- Check UI logs for errors

### Approve button doesn't work

- Check browser/GPUI console for JavaScript/Rust errors
- Verify `.surge/gates/` directory is writable
- Check file permissions

### Pipeline doesn't resume after approval

- Verify decision file was written correctly
- Check orchestrator is watching for decision files
- Review pipeline logs

### Plan diff shows no data

- Verify spec actually generated a plan
- Check if demo data is displayed (expected for initial implementation)
- Review gate context generation in orchestrator

## Success Criteria

All tests pass if:

- ✅ Gate approval screen renders correctly
- ✅ Plan diff, code changes, and QA results tabs display
- ✅ Approve button writes decision file and resumes pipeline
- ✅ Reject button shows feedback input
- ✅ Rejection feedback is saved to HUMAN_INPUT.md
- ✅ Cancel button clears feedback and returns to approval view
- ✅ Keyboard shortcuts work as expected
- ✅ No errors or crashes during any scenario
- ✅ UI is responsive and usable throughout

## Reporting Issues

If any test fails, report:

1. Which scenario failed
2. Which step failed
3. Expected vs actual behavior
4. Screenshots or screen recordings
5. Logs from UI and orchestrator
6. System information (OS, Rust version, GPUI version)
