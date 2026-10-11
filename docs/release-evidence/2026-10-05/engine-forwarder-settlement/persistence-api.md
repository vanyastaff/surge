# Persistence feed and writer settlement API contract

Implementation authority: accepted reconstructed plan `plan.md` SHA256 94153dc9e459a0d2ae1e62da138589334903430dbc0f187fc2a9c8f591252ab5 and actual baseline RED `target/ci-repair-evidence/engine-forwarder-baseline-red.log`. Root confirmed owning and independent acceptance despite the historical PROPOSED header. This file records coordination between persistence builder and engine/consumer lead before Rust edits.

Public exports through `runs`:

- `WriterLifecycle::{Running, Sealed(EventSeq), Failed(WriterFailure)}`; cloneable observation only.
- `WriterFailure::{Interrupted, ActorFailed}`; finite sanitized classifications. Only the actual actor publishes terminal state. Closed watch channel still Running is unknown owner loss.
- `FeedLifecycleError::WriterLost` for that closed-Running case.
- Opaque `RunEventFeed`, obtained only by `RunWriter::event_feed(&self)`; `run_id() -> &RunId`, `current_lifecycle() -> WriterLifecycle`, `changed(&mut self) -> Result<WriterLifecycle, FeedLifecycleError>`, `read_lease(&self) -> RunEventReadLease`.
- Opaque `RunEventReadLease::read_batch(&self, after: EventSeq, through: EventSeq) -> Result<Vec<ReadEvent>, StorageError>` is synchronous, exclusive lower/inclusive upper, checked signed SQLite sequence bounds and positive row seq, ordered, at most 256 rows. Reader and same writer lease remain inside the actual spawned blocking closure. No raw pool/connection or cross-run constructor is exposed.
- `RunWriter::request_stop(&mut self)` synchronously signals priority stop; this authority is absent from recorders and feeds. Engine lead confirmed this for unpolled owned-execution guards.
- `RunWriter::seal_for_read_drain(self) -> Result<EventSeq, CloseError>` returns only after actual actor join. Existing `close(self) -> Result<(), CloseError>` stays compatible. Receipt and actor join failures are collected independently, including a typed combined result when both fail.

Feed/read-lease field order releases reader before writer exclusion lease. No actor exit waits on Arc uniqueness or slot disappearance; the feed intentionally retains the same lease through its final drain. Storage constructor receives the separate stop sender and lifecycle receiver; only that constructor hunk is shared with the storage witness owner.

Actor priority stop uses a dedicated oneshot sender owned only by RunWriter. Explicit Drop notification and sender loss bypass a full command queue; biased selection checks stop before ordinary work. In-flight synchronous SQL completes before stop inspection. Queued replies fail after the command receiver is closed/dropped. Priority stop reports interrupted, never sealed.

Graceful serialized SealForReadDrain rejects work queued after it, queries MAX(seq) on the actor's own connection (empty=0), closes that actual connection using existing checked close policy, then publishes/replies with the prefix and exits. Query failure is not a prefix; join still occurs. The wrapper remains armed for priority stop until join finishes, including cancellation while waiting after acknowledgment. No new abort/fatal policy is introduced.

Engine lead confirmed these exact names and owns asynchronous batching/join, sequence-gap and sealed-prefix checking, finite lifecycle termination, supervisor/outcome integration. Public infinite subscription semantics remain unchanged while sharing only synchronous bounded SQL/decoding.

Validation remains pending until recorded command exits and independent review. No Cargo authorized at this contract stage; root coordinates one build process.
