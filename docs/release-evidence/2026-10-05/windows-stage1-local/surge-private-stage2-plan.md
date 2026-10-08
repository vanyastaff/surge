# Windows private namespace and restricted preparation: next implementation slice

Status: revision 2, owning architecture and independent security ACCEPTABLE for
implementation. Security accepted body SHA a129ae479ffdbe5f27e9b12733f8ff305878f9689bbc394f1d64a4c3272c3659;
this status-only update records that review. No implementation or Cargo execution
is included in this proposal.
Stage 1 is not declared complete. Its remaining native evidence and source gates
still apply. This follows plan003 and reuses its implemented backend.

## Scope and placement

Implement Stage 2 private object operations, then Stage 3 preparation/authentication
integration. Do not introduce another Windows security parser, a new crate, a VFS,
or a public native-handle API. Keep foundations in surge-persistence/state_home;
adapt WorkItemError only at the work-item boundary. Unix behavior stays intact.

Source placement:

- `state_home/windows/namespace.rs`: retained routing, narrowly scoped private
  open policy, original-object validation, private descendants and enumeration.
- `state_home/windows/native.rs`: checked native no-replace rename and disposal,
  complete flushing, ABI/status handling. Reuse existing security.rs unchanged
  unless a demonstrated missing primitive requires a reviewed extension.
- `work_items/owned_flow/private_files/windows.rs`: private namespace adapter
  implementing open, verify, read, publish and is_empty against those primitives.
- `work_items/start_preparation/secure_lock/windows.rs`: restricted retained
  preparation ownership using the same native creation/security primitives.
- `private_files.rs` and `secure_lock.rs`: final platform wiring and joint guard
  transition after prerequisite tests/reviews. Authentication formats and SQL
  authority remain in their present modules.

## Current missing interfaces

The existing native relative opener hardcodes FILE_SHARE_READ|FILE_SHARE_WRITE.
It cannot serve private publication by merely changing every caller's flags:
SQLite fences, control files, source descriptors and private objects have
different access/sharing contracts. Add an internal private-object opening policy
or equivalent narrow entry points; keep existing callers' semantics unchanged.

Needed internal operations (names are illustrative, not public API commitments):

- retain/open protected descendants with strict non-inheriting ACLs;
- open an existing private file for bounded read plus complete flush, retaining
  identity and owning a writable modified-parent barrier;
- exclusively create a protected random temporary private file, with write-through
  and DELETE authority;
- validate actual private-file descriptor security/type/link count/delete state,
  compare its named identity, and obtain descriptor length;
- `rename_no_replace(file, retained_parent, component)`;
- dispose only the retained original temporary descriptor and flush its parent;
- enumerate the retained private directory using checked native records;
- project retained directory/lock identities into the existing stable Windows
  preparation identity representation without changing its serialized bytes.

Private files use compatible read/write/delete sharing; the publisher needs DELETE
for rename. Existing readers need genuine writable handles for complete barriers.
The exact ACL protects confidentiality; sharing flags are not an authorization
mechanism. Same-user hostile mutation is outside the established Unix/Windows
contract. This sharing design still requires explicit independent security review
and native simultaneous publisher/reader evidence.

## Stage 2 private namespace

Open the existing protected runtime home using `Namespace::existing_parent(home)`,
then `work-items/private-flow-inputs`. Do not call `StateHomeOwner::open` on this
route: its unrelated runtime-directory creation violates existing-only reads.
The home retains its Stage 1 inheritable SQL policy; private descendants and files
have exact protected current-user-only non-inheriting security. `create=false`
must never create or repair a missing directory or key. Each newly created
directory requires its modified-parent durability barrier.
For `create=false`, use `existing_child` for both descendants. For `create=true`,
only those two descendants may be created using protected child creation; the
home must already exist. Missing intermediate components remain genuine absence.

Keep the complete original routing chain alive. Existing-only readers must obtain
the necessary writable parent flush handle without creating objects or replacing
the original identity. All routes remain local fixed NTFS and reject unsafe names,
ADS, reparse points and unsupported filesystems before sensitive effects.

`read(name)` verifies namespace and file security/identity, rejects files above
8 MiB, reads at most limit+1 bytes, and rechecks descriptor length plus held/named
identity after reading and around complete file/parent flushes. A successful
reader establishes its own durability barrier, including a name observed before
another publisher's final parent flush. No read-only flush fallback.

Map genuine absence to PrivateInputsRecoveryRequired. Unsafe routing/security and
corrupt contents must not be collapsed to absence: host_key uses the missing-key
branch to consider creation. Preserve NativeError discrimination and never expose
secret bytes in diagnostics.

