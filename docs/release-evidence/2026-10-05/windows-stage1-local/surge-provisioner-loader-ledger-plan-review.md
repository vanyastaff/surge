# Loader/ledger plan review receipt

Original reviewed plan SHA-256: `a3ac3c9d0070193b9c6b4e259a6a41a05f23a5cd74911d1214208657f961062f`.

Independent bounded source preflight verdict: RESHAPE. Actual PowerShell helper loading at windows-native-privileged-probe.ps1:162-165 uses path-backed assembly loading. The loader probe must remove the exact compiled DLL used for verified byte loading, then full fixed bootstrap inventory/directory, and invoke the SAME retained Assembly/Type afterward. Deleting only C# source would miss the mapped-image defect.

Actual native Relative at windows-native-provisioner.cs:438-443 invokes Own before CreateTree's owned-tree registration; partial acquisition can precede caller-visible tree ownership. Register-before-call/immediate raw-handle attachment is the correct proposed direction. Actual failure runner loops over child-returned mutation/child arrays without independently requiring expected prefixes; empty/truncated ledgers can pass vacuously. Require literal per-case mutation/resource/child prefixes, unique contiguous ordinals, exact target kind/parent/component, terminal disposition and independent malformed-ledger negative fixtures.

Root accepted these two findings and authorized documentation-only refinement. Revised plan SHA-256: `857b420c0c04086b413e15b9f260c4d167dacdaab715f20aeaf1f949e9ba4a47`.

Status: source-ready PROPOSAL for a DIFFERENT independent reviewer, not implementation acceptance or author self-certification. No script, C#, runtime or Rust changes; no builds, downloads, native launches or elevation performed. Existing Stage1 counter remains3/3. No source/native5a/5b closure and no fourth runtime-home ACL repair claim.
