---
title: Windows process identity and owned child lifecycle
status: guardian candidate; lead ACCEPTABLE; independent review pending
date: 2026-10-07
scope: native process evidence, pre-effect ownership, live settlement and cold recovery
---

# Windows process identity and owned child lifecycle

## Verdict and evidence

Pre-code lead verdict: **ACCEPTABLE as the revised guardian candidate**.
[ADR-0023](../adr/0023-windows-owned-process-guardian.md) records the proposed
ordinary bundled helper lifecycle and explicit versioned Windows identity.
Independent architecture, security and API review remain required before implementation. This plan does not authorize removing unsupported guards, claiming
Windows readiness, or changing ADR-0021's recovery contract implicitly.

PR89 Windows run37672477232, source14884ff, executed3636 tests:3586 passed,
50 failed,33 skipped. Four cold provider-route tests expect an established writer
container, but Windows process observation is unavailable. Five legacy MCP tests
time out awaiting their actual child recorder. The production chain explains the
latter: `engine/writer_coverage.rs::child_started` requires `observe_container`;
Windows observation refuses; `surge-mcp/src/connection.rs::spawn_and_serve` settles
the child and returns `mcp_child_observation_failed` before handshake. The child
can be stopped before its first recorder write. This is not evidence for longer
startup deadlines or weaker observation requirements.

## Existing contracts and affected closure

- `surge-core/src/execution_recovery/process.rs`: ProcessPlatform already includes
  Windows. ProcessIdentity validates positive decimal boot/start tokens for Windows.
  WriterContainer currently documents a process group and accepts any nonzero group;
  deserialization goes through its constructor.
- `surge-acp/src/process_evidence.rs`: Windows observation, group-state and probes
  remain unsupported/Unknown. Observation errors never prove absence.
- `surge-acp/src/bridge/worker.rs`: launches a Tokio child, applies Unix process-group
  ownership, performs handshake, later captures writer evidence, and force-cleans
  through kill_owned_child. Handshake failure has separate retained reaping paths.
- `surge-acp/src/bridge/lifecycle.rs`: retained subprocess waiter owns settlement;
  graceful-close timeout and force-close must carry the native ownership capability.
- `surge-mcp/src/child_settlement/{mod,tracker,worker}.rs`: plain standard child is
  retained by a runtime-independent owner. Pipe-registration and transfer failures
  already have fallback settlement; native ownership must survive those paths.
- `surge-mcp/src/writer_observer.rs` and orchestrator `engine/writer_coverage.rs`:
  pre-effect intent and actual child establishment precede initialization dispatch.
- Orchestrator stage/session events, persistence journal hydration and daemon cold
  recovery consume the recorded identities; update their validation together.

Keep dependency direction: pure identity types in surge-core; shared OS child
ownership can belong in the existing std-only surge-process crate if its required
Windows dependency remains target-scoped. ACP and MCP must not depend on each other
or acquire database authority. A native capability grants no accepted-input, provider
session, registry, external-effect or verification authority.

## Selected candidate: explicit guardian authority

Use an ordinary bundled `surge-process-guardian` subprocess, with no installed
service, elevated account, privileged broker or external network endpoint. The
helper retains actual Job/process/thread handles across daemon death. It does not
execute external effects on the provider's behalf. ADR-0020 stays superseded;
ADR-0021's GroupOnly and interrupted-call non-replay rules remain authoritative.
This internal lifecycle is a proposed architecture decision, not implementation
approval or a claim that surviving daemon death guarantees surviving guardian death.

### Identity and private authority

Introduce a versioned `WindowsGuardianContainer` record rather than placing a
Windows PID in Unix `group` or inventing the current `ProcessIdentity.boot` token.
It binds canonical local host/install authority, guardian PID and exact creation
FILETIME, private endpoint locator, unique ownership occurrence, selected child
PID/creation FILETIME, protocol version and establishment sequence. A random
occurrence is a capability identifier, never a kernel boot identifier. Live recovery
must authenticate the original guardian and its retained handles; persisted data
alone cannot manufacture that authority. Historical Windows process-group-shaped
records remain unproven and require attention, with explicit decoding/migration.

