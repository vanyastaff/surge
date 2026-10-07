---
title: Windows private preparation and authenticated inputs
status: accepted staged candidate; native design probe pending
date: 2026-10-07
scope: complete native Windows closure; no unsupported-test bypass
---

# Windows private preparation and authenticated inputs

## Verdict and contract

Pre-code lead verdict: ACCEPTABLE as a staged implementation candidate. Independent
critic, security and architecture reviews of the amended plan returned ACCEPTABLE,
as reported by the coordinating agent. Their constraints remain mandatory: exact
ancestor ACE policy, retained Storage/SQLite routing, compatible writable reader
barriers, descriptor-owned temporary cleanup and final-object preservation. This document is not release GO evidence. Native execution remains
required. No unsupported guard changes before the complete closure passes.

Windows stable ownership now uses retained native directory handles. That proves
object routing and exclusion, not private confidentiality. Restricted preparation,
private object storage, authenticated key establishment, production state-home
creation and test fixtures must be implemented together. Existing unsafe objects
must refuse without ACL repair, truncation or retry with weaker rights.

Preserve replay-before-filesystem work, original-owner identities, SQL generation
and transaction fences, exact frozen MCP transport, HMAC domains and established
key-marker semantics. A filesystem capability grants no provider, registry,
accepted-manifest or external-effect authority.

## Ownership and API boundaries

Implementation remains in surge-persistence. The shared owner boundary lives above
work_items in `state_home/windows/{security,native}.rs`, with `state_home.rs` exposing
the narrow retained home/directory owner to Storage and registry initialization.
Preparation locks and source capture adapt those primitives inside
`work_items/start_preparation/secure_lock/windows.rs`; private objects belong in
`work_items/owned_flow/private_files/windows.rs`. This placement is required to
avoid making foundational Storage depend on the work-item subsystem.

Suggested interfaces (not public API commitments; distinct capabilities):

```rust
struct EffectiveOwner { /* owned validated SID */ }
struct CreationSecurity { /* owned descriptor, ACL and SID storage */ }
enum DirectoryPolicy { Ancestor, StateHome, Preparation, Private }
struct StateHomeOwner { /* retained home chain and validated db/runs/daemon dirs */ }
struct RestrictedHeld { /* retained preparation chain, lock and owner */ }
struct SourceOwner { /* retained source chain and immutable source handle */ }
struct Namespace { /* retained directories, names, identities and owner */ }

impl EffectiveOwner {
    fn capture() -> NativeResult<Self>;
    fn protected_creation(&self) -> NativeResult<CreationSecurity>;
    fn validate_directory(&self, file: &File, policy: DirectoryPolicy) -> NativeResult<()>;
    fn validate_private_file(&self, file: &File) -> NativeResult<()>;
}
fn flush_complete(file: &File) -> NativeResult<()>;
fn rename_no_replace(file: &File, parent: &File, name: &OsStr) -> NativeResult<()>;
```

Security primitives use their own small typed native error. They must be runnable
as the actual production module in a native cross-probe without a fabricated
WorkItemError. Explicit adapters map NativeError into OpenError, PersistenceError
and WorkItemError at their owning boundaries; retain discriminating
failure categories, never secret bytes. No new crate is required unless the
production ownership map actually requires a shared lower-level dependency.

## Stage 1: security and state-home creation

Resolve the effective thread token first. Fall back to the process token only for
the documented no-impersonation-token condition; other errors refuse. Capture
TokenUser into owned, validated SID storage with GetTokenInformation, IsValidSid,
GetLengthSid and CopySid. Never assume token DefaultOwner equals TokenUser: an
administrator token can make an ordinary TempDir administrator-owned.

Build an explicit owner=current effective user descriptor with present non-null
protected DACL and the exact intended user-only allow ACE. Suitable APIs are
InitializeSecurityDescriptor, InitializeAcl, AddAccessAllowedAceEx,
SetSecurityDescriptorOwner, SetSecurityDescriptorDacl and
SetSecurityDescriptorControl(SE_DACL_PROTECTED). Supply the descriptor at relative
NtCreateFile(FILE_CREATE) through OBJECT_ATTRIBUTES.SecurityDescriptor. Never
create with default permissions and repair later.

