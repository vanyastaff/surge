# Registry read-only inspection — narrow policy proposal

PROPOSED CONTRACT AMENDMENT; explicit human approval REQUIRED before any policy,
assertion or implementation changes. Owner validation: ACCEPTABLE as a concrete
proposal for independent review, not approved behavior or builder release.
Baseline77c5f128 plus current initial Substeps2/3 WIP. Canonical repair count5/5
stays EXHAUSTED; this is an initial feature contract fork, not a sixth repair/reset.
Only this new proposal is written. No Rust/tests/build/runtime/provider/Git action.

## Observed conflict and recommendation

Existing static_event_pages_read_latest_live_wal_and_keep_cold_files_unchanged
(project_history_tests.rs:197) currently fails its cold zero-path assertion only:
new origin/control verification opens home/db/registry.sqlite READ_ONLY; SQLite can
create registry.sqlite-wal/-shm even though no registry row is written. All event
paging/latest-live-WAL assertions pass. This is existing-signature real RED, not a
hypothetical test. The failure must remain recorded; no blanket TDD/pass claim.
Baseline static event paging opened only events.sqlite, not registry. The baseline
connection::read_only uses ordinary READ_ONLY flags; there is NO special cold/live
safe URI helper to reuse. inspection.rs:322 deliberately avoids immutable mode.

Choose a minimal explicit allowance for SQLite-owned registry coordination paths.
This preserves normal locking, actual live-WAL records and independent origin/
control verification. It changes the zero-path promise and therefore needs approval.

| Option | Assessment |
|---|---|
| Permit only exact registry WAL/SHM coordination companions | RECOMMEND: narrow policy/test/doc change; no new production mechanism. |
| Build a no-create VFS/observation owner or authenticated external origin protocol | Much larger design/platform surface; disproportionate to necessary-only habit continuation scope. |
| immutable/nolock, ignore live WAL, delete sidecars after reading, weaken all file checks | REJECT: stale evidence, weakened locking, mutation masking or lost durable-byte oracle. |

## Exact proposed read-only contract

Keep normal SQLite READ_ONLY opening and existing actual-close/native admission.
Inspection performs NO application SQL writes, migration, registry repair, checkpoint,
control/seed creation, directory creation or application cleanup. Surge never creates
companions itself. All four current source control/guard checks stay effective.
Missing/replaced controlled origin/control cannot downgrade to Legacy. New-controlled
shared readers and actual deletion-positive behavior remain required; Legacy reads/
history/pins stay available and its physical deletion still refuses as accepted.

The ONLY permitted newly observed paths are the exact companions of the checked
registry: home/db/registry.sqlite-wal and home/db/registry.sqlite-shm, created by
SQLite while normally reading that existing registry. No suffix/global-WAL allowlist.
All preexisting paths remain present; every preexisting durable DB/WAL/content byte
must remain identical. Preexisting SHM bytes are coordination metadata and may change;
its path AND original native identity must remain. Preexisting companions, including
SHM, may not be replaced with identical bytes or another inode/file ID. Every other
path/content invariant stays exact, including run
journals, run WAL/SHM paths, checkpoints, source files, private objects and controls.

A newly created registry WAL may contain NO frames: length must be0 (empty) or32
(valid SQLite WAL header only). Reject1..31, any length>32 and invalid32-byte header.
Validate header magic/version/page-size/checksum using fixed independent fixtures;
never equate "small file" with valid WAL. No committed or uncommitted frames allowed.
Pinned Cargo.lock uses libsqlite3-sys0.30.1; its bundled sqlite3.c defines WAL_HDRSIZE32
at65391 and frame header24 at65388. Any frame requires bytes beyond the32-byte header.
An SHM companion is SQLite coordination metadata; check its exact path and regular
non-symlink type, not mutable lock-byte hashes. For BOTH new WAL and SHM require
NOFOLLOW descriptor/native opening, regular-file type, expected owner, single-link
and existing platform security checks before any hash/content read. Preserve original
parent/name-to-held native identity checks before/after observations. For preexisting
companions capture device/inode (Unix) or volume/file ID (Windows) and require SAME
identity afterward, including SHM. Reject symlink/hardlink substitution BEFORE reading
a target; do not use path strings or successful hashes as identity. Native checks
are bounded test-observation extension, not a new filesystem framework/VFS. No
arbitrary regular-file exception or weakened Windows ownership/security semantics.
SQLite-created companions may persist after return; do not unlink them to simulate
zero side effects. The previous universal no-new-path claim is withdrawn ONLY here.

