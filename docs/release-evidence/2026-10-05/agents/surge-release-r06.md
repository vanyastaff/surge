# R06 dependency expert — NEEDS WORK

Read-only audit on 2026-10-05; no repository changes, builds or dependency updates performed.

## Evidence
- `cargo metadata --locked --offline --format-version 1`: PASS, 1181 resolved packages, only crates.io sources; `/tmp/surge-release-r06-metadata.json`.
- Declared MSRV comparison: no resolved package declaring rust-version above 1.96. 463 packages do not declare rust-version; compilation under 1.96 remains necessary. Seven workspace crates lack inherited rust-version; CI pins 1.96.0.
- `cargo deny check advisories licenses bans sources`: FAIL exit 5. Bans and sources pass; advisories and licenses fail. Full `/tmp/surge-release-r06-deny.log`.
- `cargo audit --version`: unavailable; no standalone cargo-audit cross-check performed. cargo-deny fetched the live RustSec DB.
- Independently cloned RustSec primary database from https://github.com/RustSec/advisory-db ; commit ef6173cbc5c50ec8166f9a5b28f07834144373ee (2026-10-03). `/tmp/surge-release-r06-advisory-db`.
- `cargo tree --locked --offline -d`: PASS; `/tmp/surge-release-r06-duplicates.txt`. 126 names have multiple resolved versions; policy allows duplicates; no general deduplication justified for release.
- `cargo shear`: FAIL exit 1, 15 findings (unused or misplaced dependencies) and unlinked source warnings; `/tmp/surge-release-r06-shear.log`. Advisory-related actionable result: bincode unused in surge-core.
- `cargo outdated --workspace --depth 1`: PASS exit 0; `/tmp/surge-release-r06-outdated.log`. Broad updates intentionally deferred; tool says wry update unavailable.

## Confirmed blockers / minimum fix plan

| Locked crate | RustSec | Minimum patched version | Scope |
| --- | --- | --- | --- |
| anyhow 1.0.102 | RUSTSEC-2026-0190 unsound | 1.0.103 | compatible lock update |
| git2 0.20.4 | RUSTSEC-2026-0183, RUSTSEC-2026-0184 unsound | 0.21.0 | root manifest + lock; API gate needed |
| h2 0.4.13 | RUSTSEC-2026-0258 low DoS | 0.4.16 | compatible lock update |
| h2 0.3.27 | RUSTSEC-2026-0258 low DoS | 0.4.16 | incompatible: teloxide0.13→teloxide-core0.10→reqwest0.11→hyper0.14; owner must plan narrow Telegram stack upgrade |
| quick-xml 0.37.5 | RUSTSEC-2026-0194, RUSTSEC-2026-0195 high DoS | 0.41.0 | tauri-winrt-notification→notify-rust Windows runtime; parent dependency update required |
| quick-xml 0.39.2 | same | 0.41.0 | wayland-scanner Linux build-time; parent dependency update/reachability disposition required |
| rand 0.9.2 | RUSTSEC-2026-0097 unsound conditional custom logging | 0.9.3 | compatible lock update |
| rustls 0.23.37 | RUSTSEC-2026-0285 CVSS low confidentiality | 0.23.45 | compatible lock update |
| bincode 1.3.3 | RUSTSEC-2025-0141 unmaintained | none | remove unused root/core dependency; do not suppress |

Primary advisory links are https://rustsec.org/advisories/<ID>.html; cloned primary advisory markdown includes precise patched ranges. No severity invented where primary record provides none.

Direct bincode unmaintained is an error under repository cargo-deny unmaintained=workspace, while cargo-audit normally treats maintenance advisories as warning. Security CI presently runs cargo audit and cargo deny licenses/bans/sources, not cargo-deny advisories; therefore distinguish maintenance policy failure from an exploitable vulnerability. Removal is narrow and safe in principle: run_event.rs712-722 explicitly uses serde_json for to_bincode/from_bincode and cargo shear finds no library references.

License gate rejects libbz2-rs-sys0.2.5 license bzip2-1.0.6, via GPUI HTTP compression. Do not blindly waive: compare primary license text to policy before adding narrowly scoped legitimate allowance. Yanked warnings: chacha20, spin0.9.8.