`publish(name, bytes)` follows this exact state machine:

1. Validate bounded input and retained namespace.
2. Exclusively create a protected random temporary object with write-through.
3. Write bounded bytes and complete file flush.
4. Perform synchronous descriptor-based no-replace rename relative to held parent.
5. On success immediately mark Published, before any further fallible operation.
6. Flush the same renamed descriptor and modified parent; verify named identity,
   security, single-link state and stored bytes before returning true.
7. On collision preserve the final object, dispose and close only the retained
   temporary identity as specified below, then flush parent and inspect the
   existing final object before false.

Use explicit Unpublished/CleanupAttempted/Published states or an equivalent typed
representation. Prepublication errors attempt descriptor cleanup and preserve both
primary and cleanup/barrier errors. After Published, failures preserve the final
object and never trigger temporary disposal through the renamed descriptor.
Never close a temporary handle then unlink its pathname. A logging-only ignored
cleanup failure does not establish successful publication/collision handling.

Freeze cleanup as descriptor POSIX disposition **and explicit close before the
parent barrier**, not disposition alone. Use the SDK DELETE|POSIX_SEMANTICS flags
on the retained temporary handle; do not use ON_CLOSE or pathname deletion. The
owning temporary representation has one non-cloned descriptor in an Option,
separate from the retained parent. Consume that descriptor through one checked
native close after disposition succeeds; only after successful close perform the
final complete parent flush. Do not allow implicit field Drop after the parent
flush to implement this ordering. POSIX semantics matters even with another open
reader: Microsoft specifies visible-link removal when the POSIX delete handle
closes, while other handles may retain the data stream.
[SDK deletion contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddk/ns-ntddk-_file_disposition_information_ex).

Disposition/close/barrier failures retain the primary error plus cleanup phase
error and cannot return successful collision handling. A failed checked close
must not lead to a second unchecked close of a potentially consumed raw handle;
encode the consumed/uncertain state explicitly. Parent ownership survives the
whole cleanup attempt. No claim of completed cleanup is made on such an error.
On failed disposition, close the original owned handle while preserving the
disposition error; never find another object by pathname to remove. Published
objects are never passed to this cleanup operation.

`is_empty()` uses native retained-directory enumeration, not path read_dir. Open
an independently positioned enumeration handle with list-directory access;
identity-check it against the retained parent. Restart each call's cursor, bound
record offsets/name lengths and ignore only literal dot/dot-dot entries. A fresh
handle prevents concurrent calls sharing one mutable scan cursor. Verify the
namespace before and after enumeration; unexpected status/record shapes refuse.

## Stage 3 preparation integration

Current Windows Held is structural-only: default-security creation, no retained
READ_CONTROL security proof, and no restricted capability. Replace preparation's
Held internals with a nonserializable `RestrictedHeld` wrapper around one
crate-private foundational `NativePreparationOwner`. Keep the separate source
capture Chain implementation unchanged. The foundational owner lives beside
Namespace and owns, in destruction order:

- the actual locked File (no delete sharing, READ_CONTROL plus read/write access);
- lock component name and captured original volume/file identity;
- the complete Namespace from root through home/work-items/preparation-locks,
  including effective owner, security policies and retained parent barrier access.

Only this owner opens, locks, validates and flushes its resources. It exposes
crate-private acquire, verify, require_private and legacy_identity operations,
not raw handles, mutable files, a public constructor or a deserializable proof.
Work-item adapter RestrictedHeld maps errors and retains the owner directly.
Fresh stable and explicitly private acquisitions use this SAME protected owner;
stable acquisition does not itself replace the explicit require_private check.
There is no boolean-only restricted grant or second independently routed chain.

Use existing_parent(home), protected non-inheriting work-items/preparation-locks
descendants and protected non-inheriting lock creation. Existing objects receive
the same validation without repair. Acquire the OS lock, then validate actual
held and named identity/security, complete the writable lock-file flush and
lock-parent flush, and validate again before returning an owner. Each observed
preparation-directory publication also receives its own directory and parent
barrier before the identity can escape, including when adopting an existing name
whose creator may not yet have completed its barrier. Obtain writable barrier
handles only by relative reopen while the original chain remains held, compare
identity, and retain those handles. Newly created directories already require
this in Namespace::push; do not skip the corresponding adoption barrier on reuse.

`legacy_identity` and `require_private` revalidate and complete these file/parent
barriers on the SAME owner before returning. `verify` checks current security,
effective token and original held/named identities. An error returns no usable
identity/private capability and cannot be converted to Busy except the actual OS
lock contention result. The OS lock remains held through all checks and barriers.

