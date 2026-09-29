//! Symbolic-reference lookup seam used by graph validation.

/// Lookup interface for resolving symbolic references during graph validation.
///
/// `surge-core` is leaf — it does not own a profile registry. The orchestrator
/// or daemon wires a real resolver backed by the project profile registry,
/// while tests substitute in-memory implementations. Pure read-only by design.
pub trait ReferenceResolver {
    /// Returns true when a profile with this name (e.g. `"implementer@1.0"`)
    /// is registered.
    fn profile_exists(&self, name: &str) -> bool;
    /// Returns true when a pipeline template with this name is registered.
    fn template_exists(&self, name: &str) -> bool;
    /// Returns true when the named-agent registry contains this id.
    fn named_agent_exists(&self, id: &str) -> bool;

    /// Returns the resolved profile's **canonical** agent runtime id — e.g.
    /// `"claude-acp"`, `"codex-acp"` — the identity
    /// [`ValidationErrorKind::SameRuntimeVerification`] compares between an
    /// implementer and its verifier. Implementors MUST normalize through the
    /// same alias table the engine dispatches through (`surge_acp::Registry`
    /// in production), not return `Profile.runtime.agent_id` verbatim: the
    /// registry maps several spellings to one runtime (`"claude"` and
    /// `"claude-code"` both → `"claude-acp"`; `"codex"` and `"codex-cli"`
    /// both → `"codex-acp"`), and two profiles naming the same runtime under
    /// different aliases must compare equal here or this rule stays silent
    /// on exactly the case it exists to catch. `None` means *unknown*: the
    /// profile does not resolve, or this resolver cannot answer runtime
    /// questions at all. Callers must never treat `None` as equal to
    /// anything — unknown is not "same".
    ///
    /// Defaulted so every existing implementor keeps compiling unchanged;
    /// only a resolver backed by a real profile registry should override it.
    fn profile_runtime(&self, _name: &str) -> Option<String> {
        None
    }
}

/// Permissive resolver that accepts every reference. Useful for tests and
/// for the engine's terminal-only smoke path where profiles are irrelevant.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoOpResolver;

impl ReferenceResolver for NoOpResolver {
    fn profile_exists(&self, _: &str) -> bool {
        true
    }
    fn template_exists(&self, _: &str) -> bool {
        true
    }
    fn named_agent_exists(&self, _: &str) -> bool {
        true
    }
}
