//! Rendering [`RunReport`] to `json`/`md`/`html` (R28, R29).
//!
//! Every renderer here takes `&RunReport` and returns an owned `String` (or,
//! for JSON, a `Result` over the one way `serde_json` can fail on an
//! already-valid Rust value — a non-finite `f64`, e.g. `cost_usd` overflowing
//! to `inf`, which `serde_json` refuses to encode). None of the three do any
//! I/O of their own; `surge-cli` writes the returned string to stdout.
//!
//! The HTML form is one self-contained file: `<style>` is inlined, and
//! **every** value interpolated from report data is passed through
//! [`escape_html`] before it reaches the page — including values that are
//! "safe by construction" today (a validated [`crate::keys::NodeKey`], a hex
//! [`crate::content_hash::ContentHash`], an enum rendered via `{:?}`). This
//! is deliberate uniformity, not caution about one or two risky fields: a
//! reviewer auditing this file should never have to re-derive, for each of
//! the ~15 interpolation sites, whether *this particular one* happens to be
//! safe today — every one of them is escaped, full stop, so that stays true
//! even if a future field's validation loosens or a new variant is added.
//! Nothing here ever emits a `<link>`, `<script src>`, or any other
//! externally-resolved reference (R29) — the file opens and renders
//! identically with no network reachable at all.
//!
//! [`render_markdown`] escapes with the same *discipline* — every
//! interpolation site, no exceptions — but deliberately **not** the same
//! [`escape_html`] function: its own doc says the Markdown form targets a
//! terminal or an archived `.md` file first, and `escape_html`'s `' " &`
//! attribute-context escaping would visibly corrupt ordinary punctuation
//! there (`don't` → `don&#39;t`) for a threat that document never faces.
//! [`render_markdown`] is *also* the form the L3 merge gate embeds into a
//! tracker comment that leaves the machine (spec §10/R31), and a tracker
//! that renders inline HTML (GitHub) treats an unescaped
//! `<script>`/`</details>` in that comment exactly the way a browser treats
//! one in an HTML page — so it still needs *some* escaping, just not this
//! one. [`escape_markdown`] is the narrower function built for that: it
//! neutralizes the one character that can open an HTML tag (`<`) and the
//! one shape (an embedded newline followed by Markdown structure) that can
//! forge a fake section or table row, and touches nothing else. See
//! [`escape_markdown`]'s own doc for the full reasoning — including why a
//! single shared discipline still means two different functions.

use super::{
    ApprovalEntry, CostTotals, EscalationEntry, EvidenceOrigin, NodeStatus, OutcomeStatus,
    RunCompletion, RunHeader, RunReport, VerdictResult,
};

/// The one banner text for a `Completed` run with no evidence-backed verdict
/// (spec §10/R30) — shared by [`render_markdown`] and [`render_html`] so the
/// two forms say the same thing, not two independently-worded warnings.
const UNVERIFIED_SUCCESS_BANNER: &str = "⚠ UNVERIFIED SUCCESS — this run completed without an authorized verifier confirming it. \
     See Verifier verdicts below.";

/// Render `report` as pretty-printed JSON.
///
/// # Errors
/// Returns `serde_json::Error` only if the report contains a non-finite
/// `f64` (`cost.cost_usd`); every other field here is already
/// JSON-representable by construction.
pub fn render_json(report: &RunReport) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(report)
}

/// Render `report` as a Markdown document — readable in a terminal, a PR
/// description, or an archived `.md` file.
#[must_use]
pub fn render_markdown(report: &RunReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Run Report — {}\n\n",
        escape_markdown_display(&report.run_id)
    ));

    if let Some(prompt) = &report.header.initial_prompt {
        out.push_str(&format!("**Task:** {}\n\n", escape_markdown(prompt)));
    }
    match (report.header.first_event_at, report.header.last_event_at) {
        (Some(first), Some(last)) => {
            out.push_str(&format!(
                "**Started:** {}  \n**Last activity:** {}  \n**Duration:** {}\n\n",
                escape_markdown_display(&first),
                escape_markdown_display(&last),
                format_duration(first, last)
            ));
        },
        _ => out.push_str("**Started:** _no events in this log_\n\n"),
    }

    out.push_str(&format!(
        "**Status:** {}\n\n",
        completion_label(&report.completion)
    ));
    // Specifically `Incomplete`, not `!is_terminal()` — a `Parked` run is
    // also non-terminal, but it is a *proven, explained* pause (the status
    // line above already says until when and why), not the "stopped for an
    // unknown reason" case this banner exists to flag.
    if matches!(report.completion, RunCompletion::Incomplete) {
        out.push_str(
            "> **This run has not finished.** No `RunCompleted`/`RunFailed`/`RunAborted` \
             event was found in the log read for this report.\n\n",
        );
    }
    // Spec §10/R30: a `Completed` run with no evidence-backed verdict must
    // not read the same as a verified one. `report.evidence_backed` is only
    // `Some(false)` here — `None` (non-`Completed` completions) renders
    // nothing extra.
    if report.evidence_backed == Some(false) {
        out.push_str(&format!("> {UNVERIFIED_SUCCESS_BANNER}\n\n"));
    }

    if !report.caveats.is_empty() {
        out.push_str("## Caveats\n\n");
        for caveat in &report.caveats {
            out.push_str(&format!("- ⚠ {}\n", escape_markdown(caveat)));
        }
        out.push('\n');
    }

    out.push_str("## Escalations\n\n");
    if report.escalations.is_empty() {
        out.push_str("_No escalations were raised._\n\n");
    } else {
        for escalation in &report.escalations {
            out.push_str(&format!(
                "- {}\n",
                escape_markdown(&escalation_label(escalation))
            ));
        }
        out.push('\n');
    }

    out.push_str("## Cost\n\n");
    out.push_str(&cost_lines(&report.cost));
    out.push('\n');

    out.push_str("## Nodes\n\n");
    if report.nodes.is_empty() {
        out.push_str("_No nodes entered._\n\n");
    } else {
        out.push_str("| Node | Attempts | Status |\n|---|---|---|\n");
        for node in &report.nodes {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                escape_markdown_display(&node.node),
                node.attempts,
                node_status_label(&node.status)
            ));
        }
        out.push('\n');
    }

    out.push_str("## Outcomes\n\n");
    if report.outcomes.is_empty() {
        out.push_str("_No outcomes reported._\n\n");
    } else {
        for entry in &report.outcomes {
            let status = match &entry.status {
                OutcomeStatus::Accepted => String::new(),
                OutcomeStatus::RejectedByHook { hook_id } => {
                    format!(" — ⚠ REJECTED BY HOOK `{}`", escape_markdown(hook_id))
                },
            };
            out.push_str(&format!(
                "- **{}** → `{}`: {}{status}\n",
                escape_markdown_display(&entry.node),
                escape_markdown_display(&entry.outcome),
                escape_markdown(&entry.summary)
            ));
        }
        out.push('\n');
    }

    out.push_str("## Verifier verdicts\n\n");
    if report.verdicts.is_empty() {
        out.push_str("_No verifier verdicts recorded._\n\n");
    } else {
        for verdict in &report.verdicts {
            out.push_str(&format!(
                "- task `{}` ({}): {}\n",
                escape_markdown(&verdict.task_id),
                escape_markdown_display(&verdict.node),
                verdict_label(&verdict.result)
            ));
        }
        out.push('\n');
    }

    out.push_str("## Evidence\n\n");
    if report.evidence.is_empty() {
        out.push_str("_No artifacts produced._\n\n");
    } else {
        for artifact in &report.evidence {
            let origin = match &artifact.origin {
                EvidenceOrigin::Node { node } => format!("node {}", escape_markdown_display(node)),
                EvidenceOrigin::Bootstrap { stage } => {
                    format!("bootstrap {}", escape_markdown(&format!("{stage:?}")))
                },
            };
            let path = artifact.path.as_ref().map_or_else(String::new, |p| {
                format!(" ({})", escape_markdown(&p.display().to_string()))
            });
            out.push_str(&format!(
                "- **{}**{path} — {origin} — `{}`\n",
                escape_markdown(&artifact.name),
                escape_markdown_display(&artifact.hash)
            ));
        }
        out.push('\n');
    }

    out.push_str("## Skills bound (R14)\n\n");
    if report.skills.is_empty() {
        out.push_str("_No skills were bound._\n\n");
    } else {
        out.push_str("| Node | Skill | Provider | Hash | Gate |\n|---|---|---|---|---|\n");
        for skill in &report.skills {
            out.push_str(&format!(
                "| {} | {} | {} | `{}` | {} |\n",
                escape_markdown_display(&skill.node),
                escape_markdown(&skill.name),
                escape_markdown(&format!("{:?}", skill.provider)),
                escape_markdown_display(&skill.hash),
                if skill.gate_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ));
        }
        out.push('\n');
    }

    out.push_str("## Memory receipts\n\n");
    if report.memory_receipts.is_empty() {
        out.push_str("_None recorded — see the Caveats section above._\n\n");
    } else {
        for receipt in &report.memory_receipts {
            out.push_str(&format!(
                "- selected {} / dropped {} (budget {}, used {})\n",
                receipt.selected.len(),
                receipt.dropped.len(),
                receipt.budget,
                receipt.used
            ));
        }
        out.push('\n');
    }

    out.push_str("## Steers\n\n");
    if report.steers.is_empty() {
        out.push_str("_No operator steers were delivered._\n\n");
    } else {
        for steer in &report.steers {
            out.push_str(&format!(
                "- [{}] {} → {}\n",
                escape_markdown(&steer.id),
                escape_markdown_display(&steer.node),
                escape_markdown(&steer.message)
            ));
        }
        out.push('\n');
    }

    out.push_str("## Approvals\n\n");
    if report.approvals.is_empty() {
        out.push_str("_No approval requests were raised._\n\n");
    } else {
        for entry in &report.approvals {
            out.push_str(&format!("- {}\n", escape_markdown(&approval_label(entry))));
        }
        out.push('\n');
    }

    out
}

