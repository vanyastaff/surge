# Isolated runtime-drop worker ownership test

Status: root approved this direction within the original settlement matrix. This document fixes the bounded test shape before implementation. No Cargo is authorized here. The real-pool history probe remains a separate fixture: this test must not use FeedPoolProbe or FeedPoolSlots.

Source: one adjacent `runs/reader/runtime_drop_tests.rs` test module, wired under cfg(test). Exact parent/child test name will be `runs::reader::runtime_drop_tests::isolated_runtime_drop_retains_actual_worker`. Parent invokes the same compiled test executable with --exact/--nocapture and a Command::env-only child marker. The test does not mutate process-global environment.

## Child ownership and events

The child's owner OS thread builds a real multi-thread Tokio runtime and real Storage in a retained FixtureHome. It creates a writer, appends one literal event, derives a feed, installs the existing AfterCheckout gate, and moves one complete read lease into the actual spawn_blocking closure. That closure calls the production bounded read method. The gate is reached only after an actual pooled connection is acquired.

The owner thread waits for gate entry, then awaits the actual writer seal and actor join; the receipt must equal one. It drops the feed. The worker now holds the only remaining writer lease and per-run reader-pool owner. The gate itself contains only channels. The owner thread transfers to the observer only Storage, RunId, gate and seal receipt; Storage owns no strong run WriterLease and no per-run reader pool. No extra reader, feed, probe, slot guard or connection is transferred or kept by the owner thread.

The blocking closure keeps its complete read owner through query completion. After the gate releases, it returns the exact actual rows through a dedicated std channel, after explicitly dropping the read owner. The async JoinHandle is deliberately dropped before the owner's actual Runtime is dropped: runtime teardown, not that handle, owns the wait for this existing blocking closure. No shutdown_background, shutdown_timeout, fake worker, or synthetic result is used.

The child observer uses its own separate runtime to attempt the actual Storage::open_run_writer before releasing the known blocked worker. Require exact WriterAlreadyHeld for the same RunId; this evidence must come from the actual worker owner, since writer seal/join and feed drop already happened. It then releases the channel-only gate, receives exact row sequence [1] from the real SQL result, and joins the owner OS thread after actual Runtime::drop returns. Thread-exit readiness flags or a quiet timing interval are not settlement receipts.

After actual worker result plus owner-thread join, use the observer runtime to reopen the writer, await its close, drop observer Storage/runtime owners, and run fallible FixtureHome cleanup. Any unexpected writer acquired during the earlier Busy check is closed only after worker release/join, then the test reports the captured failed exclusion assertion. This branch performs no new write.

## Failure finalizers and parent watchdog

The child collects assertions until after releasing its gate and actually joining the owner thread. A test-only finalizer owns the gate and thread handle; unwinding releases the gate before joining. Setup failure before controller transfer cannot become a success: bounded channel observation reports failure and the parent watchdog remains the final escape for a genuinely non-returning OS/SQL operation.

The parent owns the actual Child handle. It concurrently drains captured output, bounds child execution, and awaits the real process exit. Timeout initiates kill and then awaits exit; it is recorded as failure/UNKNOWN, never successful runtime or SQL settlement. No test retries or deadline inflation are introduced. A successful child emits a receipt only after its real SQL rows, owner-thread join, writer reacquisition/close and fixture cleanup all succeed. The parent requires both successful exit and that exact receipt.

This test establishes retained ownership during actual runtime teardown and release after the real worker stops. It is not a native Windows receipt until executed on Windows and does not establish unrelated detached-task settlement.