Freeze a canonical installation authority as a versioned random 256-bit identity
created once in plan-003's retained private state-home namespace. Its source is
explicitly the Surge installation, not machine hardware or kernel boot identity.
Persist the identity with authenticated format/version and descriptor-owned atomic
no-replace publication; validate owner, DACL, native object identity and complete
bytes on every opening. Never silently replace a missing/corrupt authority while
journaled guardian occurrences exist: refuse and retain attention. An explicitly
new installation uses a new authority and cannot adopt prior guardian receipts.
Moving/restoring the state home preserves authority bytes but still requires actual
private namespace and live guardian authentication; a copied file grants no handle
or process ownership by itself. Authority replacement invalidates all old bindings.

The wire record is a tagged/versioned WindowsGuardianV1 container. Its installation
authority, occurrence, guardian PID/creation FILETIME, child PID/creation FILETIME,
endpoint and protocol version are mandatory; establishment sequence binds the
journal. Deserialize through validated constructors. Historical Windows group-shaped
records decode as legacy unproven evidence, not V1; unknown variants/versions refuse
recovery without signalling or deleting evidence. Do not make future records silently
look absent. The existing kernel-boot ProcessIdentity keeps its meaning unchanged.

WMI `Win32_OperatingSystem.LastBootUpTime` may be a versioned diagnostic observation:
Microsoft documents last OS restart time, not uniqueness or clock adjustment
immutability. It cannot authorize old-boot absence. No registry BootId or undocumented
boot ABI is selected. Missing original guardian remains Unknown even after reboot.

Private authority depends on native state-home/security capabilities in
[plan 003](2026-10-07-003-feat-windows-private-preparation-plan.md). Guardian
endpoint/authentication records live under the retained private home namespace;
creation-time DACL and exact owner checks precede publication. Never repair unsafe
existing objects, expose keys in argv/env/logs, or remove unsupported guards early.

For first launch, daemon generates an occurrence-bound authentication capability,
publishes it descriptor-owned in that private namespace before helper spawn, and
hands it to the exact guardian through a retained anonymous bootstrap pipe. The
helper's STARTUPINFOEX HANDLE_LIST inherits only its required stdio and bootstrap
read handle. A numeric process-local handle locator may appear in argv; capability
bytes never do. Guardian reads a bounded versioned bootstrap frame, authenticates
installation/occurrence, then closes that handle. The provider's separate HANDLE_LIST
contains only provider stdio; bootstrap/control/Job/private handles are excluded.
If bootstrap handoff fails, there is no provider dispatch and helper cleanup remains
owned. Do not substitute a path-based plaintext credential read or inherited env.

Use a local-only named control pipe. Initial daemon authentication checks the exact
retained guardian PID and creation time against the pipe server process before an
occurrence-bound challenge-response proves possession of the bootstrap capability.
The guardian likewise checks client PID/creation and capability before accepting the
lease. Subsequent recovery retrieves the original capability only through plan-003
private object validation and verifies the original guardian process/endpoint again.
Authenticated protocol frames bind installation, occurrence, version, operation and
lease. Remote clients, changed process identity and conflicting frames refuse. This
is owner-scoped authentication, not hostile same-user provider containment.

### Native launch and retained stdio

The shared std-only native owner belongs in surge-process, below ACP and MCP.
Use `CreateProcessW` with explicit application/structured argument encoding,
`CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT`, and a bounded inherited-handle
list. Create/configure the fresh retained Job first and supply it through documented
`PROC_THREAD_ATTRIBUTE_JOB_LIST`: the child is attached at creation, not through a
post-creation AssignProcessToJobObject window. Unsupported attribute/platform or
incompatible ancestor Job refuses before child creation; there is no post-assignment
fallback. Guardian death immediately after CreateProcessW returns must not leave an
unassigned suspended process. Retain the returned
`PROCESS_INFORMATION.hProcess` and `.hThread` exactly once as owned handles;
never reconstruct the primary thread through a snapshot or resume a PID-selected
thread. Validate process identity/liveness through retained handles. Before creation,
forbid both breakaway modes and configure kill-on-close on the fresh Job;
reject substituted objects, and retain GroupOnly coverage.

Guardian owns the child's three stdio pipe ends and settlement. ACP and MCP receive
separate transport endpoints: MCP adapts them to its runtime-independent standard
Read/Write ownership; ACP uses Tokio-compatible overlapped named pipes with explicit
handle transfer/registration. No blocking Read/Write runs on the ACP reactor.
All inherited handles are enumerated; parent transport ends, guardian control pipe,
Job, private handles and keys are not inherited. Native setup errors settle the
still-suspended child and retain the Job until actual zero membership is observed.
On daemon disconnect, stdio pumping cannot block ownership settlement: use bounded
buffers, then drop/seal an unavailable transport with an explicit stream error while
Job termination/membership observation proceeds independently. Dropped output does
not imply provider completion or success. Retain backpressure while connected.
Adapters preserve existing framing, exact argv/env/cwd, stderr privacy/redaction,
backpressure, handshake deadlines and transport-error classification. They must
not turn shell programs into structured ACP commands or serialize credentials.

