# R16 — Secrets and privacy release review

## Confirmed defects

- **P1: Telegram bot credential in transport diagnostics.** `crates/surge-notify/src/telegram.rs` formatted `reqwest::Error` for a request whose URL includes the bot token. Request errors attach that URL; propagating the error through logging or a UI leaks the credential. Minimal fix implemented: strip the URL with `without_url()` before formatting. Private helper used by deliverer; added localhost-only regression requiring original diagnostic to contain synthetic marker and sanitized error to exclude URL/marker while retaining transport context. Plan independently approved by R15. `git diff --check -- crates/surge-notify/src/telegram.rs` passed. Rust regression not run yet per coordinator instruction; RED/ GREEN evidence outstanding.

- **P1 integrity / P2 privacy: predictable config temp follows symlinks.** `crates/surge-core/src/config/io.rs:189` constructs `.<filename>.tmp.<PID>` then uses `std::fs::write`, which follows an existing symlink and truncates its target. A writable project directory allows a preexisting path to overwrite an unrelated file with generated config; two saves in one process also collide. Read-only confirmation from implementation; not repaired by R16 because ownership reserved for concurrency/config agent. Minimal safe fix: securely create a unique same-directory file using `create_new`, write through retained descriptor, preserve atomic rename and cleanup semantics. Prefer randomized leaf/create_new helper already available rather than pathname existence checks. Unix creation should be 0600 when config may contain agent env literals. Tests: old predictable symlink leaves victim unchanged; repeated/concurrent save produces valid complete TOML; rename/write failures preserve previous destination and clean owned temp.

## Scanned evidence / limits

- Scanned 1,332 tracked files of at most 2 MB with local patterns for private-key headers, GitHub, AWS, Slack and Telegram credentials. Nine matches across three source files were all after `#[cfg(test)]`; no actual credential finding established. No matched secret values printed. This limited pattern scan is not an exhaustive entropy/history scan.
- Tracked `.env.example` only; no tracked `.env` was listed by targeted inventory.
- Telegram runtime credential type has redacted Debug/Display and tests; config parse diagnostics omit source snippets. Generic persisted secret store intentionally stores plaintext and documents filesystem-permission security model; Telegram legacy key is retired. File-permission enforcement needs storage-owner review.
- Reports preserve initial prompt and journal text; local CLI report has no visible final redaction. Privacy risks from intentionally supplied secrets in prompts must be treated as artifact content, and external tracker publication reviewed separately. No speculative defect is asserted without inspecting that publication path.
- Slack deliverer includes full response JSON in errors; potential reflected content remains a lower-priority logging-hardening review, no concrete credential reflection established.

## Changes

Only `crates/surge-notify/src/telegram.rs` changed by R16. No cargo builds and no commits performed.
