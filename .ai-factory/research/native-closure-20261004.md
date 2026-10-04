# Native closure research — 2026-10-04

QUESTION: Can Surge signal an original macOS descendant without a PID lookup/kill race?

ANSWER: An identity-bound candidate exists in pinned Apple XNU source. This settles a source-level feasibility question; runtime/deployment and complete native closure remain unresolved.

VERSIONS: Apple XNU `xnu-12377.1.9`; installed macOS 27 SDK declarations. Running kernel and loaded-symbol compatibility have not been established.

SOURCES: The wrapper `proc_signal_with_audittoken` returns zero or an errno value. [Pinned Apple wrapper, lines 462–469](https://raw.githubusercontent.com/apple-oss-distributions/xnu/xnu-12377.1.9/libsyscall/wrappers/libproc/libproc.c).

The kernel resolves the audit token to an exact process identity, performs authorization, reacquires that identity, checks permissions and signals the retained process. This suggests a safer candidate than a userspace token check followed by `kill(pid)`; this is an inference from implementation, not a runtime result. [Pinned Apple kernel, lines 3330–3482](https://raw.githubusercontent.com/apple-oss-distributions/xnu/xnu-12377.1.9/bsd/kern/proc_info.c).

OPEN: minimum supported runtime, private-interface deployment, Rust ABI/lifetimes, changed-credential targets, continuous entitled descendant observation, stable terminal drain, genuine Stop/reuse/cold acceptance. A successful signal does not prove termination or full descendant closure. No native source authorization or OS setup is inferred.

Full read-only reports are retained in `.autopilot/competitive-waves/evidence/owned-flow-20261003/native-closure-gap-inventory-20261004.txt` and `macos-identity-safe-signal-feasibility-20261004.txt`.

NOTE: Saved under `.ai-factory/research/` to honor the project's documented location for detailed AI context instead of the research skill's default `.rust-studio/research/`.

UNRESOLVED for product/runtime acceptance. ANSWERED only for the bounded source-level candidate.