/// `last - first`, formatted compactly (`"1h 02m 03s"`, dropping leading
/// zero units). Negative durations (a malformed/reordered log) render as
/// `"0s"` rather than a confusing negative string — this is a display
/// nicety, not a correctness claim the rest of the report depends on.
fn format_duration(
    first: chrono::DateTime<chrono::Utc>,
    last: chrono::DateTime<chrono::Utc>,
) -> String {
    let total_seconds = (last - first).num_seconds().max(0);
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn cost_lines(cost: &CostTotals) -> String {
    let mut out = String::new();
    let cost_note = if cost.uncosted_token_events > 0 {
        format!(
            " (+{} event(s) without a recorded price — actual spend is at least this much)",
            cost.uncosted_token_events
        )
    } else {
        String::new()
    };
    out.push_str(&format!(
        "- Prompt tokens: {}\n- Output tokens: {}\n- Cache hits: {}\n- Cost (USD): {:.4}{cost_note}\n",
        cost.prompt_tokens, cost.output_tokens, cost.cache_hits, cost.cost_usd,
    ));
    if cost.budget_warning_raised {
        out.push_str("- ⚠ Budget warning threshold was crossed.\n");
    }
    if cost.budget_exceeded {
        out.push_str("- ⛔ Budget limit was exceeded.\n");
    }
    out
}

/// Escape a report-derived value before [`render_markdown`] interpolates it
/// — deliberately **not** [`escape_html`]. Two transformations, chosen for
/// what a third-party value (a ticket body's `initial_prompt`, an agent's
/// own text) can actually forge in *this* document, not for making the text
/// "HTML-safe" in general:
///
/// - Newlines (`\n`/`\r`) collapse to a single space. Every field this is
///   called on is one report line (a summary, a prompt, a message) — a raw
///   newline here is never legitimate content, only a structural-injection
///   vector: an embedded `\n\n## Verifier verdicts\n\n- **t1** ...: VERIFIED`
///   forges an entire fake section, ahead of the real one, indistinguishable
///   to a reader who never opens the raw source. Collapsing newlines closes
///   this off entirely — no embedded value can ever reach a true line start
///   (where `#`/`|`/`-` would take on structural meaning) except through
///   this function itself.
/// - `<` becomes `&lt;` — the one character that can open an HTML tag a
///   downstream HTML-rendering consumer (a GitHub-rendered comment; spec
///   §10/R31 embeds this output in a `<details>` block) would interpret
///   (`<script>`, `</details>`). `>` alone opens nothing on its own, so it
///   is left verbatim — escaping it too would visibly mangle ordinary
///   punctuation (`a > b`) for zero additional safety once `<` cannot
///   appear at all.
/// - `|` is backslash-escaped: several sections here place a value inside a
///   `| ... |` table row, where a raw pipe would shift or add a column.
///
/// Ordinary punctuation — apostrophes, quotes, `&`, parentheses — survives
/// untouched. This is the fix for a wrong choice, not a stricter one:
/// [`escape_html`]'s `' " &` escaping is attribute/text-context escaping for
/// an HTML *page*; applying it to a document whose primary reader is a
/// terminal or an archived `.md` file (this module's own doc) visibly
/// corrupted ordinary text (`don't` → `don&#39;t`) for safety this
/// document's threat model never needed — HTML-tag injection and
/// structural-markdown injection are the two real risks, and both are
/// closed above without touching a single quote or ampersand.
fn escape_markdown(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\n' | '\r' => out.push(' '),
            '<' => out.push_str("&lt;"),
            '|' => out.push_str("\\|"),
            other => out.push(other),
        }
    }
    out
}

