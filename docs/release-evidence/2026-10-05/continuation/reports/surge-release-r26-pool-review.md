# R26 independent bounded SQLite maintenance pool review

Read-only peer review; no Cargo or source writes. R25 owns implementation/testing. Reviewed actual r2d2-0.8.10 config/lib and scheduled-thread-pool-0.2.7 source plus current pool.rs RED probe.

## Accepted constraints and test evidence shape

r2d2 default creates three scheduled worker threads PER pool; builder.thread_pool is supported primary API. Sharing only maintenance executor retains independent connection limits, SQLite manager/PRAGMAs, DB isolation and exclusive writer leases. Normal user SQL/checkout executes outside maintenance executor. Connection creation/reaping can serialize across DBs under three threads; slow FS/SQLite init may delay other opens, existing30s pool timeouts/errors must remain visible. This is a documented tradeoff, not a reason to mask timeout failures.

on_acquire callback executes in r2d2 add_connection scheduled closure AFTER manager.connect, so ThreadId probe measures actual executor threads, independent of configured worker constant.64 retained actual readers+64 churned readers and causal nonzero count are credible thread-explosion oracle. Recommended stronger callback-count denominator and explicit writer.close().await fixture hygiene. IDs<=3 cannot establish maintenance job cleanup.

## Blocking lifetime finding

Naive process-permanent OnceLock<Arc<ScheduledThreadPool>> introduces accumulating perpetual reaper jobs under churn. Source proof:

- r2d2 lib.rs new_inner (~380): if max_lifetime or idle_timeout Some (defaults bothSome), execute_at_fixed_rate(reaper_rate, reaper_rate, move||reap_connections(&weak)); returnedJobHandle discarded.
- r2d2 lib.rs reap_connections (~284): expired Weak upgrade simply returns; no cancellation state.
- scheduled-thread-pool lib.rs Worker::run_job (~419): FixedRate invokes closure then unconditionally queues next job with same closure. JobHandle Drop itself does not cancel.

Existing per-pool executor drops when DBpool drops, removing its scheduler. A static executor never drops, so one inert recurring job remains for EVERY previously opened reader pool, even though no actual DBconnection is kept alive. Thus connection ownership/isolation okay but task queue/memory grows with historical churn, and three workers repeatedly execute dead jobs. Thread-count test alone misses this.

Sent exact dependency-source evidence to R25 and coordinator before production patch approval. Must establish lifecycle cancellation or equivalent bounded job lifetime; do not silently disable existing idle/max-lifetime policy or claim no leak based only on worker count.

Verdict NEEDS WORK naive-static plan; API-sharing/thread-oracle parts ACCEPTABLE. Runtime RED/GREEN owned by R25, not claimed executed by reviewer.

## Revised owned per-DB scheduler plan review

Coordinator selected narrower alternative: each independent DBpool still owns scheduler3, configured OnPoolDropBehavior::DiscardPendingScheduled; no static sharing/vendor/TTLchanges.

Pre-code source review ACCEPTABLE. r2d2 lib.rs scheduled sites: add_connection stores Weak then upgrades SharedPool before manager.connect/on_acquire; periodic reaper stores Weak then upgrades before actual work. SharedPool config contains Arc<ScheduledThreadPool>. LivePool/PooledConnection owns SharedPool; in-flight jobs upgrade and retain SharedPool. Consequently last ScheduledThreadPool Arc cannot disappear while a live DBpool or actual in-flight job exists. Pending jobs only carry Weak and have no durable write/ownership work to complete once owner is gone.

ScheduledThreadPool::Drop sets shutdown and notifies all; get_job Discard branch returns immediately with pending queue. Current run_job isn't interrupted and in-flight upgraded owner prevents scheduler destruction until completion. Queue jobs vanish with worker shared state after workers exit. While alive, max_lifetime/idle_timeout/reaper/connectiontimeout remain unchanged; perDB scheduler concurrency3 remains identical baseline. No crossDB serialization/starvation introduced, no global immortality.

New oracle TLS sentinel initialized from actual on_acquire, Drop records actual worker exit IDs after retained-reader reads + churn + writer.close + Storage drop; expect all measured identities exit within3s vs old next30s reaper delay. This measures OSthread completion independently of drop-policy constant. Source handles unmeasured idle worker via same get_job shutdown+notify path. Runtime RED/GREEN belongs R25; not yet claimed by reviewer.

Scope limitation: fixes prompt owner-lifetime cleanup, not hard global3-worker cap. Many genuinely retained DBpools still each consume3workers (same capacity contract); report must not claim strictprocesscap. Original naive-static lifetime blocker remains rejected; revised perDB plan addresses its cause without disabled checks.

## Final frozen-source acceptance review

Reviewed final pool.rs and both production callsites read-only. Source spec+quality ACCEPTABLE. Per-DB executor3 with DiscardPendingScheduled, original registrymax8/readersconfigmax, init PRAGMAs/error mapping preserved; no static/shared DBpool cache/TTLoverride/ownershipweakening. Debug observability records owned maintenance executor creation.

Probe parallel isolation confirmed: on_acquire uses actual connection.path canonicalized and component-prefix check against canonical unique fixturehome; foreign pools excluded. TLS Drop EXITED can remain global because ThreadId identities are unique and assertions intersect only owned observed identities. Connection counter scopes same owned filter. Retained64readers actually query expected run events, observedliveworkers must be disjoint exited, churn64 additional readers, explicit writer.close + Storagedrop precede exit measurement. Minimum130 causal acquisitions prevents vacuous probe. Deadlines poll exit evidence; they do not delay lifecycle to force it green.

Independently read actual RED `/tmp/surge-release-r25-pool-exit-red.log`:0of390 exited within3s,1testfail. Final GREEN `/tmp/surge-release-r25-pool-exit-scoped-green-retry.log`:1targettestpassed,390 observed identities exited,524 actual connection acquisitions,0.24s. Additional integration binaries show0testsfiltered, not extra proofs. Reviewer did not run Cargo. Strict persistenceclippy PASS provided by coordinator/owner; full root gates still pending.

Scope explicitly prompt retirement of expired per-pool workers. Live DBowners retain3threads EACH; no globalcap claim. Initial OS threadspawn failure remains possible at startup/resource exhaustion because dependency builder thread.spawn internally unwraps (baseline behavior); this fix removes30s orphan overlap, not all threadresource failure modes. No unlimited-lived reaper task accumulation introduced. Final acceptance ACCEPTABLE within stated scope; root full nextest/clippy/native/E2E remains release evidence gate.
