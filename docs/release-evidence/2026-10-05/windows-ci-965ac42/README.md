# Native Windows checkpoint: 965ac42

Source: `965ac42d21975ab1f094ca1e145e88333667964e`. [CI run](https://github.com/vanyastaff/surge/actions/runs/37687504548), [Windows Test Suite job](https://github.com/vanyastaff/surge/actions/runs/37687504548/job/113018996684).

The mandatory nextest suite ran **3,654 tests: 3,620 passed, 34 failed, 37 skipped**, in 312.084 seconds. The remaining failures are 16 quota-route and nine private start-preparation tests, plus five MCP ownership and four cold owned-process recovery tests. Windows CI remains failed. All eight new pure guardian identity tests passed natively; they do not implement process ownership.

The separate mandatory standard-user NTFS flush probe passed **1/1** again. The three new state-home checks were then explicitly executed under that same isolated account and reported **0 passed, 3 failed**. These failures have different meanings:

- `unsafe_existing_home_refuses_without_acl_or_byte_repair` reached its intended assertion: an inherited outsider Full grant was accepted instead of refusing before SQL. This is native behavioral RED.
- `new_state_home_has_protected_current_user_acl_after_open` and `derived_pool_retains_original_database_until_every_owner_drops` stopped during fixture setup at the null-token `SHGetKnownFolderPath` call, with `HRESULT(0x80070005)` access denied. They did not reach the backend assertions and supply no behavioral RED for those requirements.

The extra three ordinary-suite skips are these dedicated standard-user checks, explicitly run by the mandatory separate step; no existing test was disabled. The fixture replan uses an explicitly retained query token and `GetUserProfileDirectoryW`, retaining the independent owner, fixed-NTFS and non-reparse routing checks. It needs a new native run before any positive-test backend conclusion.

macOS/Ubuntu test suites and Clippy, formatting, MSRV, dependency security/licenses, packaging and benchmark jobs passed. Windows Clippy failed on two test-only style lints (`similar_names` and `assert_is_empty`), repaired separately in `7a4b9f4`; [the lint receipt](../windows-state-home-precursor/lint-repair/README.md) records that repair without claiming native revalidation.

`windows-test.log.gz` preserves the unmodified GitHub job log. SHA256SUMS identifies both compressed and uncompressed bytes. Full private storage and native guardian implementation remain open under plans 003/004.