/// Escape the `Display` form of a report-derived value for Markdown — see
/// [`escape_markdown`].
fn escape_markdown_display(value: &impl std::fmt::Display) -> String {
    escape_markdown(&value.to_string())
}

/// Markdown-only (unlike [`escalation_label`]/[`approval_label`] below,
/// which [`render_html`] also calls and escapes itself at the call site):
/// every free-text field this interpolates is escaped right here, since
/// nothing else escapes it before [`render_markdown`] embeds it.
fn completion_label(completion: &RunCompletion) -> String {
    match completion {
        RunCompletion::Completed { terminal_node } => {
            format!("Completed at `{}`", escape_markdown_display(terminal_node))
        },
        RunCompletion::Failed { error } => format!("Failed — {}", escape_markdown(error)),
        RunCompletion::Aborted { reason } => format!("Aborted — {}", escape_markdown(reason)),
        RunCompletion::Parked {
            wake_at,
            runtime,
            basis,
            reason,
        } => {
            let runtime = runtime.as_deref().unwrap_or("unknown runtime");
            format!(
                "PARKED until {} ({}, {}) — {}",
                escape_markdown_display(wake_at),
                escape_markdown(&format!("{basis:?}")),
                escape_markdown(runtime),
                escape_markdown(reason)
            )
        },
        RunCompletion::Incomplete => "NOT FINISHED".to_string(),
    }
}

/// Shared by [`render_markdown`] (raw; escaped with [`escape_markdown`] at
/// the call site below) and [`render_html`] (escaped with [`escape_html`] in
/// `render_escalations_section`) — kept unescaped here so neither caller
/// double-escapes, and so each can apply the escaping appropriate to its own
/// document type.
fn escalation_label(escalation: &EscalationEntry) -> String {
    let stage = escalation
        .stage
        .map_or_else(String::new, |s| format!(" [{s:?}]"));
    format!("{:?}{stage}: {}", escalation.cause, escalation.reason)
}

/// Markdown-only — see [`completion_label`]'s doc comment.
fn node_status_label(status: &NodeStatus) -> String {
    match status {
        NodeStatus::Completed { outcome } => {
            format!("completed ({})", escape_markdown_display(outcome))
        },
        NodeStatus::Failed {
            reason,
            retry_available,
        } => format!(
            "failed ({}){}",
            escape_markdown(reason),
            if *retry_available {
                ", retry available"
            } else {
                ""
            }
        ),
        NodeStatus::InProgress => "in progress".to_string(),
    }
}

/// Markdown-only — see [`completion_label`]'s doc comment.
fn verdict_label(result: &VerdictResult) -> String {
    match result {
        VerdictResult::Verified { evidence } => {
            format!(
                "VERIFIED (evidence `{}`)",
                escape_markdown_display(evidence)
            )
        },
        VerdictResult::Rejected => "REJECTED".to_string(),
        VerdictResult::Unauthorized => {
            "⚠ UNAUTHORIZED — node lacked verification authority".to_string()
        },
        VerdictResult::Superseded => {
            "SUPERSEDED — task moved on after this verdict; must be re-earned".to_string()
        },
    }
}

fn approval_label(entry: &ApprovalEntry) -> String {
    match entry {
        ApprovalEntry::HumanInputRequested { node, prompt } => {
            format!("[{node}] requested: {prompt}")
        },
        ApprovalEntry::HumanInputResolved { node, response } => {
            format!("[{node}] resolved: {response}")
        },
        ApprovalEntry::HumanInputTimedOut {
            node,
            elapsed_seconds,
        } => format!("[{node}] timed out after {elapsed_seconds}s"),
        ApprovalEntry::SandboxElevationRequested { node, capability } => {
            format!("[{node}] elevation requested: {capability}")
        },
        ApprovalEntry::SandboxElevationDecided {
            node,
            decision,
            remember,
        } => format!("[{node}] elevation {decision:?} (remember={remember})"),
        ApprovalEntry::SandboxElevationTimedOut {
            node,
            capability,
            elapsed_seconds,
        } => format!("[{node}] elevation for {capability} timed out after {elapsed_seconds}s"),
        ApprovalEntry::BootstrapApprovalRequested { stage } => {
            format!("bootstrap {stage:?} approval requested")
        },
        ApprovalEntry::BootstrapApprovalDecided { stage, decision } => {
            format!("bootstrap {stage:?} approval: {decision:?}")
        },
        ApprovalEntry::RoadmapPatchApprovalRequested { patch_id } => {
            format!("roadmap patch {patch_id} approval requested")
        },
        ApprovalEntry::RoadmapPatchApprovalDecided { patch_id, decision } => {
            format!("roadmap patch {patch_id} approval: {decision:?}")
        },
    }
}

/// Escape the five characters that matter inside HTML text/attribute
/// contexts. No external crate: this report is one self-contained file by
/// contract (R29), and the escaping surface needed here is small and fixed.
///
/// Every call site in [`render_html`] and its section helpers, and now
/// [`render_markdown`] too, routes every piece of report-derived text
/// through this — see this module's own doc comment for why that is uniform
/// rather than "only where it's needed."
fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// Escape the `Display` form of any report-derived value. A thin wrapper
/// over [`escape_html`] used at every interpolation site (`NodeKey`,
/// `ContentHash`, `OutcomeKey`, a `{:?}`-formatted enum, a timestamp, …) so
/// no call site has to first decide "is this particular type safe to skip."
fn escape_display(value: &impl std::fmt::Display) -> String {
    escape_html(&value.to_string())
}

