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


## Stage 1 refinement: retained SQLite routing and distinct ACL policies

This refinement is a review candidate. It does not establish native probe success
or authorize private guard removal. The complete production boundary must retain
owners through SQLite operations, not return a boolean path validation.

### Concrete owners and policy distinctions

- StateHomeOwner retains the root-to-home chain and protected current-user home.
  LocalSystem/Administrators are trusted only for ancestor maintenance, not valid
  owners of the private state home itself. An existing administrator-owned state
  home refuses; never repair it to the user SID.
- SqliteNamespaceOwner retains protected current-user db/run directories and one
  DbIdentityFence per permanent database. Its policy explicitly provides user-only
  inheritable grants for SQLite-created side files, unlike private object leaf ACLs.
- PreparationOwner retains original preparation-directory and OS lock identities;
  a restricted marker is constructed only after its independent ACL validation.
- SourceOwner retains a read-only source file with no write/delete sharing.
- PrivateNamespaceOwner retains strict protected private directories and verifies
  private objects; it does not lend authority to SQLite or provider admission.

Exact ACL policy shapes are separate and must be tested independently:

| Object | Owner | Allowed ACE flags/grants |
|---|---|---|
| Ancestor | user/System/Administrators | Known validated ACEs; trusted maintenance grants, outsider read/traverse and narrowly allowed sticky-equivalent sibling creation |
| State home/preparation | current user | Protected DACL; current-user-only grant; explicitly selected inheritance flags for the owned descendant policy |
| SQLite directory | current user | Protected DACL; current-user allow with OBJECT_INHERIT_ACE and CONTAINER_INHERIT_ACE; no inherit-only omission of directory access |
| Permanent DB | current user | Explicit protected current-user-only ACL; ordinary read/write SQLite rights |
| SQLite-created WAL/SHM/journal | user or trusted default-owner Admin/System | Known user-only effective grants inherited from retained private SQLite directory; trusted owner exception is explicit for this policy only |
| Private input directory/file | current user | Exact protected current-user-only ACL and explicit non-inherited leaf policy; no SQLite/default-owner exception |

Normalize generic masks before comparison. Reject unknown rights/flags and forbidden
allow grants independently of deny ACEs; deny ordering cannot justify a broad grant.
Inherited SQLite side-file grants do not qualify as strict private-input ACLs.
SQLite side-file trusted default ownership does not grant outsiders authority: the
trusted administrators/system threat boundary already exists on ancestors, but the
effective DACL must remain user-only. Independently review this explicit distinction
before implementation; if rejected, creation must move into an actual custom VFS,
not silently adopt token-default ACLs.

Native root opens parse only drive/share root and then resolve every component by
retained RootDirectory using NtCreateFile, OBJ_DONT_REPARSE, FILE_OPEN_REPARSE_POINT,
synchronous completion and verified type/identity. Shared production modules own
raw handle conversions exactly once. Separate desired-access profiles:

- Traversal: FILE_TRAVERSE|FILE_READ_ATTRIBUTES|READ_CONTROL|SYNCHRONIZE,
  FILE_SHARE_READ|FILE_SHARE_WRITE, no FILE_SHARE_DELETE.
- Modified owned directory: the preceding profile plus actual write/add rights
  needed for flags-zero directory flush; no delete sharing.
- Permanent DB fence: FILE_READ_DATA|FILE_READ_ATTRIBUTES|READ_CONTROL|SYNCHRONIZE,
  read/write sharing, no delete sharing. No SQLite byte-range lock is taken by this
  fence, and it does not write data or pretend to perform SQLite durability.
- Source: existing FILE_GENERIC_READ, read-only sharing, no delete sharing.
- Preparation lock: existing compatible read/write lock handle plus READ_CONTROL;
  no delete sharing and actual OS lock acquisition.
- Private publisher/reader: separately reviewed writable flush profiles described
  in Stage 2; file delete sharing never leaks into directory/DB/lock/source fences.

### Permanent DB routing compatible with stock SQLite

Before the first Connection::open, the owner securely creates a missing permanent
DB file with explicit protected user security, or validates the existing file:
regular disk file, no reparse/delete-pending, one link, current-user owner, expected
ACL. It retains a share-participating DbIdentityFence before handing SQLite the
canonical frozen absolute pathname. Native held-versus-named reopens verify that
same permanent object. Retained ancestor no-delete fences block path relocation;
private parent mutation ACLs exclude untrusted child replacement. The guarantee is
not containment against a malicious process using the same user credentials.

