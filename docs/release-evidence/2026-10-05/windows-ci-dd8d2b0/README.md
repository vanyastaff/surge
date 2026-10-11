# Native Windows result at dd8d2b0

Source: `dd8d2b07e0e2c2582d17f966968015753f0896a7`. [CI run](https://github.com/vanyastaff/surge/actions/runs/37719859930), Windows job `113124792605`. Retrieved complete job log after authoritative terminal failure.

Windows nextest: **3703 run, 3505 passed, 198 failed, 47 skipped**. The separate mandatory standard-user NTFS step passed all **13 probes**, including the repaired unsafe-ACL fixture, unsafe sidefile/hardlink refusal, pathname aliases and first-observable private creation with zero-byte database and identity settlement. All other CI jobs passed. Subsequent Windows doctests/integration/smoke steps were skipped after the nextest failure.

This confirms the bounded ACL-fixture and nested-module repairs. It does not accept the privileged two-actor fixtures, the PID/control-lock candidate `19404ba`, private preparation, guardian ownership or complete Windows/release readiness. The native engine cleanup sharing failure also remains present. Overall verdict: **NO-GO**.

Raw compressed job output and selected source-bound receipts are retained here.
