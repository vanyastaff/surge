//! Skill resolution, trust-gating, and `SkillBound` event emission for a
//! node's declared skills (`AgentConfig::declared_skills`).
//!
//! Skills bind at stage entry — exactly like a context [`Binding`]
//! (`project.md`), never re-resolved mid-stage (R10): [`bind_skills`] is
//! called once, before the agent session opens, and every entry it produces
//! becomes a durable [`EventPayload::SkillBound`] event (R13) carrying the
//! node it bound onto, so the event alone reconstructs the "node → skill →
//! hash" triple without positional inference (R17). The caller is
//! responsible for actually delivering [`BoundSkill::instructions`] into the
//! agent's prompt (`engine::stage::agent`) — resolving-and-discarding them
//! would leave a skill logged but never bound in any way the agent can act
//! on.
//!
//! A declared skill that carries no hash, or a hash that no longer matches
//! the freshly-resolved pack, routes through the same generic
//! `HumanInputRequested`/`HumanInputResolved` pair (R15 / R15.1) that
//! `HumanGate` nodes use and that `surge inbox` / the Telegram cockpit
//! already render — not a bespoke approval event with no renderer. A denial
//! or an unanswered prompt means the node does not start: no `SkillBound`
//! is appended for it. `ApprovalConfig::skill_approval` can disable the
//! gate entirely for a node (default: enabled); every `SkillBound` it
//! produces still records whether the gate was active
//! (`gate_enabled: bool`), so a disabled protection is visible in the log
//! rather than silent.
//!
//! [`Binding`]: surge_core::agent_config::Binding

use std::time::Duration;

use surge_core::content_hash::ContentHash;
use surge_core::keys::NodeKey;
use surge_core::run_event::{EventPayload, VersionedEventPayload};
use surge_core::skill::{ResolvedSkill, SkillCatalog, SkillError, SkillProvider, SkillRef};
use surge_persistence::runs::run_writer::RunWriter;
use tokio::sync::oneshot;

use crate::engine::stage::StageError;
use crate::engine::stage::human_gate::GateResolutions;

/// A skill that ended up bound onto a node: everything the caller needs to
/// inject its content into the agent's prompt without re-resolving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundSkill {
    /// The skill's declared name.
    pub name: String,
    /// Which root the bound pack was found under.
    pub provider: SkillProvider,
    /// Content hash of the pack as resolved at bind time.
    pub hash: ContentHash,
    /// The pack's instructions body (`SKILL.md` with frontmatter stripped) —
    /// this is what must reach the agent's system prompt.
    pub instructions: String,
}

/// Parameters for binding a node's declared skills at stage entry.
pub struct SkillBindingParams<'a> {
    /// The node the skills are binding onto.
    pub node: &'a NodeKey,
    /// Skills declared on the node (`AgentConfig::declared_skills()`).
    pub declared: &'a [SkillRef],
    /// Catalog to resolve `declared` against.
    pub catalog: &'a SkillCatalog,
    /// Run writer for persisting `HumanInputRequested`/`HumanInputResolved`/
    /// `HumanInputTimedOut`/`SkillBound` events.
    pub writer: &'a RunWriter,
    /// `ApprovalConfig::skill_approval` for this node, resolved by the
    /// caller. `true` (the default) enforces the pin/hash trust check below;
    /// `false` binds every declared skill immediately, with `gate_enabled:
    /// false` recorded on each `SkillBound` so the disabled protection
    /// stays visible in the log.
    pub gate_enabled: bool,
    /// Node-keyed decision registry `Engine::resolve_human_input` drains
    /// (`RunTaskParams::gate_resolutions`). A sender is registered into it
    /// only at the moment a prompt is actually requested — never eagerly —
    /// so a node whose skills all bind without approval never touches the
    /// registry at all, and an entry can never sit unread waiting on a
    /// decision nobody will ever send. `None` means no operator can answer
    /// (the approval always times out and denies, fail-closed); production
    /// always passes `Some`.
    pub gate_resolutions: Option<&'a GateResolutions>,
    /// How long to wait for a decision before treating the prompt as denied.
    pub approval_timeout: Duration,
}

