# Windows readable PID control ownership

Reconstructed after host reboot from the previously reviewed plan (original SHA256 4e01421be96d8fb59663b11412bbb1ec8675103fad44e4d6fc23581bb04adcd0). This reconstruction is not the original hashed artifact. Root owning architecture and independent security pre-code verdicts were ACCEPTABLE.

Actual native RED at 04c: ten unique failures, eight CLI startup/status reads and two PID tests. Separately opened readers receive error33 while daemon owns a whole-file lock. Existing internal duplicated-handle bounded reads pass. Raw evidence is retained in ../windows-ci-04c4678.

Keep the same original protected DELETE-capable control descriptor, retained private namespace, exclusive creation, strict owner/DACL, flushes and identity-preserving removal. Only control files deny WRITE and DELETE sharing; all other relative opens keep READ|WRITE sharing. This excludes preexisting and new independent writers while allowing normal readers, whose own share mode admits the owner's WRITE/DELETE access.

Replace only control ownership's whole-file lock with synchronous LockFileEx EXCLUSIVE|FAIL_IMMEDIATELY on [2^32,2^32+1). It lies beyond the maximum1MiB payload plus oversize-read byte, does not extend the file, and overlaps the actual legacy Rust lock [0,u64::MAX). Only ERROR_LOCK_VIOLATION is contention. Unexpected pending aborts with the stack OVERLAPPED and all owners live. Already-locked acquisition revalidates security/identity before idempotent success; no stacked locks or unlock/reacquire.

Independent read-only legacy handles must open successfully BEFORE lock contention in both directions, so write-share refusal cannot mask a non-overlapping range. Test actual std whole-file locking, release and opposite acquisition, readable unchanged payload, no extension, preexisting/new writable handles, delete/rename denial, ordinary owner write and exact removal. Existing two PID reader regressions stay intact, with product read_pid added. A dedicated ignored standard-user wrapper verifies actor identity and non-elevation, runs both compatibility oracles and emits an exact mandatory launcher receipt.

Independent bounded source preflight accepted the initial source; supplemental pending-error protection and standard-user wrapper require incremental review. Formatting/diff-only checks do not claim native execution. Actual module cross-target lint and the full native14-probe run remain required, together with the ten existing CLI/PID regressions. Overall Stage1 and release remain open.