/// Render `report` as one self-contained HTML file: inline `<style>`, no
/// external stylesheet, script, font, or image reference of any kind (R29)
/// — it opens and reads identically offline, in a PR viewer, or years later
/// from an archive.
#[must_use]
pub fn render_html(report: &RunReport) -> String {
    let mut body = String::new();

    body.push_str(&format!(
        "<h1>Run Report — {}</h1>\n",
        escape_display(&report.run_id)
    ));

    body.push_str(&render_header_section(&report.header));

    let (status_class, status_text) = match &report.completion {
        // Spec §10/R30: a `Completed` run with no evidence-backed verdict
        // renders with the `warn` class and an explicit label, not the same
        // green `ok` a verified completion gets — `report.evidence_backed`
        // is `Some(bool)` exactly when `completion` is `Completed`, so this
        // arm is the only place that reads it.
        RunCompletion::Completed { terminal_node } if report.evidence_backed == Some(false) => (
            "warn",
            format!("Completed at {terminal_node} — UNVERIFIED SUCCESS"),
        ),
        RunCompletion::Completed { terminal_node } => {
            ("ok", format!("Completed at {terminal_node}"))
        },
        RunCompletion::Failed { error } => ("bad", format!("Failed — {error}")),
        RunCompletion::Aborted { reason } => ("bad", format!("Aborted — {reason}")),
        RunCompletion::Parked {
            wake_at,
            runtime,
            basis,
            reason,
        } => {
            let runtime = runtime.as_deref().unwrap_or("unknown runtime");
            (
                "warn",
                format!("PARKED until {wake_at} ({basis:?}, {runtime}) — {reason}"),
            )
        },
        RunCompletion::Incomplete => ("warn", "NOT FINISHED".to_string()),
    };
    body.push_str(&format!(
        "<p class=\"status {status_class}\">{}</p>\n",
        escape_html(&status_text)
    ));
    // See the identical comment in `render_markdown`: `Parked` is
    // non-terminal but already explained by the status line above, unlike
    // a bare `Incomplete`.
    if matches!(report.completion, RunCompletion::Incomplete) {
        body.push_str(
            "<p class=\"warn\">This run has not finished — no <code>RunCompleted</code>/\
             <code>RunFailed</code>/<code>RunAborted</code> event was found in the log read \
             for this report.</p>\n",
        );
    }
    if report.evidence_backed == Some(false) {
        body.push_str(&format!(
            "<p class=\"warn\">{}</p>\n",
            escape_html(UNVERIFIED_SUCCESS_BANNER)
        ));
    }

    body.push_str(&render_caveats_section(report));
    body.push_str(&render_escalations_section(report));
    body.push_str(&render_cost_section(&report.cost));
    body.push_str(&render_nodes_section(report));
    body.push_str(&render_outcomes_section(report));
    body.push_str(&render_verdicts_section(report));
    body.push_str(&render_evidence_section(report));
    body.push_str(&render_skills_section(report));
    body.push_str(&render_memory_receipts_section(report));
    body.push_str(&render_steers_section(report));
    body.push_str(&render_approvals_section(report));

    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>Run Report {}</title>\n<style>{}</style>\n</head>\n<body>\n{body}</body>\n</html>\n",
        escape_display(&report.run_id),
        HTML_STYLE,
    )
}

