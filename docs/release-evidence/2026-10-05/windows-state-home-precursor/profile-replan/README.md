# Explicit-token Windows profile fixture

Native source 965ac42 reached only the unsafe-home behavioral assertion; the two positive fixture paths failed at null-token Known Folder lookup before exercising the backend. The [native receipt](../../windows-ci-965ac42/README.md) retains the exact distinction.

The reviewed replan retains the actual process query token and resolves its profile with GetUserProfileDirectoryW. It validates sizing/error/UTF-16 output, expected TokenUser, fixed NTFS, retained non-reparse ancestor routing and actual profile/fixture ownership before returning a temporary root. The three backend test bodies remain byte-identical to 7a4b9f4. No ACL repair, public-path fallback or test disabling was added.

Independent spec and quality/unsafe reviews accepted source SHA `6dd869fdb473c7142f97a3ad8a163241daa3438a0fa2323ca0a469b891ce0cbb`. Direct rustfmt and scoped diff-check passed. A compile-only external harness includes the exact test file with real windows 0.58, tempfile, Tokio, rusqlite 0.32.1 and r2d2 types. Its non-executable Storage signature bridge cannot establish production integration or behavior. Strict x86_64-pc-windows-msvc tests Clippy passed in 9.02 seconds, exit 0:

```
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/surge-native-flush-actual-module/target cargo clippy --manifest-path /tmp/surge-stage1-fixture-typecheck/Cargo.toml --target x86_64-pc-windows-msvc --tests -- -D warnings
```

Native Windows compile, lint and execution of the actual persistence crate remain pending on this checkpoint. No backend GREEN is claimed.
