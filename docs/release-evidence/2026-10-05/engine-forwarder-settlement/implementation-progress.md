# Engine forwarder settlement — bounded implementation progress

Original manager-ownership oracle: actual RED retained 1/2 managers after completed
run; initial implementation GREEN released both. Original behavior assertion is
unchanged. Receipts and pre-edit hashes are in baseline-source-freeze.json and
initial-green-source-freeze.json. No speculative recovery of older source/logs.

Joint execution owner now awaits actual writer seal and finite forwarder exit,
compares sealed prefixes, preserves original outcome with independent typed failures,
and publishes Terminal only after confirmed settlement. Preparation failures close
writers before returning; terminal-history resume also seals and drains actual owners.
Public infinite subscriptions retain their existing semantics. Interrupted completion
is UNKNOWN; actual blocking SQL keeps its reader and original writer lease.

Independent root review found a real empty-read race. The post-read test freezes an
actual empty SQL result from Running, then appends/seals seq1 before returning that
captured result. It failed with ForwarderSequence instead of Ok(1). The minimal fix
only treats an empty read as a gap when that read began with a known sealed bound;
a read begun while Running must requery the newly sealed prefix. Exact test GREEN
and all six run_forwarder tests GREEN; see seal-race-red/green-source-freeze.json.

The six tests cover 600 fixed sequential rows, zero prefix, deliberate real SQLite
gap after explicitly disabling the fixture append-only trigger, final-drain writer
exclusion while a real connection is held, cancelled observer retaining actual SQL
ownership until runtime joins it, and the concurrent empty-read/seal case. Corruption
setup is test-only; production append-only triggers are unchanged.

Still open: full original plan's production-completion before-first-poll cancellation,
full success/failure/suspension/parked/start-resume matrix, injected error/closed lifecycle
and panic paths, controlled child runtime proof, all source/strict gates, independent
spec then quality review, and native Windows evidence. A separate durable host settlement
receipt design is required for restart/history consumers: terminal journal history alone
is not accepted as proof of the new join contract. No schema or guardian authority was
introduced here. Overall Stage1 remains open.
