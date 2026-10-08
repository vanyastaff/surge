# R28 CI engineer

Implemented only assigned `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `justfile` after coordinator independent plan ACCEPTABLE.

- Release calls repository-local reusable CI from the same triggering revision (workflow_call context and checkout default to caller commit); both build-release and publish need validation. Read-only reusable permissions; no secrets or outward dispatch performed.
- Reusable gate preserves packaging Python test, native test matrix, doctests, existing MSRV contract and performance budget. Cargo test/build/check/bench/clippy commands now use --locked. Format checks cover all members.
- Workspace clippy errors on warnings in default and all-feature configurations. Historical strict per-crate checks retained.
- Linux integration uses exact R21 allowlist: ACP reconnect/classification, three real engine suites, mock MCP, controlled daemon MCP exact test and CLI daemon restart. Builds helpers first. Failures block; external runtimes and empty M6 placeholders excluded. Each ignored step bounded at 10 minutes.
- Security required within reusable CI: cargo-audit 0.22.2 and cargo-deny 0.20.2 pinned installation, fresh advisory fetch, locked advisories/licenses/bans/sources. Existing standalone security workflow unchanged.
- Local release recipe builds both sibling executables. Local just recipes match strict locked lint and integration allowlist; ci-full adds deny policy; tooling recipe installs pinned audit/deny.

## Executed evidence

`actionlint .github/workflows/ci.yml .github/workflows/release.yml`: exit 0 twice, no diagnostics.
`just --list`: exit 0, recipe parser accepted.
`just --dry-run ci-full`: exit 0, displayed strict locked commands and full local-only allowlist.
`git diff --check`: exit 0 twice, no whitespace errors.
Official GitHub RustSec release API lists cargo-audit/v0.22.2 as latest cargo-audit release. cargo-deny 0.20.2 matches R06 local installed version.

No cargo build/test/security command executed here, no GitHub run triggered, no native Windows/Linux execution, no commit/push/publication. These remain coordinator evidence obligations; syntax passing does not prove release green. Action versions follow existing repository conventions; SHA-pinning broad workflow modernization not included.