The bundled SQLite source used by this repository confirms ordinary win32 open
requests GENERIC_READ|GENERIC_WRITE and shares FILE_SHARE_READ|FILE_SHARE_WRITE,
with no FILE_SHARE_DELETE and NULL security attributes. A read-data DB fence with
read/write sharing is therefore compatible with ordinary SQLite connections while
blocking replacement. This requires native multi-connection and migration tests;
source inspection alone is insufficient. Do not use SQLite URI exclusive=1 here.

Keep DbIdentityFence and directory owners for the entire migration, pool and all
outstanding derived stores. Pinned r2d2 0.8.10 drops its manager before idle
connections, so a manager-only with_init capture is insufficient. A persistence-
owned Windows manager retains its own namespace Arc and returns an opaque owned
connection with independent namespace retention. The public Windows manager alias
changes accordingly; the Unix alias remains the existing concrete manager.

Owned connections expose immutable Deref and narrow borrowing transaction methods,
never public DerefMut, into_inner or mutable callbacks that allow safe extraction
of an unowned disk connection. Wrap before PRAGMAs, migrations and initialization
callbacks. Migrations, pools, writers, readonly inspection/recovery, refusal owners,
Store and MemoryStore all follow this same retention contract.

Use checked Connection::close while retaining namespace ownership. Explicit close
failure returns the owning connection and error; it does not release protection.
Destructor close failure uses the existing protected-owner fatal policy, aborting
with both SQLite and namespace still held. This is necessary because rusqlite's
ordinary destructor discards sqlite3_close errors. A genuine forgotten-statement
SQLITE_BUSY probe must verify the owning error return and bounded fatal-child
behavior. SQLITE_TRACE_CLOSE fires before SQLite's busy check and is only an
ordering oracle, never proof of successful close. Successful WAL cleanup, final
namespace release and reopen remain separate native acceptance requirements.

### WAL/SHM/journal lifecycle

Do not retain permanent no-delete handles to WAL, SHM or rollback journals. SQLite
normally removes WAL/SHM when its final connection closes; retained no-delete
side-file handles would alter cleanup and fail normal database lifecycle.

Instead retain the protected SQLite directory with inheritable user-only grants
through all connections and final close. Pre-existing side files are independently
inspected before SQLite opens: regular file, no reparse/delete-pending/hardlink,
expected narrowly permitted owner and user-only effective ACL. Validation handles
are temporary and must close before operations that require SQLite side-file
cleanup. Native creation by stock SQLite inherits the controlled parent grants;
known side-file recreation cannot revert to a broad default DACL. This inheritance
is an explicit SQL namespace policy and must be verified on actual owner/default-
owner tokens, including administrator-default-owner fixtures.

If real native tests show stock SQLite creates a side file with outsiders granted
access, do not chmod it after creation or bypass the guard. Resolve creation using
a genuine VFS boundary or leave private capability unavailable. Do not enable
PERSIST_WAL solely to hide lifecycle interference.

### Caller closure and fixtures

Storage::open_with owns secure home/db/runs before config load or SQL. Registry
opening, run writer creation, legacy Store/MemoryStore parent creation, lifecycle
locks, daemon runtime directory and CLI daemon/engine/bootstrap pre-Storage paths
must all receive retained capabilities from the same state-home boundary.
General standalone Store paths need an explicit safe-parent policy rather than an
unsupported claim that every arbitrary path belongs to the managed state home.
No caller may create the state home through create_dir_all first and validate later.

Fixtures receive a protected child home created at first creation by the production
helper under an inspected TempDir parent. They never mutate the parent ACL/owner.
Apply this to every affected preparation, private-input, daemon, Storage, registry,
run-writer, CLI and cold-recovery fixture. Default TempDir owner is read evidence;
it is never assumed to equal TokenUser or accepted as a private home by shape.

Acceptance: DB fence compatible with two read/write connections, migration and
pool reuse; parent and permanent DB rename/reparse attempts refuse; WAL/SHM create,
checkpoint, final-close cleanup and later reopen succeed; owner/DACL of side files
matches explicit SQL policy; dropping Storage before a derived pool/store does not
release ownership; default-owner Admin fixture creates a user-owned protected child
home without changing the parent; unsafe existing home/sidefile is unchanged after
refusal. Record native statuses and actual identities, not just fixture setup success.

