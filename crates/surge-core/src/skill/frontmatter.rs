//! Agent Skills format: `SKILL.md` with a YAML frontmatter block.
//!
//! Layout (used unmodified by Claude Code, Cursor, and Codex):
//!
//! ```text
//! ---
//! name: code-reviewer
//! description: Reviews a diff for correctness and style issues.
//! ---
//!
//! # Code Reviewer
//! ...instructions body...
//! ```
//!
//! Only `name` (required) and `version` (optional) are read here — any other
//! frontmatter key (`description`, `license`, `allowed-tools`, ...) is accepted
//! and ignored, so a third-party pack authored for another agent runtime reads
//! unmodified.
//!
//! # Parsing strategy
//! Frontmatter is parsed by a small hand-written reader over the subset that
//! actually occurs in `SKILL.md` files — flat `key: value` pairs (quoted or
//! bare scalars) and inline `key: [a, b]` lists — rather than a general YAML
//! library. Anything outside that subset is a typed parse error naming the
//! offending line, never a silent guess — **except** for a block list
//! (`allowed-tools:` followed by indented `- item` lines) or a block scalar
//! (`description: >`/`description: |` followed by an indented body) under a
//! key other than `name`/`version`: measured against the real Agent
//! Skills/Agent Plugins corpus, both are common in official packs, and
//! rejecting them there rejects a pack for content this module never reads
//! anyway (Решение §3, D04). The identity keys (`name`, `version`) stay
//! strict — nested maps, anchors/aliases, a block list/scalar, or a
//! non-inline list under either of them is still a typed parse error, since
//! that is the one thing a caller actually needs to trust.

use std::collections::HashMap;

#[derive(Debug)]
pub(super) struct SkillFrontmatter {
    pub(super) name: String,
    pub(super) version: Option<String>,
}

/// A parsed frontmatter field value: a scalar, an inline list, or a
/// non-identity key's block construct we deliberately did not parse.
/// Only `name`/`version` values are ever consumed as content, and both must
/// be a [`RawValue::Scalar`] — a recognized inline list or ignored block
/// construct carries no payload, each exists purely to accept the syntax
/// (and to reject `name`/`version` being shaped like one).
#[derive(Debug)]
enum RawValue {
    Scalar(String),
    List,
    /// A block list or block scalar under a non-identity key — accepted
    /// syntactically, its content deliberately never inspected.
    Ignored,
}

/// Split a `SKILL.md` file's raw content into its parsed frontmatter and the
/// instructions body that follows it.
///
/// The caller (which already knows the file's path) is responsible for
/// wrapping the returned reason into a [`super::error::SkillError`].
///
/// # Errors
/// Returns a human-readable reason when the opening or closing `---`
/// delimiter is missing, a line falls outside the supported flat-scalar /
/// inline-list subset, or the required `name` key is missing, empty, or not
/// a scalar.
pub(super) fn split_frontmatter(raw: &str) -> Result<(SkillFrontmatter, &str), String> {
    let after_open = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))
        .ok_or_else(|| {
            "expected the file to start with a `---` frontmatter delimiter".to_string()
        })?;

    let mut yaml_end = None;
    let mut cursor = 0usize;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            yaml_end = Some((cursor, line.len()));
            break;
        }
        cursor += line.len();
    }
    let (yaml_end, delimiter_len) =
        yaml_end.ok_or_else(|| "missing closing `---` frontmatter delimiter".to_string())?;

    let yaml_block = &after_open[..yaml_end];
    let body = &after_open[yaml_end + delimiter_len..];

    let frontmatter = parse_frontmatter(yaml_block)?;
    Ok((frontmatter, body))
}

