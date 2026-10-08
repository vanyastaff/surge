# Windows CI at 5f42653

Workflow run 37721838146, job 113131022825 completed with failure. The nextest summary is 3705 run, 3516 passed, 189 failed and 48 skipped. Later doctest, helper and integration steps were skipped; this is not a complete Windows acceptance pass.

The separate non-elevated NTFS probe step succeeded. All 14 configured cases passed, including the new readable PID/control payload, competing-writer exclusion and bidirectional legacy whole-file lock compatibility case. This establishes the bounded native check for commit 19404ba on the exact pushed head. It does not establish privileged actor, private preparation, guardian or complete release readiness.

`receipt.json` records exact identifiers and the uncompressed raw log SHA-256. `windows-test.log.gz` preserves that raw output; `selected-results.txt` contains bounded summary and PASS lines. No tests or policies were disabled to obtain these results.
