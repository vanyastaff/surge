//! Handlebars-backed prompt template rendering.
//!
//! Replaces the M5 [`substitute_template`](crate::engine::stage::bindings)
//! function with a real templating engine so profile prompts can use
//! conditionals, helpers, and partials when bundled or user-authored
//! profiles need them.
//!
//! Two operating modes:
//!
//! - **Strict (default).** Unknown variables become a render error. This
//!   is the [`Profile registry & bundled roles`] milestone default per
//!   ADR 0001 — typos in template references should fail loudly at
//!   `ProfileRegistry::load` time, not silently substitute empty strings
//!   at agent-launch time.
//! - **Lenient.** Unknown variables render as empty strings. Reserved
//!   for future flow-level overrides (e.g. an "experimental templates"
//!   flag); not exposed via configuration today.
//!
//! HTML escaping is disabled so prompt text stays verbatim — escaping a
//! prompt's quotes or angle brackets would corrupt agent input.

use std::collections::BTreeMap;

use handlebars::Handlebars;
use surge_core::agent_config::TemplateVar;
use surge_core::error::SurgeError;

/// Errors returned by [`PromptRenderer`] operations.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The template did not parse / compile against Handlebars syntax.
    #[error("template compile failed: {0}")]
    Compile(String),

    /// Rendering the template against the supplied bindings failed
    /// (typically: strict-mode reference to an unknown variable).
    #[error("template render failed: {0}")]
    Render(String),
}

impl From<RenderError> for SurgeError {
    fn from(value: RenderError) -> Self {
        SurgeError::Config(format!("prompt template error: {value}"))
    }
}

/// Wrapper around [`handlebars::Handlebars`] tuned for surge prompts.
///
/// Cheap to construct; clone when you need an `'static`-lived copy.
#[derive(Clone)]
pub struct PromptRenderer {
    inner: Handlebars<'static>,
}

impl std::fmt::Debug for PromptRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptRenderer").finish_non_exhaustive()
    }
}

impl Default for PromptRenderer {
    fn default() -> Self {
        Self::strict()
    }
}

impl PromptRenderer {
    /// Strict-mode renderer. Unknown variable references fail rendering.
    /// HTML escaping is disabled so quotes and angle brackets pass through.
    #[must_use]
    pub fn strict() -> Self {
        let mut inner = Handlebars::new();
        inner.set_strict_mode(true);
        inner.register_escape_fn(handlebars::no_escape);
        Self { inner }
    }

    /// Lenient renderer. Unknown variables render as empty strings.
    /// Reserved for future use; not currently exposed in the engine path.
    #[must_use]
    pub fn lenient() -> Self {
        let mut inner = Handlebars::new();
        inner.set_strict_mode(false);
        inner.register_escape_fn(handlebars::no_escape);
        Self { inner }
    }

    /// Render `template` substituting the variables supplied in `bindings`.
    ///
    /// `bindings` is the resolved binding list from
    /// [`crate::engine::stage::bindings::resolve_bindings`] — a
    /// `(TemplateVar, String)` pair list keyed by the template var name
    /// the agent prompt expects.
    ///
    /// # Errors
    /// Returns [`RenderError::Render`] on any Handlebars failure (strict
    /// mode unknown var, malformed helper invocation, etc.).
    pub fn render(
        &self,
        template: &str,
        bindings: &[(TemplateVar, String)],
    ) -> Result<String, RenderError> {
        let data: BTreeMap<&str, &str> = bindings
            .iter()
            .map(|(k, v)| (k.0.as_str(), v.as_str()))
            .collect();
        self.inner
            .render_template(template, &data)
            .map_err(|e| RenderError::Render(e.to_string()))
    }

