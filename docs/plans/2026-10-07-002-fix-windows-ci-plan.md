# Windows CI repair — 2026-10-07

The user explicitly requested Windows CI repair. The wider product goal is a
reliable application for vibe coding; this work repairs its tested execution
foundation. Windows is no longer deferred for CI. No test suppression, weaker
validator or privacy downgrade is an acceptable repair.

## Baseline and staged acceptance

PR #89 run `37666830209`, Windows job `112948098227`, source `392fc17`:
3626 tests run, 3510 passed, 116 failed, 33 skipped. Clippy passed. Read-only
scout classified the first failure reports (not duplicated summary rows):
59 invalid native absolute fixture paths, 25 missing real mock prerequisites,
5 invalid verification fixture identities, 1 atomic configuration replacement,
8 unsupported private preparation, and 18 remaining integration/native failures.
Passing macOS execution cannot prove Windows behavior.

Stage A owner: Windows CI builder; independent API/security review required.

1. Build the actual `mock_acp_agent` before tests on every matrix platform.
   Fix all explicit test prerequisite paths to use the native executable suffix,
   including recovery, quota and live-provider fixtures. No stub replacement.
2. Use native absolute identities consistently in facade contract, shared mock
   bridge and verification fixtures. Preserve all production absolute-path
   validators and existing denial/verification assertions.
3. Correct the Python hook oracle's platform-specific argument quoting while
   retaining writer-intent, timeout and rejection assertions.
4. Add a Windows regression for a terminated process whose `Child` handle is
   retained. The test child exits with 259, independently rejecting both the
   current OpenProcess-existence implementation and a wrong STILL_ACTIVE check.
   Capture native RED before changing production liveness.
5. Use `OpenProcess(PROCESS_SYNCHRONIZE)` and `WaitForSingleObject(handle, 0)`:
   signaled means terminated, timeout means live, failure/unexpected means unknown
   and conservatively live with tracing. Known invalid nonzero PID may mean absent;
   PID zero and access denial must not become proof of termination. Retain one
   owned handle with deterministic cleanup and SAFETY comments. Existing current
   PID and invalid PID tests remain. Public API/dependencies stay unchanged.
6. Make Windows test/lint checks mandatory again; remove advisory naming and
   continue-on-error. Native CI, rather than label changes, must establish passes.

Exact edit closure: `.github/workflows/ci.yml`; `surge-acp` facade contract and
process tracker; `surge-core` verification fixture; shared orchestrator mock
fixture and hook oracle; daemon mock prerequisite paths. Local macOS affected
tests, format and strict lint must pass. Native Windows CI provides platform
RED/GREEN evidence; record exact head, counts and unresolved failures.

## Remaining stages — explicitly open

- Atomic configuration replacement must preserve complete old/new contents under
  concurrent readers/writers and preserve destination on failure. Do not delete
  the old file before rename or swallow Windows AccessDenied.
- Private preparation requires complete native ownership/DACL/private-object
  implementation before removing unsupported guards. Existing fail-closed
  behavior is not weakened to make happy-path tests pass.
- Retained ancestor handles must protect identity and native-relative routing;
  investigate metadata-only sharing semantics before changing the directory lock.
- Repair the live-SQLite fault-injection oracle, checkpoint Git CLI native paths,
  process-owner locking and residual cold recovery/timeouts from new evidence
  after their prerequisites are fixed. No speculative timeout increase.

Stage A is not a Windows GO. The complete Windows Test Suite and Clippy must pass
on the final source, alongside macOS regression gates. Agent restriction runtime
integration remains open while this requested CI repair takes priority.

## Pre-code verdict and primary sources

Read-only scout, independent adversarial reviewer and API/unsafe/security reviewer
returned **ACCEPTABLE** for Stage A. The implementation is authorized. The native
child test must use bounded waiting and cleanup on failure while retaining its
handle through the liveness assertion. No production repair is claimed yet.

[Microsoft WaitForSingleObject](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-waitforsingleobject)
documents process waiting, SYNCHRONIZE access and zero-time polling.
[GetExitCodeProcess](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getexitcodeprocess)
documents the ambiguous application exit code 259. Both were read through
Firecrawl on 2026-10-07. The cached windows 0.58.0 API confirms typed constants
and existing features; no dependency upgrade or rights bitwise workaround is needed.

## Stage B — retained directory share fence

Latest baseline source `87ad59a`, run `37669506963`, Windows job `112957544208`:
3635 tests run, 3520 passed, 115 failed, 33 skipped. The existing
`held_parent_relative_child_cannot_route_to_replacement_tree` still fails at the
assertion rejecting parent rename. macOS/Ubuntu tests and all platform Clippy
checks passed on that baseline.

Read-only lead and independent adversarial pre-code review: **ACCEPTABLE**.
Add directory-specific `FILE_TRAVERSE` to the existing `FILE_READ_ATTRIBUTES |
SYNCHRONIZE` access in both root and parent-relative directory opens. The execute
bit participates in delete-sharing checks; metadata-only access did not. Keep
read/write sharing without delete sharing, source/lock rights, native relative
resolution, reparse/identity validation and private-input unsupported guards.
No DACL/private capability support is implied. Native Windows must prove the
existing parent-rename/routing and adjacent ordinary-child/lock tests pass.