## Remaining limits
No build/MSRV execution requested for this role; host compilation and cross-platform gates belong to coordinator. No source-level advisory exploitability audit performed. Publish-age policy missing from repository config; exact dependency publish-age-at-resolution cannot be reconstructed reliably from Cargo.lock because it contains no resolution timestamp. Feature-unification/Trojan Source scans were not completed. Unused dependency cleanup beyond bincode is not a release blocker and should avoid unrelated edits.


## Authorized implementation follow-up
Coordinator authorized narrow fixes; no commits. Changed root git2 to0.21 with explicit ssh/https (0.21 drops prior defaults), teloxide0.14; removed unused root/core bincode; clarified actual JSON architecture and existing bench filename. Lock updates precise anyhow1.0.103, teloxide0.14.0/core0.11.2, rand0.9.3, rustls0.23.45, h2 0.4.16, tauri-winrt-notification0.7.3 (no quick-xml), wayland-scanner0.31.11/quick-xml0.41.0. Removed vulnerable h2 0.3 branch automatically as Telegram now reqwest0.12. Added per-crate license exception only for libbz2-rs-sys after reading permissive upstream LICENSE obligations; not advisory suppression.

`cargo deny check advisories licenses bans sources` after changes: PASS exit0; `/tmp/surge-release-r06-deny-after.log`, final line advisories ok, bans ok, licenses ok, sources ok. Warnings on yank and workspace wildcard remain nonfatal per repo policy. Compilation/API compatibility deliberately unverified by this role; coordinator final build and tests required. Source/reflog and Windows behavior review R19 requested; no broad cleanup.

Official cargo-audit0.22.2 binary downloaded to /tmp from rustsec/rustsec release, archive sha256 ec7ca4263769593df4d909be85b94a6b79efa2897be5d2bb8ebd516e823175af matches official GitHub asset digest. Standalone audit output `/tmp/surge-release-r06-audit-after.log`.

Standalone cargo-audit0.22.2: PASS exit0 with **17 allowed warnings**, not zero findings. These include transitive unmaintained crates and rand0.7.3/0.8.5 unsoundness warnings; cargo-deny only errors on workspace direct unmaintained/unsound policy. Yanked chacha20/core2/spin warnings remain. No false all-advisories-cleared claim; final release report must disclose exact warning list in audit log.

## git2 API closure follow-up
Initial downstream compilation found 15 errors in `/tmp/surge-release-r26-admission-red.log`. Applied typed git2 getter migration across surge-git fingerprint/run_worktree/worktree/task_workspace. `Result` is propagated through existing GitError::Git2; string-array iterators use `name?` before handling true None; status path collection preserves UTF8 failures. Reflog missing-evidence behavior retained while malformed message errors propagate. Existing git ownership and merge assertions retained and adapted to Result-returning getter APIs. New Unix regression stages a non-UTF8 filename and demands GitError::Git2 rather than an empty/missing result.

UI dependency closure updated run_changes getter failures through existing error-text boundary; current_branch logs invalid UTF8 errors within existing Option display API; project_init test preserved main-branch assertion. `cargo fmt -p surge-git`, standalone rustfmt for three UI files, and `git diff --check`: PASS. Targeted compile/test evidence pending R26 serialized build slot, not claimed green.

R26 serialized `cargo check -p surge-git --locked --all-targets`: PASS exit0, `/tmp/surge-release-r06-git-check.log` (13 seconds). Runtime surge-git tests queued; UI compilation remains pending final coordinator gate.

## Verified git2 runtime closure
First suite had 76 passes,1 failure,2 ignored: the new fixture tried creating an illegal filename on macOS APFS, so filesystem creation failed before the SDK boundary. Corrected fixture constructs raw IndexEntry.path from valid README blob metadata, asserts raw bytes retained in index and method returns GitError::Git2. No platform test disabled, no claim exact getter origin when status traversal may reject first. Exact regression PASS `/tmp/surge-release-r06-git-utf8.log`; final `cargo test -p surge-git --locked --lib` PASS 77 passed,0 failed,2 ignored `/tmp/surge-release-r06-git-tests-green.log` (exit0). Historical pre-adapter compile failure→check success is API migration evidence; new invalid-data test is regression evidence, not historical behavioral RED.