GetSecurityInfo on a retained READ_CONTROL handle obtains actual owner and DACL.
Own and LocalFree its returned allocation exactly once. Validate descriptor, ACL,
ACE sizes, offsets, flags and embedded SIDs before reading records. Use SDK
layouts and aligned storage with checked lengths. Expand generic masks using the
filesystem mapping. Unknown ACE layouts, callback/object grants, malformed SID,
unqueryable security and unexpected token transitions refuse. Reject unknown access
mask and ACE flag bits. Normalize generic rights before comparing exact private
permissions; private allow ACE flags must match the explicit non-inherited policy.
A deny ACE must never cancel an otherwise forbidden allow grant during validation:
reject forbidden grants independently of deny ordering or effective-access guesses.

Strict private directories/files require current-user owner and the exact protected
user-only ACL. No inherited broad grant and no SYSTEM/Administrators exception on
private leaves. Preparation/state-home policy separately permits only trusted
owners and denies outsider mutation. No silent existing-ACL repair.

Ancestor policy mirrors Unix trusted-root/sticky-root behavior:

- Owner must be effective user, LocalSystem or Builtin Administrators.
- LocalSystem/Administrators are explicitly trusted ancestor-maintenance principals.
- Outsiders may read, list, traverse and synchronize.
- Trusted system-owned ancestors may allow outsider sibling file/subdirectory
  creation only without DELETE_CHILD, DELETE, WRITE_DAC, WRITE_OWNER,
  WRITE_ATTRIBUTES or WRITE_EA grants. Existing descended components still pass
  their own owner/security checks; a foreign name-creation winner is never trusted.
- Known deny ACEs may be understood as denials. Inherit-only ACEs do not grant
  rights on the current ancestor but must still have known validated structure.
  Unknown ACEs refuse. Protected new private children ignore inherited grants.

Retain native one-component opens, no-reparse checks and held-versus-named identity.
Directory handles use FILE_TRAVERSE, FILE_READ_ATTRIBUTES, READ_CONTROL and
SYNCHRONIZE, read/write sharing, no delete sharing. Only modified parents request
additional write rights needed for the actual flush boundary.

### Production creation closure

Current creation sites needing audit/integration include:

- `crates/surge-persistence/src/runs/storage.rs`: Storage initialization creates
  home/db and home/runs through create_dir_all.
- `crates/surge-persistence/src/runs/registry.rs`: registry opening creates db.
- `crates/surge-persistence/src/runs/file_lock.rs`, `store.rs`, `memory/store.rs`:
  parent creation can create the state home indirectly.
- `crates/surge-daemon/src/main.rs`: SURGE_HOME/default home resolution and startup.
- `crates/surge-cli/src/commands/daemon.rs`: daemon directory creation before start.
- `crates/surge-cli/src/commands/engine.rs` and `bootstrap.rs`: direct runtime-home
  creation bypasses daemon ownership.
- `crates/surge-persistence/src/work_items.rs`: lifecycle lock directory creation.

Stage 1 dependency closure includes `src/lib.rs` module/export wiring; new
`state_home.rs` and `state_home/windows/{security,native}.rs`;
`runs/storage.rs`, `runs/registry.rs`, `runs/file_lock.rs`, `store.rs`,
`memory/store.rs`, `work_items.rs`; daemon `main.rs` and its startup home helpers;
CLI `commands/daemon.rs`, `commands/engine.rs`, `commands/bootstrap.rs`, and any
additional call site discovered in the complete runtime-home creator audit.
Windows target dependency features belong to this same stage.

Before the large integration, execute an early native non-administrator local-NTFS
probe of the actual standalone production module: protected directory creation,
retained writable directory open and NtFlushBuffersFileEx flags-zero must succeed.
A failed or unsupported directory barrier keeps private guards closed and requires
resolving the native design; do not build the remaining backend around an ignored
failure. Record actual owner/ACL, API statuses and filesystem evidence without
secret bytes.

