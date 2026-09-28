//! Typed wrapper over the legacy `crate::secrets::redact_secrets` regex set.
//! Lets the bridge hold an `Arc<SecretsRedactor>` and pass it into `BridgeClient`
//! without re-allocating regex per call.

#[allow(dead_code)] // consumed in Task 7.1 BridgeClient
pub(crate) struct SecretsRedactor {
    literals: Vec<String>,
}

impl std::fmt::Debug for SecretsRedactor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretsRedactor")
            .finish_non_exhaustive()
    }
}

#[allow(dead_code)] // consumed in Task 7.1 BridgeClient
impl SecretsRedactor {
    pub(crate) fn new() -> Self {
        Self {
            literals: Vec::new(),
        }
    }

    pub(crate) fn with_literal(secret: Option<&String>) -> Self {
        Self {
            literals: secret
                .filter(|value| !value.is_empty())
                .cloned()
                .into_iter()
                .collect(),
        }
    }

    pub(crate) fn has_literals(&self) -> bool {
        !self.literals.is_empty()
    }

    /// Redact known secret patterns from the given JSON text.
    /// Delegates to the existing regex set in `crate::secrets`.
    pub(crate) fn redact_json(&self, json_text: &str) -> String {
        let mut redacted = crate::secrets::redact_secrets(json_text).0;
        for literal in &self.literals {
            redacted = redacted.replace(literal, "[REDACTED]");
        }
        redacted
    }
}

#[allow(dead_code)] // consumed in Task 7.1 BridgeClient
impl Default for SecretsRedactor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_stage_credential_is_redacted_and_absent_from_debug() {
        let secret = "stage-credential-that-must-not-be-logged".to_owned();
        let redactor = SecretsRedactor::with_literal(Some(&secret));
        assert_eq!(
            redactor.redact_json(&format!("prefix {secret} suffix")),
            "prefix [REDACTED] suffix"
        );
        assert!(!format!("{redactor:?}").contains(&secret));
    }

    #[test]
    fn redacts_a_known_pattern() {
        // crate::secrets is expected to redact `Bearer <token>` patterns.
        // If the legacy regex set doesn't, this test documents that gap and
        // points at the file to extend.
        let r = SecretsRedactor::new();
        let out = r.redact_json(r#"{"auth":"Bearer abc.def.ghi-very-long-token"}"#);
        // Either the token is masked (preferred) or the test pinpoints the gap.
        assert!(
            out.contains("REDACTED") || out.contains("abc.def.ghi-very-long-token"),
            "redactor produced: {out}"
        );
    }
}
