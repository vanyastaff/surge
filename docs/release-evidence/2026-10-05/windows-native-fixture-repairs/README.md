# Native fixture setup and module-path repairs

The 04c native ACL matrix stopped before backend refusal: legacy SetFileSecurityW with UNPROTECTED already present left the observed protected DACL unchanged. Only this unprotected fixture now uses held-handle SetSecurityInfo, then independently SDK-queries DACL_PRESENT and cleared SE_DACL_PROTECTED. It closes the handle before production prepare and requires the exact unprotected-directory refusal, unchanged owner, descriptor, sentinel bytes and entry count. Production policy and the other crafted ACL branches are unchanged.

The first-observable probe also needed an explicit nested module path. Actual hosted a740 fmt failed resolving state_home/observation.rs. Full local cargo fmt reproduced exit1 and passed exit0 after the one-line path fix. Original frozen receipt remains historical; the appended path receipt supersedes its one changed source hash. No behavior change is claimed from this module-path repair.

Independent pre-code security review accepted the bounded SDK fixture plan. Subsequent independent frozen-source preflight accepted both exact hashes and lifetimes. The SDK cross-target strict Clippy gate compiles actual imports/setup helpers only; it does not compile the full native matrix. The full Storage probe and ACL mutation/refusal behavior still require native Windows execution. These are candidate repairs, not Stage1 acceptance.

The original per-file formatting check skipped child resolution and missed the module-path defect. The full workspace formatter is the recorded corrective gate. Raw failed and successful receipts are retained here.