Logical registry rows remain unchanged by inspection. Existing WAL records must
remain visible, including origin/control newly committed in live WAL; retain exact
preexisting registry WAL bytes. Comparisons attribute changes to the read under a
fixture with no concurrent SQL writer, not arbitrary production concurrent writes.
A genuinely read-only filesystem lacking SQLite's required existing companions may
return its actual typed SQLite/native error; no universal successful no-create claim.
Missing/corrupt registry and replacement/unsupported schema retain truthful errors.

## Test-oracle safety and exact implementation closure after approval

Current files(root) in runs/inspection/project_history_tests.rs:5 uses std::fs::read
on live SQLite database files in the SAME process as active SQLite connections.
That opens/closes an extra descriptor: SQLite warns POSIX locks can be canceled by
ANY same-inode close, outside SQLite's deferred-close bookkeeping. Official basis:
https://www.sqlite.org/howtocorrupt.html#posix_advisory_locks_are_canceled_by_a_close
The oracle must not cause the concurrency defect it is supposed to detect.

Cold snapshots may raw-hash only after actual writer join, pool/manager/connection/
worker settlement is proved. For LIVE snapshots use an exec'd dedicated test helper
with no SQLite connections: independently enumerate/hash files and emit structured
facts on stdout. Do not just spawn a thread or fork without exec. Parent retains
original owners; child raw descriptor closes cannot clear parent-process locks.
The helper creates no files; hash all durable bytes, record SHM path/type without
reading lock bytes, retain current empty-lock-file handling. Await helper exit and
check status/decoding. Fixture writer is idle between intentional appends; no busy
checkpoint/append races are hidden by retries or loose hashes. Preserve the byte
oracle rather than replacing it with logical SQL snapshots alone.

Exact test changes: project_history_tests.rs files/assert_readonly helpers and their
callers; keep static_event_pages... pagination/seq/payload/liveWAL/cold assertions.
Separate exact allowed registry companion additions from all forbidden additions,
removed paths, preexisting durable mutations and invalid new WAL forms. Add fixed
negative oracle cases for other-home/other-name/run sidecars, extra path, existing
WAL change, invalid/truncated header and any framed WAL. Keep missing/corrupt/foreign
registry/page tests, immutable-bootstrap reference tests and all old durable hashes.

Cold oracle order is mandatory: capture expected logical rows while original
producer is alive; settle ALL owners; raw BEFORE facts and assert BOTH registry
companions ABSENT; perform actual inspected operation; raw AFTER facts BEFORE ANY
SQL verification query; only then verify logical rows. Otherwise a verification
query could create WAL/SHM before BEFORE and make the test vacuous. Assert allowed
SQLite-created empty/header-only WAL+SHM and unchanged DB/rows/run contents. Add
native same-byte replacement and symlink/hardlink rejection cases for WAL AND SHM;
SHM mutable-byte exemption never exempts identity/owner/link/security checks. Add
live registry WAL with uncheckpointed latest origin/control; assert actual newest
records/read admission, preexisting bytes unchanged. Keep controlled-loss refusal,
writer+64 readers, held-reader deletion exclusion, real positive new-run deletion,
cold retries and Legacy zero application mutation. No new skip/retry masks.

Production: NO opening flags/control checks changed by this proposal. If a genuinely
necessary production helper change emerges, root must review its concrete diff;
this contract does not authorize a VFS, custom locking, extra DB descriptor or new
registry/writer lifecycle. Review docs in runs/inspection.rs and relevant helper
rustdoc: replace broad no-path wording with this exact registry-only allowance.
After approval append cross-references to owning Stage2 amendment/task notes rather
than rewriting their accepted history. No new UI or endpoint. Existing history
support and same habit/all8/codex-acp/exact gpt-6.1-sol evidence remain mandatory.

Preserve original failure receipt, obtain focused tests and full affected gates after
approved edits, then independent required reviews. No current green/new-platform/
MSRV/native-provider claim follows from this document. Until explicit human approval,
policy/test changes remain HELD; seed work may continue only within its prior scope.
