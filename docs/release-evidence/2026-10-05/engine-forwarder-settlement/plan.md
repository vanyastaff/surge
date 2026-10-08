# Engine event-forwarder settlement — reconstructed design

Status: **PROPOSED; owning architecture and independent review required before implementation.** Reconstructed after reboot removed `/tmp/surge-engine-forwarder-settlement-plan.md`; not a byte-identical recovery. Original independent review counter remains **1/3**; reconstruction does not reset the repair budget. No Rust edits or Cargo execution were performed to produce this proposal.

Owning decision: **no new host-fatal behavior** for public completion abort or runtime shutdown. Normal confirmed completion and interrupted UNKNOWN settlement are separate contracts. The earlier fatal-supervisor proposal is superseded, not accepted.

## Evidence and scope

Current engine.rs::spawn_event_forwarder detaches a Tokio task owning RunWriter::subscribe_events. Its cloned RunReader owns a pool; runs/subscribe.rs polls forever. Storage drop/writer close does not terminate it. Public subscribe_outlives_storage_handle semantics are intentional and remain unchanged.

Parent previously observed Windows error 32 after run completion/pump join at engine_capacity_park_test.rs:285 on 04c4678. Its raw /tmp log was lost at reboot: recover it before citing retained native evidence. It does not isolate every remaining owner. The infinite pool chain is independently source-proven.

Normal completion means writer exit plus internal tap offering every persisted event through its sealed prefix, actual SQL job joins and release of its reader/lease owners. Broadcast receiver delivery/lag guarantees do not change. Hard abort is an unconfirmed interruption, not that completion. Actual workers retain resources until they stop; no full command queue may strand writer shutdown or an immortal forwarder. No cleanup retry or error suppression.

## Writer lifecycle and priority stop

Add internal WriterLifecycle::{Running, Sealed(EventSeq), Failed(WriterFailure)} over a watch channel separate from bounded WriterCommand. Only the actual writer actor publishes a terminal state. Lifecycle describes facts and grants no write capability. Closed channel still containing Running means owner loss/UNKNOWN, never successful seal.

Add a separate one-shot stop channel: Sender belongs only to RunWriter, not cloned RunEventRecorders. Actor uses a biased select with stop first, and checks pending stop before dequeuing the next ordinary command. Sender drop or explicit signal works with a full command queue. RunWriter Drop signals it; retire try_send(Shutdown) fallback. Drop neither spawns a finalizer nor blocks. When both stop and an ordinary command are ready, stop wins. Stop cannot interrupt SQL already running: the actor retains connection and WriterLease until it returns, then closes/drops the command receiver, fails queued replies, and closes the connection.

Priority stop publishes Failed/interrupted, even if a final prefix could be queried. A successful enqueue is not a commit: only successful append reply establishes committed data. Existing explicit close remains serialized and joins the actor. Do not set fully-closed/disarmed state after acknowledgment while actor join is pending: dropping an in-progress close/seal future must still signal priority stop. A join handle is taken once; all ordinary returned errors still await actual join. Actor errors/panic close lifecycle through Sender drop if explicit Failed publication is unavailable. No destructor-spawned task is needed.

This is a bounded RunWriter lifecycle correction, not a change to public subscription termination or external process ownership.

## Atomic seal and opaque feed owner

Add distinct serialized SealForReadDrain command. Actor recognizes it after prior dequeued commands, closes/drops receiver, rejects subsequent sends and queued commands behind Seal, and queries MAX(seq) on its own connection. Empty journal yields zero. Every successful prior append is included; no command behind Seal can return append success. Preserve SQLite durability policy: prefix receipt is not an additional fsync claim.

After querying, close the actor connection through existing checked ownership. On Windows genuine failed close retains connection and namespace and follows the already-existing protected fatal-close policy; no retry or weakening. Only after actual connection close publish Sealed(prefix), reply, and exit actor. Query/channel/close/join failure is not successful seal. Collect receipt and join failures independently; first failure cannot bypass join.

Expose a documented opaque RunEventFeed from RunWriter: private RunReader, same Arc<WriterLease>, lifecycle receiver, run ID. No constructor, raw pool/connection, mutable writer or cross-run conversion. Feed retains writer exclusion through final drain even after wrapper/actor close. A feed's bounded SQL worker clone separately retains reader AND the same lease until its real job finishes.