Map every real caller before implementation. A new state home is created with the
protected descriptor at first creation through retained parent ownership, then
verified before SQLite or private writes. Missing and existing db, runs, daemon
and lifecycle directories must all be securely created or validated before the
corresponding operation, including CLI paths that formerly ran before Storage.
Existing homes are validated unchanged;
unsafe owner/ACL yields an explicit compatibility refusal. Retain StateHomeOwner
and its relevant directory handles through actual Storage/registry/SQLite and
startup operations. A validate(path) -> () check followed by unrelated path routing
is not an ownership guarantee. Audit pathname-only SQLite opening explicitly: the
retained no-delete ancestor fence, endpoint security/identity checks and lifetime
must justify the actual route and prevent substitution; otherwise refuse or redesign
the opening boundary. SourceOwner, preparation lock and Namespace remain distinct
capabilities with independent invariants. Do not recursively
chmod or overwrite an existing home, including a fresh default TempDir.

Windows fixtures create a protected child state home under their TempDir via the
same real native creation primitive. Inspect the parent actual owner/ACL first;
TempDir default owner is evidence, not an assumed current-user identity. Test an
administrator-default-owner parent explicitly. Default TempDir rejection must not
be fixed by skipping tests or rewriting its ACL. Native probes must run production
security code and inspect actual descriptors independently.

Stage acceptance: first-observable creation has protected current-user ACL;
malformed/null/unprotected/widened/inherited/foreign-owner security refuses;
unsafe existing home unchanged; normal production Storage and daemon startup use
the secure boundary; no sensitive bytes/SQL writes precede validation.

## Stage 2: private namespace and durability

Implement Namespace open, verify, read, publish and is_empty behind PrivateFiles.
Do not enable Windows private guards yet. The initial proved durability scope is
actual local NTFS, verified using retained handle filesystem information. UNC and
other filesystems explicitly refuse until separately proved; no weaker fallback.

flush_complete uses NtFlushBuffersFileEx flags zero, null Parameters and zero
ParametersSize. Require synchronous success. This flushes data, metadata and
storage cache. DATA_ONLY, NO_SYNC and DATA_SYNC_ONLY are prohibited substitutes.
Native writable directory barriers must succeed as a non-administrator on NTFS;
access denied/unsupported is an error, never ignored.

Publication sequence:

1. Verify namespace identities and actual security.
2. Exclusively create protected random temporary object with FILE_WRITE_THROUGH.
3. Write bounded contents and flush complete.
4. Synchronous native no-replace rename relative to retained parent.
5. Flush the same renamed file handle, then modified parent directory.
6. Verify named identity/security/single-link state and stored bytes before success.
7. Collision preserves final object, deletes only the descriptor-owned temporary
   object and flushes the changed parent. Unexpected failures refuse.

Temporary cleanup is descriptor-owned using native FileDispositionInformationEx
or an equally identity-preserving native operation on the still-held object. Never
close the temporary handle and unlink by pathname, especially with delete sharing.
Use an explicit publication state: before rename success, cleanup owns only the
temporary identity; immediately after successful rename, disarm temporary deletion
before any subsequent fallible barrier. A post-publication flush/verification error
returns an error while preserving the final object, never deleting it through the
retained renamed handle. Before-publication errors and collisions delete the owned
temporary object and establish the modified-parent barrier; cleanup failures are
observable errors and cannot justify success or deletion of a replacement name.

Each created directory requires a barrier on its modified parent. A successful
existing-only reader establishes its own file and parent barrier, covering another
process publishing a visible name before its last flush. No read-only flush fallback.

Reader concurrency candidate pending security review: private object handles use
FILE_GENERIC_READ|FILE_GENERIC_WRITE|READ_CONTROL and compatible read/write/delete
sharing; publisher retains DELETE for native rename. This avoids conflicting with
a still-open publisher and permits simultaneous writable flush handles. Directory,
preparation-lock and source handles retain their no-delete fences. File sharing
itself grants no access: exact owner-only DACL supplies confidentiality and named
identity plus HMAC supplies authenticity. Same-user hostile writes were not prevented
by the Unix ACL contract either. Review must prove this boundary, not assume it.

