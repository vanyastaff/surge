# Bounded fixture repair: actual unprotected DACL setup

Read-only plan, pending independent review. Source zone only
crates/surge-persistence/src/state_home/security_tests.rs. No production policy,
frozen fixture3, privileged actor or script changes in this repair.

## Native evidence

04c4678 run37714922396 actual log /tmp/surge-windows-ci-04c4678-clean.log
lines17045-17048: security_tests.rs93 assert_ne failed for unprotected; both
observed and original SDDL are exact protected D:P user-only OICI Full ACEs.
Current install_dacl ALREADY passes DACL_SECURITY_INFORMATION |
UNPROTECTED_DACL_SECURITY_INFORMATION to legacy SetFileSecurityW. This is not a
missing-flag issue. Prior real RuntimeHomeOwner::prepare on the same object
succeeded, so the setup must establish actual unprotection before refusal counts.

## Primary-source basis and limited inference

Microsoft documents SetFileSecurityW obsolete and says security it applies to a
directory is not inherited by children:
https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-setfilesecurityw
Microsoft documents handle-based SetSecurityInfo accepts SECURITY_INFORMATION
flags and applies inheritance behavior:
https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-setsecurityinfo
https://learn.microsoft.com/en-us/windows/win32/secauthz/automatic-propagation-of-inheritable-aces
UNPROTECTED_DACL_SECURITY_INFORMATION denotes parent ACE inheritance and needs
WRITE_DAC:
https://learn.microsoft.com/en-us/windows/win32/secauthz/security-information
Observed legacy no-op plus these contracts support switching this setup to the
current inheritance-aware SDK API. Do not claim all legacy implementation detail
or native success from documentation or the compile harness.

## Exact edit map

1. In install_dacl retain existing descriptor parsing/allocation and legacy
SetFileSecurityW path for protected/null/widened/inherited setup. Those deliberate
ACE fixtures must not be normalized incidentally by an across-the-board API swap.
2. Only the unprotected branch calls a new test-private unprotect_dacl helper.
Extract the present non-null valid ACL from the SDK-created input descriptor using
GetSecurityDescriptorDacl; retain its LocalAlloc owner across the mutation call.
Independently open the exact owned fixture with WRITE_DAC|READ_CONTROL|
FILE_READ_ATTRIBUTES, BACKUP_SEMANTICS|OPEN_REPARSE_POINT, READ|WRITE sharing and
no DELETE sharing. Use SetSecurityInfo(handle, SE_FILE_OBJECT,
DACL_SECURITY_INFORMATION|UNPROTECTED_DACL_SECURITY_INFORMATION, no owner/group,
pDacl, no SACL); require ERROR_SUCCESS. No privileges, retry, shell/ACL repair or
fallback. Close all handles before production re-open.
3. While retaining that exact handle, independently query resulting owner/DACL via
GetSecurityInfo and GetSecurityDescriptorControl. Require DACL_PRESENT and
SE_DACL_PROTECTED == 0 before setup returns. This is an SDK fact, not the
production validator as oracle. Keep SDK allocations in RAII owners.
4. Keep original assert_ne(before,original), sentinel bytes, complete post-setup
SDDL and entry-count unchanged assertions. Add owner_sid(before)==original SID.
For unprotected case require the typed security refusal and exact underlying
'directory DACL is not protected' reason; do not let an earlier owner/ancestor or
API error satisfy it. Other matrix cases retain their original assertions.

## Checks and limits

Compile/lint actual helper source in a narrow Windows-target harness if feasible;
no test/Storage behavior stubs. rustfmt and git diff --check. Actual standard-user
ignored native matrix is required to establish mutation and unchanged refusal;
no macOS execution or unrelated test denominator substitutes for it. If current
SDK call does not produce unprotected control, fail setup explicitly; never lower
the oracle. Fixtures1/2 and first-observable fixture3 remain separate.