RunWriter::seal_for_read_drain(self) returns typed prefix/close result only after actor join. The already-created feed keeps the lease throughout. Its lifecycle Sealed prefix and joined seal result must agree. Existing public close signature stays compatible; successful Shutdown may publish a sealed prefix to terminate any feed, only after checked actor close. Unexpected exit publishes Failed or closes Running. Never derive target using current_seq before close (misses queued writes) or after releasing the lease (can include a replacement writer).

Ownership ordering: actor closes its connection before releasing its own lease; feed releases reader before its lease; each SQL worker releases statements/connection borrow and reader before its lease. **Actor exit must not wait for Arc uniqueness or writer-slot release**: feed intentionally retains that lease. Normal owner awaits actor exit, then feed drain, then final lease release. Waiting for lease uniqueness inside close would deadlock and is forbidden.

## Finite forwarder and complete SQL batches

Replace generic infinite stream task with private RunEventForwarder owning feed and returning Result<ForwardedPrefix, ForwarderError>. Execution owner retains its real JoinHandle and awaits it before confirmed completion. Independently observing writer lifecycle makes it terminate even when execution is cancelled and that join handle is dropped.

Factor existing subscription SQL/decoding into synchronous bounded helper with exclusive lower seq, inclusive upper, existing 256-row cap, checked positive i64 sequence domain, ORDER BY seq. Use <= upper, not overflowing upper+1. Public subscribe uses shared helper but retains existing polling, lifetime and decoding behavior; no schema/format changes. Expose only narrow synchronous feed read capability across the crate boundary, not SQL handles.

Actual spawned closure owns a feed read lease (reader plus WriterLease), not a path or weak witness. At most one batch per forwarder. Keep its JoinHandle outside transient select branches and await it completely before stop inspection, replacement or another batch. Never select away stream.next containing a spawn_blocking job. Statements are created/dropped within the worker.

Idle forwarder selects lifecycle change versus existing polling interval. During active SQL, finish/join batch first, then inspect latest lifecycle. Running permits empty batch and another interval. Sealed fixes upper prefix: reject already-delivered watermark above it, drain consecutive events through exactly it; empty batch below known prefix is an error, not infinite polling. Failed/closed-Running joins current batch then returns error without inventing target. Zero prefix exits without a phantom poll when no batch already exists. No broadcast receivers is not an error.

Preserve start/resume tap lower bound **zero**: current stream replays history on resume, downstream consumers apply their own watermark. Before seal use i64::MAX upper; same retained lease prevents another writer's additions after seal. After seal use fixed upper.

Runtime teardown can cancel forwarder while blocking SQL remains running. Actual closure independently retains reader/lease/native fences until SQL ends, even if async join handle drops. Cancelled JoinError is not proof that worker settled. Existing Windows per-connection/manager ownership remains unchanged. There is no forever-polling task after writer terminal lifecycle when runtime continues polling tasks.

## Execution and error surface

Create feed/forwarder only at execution handoff after fallible preparation; remove both early spawn_event_forwarder calls. Before spawning public completion, construct private owned RunExecution with RunTaskParams and forwarder control. It already owns a live writer before its first poll. Its Drop synchronously signals priority stop before resource fields drop, without aborting host. Before-first-poll completion abort therefore stops actor and finite forwarder while runtime remains available.

Refactor run_task::execute into owned execution returning ExecutionFinished { outcome, seal_result }. Preserve MCP cleanup and pending-suspension/event logic. Replace final log-only close with awaited seal. Drop remaining RunTaskParams fields, including estimator/readers, before returning. Small supervisor jointly polls execution and retained forwarder JoinHandle. Early forwarder error requests existing cooperative cancellation, preserves first error, but still awaits execution/seal. Execution completion still awaits forwarder join. Compare prefixes, drop remaining feed owners, then return. CancellationToken alone is not forwarder stop authority: writer terminal lifecycle is.

Add RunOutcome::SettlementFailed { original: Box<RunOutcome>, failures: Vec<RunSettlementFailure> } with finite typed failure categories and sanitized diagnostics; production constructor never nests this variant recursively. Keep JoinHandle<RunOutcome> and await_completion type. Never append fictitious RunFailed after sealing, report original as success or erase first error. Ordinary returned settlement failure still requires actual joins; hard cancellation instead yields existing cancelled JoinError/UNKNOWN.

