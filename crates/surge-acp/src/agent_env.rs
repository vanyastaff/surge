//! Spawn-time resolution of per-agent environment variables.
//!
//! `surge.toml` (and the builtin registry) describe agent env with
//! [`AgentEnvValue`]: a literal, or an injection of the operator's own
//! environment by variable **name** (`{ from = "OLLAMA_API_KEY" }`). This
//! module is the single place that turns that spec into concrete
//! `(name, value)` pairs at process-spawn time.
//!
//! Secret handling: the injected value is read with [`std::env::var`] and
//! never written to logs. [`AgentEnvError`] carries the variable *name* and
//! the agent id, never the value — the whole point of name indirection is
//! that the value exists only in the child's environment.
//!
//! # Required vs. optional
//!
//! An injection is `required = true` by default: a missing source variable
//! with no `default` is a typed error, so an agent never launches with a
//! silently absent credential. Set `required = false` for "the runtime has
//! its own fallback" cases (e.g. a local Ollama server, which needs no API
//! key). When `default` is present the injection always resolves.

use std::collections::BTreeMap;

use surge_core::config::AgentEnvValue;

/// Why an agent env spec could not be resolved.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentEnvError {
    /// A `required` injection's source variable is unset (or empty) and the
    /// spec carries no `default`. Names the variable and the agent, never
    /// any value.
    #[error(
        "agent '{agent_id}' requires environment variable '{var}' to be set \
         (referenced by env['{target}'])"
    )]
    MissingSource {
        /// Agent (registry id or config key) whose env spec failed.
        agent_id: String,
        /// Environment variable name the spec points at.
        var: String,
        /// Target variable the spec was going to set.
        target: String,
    },
    /// The spec carries a value shape this build does not understand.
    #[error("agent '{agent_id}' has an unsupported env value for '{target}'")]
    UnsupportedValue {
        /// Agent (registry id or config key) whose env spec failed.
        agent_id: String,
        /// Target variable whose value shape is unsupported.
        target: String,
    },
}

/// Resolve `spec` against the current process environment into concrete
/// `(name, value)` pairs. Keys are sorted by the `BTreeMap` iteration order,
/// so spawning is deterministic.
///
/// # Errors
///
/// Returns [`AgentEnvError::MissingSource`] for a required injection whose
/// source variable is unset.
pub fn resolve(
    agent_id: &str,
    spec: &BTreeMap<String, AgentEnvValue>,
) -> Result<BTreeMap<String, String>, AgentEnvError> {
    resolve_with(agent_id, spec, |name| std::env::var(name).ok())
}