/// Bind `params.declared` skills onto `params.node`.
///
/// Resolves every declared skill against `params.catalog`, gates any
/// unpinned or content-changed one behind a single node-level approval
/// round trip (unless `params.gate_enabled` is `false`), and — only once
/// every skill is trusted — appends one `SkillBound` event per declared
/// skill, in declaration order, and returns the resolved [`BoundSkill`]
/// list so the caller can inject `instructions` into the agent's prompt. No
/// event is appended for a node whose approval is denied: partial trust is
/// not partial binding.
///
/// # Errors
/// - [`StageError::SkillResolutionFailed`] when a declared skill fails to
///   resolve (not found, malformed manifest, or ambiguous content).
/// - [`StageError::SkillApprovalRejected`] when approval was required and
///   the operator denied it, or the prompt timed out unanswered.
pub async fn bind_skills(params: SkillBindingParams<'_>) -> Result<Vec<BoundSkill>, StageError> {
    if params.declared.is_empty() {
        return Ok(Vec::new());
    }

    let mut resolved: Vec<DeclaredResolution<'_>> = Vec::with_capacity(params.declared.len());
    for declared_ref in params.declared {
        resolved.push(resolve_declared(params.catalog, declared_ref)?);
    }

    if params.gate_enabled {
        let pending: Vec<&DeclaredResolution<'_>> = resolved
            .iter()
            .filter(|r| r.approval_reason.is_some())
            .collect();

        if !pending.is_empty() {
            let prompt = render_approval_prompt(&pending);
            request_and_await_approval(
                params.node,
                params.writer,
                prompt,
                params.gate_resolutions,
                params.approval_timeout,
            )
            .await?;
        }
    }

    let mut bound = Vec::with_capacity(resolved.len());
    for r in resolved {
        params
            .writer
            .append_event(VersionedEventPayload::new(EventPayload::SkillBound {
                node: params.node.clone(),
                name: r.declared.name.clone(),
                provider: r.declared.provider,
                hash: r.resolved.hash,
                gate_enabled: params.gate_enabled,
            }))
            .await
            .map_err(|e| StageError::Storage(e.to_string()))?;
        bound.push(BoundSkill {
            name: r.declared.name.clone(),
            provider: r.declared.provider,
            hash: r.resolved.hash,
            instructions: r.resolved.instructions,
        });
    }

    Ok(bound)
}

/// Why a declared skill's current resolution needs operator approval before
/// it may bind. `None` (kept as the caller's `approval_reason: Option<_>`,
/// not a variant here) means the declared pin matched the freshly-resolved
/// content exactly — no prompt needed.
#[derive(Debug)]
enum ApprovalReason {
    /// The declaration carried no hash at all.
    Unpinned,
    /// The declaration pinned a hash, but no pack currently on disk has that
    /// exact content — the pin has gone stale.
    Drifted { previous_pin: ContentHash },
    /// Name/provider(/version) matched more than one pack with genuinely
    /// different content — expected corpus shape (Решение §4; measured on
    /// `~/.claude/plugins`: 49 names each with up to five differing-content
    /// packs), not a resolution failure. Every current candidate's hash is
    /// listed for the operator; the lexicographically smallest is the one
    /// that binds if approved (a stable, deterministic tie-break, not a
    /// guess) — recorded in the accompanying `DeclaredResolution::resolved`.
    Ambiguous { candidates: Vec<ContentHash> },
}

/// One declared skill, resolved against the catalog, together with whatever
/// trust decision it needs before it may bind.
struct DeclaredResolution<'a> {
    declared: &'a SkillRef,
    resolved: ResolvedSkill,
    approval_reason: Option<ApprovalReason>,
}

