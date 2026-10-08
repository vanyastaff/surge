# Role 02 — architecture release audit

Scope: canonical architecture, crate dependency manifests, core configuration I/O, daemon admission and queue drain. Read-only repository inspection; no cargo/build/test invocation (coordinator requested no concurrent builds). No source edits. This is an architecture lens, not a release GO verdict.

## Findings

1. P1 correctness candidate: queued runs can remain stalled after capacity becomes free. `crates/surge-daemon/src/admission.rs:163` calls `Notify::notify_waiters`, which stores no permit if there is no registered waiter. `crates/surge-daemon/src/server.rs:1291-1310` waits for notifications and registers a new waiter only after `drain_one_pass` completes. A completion between the final unsuccessful `pop_queued` and registration of the next waiter is lost; no timer or predicate check wakes that queue again. The same notification style is used by BootstrapAdmissionGuard drop at admission.rs:90. Existing admission tests exercise queue state sequentially (admission.rs:254 onwards) but do not exercise this interleaving. Requested validation: deterministic test that releases the final active slot before drain subscribes, then observes queued dispatch without another completion. Fix direction: retained single-consumer permit (`notify_one`) or a predicate/version loop with notification registration before state check. Reported to coordinator; runtime reproduction remains UNVERIFIED.

2. P2 architectural documentation drift: `docs/ARCHITECTURE.md:291` says event payloads are bincode, while `crates/surge-persistence/src/runs/writer.rs:204`, `:406`, `:497`, `:539`, `:611` serialize with `serde_json::to_vec`. This misleads backup, recovery and migration work. Correct documentation to JSON plus actual version migration contract; no runtime refactor necessary.

3. P2 documentation/boundary drift: `docs/ARCHITECTURE.md:304,317` and AGENTS.md describe surge-core as no-I/O/pure domain leaf. Actual production I/O lives in `crates/surge-core/src/config/io.rs:20,169,190`, `skill/plugin.rs:26`, `skill/scan.rs:128,189,249,334,427`. Dependency graph remains acyclic, but the published purity claim is false. For this release document the exception; extracting those modules is a future scoped refactor, not a release blocker.

4. P2 crate map omission: `docs/ARCHITECTURE.md:302-315` lists 12 members and omits surge-telegram although Cargo.toml includes it (workspace member line 17) and daemon depends on it (`crates/surge-daemon/Cargo.toml:45`). Add explicit cockpit ownership to the architecture map. Manifest audit also showed daemon/UI using direct path dependencies despite the root workspace-dependency rule; standardization is low-risk cleanup, not an observed functional defect.

5. P2 concurrency candidate, not yet established release blocker: `crates/surge-core/src/config/io.rs:189-190` derives temporary config filename only from PID, so concurrent saves to one path in a process share the temporary file. One rename may remove the other writer temp, causing errors or publishing a truncated/mixed config. Current call sites (UI app_state.rs:491,604; CLI init.rs:54) do not establish an observed concurrent path. Use a uniquely owned tempfile if this API is expected to support concurrent callers. Existing config/tests.rs has no save test (rg returned no matches). Security lens should also inspect symlink following at this predictable temp path.

## Non-blocking future work

UI editor and full replay are explicitly marked intent/in-development (`docs/ARCHITECTURE.md:271-278`). Remote profile trust is explicitly deferred. Do not implement these solely for release preparation. Large cross-crate extraction of core configuration/skill scanning is outside minimal release scope.

## Checks performed

- Read attachment requirements, Cargo.toml and canonical architecture.
- Inspected all internal dependency edges via `rg` on crate Cargo.toml files: no observed dependency cycle; Cargo resolution not executed.
- Searched core filesystem/network/process use and examined actual config save implementation.
- Inspected daemon admission state transitions and queue-drain caller, plus existing sequential admission tests.
- Compared event payload writer format with architecture claims.

Build, lint, tests, runtime reproduction: NOT RUN / UNVERIFIED. No corrected defect claimed. Priority recommendation: hand finding 1 to concurrency/reliability owner for reproduction and fix before release; documentation findings to technical writer; config tempfile finding to security/concurrency owner.