Preserve existing Windows serialized identity: UTF-16 root/component names plus
volume/file IDs, lock name and lock identity; preserve PreparationLock's outer
stable-owned-flow-v1 wrapper. Do not replace an unsafe old object, repair ACLs,
manufacture a new identity, or silently rewrite accepted SQL receipts.

Important existing caller: `begin_owned_flow` chooses acquire_stable for a public
request, but `capture_inputs` later calls require_private when resolved host
defaults contain MCP/private inputs. Supporting restricted acquisition only in
acquire_private would leave this path broken.

Fresh stable preparation objects therefore need protected creation too, and
require_private must explicitly validate the SAME held stable chain and lock.
It must not release/reacquire ownership or infer private authority solely from
stable ownership. Existing unsafe objects refuse unchanged. The concrete
retained owner above supplies that validation; never use a boolean-only
validate(path) grant followed by unrelated path routing.

Source capture remains a distinct arbitrary-project capability. Do not impose
private-runtime ACLs or the private local-NTFS restriction on ordinary source
files merely because their routing helpers are nearby.

After Stage 1/2 evidence and independent reviews, enable Windows PrivateFiles,
acquire_private/acquire_task, and require_private together. Other unsupported
platforms retain their existing refusal. Keep authentication envelopes, HMAC,
KEY_MARKER, SQL private_key_creation_allowed, replay-before-filesystem work,
generation fencing and original-lock reclamation unchanged.

## Tests and implementation order

1. Freeze this capability/error/sharing proposal after independent review.
2. Add real outer Windows RED cases for explicit private Flow, task preparation,
   and public request with private host defaults. Record the actual unsupported
   boundary. Do not count a fixture or compile failure as behavioral RED.
3. Implement Stage 2 production primitives and native tests while public private
   guards remain closed. Exercise the real production module, not a mock backend.
4. Implement restricted preparation and joint integration; run unchanged existing
   authentication/lifecycle cases and the outer RED cases to native GREEN.
5. Add every native ignored test to standard-user CI with exact filters/receipts;
   run scoped host checks and native Windows checks, then spec review before
   quality/security review. No guard removal based on cross-compilation alone.

Native primitive oracles:

- Fixed-name publish/read; collision preserves original bytes/security; repeated
  enumeration; simultaneous two publishers and publisher/reader.
- Real ACL widening, hardlink, reparse, ADS, delete-pending and parent replacement
  probes. Parent rename while retained should fail on Windows, not mimic Unix
  rename success or skip the assertion.
- Competing temporary-name move/replacement while the original handle remains
  held; force cleanup and prove only original identity is disposed, substitute
  and final bytes untouched.
- Inject failure immediately after successful rename: final survives and cleanup
  is disarmed. Inject cleanup/parent-barrier failure: error remains observable.
- Pause immediately before the final cleanup parent barrier: an independent
  observer must already see the original temporary link absent, even with a
  compatible reader retained. Assert disposition -> checked close -> parent flush
  order and unchanged substitute/final objects; do not use eventual absence.
- Size limits and unsafe/missing classification; reader-owned durability.

Preparation/authentication oracles:

- Same-key Busy while held, Arc lifetime retention, killed owner release and
  original identity reacquisition; replacement identity cannot reclaim old SQL.
- Build the old Windows JSON tuple independently from SDK-observed original
  root/component UTF-16 names, volume/file IDs and lock name/ID, including the
  literal `volume`/`file` field names and stable-owned-flow-v1 outer wrapper.
  Compare exact bytes with both task and stable identities. New-encoder
  reacquisition equality alone is not the compatibility oracle.
- Inject lock-file and each required parent-barrier failure on creation and
  reuse: no identity or private capability returns. For stable-to-private host
  defaults, retain a competing-owner Busy oracle across require_private and
  verify that original SDK identity never changes.
- Explicit private inputs and public request with populated host-default MCP.
- Existing command/args/env/tools/sandbox/deadline round trips; opaque public
  projection; changed request, foreign binding/run, partial/reordered envelope and
  transplanted objects refuse.
- Established missing/corrupt key never regenerates; empty snapshots do no private
  I/O; exact replay still precedes filesystem work.
- Crash boundaries before/after write, flush, rename, parent flush and key marker
  preserve the original authenticated identity or fail closed. Process death is
  not power-loss proof; VM hard-stop evidence remains a separate release claim.

No tests may be disabled, unsupported behavior counted as success, or weaker
flags/permissions used as a fallback. Builders do not commit/push. Parent owns
the Cargo token and staged checkpoint sequence; CARGO_INCREMENTAL=0.
