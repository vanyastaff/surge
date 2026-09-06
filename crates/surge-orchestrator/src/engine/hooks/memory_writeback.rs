//! Memory write-back at a node's terminal failure.
//!
//! A memory write is a node *outcome*, never something the agent under
//! execution can trigger on its own initiative
//! (`.autopilot/competitive-waves/spec.md` §24, Histories 21/22/46/47;
//! R22/R23/R23.1). This module is wired from exactly one call site —
//! [`crate::engine::run_task`]'s `resolve_stage_error`, right after a
//! stage's failure becomes genuinely terminal (not suppressed by an
//! `on_error` hook) and its `StageFailed` event is already durable. A clean
//! run never reaches that call site, so it never calls into this module —
//! silence on a clean run is a property of *not being invoked*, not of an
//! internal "nothing to report" branch here.
//!
//! Four properties, matching the ticket's brief verbatim:
//!
//! 1. **Root cause over symptom** — see [`root_cause`].
//! 2. **Update in place over near-duplicate** — see [`write_claim`].
//! 3. **Silence on a clean run** — this module is simply never invoked.
//! 4. **Never on the agent's own initiative** — the only caller is engine
//!    code that runs after the agent's session has already ended.

use std::path::Path;

use surge_core::content_hash::ContentHash;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::memory::MemoryClaim;
use surge_core::run_event::EventPayload;
use surge_persistence::memory::MemoryStore;
use surge_persistence::runs::EventSeq;
use surge_persistence::runs::run_writer::RunWriter;

/// Record `node`'s failure in `run_id` as a memory claim. `symptom` is the
/// reason text `resolve_stage_error` already produced (and already
/// persisted in `StageFailed`); `stage_failed_seq` is the `EventSeq` that
/// `StageFailed` was assigned, used both as the write-back's "turn" number
/// and as the upper bound for the root-cause scan below it. `store_path_override`
/// is `EngineRunConfig::memory_store_path` threaded down from the run
/// config: `Some(path)` writes there instead (test-only — see that field's
/// doc comment), `None` (every production run) resolves
/// `MemoryStore::default_path()`.
///
/// Tolerant of every store failure: write-back must never be the reason a
/// run fails, mirroring `project_context::load_memory_claims_seed`'s
/// tolerance for a missing or unreadable store.
pub async fn record_node_failure(
    run_id: RunId,
    node: &NodeKey,
    symptom: &str,
    writer: &RunWriter,
    stage_failed_seq: EventSeq,
    store_path_override: Option<&Path>,
) {
    let cause = root_cause(node, symptom, writer, stage_failed_seq).await;
    let text = format!("{}{cause}", node_tag(node));

    let store_path = match store_path_override {
        Some(path) => path.to_path_buf(),
        None => match MemoryStore::default_path() {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(
                    target: "engine::memory_writeback",
                    node = %node,
                    run = %run_id,
                    %error,
                    "cannot resolve memory store path; skipping write-back"
                );
                return;
            },
        },
    };
    let store = match MemoryStore::open(&store_path) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(
                target: "engine::memory_writeback",
                node = %node,
                run = %run_id,
                %error,
                "memory store unavailable; skipping write-back"
            );
            return;
        },
    };

    match write_claim(&store, run_id, node, &text, stage_failed_seq.as_u64()) {
        Ok(true) => tracing::info!(
            target: "engine::memory_writeback",
            node = %node,
            run = %run_id,
            "existing memory claim about this node updated in place"
        ),
        Ok(false) => tracing::info!(
            target: "engine::memory_writeback",
            node = %node,
            run = %run_id,
            "memory claim recorded for node failure"
        ),
        Err(error) => tracing::warn!(
            target: "engine::memory_writeback",
            node = %node,
            run = %run_id,
            %error,
            "failed to persist memory claim for node failure"
        ),
    }
}

/// Tag every claim this module writes about `node` opens with, so a later
/// write-back can recognize "an existing claim about this same node" (see
/// [`write_claim`]). `NodeKey`'s own grammar (`^[A-Za-z][A-Za-z0-9_]*$`, no
/// quote character) rules out one node's tag ever forming a prefix of a
/// different node's.
fn node_tag(node: &NodeKey) -> String {
    format!("node '{node}' failed: ")
}

/// Root cause of `node`'s failure chain in this run, preferred over
/// `symptom` — the message `resolve_stage_error` already produced, which
/// for a multi-attempt chain (e.g. the `on_outcome` rejection budget) names
/// only the *last* attempt (`record_outcome_rejection`'s "last reject from
/// '<hook>'"). Scans this run's own event log, from the start up to (not
/// including) the just-appended `StageFailed` at `before`, for every
/// `OutcomeRejectedByHook` this node produced, and if any exist, names the
/// *first* rejecting hook — the one that started the chain — instead.
///
/// Falls back to `symptom` unchanged when no such chain exists: there is
/// then nothing upstream to prefer over it, and inventing one would
/// fabricate a cause that was never observed.
async fn root_cause(node: &NodeKey, symptom: &str, writer: &RunWriter, before: EventSeq) -> String {
    let Ok(events) = writer.read_events(EventSeq::ZERO..before).await else {
        return symptom.to_owned();
    };
    let first_rejecting_hook = events
        .iter()
        .find_map(|event| match &event.payload.payload {
            EventPayload::OutcomeRejectedByHook {
                node: rejected_node,
                hook_id,
                ..
            } if rejected_node == node => Some(hook_id.clone()),
            _ => None,
        });
    match first_rejecting_hook {
        Some(hook_id) => {
            format!("first rejected by hook '{hook_id}' (see the run log for the full retry chain)")
        },
        None => symptom.to_owned(),
    }
}

/// Persist `text` as a claim about `node`'s failure in `run_id`, sourced as
/// `transcript:run-<ULID>#turn-<turn>` per the memory-locator contract
/// (`.autopilot/competitive-waves/interfaces.md`, "Контракт локаторов
/// памяти") so a claim this function writes is actually parseable by
/// `surge_persistence::memory::audit`'s `run_id_from_source`, not merely
/// shaped like it is.
///
/// Before inserting, looks for an existing *aging-eligible* claim already
/// tagged for the same node ([`node_tag`], [`MemoryClaim::is_aging_eligible`])
/// and, if one covers it, updates it in place — same id, refreshed text and
/// provenance — instead of inserting a near-duplicate. A `Verified` claim
/// covering the same node is never eligible and is left completely alone
/// (R22): write-back only ever supersedes a claim nobody has proven yet.
///
/// Returns whether an existing claim was updated (`true`) or a fresh one
/// inserted (`false`).
fn write_claim(
    store: &MemoryStore,
    run_id: RunId,
    node: &NodeKey,
    text: &str,
    turn: u64,
) -> surge_persistence::Result<bool> {
    let source = format!("transcript:{run_id}#turn-{turn}");
    let hash = ContentHash::compute(text.as_bytes());
    let tag = node_tag(node);

    let covering = store
        .list_claims()?
        .into_iter()
        .find(|claim| claim.is_aging_eligible() && claim.text().starts_with(&tag));

    let Some(existing) = covering else {
        store.add_claim(&MemoryClaim::from_transcript(text, source, hash))?;
        return Ok(false);
    };

    store.delete_claim(existing.id())?;
    let refreshed = MemoryClaim::from_transcript(text, source, hash).with_id(existing.id());
    store.add_claim(&refreshed)?;
    Ok(true)
}
