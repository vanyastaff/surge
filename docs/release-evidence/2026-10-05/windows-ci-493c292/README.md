# Native Windows checkpoint: 493c292

Source: `493c29264ab9e742071567907f2b8f1d83621d25`. [CI run](https://github.com/vanyastaff/surge/actions/runs/37682450589), [Windows Test Suite job](https://github.com/vanyastaff/surge/actions/runs/37682450589/job/113002217595).

The mandatory nextest suite ran **3,646 tests: 3,612 passed, 34 failed, 34 skipped**, in 328.728 seconds. Windows is not ready. Remaining failures are 16 quota-route tests and nine private start-preparation tests, plus five MCP ownership integration tests and four cold owned-process recovery tests. Plans 003/004 retain the implementation closure; unsupported ownership/preparation must not be replaced with successful placeholders.

Native GREEN includes all seven configuration save regressions (cooperative and denied-delete-sharing readers, concurrent writers, complete reads and failure cleanup), both quoted executable/compound shell oracles, and both bootstrap telemetry writer-release regressions. Clippy on Windows/macOS/Ubuntu, macOS/Ubuntu test suites, MSRV, formatting, security, packaging and benchmark jobs passed on this source.

The additional mandatory isolated standard-user NTFS step **passed 1/1**. It ran the real persistence unit-test binary under a dedicated non-elevated account; actual TokenUser and TokenDefaultOwner matched SID ending 1003. Local fixed NTFS was verified. `NtFlushBuffersFileEx` with flags 0 returned and completed `STATUS_SUCCESS` for both writable file and directory. Read-only file flush returned the exact expected access-denied error. The raw receipt includes directory owner and ACL metadata. The probe's isolated directory had four ACEs and is not evidence of a private user-only preparation namespace.

The probe test is ignored in the ordinary elevated suite and executed explicitly by the required separate standard-user step. This accounts for the additional skipped test relative to the earlier 33 skips; no existing test was disabled.

`windows-test.log.gz` preserves the unmodified GitHub job log, including failing suite output and successful standard-user probe. SHA256SUMS identifies compressed and uncompressed bytes. Native G1 guardian types and the full private backend were not present in this source.
