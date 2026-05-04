//! Template rendering for notification titles and bodies.
//!
//! Mustache-style placeholders supported per spec §10.2:
//! `{{run_id}}`, `{{node}}`, `{{outcome}}`, `{{artifact:NAME}}`, `{{stage_summary}}`.
//! Missing placeholders render as empty strings.

use crate::deliverer::{NotifyError, RenderedNotification};
use std::path::PathBuf;
use surge_core::id::RunId;
use surge_core::keys::NodeKey;
use surge_core::notify_config::NotifyTemplate;
use surge_core::run_state::RunMemory;

/// Per-render context — provides values for placeholder substitution.
pub struct RenderContext<'a> {
    /// Run id for `{{run_id}}`.
    pub run_id: RunId,
    /// Notify node key for `{{node}}`.
    pub node: &'a NodeKey,
    /// Run memory for `{{outcome}}`, `{{stage_summary}}`, `{{artifact:NAME}}`.
    pub run_memory: &'a RunMemory,
}

/// Render the template against the context, returning a payload ready
/// for delivery.
pub fn render(
    template: &NotifyTemplate,
    ctx: &RenderContext<'_>,
) -> Result<RenderedNotification, NotifyError> {
    let title = render_string(&template.title, ctx)?;
    let body = render_string(&template.body, ctx)?;
    let artifact_paths = template
        .artifacts
        .iter()
        .filter_map(|src| resolve_artifact_path(src, ctx.run_memory))
        .collect();
    Ok(RenderedNotification {
        severity: template.severity,
        title,
        body,
        artifact_paths,
    })
}

fn render_string(template: &str, ctx: &RenderContext<'_>) -> Result<String, NotifyError> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'{') {
            chars.next(); // consume second '{'
            let mut placeholder = String::new();
            let mut closed = false;
            while let Some(c) = chars.next() {
                if c == '}' && chars.peek() == Some(&'}') {
                    chars.next();
                    closed = true;
                    break;
                }
                placeholder.push(c);
            }
            if !closed {
                return Err(NotifyError::Render(format!(
                    "unclosed placeholder: {{{{ {} }}}}",
                    placeholder.trim()
                )));
            }
            out.push_str(&substitute(placeholder.trim(), ctx));
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn substitute(placeholder: &str, ctx: &RenderContext<'_>) -> String {
    if let Some(name) = placeholder.strip_prefix("artifact:") {
        return ctx
            .run_memory
            .artifacts
            .get(name)
            .map(|a| a.path.to_string_lossy().to_string())
            .unwrap_or_default();
    }
    match placeholder {
        "run_id" => ctx.run_id.to_string(),
        "node" => ctx.node.to_string(),
        "outcome" => ctx
            .run_memory
            .outcomes
            .values()
            .flatten()
            .max_by_key(|r| r.seq)
            .map(|r| r.outcome.to_string())
            .unwrap_or_default(),
        "stage_summary" => ctx
            .run_memory
            .outcomes
            .values()
            .flatten()
            .max_by_key(|r| r.seq)
            .map(|r| r.summary.clone())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn resolve_artifact_path(
    src: &surge_core::agent_config::ArtifactSource,
    memory: &RunMemory,
) -> Option<PathBuf> {
    use surge_core::agent_config::ArtifactSource;
    match src {
        ArtifactSource::NodeOutput { node, artifact } => memory
            .artifacts_by_node
            .get(node)
            .and_then(|list| list.iter().find(|a| a.name == *artifact))
            .map(|a| a.path.clone()),
        ArtifactSource::RunArtifact { name } => memory.artifacts.get(name).map(|a| a.path.clone()),
        ArtifactSource::GlobPattern { .. } | ArtifactSource::Static { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::content_hash::ContentHash;
    use surge_core::keys::OutcomeKey;
    use surge_core::run_state::{ArtifactRef, OutcomeRecord};

    fn empty_memory() -> RunMemory {
        RunMemory::default()
    }

    fn ctx<'a>(run_id: RunId, node: &'a NodeKey, mem: &'a RunMemory) -> RenderContext<'a> {
        RenderContext {
            run_id,
            node,
            run_memory: mem,
        }
    }

    #[test]
    fn substitutes_run_id_and_node() {
        let run = RunId::new();
        let node = NodeKey::try_from("plan_1").unwrap();
        let mem = empty_memory();
        let r = render_string("run={{run_id}} node={{node}}", &ctx(run, &node, &mem)).unwrap();
        assert!(r.contains(&run.to_string()));
        assert!(r.contains("plan_1"));
    }

    #[test]
    fn missing_placeholder_renders_empty() {
        let run = RunId::new();
        let node = NodeKey::try_from("n").unwrap();
        let mem = empty_memory();
        let r = render_string("[{{nonexistent}}]", &ctx(run, &node, &mem)).unwrap();
        assert_eq!(r, "[]");
    }

    #[test]
    fn substitutes_artifact_path() {
        let mut mem = RunMemory::default();
        mem.artifacts.insert(
            "plan.md".into(),
            ArtifactRef {
                hash: ContentHash::compute(b"x"),
                path: "/tmp/plan.md".into(),
                name: "plan.md".into(),
                produced_by: NodeKey::try_from("p").unwrap(),
                produced_at_seq: 1,
            },
        );
        let run = RunId::new();
        let node = NodeKey::try_from("n").unwrap();
        let r = render_string("see {{artifact:plan.md}}", &ctx(run, &node, &mem)).unwrap();
        assert!(r.contains("/tmp/plan.md"));
    }

    #[test]
    fn substitutes_outcome_and_stage_summary() {
        let mut mem = RunMemory::default();
        mem.outcomes
            .entry(NodeKey::try_from("a").unwrap())
            .or_default()
            .push(OutcomeRecord {
                outcome: OutcomeKey::try_from("done").unwrap(),
                summary: "all good".into(),
                seq: 5,
            });
        let run = RunId::new();
        let node = NodeKey::try_from("n").unwrap();
        let r = render_string("o={{outcome}} s={{stage_summary}}", &ctx(run, &node, &mem)).unwrap();
        assert!(r.contains("o=done"));
        assert!(r.contains("s=all good"));
    }

    #[test]
    fn unclosed_placeholder_returns_render_error() {
        let run = RunId::new();
        let node = NodeKey::try_from("n").unwrap();
        let mem = empty_memory();
        let result = render_string("{{run_id", &ctx(run, &node, &mem));
        assert!(matches!(result, Err(NotifyError::Render(_))));
    }
}