/// Parse the frontmatter block (the text between the `---` delimiters) as
/// flat `key: value` pairs, indexed so a block list/scalar under a
/// non-identity key can consume its own indented continuation lines without
/// trying to understand them (see the module doc).
fn parse_frontmatter(yaml_block: &str) -> Result<SkillFrontmatter, String> {
    let mut fields: HashMap<String, RawValue> = HashMap::new();
    let lines: Vec<&str> = yaml_block.split_inclusive('\n').collect();
    let mut idx = 0usize;

    while idx < lines.len() {
        let line_no = idx + 1;
        let content = lines[idx].trim_end_matches(['\n', '\r']);

        if content.trim().is_empty() || content.trim_start().starts_with('#') {
            idx += 1;
            continue;
        }
        if content.trim_start().len() != content.len() {
            return Err(format!(
                "line {line_no}: indented content is not supported (no nested structures or block-list continuations)"
            ));
        }

        let colon_idx = content
            .find(':')
            .ok_or_else(|| format!("line {line_no}: expected `key: value`, got {content:?}"))?;
        let key = content[..colon_idx].trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(format!("line {line_no}: invalid frontmatter key {key:?}"));
        }
        if fields.contains_key(key) {
            return Err(format!("line {line_no}: duplicate frontmatter key {key:?}"));
        }
        let is_identity_key = key == "name" || key == "version";

        let value_part = &content[colon_idx + 1..];
        let value_trimmed = value_part.trim();

        if value_trimmed.is_empty() {
            if let Some((block_end, shape)) = following_indented_block(&lines, idx + 1) {
                let tolerate = !is_identity_key
                    && matches!(
                        shape,
                        ContinuationShape::Sequence | ContinuationShape::Opaque
                    );
                if tolerate {
                    // A non-identity key's block list (`allowed-tools:` +
                    // `  - Read`) or multi-line scalar continuation
                    // (`description:` + an indented quoted string spanning
                    // several lines, as official marketplace packs write
                    // it) — accept the syntax, skip its content.
                    fields.insert(key.to_string(), RawValue::Ignored);
                    idx = block_end;
                    continue;
                }
                return Err(match shape {
                    ContinuationShape::Sequence => format!(
                        "line {line_no}: frontmatter `{key}` must be a scalar string, not a block list"
                    ),
                    ContinuationShape::NestedMap | ContinuationShape::Opaque => format!(
                        "line {line_no}: indented content is not supported (no nested structures or block-list continuations)"
                    ),
                });
            }
            fields.insert(key.to_string(), RawValue::Scalar(String::new()));
            idx += 1;
            continue;
        }

        if value_trimmed.starts_with('[') {
            validate_inline_list(value_trimmed, line_no)?;
            fields.insert(key.to_string(), RawValue::List);
            idx += 1;
            continue;
        }

        if matches!(value_trimmed, "|" | "|-" | "|+" | ">" | ">-" | ">+") {
            if is_identity_key {
                return Err(format!(
                    "line {line_no}: multi-line block scalars are not supported"
                ));
            }
            // A non-identity key's block scalar (e.g. `description: >`) —
            // accept the syntax, skip its indented body (if any: an empty
            // block scalar body is also valid YAML).
            fields.insert(key.to_string(), RawValue::Ignored);
            idx = following_indented_block(&lines, idx + 1).map_or(idx + 1, |(end, _)| end);
            continue;
        }

        let scalar = parse_scalar(value_part, line_no)?;
        fields.insert(key.to_string(), RawValue::Scalar(scalar));
        idx += 1;
    }

    let name = match fields.remove("name") {
        Some(RawValue::Scalar(s)) if !s.trim().is_empty() => s,
        Some(RawValue::List | RawValue::Ignored) => {
            return Err("frontmatter `name` must be a scalar string, not a list".to_string());
        },
        Some(RawValue::Scalar(_)) | None => {
            return Err("frontmatter `name` must be a non-empty string".to_string());
        },
    };
    let version = match fields.remove("version") {
        Some(RawValue::Scalar(s)) => Some(s),
        Some(RawValue::List | RawValue::Ignored) => {
            return Err("frontmatter `version` must be a scalar string, not a list".to_string());
        },
        None => None,
    };

    Ok(SkillFrontmatter { name, version })
}

/// How the first non-blank line of an indented continuation block is
/// shaped. Only [`ContinuationShape::NestedMap`] is unconditionally
/// unsupported (regardless of which key it continues) — the other two are
/// tolerated under a non-identity key (see the module doc).
#[derive(Debug, PartialEq, Eq)]
enum ContinuationShape {
    /// A block-sequence item: `- ...`.
    Sequence,
    /// Looks like its own `key: value` pair — an actually-nested map, which
    /// this parser does not support at any key.
    NestedMap,
    /// Anything else — most commonly the continuation of a multi-line
    /// quoted or plain scalar (e.g. a `description:` whose value starts on
    /// the next line and wraps across several, as official marketplace
    /// packs write it). Content this parser deliberately never inspects.
    Opaque,
}

