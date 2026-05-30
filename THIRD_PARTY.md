# Third-Party Licenses

Surge is dual-licensed under [MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).
Every third-party dependency carries an OSI-approved permissive (or
weak-copyleft) license compatible with shipping that dual-licensed binary.

License compliance is **enforced in CI** by [`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny)
against the allow-list in [`deny.toml`](deny.toml):

```
cargo deny check licenses
```

This check is green for the current `Cargo.lock`. No copyleft-only
(`GPL`/`AGPL`) code ships in any Surge binary.

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

## Tag frequency (informational)

`cargo deny list` reports the raw SPDX-tag frequency across the resolved
dependency graph (`--all-features`). **Crates with an `OR` expression
appear under *each* tag** — e.g. a crate licensed `MIT OR GPL-2.0-only`
shows under both `MIT` and `GPL-2.0-only`, but Surge selects the permissive
option, which is why `cargo deny check` passes while copyleft tags still
appear in this raw frequency:

```text
MIT (808)         Apache-2.0 (618)   Unicode-3.0 (19)   Zlib (19)
BSD-3-Clause (13) ISC (13)           Unlicense (10)*    Apache WITH LLVM-exception (9)
0BSD (7)          BSD-2-Clause (6)   CC0-1.0 (6)        MPL-2.0 (3)
LGPL-2.1-or-later (2)*  GPL-2.0-only (1)*  BSL-1.0 (1)  CDLA-Permissive-2.0 (1)
NCSA (1)          MIT-0 (1)
```

`*` = appears only as one arm of a dual/`OR` license; the permissive arm is
the one selected. The `cargo deny check licenses` gate is the source of
truth, not this frequency table.

## Regenerating

```shell
cargo deny list            # full per-crate breakdown by license
cargo deny check licenses  # enforce the deny.toml allow-list
```

Regenerate this file's summary after a significant dependency change
(`cargo update`, new crate, major version bump).
