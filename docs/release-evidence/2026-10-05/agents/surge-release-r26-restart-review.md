# R26 independent daemon restart fixture review

Read-only daemon_restart.rs review, no Cargo/source writes. Production unchanged; two real ignored tests explicitly invoke actual CLI restart.

Accepted new behavior: idle test asserts prompt<10s settlement with configured30s grace; delayed test SIGSTOPs exact retained initial Child, resumes after11s, requires restart wait>=11s plus original success/new distinct PID/live ping. Correctly replaces old wrong oracle equating idle graceful shutdown with mandatory11s sleep.

Delayed signal lifetime safe: retained Child remains unreaped while SIGCONT worker waits. Success joins worker before initial.try_wait (reap). Failure unwinds into Drop, sends cancel, joins worker, then kills/waits child. Thus original PID cannot be reused before its delayed sender is settled. Negative path cancels or joins in-flight worker before reaping. No production shim.

Inherited cleanup issue: Drop still raw SIGKILLs PID obtained from pidfile without verifying live identity. Fresh isolated home proves the test generated the file, but a stale successor file after detached successor dies does not prove current process at PID is owned; reused PID could be signalled. This code predates new diff but does not satisfy requested absolute negative-branch ownership contract. Recommended narrow correction: stop successor via scoped daemon IPC/home rather than kill raw pidfile PID (keep initial Child kill/wait); or explicit exact process-identity owner verification before signal. Sent root for R05 closure.

Source verdict: new delay-oracle and initial-child lifetime ACCEPTABLE; successor cleanup NEEDS WORK for strict unowned-PID rule. Runtime GREEN is coordinator-owned and not claimed by reviewer.

## Final cleanup re-review

Earlier successor signal finding CLOSED. All successor PID-driven kill calls removed; remaining pidfile read only asserts distinct PID. Normal healthy successor retains UnixStream through test; shutdown_successor sends actual shutdown request, checks shutdown_ok + request_id=1 correlation, bounds read/write1s and socket removal5s. Both tests assert shutdown result with unwrap. Drop best-effort retries via test-private IPC endpoint and reports failure instead of signaling unknown PID. Original retained Child kill/wait remains; delayed worker cancelled/joined first. No process PID from file is mutated.

No source correctness blocker found. Source spec/quality ACCEPTABLE. Independently read `/tmp/surge-release-frozen-restart.log`: actual ignored tests explicitly executed, 2passed0failed0ignored,13.82s. Reviewer did not execute Cargo. Scope limits: Unix-only real daemon fixture, uses bounded local IPC shutdown, not evidence for Windows restart. If shutdown itself fails during panic, Drop reports diagnostic and avoids unsafe PID cleanup; normal path requires shutdown success. Final report ACCEPTABLE with actual coordinator2/2GREEN evidence.