/// Classify a trimmed, already-known-to-be-indented line as one of the
/// [`ContinuationShape`]s.
fn classify_continuation(trimmed: &str) -> ContinuationShape {
    if trimmed.starts_with("- ") || trimmed == "-" {
        return ContinuationShape::Sequence;
    }
    if let Some(colon_idx) = trimmed.find(':') {
        let candidate_key = trimmed[..colon_idx].trim();
        if !candidate_key.is_empty()
            && candidate_key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return ContinuationShape::NestedMap;
        }
    }
    ContinuationShape::Opaque
}

/// Look ahead from `start` for a run of indented (and/or blank) lines with
/// no intervening column-0 content — the continuation of an empty `key:`
/// value or a block-scalar header on the line before `start`.
///
/// Returns `None` when `start` itself is not indented at all (a truly empty
/// scalar, e.g. `version:` with nothing following). Otherwise returns the
/// index just past the block, and the [`ContinuationShape`] of its first
/// non-blank line.
fn following_indented_block(lines: &[&str], start: usize) -> Option<(usize, ContinuationShape)> {
    let mut first = start;
    while first < lines.len() {
        let content = lines[first].trim_end_matches(['\n', '\r']);
        if content.trim().is_empty() {
            first += 1;
            continue;
        }
        break;
    }
    if first >= lines.len() {
        return None;
    }
    let first_content = lines[first].trim_end_matches(['\n', '\r']);
    let first_trimmed = first_content.trim_start();
    if first_trimmed.len() == first_content.len() {
        return None; // not indented: no continuation block at all
    }
    let shape = classify_continuation(first_trimmed);

    let mut end = first;
    while end < lines.len() {
        let content = lines[end].trim_end_matches(['\n', '\r']);
        if content.trim().is_empty() {
            end += 1;
            continue;
        }
        if content.trim_start().len() == content.len() {
            break; // back to column 0
        }
        end += 1;
    }
    Some((end, shape))
}

/// Parse a scalar value: a quoted string (rejecting an unclosed quote or
/// unexpected trailing content after the closing quote), or a bare
/// (unquoted) string with any trailing `# comment` stripped. Rejects
/// anchors/aliases and block-scalar headers in the unquoted case.
fn parse_scalar(raw: &str, line_no: usize) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }

    if let Some(quote) = trimmed.chars().next().filter(|c| *c == '"' || *c == '\'') {
        return parse_quoted_scalar(trimmed, quote, line_no);
    }

    let unquoted = strip_trailing_comment(trimmed);
    if unquoted.is_empty() {
        return Ok(String::new());
    }
    if unquoted.starts_with('&') || unquoted.starts_with('*') {
        return Err(format!(
            "line {line_no}: YAML anchors/aliases are not supported"
        ));
    }
    if matches!(unquoted, "|" | "|-" | "|+" | ">" | ">-" | ">+") {
        return Err(format!(
            "line {line_no}: multi-line block scalars are not supported"
        ));
    }
    Ok(unquoted.to_string())
}

/// Parse a `"..."`/`'...'` quoted scalar. `trimmed` is known to start with
/// `quote`. Rejects a missing closing quote and any non-comment content
/// trailing the closing quote (e.g. `"a"b`).
fn parse_quoted_scalar(trimmed: &str, quote: char, line_no: usize) -> Result<String, String> {
    let rest = &trimmed[quote.len_utf8()..];
    let Some(close_at) = rest.find(quote) else {
        return Err(format!(
            "line {line_no}: unclosed {quote} quote in scalar value"
        ));
    };
    let inner = &rest[..close_at];
    let after = rest[close_at + quote.len_utf8()..].trim();
    if !after.is_empty() && !after.starts_with('#') {
        return Err(format!(
            "line {line_no}: unexpected content after closing quote: {after:?}"
        ));
    }
    Ok(inner.to_string())
}

/// Strip a trailing `# comment`: a `#` is a comment start only at the
/// beginning of the value or when preceded by whitespace (so `value#tag`
/// stays literal, matching plain-scalar YAML comment rules).
fn strip_trailing_comment(value: &str) -> &str {
    let mut prev: Option<char> = None;
    for (idx, ch) in value.char_indices() {
        if ch == '#' && prev.is_none_or(char::is_whitespace) {
            return value[..idx].trim_end();
        }
        prev = Some(ch);
    }
    value
}