### Durable dispatch, leases and settlement

1. Daemon persists launch intent with ownership occurrence, expected executable
   identity and private endpoint authority before helper/provider effects.
2. Guardian establishes its authenticated bootstrap/control channel but cannot
   create a child yet. Daemon verifies the exact retained guardian PID/creation and
   endpoint, durably commits that binding, and acknowledges its journal sequence
   with an occurrence-bound child-creation authorization. Only then may guardian
   create its already-Job-attached suspended child and report its establishment.
   Death after helper spawn/bootstrap but before guardian-binding commit leaves
   no child authority; recovery without that exact binding remains Unknown and
   never infers identity merely from an endpoint.
3. Daemon commits that receipt through the existing writer ownership journal and
   returns an authenticated acknowledgement bound to occurrence, receipt hash and
   journal sequence. Until acknowledged, the guardian cannot resume the child.
4. Guardian verifies the current daemon lease and exact acknowledgement, calls
   ResumeThread once, and requires previous suspend count one. Wrong count or
   failure settles the child; possible execution is reported as uncertain.
5. Daemon death invalidates its control lease. Guardian retains the actual Job and
   may proactively terminate the provider. It observes membership through retained
   handles and keeps the completion receipt/capability until recovery consumes it.
6. Recovery authenticates original guardian PID/creation and endpoint server PID,
   presents its occurrence-bound credential and acquires a monotonic fenced lease.
   It requests settlement idempotently and observes active membership zero through
   that guardian's retained Job before any replacement provider opening.
7. Journal consumption commits the exact settlement receipt. Only an authenticated
   acknowledgement of that durable commit authorizes guardian release/exit. Normal
   shutdown uses the same protocol and joins/reaps the guardian.

### Guardian transition authority and lost responses

The retained guardian is the sole owner of dispatch state:

| State | Proven authority and permitted transition |
|---|---|
| Preauthorized | Valid bootstrap/installation/occurrence and authenticated durable exact guardian-binding acknowledgement; no child exists. Create fresh Job and atomically Job-attached suspended child, then enter Suspended. |
| Suspended | Actual retained child/thread/Job identity; no resume performed. Only exact durable establishment acknowledgement under current authenticated lease can authorize Resuming. |
| Resuming | Guardian has consumed the unique resume authorization in its owned state before invoking ResumeThread. No duplicate request may invoke ResumeThread. Resolve the actual API result into Running or uncertainty/settlement. |
| Running | Exact retained ResumeThread result established previous count one. Query reports this state; termination transitions to Settling. |
| Settling | Retained Job owns termination and membership observation. Repeated requests join the same operation; nonzero/failed observation cannot produce a completion receipt. |
| Settled | Actual observed zero membership and occurrence-bound settlement receipt retained. Only exact authenticated durable-consumption acknowledgement permits Consumed. |
| Consumed | Receipt hash/journal sequence matched and original handles may release; normal shutdown joins the helper. |

A daemon journal recording establishment or resume intent never proves actual
ResumeThread completion. After acknowledgement response loss, reconnect and query
this exact guardian's state: Suspended permits the original idempotent authorization;
Running proves its recorded result; Resuming requires joining/querying the original
operation, never issuing another resume. Guardian retains operation IDs, payload
hashes and state results for the occurrence so duplicate acknowledgements replay or
join the same transition. Wrong suspend count/failure permanently prohibits another
resume attempt and enters Settling with possible-execution uncertainty. Guardian
crash in any state loses this live authority and yields Unknown/attention; disk
state cannot reconstruct Resuming/Running proof or justify redispatch.

Old/competing leases cannot resume, terminate unrelated occurrences, consume a
receipt or release handles. Lease acquisition and transition acceptance serialize
inside the guardian, monotonically advancing its retained lease epoch. Duplicate
operations return the same occurrence-bound result; conflicting payloads fail.
Guardian cannot reconstruct authority from a Job name after losing its capability.
Daemon death before establishment/ack/resume retains suspended/no-dispatch evidence
only when the still-live guardian proves it; otherwise uncertainty is explicit.
Incompatible ancestor Job, inaccessible pipe, lost credentials, changed executable
or installation authority, corrupt receipts and ambiguous completion retain
Unknown/attention. Kill-on-close is a safety measure, not completion evidence.
An already admitted external effect may still be outcome-unknown after settlement.

