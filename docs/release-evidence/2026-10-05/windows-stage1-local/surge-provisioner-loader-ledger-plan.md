# Privileged provisioner: loader and partial-acquisition closure

Status: proposed source-only refinement of the accepted fixture 1/2 provisioner.
No native launch, compilation or acceptance claim. Existing review budget remains
unchanged. This document does not reopen the runtime-home authorization policy.

## Finding and primary evidence

The current helper bootstrap compiles with `Add-Type -OutputAssembly` and then
loads the output with `Add-Type -Path`. PowerShell v7.4.6 `AddType.cs` lines
1180–1220 shows output-only compilation closes its FileStream and loads the file
only when PassThru is requested; DLL loading at line 687 calls Assembly.LoadFrom.
[PowerShell source](https://github.com/PowerShell/PowerShell/blob/v7.4.6/src/Microsoft.PowerShell.Commands.Utility/commands/utility/AddType.cs).

.NET v8.0.11 `Assembly.cs` routes LoadFrom to Default.LoadFromAssemblyPath; its
byte-array Load overload creates an individual load context and passes only the
bytes to InternalLoad. Resolution of dependencies still reaches the default
context. [Assembly source](https://github.com/dotnet/runtime/blob/v8.0.11/src/libraries/System.Private.CoreLib/src/System/Reflection/Assembly.cs).

The corresponding native loader uses OpenImage(path) for path loading and
CreateFromByteArray for stream loading. This is source evidence for eliminating
the helper's path-backed image, not evidence that every Windows host will delete
the current file successfully. [CoreCLR source](https://github.com/dotnet/runtime/blob/v8.0.11/src/coreclr/vm/assemblynative.cpp).
Copies of these version-tagged primary sources are retained in
`target/ci-repair-evidence/windows-privileged-provisioner/primary-source/`.
The actual CI PowerShell/runtime versions and hashes must be recorded and checked;
the researched versions are not asserted to be the installed CI versions.

## Minimal loader change proposed

1. Keep the protected, provisioner-owned bootstrap directory and immutable source
   and dependency pins. Compile to the protected DLL with no PassThru. Do not
   invoke Add-Type on that DLL, LoadFile, LoadFrom or LoadFromAssemblyPath.
2. Open the DLL once with read access and no write/delete sharing. Read a bounded
   byte array (at most 16 MiB, exact length plus EOF) through that retained stream.
   Compute SHA-256 from that exact array and compare against the emitted-file
   receipt. Retain the stream through actual child settlement.
3. Use `Assembly.Load(byte[])` on that exact verified array; retain the returned
   assembly object, record its full name/MVID, require empty Location, and require
   the provisioner type's Assembly reference to be the returned instance. Reject
   preexisting Surge.NativeFixtures types. Do not introduce a custom dependency
   resolver or private assembly probing. Installed runtime/toolchain dependency
   identities remain checked/pinned, and actual dependency closure is audited.
   Empty Location alone is not the authenticity proof: retained input identity,
   exact byte hash and returned assembly/type identity are the proof chain.
4. Keep the helper artifact pin until all original child exits are observed. Then
   close pins and remove only the fixed authenticated bootstrap inventory through
   existing DELETE-capable descriptor cleanup. Treat a sharing violation or any
   other deletion failure as failed cleanup and preserve diagnostic resources.
5. Before privileged fixtures, run a separate non-privileged Windows loader
   acceptance probe: compile the exact helper, read/hash from retained descriptor,
   byte-load, retain the returned Assembly and exact provisioner Type, and invoke
   a pure Contract method. Close disk pins only after that first invocation;
   delete the exact compiled `Surge.NativeFixtures.dll` whose retained-descriptor
   bytes/hash were passed to Assembly.Load (not merely the `.cs` source). Then
   remove every remaining fixed authenticated bootstrap inventory file and its
   directory through exact original-identity descriptor cleanup. Require DLL,
   each fixed file and bootstrap directory all actually absent. Invoke the SAME
   method again through the SAME retained Assembly/Type references after full
   removal; require reference identity, full name/MVID unchanged and successful
   returned result. Record original DLL identity/hash, exact removed inventory,
   actual absence checks and both invocation results. Do not reload, recompile,
   reacquire the type by a global name or replace retained Assembly between
   invocations. A refusal is terminal; no
   retry, forced GC, relaxed sharing, unload assumption or skipped assertion.

An external bootstrap owner is unnecessary if this actual probe passes. It would
introduce another process, inherited ownership and settlement protocol and is not
part of this bounded change. If stream loading fails the native probe, stop and
review rather than silently switching designs.

## Partial-acquisition ledger design proposed

Current gap: Relative can create an object and then fail in Own/Inspect before
CreateTree records the path; PowerShell can also fail before appending its tree
entry. The native handle list is useful but not a complete mutation receipt.

- Introduce one typed Mutation record before each attempted creation: stable
  ordinal, kind, role, parent record/held parent, literal component, optional full
  diagnostic path, attempted=true, outcome=Pending, handle/identity optional,
  validated=false, cleanup state and errors. Preallocate/register it before the
  system call; no receipt relies on the caller receiving a return value.
- Relative receives the optional mutation record for CREATE-only operations. On
  actual synchronous success, attach the original handle immediately and set
  Created before any injected failure, inspection, descriptor validation or
  callback. Then inspect via that handle, attach identity, set Validated and emit
  the acquisition hook. Existing/open-only calls never claim creation ownership.
- A failing native call records Refused plus native status. A result with an
  ambiguous handle/final-status combination records Unknown and preserves the
  affected tree. The existing unexpected-pending fail-fast is not a clean failure
  receipt: external termination means UNKNOWN, never a PASS or cleanup claim.
- Native CreateTree/NewFile/CreateOrdinaryTree allocate a record before Relative;
  account/bootstrap/profile/process ownership follows the same register-before-
  next-fallible-step rule in PowerShell. Creation failure after a newly returned
  account SID still retains that exact SID; no name-based ownership inference.
- Finalizer reconciles native typed records unconditionally, even when the PS
  local tree variable was never assigned. A known validated object can be removed
  only by retained descriptor with original identity; incomplete identity/status
  preserves its narrow root and affected accounts/profile with an explicit reason.
  Successful sibling cleanup is still attempted. No global ACL/owner repair.
- Every mutation ends with one honest terminal ledger state: Removed with actual
  absent proof, Preserved with reason, Refused without creation, or Unknown with
  cause. A Pending/Created/Validated record without a terminal cleanup disposition
  forces final FAIL. No mutation can disappear because a return value was lost.
- Resource handles and original Process owners remain a separate closure ledger;
  resources are not cleanup-authorized solely because their acquisition was logged.

## Failure runner protocol proposed

A fresh no-profile PowerShell subprocess per fixed case, in a dedicated runner,
uses a reviewed manifest, exact candidate revision/helper hash and a literal
FaultCase parameter. Normal entry has no injection. One-shot hooks target a named
phase plus occurrence, not environment-controlled arbitrary commands. The child
ledger records requested case, reached hook, actual native outcome, every mutation,
all child exits and token-before/token-after. The outer runner requires actual
exit code 1 plus a terminal FAIL receipt; exit 0, absent hook, malformed receipt,
unaccounted mutation, changed parent token, or unobserved child treated as settled
fails the failure test. An injected failure is never a fixture acceptance PASS.

The reviewed literal fault manifest/protocol defines an independent expected
PREFIX for each case, rather than accepting whatever ledger the child returns.
For mutations specify stable ordinal, kind, role, parent ordinal/held parent
identity, literal component (or exact freshly acquired account SID role), and
required target hook phase/occurrence. Mutation ordinals must be unique and
contiguous from1; records cannot be reordered, omitted or duplicated. Target
record MUST exist even when failure occurs before creation returns, before Own,
before identity inspection or before the PowerShell variable is assigned. Its
native outcome/validation stage must agree with the fixed hook, and cleanup
terminal disposition must be Removed-with-actual-absence, Preserved-with-reason,
Refused-with-unambiguous-no-creation or Unknown-with-cause. A Pending/Created/
Validated target without terminal disposition fails. An absent row is never
interpreted as refused acquisition.

The manifest defines separate exact acquired-resource and child prefixes for
each hook: unique contiguous resource/child ordinals, required kinds/roles,
original process ownership where applicable, and actual observed child exits
before owner release. Hooks preceding all acquisition have literal expected
zero count; hooks after acquisition require the corresponding nonempty prefix.
Outer runner checks exact counts and required records against the selected case,
including reached target ordinal/kind/parent/component; missing, empty or
truncated mutations/resources/children arrays fail whenever the expected prefix
is nonempty. Unexpected acquisitions require separately reviewed case contracts,
never silent omission. `cleanup_all_independent_attempted` is supplementary,
not a substitute for prefix and terminal-state validation. Freeze literal prefix
definitions with actual hook placement and helper/script hashes before acceptance.

Required non-native runner/parser negative cases: missing entire array; empty or
truncated nonempty prefix; duplicate/gapped/out-of-order ordinal; wrong target
kind/parent/component; missing target before caller return; nonterminal target;
false child settlement; omitted independent cleanup disposition. Negative
fixtures use literal independently expected records, not output from the same
implementation that emits the live ledger.

Required fixed cases:

- bootstrap directory acquired, helper source copied, helper assembly emitted;
- each account SID acquired and profile observation acquired;
- launch root/admin-data/standard-data/ordinary tree created before return;
- candidate file created before write/identity/return;
- admin duplicate, restore duplicate, suspended process original handles acquired;
- each of five explicit child handle transfers (occurrences 1–5);
- bootstrap manifest written before resume; ready received before proceed;
- malformed ready binding causes cancel and no proceed;
- final receipt mismatch causes sticky FAIL and actual settlement;
- cancel action failure still attempts settlement of every other child;
- first/middle/last resource cleanup failure still attempts independent cleanup;
- native observation failure causes preserved failed hold, never closing original
  process handles. This case is coordinated by an external observer and cannot be
  made green by killing the holding owner. Actual eventual child exit is required;
  failure acceptance remains sticky.

The executable runner must reject unsupported case protocol before spawning any
accounts. It may select a bounded subset explicitly, but must report coverage as
that subset; all cases are required for complete failure-path acceptance. There
is no default CI workflow or native launch until independent source review.

## Bounded review refinement

Independent review of original plan SHA-256
`a3ac3c9d0070193b9c6b4e259a6a41a05f23a5cd74911d1214208657f961062f`
returned RESHAPE for ambiguous source-only loader deletion and vacuous acceptance
of missing/truncated ledgers by the current runner. Explicit compiled DLL/full
directory continued-invocation proof and independent literal per-case prefix
validation above address those design findings. This revised authored plan needs
a DIFFERENT independent reviewer before implementation; it is not self-certified.
Existing Stage1 counter remains3/3; no native/source5a/5b acceptance or fourth
runtime ACL repair is claimed.
