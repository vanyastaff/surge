# R20 critical unit coverage review

Read-only review completed; cargo not run because R05 owns builds.

## Priority proofs

1. Admission retained wake: fill active capacity, queue another fixed RunId, complete active run before first polling wait_changed, then bound wait_changed with tokio timeout and assert pop_queued returns exact queued ID. This models the real between-drain-and-wait race deterministically; no sleeps or mirrored implementation. Repeat for provisional BootstrapAdmissionGuard rollback. Owner must cover both notification producers.
2. FTS persisted v2 upgrade: use actual v2 schema fixture and source rows for all four categories; reproduce broken update/delete state before upgrade; reopen with current store; old tokens absent, replacement tokens present, deleted rows absent, untouched IDs/content intact. Use literal expected row IDs, not generated SQL traversal as oracle.
3. FTS failure atomicity: fail a later rebuild using a controlled malformed FTS table; failed upgrade must retain schema_version=2 and source rows and must not commit earlier trigger replacements. Recovery after removing fault must succeed.
4. Per-run migration: create on-disk historical run DB with old migration set, then open through production RunsStore accessor, verify new columns exist and prior events/stage rows remain; repeat reopen to prove idempotence. Calling migrations::apply directly cannot prove the production lazy/open path.

## Existing evidence gaps

- memory/store.rs existing tests cover inserts/search and v1-to-v2 backfill but need UPDATE/DELETE plus v2-to-v3 repair proof.
- admission.rs has capacity/FIFO/cancellation tests; no pre-registration retained notification assertion.
- migrations.rs tests direct migration helper, so existing green tests do not prove old run DB opening invokes upgrades.

No user data modified and no repository changes by R20 so far.