### Complete edit/dependency closure

- Core: versioned guardian identity/container, occurrence/lease/receipt validation,
  deserialization and historical migration; no fake kernel boot identity.
- surge-process: native suspended spawn, retained process/thread/Job handles,
  membership/settlement, stdio ownership, protected panic handling.
- Bundled helper binary: authenticated IPC state machine, fenced lease, retained
  settlement receipts and terminal acknowledgement; CLI/bootstrap spawn discovery.
- ACP: stdio adapter, handshake/open/resume/load, live evidence, prompt cancellation,
  lifecycle and failed-open reaping, runtime teardown and provider-session identity.
- MCP: standard stdio adapter, connection setup, observer, child settlement transfer
  and fallback failures, outcome-unknown interrupted calls and frozen restart.
- Orchestrator: writer coverage, hooks and shell_exec native producer ownership,
  final dispatch fences, recipe opening and no coverage upgrade.
- Persistence/daemon: plan-003 private authority, durable establishment/settlement,
  replay/cold reconciliation and fail-closed attention; cleanup consumption atomicity.
- Release: bundled executable lookup and provenance/notices/package membership,
  docs/diagnostics and native crash lifecycle fixtures.

Do not expose partially implemented Windows support. Native acceptance must cover
all ownership transitions and each adapter failure before guards are retired.

## Dependency-closed implementation stages

The guardian ADR remains proposed; architectural candidate approval is not maintainer
acceptance. Each stage requires independent spec and quality/security review. No
runtime unsupported guard is removed in a pure-type or native-probe stage.

### G1 — pure canonical identity and version contract

This is the first independently implementable unit after the staged candidate's
security/critic review. Files: new `surge-core/src/execution_recovery/guardian.rs`
and its export in `execution_recovery.rs`; adjacent unit tests only.
Use module-local `GuardianIdentityError`; do not extend RecoveryIdentityError. Do not change
run_event.rs, process.rs, migrations, storage readers or runtime adapters in G1.

Concrete data APIs (all private fields, validating constructors and serde try_from):

- `GuardianInstallationId`: exactly 32 bytes with the all-zero value rejected, canonical lowercase 64-digit
  hex on the wire. Pure validation does not claim cryptographic generation;
  plan-003 owner publication later supplies a CSPRNG source and private persistence.
- `GuardianAuthority`: installation ID plus frozen guardian executable ContentHash,
  format/protocol V1. This is public binding data, not an authenticated live capability
  or secret. Binary basename is fixed `surge-process-guardian.exe`; its lookup,
  descriptor authentication and byte hashing are runtime-owner responsibilities.
- `WindowsGuardianProcessIdentity`: positive u32 PID and positive u64 creation
  FILETIME, encoded canonically on the wire. Creation identity is not uptime/boot.
- `WindowsGuardianIdentity`: authority, existing ExecutionWriterId as ownership
  occurrence, guardian process identity, child process identity, and bounded local
  endpoint label. Require distinct guardian/child PIDs; exact immutable fields.
  Endpoint is exactly `surge-guardian-<installation hex>-<lowercase bare occurrence ULID>`
  (106 ASCII bytes, within 128); it is never a filesystem/UNC path, secret or remote
  pipe locator. Constructor derives it; serde validates exact equality. Native owner
  derives its local pipe path from this public label.
- `WindowsGuardianContainer`: explicitly tagged `kind=windows_guardian`, version1,
  identity and GroupOnly coverage. Constructor/deserialization cannot accept or
  manufacture CoveredDomain/Incomplete. Establishment journal sequence and lease
  are later receipt types; G1 must not pretend it already owns those authorities.

Frozen V1 wire contract:

```json
{
  "kind": "windows_guardian",
  "version": 1,
  "identity": {
    "authority": {
      "installation_id": "<64 lowercase hex digits>",
      "executable_hash": "sha256:<64 lowercase hex digits>",
      "protocol_version": 1
    },
    "occurrence": "<canonical bare ULID>",
    "guardian": {"pid": 100, "creation_filetime": 123},
    "child": {"pid": 101, "creation_filetime": 124},
    "endpoint": "surge-guardian-<installation hex>-<lowercase bare occurrence ULID>"
  },
  "coverage": "group_only"
}
```