/// Like [`resolve`], but reads source variables through `lookup` instead of
/// the process environment. Exists so tests (and callers that already hold a
/// resolved environment) can exercise the spec logic without mutating the
/// process environment — `std::env::set_var` is process-global and races
/// under the parallel test runner.
///
/// # Errors
///
/// Same as [`resolve`].
pub fn resolve_with(
    agent_id: &str,
    spec: &BTreeMap<String, AgentEnvValue>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<BTreeMap<String, String>, AgentEnvError> {
    let mut out = BTreeMap::new();
    for (target, value) in spec {
        match value {
            AgentEnvValue::Literal(literal) => {
                out.insert(target.clone(), literal.clone());
            },
            AgentEnvValue::Inject {
                from,
                default,
                required,
            } => {
                let resolved = lookup(from)
                    .filter(|v| !v.is_empty())
                    .or_else(|| default.clone());
                match resolved {
                    Some(v) => {
                        out.insert(target.clone(), v);
                    },
                    None if *required => {
                        return Err(AgentEnvError::MissingSource {
                            agent_id: agent_id.to_string(),
                            var: from.clone(),
                            target: target.clone(),
                        });
                    },
                    None => {},
                }
            },
            // `AgentEnvValue` is `#[non_exhaustive]`: a future value shape
            // surge does not yet understand must not silently vanish into
            // the child's environment — fail closed on the variable, naming
            // it, rather than launching with a partial env. The debug form of
            // the value is deliberately not included: it could be a literal
            // credential.
            _ => {
                return Err(AgentEnvError::UnsupportedValue {
                    agent_id: agent_id.to_string(),
                    target: target.clone(),
                });
            },
        }
    }
    Ok(out)
}

/// Best-effort detection of a literal that looks like a credential.
///
/// Uses the same pattern table the secret redactor uses
/// ([`crate::secrets::redact_secrets`]), so a value this function flags is a
/// value the redactor would have masked had it appeared in agent-visible
/// content. Config validation warns — it does not reject — because the
/// literal may be a placeholder; the warning names the key, not the value.
#[must_use]
pub fn looks_like_secret(value: &str) -> bool {
    !value.is_empty() && crate::secrets::redact_secrets(value).1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(value: AgentEnvValue) -> BTreeMap<String, AgentEnvValue> {
        let mut m = BTreeMap::new();
        m.insert("TARGET".to_string(), value);
        m
    }

    fn inject(from: &str, default: Option<&str>, required: bool) -> AgentEnvValue {
        AgentEnvValue::Inject {
            from: from.to_string(),
            default: default.map(str::to_string),
            required,
        }
    }

    #[test]
    fn literal_resolves_verbatim() {
        let out = resolve_with("a", &spec(AgentEnvValue::Literal("".into())), |_| None).unwrap();
        assert_eq!(out.get("TARGET").map(String::as_str), Some(""));
    }

    #[test]
    fn injection_reads_the_source_variable_at_spawn_time() {
        let out = resolve_with("a", &spec(inject("SRC", None, true)), |name| {
            (name == "SRC").then(|| "from-process".to_string())
        })
        .unwrap();
        assert_eq!(out.get("TARGET").map(String::as_str), Some("from-process"));
    }

    #[test]
    fn default_fills_a_missing_source() {
        let out = resolve_with("a", &spec(inject("SRC", Some("dflt"), true)), |_| None).unwrap();
        assert_eq!(out.get("TARGET").map(String::as_str), Some("dflt"));
    }

    #[test]
    fn missing_required_source_is_a_typed_error_naming_var_and_agent() {
        let err = resolve_with(
            "ollama-acp",
            &spec(inject("OLLAMA_MODEL", None, true)),
            |_| None,
        )
        .unwrap_err();
        assert_eq!(
            err,
            AgentEnvError::MissingSource {
                agent_id: "ollama-acp".into(),
                var: "OLLAMA_MODEL".into(),
                target: "TARGET".into(),
            }
        );
        let rendered = err.to_string();
        assert!(rendered.contains("OLLAMA_MODEL"), "names the variable");
        assert!(rendered.contains("ollama-acp"), "names the agent");
    }

    #[test]
    fn missing_optional_source_is_omitted_not_failed() {
        let out = resolve_with("a", &spec(inject("SRC", None, false)), |_| None).unwrap();
        assert!(!out.contains_key("TARGET"));
    }

    #[test]
    fn empty_source_falls_back_to_default() {
        let out = resolve_with("a", &spec(inject("SRC", Some("dflt"), true)), |_| {
            Some(String::new())
        })
        .unwrap();
        assert_eq!(out.get("TARGET").map(String::as_str), Some("dflt"));
    }

    #[test]
    fn injected_values_never_appear_in_the_error() {
        // A missing var cannot carry a value by definition, but guard the
        // contract the module doc states: the error names names.
        let err = resolve_with("a", &spec(inject("SRC", None, true)), |_| None).unwrap_err();
        let rendered = err.to_string();
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn literal_that_looks_like_a_key_is_flagged() {
        assert!(looks_like_secret(
            "sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890abcdef"
        ));
        assert!(looks_like_secret(
            "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890"
        ));
        assert!(!looks_like_secret(""));
        assert!(!looks_like_secret("http://localhost:11434"));
    }
}
