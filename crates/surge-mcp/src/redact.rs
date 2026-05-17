//! Secret redaction for captured MCP child-process stderr.
//!
//! MCP server processes commonly echo their startup configuration —
//! including bearer tokens, API keys, and passwords — to stderr. Surge
//! captures that stderr into `tracing` and a per-connection file, so it
//! must mask credential-shaped content before it is persisted or logged.
//!
//! Redaction is **always on** (no opt-out config knob in v0.1 — a
//! deliberate decide-or-defer: the safe default ships, the per-server
//! toggle is deferred). Operators who need raw MCP stderr for debugging
//! run the server directly.
//!
//! The implementation is dependency-free (no `regex`): it tokenises on
//! whitespace and masks values whose key is sensitive, values that
//! follow a `bearer`-style keyword, and standalone high-entropy blobs.
//! It errs toward over-masking — losing a non-secret token to `***` is
//! cheaper than leaking a credential.

/// Replacement emitted in place of a redacted value.
const MASK: &str = "<redacted>";

/// Minimum length for a bare token to be treated as a high-entropy
/// secret blob (base64 / hex / url-safe alphabets).
const MIN_BLOB_LEN: usize = 32;

/// Lowercased key fragments that mark the *following* value (after
/// `=`, `:`, or whitespace) as sensitive.
const SENSITIVE_KEYS: &[&str] = &[
    "authorization",
    "api_key",
    "api-key",
    "apikey",
    "access_token",
    "access-token",
    "refresh_token",
    "client_secret",
    "client-secret",
    "secret",
    "password",
    "passwd",
    "token",
    // Connection strings routinely embed `user:pass@host` — mask the
    // whole value when it arrives under one of these keys.
    "database_url",
    "database-url",
    "dsn",
    "connection_string",
];

/// Lowercased standalone keywords that mark the *next whitespace token*
/// as sensitive (e.g. the `abc` in `Authorization: Bearer abc`).
const SENSITIVE_PREFIXES: &[&str] = &["bearer", "token", "basic"];

/// Redact secret-shaped content from a single line of child stderr.
///
/// Whitespace structure is preserved; only values are masked, so the
/// redacted line stays useful for debugging non-secret context.
#[must_use]
pub fn redact_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut redact_next_token = false;

    for (idx, token) in line.split_inclusive(char::is_whitespace).enumerate() {
        // Split the token into its non-whitespace core and the trailing
        // whitespace so we can re-emit spacing verbatim.
        let trailing_ws_at = token.find(char::is_whitespace).unwrap_or(token.len());
        let (core, ws) = token.split_at(trailing_ws_at);

        if core.is_empty() {
            out.push_str(token);
            continue;
        }

        let lowered = core.to_ascii_lowercase();

        if redact_next_token {
            out.push_str(MASK);
            out.push_str(ws);
            redact_next_token = false;
            continue;
        }

        // `key=value` / `key:value` where key is sensitive. The flag
        // marks the masked value as itself a credential-scheme keyword:
        // `Authorization:Bearer <tok>` packs key+scheme+delimiter into
        // one whitespace token, so the real secret is the *next* token
        // and must also be redacted (else it leaks — the compact form
        // of `Authorization: Bearer <tok>`).
        if let Some((masked, redact_next)) = mask_key_value(core, &lowered) {
            out.push_str(&masked);
            out.push_str(ws);
            if redact_next {
                redact_next_token = true;
            }
            continue;
        }

        // Connection-string URLs carry credentials inline
        // (`postgres://user:pass@host`); mask just the userinfo and
        // keep the scheme/host so the line stays useful for debugging.
        if let Some(masked) = mask_url_userinfo(core) {
            out.push_str(&masked);
            out.push_str(ws);
            continue;
        }

        // A standalone keyword whose following token is the secret
        // (`Bearer <tok>`, `Token <tok>`). Strip a trailing `:`.
        let kw = lowered.trim_end_matches(':');
        if SENSITIVE_PREFIXES.contains(&kw) {
            out.push_str(core);
            out.push_str(ws);
            redact_next_token = true;
            continue;
        }

        // A bare high-entropy blob anywhere on the line (don't mask the
        // very first token — it is typically a level/prefix like `INFO`).
        if idx > 0 && looks_like_secret_blob(core) {
            out.push_str(MASK);
            out.push_str(ws);
            continue;
        }

        out.push_str(core);
        out.push_str(ws);
    }

    out
}

/// If `core` is `<sensitive-key><delim><value>`, return the masked form
/// `<sensitive-key><delim><redacted>` and a flag: when the value is
/// itself a credential-scheme keyword (`bearer`/`token`/`basic`) the
/// real secret is the *next* whitespace token, so the caller must
/// redact that too. Returns `None` when the key is not sensitive.
fn mask_key_value(core: &str, lowered: &str) -> Option<(String, bool)> {
    let delim = lowered.find(['=', ':'])?;
    let (key_raw, rest) = core.split_at(delim);
    let key =
        lowered[..delim].trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
    if key.is_empty() || !SENSITIVE_KEYS.contains(&key) {
        return None;
    }
    // `rest` starts with the delimiter char; keep it, mask the value.
    // `delim` indexes an ASCII `=`/`:`, so `delim + 1` is a char boundary.
    let delim_char = &rest[..1];
    let value_lc =
        lowered[delim + 1..].trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
    let redact_next = SENSITIVE_PREFIXES.contains(&value_lc);
    Some((format!("{key_raw}{delim_char}{MASK}"), redact_next))
}