Add typed daemon TrackingError for settlement failure. Durable tracking recognizes it before terminal confirmation, emits StreamError rather than Terminal/RunFinished. Task supervisor must not confirm Suspended or consume wake from it. Coordinate diagnostic Attention write with separate stale-reconciliation CAS work. record_terminal_status does not map SettlementFailed to terminal status or inspect original. Enumerate all RunOutcome consumers (facade, CLI, bootstrap, inbox/intake, UI) and explicitly reject success; non-exhaustive wildcards are not sufficient evidence. Persisted valid journal remains history distinct from settlement evidence. Admission release after normal result follows actual settlement.

## Startup, terminal resume, interruption limits

Audit all fallible paths after writer acquisition in prepare_run_start/resume and subsequent start_run_owned/resume. Use result-producing scope and on ordinary returned error await actual writer.close; combine original EngineError with close failure, drop all owned readers. Drop priority stop covers cancelled preparation but is not a normal-error settlement receipt. Register active-run state late, or remove exactly the registration acquired by that invocation on returned failure.

Already-terminal resume currently drops writer after spawning old forwarder. Use finite seal/drain path for historical replay and return synthetic completed handle only afterward; separate regression required.

Keep CLI run_lifecycle::stop_and_join's 10-second deadline and forced completion.abort behavior. Correct wording/results: awaiting cancellation confirms task interruption only, **not actual writer/SQL settlement**. Return UNKNOWN/unconfirmed failure; never success or permission to claim successful cleanup. Audit callers closing fixtures/runtime or releasing admission after abort. Only normal joined settlement gets stronger guarantee. Synthetic fixture aborts are not production ownership evidence.

With runtime alive, independent priority stop plus lifecycle-driven forwarder gives eventual real release without finalizer tasks or retries. During runtime teardown async actors may drop; actual running blocking closures keep complete owners until exit. If SQLite/OS never returns, those owners remain and release is UNKNOWN, not a reason to fabricate deadline success. Process death permits OS reclamation but is not an application-level settlement receipt. Preserve existing native close policy without broadening it to ordinary hard abort. No global claim about all unrelated detached tasks.

## File map and verification

Persistence: runs/{writer,run_writer,reader,subscribe,error}.rs, exports, adjacent tests. Orchestrator: engine/{engine,run_task,handle,error}.rs, private run_forwarder.rs and facade/consumer mappings. Daemon: tracked_run typed errors and consumer tests; coordinate work_items changes. CLI: run_lifecycle abort evidence wording, outcome mappings/tests. No guardian, private-file guard, provider protocol or runtime-home policy edits.

1. Actual baseline completed run: after legitimate Engine/Storage/readers/pump owners drop, independent pool-manager Weak still upgrades due to forwarder. This proves retention, not actual connection-close ordering; native final namespace/rename proof complements it.
2. Completed, failed, cooperative aborted, suspended, parked, start/resume errors and already-terminal resume settle before normal return. Public subscribe-outlives-storage preserved.
3. Gate inside actual SQL worker holding real reader/connection: completion stays Pending until release. Test finalizers always release and join before propagating assertions. No implementation flag as owner oracle.
4. Fixed expected sequence list (>256 rows, empty journal, resume replay, final event); append before Seal succeeds/included, recorder queued behind Seal fails. Writer reacquisition Busy throughout blocked final drain, succeeds only afterward.
5. Inject receipt/query/join/read failures plus missing/gapped prefix. Require typed original-plus-settlement failure, finite termination, joins, no successful terminal/suspension acknowledgment.
6. Fill command queue while actor gated, then drop writer while retaining recorder senders. Release actor; priority stop wins over queued work, replies fail, actor and forwarder terminate. Proves replacement for queue-full try_send leak.
7. Abort actual production completion before first poll and during blocked read. Join returns cancelled/UNKNOWN; actual worker-owned lease remains (reacquisition refused) until release, then actor/forwarder ownership settles. Observe test-only actual actor exit and pool/lease witnesses, no cleanup retries or arbitrary sleeps.
8. Controlled child runtime teardown with actual blocked worker and independent release channel: no false success or premature namespace/lease drop, worker exits after release. Parent watchdog awaits actual child exit. Actor/worker panic and closed lifecycle cannot strand infinite polling; native fatal-close is separate from ordinary abort.

Root coordinates builds. Owning acceptance, independent revised-plan review, actual RED, bounded implementation, focused GREEN/strict checks, independent spec then quality review required. Recover/rerun exact native Windows evidence; local/macOS success does not close Stage1 or release.