Angle-bracket strings describe field formats, not executable test fixtures. Use
independent literal canonical values in tests. Reject nil ExecutionWriterId: its
existing `nil()` is a fold placeholder, not a real ownership occurrence. Existing
ID Display uses `writer-<ULID>` while serde uses bare ULID; strict guardian wire
uses the canonical uppercase bare ULID, and the endpoint uses its lowercase form.
Do not build JSON fixtures from Display or normalize noncanonical input silently.

GuardianAuthority's raw serde DTO reads the executable hash as a string, parses
existing ContentHash, and requires equality with its canonical `to_string()`.
Existing ContentHash accepts bare/uppercase hex generally; do not change that shared
API or permit those spellings through the guardian envelope. Typed constructor uses
the existing 32-byte digest; individual zero bytes and even an all-zero digest are
structurally valid hashes. Only the installation identifier rejects all-zero bytes.
Protocol/version fields are exactly numeric1; future values fail explicit validation.

Constructors take typed validated dependencies; getters expose named installation,
executable_hash, protocol_version, occurrence, guardian, child, pid, creation_filetime,
endpoint, identity, version and coverage values. All fields remain private and getter
access grants no live capability. A module-local GuardianIdentityError distinguishes
invalid installation, noncanonical wire ID/hash, unsupported kind/version/protocol,
invalid process/occurrence, identity collision, endpoint mismatch and invalid coverage.
No clock, file, process or cryptographic generation operation belongs in G1.

All DTOs deny unknown fields; reject unknown kind/version, blank/noncanonical
identifiers, zero PID/FILETIME or all-zero installation ID, conflicting process identities
and oversized/control-bearing endpoints. No `Deserialize` derive may bypass a
constructor invariant. Do not accept arbitrary unsigned values as boottokens or
coerce legacy ProcessIdentity into the new identity. No secret data is serialized.

G1 evidence: literal independent wire fixtures for exact V1 roundtrip, every invalid
constructor/serde boundary, all-zero/uppercase/truncated installation ID, future
version and unknown kind/fields, endpoint controls/remote syntax, guardian/child
collision, and attempted coverage upgrade. Existing process identity/group fixtures
must remain unchanged and pass; legacy Windows group-shaped JSON fails decoding as
WindowsGuardianContainer rather than gaining authority. These prove data validation
only; no native/live proof is claimed. Core strict lint/fmt/MSRV and full core tests
are required; no Cargo run before ownership/disk checks.

### G2 — explicit historical decoding and durable receipt contract

Affected closure: execution_recovery/process.rs, guardian.rs, run_event.rs,
core migrations/version constants and persistence runs/inspection, fold and writer
hydration. Introduce a tagged container evidence union with explicit historical
legacy variant; preserve existing Unix wire contracts through an explicit migration.
Legacy Windows records remain readable but unproven; future kind/version is an
error, never None/empty. Add occurrence/lease/resume/settlement/consumption receipt
constructors and pure transition validation. Authenticity and observations remain
owned capabilities, not DTO permissions. Independent API/serde review must approve
the migration before event writers change. Tests include historical journal fixtures,
unknown future decoding, contradictory establishment and response-loss transcripts.

### G3 — private authority and native guardian probe

Blocked on complete plan-003 private namespace/capability publication. Implement
surge-process retained native handles and helper bootstrap/control IPC in a scoped
native probe before provider wiring. Native tests prove inherited handle exclusion,
private first handoff, suspended marker absence, exact one resume, matching PID/
creation, state query after response loss, lease fencing, Job zero membership and
guardian death Unknown. No process/endpoint absence shortcut. Bundle discovery and
frozen executable authentication must be exercised against actual build artifacts.

### G4 — transport, producer and recovery closure

Implement ACP Tokio/MCP standard adapters, hooks/shell writer ownership, persistence
receipts, daemon recovery and normal shutdown joining as one dependency-closed
runtime stage. All pipe registration/transfer/failure and panic paths retain the
native owner. Explicit native crash cuts kill the daemon after helper spawn and
bootstrap but before exact guardian binding/child authorization, and kill guardian
immediately after CreateProcessW returns with creation-time Job membership. Neither
cut may authorize provider redispatch or leak an unassigned suspended child. Native
cold tests kill the actual daemon while guardian survives;
separate tests kill the guardian and require attention/no provider redispatch.
Retain exact provider session identity, one authorized turn and interrupted-call
non-replay. Only this complete stage may retire matching unsupported guards.