Primary sources: [Microsoft access masks](https://learn.microsoft.com/en-us/windows-hardware/drivers/ifs/access-mask)
and [MS-FSA sharing rules](https://winprotocoldoc.z19.web.core.windows.net/MS-FSA/%5BMS-FSA%5D-200304.pdf).

## Stage C — Windows atomic configuration publication

Owning lead pre-code verdict **ACCEPTABLE** for a Windows-only helper beneath
existing `SurgeConfig::save`; Unix behavior remains unchanged. Existing native
concurrent-save failure is RED, but its precise competing handle is unknown.
Do not replace the contract with retries, destination unlink or ReplaceFileW
(which has documented failure states removing the original destination).

Exclusive randomized same-directory creation uses a retained descriptor with
read/write and DELETE rights and only FILE_SHARE_READ from its first open.
Denying write/delete sharing protects the retained object and armed temporary
pathname against writes/rename/replacement. Read sharing lets new destination readers
open the published object during the brief interval before its descriptor closes;
share_mode(0) would introduce reader sharing failures. Randomized create_new
alone does not establish the rename fence. Write and sync this descriptor.
Publish that same object using `SetFileInformationByHandle(FileRenameInfoEx)`
with only named `REPLACE_IF_EXISTS | POSIX_SEMANTICS` SDK flags. Create a checked,
aligned, initialized flexible FILE_RENAME_INFO buffer using SDK offset_of and
checked byte lengths; encode an absolute UTF-16 destination and reject NUL.
No path reopen, attribute override, ACL bypass or weaker compatibility fallback.
After success immediately disable NamedTempFile cleanup; do not call keep(),
which accesses its now-absent old pathname. Before success cleanup remains armed.
Preserve actual native error diagnostics and the existing no-power-loss guarantee.

Target-only workspace-managed windows 0.58 dependency in core needs Foundation,
Win32 Storage/FileSystem and Wdk Storage/FileSystem for named flags. Scope:
core config io helper/test module, core Cargo.toml and lockfile closure. No
public API or schema changes. Require independent unsafe/security pre-code
approval before implementation. New native-only tests may be committed before
implementation to capture retained-reader/readonly RED; existing concurrency
RED remains required. Native acceptance: eight writers, retained old reader,
continuous complete reads, readonly and directory failures preserving original
and cleaning temporary, first publication, spaces/Unicode. macOS regression
checks, strict Clippy and MSRV must pass; native Windows proves semantics.

Sources: [Microsoft rename semantics](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information),
[ReplaceFileW failure states](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew).
Cached tempfile 3.27 and windows 0.58 sources were inspected.

## Stage D — native integration fixture corrections

Read-only lead: **ACCEPTABLE**, independent pre-code reviews required.
Scope three test files only; production owner/storage/checkpoint contracts stay
unchanged. Native baseline records these failures.

- `surge-daemon/tests/work_item_route_test.rs`: each simultaneous cold host
  receives a unique endpoint, retaining identical storage/work item/attempt/project.
  Replace filesystem existence readiness with bounded actual DaemonClient connect
  attempts at the existing one-second deadline. No mutation RPC or timeout increase.
  Retain both server outcomes, no duplicate prompt and live attempt assertions.
- `surge-daemon/tests/daemon_persisted_gate.rs`: replace rename of a live SQLite
  database (Windows sharing violation) with a BEGIN IMMEDIATE transaction that
  captures the maximum sequence watermark and appends exactly one malformed
  payload at watermark + 1, copying an existing gate event metadata. Preserve
  all append-only triggers, schema/inode/live writer and second run.
  Assert scoped inspect_events_after(watermark, 1) and full inspect_run fail
  decoding the exact new sequence before asserting StreamError, no
  invented Terminal, first completion error, second gate resolves/completes,
  admission settles and successful server shutdown. Exercise real decode failure;
  no production fault injection API.
- `surge-orchestrator/tests/engine_snapshot_unit.rs`: normalize only Git CLI
  argument using structured Windows Prefix: VerbatimDisk to drive-qualified,
  VerbatimUNC to ordinary UNC, others unchanged. Persisted canonical checkpoint
  identity unchanged. Keep independent git show of exact recorded historical
  commit/bytes, with stderr on failure; no blanket string strip.

Run affected macOS test targets and strict lint; native Windows/full nextest
results must establish acceptance. Existing assertions cannot be weakened.

## Implementation review checkpoints

Stage A precursor: independent spec COMPLETE after removing an unbounded cleanup
wait; API/security COMPLETE. Local macOS 899 scoped tests and strict Rust 1.99
lint passed. Native process regression remains intentionally RED pending the
production liveness repair. Commits `7a594aa` and directory Stage B `14884ff`
are pushed; run `37672477232` is the native verification run.

Stage B implementation: independent spec and unsafe/security COMPLETE for scope;
existing Windows parent-rename RED retained. Native GREEN is pending.

Stage C: independent adversarial and unsafe/security pre-code ACCEPTABLE after
refining source sharing to FILE_SHARE_READ only. Independent implementation spec
and unsafe/API/security COMPLETE. Local and cross-target receipts are in
[atomic configuration evidence](../release-evidence/2026-10-05/windows-atomic-config/README.md).
Native Windows execution remains pending; no new-test initial RED is claimed.

Stage D: lead/adversarial/API/security pre-code ACCEPTABLE; implementation spec
and quality/security COMPLETE. Preserve the agreed child-fixture first endpoint
`cold.sock`; subsequent hosts in the same process use unique numbered endpoints.
The original delivered-row corruption proposal passed code review but failed
the live-stream macOS test (1 passed, 1 failed): the tail cursor had already
consumed that row. Revised lead/adversarial/API/security pre-code ACCEPTABLE:
append the next unread malformed row atomically without changing triggers;
prove exact scoped and full decode failure. Preserve all original live-stream
assertions. Snapshot 8 tests and route 50 tests passed, as did all three strict
lint targets. Revised gate execution and native acceptance remain pending.
