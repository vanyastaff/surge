# Readable Windows PID ownership candidate

Actual 04c native evidence contains ten failures caused by mandatory whole-file locks rejecting external PID readers (eight CLI startup/status and two PID unit tests). This candidate keeps the original protected descriptor and moves only control arbitration to an exclusive reserved byte overlapping legacy Rust whole-file locks. Control-file sharing denies independent writers/deleters while allowing ordinary readers; append/journal/SQLite sharing is unchanged.

Independent tests open read-only legacy handles successfully before lock assertions in both directions, so sharing rejection cannot mask range compatibility. They also check preexisting/new writers, delete/rename refusal, readable payload and no file extension. A mandatory standard-user wrapper checks actual SID/elevation and emits its receipt after both oracles; the launcher now has fourteen exact probes. Existing PID external-reader assertions remain, with production read_pid added.

Root owning architecture and independent pre-code security review accepted the plan; independent frozen-source and incremental security preflight accepted the exact candidate. Strict Windows-target Clippy passed for actual native/security/namespace/creation hook modules, including their cfg(test) code. The harness does not compile the outer control fixtures or daemon PID test. Full workspace formatting and diff checks pass. Source/commands/receipts are retained here.

Actual Windows compilation and execution of all relevant controls, legacy overlap and ten existing regressions remain required. Neither Stage1 nor Windows CI/release is accepted by these compile/preflight results.