/// Mask the `user:pass@` userinfo of a connection-string-style URL
/// (`scheme://user:pass@host/...`), preserving scheme and host so the
/// line stays diagnostically useful. Returns `None` when `core` has no
/// URL authority with userinfo (ordinary `https://host/path` URLs and
/// scp-style `git@host:path` refs are left untouched).
fn mask_url_userinfo(core: &str) -> Option<String> {
    let scheme_end = core.find("://")?;
    let after = scheme_end + 3;
    // The authority ends at the first '/', '?', or '#'.
    let authority_end = core[after..]
        .find(['/', '?', '#'])
        .map_or(core.len(), |i| after + i);
    let at = after + core[after..authority_end].find('@')?;
    if at == after {
        // Empty userinfo ("scheme://@host") — nothing to mask.
        return None;
    }
    Some(format!("{}://{}{}", &core[..scheme_end], MASK, &core[at..]))
}

/// Heuristic: a long run of base64 / hex / url-safe characters with no
/// natural-language punctuation is almost certainly a credential.
fn looks_like_secret_blob(token: &str) -> bool {
    let trimmed = token.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')'));
    if trimmed.len() < MIN_BLOB_LEN {
        return false;
    }
    trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-' | '.'))
        && trimmed.chars().any(|c| c.is_ascii_digit())
        && trimmed.chars().any(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_authorization_bearer() {
        let r = redact_line("INFO Authorization: Bearer abc123def456ghi789");
        assert!(!r.contains("abc123def456ghi789"), "token leaked: {r}");
        assert!(r.contains("Authorization:"), "structure lost: {r}");
        assert!(r.contains("Bearer"));
    }

    #[test]
    fn masks_api_key_assignment() {
        let r = redact_line("starting server api_key=sk-supersecretvalue1234 ok");
        assert!(!r.contains("sk-supersecretvalue1234"), "key leaked: {r}");
        assert!(r.contains("api_key="), "key name preserved: {r}");
        assert!(r.contains("ok"), "trailing context preserved: {r}");
    }

    #[test]
    fn masks_password_and_token_colon_forms() {
        let r = redact_line("db password:hunter2supersecret token:eyJabc.def.ghi");
        assert!(!r.contains("hunter2supersecret"), "password leaked: {r}");
        assert!(!r.contains("eyJabc.def.ghi"), "token leaked: {r}");
    }

    #[test]
    fn masks_standalone_high_entropy_blob() {
        let r = redact_line("loaded ghp_AbCdEf0123456789AbCdEf0123456789xx");
        assert!(
            !r.contains("ghp_AbCdEf0123456789AbCdEf0123456789xx"),
            "blob leaked: {r}"
        );
        assert!(r.contains("loaded"));
    }

    #[test]
    fn passes_through_ordinary_lines() {
        let line = "INFO listening on stdio, 7 tools registered";
        assert_eq!(redact_line(line), line);
    }

    #[test]
    fn does_not_mask_first_token_levelish() {
        // A long-ish first token that is plain words must survive.
        let line = "ServerStartupCompleteSuccessfully now serving";
        assert_eq!(redact_line(line), line);
    }

    #[test]
    fn preserves_blank_and_whitespace() {
        assert_eq!(redact_line(""), "");
        assert_eq!(redact_line("   "), "   ");
    }

    #[test]
    fn masks_compact_authorization_bearer() {
        // No space between the key and `Bearer`: the scheme keyword is
        // packed into the key token, the secret is the next token.
        let r = redact_line("INFO Authorization:Bearer abc123def456ghi789tok");
        assert!(
            !r.contains("abc123def456ghi789tok"),
            "compact bearer token leaked: {r}"
        );
        assert!(r.contains("Authorization:"), "structure lost: {r}");
    }

    #[test]
    fn masks_connection_string_userinfo() {
        let r = redact_line("INFO connecting postgres://admin:s3cr3tpw@db.internal:5432/app");
        assert!(!r.contains("s3cr3tpw"), "db password leaked: {r}");
        assert!(!r.contains("admin:s3cr3tpw"), "userinfo leaked: {r}");
        assert!(r.contains("postgres://"), "scheme lost: {r}");
        assert!(r.contains("db.internal"), "host lost (debuggability): {r}");
    }

    #[test]
    fn masks_database_url_env_assignment() {
        let r = redact_line("DATABASE_URL=postgres://u:p@h:5432/db starting");
        assert!(
            !r.contains("postgres://u:p@h"),
            "DATABASE_URL value leaked: {r}"
        );
        assert!(r.contains("DATABASE_URL="), "key preserved: {r}");
        assert!(r.contains("starting"), "trailing context preserved: {r}");
    }

    #[test]
    fn ordinary_url_without_userinfo_passes_through() {
        // No credentials in the authority — keep it readable.
        let line = "fetched https://example.com/path?q=1 ok";
        assert_eq!(redact_line(line), line);
    }
}