### G5 — release and independent acceptance

Add guardian executable to native packaging/provenance/notices and test bundled
lookup, revision binding and failure diagnostics. Run current Windows focused and
full tests/lint/MSRV, plus affected macOS regressions. Preserve source identities,
exact denominators and failures. ADR acceptance and release GO are separate explicit
claims; no pure-type/native-probe completion marks the whole lifecycle complete.

## Diagnostics and acceptance

Expose sealed public startup reason, dispatch boundary and actual child exit state
when legacy fixtures wait for receipts. Keep server/tool public names where already
authorized; do not dump raw command arguments, env values, private manifests or
unprotected stderr. Windows secure stderr capture remains disabled until the separate
private-object/DACL implementation proves its descriptor contract. Bootstrap and
archetype timeouts still require their actual durable event/gate cause; no deadline
increase is justified by the current evidence.

Required independent native oracles, before claiming Windows readiness:

1. Cross-process guardian/child creation identity and exact retained handle binding;
   WMI observation never authorizes boot absence; identity/permission failures stay Unknown.
2. Suspended child cannot create a marker before Job assignment and durable ownership;
   ambiguous thread acquisition/refused assignment never reports dispatch success.
3. Valid resume, wrong suspend count, thread-owner mismatch and retained dead handles;
   actual exit259 remains terminated.
4. Descendants survive leader exit only within the actual Job; force stop settles
   observed membership, with no false CoveredDomain claim.
5. Same-user retained query handle, inaccessible Job, precreated/substituted Job,
   changed limits and PID reuse fail closed.
6. Owner death before establishment, after establishment, before resume, during RPC,
   and during settlement; surviving authenticated guardian proves settlement; guardian loss stays Unknown.
7. Current and historical journal decoding; missing/corrupt/conflicting establishment
   cannot authorize signalling or provider redispatch.
8. Five legacy MCP positives establish actual catalogs; interrupted calls are never
   replayed. Four cold provider-route positives retain exact session identity and
   one provider turn under the approved ownership lifecycle.
9. ACP/MCP handshake failures, pipe preparation failures, transfer failures, runtime
   teardown and protected panic boundaries retain and settle native ownership.

Run native Windows focused RED/GREEN, strict Clippy, fmt, MSRV and full nextest;
run affected macOS regressions to protect existing Unix lifecycle. Record source
revision, exact test denominator and remaining failures. Cross-compilation and static
SDK inspection do not prove Windows execution. No tests are disabled and no advisory
setting counts as a passing gate.

## Primary sources

