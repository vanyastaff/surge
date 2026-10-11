# Windows diagnostic candidate 8dac04c

Revision: 8dac04c1d01ed761944a43b34f639e5c8d1442e3.
Run: https://github.com/vanyastaff/surge/actions/runs/37712492476
Windows test job: 113101382188. Windows Clippy passed.

Windows nextest: 3,685 run, 2,922 passed, 763 failed, 46 skipped.
Native flush passed; missing protected home failed before creation. The remaining
ten native probes did not execute.

Independent SDK diagnostics identify the common volume root C:\ as owned by
TrustedInstaller SID S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464.
The current policy accepts only TokenUser/SYSTEM/Administrators for ancestors,
so Namespace rejects this root before reaching the safe profile or fixture.
The recorded root DACL includes an effective Authenticated Users ADD_SUBDIRECTORY
grant, read grants, SYSTEM/Admin full grants, and inherit-only user mutation.
No TrustedInstaller mutation ACE is present. C:\Users and the profile are SYSTEM
owned; the fixture is owned by the actual standard test user. Full observed SDDL
is retained in observed-route.txt and the raw job log.

This establishes the root-owner regression cause, not acceptance of a repair.
No rules were relaxed by this diagnostic candidate. Windows release remains NO-GO.

The exact SID is identified by [Microsoft's permissions article](https://learn.microsoft.com/en-us/archive/msdn-magazine/2008/november/access-control-understanding-windows-file-and-registry-permissions).
The bounded repair plan is retained alongside this receipt. Independent architecture
and security pre-code verdicts accept only ancestor-owner recognition plus the
separate child-create allowance; mutation-ACE trust stays unchanged.

## Repair candidate local evidence

Frozen security.rs SHA256:
274e106e9df39257624bef4254354936643474e811f44d9a9747d639d1eba1dd.
Cross-target Rust 1.99 Clippy with all targets compiled the actual native.rs and
security.rs, including cfg(test), with the project Clippy configuration: PASS.
This is compile/lint evidence only, not Windows execution or full workspace/MSRV
acceptance. The standalone wrapper allows dead code for unconsumed production
items; the actual workspace gate remains required. Harness inputs are stored with
.txt suffixes; the original command and paths are retained in repair-source-freeze.json.
Formatting and scoped diff checks passed. Three added policy tests were not run
before the fix; the existing standard-user missing-home failure is the native RED.

Independent bounded pre-native source/spec verdict: ACCEPTABLE, exact frozen hash
verified. Exactly the two agreed ancestor roles change; SQLite, mutation-ACE trust,
protected owner/ACL, token capture and routing remain unchanged. Literal SID/mask
controls distinguish intended owner refusal from intended ACL refusal. No native
positive is claimed. Full 5a, 5b and Stage1 acceptance await native gates; isolated
privileged fixture gaps remain open.
