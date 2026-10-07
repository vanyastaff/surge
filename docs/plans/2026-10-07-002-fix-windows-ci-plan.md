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
