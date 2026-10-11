# R09 — Frontend task workflow review

Scope: source review of surge-ui task_create.rs, task_detail.rs, work_items.rs, fleet.rs; daemon operation admission classification; release workflow and macOS bundle script. Git working tree was initially clean. crates/surge-ui/CLAUDE.md does not exist. Read desktop-ui-design-direction memory and Rust Studio help/review disciplines. No edits to repository. No build/test/UI smoke run (coordinator explicitly requested no cargo builds).

## Finding: frozen creation cannot be replaced before any RPC (P2 desktop)

Evidence: task_create.rs:129-137 freezes creation_submission/project before checking facade availability at 139. If no daemon is connected, line 140 returns with frozen original inputs. Subsequent submission skips validation/capture because creation_submission remains Some (93). Cancel at 75-84 only hides the form; fleet.rs:237-244 opens the same pending form. A user cannot change their mind before dispatch, or create in another project without restarting the desktop. The form remains editable even though those edits will not be sent. Failure path line 207 similarly preserves every error; there is no WorkItemRejected-only dismissal path, while existing tasks have one at task_detail.rs:1307-1317.

Suggested smallest correction: check facade availability before freezing a previously unsubmitted creation, retaining just the editable draft while disconnected. Keep freezing and retrying exact identity once an RPC is dispatched. Add a creation rejection marker and an explicit dismiss button only for a definitive WorkItemRejected response; never discard an uncertain dispatched creation. Acceptance: disconnected Create then edit/change project then reconnect creates using the newly accepted inputs; an uncertain RPC remains immutable and retryable; a definitive refusal can be dismissed with draft retained.

This is a source-supported scenario, not runtime reproduced. Current release workflow packages only surge CLI + daemon (.github/workflows/release.yml:65-71). It therefore does not by itself block that archive release; it does block claiming the desktop create flow is fully ready.

## Positive source evidence / remaining verification

- Existing task operations check selected item + expected version and confirmed matching details before freezing (task_detail.rs:1265-1305).
- Existing task retries clear rejection status before each dispatch so a later uncertain reply cannot inherit permission to dismiss (1336-1339).
- Project cache rejects stale scope and detail responses (work_items.rs:224-278), and global list scans empty project pages rather than treating them as EOF (394-431).
- Reviewed Start parses/validates TOML and limits it to 1 MiB (work_items.rs:361-389); daemon owns launch.
- No todo!/unimplemented! bodies found in surge-ui/src by text scan.
- No native render, keyboard, clean-install desktop launch, production desktop build, or packaged desktop smoke verified in this role. Release desktop artifacts are not included in current CI archive contract. scripts/bundle-macos-app.sh creates an ad-hoc signed local bundle, not a notarized distribution.

Verdict: source review NEEDS WORK for desktop creation usability; CLI/daemon archive release decision belongs to coordinator and actual gates.
