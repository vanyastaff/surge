+++
status = "accepted"
deciders = ["vanyastaff"]
date = "2026-10-05"
+++

# ADR 0020 — Opt-in managed MCP execution and recovery

## Status

Accepted direction; implementation and executable acceptance remain open.
Supersedes ADR-0014 decision 4 only for the future managed opt-in path.
Native MCP behavior and ACP runtime delegation remain unchanged.

## Context

A direct child exiting, a process group disappearing, or MCP cancellation does
not prove that descendants or previously dispatched external effects stopped.
The release acceptance run still has ten failing MCP lifecycle cases. Their
original executable, arguments, cwd and positive recovery requirements remain
binding; adding a different fixture does not close them.

The user accepted managed opt-in execution after primary-source research.
Current macOS 27 ARM infrastructure has no installed VM backend or guest and
approximately 6.1 GiB free disk space. Virtualization SDK availability alone
is not an executable backend. No backend was installed or provisioned.

## Decision

Managed execution requires two independent host-issued proofs for the exact
writer, invocation, immutable backend instance and ownership generation:

1. Admission is revoked and the entire execution domain is verified empty.
2. Every admitted external effect is durably settled. Unknown outcomes remain
   unresolved even after the execution domain disappears.

Containment must exist before the target executes any instruction. Linux
requires an actual cgroup-v2 domain with race-free placement and enforced denial
of migration, privilege, filesystem, descriptor and IPC escape. Windows needs
pre-execution Job assignment plus an enforced resource/capability policy; Job
membership alone is insufficient. macOS requires a supported managed VM
backend. Unsupported custom SBPL and process-group relabeling are rejected.
Availability or configuration flags cannot mint coverage evidence.

The backend denies direct access to external networks, host services, control
sockets and shared runtime state. Approved external effects pass through a
host-owned broker. Before dispatch, the broker atomically validates ownership
and records a durable operation intent. Stable operation identity survives
retries; it binds the request digest and service capability identity. Ownership
generation fences dispatch but does not replace the operation's deduplication
identity. Receipts and reconciliation bind the same operation. A remote service
accepting a request before a host crash creates ambiguity unless its actual
idempotency/status contract can resolve it; local journaling alone cannot.
Unresolved effects prevent cleanup confirmation, replay and automatic recovery.

Freeze backend/image/runtime identity, policy, approved filesystem mappings and
broker capabilities with the accepted MCP manifest. Preserve the authenticated
command, arguments, environment and cwd. A macOS executable cannot silently
become a Linux guest executable. Cold recovery validates the retained instance,
boot, volumes and policy; changed identities or missing evidence fail closed.
There is no fallback from managed to native launch.

A genuinely enforced local-only backend can be an initial capability. It does
not complete the external-effect broker or retire the original acceptance
requirements. MCP Tasks may help reconcile cooperating services; protocol
cancellation and task responses are not domain or effect settlement proofs.

## Current implementation scope

The user narrowed active work to macOS on 2026-10-05. Implement and verify
the supported macOS VM backend and host-effect broker first. Linux and Windows
backend work and runner checks are deferred; their missing receipts do not block
this macOS development stage. Existing cross-platform release evidence remains
historical, and this scope change grants no unverified platform support claim.

## Ownership and implementation gates

Reuse existing IDs, frozen input authentication, run event recording, launch
claims and retained child ownership. Core owns validated serializable contracts;
MCP owns the actual backend and retained domain handle; the orchestrator owns
admission and proof issuance; persistence owns durable frozen authority and
effect transactions. Existing process-group identities remain distinct from
managed domain identities. A backend trait without a backend is not delivery.

Before production code, demonstrate a runnable backend and review its concrete
syscall, descriptor, mount, IPC and privilege policy plus broker crash windows.
The current host cannot execute this gate. Required infrastructure for the
active scope is a provisioned compatible macOS VM with enough storage and a
supported signed/entitled manager for unchanged macOS transport acceptance.

Required independent acceptance evidence includes: zero target execution on
journal/admission failure; containment of setsid/double-fork descendants;
no recovery while a domain is live; no replay when a remote effect is unknown;
exactly-once settlement under the service's real deduplication contract;
stale-generation rejection; identity/image/path-map mismatch rejection;
retained-owner behavior after runtime and host loss; native safe refusal;
and all ten original MCP cases with their exact transport oracle preserved.
Public manifests and logs must exclude credentials and private request material.

## Consequences

The architecture decision is recorded without claiming implementation. Release
remains NO-GO. Supported backend provisioning, broker implementation, original
MCP acceptance and other native-platform receipts remain open. A future code
stage must complete the cross-crate freezing, spawn, journal and cold-recovery
path before any cleanup proof is authoritative.

## Primary sources

- [Linux cgroup v2](https://docs.kernel.org/admin-guide/cgroup-v2.html)
- [clone3 and CLONE_INTO_CGROUP](https://man7.org/linux/man-pages/man2/clone.2.html)
- [Windows Job objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Apple supported sandbox guidance](https://developer.apple.com/forums/thread/661939)
- [Apple container requirements](https://github.com/apple/container)
- [MCP Tasks](https://modelcontextprotocol.io/extensions/tasks/overview)
- [Safe retries and idempotent APIs](https://aws.amazon.com/builders-library/making-retries-safe-with-idempotent-APIs/)