Reads reject unsafe components/ADS, non-disk/reparse/directory/delete-pending,
hardlinks and files over 8 MiB. Check owner/security/held and named identity before
and after bounded read and again around flush. is_empty uses retained-directory
native enumeration with explicit cursor restart on every call and verifies before/
after enumeration. Never use path read_dir as the sensitive ownership boundary.

Stage acceptance: actual non-admin NTFS barriers, no-replace collisions, concurrent
publication/read, repeated enumeration, ACL widening, hardlink, reparse, ADS,
delete-pending and parent replacement probes. Include a native competing temporary
name replacement oracle: replace/move the temporary name while the original handle
is retained, then force cleanup; original owned object is disposed, substitute and
final objects remain unchanged. Inject failure immediately after successful rename
and prove final object preservation plus disarmed cleanup. All unexpected API
errors fail tests.

## Stage 3: preparation and authentication integration

Only after Stages 1-2 pass, enable Windows PrivateFiles, acquire_private and
require_private together. RestrictedHeld is nonserializable and validates retained
security on every require_private/verify. Preserve unsupported handling elsewhere.

Run existing authentication tests on Windows: exact command/args/env/tools/sandbox/
startup and request deadlines round-trip; opaque public projection; changed request,
foreign binding/run, partial/reordered envelope and transplanted objects refuse;
established missing/corrupt key cannot regenerate; host-default populated MCP is
private even with public request identity; empty snapshots perform no private I/O.

Keep KEY_MARKER and SQL private_key_creation_allowed semantics. Filesystem safety
never substitutes for actual accepted SQL, operation, generation or provider-opening
authority. Replay checks stay before filesystem work, and reclamation requires the
original actual lock identity. Freeze/hydrate helpers need no weaker Windows branch.

## Stage 4: full native acceptance and review

Run the existing task preparation lifecycle suite: generic reservation refusal,
exact replay without filesystem work, concurrent exact operation, snapshot drift,
live/dead owner rows, killed owner and original-object reacquisition. Existing
relocated-lock tests need actual Windows-native refusal/replacement interleavings,
not ignored unsupported results.

Exercise explicit/default populated private Flow, cold daemon restart using frozen
authenticated inputs, tampered/missing inputs refusing before provider or MCP calls,
and no manufactured replacement identities. Full Windows CI remains required;
unsupported-test skips cannot close this plan.

Crash probes interrupt before/after temporary write, file flush, rename, parent flush
and established marker. Verify original authenticated identity or fail-closed recovery.
These are process-death tests, not power-loss proof. Independent VM hard-stop testing
is necessary to claim power-loss evidence; absent evidence stays explicitly unverified.

Independent reviews before guard removal: security (ACL parsing/ancestor grants/
sharing), architecture/API (capability and state-home ownership), and specification
(full requirements and durability). Actual Windows execution decides native support;
a cross-build or green Unix run does not prove it.

## Dependencies

Root Cargo.toml already declares windows 0.58. Add Windows-only persistence features
Win32_Security, Win32_Security_Authorization and Win32_System_Threading, plus actual
generated-symbol requirements verified against that version. Existing Wdk/FileSystem/
IO features cover native ownership. Keep dependencies workspace-managed.

## Primary sources

