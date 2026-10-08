# Third-Party Licenses

Surge is dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).
Dependency license selection is checked in CI by
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny) against the
allow-list and scoped exceptions in [`deny.toml`](deny.toml):

```shell
cargo deny check licenses
```

This command passed on 2026-10-05 for the current `Cargo.lock` with
cargo-deny 0.20.2. It checks whether resolved Cargo dependency licenses
satisfy the repository's selection policy. It does not collect the
copyright notices, license copies or upstream NOTICE files required for
binary redistribution, or inspect native system libraries linked into a
release binary.

## Release archive notice coverage

The archive includes `README.md`, `LICENSE-MIT` and `LICENSE-APACHE`.
The complete Apache-2.0 terms and README attribution cover the checked
OpenSSL 3.6.4 static-link license: its official source tree contains no
upstream NOTICE file. This evidence is specific to OpenSSL 3.6.4; a
changed native-library version requires another source check.

A complete redistribution-notice inventory for the remaining linked Rust
and native dependencies has not been verified. The historical local archive
does not include a collected per-dependency copyright/license/NOTICE bundle.
A passing Cargo license-selection gate must not be reported as proof that
all archive redistribution notices are complete. Before publishing, match
the actual linked dependency set to its source license and notice files
and include the required text in each archive.

The release collector reads license and notice bytes from Cargo archives whose
SHA256 matches `Cargo.lock`. Missing published texts use reviewed immutable
upstream supplements under `scripts/notice-sources/`. Native capture records the
successful Cargo JSON stream, selected source inputs, link origins, Rust runtime
copyright inventory and binary hashes before and after strip. New packaging
requires complete evidence, a `THIRD_PARTY_NOTICES.txt` archive member and paired
provenance JSON; diagnostic inventories cannot satisfy that gate.
Vendored notice files preserve upstream bytes, including whitespace, so their
recorded hashes can be verified without normalizing source evidence.

The actual macOS ARM64 inventory selects 351 registry packages. Exact reviewed
checksums and complete member inventories establish `lineark` correspondence
despite its published dirty flag. The scoped compiled-macOS `objc2` mappings retain
full canonical MIT terms, the entire upstream declaration and SDK caveat, and
factual author metadata without inventing copyright holders or years. Six vendor
native notice inventories, the checked local OpenSSL inputs and Rust 1.98.1 runtime
notices have independent review. The gate still requires actual matching native
build/source/binary receipts; another version, target or source origin needs its
own evidence. These mappings do not establish general SDK/source redistribution
assurance. See the
[release procedure](docs/release-procedure.md) for collection and diagnostics.

## Accepted licenses

The licenses Surge accepts for dependencies (see `deny.toml` for the
authoritative list):

| SPDX | Notes |
|------|-------|
| MIT | |
| Apache-2.0 | incl. `WITH LLVM-exception` |
| BSD-2-Clause / BSD-3-Clause | |
| ISC | |
| Zlib | |
| MPL-2.0 | weak copyleft; file-level, compatible |
| Unicode-3.0 | Unicode data tables |
| CC0-1.0 / 0BSD / MIT-0 | public-domain-equivalent |
| NCSA | permissive (BSD-style); via `libfuzzer-sys` fuzz tooling |
| CDLA-Permissive-2.0 | Mozilla root-cert data via `webpki-roots` |
| bzip2-1.0.6 | scoped exception for `libbz2-rs-sys`; permissive redistribution conditions verified from its license text |

## Tag frequency (informational)

`cargo deny list` reports the raw SPDX-tag frequency across the resolved
dependency graph (`--all-features`). **Crates with an `OR` expression
appear under *each* tag** — e.g. a crate licensed `MIT OR GPL-2.0-only`
shows under both `MIT` and `GPL-2.0-only`, but Surge selects the permissive
option, which is why `cargo deny check` passes while copyleft tags still
appear in this raw frequency:

```text
0BSD: 7
Apache-2.0: 730
Apache-2.0 WITH LLVM-exception: 19
BSD-2-Clause: 4
BSD-3-Clause: 13
BSL-1.0: 1
CC0-1.0: 4
CDLA-Permissive-2.0: 1
GPL-2.0-only: 1
ISC: 12
LGPL-2.1-or-later: 2
MIT: 934
MIT-0: 1
MPL-2.0: 7
Unicode-3.0: 19
Unlicense: 9
Zlib: 35
bzip2-1.0.6: 1
```

These are raw SPDX expression tags, not selected license obligations.
Dual-license alternatives can include tags outside the allow-list; the
selection gate evaluates the full expressions. Counts were regenerated
with `cargo deny list --format json` on 2026-10-05 using the all-features
graph configured in `deny.toml`.

## Regenerating

```shell
cargo deny list            # full per-crate breakdown by license
cargo deny check licenses  # enforce the deny.toml allow-list
```

Regenerate this file's summary after a significant dependency change
(`cargo update`, new crate, major version bump).