Primary evidence: [SQLite WAL lifecycle](https://www.sqlite.org/wal.html),
[SQLite VFS contract](https://www.sqlite.org/vfs.html),
[Microsoft new-object security](https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptors-for-new-objects),
[Microsoft file ACL inheritance](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights).
Local version-pinned inspection: libsqlite3-sys 0.30.1 bundled sqlite3.c winOpen
(ordinary desired access/share/create/security parameters); r2d2_sqlite manager
stores the initialization closure, whose retained owner still requires lifetime tests.

Refined Stage 1 owning-lead, independent architecture, critic and security verdicts:
ACCEPTABLE for staged implementation. Native standard-user flags-zero durability
gate remains pending on source 493c292; no backend integration or guard removal is
authorized by a missing result. Acceptance still requires real two-connection DB
compatibility, derived-store/pool lifetime, side-file default-owner ACL and cleanup,
all pre-Storage creators, protected-child fixtures and unchanged refusal objects.

### Native prerequisite result and standard-user fixture refinement

Source `493c292`, native run `37682450589`, job `113002217595` passes the
mandatory non-elevated local fixed NTFS flush probe 1/1: writable file and
directory complete flags-0 flush, exact read-only access-denied error. This
unblocks full Stage 1 implementation, without granting a private capability.

For full-backend standard-user acceptance, resolve UserProfile/LocalAppData via
the current token's Windows Known Folder API and create the protected child
through production helpers. Do not use inherited runner TEMP or the primitive
probe's PublicRoot as a trusted private-home ancestor: PublicRoot is runner-owned
and grants that distinct account full control. The copied test executable may
stay there; the private namespace must satisfy its own complete ancestor policy.
No parent ACL repair or enlarged user grant is permitted.

### Native-to-SQLite pathname agreement

Relative NT opens must identify the same pathname that stock SQLite's Win32 VFS
opens. Reject ambiguous trailing-dot/space components, DOS device basenames
(including their extensions and documented superscript COM/LPT digits), wildcard
and control characters before creation. Keep ordinary Unicode names supported.
A held-versus-named NT check alone cannot prove Win32 pathname agreement when
normalization differs. Independent alias-target fixtures must prove refusal
without SQL or filesystem effects. Microsoft documents these
[naming and reserved-name rules](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file).

Actual rusqlite 0.32.1 non-Unix path_to_cstring rejects paths without Unicode
representation; it does not perform lossy conversion. Validate that boundary
before namespace creation too, rather than leaving newly created private objects
behind for an input SQLite cannot represent. This refines the existing device/path
refusal contract; it grants no new Windows capability.

### Explicit-token profile fixture replan after native 965ac42

The null-token Known Folder lookup failed with E_ACCESSDENIED before two of the
three new acceptance assertions. Only the unsafe inherited home produced actual
backend RED. Preserve those distinctions in receipts; do not attribute an
unconfirmed cause to the profile failure.

Owning architecture and security review accepted an explicit-token profile
locator: retained TOKEN_QUERY handle, GetUserProfileDirectoryW two-call sizing
accepting only ERROR_INSUFFICIENT_BUFFER, bounded initialized UTF-16 output with
length/NUL validation, and no environment or public-directory fallback. Before
fixture creation, independently validate actual TokenUser against the CI account,
absolute local fixed NTFS routing without reparse points, and profile ownership;
retain ancestor handles through creation and inspect the resulting owner. Keep
PowerShell LoadUserProfile. Existing three backend assertion bodies stay intact.
This is a reviewed fixture foundation replan after the original precursor's three
repairs, not a reset of that completed repair history. Native results remain open.

### Runtime caller capabilities and canonical home inheritance

Canonical state-home creation uses protected explicit current-user OI|CI rights,
matching db/runs, because legacy usage.db and memory.db create SQLite side files
directly beneath it. daemon, lifecycle locks and private object namespaces retain
their separate strict non-inheriting policy. SQL side-file owner exceptions never
relax private object ownership or effective user-only grants.

Windows callers use opaque RuntimeHomeOwner/RuntimeDirectoryOwner capabilities.
Append output returns both Stdio and its retained RuntimeAppendFile lease from
stdio_clone; callers retain that lease through child readiness or failed-start
settlement. A separate writable flush handle never enters child Stdio. Control
files acquire an actual exclusive lock before stale/live inspection or byte
replacement; deletion uses the owned descriptor. OwnershipBusy denotes actual
sharing/lock contention, never generic access denial or unknown liveness. These
contracts need caller integration and native acceptance before Stage 1 closes.

### Caller lifetime refinement accepted before implementation

Preserve the existing canonical home resolver across CLI, daemon and persistence;
validate its selected route through the native owner. The effective-token profile
locator is used by fixtures here. Do not silently introduce different production
defaults under impersonation in just one caller.

Windows PidfileGuard holds the original control object and exclusive OS lock through
HostRuntime shutdown and terminal joins. Native create-versus-open provenance
allows initialization of an empty newly created PID file; a preexisting empty,
malformed, live or unknown PID refuses unchanged. Arm descriptor deletion only
after successful own-PID publication and complete flush. Process observation uses
a retained synchronization handle and explicit unknown errors, never exit-code 259
or access denial as proof of termination.

Detached CLI startup retains its actual Child and append-output leases. Readiness
requires the connected Windows stream's kernel peer PID to equal that retained
child's PID, followed by a still-live child check. Pinned interprocess 2.4.2 exposes
this through StreamCommon::peer_creds; an unrelated concurrently started daemon
cannot authorize handoff. Missing, failed or mismatched identity settles only the
launched child. Failure/cancellation requests kill once, then retains ownership
while polling until actual exit, with diagnostics for uncertainty. Settlement may
outlast the readiness deadline. It proves direct-child exit only. Output flush
errors after exit remain errors without undoing the observed settlement fact.

Architecture and independent security reviewers accepted this refinement after
closing creation-provenance and wrong-server readiness findings. Required literal
native checks include preexisting empty-file preservation, real active/exit-259
PID handling, concurrent acquisition, correct/wrong server identity, failed-start
settlement and cleanup after all actual owners release. Implementation and native
acceptance remain open; no Stage 2–4 guard is enabled by this plan acceptance.

### Ordinary test-home migration accepted before implementation

A shared test-only FixtureHome helper owns an ordinary temporary outer directory
under the effective-token profile and a missing protected child created through
RuntimeHomeOwner. It retains only namespace directories, never a SQLite connection
or permanent DB fence. Non-Windows uses ordinary TempDir behavior. The helper is
included once per test binary; persistence supplies its own crate-local owner
import, avoiding a production test-support API or self-dependency.

Keep canonical relative memory.db/usage.db paths. Separate project/worktree roots
when a test's contract requires them; repeated opens reuse the same child identity.
Owner fields precede the outer TempDir, fixture fields follow all actual owners in
harness structs, and successful tests explicitly settle writers/tasks/readers/pools
before fallible close. No delayed cleanup, retries, leaked TempDirs or ignored
errors stand in for settlement. Independent unsafe-home, absent-home creation and
original DB ownership oracles remain excluded from this convenience helper.

The owning role, independent critic and security reviewer accepted this bounded
migration. The initial orchestrator inventory has 191 textual Storage open calls
across 73 files, not 191 unique tests. Start with the shared mock admission fixture,
bootstrap/loop harnesses, then unit and integration callers; reconcile every actual
caller's path and lifetime. Native elevated and standard-user creation, unchanged
outer ACL, same-database reopen, no helper-created DB fence, and completed cleanup
remain required evidence. Merely compiling these Windows branches is insufficient.

### Directory-only run reservation

Fork reserves its destination before copying artifacts and creating the run. On
Windows, RuntimeHomeOwner::reserve_run_directory creates the canonical RunId child
and artifacts directory using exclusive native FILE_CREATE, with no fallback to
opening collisions. It creates no SQLite file, schema, migration or registry row.
The returned directory owner retains both descriptor chains through copying and
Storage handoff; append operations target the run root. Partial failures leave a
protected incomplete directory and report error, without pathname cleanup.

This capability also permits exact empty/legacy database fixture setup without
precreating a valid schema and erasing its refusal trigger. Empty-file append must
flush and drop before checked SQLite opens it; retain directory ownership during
setup. Native tests must prove absence of SQL after reservation, unchanged bytes
and ACL on collision, and run/artifacts rename refusal while held followed by
release. Architecture and independent security accepted this refined contract.

### Profile fixture repair after native 23460aa

Explicit-token profile lookup now succeeds. The profile directory is actually
LocalSystem-owned, so requiring TokenUser at this ancestor incorrectly stopped
the positive tests. The fixture accepts only the already-reviewed ancestor owner
set: current effective user, LocalSystem or Builtin Administrators. The created
outer fixture and protected child remain strictly current-user-owned. Retained
routing, fixed NTFS and all three backend assertions remain unchanged. Independent
security accepted this focused repair (profile replan repair 1); native positive
backend RED is still pending and must not be inferred from this fixture fix.