    /// Compile-check `template` without rendering. Used by
    /// `ProfileRegistry::load` to fail-fast on broken bundled / disk
    /// templates rather than discovering them at agent-launch time.
    ///
    /// # Errors
    /// Returns [`RenderError::Compile`] if Handlebars cannot parse the
    /// template (mismatched braces, unknown helper call, etc.).
    pub fn validate_template(&self, template: &str) -> Result<(), RenderError> {
        // Use a non-strict throwaway for validation so unknown variables
        // do not fail the check. Compile errors (mismatched braces,
        // unknown helpers) still surface as render errors here.
        let mut probe = Handlebars::new();
        probe.set_strict_mode(false);
        probe.register_escape_fn(handlebars::no_escape);
        probe
            .render_template(template, &serde_json::json!({}))
            .map(|_| ())
            .map_err(|e| RenderError::Compile(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_simple_substitution() {
        let r = PromptRenderer::strict();
        let bindings = vec![(TemplateVar("name".into()), "World".into())];
        let out = r.render("Hello, {{name}}!", &bindings).unwrap();
        assert_eq!(out, "Hello, World!");
    }

    #[test]
    fn renders_multiple_bindings() {
        let r = PromptRenderer::strict();
        let bindings = vec![
            (TemplateVar("greeting".into()), "Hi".into()),
            (TemplateVar("name".into()), "Bob".into()),
        ];
        let out = r.render("{{greeting}}, {{name}}!", &bindings).unwrap();
        assert_eq!(out, "Hi, Bob!");
    }

    #[test]
    fn no_html_escape_passes_through_specials() {
        let r = PromptRenderer::strict();
        let bindings = vec![(TemplateVar("payload".into()), "<tag>quote\"</tag>".into())];
        let out = r.render("Body: {{payload}}", &bindings).unwrap();
        assert_eq!(out, "Body: <tag>quote\"</tag>");
    }

    #[test]
    fn strict_mode_rejects_unknown_vars() {
        let r = PromptRenderer::strict();
        let err = r.render("Hello, {{missing}}", &[]).unwrap_err();
        assert!(matches!(err, RenderError::Render(_)));
    }

    #[test]
    fn lenient_mode_renders_unknown_as_empty() {
        let r = PromptRenderer::lenient();
        let out = r.render("Hello, {{missing}}", &[]).unwrap();
        assert_eq!(out, "Hello, ");
    }

    #[test]
    fn malformed_template_fails_compile() {
        let r = PromptRenderer::strict();
        // Unmatched braces -> compile error.
        let err = r.validate_template("Hello {{").unwrap_err();
        assert!(matches!(err, RenderError::Compile(_)));
    }

    #[test]
    fn validate_tolerates_unknown_var_references() {
        // A template that references {{spec}} should validate even when
        // we don't have that binding yet at load time.
        let r = PromptRenderer::strict();
        r.validate_template("Implement {{spec}}").unwrap();
    }

    #[test]
    fn validate_tolerates_empty_template() {
        let r = PromptRenderer::strict();
        r.validate_template("").unwrap();
    }

    #[test]
    fn render_supports_conditionals() {
        let r = PromptRenderer::strict();
        // Handlebars conditional shouldn't ICE on missing var when wrapped.
        let bindings = vec![(TemplateVar("present".into()), "yes".into())];
        let out = r
            .render("{{#if present}}has value{{/if}}", &bindings)
            .unwrap();
        assert_eq!(out, "has value");
    }

    #[test]
    fn from_render_error_to_surge_error() {
        let err = RenderError::Compile("oops".into());
        let surge_err: SurgeError = err.into();
        assert!(matches!(surge_err, SurgeError::Config(msg) if msg.contains("template")));
    }

    fn bundled_prompt(name: &str) -> String {
        surge_core::profile::bundled::BundledRegistry::by_name_latest(name)
            .unwrap_or_else(|| panic!("{name} must be bundled"))
            .prompt
            .system
    }

    fn representative_bootstrap_bindings() -> Vec<(TemplateVar, String)> {
        vec![
            (
                TemplateVar("user_prompt".into()),
                "Build an AFK coding workflow that drafts a roadmap, asks for approval, and then \
                 opens a PR."
                    .into(),
            ),
            (
                TemplateVar("description_artifact".into()),
                "## Goal\nCreate a bootstrap flow for adaptive project execution.\n\n\
                 ## Requirements\n- Produce description.md\n- Produce roadmap.md"
                    .into(),
            ),
            (
                TemplateVar("roadmap_artifact".into()),
                "## Milestones\n1. Bootstrap graph asset\n2. CLI entrypoint\n\n\
                 ## Dependencies\nBootstrap graph before CLI entrypoint."
                    .into(),
            ),
            (
                TemplateVar("edit_feedback".into()),
                "Tighten the scope and make the terminal outcome explicit.".into(),
            ),
        ]
    }

    #[test]
    fn snapshot_description_author_prompt() {
        let renderer = PromptRenderer::strict();
        let out = renderer
            .render(
                &bundled_prompt("description-author"),
                &representative_bootstrap_bindings(),
            )
            .unwrap();
        insta::assert_snapshot!("description_author_bootstrap_prompt", out);
    }

    #[test]
    fn snapshot_roadmap_planner_prompt() {
        let renderer = PromptRenderer::strict();
        let out = renderer
            .render(
                &bundled_prompt("roadmap-planner"),
                &representative_bootstrap_bindings(),
            )
            .unwrap();
        insta::assert_snapshot!("roadmap_planner_bootstrap_prompt", out);
    }

    #[test]
    fn snapshot_flow_generator_prompt() {
        let renderer = PromptRenderer::strict();
        let out = renderer
            .render(
                &bundled_prompt("flow-generator"),
                &representative_bootstrap_bindings(),
            )
            .unwrap();
        insta::assert_snapshot!("flow_generator_bootstrap_prompt", out);
    }
}
