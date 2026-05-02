//! Secret scanner — detects and redacts credentials before they enter LLM context.
//!
//! # Threat model
//!
//! Agents are allowed to read files as part of their task. If the worktree
//! contains credentials (`.env`, config files, accidentally-committed CI secrets)
//! those values would flow into the LLM context window, logs, and the event
//! stream.
//!
//! [`redact_secrets`] scans content against known credential patterns and
//! replaces matches with `[REDACTED:<type>]` before the content is returned
//! to the agent. A `tracing::warn!` is emitted for each hit so operators can
//! audit which files triggered redaction.

use regex::{NoExpand, Regex};
use std::sync::LazyLock;
use tracing::warn;

// ── Pattern table ────────────────────────────────────────────────────

struct Pattern {
    name: &'static str,
    re: Regex,
}

static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let raw: &[(&str, &str)] = &[
        // Anthropic keys: sk-ant-api03-<base64>
        ("anthropic-key", r"sk-ant-[A-Za-z0-9_\-]{20,}"),
        // AWS IAM access key IDs
        ("aws-access-key", r"AKIA[0-9A-Z]{16}"),
        // OpenAI API keys
        ("openai-key", r"sk-[A-Za-z0-9]{48}"),
        // GitHub personal access tokens (new ghp_ format)
        ("github-pat", r"ghp_[A-Za-z0-9]{36}"),
        // PEM private key headers (RSA, EC, OPENSSH, etc.)
        ("private-key", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
        // Database DSNs with embedded credentials (user:pass@host)
        ("postgres-dsn", r"postgres://[^:@\s]{1,64}:[^@\s]{1,128}@"),
        ("mysql-dsn", r"mysql://[^:@\s]{1,64}:[^@\s]{1,128}@"),
        (
            "mongodb-dsn",
            r"mongodb(?:\+srv)?://[^:@\s]{1,64}:[^@\s]{1,128}@",
        ),
        // Slack bot/user tokens
        ("slack-token", r"xox[bporas]-[A-Za-z0-9\-]{10,}"),
        // Stripe secret keys
        ("stripe-key", r"sk_(live|test)_[A-Za-z0-9]{20,}"),
        // JWT tokens (three base64url-encoded segments)
        (
            "jwt",
            r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{20,}",
        ),
        // Generic env var patterns: FOO_API_KEY=<hex-or-alnum 32+>
        (
            "generic-api-key",
            r"(?i)(api[_-]?key|api[_-]?secret|secret[_-]?key)\s*=\s*[A-Za-z0-9]{32,}",
        ),
    ];

    raw.iter()
        .map(|(name, pattern)| Pattern {
            name,
            re: Regex::new(pattern)
                .unwrap_or_else(|e| panic!("invalid secret pattern '{name}': {e}")),
        })
        .collect()
});

// ── Public API ───────────────────────────────────────────────────────

/// Scan `content` for credential patterns and replace matches with
/// `[REDACTED:<type>]`.
///
/// Returns `(redacted_content, was_redacted)`. When `was_redacted` is `true`,
/// a `tracing::warn!` has already been emitted for each matched pattern.
#[must_use]
pub fn redact_secrets(content: &str) -> (String, bool) {
    let mut result = content.to_string();
    let mut was_redacted = false;

    for p in PATTERNS.iter() {
        if p.re.is_match(&result) {
            warn!(
                pattern = p.name,
                "secret pattern detected in file content — redacting before sending to agent"
            );
            let replacement = format!("[REDACTED:{}]", p.name);
            result =
                p.re.replace_all(&result, NoExpand(&replacement))
                    .into_owned();
            was_redacted = true;
        }
    }

    (result, was_redacted)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_secrets_unchanged() {
        let (out, redacted) = redact_secrets("fn main() { println!(\"hello\"); }");
        assert!(!redacted);
        assert_eq!(out, "fn main() { println!(\"hello\"); }");
    }

    #[test]
    fn test_anthropic_key_redacted() {
        let input = "ANTHROPIC_API_KEY=sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890abcdef";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:anthropic-key]"));
        assert!(!out.contains("sk-ant-api03"));
    }

    #[test]
    fn test_aws_key_redacted() {
        let input = "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:aws-access-key]"));
        assert!(!out.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn test_github_pat_redacted() {
        let input = "token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:github-pat]"));
    }

    #[test]
    fn test_private_key_header_redacted() {
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA...";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:private-key]"));
        assert!(!out.contains("-----BEGIN RSA PRIVATE KEY-----"));
    }

    #[test]
    fn test_postgres_dsn_redacted() {
        let input = "DATABASE_URL=postgres://admin:s3cr3t@localhost:5432/mydb";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:postgres-dsn]"));
        assert!(!out.contains("s3cr3t"));
    }

    #[test]
    fn test_mongodb_dsn_redacted() {
        let input = "MONGO_URL=mongodb+srv://user:pass123@cluster.mongodb.net/db";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:mongodb-dsn]"));
    }

    #[test]
    fn test_slack_token_redacted() {
        let input = "SLACK_TOKEN=xoxb-not-a-real-token-placeholder-value";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:slack-token]"));
    }

    #[test]
    fn test_stripe_key_redacted() {
        let input = "STRIPE_SECRET_KEY=sk_test_FAKEFAKEFAKEFAKEFAKE00";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:stripe-key]"));
    }

    #[test]
    fn test_jwt_redacted() {
        let input = "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:jwt]"));
    }

    #[test]
    fn test_generic_api_key_env_redacted() {
        let input = "API_KEY=abcdef1234567890abcdef1234567890";
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(out.contains("[REDACTED:generic-api-key]"));
    }

    #[test]
    fn test_multiple_secrets_all_redacted() {
        let input = concat!(
            "ANTHROPIC_API_KEY=sk-ant-api03-ABCDEFGHIJKLMNOPQRSTUVWXYZ1234567890abcdef\n",
            "DB=postgres://admin:hunter2@db.example.com/prod",
        );
        let (out, redacted) = redact_secrets(input);
        assert!(redacted);
        assert!(!out.contains("sk-ant-api03"));
        assert!(!out.contains("hunter2"));
    }
}
