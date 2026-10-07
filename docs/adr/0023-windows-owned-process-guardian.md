+++
status = "proposed"
deciders = ["vanyastaff"]
date = "2026-10-07"
+++

# ADR 0023 — Windows owned-process guardian

## Status

Candidate architecture; owning lead considers the scoped design ACCEPTABLE for
independent review. Implementation is not approved. Native acceptance and complete
private authority remain prerequisites. This does not restore ADR-0020 or change
ADR-0021's GroupOnly coverage and interrupted-call non-replay contract.

## Context

Windows Jobs provide retained native process ownership. After daemon death, a Job
name is insufficient to prove settlement: a temporary kernel object's namespace
entry can disappear while references keep the object alive. Persisted creation and
kill-on-close intent are not completion receipts. Tokio/std child APIs also do not
return the retained primary-thread handle needed for exact suspended dispatch.

The required product behavior is daemon restart with verified settlement of prior
owned processes. The runtime already fails closed when identity or cleanup cannot
be established. Guardian failure may retain Unknown/attention; it must never permit
redispatch from an absent pipe or Job name. Existing positive cold fixtures must
exercise daemon death while the authenticated guardian remains alive.

## Proposed decision

Ship an ordinary bundled `surge-process-guardian` subprocess. It requires no
installation, service registration, elevation, privileged broker or external network
endpoint. Its narrow authority is the native Job and child handles for one ownership
occurrence. It never brokers arbitrary provider effects or grants domain containment.

Use an explicit versioned Windows guardian container identity. Freeze a canonical, private, versioned Surge installation authority; it is not a
hardware machine or kernel boot identifier. Missing/corrupt authority with live
journal occurrences refuses; replacement cannot adopt old receipts. Bind guardian and
child PIDs to actual retained creation FILETIMEs, the private endpoint and protocol
version, an authenticated ownership occurrence, and durable establishment. The
occurrence/guardian lease is not a kernel boot token. Do not force this record into
the historical Unix process-group-shaped representation or current kernel-boot
identity. Old Windows records without the capability decode as legacy unproven evidence.
Unknown guardian variants/versions refuse recovery without signalling or deletion.
Validated constructors preserve these distinctions during deserialization.

The cold proof is the original still-live authenticated guardian querying its actual
retained Job. Missing guardian, lost credentials, inaccessible or substituted IPC,
conflicting identity, or unconfirmed completion yields Unknown/attention. A WMI
LastBootUpTime diagnostic does not authorize old-boot absence: documented restart
time alone does not prove unique immutable boot identity. No undocumented boot ABI,
registry BootId or wall-clock-minus-uptime construction is selected.

Before child creation, daemon durably binds the exact guardian PID/creation and
endpoint and acknowledges that occurrence-bound child-creation authority. Helper
spawn/bootstrap alone cannot authorize a child; missing binding stays Unknown.

The guardian creates/configures its fresh Job first, then uses native CreateProcessW
with CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT and PROC_THREAD_ATTRIBUTE_JOB_LIST
for creation-time Job attachment. It retains both PROCESS_INFORMATION handles.
Unsupported attributes or incompatible ancestor Jobs refuse before creation.
No post-creation assignment fallback is permitted. Only an authenticated acknowledgement of durable establishment permits one resume
of that exact primary thread. Unexpected suspend count refuses dispatch and settles
owned work, reporting uncertainty if execution may have started.

On daemon disconnect the guardian fences the old lease and retains the Job. It may
terminate members proactively, but keeps its retained capability and completion
receipt until an authenticated recovery owner observes zero membership, durably
consumes settlement and acknowledges that commit. Normal shutdown follows the same
protocol and joins/reaps the guardian. Guardian death triggers fail-closed recovery;
kill-on-close remains a safety mechanism, not substitute completion evidence.

## Authority and protocol