- [Microsoft access tokens](https://github.com/MicrosoftDocs/win32/blob/docs/desktop-src/SecAuthZ/access-tokens.md): effective identity and TokenUser APIs.
- [OBJECT_ATTRIBUTES](https://learn.microsoft.com/en-us/windows/win32/api/ntdef/ns-ntdef-_object_attributes): creation-time security and retained RootDirectory.
- [ACL inheritance](https://learn.microsoft.com/en-us/windows/win32/secauthz/ace-inheritance): protected DACL inheritance boundary.
- [Security descriptor control](https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptor-control): DACL presence/protection.
- [Access Mask](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/access-mask): directory-specific rights.
- [MS-FSA sharing specification](https://winprotocoldoc.z19.web.core.windows.net/MS-FSA/%5BMS-FSA%5D-200304.pdf): share-participating access.
- [NtFlushBuffersFileEx](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntflushbuffersfileex): flags-zero complete barrier and weaker modes.
- [CreateFileW](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew): NTFS write-through metadata including rename, hardware limitations.
- [ZwCreateFile](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-zwcreatefile): native directory/write-through compatibility.
- [NtSetInformationFile](https://learn.microsoft.com/ru-ru/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntsetinformationfile): native rename and DELETE access.

Amended lead verdict: ACCEPTABLE as a complete staged candidate incorporating
independent-review corrections. Security/architecture reviewers must approve the
exact ancestor ACE policy, descriptor-owned cleanup state machine, reader sharing
and retained SQLite routing before implementation integration. The early actual
non-admin NTFS directory flush probe is a mandatory executable design gate, not
release proof. No implementation or native capability claim is established by this
plan.


## First implementation slice: executable native durability design gate

This slice answers whether the documented directory barrier works under an actual
standard user on local NTFS before the complete Stage 1 integration. It does not
mint a private capability or remove an unsupported guard.

Concrete files:

- `crates/surge-persistence/src/lib.rs`: private Windows module wiring only.
- `crates/surge-persistence/src/state_home.rs`: Windows private module boundary.
- `crates/surge-persistence/src/state_home/windows/mod.rs`: native module wiring.
- `crates/surge-persistence/src/state_home/windows/native.rs`: actual production
  NativeError/NativeResult and flush_complete; native unit tests next to code.
- `crates/surge-persistence/Cargo.toml`: only Windows API features actually needed.
- `.github/workflows/ci.yml`: explicit standard-user NTFS probe execution if the
  existing Windows nextest account is elevated; existing full suite remains intact.

Production primitive:

```rust
#[derive(Debug, thiserror::Error)]
enum NativeError {
    // Typed native status, filesystem/access/identity validation failures;
    // no secret-bearing contents and no fabricated WorkItemError adapter.
}
type NativeResult<T> = Result<T, NativeError>;
fn flush_complete(file: &File) -> NativeResult<()>;
```

flush_complete calls NtFlushBuffersFileEx flags zero, null parameters, size zero,
with live file handle and initialized IO_STATUS_BLOCK. It must check returned status
and final synchronous status. Unexpected STATUS_PENDING cannot release stack-backed
storage; use the existing synchronous native invariant containment discipline.
There is no retry to weaker flags, ignored access error or readonly fallback.

The native test uses a real writable directory handle and a real regular-file
handle. Use FILE_TRAVERSE plus documented writable directory rights, SYNCHRONIZE,
READ_CONTROL and no-delete sharing. Inspect actual local filesystem using
GetVolumeInformationByHandleW and enforce NTFS/local scope. TokenElevation verifies
that the executing identity is not elevated; default-owner evidence is recorded
separately from TokenUser. A privilege/filesystem mismatch is a failing design-gate
result, not an unsupported skip.

For this first primitive probe, no production security capability is claimed. Test
setup may use a private test-only creation helper with actual protected user-SID
security supplied at first creation; it must not alter any existing TempDir ACL.
The alternative is an isolated existing ordinary test directory whose actual
owner/ACL are inspected, with no private bytes and no claim of private namespace
support. Select the latter if it keeps the slice independent: this probe tests the
real flush primitive, not the later owner-only creation implementation. Full
production creation/validation helpers remain Stage 1 requirements.

Minimal API features: existing Wdk_Storage_FileSystem for NtFlushBuffersFileEx and
Win32_Storage_FileSystem for real file/directory information; Win32_Security and
Win32_System_Threading for token elevation/user inspection. Add Authorization only
if protected test creation actually uses those APIs. Verify generated windows 0.58
feature ownership before edits.

CI must run the actual module's probe under an explicitly non-elevated identity.
If a hosted Windows runner executes elevated, use a dedicated standard local test
account/process for this isolated probe, verify token elevation inside it, and retain
an exit-status/log receipt. Do not claim that a administrator's successful flush
answers the non-admin requirement. Avoid changing machine/user ACLs as a workaround.

Slice acceptance: local NTFS evidence; actual non-elevated token; file and directory
flags-zero flush return synchronous success; access-denied flush is propagated in a
negative control; production primitive used directly; current private unsupported
guards remain unchanged. Process-death or power-loss proof is not claimed by this
slice. After success proceed through the full Stage 1-4 dependency closure.