/// Resolve one declared skill against `catalog`, deciding along the way
/// whether it needs operator approval.
///
/// - `declared.hash: Some(pin)` resolves **directly against that pin**
///   (never stripped to `None` first): [`SkillCatalog::resolve`] narrows a
///   `Some(hash)` lookup to that exact content, and candidates sharing a
///   hash are by definition identical content (Решение §4) — so this path
///   can only ever succeed or return [`SkillError::NotFound`], never
///   [`SkillError::Ambiguous`]. A match means the pin still holds: bind
///   immediately, no approval.
/// - A pinned lookup's `NotFound` means the pin no longer matches anything
///   on disk. Falls back to a name/provider(/version)-only lookup to see
///   what's actually there now — which may itself be ambiguous.
/// - `declared.hash: None`, or the pin's `NotFound` fallback above, resolves
///   by name/provider(/version) alone. A unique match needs approval
///   (`Unpinned` or `Drifted`, depending on whether a stale pin was
///   present). [`SkillError::Ambiguous`] here is not propagated as a
///   failure — the run must not die "half a corpus's worth" of the time on
///   a shape the catalog contract already documents as legitimate; instead
///   every current candidate is enumerated via `catalog.skills()` (every
///   discovered [`SkillRef`] always carries `Some(hash)`) and surfaced to
///   the operator as approval material.
///
/// # Errors
/// Any other [`SkillError`] (not found with no candidates at all, a
/// malformed manifest, or an I/O failure) propagates as
/// [`StageError::SkillResolutionFailed`].
fn resolve_declared<'a>(
    catalog: &SkillCatalog,
    declared: &'a SkillRef,
) -> Result<DeclaredResolution<'a>, StageError> {
    if declared.hash.is_some() {
        match catalog.resolve(declared) {
            Ok(resolved) => {
                return Ok(DeclaredResolution {
                    declared,
                    resolved,
                    approval_reason: None,
                });
            },
            Err(SkillError::NotFound { .. }) => {
                // Fall through: the pin is stale, see what's current below.
            },
            Err(other) => return Err(StageError::SkillResolutionFailed(other)),
        }
    }

    let unpinned_lookup = SkillRef {
        hash: None,
        ..declared.clone()
    };
    match catalog.resolve(&unpinned_lookup) {
        Ok(resolved) => {
            let approval_reason = Some(match declared.hash {
                Some(previous_pin) => ApprovalReason::Drifted { previous_pin },
                None => ApprovalReason::Unpinned,
            });
            Ok(DeclaredResolution {
                declared,
                resolved,
                approval_reason,
            })
        },
        Err(SkillError::Ambiguous { .. }) => {
            let mut candidates: Vec<ContentHash> = catalog
                .skills()
                .filter(|entry| {
                    entry.name == declared.name
                        && entry.provider == declared.provider
                        && declared
                            .version
                            .as_deref()
                            .is_none_or(|v| entry.version.as_deref() == Some(v))
                })
                .filter_map(|entry| entry.hash)
                .collect();
            candidates.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            candidates.dedup_by(|a, b| a.as_bytes() == b.as_bytes());
            let Some(&chosen) = candidates.first() else {
                // `Ambiguous` implies at least two differently-hashed
                // candidates; every discovered `SkillRef` carries `Some`.
                // Reaching here means `catalog.skills()` disagrees with
                // `resolve()` about what's on disk — an internal
                // inconsistency, not an operator-actionable condition.
                return Err(StageError::SkillResolutionFailed(SkillError::Ambiguous {
                    name: declared.name.clone(),
                    provider: declared.provider,
                }));
            };
            let pinned_lookup = SkillRef {
                hash: Some(chosen),
                ..declared.clone()
            };
            // Cannot itself be `Ambiguous`: `chosen` is a specific hash, and
            // a hash-pinned lookup only ever matches identical content.
            let resolved = catalog.resolve(&pinned_lookup)?;
            Ok(DeclaredResolution {
                declared,
                resolved,
                approval_reason: Some(ApprovalReason::Ambiguous { candidates }),
            })
        },
        Err(other) => Err(StageError::SkillResolutionFailed(other)),
    }
}

/// Human-readable summary of the skills awaiting approval, shown to the
/// operator as `HumanInputRequested::prompt`. Deterministically ordered
/// (sorted by name) so the message — and any test asserting on it — doesn't
/// depend on declaration order.
fn render_approval_prompt(pending: &[&DeclaredResolution<'_>]) -> String {
    let has_ambiguous = pending
        .iter()
        .any(|r| matches!(r.approval_reason, Some(ApprovalReason::Ambiguous { .. })));

    let mut lines: Vec<String> = pending
        .iter()
        .map(|r| match &r.approval_reason {
            Some(ApprovalReason::Unpinned) => format!(
                "- {} ({:?}): unpinned — would bind hash {}",
                r.declared.name, r.declared.provider, r.resolved.hash
            ),
            Some(ApprovalReason::Drifted { previous_pin }) => format!(
                "- {} ({:?}): pin drifted — declared {previous_pin}, now {}",
                r.declared.name, r.declared.provider, r.resolved.hash
            ),
            Some(ApprovalReason::Ambiguous { candidates }) => {
                let listed = candidates
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "- {} ({:?}): ambiguous — {} candidates with differing content [{listed}]; \
                     would bind {} (lexicographically smallest hash, deterministic tie-break)",
                    r.declared.name,
                    r.declared.provider,
                    candidates.len(),
                    r.resolved.hash,
                )
            },
            // Only entries with `approval_reason.is_some()` are ever passed
            // in — see `bind_skills`'s `pending` filter above.
            None => String::new(),
        })
        .collect();
    lines.sort_unstable();
    let mut prompt = String::from(
        "The following unpinned, content-drifted, or ambiguous skill(s) require \
         approval before this node can start:\n",
    );
    prompt.push_str(&lines.join("\n"));
    if has_ambiguous {
        // The tie-break is deterministic, not meaningful: on this corpus
        // "ambiguous" usually means several versions of one plugin, and
        // lexicographically-smallest-hash carries no claim to being the
        // newest, the most trusted, or the one the operator actually wants
        // — approving is only safe once that's understood as a choice, not
        // a rubber stamp, and the escape hatch is spelled out rather than
        // left implicit.
        prompt.push_str(
            "\n\nApproving an ambiguous entry above binds the listed hash chosen by a \
             deterministic tie-break (lexicographically smallest content hash) — this is \
             not a claim that it is the newest, most trusted, or otherwise preferred \
             candidate. To bind a different one instead, put its hash in the node's \
             `skills` declaration.",
        );
    }
    prompt
}

