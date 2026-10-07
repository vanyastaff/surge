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