/// Validate an inline `[a, "b", c]` list's syntax (each item must itself be a
/// valid scalar). Block-style (`- item` on following lines) lists are not
/// supported — see the module docs.
fn validate_inline_list(raw: &str, line_no: usize) -> Result<(), String> {
    let inner = raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| format!("line {line_no}: expected an inline `[...]` list"))?;
    if inner.trim().is_empty() {
        return Ok(());
    }
    for item in inner.split(',') {
        parse_scalar(item, line_no)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(yaml: &str) -> String {
        format!("---\n{yaml}\n---\n\nBody.\n")
    }

    fn parse_ok(yaml: &str) -> SkillFrontmatter {
        let raw = wrap(yaml);
        split_frontmatter(&raw)
            .unwrap_or_else(|reason| panic!("expected {yaml:?} to parse, got error: {reason}"))
            .0
    }

    fn parse_err(yaml: &str) -> String {
        let raw = wrap(yaml);
        split_frontmatter(&raw)
            .err()
            .unwrap_or_else(|| panic!("expected {yaml:?} to be rejected, but it parsed"))
    }

    #[test]
    fn flat_scalars_parse_name_and_version() {
        let fm = parse_ok("name: code-reviewer\ndescription: reviews diffs\nversion: \"1.0.0\"");
        assert_eq!(fm.name, "code-reviewer");
        assert_eq!(fm.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn missing_opening_delimiter_is_rejected() {
        let reason = split_frontmatter("name: code-reviewer\n---\n\nBody.\n")
            .expect_err("missing opening `---` must be rejected");
        assert!(
            reason.contains("start"),
            "reason should explain the missing opening delimiter, got {reason:?}"
        );
    }

    #[test]
    fn missing_closing_delimiter_is_rejected() {
        let reason = split_frontmatter("---\nname: code-reviewer\n\nBody.\n")
            .expect_err("missing closing `---` must be rejected");
        assert!(
            reason.contains("closing"),
            "reason should explain the missing closing delimiter, got {reason:?}"
        );
    }

    #[test]
    fn indented_content_is_rejected() {
        let reason = parse_err("name: code-reviewer\nmetadata:\n  author: acme");
        assert!(
            reason.contains("indented"),
            "reason should name the unsupported nested/indented construct, got {reason:?}"
        );
    }

    #[test]
    fn anchor_value_is_rejected() {
        let reason = parse_err("name: code-reviewer\ntags: &shared-anchor");
        assert!(
            reason.contains("anchor"),
            "reason should name the unsupported anchor, got {reason:?}"
        );
    }

    #[test]
    fn alias_value_is_rejected() {
        let reason = parse_err("name: code-reviewer\ntags: *shared-anchor");
        assert!(
            reason.contains("alias") || reason.contains("anchor"),
            "reason should name the unsupported alias, got {reason:?}"
        );
    }

    #[test]
    fn block_scalar_header_on_identity_key_is_rejected() {
        // Revised after D04: only the identity keys (`name`/`version`) stay
        // strict about a block scalar — see
        // `block_scalar_on_foreign_key_is_accepted_and_ignored` below for the
        // non-identity case this test used to (wrongly) cover too.
        for header in ["|", "|-", "|+", ">", ">-", ">+"] {
            let reason = parse_err(&format!("name: code-reviewer\nversion: {header}"));
            assert!(
                reason.contains("block scalar"),
                "header {header:?} on the identity key `version` should still be rejected, got {reason:?}"
            );
        }
    }

    #[test]
    fn block_scalar_on_foreign_key_is_accepted_and_ignored() {
        // §3, D04: measured against the real Agent Skills/Agent Plugins
        // corpus, `description: >` with a folded multi-line body is common
        // in official marketplace packs — rejecting it rejects a
        // well-formed pack over content this parser never reads anyway.
        let fm = parse_ok(
            "name: code-reviewer\ndescription: >\n  Reviews a diff for\n  correctness first.\nversion: \"1.0.0\"",
        );
        assert_eq!(fm.name, "code-reviewer");
        assert_eq!(fm.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn block_list_on_foreign_key_is_accepted_and_ignored() {
        // §3, D04: `allowed-tools:` written as a block sequence (`  - Read`)
        // is likewise common in the real corpus and must not reject an
        // otherwise well-formed pack.
        let fm =
            parse_ok("name: code-reviewer\nallowed-tools:\n  - Read\n  - Grep\nversion: \"1.0.0\"");
        assert_eq!(fm.name, "code-reviewer");
        assert_eq!(fm.version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn multiline_quoted_scalar_on_foreign_key_is_accepted_and_ignored() {
        // Measured on the real corpus: the official `math-olympiad` pack's
        // `description:` value starts on the following line as a quoted
        // scalar that wraps across several lines — no `- ` prefix (so it
        // isn't a block list) and no `key:` shape on its continuation lines
        // (so it isn't a nested map either). §3, D04.
        let fm = parse_ok(
            "name: math-olympiad\ndescription:\n  \"Solve competition math\n  problems with adversarial verification.\"\nversion: 0.1.0",
        );
        assert_eq!(fm.name, "math-olympiad");
        assert_eq!(fm.version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn multiline_quoted_scalar_on_identity_key_is_rejected() {
        // The symmetric identity-key case for the test above: the third
        // tolerance construct (opaque multi-line continuation) must stay as
        // strict on `name`/`version` as the other two do — leniency must not
        // leak past the one thing a caller actually needs to trust.
        let reason = parse_err("name:\n  \"a multi-line\n  quoted value\"\nversion: 1.0.0");
        assert!(
            reason.contains("indented"),
            "a multi-line continuation under the identity key `name` should be rejected, \
             got {reason:?}"
        );
    }

    #[test]
    fn block_list_on_identity_key_is_rejected() {
        let reason = parse_err("name:\n  - code-reviewer\n  - another-name");
        assert!(
            reason.contains("name"),
            "a block list under the identity key `name` should be rejected, got {reason:?}"
        );
    }

    #[test]
    fn unclosed_inline_list_is_rejected() {
        let reason = parse_err("name: code-reviewer\ntags: [a, b");
        assert!(
            reason.contains("inline"),
            "reason should explain the unterminated inline list, got {reason:?}"
        );
    }

    #[test]
    fn inline_list_with_quoted_items_is_accepted_and_ignored() {
        let fm = parse_ok(
            r#"name: code-reviewer
tags: ["a", "b", c]"#,
        );
        assert_eq!(fm.name, "code-reviewer");
    }

    #[test]
    fn invalid_key_is_rejected() {
        let reason = parse_err("name: code-reviewer\nbad key: value");
        assert!(
            reason.contains("key"),
            "reason should name the invalid key, got {reason:?}"
        );
    }

    #[test]
    fn missing_name_key_is_rejected() {
        let reason = parse_err("description: reviews diffs");
        assert!(
            reason.contains("name"),
            "reason should name the missing `name` key, got {reason:?}"
        );
    }

    #[test]
    fn empty_name_value_is_rejected() {
        let reason = parse_err("name:\ndescription: reviews diffs");
        assert!(
            reason.contains("name"),
            "reason should name the empty `name` value, got {reason:?}"
        );
    }

    #[test]
    fn name_as_list_is_rejected() {
        let reason = parse_err("name: [code-reviewer]");
        assert!(
            reason.contains("name") && reason.contains("list"),
            "reason should say `name` cannot be a list, got {reason:?}"
        );
    }

    #[test]
    fn unclosed_quote_is_rejected() {
        let reason = parse_err(r#"name: "code-reviewer"#);
        assert!(
            reason.contains("unclosed") || reason.contains("quote"),
            "reason should explain the unclosed quote, got {reason:?}"
        );
    }

    #[test]
    fn trailing_content_after_closing_quote_is_rejected() {
        let reason = parse_err(r#"name: "code-reviewer"junk"#);
        assert!(
            reason.contains("after closing quote"),
            "reason should name the unexpected trailing content, got {reason:?}"
        );
    }

    #[test]
    fn trailing_comment_is_stripped_from_bare_scalar() {
        let fm = parse_ok("name: code-reviewer # a trailing note");
        assert_eq!(
            fm.name, "code-reviewer",
            "the `# ...` comment must not become part of the value"
        );
    }

    #[test]
    fn trailing_comment_after_quoted_scalar_is_stripped() {
        let fm = parse_ok(r#"name: "code-reviewer" # a trailing note"#);
        assert_eq!(fm.name, "code-reviewer");
    }

    #[test]
    fn hash_without_preceding_space_stays_literal() {
        // `#` immediately after a non-space character is not a comment start
        // in plain YAML scalars — it is part of the value.
        let fm = parse_ok("name: code#reviewer");
        assert_eq!(fm.name, "code#reviewer");
    }

    #[test]
    fn duplicate_key_is_rejected() {
        let reason = parse_err("name: code-reviewer\nname: something-else");
        assert!(
            reason.contains("duplicate"),
            "reason should name the duplicate key, got {reason:?}"
        );
    }
}