Guardian-owned states are Preauthorized, Suspended, Resuming, Running, Settling,
Settled and Consumed. Unique resume authorization is consumed before ResumeThread;
duplicate requests join/replay one transition. Durable establishment acknowledgement
is not proof of actual resume. On response loss, query the original live guardian:
join Resuming, consume Running's actual result, or retry the original authorization
idempotently from Suspended. Never resume again from journal intent alone. Wrong
count/API failure enters settlement with possible-execution uncertainty; guardian
loss in any state remains Unknown. Receipt release requires matching durable
consumption acknowledgement. See the transition table in plan 004.

The complete protocol is specified in [plan 004](../plans/2026-10-07-004-feat-windows-process-ownership-plan.md):
launch intent → suspended establishment → durable acknowledgement → exact resume →
fenced settlement → zero membership receipt → durable consumption → release.

Authentication requires [plan 003's native private state-home capabilities](../plans/2026-10-07-003-feat-windows-private-preparation-plan.md),
creation-time DACLs, retained owner/object checks, local-only named pipes, exact pipe
server PID/creation binding and an occurrence-bound secret challenge. Initial capability handoff uses a retained anonymous bootstrap pipe inherited only
by the exact guardian through STARTUPINFOEX HANDLE_LIST; a numeric process-local
handle locator is nonsecret, while capability bytes never enter argv/env/logs.
First daemon authentication binds retained guardian PID/creation to pipe server PID
and an occurrence challenge; guardian authenticates client PID/creation and lease.
Provider HANDLE_LIST excludes bootstrap/control/Job/private handles. Recovery reads
the original credential through validated private objects, never a pathname-only
fallback. Monotonic leases fence old and
competing daemons. Duplicate operations replay the same receipt; conflicting data
refuses. A same-user provider remains outside a hostile-provider security guarantee.

Shared native owner code belongs in surge-process. Guardian owns process/thread/Job
handles and stdio settlement. MCP consumes standard pipe adapters without a Tokio
runtime; ACP consumes overlapped Tokio pipe adapters without blocking its reactor.
Inherited-handle lists expose only child stdio, never Job/control/private handles.
The adapters preserve exact program/args/env/cwd, framing, backpressure, redaction,
handshake deadlines and retained cleanup on registration/transfer failures.

Hooks and shell_exec are native producers too: they must use the same ownership
boundary and durable writer-before-effect contract. The full implementation includes
core identity/versioning, helper protocol, ACP/MCP adapters, persistence/recovery,
hooks/shell ownership and release packaging/provenance. No partially enforced user
setting or premature guard removal is permitted.

## Alternatives

- Missing named Job means empty: rejected; namespace lifetime does not prove member
  termination.
- Fake boot token, PID-only lookup or reconstructed primary thread: rejected; these
  cannot establish the required retained identity/capability.
- Installed service or privileged broker: not selected; introduces installation and
  security scope unnecessary for this candidate.
- Live-only ownership with every daemon restart refused: honest but insufficient for
  the required positive daemon-death recovery behavior.

## Consequences and review requirements

The helper can outlive the daemon and consume resources until recovery completes.
It must have observable retained ownership, bounded protocol operations and explicit
normal-shutdown settlement. It must not forget capability because a timeout occurs.
An inherited ancestor Job may prevent independent survival; detect unsupported
launch constraints and refuse rather than claim recovery coverage.

Independent architecture/API/security review must resolve exact new identity fields,
private credential publication, lease/receipt transitions and stdio adapter ownership.
Native oracles must cover all daemon-death cuts, guardian death, descendants after
leader exit, endpoint substitution, duplicate/conflicting recovery, adapter failures,
protected panic paths, exact provider session continuity and normal shutdown joining.
Explicit cuts include daemon death after helper spawn/bootstrap before durable exact
guardian binding, and guardian death immediately after CreateProcessW returns.
Disconnected stdio pumps use bounded buffers and sealed stream errors without
blocking Job settlement or inventing provider completion.
Only executed native evidence can establish Windows readiness.

## Primary evidence

- [Kernel object namespace lifetime](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/life-cycle-of-an-object)
- [Job ownership and lifecycle](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Returned process and primary-thread handles](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/ns-processthreadsapi-process_information)
- [WMI last OS restart observation](https://learn.microsoft.com/en-us/windows/win32/cimwin32prov/win32-operatingsystem)

- [Creation-time Job assignment](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
