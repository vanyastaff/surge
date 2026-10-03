//! One read-only freshness projection shared by operator, ledger and reports.
use std::collections::BTreeMap;
use surge_core::{RunId, verification_evidence::ProofFreshness};
use surge_persistence::runs::Storage;

pub(super) fn current_proofs(
    storage: &Storage,
    run: RunId,
) -> Option<BTreeMap<String, ProofFreshness>> {
    let proofs = storage.inspect_verification_proofs(run).ok()?;
    let mut observations = BTreeMap::new();
    let mut states = BTreeMap::new();
    for proof in proofs {
        let observed = observations
            .entry(proof.binding.subject.worktree.clone())
            .or_insert_with(|| {
                surge_git::fingerprint::observe(&proof.binding.subject.worktree)
                    .ok()
                    .flatten()
            });
        let state = if let Some(observed) = observed {
            if observed.repository != proof.binding.subject.repository
                || observed.worktree != proof.binding.subject.worktree
                || !std::fs::read(&proof.report_path)
                    .is_ok_and(|bytes| surge_core::ContentHash::compute(&bytes) == proof.evidence)
            {
                ProofFreshness::Unknown
            } else if !proof.verified || observed.tree != proof.binding.subject.tree {
                ProofFreshness::Stale
            } else {
                ProofFreshness::Current
            }
        } else {
            ProofFreshness::Unknown
        };
        states.insert(proof.task_id, state);
    }
    Some(states)
}

pub(super) fn enrich_records(
    storage: &Storage,
    records: &mut [surge_persistence::task_ledger::TaskLedgerIndexRecord],
) {
    let mut runs = std::collections::HashMap::new();
    for record in records {
        let proofs = runs
            .entry(record.run_id)
            .or_insert_with(|| current_proofs(storage, record.run_id));
        record.freshness = match proofs {
            Some(proofs) => proofs
                .get(&record.task_id)
                .copied()
                .unwrap_or(ProofFreshness::Unbound),
            None => ProofFreshness::Unknown,
        };
        record.verified = record.verified && record.freshness == ProofFreshness::Current;
    }
}
