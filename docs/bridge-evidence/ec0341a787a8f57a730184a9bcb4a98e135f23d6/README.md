# Native workspace verification

Tested source: `ec0341a787a8f57a730184a9bcb4a98e135f23d6`.
[CI run 38230456119](https://github.com/vanyastaff/surge/actions/runs/38230456119)
completed successfully: all 11 jobs passed. The later evidence-only commit does
not change this tested source identity.

| Native platform | Workspace passed | Workspace skipped | Bridge passed | Bridge inventory ignored |
|---|---:|---:|---:|---:|
| Linux x86_64 | 4169 | 36 | 383 | 4 |
| macOS arm64 | 3941 | 37 | 383 | 4 |
| Windows x86_64 | 3940 | 55 | 388 | 5 |

Workspace totals are actual nextest summaries, with zero failures. Native bridge
manifests retain exact native targets, tool versions, clean before/after Git
identity, inventories, commands and JUnit; all seven mandatory cases passed on
each platform. All 63 recorded bridge artifact digests were independently checked.

The existing CI runs UI tests/doctests on Linux; macOS and Windows test/doctest
commands exclude surge-ui. Strict default/all-features workspace Clippy includes
all crates on all three systems. Ignored integration smoke commands run on Linux
only. All configured doctest steps passed. Other successful jobs cover formatting,
MSRV 1.96, packaging, stage-transition benchmark and dependency security/licenses.

`windows-standard-user/manifest.json` records 48 unique successful probes:
33 persistence, eight process, one MCP and six orchestrator. Every probe actually
ran one named test (one passed, zero failed/ignored), under the recorded ordinary
Windows SID, elevation zero, on NTFS. Logs retain those security receipts. Four
executable hashes were recorded by the runner and agree across result rows;
executable bytes are not retained here, so they cannot be independently rehashed.

`workspace-*.log` and the eight named gate logs preserve downloaded GitHub job
bytes, including ANSI/CRLF. `workflow-status.json` records final job/step status;
`artifact-metadata.json` records GitHub artifact metadata, not independent ZIP
byte verification. `artifact-hashes.json` hashes every retained file except itself.
Historical failed/cancelled runs are recorded separately in `../intermediate/`.