const HTML_STYLE: &str = "
body { font-family: system-ui, sans-serif; margin: 2rem; color: #1a1a1a; background: #fff; }
h1 { font-size: 1.4rem; }
h2 { font-size: 1.1rem; margin-top: 2rem; border-bottom: 1px solid #ccc; padding-bottom: 0.25rem; }
table { border-collapse: collapse; width: 100%; margin: 0.5rem 0; }
th, td { text-align: left; padding: 0.3rem 0.6rem; border: 1px solid #ddd; font-size: 0.9rem; }
code { background: #f2f2f2; padding: 0 0.25rem; }
.status { font-weight: bold; padding: 0.4rem 0.6rem; display: inline-block; border-radius: 4px; }
.ok { background: #e3f7e8; color: #1a7a34; }
.bad { background: #fbe4e4; color: #a12525; }
.warn { background: #fff4dc; color: #8a6100; }
.empty { color: #777; font-style: italic; }
";

fn render_header_section(header: &RunHeader) -> String {
    let mut lines = String::new();
    if let Some(prompt) = &header.initial_prompt {
        lines.push_str(&format!("<li><b>Task:</b> {}</li>", escape_html(prompt)));
    }
    match (header.first_event_at, header.last_event_at) {
        (Some(first), Some(last)) => {
            lines.push_str(&format!(
                "<li><b>Started:</b> {}</li><li><b>Last activity:</b> {}</li>\
                 <li><b>Duration:</b> {}</li>",
                escape_display(&first),
                escape_display(&last),
                escape_html(&format_duration(first, last)),
            ));
        },
        _ => lines.push_str("<li><b>Started:</b> <em>no events in this log</em></li>"),
    }
    format!("<ul>{lines}</ul>\n")
}

fn render_caveats_section(report: &RunReport) -> String {
    if report.caveats.is_empty() {
        return String::new();
    }
    let mut items = String::new();
    for caveat in &report.caveats {
        items.push_str(&format!(
            "<li class=\"warn\">{}</li>\n",
            escape_html(caveat)
        ));
    }
    format!("<h2>Caveats</h2>\n<ul>{items}</ul>\n")
}

fn render_escalations_section(report: &RunReport) -> String {
    if report.escalations.is_empty() {
        return "<h2>Escalations</h2>\n<p class=\"empty\">No escalations were raised.</p>\n"
            .to_string();
    }
    let mut items = String::new();
    for escalation in &report.escalations {
        items.push_str(&format!(
            "<li class=\"bad\">{}</li>\n",
            escape_html(&escalation_label(escalation))
        ));
    }
    format!("<h2>Escalations</h2>\n<ul>{items}</ul>\n")
}

fn render_cost_section(cost: &CostTotals) -> String {
    let mut warnings = String::new();
    if cost.budget_warning_raised {
        warnings.push_str("<li class=\"warn\">Budget warning threshold was crossed.</li>");
    }
    if cost.budget_exceeded {
        warnings.push_str("<li class=\"bad\">Budget limit was exceeded.</li>");
    }
    let uncosted = if cost.uncosted_token_events > 0 {
        format!(
            "<li class=\"warn\">{} event(s) recorded token spend with no price — the cost \
             figure above is a floor, not the true total.</li>",
            cost.uncosted_token_events
        )
    } else {
        String::new()
    };
    format!(
        "<h2>Cost</h2>\n<ul><li>Prompt tokens: {}</li><li>Output tokens: {}</li>\
         <li>Cache hits: {}</li><li>Cost (USD): {:.4}</li>{uncosted}{warnings}</ul>\n",
        cost.prompt_tokens, cost.output_tokens, cost.cache_hits, cost.cost_usd,
    )
}

fn render_nodes_section(report: &RunReport) -> String {
    if report.nodes.is_empty() {
        return "<h2>Nodes</h2>\n<p class=\"empty\">No nodes entered.</p>\n".to_string();
    }
    let mut rows = String::new();
    for node in &report.nodes {
        let (class, label) = match &node.status {
            NodeStatus::Completed { outcome } => {
                ("ok", format!("completed ({})", escape_display(outcome)))
            },
            NodeStatus::Failed {
                reason,
                retry_available,
            } => (
                "bad",
                format!(
                    "failed ({}){}",
                    escape_html(reason),
                    if *retry_available {
                        ", retry available"
                    } else {
                        ""
                    }
                ),
            ),
            NodeStatus::InProgress => ("warn", "in progress".to_string()),
        };
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td class=\"{class}\">{}</td></tr>\n",
            escape_display(&node.node),
            node.attempts,
            label,
        ));
    }
    format!(
        "<h2>Nodes</h2>\n<table><tr><th>Node</th><th>Attempts</th><th>Status</th></tr>\n{rows}</table>\n"
    )
}

fn render_outcomes_section(report: &RunReport) -> String {
    if report.outcomes.is_empty() {
        return "<h2>Outcomes</h2>\n<p class=\"empty\">No outcomes reported.</p>\n".to_string();
    }
    let mut items = String::new();
    for entry in &report.outcomes {
        let status = match &entry.status {
            OutcomeStatus::Accepted => String::new(),
            OutcomeStatus::RejectedByHook { hook_id } => {
                format!(
                    " — <span class=\"bad\">REJECTED BY HOOK {}</span>",
                    escape_html(hook_id)
                )
            },
        };
        items.push_str(&format!(
            "<li><b>{}</b> → <code>{}</code>: {}{status}</li>\n",
            escape_display(&entry.node),
            escape_display(&entry.outcome),
            escape_html(&entry.summary),
        ));
    }
    format!("<h2>Outcomes</h2>\n<ul>{items}</ul>\n")
}

fn render_verdicts_section(report: &RunReport) -> String {
    if report.verdicts.is_empty() {
        return "<h2>Verifier verdicts</h2>\n<p class=\"empty\">No verifier verdicts recorded.</p>\n"
            .to_string();
    }
    let mut items = String::new();
    for verdict in &report.verdicts {
        let (class, label) = match &verdict.result {
            VerdictResult::Verified { evidence } => (
                "ok",
                format!("VERIFIED (evidence {})", escape_display(evidence)),
            ),
            VerdictResult::Rejected => ("bad", "REJECTED".to_string()),
            VerdictResult::Unauthorized => (
                "bad",
                "UNAUTHORIZED — node lacked verification authority".to_string(),
            ),
            VerdictResult::Superseded => (
                "warn",
                "SUPERSEDED — task moved on after this verdict; must be re-earned".to_string(),
            ),
        };
        items.push_str(&format!(
            "<li>task <code>{}</code> ({}): <span class=\"{class}\">{}</span></li>\n",
            escape_html(&verdict.task_id),
            escape_display(&verdict.node),
            label,
        ));
    }
    format!("<h2>Verifier verdicts</h2>\n<ul>{items}</ul>\n")
}

fn render_evidence_section(report: &RunReport) -> String {
    if report.evidence.is_empty() {
        return "<h2>Evidence</h2>\n<p class=\"empty\">No artifacts produced.</p>\n".to_string();
    }
    let mut items = String::new();
    for artifact in &report.evidence {
        let origin = match &artifact.origin {
            EvidenceOrigin::Node { node } => format!("node {}", escape_display(node)),
            EvidenceOrigin::Bootstrap { stage } => {
                format!("bootstrap {}", escape_html(&format!("{stage:?}")))
            },
        };
        let path = artifact.path.as_ref().map_or_else(String::new, |p| {
            format!(" ({})", escape_html(&p.display().to_string()))
        });
        items.push_str(&format!(
            "<li><b>{}</b>{path} — {origin} — <code>{}</code></li>\n",
            escape_html(&artifact.name),
            escape_display(&artifact.hash),
        ));
    }
    format!("<h2>Evidence</h2>\n<ul>{items}</ul>\n")
}

fn render_skills_section(report: &RunReport) -> String {
    if report.skills.is_empty() {
        return "<h2>Skills bound (R14)</h2>\n<p class=\"empty\">No skills were bound.</p>\n"
            .to_string();
    }
    let mut rows = String::new();
    for skill in &report.skills {
        rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td><td>{}</td></tr>\n",
            escape_display(&skill.node),
            escape_html(&skill.name),
            escape_html(&format!("{:?}", skill.provider)),
            escape_display(&skill.hash),
            if skill.gate_enabled {
                "enabled"
            } else {
                "disabled"
            },
        ));
    }
    format!(
        "<h2>Skills bound (R14)</h2>\n<table><tr><th>Node</th><th>Skill</th><th>Provider</th>\
         <th>Hash</th><th>Gate</th></tr>\n{rows}</table>\n"
    )
}

fn render_memory_receipts_section(report: &RunReport) -> String {
    if report.memory_receipts.is_empty() {
        return "<h2>Memory receipts</h2>\n<p class=\"empty\">None recorded — see Caveats \
                above.</p>\n"
            .to_string();
    }
    let mut items = String::new();
    for receipt in &report.memory_receipts {
        items.push_str(&format!(
            "<li>selected {} / dropped {} (budget {}, used {})</li>\n",
            receipt.selected.len(),
            receipt.dropped.len(),
            receipt.budget,
            receipt.used,
        ));
    }
    format!("<h2>Memory receipts</h2>\n<ul>{items}</ul>\n")
}

fn render_steers_section(report: &RunReport) -> String {
    if report.steers.is_empty() {
        return "<h2>Steers</h2>\n<p class=\"empty\">No operator steers were delivered.</p>\n"
            .to_string();
    }
    let mut items = String::new();
    for steer in &report.steers {
        items.push_str(&format!(
            "<li>[{}] {} → {}</li>\n",
            escape_html(&steer.id),
            escape_display(&steer.node),
            escape_html(&steer.message),
        ));
    }
    format!("<h2>Steers</h2>\n<ul>{items}</ul>\n")
}

fn render_approvals_section(report: &RunReport) -> String {
    if report.approvals.is_empty() {
        return "<h2>Approvals</h2>\n<p class=\"empty\">No approval requests were raised.</p>\n"
            .to_string();
    }
    let mut items = String::new();
    for entry in &report.approvals {
        items.push_str(&format!(
            "<li>{}</li>\n",
            escape_html(&approval_label(entry))
        ));
    }
    format!("<h2>Approvals</h2>\n<ul>{items}</ul>\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_hash::ContentHash;
    use crate::id::RunId;
    use crate::keys::{NodeKey, OutcomeKey};
    use crate::run_event::{BootstrapStage, EventPayload, RunEvent};
    use crate::skill::SkillProvider;

    fn node_key(name: &str) -> NodeKey {
        NodeKey::try_from(name).unwrap()
    }

    fn sample_report() -> RunReport {
        let run_id = RunId::new();
        let events = vec![
            RunEvent {
                run_id,
                seq: 1,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::OutcomeReported {
                    node: node_key("impl_1"),
                    outcome: OutcomeKey::try_from("done").unwrap(),
                    summary: "<script>alert(1)</script> & \"quoted\"".into(),
                },
            },
            RunEvent {
                run_id,
                seq: 2,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::RunCompleted {
                    terminal_node: node_key("impl_1"),
                },
            },
        ];
        RunReport::compile(run_id, &events)
    }

    /// Every event variant that carries a `String` (or a string embedded in
    /// a `serde_json::Value`) this module ever renders, seeded with the
    /// same `XSS_PAYLOAD` — the mutation the review round asked for made
    /// concrete: removing any one `escape_html`/`escape_display` call along
    /// this fixture's path must turn a section-specific assertion red.
    const XSS_PAYLOAD: &str = "<script>xss</script> & \"quo'te\"";

    fn full_coverage_report() -> RunReport {
        let run_id = RunId::new();
        let node = node_key("n");
        let hook_node = node_key("hooked");
        let events = vec![
            RunEvent {
                run_id,
                seq: 1,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: "/p".into(),
                    initial_prompt: XSS_PAYLOAD.into(),
                    config: crate::run_event::RunConfig {
                        sandbox_default: crate::sandbox::SandboxMode::WorkspaceWrite,
                        approval_default: crate::approvals::ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: vec![],
                        budget: Default::default(),
                    },
                },
            },
            RunEvent {
                run_id,
                seq: 2,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::StageFailed {
                    node: node.clone(),
                    reason: XSS_PAYLOAD.into(),
                    retry_available: true,
                },
            },
            RunEvent {
                run_id,
                seq: 3,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::OutcomeReported {
                    node: node.clone(),
                    outcome: OutcomeKey::try_from("done").unwrap(),
                    summary: XSS_PAYLOAD.into(),
                },
            },
            RunEvent {
                run_id,
                seq: 4,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::OutcomeReported {
                    node: hook_node.clone(),
                    outcome: OutcomeKey::try_from("done").unwrap(),
                    summary: "will be rejected".into(),
                },
            },
            RunEvent {
                run_id,
                seq: 5,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::OutcomeRejectedByHook {
                    node: hook_node,
                    outcome: OutcomeKey::try_from("done").unwrap(),
                    hook_id: XSS_PAYLOAD.into(),
                },
            },
            RunEvent {
                run_id,
                seq: 6,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::TaskVerified {
                    task_id: XSS_PAYLOAD.into(),
                    node: node.clone(),
                    evidence: ContentHash::compute(b"e"),
                },
            },
            RunEvent {
                run_id,
                seq: 7,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::ArtifactProduced {
                    node: node.clone(),
                    artifact: ContentHash::compute(b"a"),
                    path: format!("artifacts/{XSS_PAYLOAD}").into(),
                    name: XSS_PAYLOAD.into(),
                },
            },
            RunEvent {
                run_id,
                seq: 8,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::BootstrapArtifactProduced {
                    stage: BootstrapStage::Description,
                    artifact: ContentHash::compute(b"b"),
                    name: XSS_PAYLOAD.into(),
                },
            },
            RunEvent {
                run_id,
                seq: 9,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::SkillBound {
                    node: node.clone(),
                    name: XSS_PAYLOAD.into(),
                    provider: SkillProvider::ProjectDir,
                    hash: ContentHash::compute(b"s"),
                    gate_enabled: true,
                },
            },
            RunEvent {
                run_id,
                seq: 10,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::SteerDelivered {
                    id: XSS_PAYLOAD.into(),
                    node: node.clone(),
                    message: XSS_PAYLOAD.into(),
                },
            },
            RunEvent {
                run_id,
                seq: 11,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::HumanInputRequested {
                    node: node.clone(),
                    session: None,
                    call_id: None,
                    prompt: XSS_PAYLOAD.into(),
                    schema: None,
                },
            },
            RunEvent {
                run_id,
                seq: 12,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::HumanInputResolved {
                    node: node.clone(),
                    call_id: None,
                    response: serde_json::json!({ "text": XSS_PAYLOAD }),
                },
            },
            RunEvent {
                run_id,
                seq: 13,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::SandboxElevationRequested {
                    node: node.clone(),
                    capability: XSS_PAYLOAD.into(),
                },
            },
            RunEvent {
                run_id,
                seq: 14,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::RunFailed {
                    error: XSS_PAYLOAD.into(),
                },
            },
        ];
        RunReport::compile(run_id, &events)
    }

    #[test]
    fn json_round_trips_through_serde() {
        let report = sample_report();
        let json = render_json(&report).unwrap();
        let parsed: RunReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, report);
    }

    #[test]
    fn markdown_contains_every_section_header() {
        let md = render_markdown(&sample_report());
        for header in [
            "## Escalations",
            "## Cost",
            "## Nodes",
            "## Outcomes",
            "## Verifier verdicts",
            "## Evidence",
            "## Skills bound (R14)",
            "## Memory receipts",
            "## Steers",
            "## Approvals",
        ] {
            assert!(md.contains(header), "missing section: {header}");
        }
    }

    #[test]
    fn markdown_flags_an_incomplete_run() {
        let run_id = RunId::new();
        let report = RunReport::compile(run_id, &[]);
        let md = render_markdown(&report);
        assert!(md.contains("This run has not finished"));
    }

    #[test]
    fn markdown_shows_the_initial_prompt_and_activity_window() {
        let md = render_markdown(&full_coverage_report());
        assert!(md.contains("**Task:**"));
        assert!(md.contains("**Started:**"));
        assert!(md.contains("**Duration:**"));
    }

    #[test]
    fn markdown_caveats_section_names_the_memory_receipts_gap() {
        let md = render_markdown(&sample_report());
        assert!(md.contains("## Caveats"));
        assert!(md.contains("memory_receipts is always empty"));
    }

    /// R29: the HTML output must not contain any externally-resolved
    /// reference — no `<link`, no `<script src`, no remote `http(s)://` URL
    /// anywhere in the document.
    #[test]
    fn html_has_no_external_references() {
        let html = render_html(&sample_report());
        assert!(
            !html.contains("<link"),
            "must not link an external stylesheet"
        );
        assert!(
            !html.to_lowercase().contains("<script"),
            "must not load any script"
        );
        assert!(
            !html.contains("http://"),
            "must not reference an external http URL"
        );
        assert!(
            !html.contains("https://"),
            "must not reference an external https URL"
        );
        assert!(html.contains("<style>"), "styles must be inlined");
    }

    #[test]
    fn html_is_a_single_well_formed_document() {
        let html = render_html(&sample_report());
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<html"));
        assert!(html.contains("</html>"));
    }

    #[test]
    fn html_escapes_untrusted_event_text() {
        let html = render_html(&sample_report());
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "raw script tag from event data must be escaped"
        );
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp;"));
        assert!(html.contains("&quot;quoted&quot;"));
    }

    /// The decisive coverage test the review round asked for: one fixture
    /// puts the same XSS payload into every `String`-carrying field this
    /// renderer touches, across header, nodes, outcomes (both accepted and
    /// hook-rejected), verdicts, evidence (node- and bootstrap-origin),
    /// skills, steers, and approvals (human-input request/resolve, sandbox
    /// elevation) — plus `RunFailed`'s error. No raw `<script>` may survive
    /// anywhere in the page, and each section is checked individually so a
    /// single missing `escape_html` call in any one of them fails exactly
    /// that assertion rather than being masked by another section's pass.
    #[test]
    fn every_string_carrying_section_escapes_its_xss_fixture() {
        let html = render_html(&full_coverage_report());

        assert!(
            !html.contains("<script>xss</script>"),
            "raw script tag survived somewhere in the page:\n{html}"
        );

        let sections: Vec<&str> = html.split("<h2>").collect();
        let section = |name: &str| {
            sections
                .iter()
                .find(|s| s.starts_with(name))
                .unwrap_or_else(|| panic!("section {name:?} not found in rendered HTML"))
        };

        // Header (task/prompt) — rendered before the first <h2>.
        let header = sections[0];
        assert!(
            header.contains("&lt;script&gt;xss&lt;/script&gt;"),
            "header (initial_prompt) must escape its XSS fixture"
        );

        assert!(section("Nodes").contains("&lt;script&gt;xss&lt;/script&gt;"));
        assert!(section("Outcomes").contains("&lt;script&gt;xss&lt;/script&gt;"));
        assert!(
            section("Outcomes").contains("REJECTED BY HOOK"),
            "a hook-rejected outcome must render as rejected, not accepted"
        );
        assert!(section("Verifier verdicts").contains("&lt;script&gt;xss&lt;/script&gt;"));
        assert!(section("Evidence").contains("&lt;script&gt;xss&lt;/script&gt;"));
        assert!(section("Skills bound").contains("&lt;script&gt;xss&lt;/script&gt;"));
        assert!(section("Steers").contains("&lt;script&gt;xss&lt;/script&gt;"));
        assert!(section("Approvals").contains("&lt;script&gt;xss&lt;/script&gt;"));
        assert!(
            html.contains("Failed") && html.contains("&lt;script&gt;xss&lt;/script&gt;"),
            "RunFailed's error must be escaped in the status banner"
        );
    }

    /// The Markdown counterpart of the test above: `render_markdown` is used
    /// for more than a terminal — the merge gate (spec §10/R31) embeds its
    /// output straight into a `<details>` block on a tracker comment that
    /// leaves the machine, and a tracker that renders inline HTML (GitHub)
    /// will interpret an unescaped `<script>`/`</details>` the same way a
    /// browser would. Before this test, `render_markdown` escaped nothing;
    /// it now must match `render_html`'s coverage, section for section,
    /// against the identical fixture.
    #[test]
    fn every_string_carrying_section_escapes_its_xss_fixture_in_markdown_too() {
        let md = render_markdown(&full_coverage_report());

        assert!(
            !md.contains("<script>xss</script>"),
            "raw script tag survived somewhere in the Markdown:\n{md}"
        );
        // `escape_markdown` neutralizes `<` only — `>` opens no HTML tag on
        // its own, and the rest of `XSS_PAYLOAD` (` & "quo'te"`) is ordinary
        // punctuation that must survive verbatim (spec §10/R31 review: a
        // wrong choice of escaper corrupted exactly this before).
        const ESCAPED_PAYLOAD: &str = "&lt;script>xss&lt;/script> & \"quo'te\"";

        let sections: Vec<&str> = md.split("## ").collect();
        let section = |name: &str| {
            sections
                .iter()
                .find(|s| s.starts_with(name))
                .unwrap_or_else(|| panic!("section {name:?} not found in rendered Markdown"))
        };

        // Header (task/prompt) — rendered before the first `## ` header.
        let header = sections[0];
        assert!(
            header.contains(ESCAPED_PAYLOAD),
            "header (initial_prompt) must escape only `<`, leaving ordinary \
             punctuation verbatim:\n{header}"
        );

        assert!(section("Nodes").contains(ESCAPED_PAYLOAD));
        assert!(section("Outcomes").contains(ESCAPED_PAYLOAD));
        assert!(
            section("Outcomes").contains("REJECTED BY HOOK"),
            "a hook-rejected outcome must render as rejected, not accepted"
        );
        assert!(section("Verifier verdicts").contains(ESCAPED_PAYLOAD));
        assert!(section("Evidence").contains(ESCAPED_PAYLOAD));
        assert!(section("Skills bound").contains(ESCAPED_PAYLOAD));
        assert!(section("Steers").contains(ESCAPED_PAYLOAD));
        assert!(section("Approvals").contains(ESCAPED_PAYLOAD));
        assert!(
            md.contains("Failed") && md.contains(ESCAPED_PAYLOAD),
            "RunFailed's error must be escaped in the status line"
        );
    }

    /// Spec §10/R31 review, finding 1: `escape_html`'s attribute-context
    /// escaping (`' " &`) was applied to a document whose primary reader is
    /// a terminal, corrupting ordinary punctuation
    /// (`don't` → `don&#39;t`). `escape_markdown` must leave every one of
    /// these characters exactly as written — the only prior XSS-fixture
    /// test asserted the presence of the escaped `<script>` tag and never
    /// checked whether the payload's own `"quo'te"` survived, which is
    /// exactly how that regression went unnoticed.
    #[test]
    fn markdown_leaves_ordinary_punctuation_verbatim() {
        let run_id = RunId::new();
        let text = "don't merge until Bob's \"final\" review & CI pass (a > b)";
        let events = vec![RunEvent {
            run_id,
            seq: 1,
            timestamp: chrono::Utc::now(),
            payload: EventPayload::RunFailed {
                error: text.to_string(),
            },
        }];
        let report = RunReport::compile(run_id, &events);
        let md = render_markdown(&report);
        assert!(
            md.contains(text),
            "ordinary punctuation must survive byte-for-byte, not just avoid \
             a script-tag substring:\n{md}"
        );
    }

    /// A closing `</details>` in a free-text field must not survive
    /// unescaped: the merge gate wraps a Run Report in exactly this tag
    /// (spec §10/R31), and an un-escaped `</details>` anywhere in the body
    /// closes the block early on GitHub, spilling the rest of the comment
    /// (or a forged follow-on block) outside the collapsed section.
    #[test]
    fn markdown_escapes_a_details_close_tag_in_free_text() {
        let run_id = RunId::new();
        let events = vec![RunEvent {
            run_id,
            seq: 1,
            timestamp: chrono::Utc::now(),
            payload: EventPayload::RunFailed {
                error: "</details><img src=x onerror=alert(1)>".into(),
            },
        }];
        let report = RunReport::compile(run_id, &events);
        let md = render_markdown(&report);
        assert!(
            !md.contains("</details>"),
            "an unescaped </details> in report text must not survive into Markdown:\n{md}"
        );
        assert!(!md.contains("<img"));
    }

    /// Spec §10/R31 review, finding 2: HTML-escaping alone does not stop
    /// **markdown-structural** injection. `initial_prompt` comes from a
    /// ticket body — third-party, not this run's own agent — and a value
    /// containing a real blank line followed by `## Verifier verdicts` used
    /// to forge an entire fake section, ahead of the real one, that a
    /// reader would not distinguish from genuine report content.
    /// `escape_markdown` collapses embedded newlines specifically so no
    /// value can ever reach a true line start and take on that structural
    /// meaning.
    #[test]
    fn markdown_does_not_let_initial_prompt_forge_a_fake_section() {
        let run_id = RunId::new();
        let verifier = node_key("verify_1");
        let forged_prompt =
            "legitimate task\n\n## Verifier verdicts\n\n- **t1** (node `verify_1`): VERIFIED";
        let events = vec![
            RunEvent {
                run_id,
                seq: 1,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::RunStarted {
                    pipeline_template: None,
                    project_path: "/p".into(),
                    initial_prompt: forged_prompt.into(),
                    config: crate::run_event::RunConfig {
                        sandbox_default: crate::sandbox::SandboxMode::WorkspaceWrite,
                        approval_default: crate::approvals::ApprovalPolicy::OnRequest,
                        auto_pr: false,
                        mcp_servers: vec![],
                        budget: Default::default(),
                    },
                },
            },
            RunEvent {
                run_id,
                seq: 2,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::RunCompleted {
                    terminal_node: verifier,
                },
            },
        ];
        let report = RunReport::compile(run_id, &events);
        assert_eq!(
            report.evidence_backed,
            Some(false),
            "no real verifier ran — the forged prompt must not change this"
        );
        let md = render_markdown(&report);
        // A *real* section header is always preceded by a newline (the
        // blank line the renderer inserts between sections) — the forged
        // copy inside `initial_prompt` has its embedded newlines collapsed
        // to spaces by `escape_markdown`, so it can never match this
        // pattern. A bare substring count would conflate the two (the
        // forged text still contains the literal characters "## Verifier
        // verdicts", just inline, not at a line start).
        assert_eq!(
            md.matches("\n## Verifier verdicts").count(),
            1,
            "exactly one *real* Verifier verdicts section — the real, empty \
             one — must exist; the forged text must not create a second:\n{md}"
        );
        let real_section = md
            .split("\n## Verifier verdicts")
            .nth(1)
            .expect("the real section must exist");
        let real_section = real_section.split("\n## ").next().unwrap();
        assert!(
            !real_section.contains("VERIFIED"),
            "the real Verifier verdicts section must stay empty — the forged \
             \"VERIFIED\" line must not land inside it:\n{real_section}"
        );
    }

    #[test]
    fn html_renders_a_parked_run_distinctly_from_not_finished() {
        let run_id = RunId::new();
        let events = vec![RunEvent {
            run_id,
            seq: 1,
            timestamp: chrono::Utc::now(),
            payload: EventPayload::RunParked {
                wake_at: chrono::Utc::now(),
                runtime: Some("claude-acp".into()),
                worktree: "/wt".into(),
                basis: crate::capacity::WakeBasis::ObservedReset,
                reason: "rate limited".into(),
            },
        }];
        let report = RunReport::compile(run_id, &events);
        let html = render_html(&report);
        let md = render_markdown(&report);
        assert!(html.contains("PARKED"));
        assert!(!html.contains("NOT FINISHED"));
        // The bug this specifically catches: the explanatory paragraph
        // ("no RunCompleted/RunFailed/RunAborted event was found") is
        // gated on `!is_terminal()`, and `Parked` is also non-terminal —
        // an earlier version of this code showed it for a parked run too,
        // contradicting the whole point of a distinct `Parked` status.
        assert!(
            !html.contains("This run has not finished"),
            "a parked run is a proven, explained pause, not an unexplained stall:\n{html}"
        );
        assert!(
            !md.contains("This run has not finished"),
            "same bug, Markdown form:\n{md}"
        );
        assert!(!report.completion.is_terminal());
    }

    #[test]
    fn escape_html_covers_all_five_special_characters() {
        assert_eq!(
            escape_html("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    /// Spec §10/R30: `sample_report()` completes with zero verifier verdicts
    /// — both render forms must say so, distinctly from a verified
    /// completion (the next test). Checked in both forms with one fixture so
    /// a regression in either renderer's wiring, not just one, fails here.
    #[test]
    fn unverified_success_is_flagged_in_both_render_forms() {
        let report = sample_report();
        assert_eq!(report.evidence_backed, Some(false));

        let html = render_html(&report);
        assert!(
            html.contains("UNVERIFIED SUCCESS"),
            "html must flag an unverified completion:\n{html}"
        );
        assert!(
            !html.contains("<p class=\"status ok\">"),
            "an unverified completion must not render the plain green 'ok' status:\n{html}"
        );

        let md = render_markdown(&report);
        assert!(
            md.contains("UNVERIFIED SUCCESS"),
            "markdown must flag an unverified completion:\n{md}"
        );
    }

    /// The other half of the pair above: a `Completed` run backed by an
    /// authorized verifier verdict must render as a plain, unflagged
    /// success — no warning banner, `ok`-class status. Pinned alongside the
    /// unverified case so a mutation that always shows (or always hides)
    /// the banner fails one test or the other.
    #[test]
    fn verified_success_shows_no_unverified_banner() {
        let run_id = RunId::new();
        let verifier = node_key("verify_1");
        let declared_outcomes = vec![crate::node::OutcomeDecl {
            id: OutcomeKey::try_from("verified").unwrap(),
            description: String::new(),
            edge_kind_hint: crate::edge::EdgeKind::Forward,
            is_terminal: true,
            ledger_effect: crate::node::LedgerEffect::Verified,
        }];
        let mut nodes = std::collections::BTreeMap::new();
        nodes.insert(
            verifier.clone(),
            crate::node::Node {
                id: verifier.clone(),
                position: crate::node::Position::default(),
                declared_outcomes,
                config: crate::node::NodeConfig::Terminal(crate::terminal_config::TerminalConfig {
                    kind: crate::terminal_config::TerminalKind::Success,
                    message: None,
                }),
            },
        );
        let graph = crate::graph::Graph {
            schema_version: crate::graph::SCHEMA_VERSION,
            metadata: crate::graph::GraphMetadata {
                name: "verified-graph".into(),
                description: None,
                template_origin: None,
                created_at: chrono::Utc::now(),
                author: None,
                archetype: None,
            },
            start: verifier.clone(),
            nodes,
            edges: vec![],
            subgraphs: std::collections::BTreeMap::new(),
        };

        let events = vec![
            RunEvent {
                run_id,
                seq: 1,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::PipelineMaterialized {
                    graph: Box::new(graph),
                    graph_hash: ContentHash::compute(b"graph"),
                },
            },
            RunEvent {
                run_id,
                seq: 2,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::TaskVerified {
                    task_id: "t1".into(),
                    node: verifier.clone(),
                    evidence: ContentHash::compute(b"evidence"),
                },
            },
            RunEvent {
                run_id,
                seq: 3,
                timestamp: chrono::Utc::now(),
                payload: EventPayload::RunCompleted {
                    terminal_node: verifier,
                },
            },
        ];
        let report = RunReport::compile(run_id, &events);
        assert_eq!(report.evidence_backed, Some(true));

        let html = render_html(&report);
        assert!(
            !html.contains("UNVERIFIED SUCCESS"),
            "a verified completion must not show the unverified banner:\n{html}"
        );
        assert!(html.contains("<p class=\"status ok\">"));

        let md = render_markdown(&report);
        assert!(!md.contains("UNVERIFIED SUCCESS"));
    }
}
