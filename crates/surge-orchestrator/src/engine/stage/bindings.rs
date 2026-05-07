//! Resolve `Binding[]` from an `AgentConfig` into a template-substituted prompt.
//!
//! M5 supports:
//! - `ArtifactSource::RunArtifact`: looks up by name in `RunMemory::artifacts`
//! - `ArtifactSource::NodeOutput`: looks up the latest artifact produced by a node
//! - `ArtifactSource::Static`: literal content
//!
//! `ArtifactSource::GlobPattern` is M6+ — returns an error.

use std::path::Path;
use surge_core::agent_config::{ArtifactSource, Binding, TemplateVar};
use surge_core::run_state::RunMemory;

/// Errors returned by [`resolve_bindings`].
#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    /// The referenced artifact name is not present in `RunMemory`.
    #[error("unknown artifact name: {0}")]
    UnknownArtifact(String),
    /// The referenced node has not produced any artifacts yet.
    #[error("node {0} produced no artifacts")]
    NoArtifactsForNode(String),
    /// `GlobPattern` bindings are not supported until M6.
    #[error("GlobPattern bindings are M6+; not supported in M5")]
    GlobUnsupported,
    /// Reading the artifact file from disk failed.
    #[error("io error reading artifact {0}: {1}")]
    Io(String, std::io::Error),
}

/// Resolve a slice of [`Binding`]s against the current [`RunMemory`], returning
/// a list of `(TemplateVar, resolved_content)` pairs ready for template
/// substitution.
///
/// # Errors
/// Returns [`BindingError`] if an artifact is missing, a node has produced no
/// artifacts, or a `GlobPattern` source is encountered (unsupported in M5).
pub async fn resolve_bindings(
    bindings: &[Binding],
    memory: &RunMemory,
    worktree_root: &Path,
) -> Result<Vec<(TemplateVar, String)>, BindingError> {
    let mut out = Vec::with_capacity(bindings.len());
    for b in bindings {
        let value = match &b.source {
            ArtifactSource::RunArtifact { name } => {
                let aref = memory
                    .artifacts
                    .get(name)
                    .ok_or_else(|| BindingError::UnknownArtifact(name.clone()))?;
                read_artifact_text(&aref.path, worktree_root, &aref.name).await?
            },
            ArtifactSource::NodeOutput { node, artifact } => {
                let arefs = memory
                    .artifacts_by_node
                    .get(node)
                    .ok_or_else(|| BindingError::NoArtifactsForNode(node.to_string()))?;
                let aref = arefs
                    .iter()
                    .find(|a| &a.name == artifact)
                    .ok_or_else(|| BindingError::UnknownArtifact(artifact.clone()))?;
                read_artifact_text(&aref.path, worktree_root, &aref.name).await?
            },
            ArtifactSource::Static { content } => content.clone(),
            ArtifactSource::GlobPattern { .. } => return Err(BindingError::GlobUnsupported),
        };
        out.push((b.target.clone(), value));
    }
    Ok(out)
}

async fn read_artifact_text(
    path: &Path,
    worktree_root: &Path,
    name: &str,
) -> Result<String, BindingError> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree_root.join(path)
    };
    tokio::fs::read_to_string(&abs)
        .await
        .map_err(|e| BindingError::Io(name.to_string(), e))
}

/// Substitute `{{var}}` placeholders in `template` with `bindings`.
/// Unknown placeholders are left as-is (best-effort).
///
/// **Deprecated:** retained only for the legacy unit tests in this
/// module. New code uses [`crate::prompt::PromptRenderer`], which is
/// Handlebars-backed and validates templates at registry load time.
#[must_use]
#[deprecated(
    note = "use crate::prompt::PromptRenderer; substitute_template is the M5 fallback retained \
            only for the legacy substitute_template tests in this file"
)]
pub fn substitute_template(template: &str, bindings: &[(TemplateVar, String)]) -> String {
    let mut out = template.to_string();
    for (var, val) in bindings {
        let placeholder = format!("{{{{{}}}}}", var.0);
        out = out.replace(&placeholder, val);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_binding_resolves_immediately() {
        let bindings = vec![Binding {
            source: ArtifactSource::Static {
                content: "hello".into(),
            },
            target: TemplateVar("greeting".into()),
        }];
        let mem = RunMemory::default();
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_bindings(&bindings, &mem, dir.path()).await.unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].1, "hello");
    }

    #[test]
    #[allow(deprecated)]
    fn substitute_replaces_known_vars() {
        let bindings = vec![(TemplateVar("name".into()), "World".into())];
        let out = substitute_template("Hello, {{name}}!", &bindings);
        assert_eq!(out, "Hello, World!");
    }

    #[test]
    #[allow(deprecated)]
    fn substitute_leaves_unknown_vars_alone() {
        let bindings = vec![];
        let out = substitute_template("Hello, {{unknown}}!", &bindings);
        assert_eq!(out, "Hello, {{unknown}}!");
    }

    #[tokio::test]
    async fn glob_binding_returns_unsupported_error() {
        let bindings = vec![Binding {
            source: ArtifactSource::GlobPattern {
                node: surge_core::keys::NodeKey::try_from("x").unwrap(),
                pattern: "*.md".into(),
            },
            target: TemplateVar("v".into()),
        }];
        let mem = RunMemory::default();
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_bindings(&bindings, &mem, dir.path())
            .await
            .unwrap_err();
        assert!(matches!(err, BindingError::GlobUnsupported));
    }
}
