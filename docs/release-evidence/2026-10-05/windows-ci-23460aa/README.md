# Native Windows checkpoint: 23460aa

Source `23460aa31440bff21b511c5ae739ddb6147b3861`, [run 37691951295](https://github.com/vanyastaff/surge/actions/runs/37691951295), [Windows Test Suite](https://github.com/vanyastaff/surge/actions/runs/37691951295/job/113034068912).

Normal nextest: **3,654 run, 3,619 passed, 35 failed, 37 skipped**, in 344.498 seconds. The preceding 34 failures remain. One additional failure is `config::io::save_tests::continuous_readers_observe_complete_configs_from_all_eight_writers`: the reader's `read_to_string` at io.rs:391 returned Windows error 5 (PermissionDenied). The atomic configuration implementation and this oracle are unchanged since the earlier native passing checkpoints; this is an observed intermittent failure needing diagnosis, not a reason to ignore/retry the assertion. The combined helper does not yet identify whether open or read failed.

All platform Clippy jobs passed, including [Windows Clippy](https://github.com/vanyastaff/surge/actions/runs/37691951295/job/113034069047). macOS/Ubuntu tests, format, MSRV, packaging, dependency security/licenses and benchmark passed. This natively validates the prior Windows lint repair and compilation of the explicit-token profile fixture.

The dedicated standard-user NTFS flush probe passed **1/1**. The state-home batch again reported **0/3**:

- Unsafe inherited outsider grant reaches the intended refusal assertion and remains native behavioral RED.
- Both positive paths now resolve the explicit-token profile successfully, then stop at fixture line 193: actual profile ancestor owner is LocalSystem (`S-1-5-18`), not TokenUser (SID ending 1003). They still do not reach their backend assertions. This is a fixture owner-assumption failure, not a backend result.

The focused repair accepts the exact trusted ancestor owners already permitted by the reviewed policy (actual user, LocalSystem or Builtin Administrators), while keeping newly created fixture and private child ownership strictly current-user. It neither repairs ACLs nor changes the three backend test bodies. Independent security review accepted the correction; native positive-test RED remains required after it.

Raw unmodified job logs are compressed alongside SHA256SUMS. Full retained storage/private preparation and process guardian behavior remain unimplemented in this source; locally evolving work is not covered by this run.
