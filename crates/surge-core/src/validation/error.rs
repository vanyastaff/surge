//! Diagnostic model: findings, their kinds and severities.

use crate::keys::{NodeKey, OutcomeKey, SubgraphKey};

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub kind: ValidationErrorKind,
    pub location: ErrorLocation,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ErrorLocation {
    Graph,
    Node { id: NodeKey },
    Edge { id: crate::keys::EdgeKey },
    Outcome { node: NodeKey, outcome: OutcomeKey },
    Subgraph { path: Vec<SubgraphKey> },
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ValidationErrorKind {
    StartNodeMissing,
    EdgeFromUnknownNode,
    EdgeToUnknownNode,
    EdgeFromUndeclaredOutcome,
    DuplicateEdgeFromSamePort,
    OutcomeWithNoEdge,
    UnreachableNode,
    NoTerminalReachable,
    InvalidProfileRef,
    HumanGateWithoutOptions,
    BranchWithoutArms,
    LoopIterableInvalid,
    LoopBodyMissingStart,
    SubgraphInvalid,
    TerminalOutcomeHasEdge,
    BacktrackTargetUnreachable,
    EscalateTargetNotHumanOrNotify,
    SchemaVersionMismatch,
    KeyFormatViolation {
        key: String,
    },
    SubgraphRefMissing {
        subgraph: SubgraphKey,
    },
    SubgraphReferenceCycle {
        cycle: Vec<SubgraphKey>,
    },
    NodeKeyCollision {
        key: NodeKey,
        locations: Vec<NodeKeyOrigin>,
    },
    OrphanSubgraph {
        key: SubgraphKey,
    },
    /// A `NodeKind::Notify` node does not declare the required `delivered`
    /// outcome. Engine emits this on every successful delivery, so the
    /// outcome must exist for routing to work.
    NotifyMissingDelivered {
        node: NodeKey,
    },
    /// A `NodeKind::Notify` node configured with `on_failure: Fail`
    /// does not declare an `undeliverable` outcome. Without it, a
    /// failed delivery in `Fail` mode produces `StageFailed` and halts
    /// the run. Warning, not error — authors may want fail-fast.
    NotifyFailMissingUndeliverable {
        node: NodeKey,
    },
    /// A `LoopConfig::iterates_over::Static` carries more than
    /// `MAX_LOOP_ITEMS_STATIC` (1000) items. Bound at graph-load time
    /// to prevent unbounded memory growth in the engine's frame stack.
    LoopStaticTooLarge {
        node: NodeKey,
        count: usize,
        max: usize,
    },
    /// A stage's `ToolOverride::mcp_add` references a server name not
    /// declared in `RunConfig::mcp_servers`.
    McpServerUndeclared {
        stage: String,
        server: String,
    },
    /// `McpServerRef::name` is empty.
    McpServerNameEmpty,
    /// `McpTransportConfig::Stdio::command` contains `..` segments.
    McpCommandPathUnsafe {
        command: String,
    },
    /// Agent node references a profile name that the supplied
    /// [`ReferenceResolver`] reports unknown. Surfaced only by
    /// [`validate_with_resolver`] — the syntactic [`validate`] entry point
    /// does not have a resolver.
    ProfileNotFound {
        node: NodeKey,
        profile: String,
    },
    /// Pipeline template name unknown to the resolver.
    TemplateNotFound {
        template: String,
    },
    /// Named-agent reference unknown to the resolver.
    NamedAgentNotFound {
        node: NodeKey,
        agent_id: String,
    },
    /// An `Agent` node's `sandbox_override` declared `mode = Custom` but every
    /// allowlist was empty — there is no signal for what the sandbox should
    /// permit.
    SandboxCustomEmpty {
        node: NodeKey,
    },
    /// An `Agent` node's `sandbox_override.writable_roots` contained a `..`
    /// segment, which would let the runtime resolve outside the intended
    /// root.
    SandboxWritableRootEscape {
        node: NodeKey,
        path: String,
    },
    /// An `Agent` node's `sandbox_override.network_allowlist` contained an
    /// entry that does not parse as a host or IP pattern.
    SandboxNetworkPatternInvalid {
        node: NodeKey,
        entry: String,
    },
    /// An `Agent` node's `sandbox_override.shell_allowlist` contained shell
    /// metacharacters that would let the agent chain commands past the
    /// allowlist.
    SandboxShellMetacharacters {
        node: NodeKey,
        entry: String,
    },
    /// A `Terminal { kind: Success }` node is reachable from `start` without
    /// passing through a verification-authority node (one that declares an
    /// outcome with [`LedgerEffect::Verified`](crate::node::LedgerEffect), or a
    /// Loop/Subgraph whose body contains one). Warning, not error — the run is
    /// structurally valid, but its "done" is not backed by a verifier, so it
    /// should render differently from a verified completion (Phase 1 A2).
    UnverifiedSuccessPath {
        terminal: NodeKey,
    },
    /// A verification-authority node (declares an outcome with
    /// [`LedgerEffect::Verified`](crate::node::LedgerEffect)) resolves to the
    /// **same** agent runtime as an implementer node it verifies — a node
    /// that declares [`LedgerEffect::ReadyForVerification`](crate::node::LedgerEffect)
    /// and from which the verifier is reachable over [`EdgeKind::Forward`].
    /// Warning, not error — the graph is structurally valid, but a
    /// same-vendor verifier cannot catch that vendor's own blind spots,
    /// which is exactly the failure mode cross-vendor verification exists to
    /// close (spec item 58: `runtime` ноды-верификатора ≠ `runtime`
    /// проверяемой ноды). The compared `runtime` is whatever
    /// `Profile.runtime.agent_id` resolves to — declared explicitly by the
    /// profile author or left at its serde default — either way it is what
    /// the engine actually dispatches on, so the finding is true regardless
    /// of which one produced it.
    ///
    /// Only [`validate_with_resolver`] can raise this — deciding "same
    /// runtime" needs [`ReferenceResolver::profile_runtime`], which the
    /// syntactic [`validate`] entry point has no resolver to call.
    /// `profile_runtime` returning `None` (profile does not resolve, or the
    /// resolver cannot answer runtime questions at all) never counts as a
    /// match — unknown is not "same".
    ///
    /// # What counts as the implementer, and the one caveat
    /// The implementer is the verifier's direct `EdgeKind::Forward`
    /// predecessor — graph shape, which every flow carries. Keying it on a
    /// declared `ready_for_verification` outcome instead would make the rule
    /// inert: no flow in this repository declares that ledger effect, names
    /// an outcome that way, or binds the `implementer@2.0` profile the
    /// flow-generator prompt describes. A node declaring *both*
    /// `ReadyForVerification` and `Verified` is additionally reported
    /// against itself — it verifies its own work, and needs no edge to say so.
    ///
    /// Caveat: top-level only. `agent_runtime` answers `None` for any
    /// non-`Agent` node, so a verifier reached only through
    /// `node_is_verification_gate`'s transitive Loop/Subgraph descent never
    /// pairs. W4 still uses that descent correctly.
    ///
    /// Measured against the bundled set on 2026-09-07: **4 of 13** flows
    /// raise this — `linear-3`, `linear-with-review`, `bug-fix`, `refactor`
    /// — because every bundled profile resolves to one runtime. It is a
    /// finding about what we ship, not noise.
    ///
    /// # Why the verifier gets artifacts, never the transcript
    /// A cross-vendor verifier's input is meant to be the diff, the
    /// originating spec with its constraints intact, and the evidence
    /// bundle — never the implementer's transcript (spec item 74). Per-
    /// boundary hallucination escape was measured at 24.6% → 48.3% → 89.3%
    /// as the same claim crosses one, two, then three narrative hand-offs
    /// (arXiv:2608.14588); the first boundary is still 75.4% catchable, the
    /// last has effectively erased the original checkable claim. That is the
    /// argument for a structured-artifact contract enforced at flow-load
    /// time, not for widening this rule into a transcript audit — the
    /// artifact shape itself is a separate, later deliverable.
    SameRuntimeVerification {
        implementer: NodeKey,
        verifier: NodeKey,
        runtime: String,
    },
    /// The graph has two or more work-producing `Agent` nodes but exactly
    /// **one** verification-authority node, so at most the final hand-off is
    /// gated and every earlier one passes unchecked.
    ///
    /// This is a different fault from [`Self::UnverifiedSuccessPath`], which
    /// fires when a success terminal is reachable with *no* verifier at all.
    /// Here a verifier exists; the objection is where it stands.
    ///
    /// # Why one gate at the end is close to none
    /// Instrumenting a four-agent pipeline with 346 injected hallucinations
    /// (arXiv:2608.14588) measured survival at 60.7% with no verification and
    /// **58.4% with end-of-pipeline checking** — a 2.3pp difference, against
    /// **16.2%** for the same detectors placed at every hand-off boundary
    /// (Cohen's *h* = −0.911, *p* < 0.000001). The mechanism is that errors
    /// transform as they are passed on: per-boundary escape rises
    /// 24.6% → 48.3% → 89.3%, so by the final gate the original checkable
    /// claim has been absorbed into narrative and no longer exists in
    /// verifiable form. The budget belongs at the *first* boundary, where
    /// 75.4% is still catchable.
    ///
    /// Warning, not error: a single-gate graph is structurally valid and
    /// may be deliberate for short flows. Counted across subgraph bodies,
    /// since loop archetypes put their verifier inside the task body.
    EndOfPipelineVerification {
        gate: NodeKey,
        work_nodes: usize,
    },
    /// An `Agent` node's `custom_fields["skills"]` does not deserialize as a
    /// list of `surge_core::skill::SkillRef` — a graph-authoring mistake,
    /// caught here so it fails the run at load time, before a worktree is
    /// even created, rather than lazily the first time that node's stage
    /// runs (History 15 / R09.1's "a broken declaration does not fail the
    /// run silently" standard, applied to skill declarations).
    InvalidSkillsDeclaration {
        node: NodeKey,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum NodeKeyOrigin {
    Root,
    Subgraph(SubgraphKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Severity {
    Error,
    Warning,
}

impl ValidationErrorKind {
    #[must_use]
    pub fn severity(&self) -> Severity {
        match self {
            // Warnings — informational, do not block the run.
            Self::EscalateTargetNotHumanOrNotify
            | Self::OrphanSubgraph { .. }
            | Self::NotifyFailMissingUndeliverable { .. }
            | Self::UnverifiedSuccessPath { .. }
            | Self::SameRuntimeVerification { .. }
            | Self::EndOfPipelineVerification { .. } => Severity::Warning,

            // Errors — graph is structurally invalid or will misbehave at runtime.
            Self::StartNodeMissing
            | Self::EdgeFromUnknownNode
            | Self::EdgeToUnknownNode
            | Self::EdgeFromUndeclaredOutcome
            | Self::DuplicateEdgeFromSamePort
            | Self::OutcomeWithNoEdge
            | Self::UnreachableNode
            | Self::NoTerminalReachable
            | Self::InvalidProfileRef
            | Self::HumanGateWithoutOptions
            | Self::BranchWithoutArms
            | Self::LoopIterableInvalid
            | Self::LoopBodyMissingStart
            | Self::SubgraphInvalid
            | Self::TerminalOutcomeHasEdge
            | Self::BacktrackTargetUnreachable
            | Self::SchemaVersionMismatch
            | Self::KeyFormatViolation { .. }
            | Self::SubgraphRefMissing { .. }
            | Self::SubgraphReferenceCycle { .. }
            | Self::NodeKeyCollision { .. }
            | Self::NotifyMissingDelivered { .. }
            | Self::LoopStaticTooLarge { .. }
            | Self::McpServerUndeclared { .. }
            | Self::McpServerNameEmpty
            | Self::McpCommandPathUnsafe { .. }
            | Self::ProfileNotFound { .. }
            | Self::TemplateNotFound { .. }
            | Self::NamedAgentNotFound { .. }
            | Self::SandboxCustomEmpty { .. }
            | Self::SandboxWritableRootEscape { .. }
            | Self::SandboxNetworkPatternInvalid { .. }
            | Self::SandboxShellMetacharacters { .. }
            | Self::InvalidSkillsDeclaration { .. } => Severity::Error,
        }
    }
}

impl ValidationError {
    /// Whether this finding blocks a run or is only advice.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.kind.severity()
    }
}