/// JSON Schema for the operator's response: an `approve`/`reject` outcome
/// plus an optional free-text comment — the same shape `HumanGate`'s own
/// generic (non-freetext) options schema produces.
fn approval_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "outcome": { "type": "string", "enum": ["approve", "reject"] },
            "comment": { "type": "string" },
        },
        "required": ["outcome"],
    })
}

/// Request approval for the pending skill set and wait for a decision (or
/// the timeout).
///
/// Delivered through the same `HumanInputRequested` / `HumanInputResolved` /
/// `HumanInputTimedOut` events a `HumanGate` node uses — the path
/// `cockpit::dispatch::decide_action` and `surge inbox` already render —
/// rather than the `ApprovalRequested`/`ApprovalDecided` pair, which no
/// surface renders. The decision-channel sender is registered into
/// `gate_resolutions` only here, immediately before the request is
/// durable, never earlier: a node whose skills never need this function has
/// no entry in the registry to leak, and a node that does can never leave a
/// stale unread entry behind (the bug this replaces).
async fn request_and_await_approval(
    node: &NodeKey,
    writer: &RunWriter,
    prompt: String,
    gate_resolutions: Option<&GateResolutions>,
    timeout: Duration,
) -> Result<(), StageError> {
    let rx = match gate_resolutions {
        Some(registry) => {
            let (tx, rx) = oneshot::channel();
            registry.lock().await.insert(node.clone(), tx);
            Some(rx)
        },
        None => None,
    };

    writer
        .append_event(VersionedEventPayload::new(
            EventPayload::HumanInputRequested {
                node: node.clone(),
                session: None,
                call_id: None,
                prompt,
                schema: Some(approval_response_schema()),
            },
        ))
        .await
        .map_err(|e| StageError::Storage(e.to_string()))?;

    let resolution = if let Some(rx) = rx {
        tokio::select! {
            resolved = rx => resolved.ok(),
            () = tokio::time::sleep(timeout) => None,
        }
    } else {
        tokio::time::sleep(timeout).await;
        None
    };

    // The registry entry is consumed by a resolving `Engine::resolve_human_
    // input` call already; this covers the timeout path, where it must
    // still be removed so a later, stray decision can't land on a receiver
    // nobody is waiting on anymore.
    if let Some(registry) = gate_resolutions {
        registry.lock().await.remove(node);
    }

    // Mirrors `HumanGate`'s own event shape: any actual response — approve
    // or reject — appends `HumanInputResolved` with the response verbatim;
    // only a genuine non-response (channel dropped or timeout elapsed)
    // appends `HumanInputTimedOut`. Only the canonical "approve" outcome
    // grants trust; anything else (an explicit "reject", or no response at
    // all) fails closed.
    match &resolution {
        Some(res) => {
            writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::HumanInputResolved {
                        node: node.clone(),
                        call_id: None,
                        response: res.response.clone(),
                    },
                ))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
        },
        None => {
            writer
                .append_event(VersionedEventPayload::new(
                    EventPayload::HumanInputTimedOut {
                        node: node.clone(),
                        call_id: None,
                        elapsed_seconds: u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX),
                    },
                ))
                .await
                .map_err(|e| StageError::Storage(e.to_string()))?;
        },
    }

    let approved = resolution
        .as_ref()
        .is_some_and(|res| res.outcome.as_ref() == "approve");
    if approved {
        Ok(())
    } else {
        Err(StageError::SkillApprovalRejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::stage::human_gate::HumanGateResolution;
    use std::collections::HashMap;
    use std::sync::Arc;
    use surge_core::skill::{SkillProvider, SkillRoot};
    use surge_persistence::runs::Storage;

    /// Writes a single Agent Skills pack (`SKILL.md` only) under
    /// `root/<dir_name>/SKILL.md` and returns the discovery root.
    fn write_project_skill(root: &std::path::Path, dir_name: &str, name: &str, body: &str) {
        let pack_dir = root.join(dir_name);
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(
            pack_dir.join("SKILL.md"),
            format!("---\nname: {name}\n---\n\n{body}\n"),
        )
        .unwrap();
    }

    fn project_root(skills_root: &std::path::Path) -> SkillRoot {
        SkillRoot {
            provider: SkillProvider::ProjectDir,
            path: skills_root.to_path_buf(),
        }
    }

    fn empty_registry() -> Arc<GateResolutions> {
        Arc::new(tokio::sync::Mutex::new(HashMap::new()))
    }

    /// Simulates `Engine::resolve_human_input`: polls `registry` until the
    /// entry `bind_skills` registers for `node` appears, then sends
    /// `outcome`. Modeling this as a concurrent poll (rather than
    /// pre-building the channel and handing `bind_skills` the receiver
    /// directly) exercises the exact lazy-registration path production
    /// uses — the sender genuinely does not exist until the moment a
    /// prompt is requested.
    fn spawn_operator_response(
        registry: Arc<GateResolutions>,
        node: NodeKey,
        outcome: &'static str,
    ) {
        tokio::spawn(async move {
            loop {
                let mut guard = registry.lock().await;
                if let Some(tx) = guard.remove(&node) {
                    drop(guard);
                    let _ = tx.send(HumanGateResolution {
                        outcome: surge_core::keys::OutcomeKey::try_from(outcome).unwrap(),
                        response: serde_json::json!({"outcome": outcome}),
                    });
                    return;
                }
                drop(guard);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
    }

    async fn payload_kinds(
        storage: &std::sync::Arc<Storage>,
        run_id: surge_core::id::RunId,
    ) -> Vec<&'static str> {
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        events
            .into_iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pinned_matching_hash_binds_without_approval() {
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Review carefully.",
        );
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        // Learn the pack's real hash from the catalog itself (a dependency
        // of the code under test, not a re-derivation of it) so the pin
        // below is a known-correct value, not a guess.
        let probe = SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        };
        let real = catalog.resolve(&probe).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: Some(real.hash),
        }];

        let registry = empty_registry();
        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_millis(20),
        })
        .await;

        let bound = result.expect("expected pinned+matching skill to bind");
        assert_eq!(bound.len(), 1);
        assert_eq!(bound[0].name, "code-reviewer");
        assert_eq!(bound[0].hash, real.hash);
        assert!(
            bound[0].instructions.contains("Review carefully"),
            "resolved instructions must reach the caller for prompt injection: {:?}",
            bound[0].instructions,
        );
        assert!(
            registry.lock().await.is_empty(),
            "a bind that never needs approval must never touch the gate registry"
        );

        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(kinds, vec!["SkillBound"], "no approval round trip expected");

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        match &events[0].payload.payload {
            EventPayload::SkillBound {
                node: bound_node,
                name,
                provider,
                hash,
                gate_enabled,
            } => {
                assert_eq!(*bound_node, node);
                assert_eq!(name, "code-reviewer");
                assert_eq!(*provider, SkillProvider::ProjectDir);
                assert_eq!(*hash, real.hash);
                assert!(*gate_enabled);
            },
            other => panic!("expected SkillBound, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unpinned_skill_approved_binds_after_decision() {
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Review carefully.",
        );
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let registry = empty_registry();
        spawn_operator_response(registry.clone(), node.clone(), "approve");

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_secs(5),
        })
        .await;

        let bound = result.expect("expected approved skill to bind");
        assert_eq!(bound.len(), 1);
        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(
            kinds,
            vec!["HumanInputRequested", "HumanInputResolved", "SkillBound"],
        );
        assert!(registry.lock().await.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn denied_approval_rejects_and_node_does_not_start() {
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Review carefully.",
        );
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let registry = empty_registry();
        spawn_operator_response(registry.clone(), node.clone(), "reject");

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_secs(5),
        })
        .await;

        assert!(
            matches!(result, Err(StageError::SkillApprovalRejected)),
            "expected denial to reject the node start, got {result:?}"
        );
        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(kinds, vec!["HumanInputRequested", "HumanInputResolved"]);
        assert!(
            !kinds.contains(&"SkillBound"),
            "a denied skill must never be bound, got {kinds:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unanswered_prompt_times_out_through_a_real_registry_and_denies() {
        // Uses a live, wired registry that simply never receives a
        // resolution — the shape a production run actually exhibits when
        // nobody answers in time (the engine always passes `Some`; the
        // caller never has "no registry available" in the real path).
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Review carefully.",
        );
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let registry = empty_registry();
        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_millis(10),
        })
        .await;

        assert!(matches!(result, Err(StageError::SkillApprovalRejected)));
        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(kinds, vec!["HumanInputRequested", "HumanInputTimedOut"]);
        assert!(
            registry.lock().await.is_empty(),
            "the timed-out entry must be removed, not left stale in the registry"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_registry_wired_fails_closed_without_touching_a_registry() {
        // `gate_resolutions: None` — no operator could ever be reached
        // (e.g. a caller with no live decision registry at all). Distinct
        // from the real-registry timeout above: this path never creates a
        // channel or inserts anywhere.
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Review carefully.",
        );
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: None,
            approval_timeout: Duration::from_millis(10),
        })
        .await;

        assert!(matches!(result, Err(StageError::SkillApprovalRejected)));
        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(kinds, vec!["HumanInputRequested", "HumanInputTimedOut"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pack_changed_since_pin_requires_approval_and_binds_fresh_hash() {
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Original body.",
        );
        let catalog_at_pin_time = SkillCatalog::discover(&[project_root(skills_dir.path())]);
        let probe = SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        };
        let pinned = catalog_at_pin_time.resolve(&probe).unwrap();

        // The pack changes on disk between pin and run — a real edit, not a
        // substituted foreign hash.
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Edited body.",
        );
        let catalog_at_run_time = SkillCatalog::discover(&[project_root(skills_dir.path())]);
        let fresh = catalog_at_run_time.resolve(&probe).unwrap();
        assert_ne!(
            pinned.hash, fresh.hash,
            "editing the pack must change its hash for this test to mean anything"
        );

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: Some(pinned.hash),
        }];

        let registry = empty_registry();
        spawn_operator_response(registry.clone(), node.clone(), "approve");

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog_at_run_time,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_secs(5),
        })
        .await;

        let bound = result.expect("operator approved the drifted pack");
        assert_eq!(bound[0].hash, fresh.hash);

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        let kinds: Vec<&str> = events
            .iter()
            .map(|re| re.payload.payload.discriminant_str())
            .collect();
        assert_eq!(
            kinds,
            vec!["HumanInputRequested", "HumanInputResolved", "SkillBound"],
            "a stale pin must still go through approval, not bind silently"
        );
        let bound_hash = events
            .iter()
            .find_map(|re| match &re.payload.payload {
                EventPayload::SkillBound { hash, .. } => Some(*hash),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            bound_hash, fresh.hash,
            "the bound hash must be the freshly-resolved content, not the stale pin"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn skill_approval_disabled_binds_without_prompt_and_records_gate_disabled() {
        // `ApprovalConfig::skill_approval == false` for this node: an
        // otherwise-unpinned skill (which would normally block on approval)
        // binds immediately, and the event log carries the fact that the
        // gate was off rather than silently omitting it.
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "reviewer",
            "code-reviewer",
            "Review carefully.",
        );
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "code-reviewer".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let registry = empty_registry();
        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: false,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_millis(10),
        })
        .await;

        assert!(
            result.is_ok(),
            "disabled gate must bind, not block: {result:?}"
        );
        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(
            kinds,
            vec!["SkillBound"],
            "no approval round trip when the gate is disabled"
        );
        assert!(registry.lock().await.is_empty());

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        match &events[0].payload.payload {
            EventPayload::SkillBound { gate_enabled, .. } => {
                assert!(
                    !gate_enabled,
                    "the disabled protection must be visible in the log, not silent"
                );
            },
            other => panic!("expected SkillBound, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unknown_skill_surfaces_typed_resolution_error() {
        let skills_dir = tempfile::tempdir().unwrap();
        // Root exists but has no packs at all.
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "does-not-exist".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: None,
            approval_timeout: Duration::from_millis(10),
        })
        .await;

        assert!(matches!(result, Err(StageError::SkillResolutionFailed(_))));
        let kinds = payload_kinds(&storage, run_id).await;
        assert!(
            kinds.is_empty(),
            "no events for a node that never resolved its skills"
        );
    }

    /// R17 / "test reads the log back": binds two different nodes in the
    /// same run and reconstructs the "node → skill → hash" triple purely
    /// by reading `SkillBound` events back out of the event log — not by
    /// asserting the call succeeded or that some event was appended.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn event_log_alone_reconstructs_node_skill_hash_for_every_bound_node() {
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(skills_dir.path(), "reviewer", "code-reviewer", "Review.");
        write_project_skill(skills_dir.path(), "planner", "planning-guide", "Plan.");
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let reviewer_hash = catalog
            .resolve(&SkillRef {
                name: "code-reviewer".into(),
                provider: SkillProvider::ProjectDir,
                version: None,
                hash: None,
            })
            .unwrap()
            .hash;
        let planner_hash = catalog
            .resolve(&SkillRef {
                name: "planning-guide".into(),
                provider: SkillProvider::ProjectDir,
                version: None,
                hash: None,
            })
            .unwrap()
            .hash;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();

        let review_node = surge_core::keys::NodeKey::try_from("review_1").unwrap();
        let plan_node = surge_core::keys::NodeKey::try_from("plan_1").unwrap();

        // Two separate stage entries against the same run's log — the
        // production shape (one `bind_skills` call per node, sequentially).
        bind_skills(SkillBindingParams {
            node: &review_node,
            declared: &[SkillRef {
                name: "code-reviewer".into(),
                provider: SkillProvider::ProjectDir,
                version: None,
                hash: Some(reviewer_hash),
            }],
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: None,
            approval_timeout: Duration::from_millis(20),
        })
        .await
        .unwrap();

        bind_skills(SkillBindingParams {
            node: &plan_node,
            declared: &[SkillRef {
                name: "planning-guide".into(),
                provider: SkillProvider::ProjectDir,
                version: None,
                hash: Some(planner_hash),
            }],
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: None,
            approval_timeout: Duration::from_millis(20),
        })
        .await
        .unwrap();

        // Read the log back from scratch — a fresh reader, not the writer's
        // own in-memory state — and fold only `SkillBound` events into the
        // triple under test.
        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        let reconstructed: HashMap<surge_core::keys::NodeKey, (String, ContentHash)> = events
            .iter()
            .filter_map(|re| match &re.payload.payload {
                EventPayload::SkillBound {
                    node, name, hash, ..
                } => Some((node.clone(), (name.clone(), *hash))),
                _ => None,
            })
            .collect();

        assert_eq!(
            reconstructed.get(&review_node),
            Some(&("code-reviewer".to_string(), reviewer_hash)),
            "the log alone must attribute the reviewer skill to review_1"
        );
        assert_eq!(
            reconstructed.get(&plan_node),
            Some(&("planning-guide".to_string(), planner_hash)),
            "the log alone must attribute the planning skill to plan_1"
        );
    }

    /// Real corpus shape (measured on `~/.claude/plugins`: 49 names, each
    /// with up to five differing-content packs): two physically distinct
    /// packs share `name`/`provider` but hash differently.
    fn write_two_differing_packs_with_shared_name(root: &std::path::Path, name: &str) {
        write_project_skill(root, "copy-a", name, "Body A.");
        write_project_skill(root, "copy-b", name, "Body B.");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pinned_skill_binds_immediately_even_when_other_candidates_are_ambiguous() {
        // The pin narrows the lookup to exact content (Решение §4): a
        // hash-pinned resolve can only ever match identical content, so the
        // presence of *other* differently-hashed packs sharing the name
        // must never surface as `Ambiguous` on this path.
        let skills_dir = tempfile::tempdir().unwrap();
        write_two_differing_packs_with_shared_name(skills_dir.path(), "verify-loop");
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let probe = SkillRef {
            name: "verify-loop".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        };
        // Confirm the corpus shape actually produced ambiguity for the
        // unpinned lookup, so this test means something.
        assert!(matches!(
            catalog.resolve(&probe),
            Err(SkillError::Ambiguous { .. })
        ));

        // Learn one real candidate's hash directly from the catalog's own
        // listing (a dependency of the code under test, not a
        // re-derivation of it).
        let one_candidate_hash = catalog
            .skills()
            .find(|s| s.name == "verify-loop")
            .and_then(|s| s.hash)
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "verify-loop".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: Some(one_candidate_hash),
        }];

        let registry = empty_registry();
        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_millis(20),
        })
        .await;

        let bound = result.expect("a pin must bind unambiguously despite other candidates");
        assert_eq!(bound[0].hash, one_candidate_hash);
        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(
            kinds,
            vec!["SkillBound"],
            "a matching pin must never prompt, even amid ambiguous siblings"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unpinned_ambiguous_skill_requests_approval_naming_every_candidate() {
        let skills_dir = tempfile::tempdir().unwrap();
        write_two_differing_packs_with_shared_name(skills_dir.path(), "test-plan");
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let mut candidate_hashes: Vec<ContentHash> = catalog
            .skills()
            .filter(|s| s.name == "test-plan")
            .filter_map(|s| s.hash)
            .collect();
        candidate_hashes.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        assert_eq!(
            candidate_hashes.len(),
            2,
            "fixture must produce two distinct hashes"
        );
        let expected_choice = candidate_hashes[0];

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "test-plan".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None, // unpinned — the ambiguity must surface as approval material, not a hard failure
        }];

        let registry = empty_registry();
        spawn_operator_response(registry.clone(), node.clone(), "approve");

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_secs(5),
        })
        .await;

        let bound = result.expect(
            "an unpinned ambiguous name must go to approval, not fail the node before anyone is asked",
        );
        assert_eq!(
            bound[0].hash, expected_choice,
            "approval must bind the deterministic (lexicographically smallest hash) candidate"
        );

        let reader = storage.open_run_reader(run_id).await.unwrap();
        let events = reader
            .read_events(
                surge_persistence::runs::EventSeq(0)..surge_persistence::runs::EventSeq(64),
            )
            .await
            .unwrap();
        let prompt = events
            .iter()
            .find_map(|re| match &re.payload.payload {
                EventPayload::HumanInputRequested { prompt, .. } => Some(prompt.clone()),
                _ => None,
            })
            .expect("HumanInputRequested must have been appended");
        for hash in &candidate_hashes {
            assert!(
                prompt.contains(&hash.to_string()),
                "prompt must name every candidate so the operator can decide with full information: {prompt}"
            );
        }
        assert!(
            prompt.to_lowercase().contains("ambiguous"),
            "prompt must say why approval is being asked: {prompt}"
        );

        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(
            kinds,
            vec!["HumanInputRequested", "HumanInputResolved", "SkillBound"],
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unpinned_ambiguous_skill_denied_does_not_bind() {
        let skills_dir = tempfile::tempdir().unwrap();
        write_two_differing_packs_with_shared_name(skills_dir.path(), "tech-debt");
        let catalog = SkillCatalog::discover(&[project_root(skills_dir.path())]);

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "tech-debt".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: None,
        }];

        let registry = empty_registry();
        spawn_operator_response(registry.clone(), node.clone(), "reject");

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_secs(5),
        })
        .await;

        assert!(matches!(result, Err(StageError::SkillApprovalRejected)));
        let kinds = payload_kinds(&storage, run_id).await;
        assert!(!kinds.contains(&"SkillBound"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_pin_falling_back_to_now_ambiguous_candidates_still_requests_approval() {
        // The pack the operator originally pinned is gone; two *different*
        // packs now share the name. This must fall back to the ambiguous
        // path, not `NotFound`.
        let skills_dir = tempfile::tempdir().unwrap();
        write_project_skill(
            skills_dir.path(),
            "copy-a",
            "go-modern-guidelines",
            "Original.",
        );
        let catalog_at_pin_time = SkillCatalog::discover(&[project_root(skills_dir.path())]);
        let stale_pin = catalog_at_pin_time
            .resolve(&SkillRef {
                name: "go-modern-guidelines".into(),
                provider: SkillProvider::ProjectDir,
                version: None,
                hash: None,
            })
            .unwrap()
            .hash;

        // The originally-pinned content is replaced (not just edited), and
        // a second, differently-hashed pack with the same name appears —
        // the pin now matches nothing, and what remains is ambiguous.
        std::fs::write(
            skills_dir.path().join("copy-a/SKILL.md"),
            "---\nname: go-modern-guidelines\n---\n\nReplaced.\n",
        )
        .unwrap();
        write_project_skill(
            skills_dir.path(),
            "copy-b",
            "go-modern-guidelines",
            "A different second candidate.",
        );
        let catalog_now = SkillCatalog::discover(&[project_root(skills_dir.path())]);
        assert!(matches!(
            catalog_now.resolve(&SkillRef {
                name: "go-modern-guidelines".into(),
                provider: SkillProvider::ProjectDir,
                version: None,
                hash: None,
            }),
            Err(SkillError::Ambiguous { .. })
        ));

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();
        let run_id = surge_core::id::RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();

        let declared = vec![SkillRef {
            name: "go-modern-guidelines".into(),
            provider: SkillProvider::ProjectDir,
            version: None,
            hash: Some(stale_pin),
        }];

        let registry = empty_registry();
        spawn_operator_response(registry.clone(), node.clone(), "approve");

        let result = bind_skills(SkillBindingParams {
            node: &node,
            declared: &declared,
            catalog: &catalog_now,
            writer: &writer,
            gate_enabled: true,
            gate_resolutions: Some(&registry),
            approval_timeout: Duration::from_secs(5),
        })
        .await;

        assert!(
            result.is_ok(),
            "a stale pin falling back to an ambiguous current state must still reach approval: {result:?}"
        );
        let kinds = payload_kinds(&storage, run_id).await;
        assert_eq!(
            kinds,
            vec!["HumanInputRequested", "HumanInputResolved", "SkillBound"],
        );
    }
}
