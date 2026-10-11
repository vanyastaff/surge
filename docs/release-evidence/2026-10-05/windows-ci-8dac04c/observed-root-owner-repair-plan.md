# Observed Windows root owner regression: bounded repair

Native RED: revision 8dac04c1d01ed761944a43b34f639e5c8d1442e3,
run37712492476, job113101382188. Standard-user protected-home creation fails
before creation. Independent SDK route output identifies the first production
Ancestor object C:\ with exact TrustedInstaller owner:
S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464.
Its DACL has effective Authenticated Users ADD_SUBDIRECTORY, read grants,
SYSTEM/Admin full grants and inherit-only user mutation; no TI mutation ACE.
The same run has 3685 tests run,2922 passed,763 failed,46 skipped; evidence proves
this probe's immediate cause, not that every failure shares it.

This supersedes the broader conditional plan's proposed TI mutation-ACE trust.
Implement exactly two explicit ancestor-role changes in Windows security.rs:
1. Accept the exact TI SID as an ancestor owner alongside existing accepted owners.
2. For an ancestor owned by this exact SID, permit the existing outsider read
   baseline plus ADD_FILE/ADD_SUBDIRECTORY only.

Keep mutation-ACE trust restricted to current user/SYSTEM/Administrators.
An effective TI mutation ACE therefore remains rejected. Preserve SQLite's
captured default-owner exception for SYSTEM/Admin only, strict TokenUser owner
and exact user-only ACL for protected objects, token capture, identity routing,
flush behavior, inheritance handling and all existing no-repair refusal behavior.
No name/prefix/group/environment/path-root heuristics. No new dependency or API.

Native unit tests must separate these roles with literal independent SID/mask
expectations: exact TI owner recognized without TI ACE; other service/foreign
owner rejected; TI mutation ACE rejected; TI-owned ancestor foreign child-create
allowed but each DELETE_CHILD/DELETE/WRITE_DAC/WRITE_OWNER/WRITE_ATTRIBUTES/
WRITE_EA/GENERIC_WRITE rejected; user-owned ancestor still refuses those foreign
child-create grants. Keep direct explicit-user grant where production writable
parent access is required. Preserve safe-existing collision reopen.

The existing standard-user missing-home test supplies actual pre-fix RED. Rerun
it and all original standalone ownership/security/SQLite probes against the new
candidate, plus normal Windows suite/Clippy. Generic earlier ancestor refusal
must not satisfy negative oracles: require a successful same-route control.
Privileged isolated owner/creation fixture gaps remain explicit; a fixed root
route alone is not full Stage1 native acceptance, G2b or process guardian closure.

Precode roles: architecture lens accepts this two-role minimal repair; security
lens separately checks the narrowed policy. Existing user authorization covers
CI repair and pushing completed stages. Release remains NO-GO until open gates pass.