- [Rust Windows command extensions](https://doc.rust-lang.org/std/os/windows/process/trait.CommandExt.html)
- [Suspended thread execution](https://learn.microsoft.com/en-us/windows/win32/procthread/suspending-thread-execution)
- [Toolhelp snapshot and thread-owner filtering](https://learn.microsoft.com/en-us/windows/win32/api/tlhelp32/nf-tlhelp32-createtoolhelp32snapshot)
- [Handle-based thread process identity](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getprocessidofthread)
- [ResumeThread count semantics](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-resumethread)
- [Job objects and lifecycle](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Native system information API compatibility](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntquerysysteminformation)

This is a revised candidate plan. Independent reviewers must approve the explicit
identity, private IPC authority, stdio adapters and guardian receipt lifecycle before
changing production contracts. No implementation approval or release GO is recorded.

Additional primary sources:
- [Kernel object namespace lifetime](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/life-cycle-of-an-object)
- [Retained process/primary-thread handles](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-process_information)
- [WMI last OS restart observation](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-operatingsystem)

- [Creation-time Job assignment attribute](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)

## G2 refinement: append-only receipts and typed recovery projection

This is a durable-contract design candidate. G1 types are implemented; no guardian
runtime or native settlement authority is established by this appendix. ADR-0023
remains proposed. Owning lead and architecture reviewed the corrected partial
order as ACCEPTABLE; independent spec/security review remains required before code.

### Historical wire boundary

Advance the event envelope from version 21 to 22 for a distinct
`WindowsGuardianRecord { record }` event family. Preserve every existing
`ExecutionWriterEstablished`, `WriterContainer` and v1–21 payload byte and hash.
Never update append-only journal rows or recompute historical hashes. Existing
SQL event columns already contain the payload and schema version; no SQL row
rewrite is required. Every v1–21 decoder must explicitly reject the new event
discriminator before decoding through the current enum. Current identity
migrations otherwise accept a newly added variant under an old envelope.

The inner record has version 1 and a tagged kind; unknown versions, kinds and
fields refuse. Kinds are LaunchIntent, GuardianBound, ChildEstablished,
ResumeAuthorized, ResumeObserved, SettlementRequested, SettlementObserved,
SettlementConsumed, LeaseTakenOver and CancelledBeforeSpawn. Constructor validation applies to
each kind, not only the containing enum. Pure validation grants no live capability.

### Binding, ordering and settlement branches

Common binding includes run, invocation, writer occurrence, installation authority,
operation identity, positive lease generation and predecessor receipt hash.
LaunchIntent is genesis with no predecessor; subsequent accepted records require
the exact preceding hash. IDs are nonnil, integer fields have explicit ranges,
and occurrence/authority bindings agree. Define and test a domain-separated
canonical receipt encoding before implementing hashes; no credential enters
journal payloads. Protocol lease/query counters are not inferred from clocks.

GuardianBound carries exact guardian PID/creation and canonical endpoint, the
initial exact host identity and positive registry owner generation, without a
child identity. These initial fields establish the later takeover comparison baseline. G1's complete child identity cannot represent this phase.
Binding commits before child creation authorization; ChildEstablished carries
the G1 container and commits before resume authorization. ResumeObserved records
Started (actual previous suspend count one), Failed (native error), or Uncertain.
Authorization never proves execution; failed/uncertain observation cannot grant
Running authority.

SettlementRequested may branch from any bound active phase: no journaled child,
suspended child, authorized resume, failed/uncertain resume, or running. It fences
further creation and resume. Settlement does not require a successful resume.
SettlementObserved carries one of these explicit proofs:

- BoundJobEmpty: exact original guardian binding, actual zero-membership query
  generation, and sealed creation/resume fence generation. This does not assert
  that no child ever existed; it covers creation before establishment was
  journaled only when the original authenticated guardian retains the
  creation-assigned Job and observes it empty after fencing.
- ChildJobEmpty: the same evidence plus the exact established G1 child container.

CancelledBeforeSpawn is a separate terminal branch with positive producer-owned
proof that its still-held launch authorizer was atomically revoked before helper
spawn. Missing records do not supply that proof. Helper spawn without a durable
exact binding remains Unknown; empty Job or cancellation cannot be inferred.
Consumption references the exact accepted settlement proof and committed journal
sequence before helper handle release. Late establishment/resume after settlement
fencing conflicts.

Exact duplicates are idempotent. Differing duplicates, absent predecessors, wrong
occurrences, reordered transitions and stale leases create sticky conflict/attention;
later valid-looking records cannot erase it. Actual live origin authentication,
retained Job observation and fences remain G3/G4 requirements.

### Historical and mixed-run authority

Add a separate typed guardian projection to RunMemory. Classify evidence as legacy
Unix, historical Windows unproven, or guardian before examining legacy cleanup
shortcuts. Preserve Unix behavior. Historical Windows group-shaped records and
closed flags remain Unknown for active recovery unless an independently valid
authority contract supports them. Preserve completed-run historical reporting;
do not rewrite terminal UI status to implement an active admission guard.

SessionClosed, ExecutionWriterClosed, ExecutionWriterGroupStopped and
RunSuspended.cleanup_confirmed never settle guardian proof. An empty guardian map
is insufficient when historical Windows writers exist. Mixed runs require every
relevant occurrence to satisfy its own typed gate. Restart, continuation, retained
workspace reuse and replacement-writer admission use the same centralized decision.
Windows identities never pass into Unix group probing or signalling. G2 explicitly
refuses unimplemented guardian runtime paths; it cannot coerce them into legacy
observations.

### Full consumer closure

| Boundary | Files/modules |
|---|---|
| Types and projection | core execution_recovery/process.rs, guardian ledger module, run_event.rs, run_state.rs, migrations/mod.rs |
| Journal inspection and exhaustive matching | persistence runs/inspection.rs, writer.rs, run_writer.rs, views.rs; core run_report/mod.rs |
| Replay and ownership decisions | orchestrator engine/replay.rs, writer_coverage.rs, gate_answers.rs |
| Durable admission and suspension | persistence work_items/owned_flow/launch.rs, recovery_cycles.rs, control.rs |
| Daemon controls and admission | daemon tracked_run.rs, work_items.rs, work_items/start_preparation_barrier.rs |
| Later native producers | ACP bridge/worker.rs, bridge/session.rs, process_evidence.rs; MCP writer_observer.rs and child_settlement/ |

Current RunMemory legacy closure booleans and writer_coverage early returns cannot
bypass the new discriminator. Run-level suspension authority is also in scope:
owned-flow launch, recovery reservations, controls, daemon tracking and gate answers
currently consume RunSuspended cleanup booleans. Retain readable historical values
but independently gate active authority with typed occurrence evidence.

### Independent acceptance

Use fixed captured v1–21 fixtures to prove successful historical decoding and
unchanged original hashes/Unix replay. Every new guardian event under each old
envelope refuses; valid v22 fixtures decode while future envelopes, unknown kinds
and malformed nested identities refuse.

Independent transcripts cover missing intent/binding, all early settlement branches,
failed/uncertain resume, reordered records, stale generation, wrong authority, wrong
child, conflicting duplicate and consumption of another settlement. Exact duplicates
retain the same projection; conflict stays sticky. Insert legacy closure and true
suspension booleans before and after guardian records: none authorizes replacement.
Old-only and mixed Unix/guardian/historical-Windows journals check every occurrence.
Persistence inspection and replay must agree on independently specified fixtures;
malformed payloads produce decode errors, not omitted evidence.

G2 proves durable contracts and refusal only. Native termination/recovery acceptance
requires G3/G4 with the original authenticated live guardian, held Job handle, exact
query/fence generation, producer adapters and response-loss/crash transcripts.

### G2 lease advancement refinement

The original live guardian allocates leases. GuardianBound establishes lease 1;
LeaseTakenOver advances by exactly one with checked addition, refusing overflow.
A higher journal integer grants no authority. Takeover binds predecessor hash,
old/new exact host identities, old/new lease, authenticated caller and current
durable registry owner generation. Registry generation exceeds the previous one
and matches the current exclusive owner fence; it may skip values.

Takeover requires positively observed termination of the exact previous host,
preferably through its retained process handle, or its authenticated explicit
relinquishment. Missing PID/endpoint and unknown liveness refuse. Guardian operations
and takeover serialize. Once-only resume is occurrence-wide across leases; takeover
never repeats ResumeThread.

The current authenticated owner may query an old operation; an authenticated
previous owner may retrieve only its exact recorded receipt. Read-only results
identify both original operation lease and current guardian lease. They do not
mutate, resume, reexecute, advance, consume, or provide fresh Job evidence. Stale
mutations refuse before effects. Exact duplicates return existing receipts without
execution; differing duplicates create sticky conflict.

Takeover invalidates unconsumed prior-lease settlement as current authority. The
new owner needs a fresh zero-membership query after sealing creation/resume under
its accepted lease. Consumption binds current lease, barrier/query generations,
exact proof hash and durable sequence; old-lease acknowledgement cannot release
handles. A durably consumed terminal occurrence never advances lease or reopens
creation/resume. Its permanent sealed proof is not erased by host death; repeated
consumption acknowledgement/query is read-only and idempotent. The helper exits
only after durable consumption acknowledgement. Helper loss before consumption
remains Unknown; endpoint absence supplies no substitute receipt.

Fixed lease transcripts include:

1. Positively observed lease-1 host death, registry generation 7, lease-2 takeover,
   fresh sealed zero query and exact consumption: accepted.
2. Lease 1 to 3, wrong predecessor, mismatched registry generation or unknown
   old-host liveness: refused.
3. Lease-2 active with lease-1 resume/settlement/consumption: refused before effects.
4. Lost lease-1 resume response, takeover, read-only old-operation query: the
   original result is returned and resume still executed exactly once.
5. Lease-1 observed settlement, takeover, old acknowledgement refused, fresh
   lease-2 query and consumption accepted.
6. Conflicting duplicate takeover/operation: sticky attention.
7. Durable consumed proof before host death: terminal sealed evidence remains
   terminal, without granting another lease or permitting another child.
8. Observed but unconsumed proof before host death: prior observation alone
   cannot authorize handle release or replacement; fresh current-lease evidence
   or Unknown is required.

G2 refinement owning-lead, architecture, independent spec and security pre-code
verdicts: ACCEPTABLE for staged durable-contract implementation. LeaseTakenOver
is an explicit record kind; GuardianBound includes the initial host and registry
generation baseline. Native guardian authority remains unavailable until the
private capability and G3/G4 producer/observation closure pass.
