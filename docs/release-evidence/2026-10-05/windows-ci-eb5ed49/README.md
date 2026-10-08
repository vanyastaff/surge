# Native Windows receipt: eb5ed49

[CI run](https://github.com/vanyastaff/surge/actions/runs/37679286936): Windows nextest ran 3,642 tests; 3,603 passed, 39 failed, 33 skipped (333.984 seconds test execution). macOS suite and Clippy passed. Windows Clippy failed on the Windows-only items-after-statements import, separately repaired in ae213a7. No Windows GO is claimed.

Directory/readonly atomic-save failure cleanup now passes. Production quoted hook ownership, bootstrap materialization and all archetypes pass natively. The supervisor restart test passes this run, while its prior actual WriterAlreadyHeld failure and independent regression demonstrate the writer-settlement defect; 92cd502 repairs that lifecycle explicitly.

Remaining native failures: three config publication sharing oracles (repair 2 committed subsequently), five legacy MCP ownership cases, sixteen quota route cases now reaching the explicit private-preparation unsupported error, four cold provider-route ownership cases, nine private-preparation cases, and two new exact shell-output fixtures whose echo programs include two incidental trailing spaces while expecting one. The shell oracle is being corrected without trimming or relaxing exact output/file assertions. Full private preparation and retained Windows process ownership remain implementation requirements.

This receipt predates the subsequent bootstrap settlement, object-owned publication repair, isolated standard-user durability probe and import fix. Raw bytes and SHA-256 are retained.
