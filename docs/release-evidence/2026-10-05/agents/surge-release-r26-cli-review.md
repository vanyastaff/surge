# R26 independent CLI final follow-up review

Scope read-only: current engine.rs production + watch tests, cli_run_lifecycle.rs, examples_smoke.rs, fault_injection.rs, new common/owned_flow.rs. No Cargo.

## Actionable finding

P1 at engine.rs watch_command default disk branch (250-255): follow_log_from only prints events and returns max seq, then watch returns Ok without requiring completed history. Failed/aborted/nonterminal/conflicting history therefore still yields exit0 for `surge engine watch ID` without --daemon. New require_completed_history is called only on --daemon RunNotActive fallback. This misses required disk outcome failclosed contract. Sent root and R05 before GREEN. Fix routing default disk branch through same gate and add actual direct-disk regression.

## Accepted behavior and fixture intent

Live Terminal Failed/Aborted now uses shared require_completed and propagates error/reason. Closed, lagged, StreamError failclosed retained; unsubscribe runs for both success and errors. Completed succeeds.

Disk gate requires exactly1 terminal, maps failed/aborted reasons, absent and any duplicate terminal (including conflicting) failclosed. Helper tests cover these but do not exercise default disk branch; hence finding above.

Owner fixture uses committed clean project source, flow relative within project, isolated home outside project, real retained daemon child and RAII kill/wait. No scope validator relaxation. Gate changes align with current daemon-owned run behavior: persisted HumanInputRequested + gate node + nonterminal before explicit stop and durable aborted result. Checkpoint fault now kills actual executing daemon (exit99), asserts receipt accepted, and replay checks exact impl_1 nonterminal committed state; original durability intent preserved. Examples onboarding remains init→describe→realrun→replay completion.

No other source correctness blocker found in requested diff. Runtime tests remain root-owned; existing RED logs read via R05 report source, not re-executed. Verdict NEEDS WORK until default disk route fixed and serialized tests pass.

## Final re-review closure

R05 fixed default disk branch to return require_completed_history(id).await after printing history. Earlier P1 source finding CLOSED. Both disk entry routes now share exact-one-terminal failclosed classification, failed/aborted reason propagation and completed-only success. No other source blocker found.

Added real CLI assertions within existing lifecycle tests: disk watch after each success/failure run must match expected exit; pending gate watch must fail with no durable terminal; explicitly cancelled gate watch must fail with aborted. This closes earlier routing coverage gap. Conflicting+duplicate record validation remains pure deterministic event fixture (appropriate no need corrupt real journals).

Read actual prior logs: `/tmp/surge-release-final-cli-watch-red.log` six tests4pass2fail live Failed/Aborted incorrectly Ok; `/tmp/surge-release-final-cli-disk-watch-red.log` old binary watched failed history exits0 before assertion; `/tmp/surge-release-final-cli-disk-watch-red-confirmed.log` direct disk watch prints RunFailed, coordinator separately confirmed old binary incorrect status0. Logs independently read; no Cargo executed by reviewer.

Source spec+quality verdict ACCEPTABLE. Green outcome unit7/lifecycle6/examples10/fault1 pending coordinator serialized execution; not claimed passed by reviewer. Review source closure does not substitute runtime gates.

## Final abort projection and owned StopRun contract review

Narrow query.rs RunCompleted|RunAborted branch correctly sets terminal=true; valid aborted history leaves failed=false, retains most recent active_node. Existing RunFailed branch unchanged. Added regression uses contiguous valid RunStarted→StageEntered(gate)→RunAborted history and compares canonical Done(Aborted) display plus terminal/failed/active-node facts. Actual `/tmp/surge-release-final-abort-projection-red2.log` independently inspected: prior code fails snap.terminal (1fail). Watch GREEN log independently inspected7passed.

Owned StopRun contract confirmed actual server.rs StopRun routes bound attempt to owned_flows::stop; that issues WorkItemCommand::Suspend and execute_control invokes engine.suspend_work_item. Revised gate test therefore waits for persisted RunSuspended, requires top-level terminal=false and view.terminal=null, and disk watch fails no durable terminal. This is real current contract, not weakened abort outcome assertion. Valid aborted outcome remains covered by query and watch tests independently.

Fresh log read precedes fresh replay when HumanInputRequested or RunSuspended found, removing stale pre-event replay ordering race; pending gate node and nonterminal assertions retained. No source correctness blocker. Source spec+quality ACCEPTABLE, final runtime closure remains coordinator full gate. No Cargo by reviewer.

Minor nonblocking documentation drift: RunStatusSnapshot rustdoc query.rs25/39 terminal event list omits RunAborted; implementation/tests now include it. Followup wording should distinguish nonfailed aborted terminal from successful completion.
