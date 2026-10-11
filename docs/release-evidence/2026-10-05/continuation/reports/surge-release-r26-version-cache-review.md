# R26 independent version cache identity/concurrency review

Read-only review version_probe.rs, local tokio OnceCell +which source, real counter fixture and executed R10 logs. No source edits/Cargo.

## Findings resolved

Original canonicalize→execute canonicaltarget changes multicallargv0 and collapses symlink aliases. Old unlocklookup→probe→insert admits duplicate concurrent samekey executions despite idempotence docs. Actual RED `/tmp/r10-version-red.log` independently read:3tests1pass2fail,alias expected2.0got1.0,counter expected1got2.

Final source spec+code ACCEPTABLE. Resolvebare names via which::which (noncanonical path API), then std::path::absolute without leaf canonicalization preserves command alias behavior and relative/cwd identity. Distinct aliases keep independent cache entries/results. Missing binaries retain typed SpawnFailed, cached as Result. Short HashMap mutex yields cloned Arc<OnceCell<Result>>; letcell statement releases mutex before get_or_initawait, so unrelatedkeys are not serialized across process execution. Samekey in-progress users share result cell. TokioOnceCell cancellation permits initretry; this is explicitly documented rather than overstating strict ever-once under cancellation. Existing1s timeout and errorvariants unchanged, no ignoredtests/lintsuppression.

Owned Unix shellfixture computes different version from actual$0 alias and appends actualsubprocesscounter. Alias test expects2actualcalls across2aliases/reuse and2cacheentries. join2samekey expectsactualcounter1, not merely equal returnedversions. Missingtypederror cache testcrossplatform. Independently read final `/tmp/r10-version-green.log`:12passed0failed,0.07s. Reviewer did not execute commands. Scopedstrictclippy R10running at review time; root fullgatespending.

## Exact limits

Normal samekey singleflight proven; cancellation before cellinit finishes can triggeranother attempt bytokio contract. Probeprocess lifecycle on timeout unchanged: cmd.output without kill_on_drop may let child finishlater. This preexisting behavior is not repaired or claimedresolved by current scopedidentity/cachefix. Windows positivealias/counter fixture excluded via cfg(unix), typedmissingprobe test remainscrossplatform; no Windowsruntimeevidenceclaimed. Replacing executable or PATH resolutionduringdaemonlife follows perinvocationpathcache lifetime, no freshness guaranteeadded.

Verdict ACCEPTABLE scoped release repair; no further actionable defect in finaldiff. Evidence must remain revisionbound by coordinator finalgates.
