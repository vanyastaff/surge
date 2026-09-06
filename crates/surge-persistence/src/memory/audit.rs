//! `surge memory audit` — read-only correlation of memory claims against
//! source drift and run outcomes.
//!
//! Two independent findings, never a deletion: claims whose file source has
//! drifted since it was hashed ([`stale_claims`]), and claims that trace
//! back to a run that failed ([`failed_run_correlated_claims`]). Both are
//! read-only proposals — [`run_audit`] never writes to the claim store or
//! the run registry. Pruning, if an operator decides to act on a finding,
//! is a separate, explicit, human-issued command (see
//! `.autopilot/competitive-waves/spec.md` §7: an agent that prunes its own
//! memory is a way to lose the one thing that outlives a run).

use crate::runs::escalations::list_loop_guard_stopped_runs;
use crate::runs::registry::{RunFilter, RunSummary};
use crate::runs::storage::Storage;
use crate::{PersistenceError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use surge_core::memory::MemoryClaim;
use surge_core::run_event::EscalationCause;
use surge_core::{ContentHash, MemoryClaimId, RunId, RunStatus};

/// Why [`stale_claims`] flagged a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleReason {
    /// The source still exists, but re-hashing its current content no
    /// longer matches the hash recorded in `Provenance`.
    ContentChanged,
    /// The source no longer exists at all — a disappeared source is a
    /// finding in its own right, not something to silently skip.
    SourceMissing,
}

/// One claim [`stale_claims`] flagged: its recorded provenance hash no
/// longer matches what `source` re-hashes to today.
#[derive(Debug, Clone, Serialize)]
pub struct StaleClaim {
    /// The claim this finding is about.
    pub claim_id: MemoryClaimId,
    /// `Provenance::source` at the time of the audit.
    pub source: String,
    /// Why it was flagged.
    pub reason: StaleReason,
}

/// One claim [`stale_claims`] could not assess for staleness at all — a
/// distinct outcome from both "found stale" and "found fresh". A source
/// audit does not know how to check (a URL, or any other form it cannot
/// positively identify as a file path) and a file path that exists but
/// could not be read (permissions, a path that turned out to be a
/// directory) both land here rather than in `tracing::warn!` alone: a
/// claim this function could not check must never look the same in the
/// report as a claim it checked and found fine.
#[derive(Debug, Clone, Serialize)]
pub struct UnverifiableClaim {
    /// The claim this finding is about.
    pub claim_id: MemoryClaimId,
    /// `Provenance::source` at the time of the audit.
    pub source: String,
    /// Why staleness could not be assessed for it.
    pub reason: String,
}

/// One claim [`failed_run_correlated_claims`] traced to a run.
#[derive(Debug, Clone, Serialize)]
pub struct RunCorrelatedClaim {
    /// The claim this finding is about.
    pub claim_id: MemoryClaimId,
    /// The run its `Provenance::source` names.
    pub run_id: RunId,
    /// That run's registry status at the time of the audit.
    pub run_status: RunStatus,
    /// `Some(cause)` when the run's durable event log carries a loop-guard
    /// escalation (`surge_persistence::runs::{read_escalations,
    /// is_loop_guard_cause}`) — the engine's loop guard stopped this run,
    /// not merely "some stage failed." `None` for a run correlated here
    /// only because its registry status reads `RunStatus::Failed`. Typed,
    /// not inferred from `reason` prose — see
    /// `.autopilot/competitive-waves/interfaces.md`'s note on task 17.
    pub loop_guard_cause: Option<EscalationCause>,
}

/// The full result of `surge memory audit`: read-only findings, nothing
/// deleted or changed. See [`run_audit`].
#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    /// Claims whose source has drifted (or disappeared) since capture.
    pub stale: Vec<StaleClaim>,
    /// Claims staleness could not be assessed for at all — reported, not
    /// silenced. See [`UnverifiableClaim`].
    pub unverifiable: Vec<UnverifiableClaim>,
    /// Claims that trace back to a run that failed, or that the engine's
    /// loop guard stopped (see [`RunCorrelatedClaim::loop_guard_cause`]).
    pub run_correlated: Vec<RunCorrelatedClaim>,
    /// Known gaps in this report. Never a fabricated finding — only an
    /// honest statement of what could not be computed. Populated by
    /// [`run_audit`] for two cases, each one that would otherwise drop a
    /// claim silently out of `run_correlated`: a run named by a claim's
    /// `transcript:` locator that is not present in the run registry at
    /// all, and a run whose per-run event log could not be opened or read
    /// (see [`list_loop_guard_stopped_runs`]). Empty when neither applies.
    pub caveats: Vec<String>,
}

/// What kind of locator a claim's `Provenance::source` is, as far as this
/// module can tell. `Provenance::source`'s own doc allows a file path, a
/// transcript turn, or a URL; this crate additionally mints
/// `legacy:<table>:<id>` for migrated rows, and treats a `file:` URI as an
/// alias for the path it names (see [`file_uri_to_path`]). Only
/// [`SourceKind::FilePath`] has a "current file hash" to compare provenance
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// A path this crate can read from disk and re-hash — written
    /// directly, recovered from a `file:` URI, or (see [`classify_source`])
    /// an otherwise scheme-shaped locator confirmed to name something that
    /// exists on disk right now.
    FilePath,
    /// A transcript-ingest locator (`transcript:run-<ULID>#turn-<N>`).
    /// Never hashed from fetchable content in the first place — not
    /// eligible for staleness, and not a gap in the report either.
    Transcript,
    /// A migrated-legacy locator (`legacy:<table>:<id>`). Same treatment
    /// as `Transcript`.
    Legacy,
    /// A scheme-shaped locator (`scheme://...` or `scheme:...`) that
    /// [`classify_source`] could not confirm names a local file — an
    /// allowed locator per `Provenance::source`'s doc, but fetching one
    /// over the network is out of scope for a local staleness check.
    Url,
}

/// Classify `source` **positively**: a file path is recognized by its own
/// shape (starts with `/`, `./`, or `../`, carries no scheme-shaped prefix
/// at all, or — see below — resolves to something on disk), not by process
/// of elimination against `transcript:`/`legacy:` alone. That distinction
/// matters because `Provenance::source`'s doc explicitly allows a URL as a
/// locator too — treating "not transcript, not legacy" as "must be a file"
/// would send a URL straight into `fs::read`, get `NotFound`, and misreport
/// it as a vanished file source it never was.
///
/// `project_root` anchors every relative path this function or its caller
/// resolves — `Provenance::source`'s file-path form is documented as
/// relative to the project root
/// (`.autopilot/competitive-waves/interfaces.md`, "Контракт локаторов
/// памяти"), not to the audit process's ambient current directory. Using
/// the implicit CWD instead would classify the exact same claim differently
/// depending on which directory `surge memory audit` happened to be run
/// from — worse than an ordinary wrong-verdict bug, because it flips the
/// *kind*, not just the staleness answer.
///
/// Two locator forms that shape alone used to swallow silently are handled
/// explicitly, not by adding another prefix check to the pile (that never
/// terminates — the next one is always one more `if`): `file:` is a scheme
/// whose *only* meaning is "read this local path" (RFC 8089), so
/// [`file_uri_to_path`] expands it unconditionally rather than routing it
/// into [`SourceKind::Url`], where it would be reported as unfetchable when
/// it is in fact sitting right there on disk. Every other scheme-shaped
/// prefix that isn't `transcript:`/`legacy:`/`file:` is genuinely
/// ambiguous — colon is a legal path-component character on every OS this
/// crate reads from (`my:notes/file.md`, `src:v2/...` are real relative
/// paths, not URLs, on any of them) — so instead of guessing from the
/// string, this asks the filesystem: a locator that exists on disk right
/// now is conclusively a file (no URL resolves as a literal local relative
/// path), so it is checked like any other file path instead of being
/// classified away. One that does **not** currently exist stays
/// [`SourceKind::Url`] — existence is the only signal available to tell a
/// vanished real file apart from a genuine network locator that was never
/// local, and guessing either way from the string alone is the exact
/// shape-based mistake this fix removes; that case is still reported,
/// visibly, in `unverifiable`, never silently dropped.
fn classify_source(source: &str, project_root: &Path) -> SourceKind {
    if source.starts_with("transcript:") {
        return SourceKind::Transcript;
    }
    if source.starts_with("legacy:") {
        return SourceKind::Legacy;
    }
    if file_uri_to_path(source).is_some() {
        return SourceKind::FilePath;
    }
    if has_explicit_path_marker(source) {
        return SourceKind::FilePath;
    }
    match parse_uri_scheme(source) {
        // No scheme shape at all (e.g. a bare `notes.md`) — a path by
        // elimination, the one case `Provenance::source`'s doc leaves for
        // this branch once transcript/legacy/file/marker are ruled out.
        None => SourceKind::FilePath,
        // Scheme-shaped but not one of the three forms handled above:
        // existence on disk is what decides, per this function's doc.
        Some(_) => {
            if resolve_against(project_root, Path::new(source)).exists() {
                SourceKind::FilePath
            } else {
                SourceKind::Url
            }
        },
    }
}

/// Resolve `path` against `project_root` when it is relative; an already
/// absolute path (a `file:` URI always expands to one) is returned
/// unchanged. See [`classify_source`]'s doc for why the anchor is the
/// project root and not the audit process's current directory.
fn resolve_against(project_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    }
}

/// Explicit relative/absolute path marker: starts with `/`, `./`, or `../`.
/// Does not attempt the "no scheme prefix at all" case itself —
/// [`classify_source`] handles that directly, so the two are not checked
/// twice (a bare relative path like `notes.md` has neither a marker nor a
/// scheme, and still needs to come out `FilePath`).
fn has_explicit_path_marker(source: &str) -> bool {
    source.starts_with('/') || source.starts_with("./") || source.starts_with("../")
}

/// Expand a `file:` locator (the empty-authority form, `file:///path`) into
/// the local path it names. `Provenance::source`'s doc allows a URL
/// locator, and `file:` is the one URI scheme whose entire meaning is "a
/// local path" (RFC 8089) — treating it as an opaque, unfetchable "URL" the
/// way a genuine network scheme is treated would silently stop checking a
/// claim that is, in fact, perfectly readable from disk.
///
/// Only the empty-authority form (`file:///...`) is handled: a non-empty
/// authority (`file://host/path`, meaning "read `path` from `host`") has no
/// local meaning, this crate's own writers never produce one, and guessing
/// at a remote read is worse than leaving it to [`classify_source`]'s
/// ordinary ambiguous-scheme handling — so this returns `None` for it.
///
/// Also un-does RFC 8089's Windows-drive encoding: `file:///C:/Users/...`
/// carries the drive path as `/C:/Users/...` (a leading slash the drive
/// letter absorbs), which `C:` on Windows does not recognize as itself —
/// strip it so the result round-trips to the same `C:/Users/...` form
/// [`parse_uri_scheme`]'s doc already treats as a real Windows path
/// elsewhere in this module.
///
/// Does **not** decode percent-encoding (`%20` for a space, etc.): a
/// path segment carrying a raw `%` is refused (`None`) rather than resolved
/// against its still-encoded text, which would look for a literally wrong
/// filename and could misreport a real, present file as `SourceMissing` —
/// worse than "could not verify" for a tool whose output is a delete
/// proposal. A refused `file:` URI falls through to
/// [`classify_source`]'s ordinary ambiguous-scheme handling, which reports
/// it as `unverifiable` instead of guessing wrong.
fn file_uri_to_path(source: &str) -> Option<PathBuf> {
    let after_scheme = source.strip_prefix("file://")?;
    let path_part = after_scheme.strip_prefix('/')?;
    if path_part.contains('%') {
        return None;
    }
    let bytes = path_part.as_bytes();
    let is_windows_drive_form =
        bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':');
    Some(PathBuf::from(if is_windows_drive_form {
        path_part
    } else {
        after_scheme
    }))
}

/// The URI scheme prefix at the start of `source`, if it has the shape of
/// one (RFC 3986 §3.1: a letter, then letters/digits/`+`/`-`/`.`, then
/// `:`) — `https:`, `mailto:`, `git+ssh:`, `transcript:`, `legacy:`, and
/// so on. A path that merely happens to contain a colon somewhere is not
/// enough to trip this: a **one-character** candidate is rejected outright
/// — this project's CI matrix targets `windows-latest`
/// (`.github/workflows/ci.yml`), where `C:\Users\...` / `C:/Users/...` is
/// a drive letter, not a scheme, and treating it as one would make a real,
/// existing file path silently unrecognized (and its staleness silently
/// unchecked) rather than merely flagged wrong. No scheme in the IANA
/// registry is one character, so requiring at least two costs nothing
/// against genuine schemes.
fn parse_uri_scheme(source: &str) -> Option<&str> {
    let colon = source.find(':')?;
    let candidate = &source[..colon];
    if candidate.len() < 2 {
        return None;
    }
    let mut chars = candidate.chars();
    let starts_alpha = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (starts_alpha && rest_ok).then_some(candidate)
}

/// Flag claims whose file source has drifted since it was hashed (the file
/// still exists but hashes differently, or it is gone entirely), and
/// separately list every claim staleness could not be assessed for at all
/// — see [`UnverifiableClaim`]. Non-file locators (`transcript:...`,
/// `legacy:...`) are excluded from both lists: they were never hashed from
/// fetchable content in the first place, so their absence from either
/// list is by design, not a silent gap.
///
/// `project_root` anchors every relative `Provenance::source` file path —
/// see [`classify_source`]'s doc for why that must be the project root, not
/// this process's current directory.
#[must_use]
pub fn stale_claims(
    claims: &[MemoryClaim],
    project_root: &Path,
) -> (Vec<StaleClaim>, Vec<UnverifiableClaim>) {
    let mut stale = Vec::new();
    let mut unverifiable = Vec::new();

    for claim in claims {
        let source = &claim.provenance().source;
        match classify_source(source, project_root) {
            SourceKind::Transcript | SourceKind::Legacy => {},
            SourceKind::Url => unverifiable.push(UnverifiableClaim {
                claim_id: claim.id(),
                source: source.clone(),
                reason: "source is a URL; staleness cannot be checked without fetching it over \
                         the network, which is out of scope for this audit"
                    .to_string(),
            }),
            SourceKind::FilePath => {
                // `classify_source` may have recognized `FilePath` via a
                // `file:` URI (the raw `source` string itself is not a
                // filesystem path in that case) — resolve the same way it
                // did, once, rather than re-deriving a different answer
                // here, then anchor a relative result at `project_root`.
                let raw_path = file_uri_to_path(source).unwrap_or_else(|| PathBuf::from(source));
                let path = resolve_against(project_root, &raw_path);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let current = ContentHash::compute(&bytes);
                        if current != claim.provenance().hash {
                            stale.push(StaleClaim {
                                claim_id: claim.id(),
                                source: source.clone(),
                                reason: StaleReason::ContentChanged,
                            });
                        }
                    },
                    Err(e) if e.kind() == ErrorKind::NotFound => stale.push(StaleClaim {
                        claim_id: claim.id(),
                        source: source.clone(),
                        reason: StaleReason::SourceMissing,
                    }),
                    Err(e) => {
                        // Some other I/O failure (permissions, a source
                        // that turned out to be a directory, ...) is not
                        // itself a staleness verdict either way, but it
                        // must not vanish either: an audit that silently
                        // failed to check half its claims and reported
                        // green is the same defect this whole feature
                        // exists to catch.
                        tracing::warn!(
                            claim_id = %claim.id(),
                            source = %source,
                            error = %e,
                            "could not re-hash memory claim source; reported as unverifiable"
                        );
                        unverifiable.push(UnverifiableClaim {
                            claim_id: claim.id(),
                            source: source.clone(),
                            reason: format!("could not read source: {e}"),
                        });
                    },
                }
            },
        }
    }

    (stale, unverifiable)
}

/// Parse the `RunId` out of a `MemoryClaim` source shaped like
/// `"transcript:run-<ULID>#turn-<N>"` — the locator
/// `MemoryClaim::from_transcript` callers use (see its doc example). `None`
/// for any other source shape (a file path, `legacy:<table>:<id>`): only a
/// transcript-ingested claim names a run this way.
fn run_id_from_source(source: &str) -> Option<RunId> {
    let after_prefix = source.strip_prefix("transcript:")?;
    let run_part = after_prefix.split('#').next()?;
    run_part.parse().ok()
}

/// Flag claims that trace back to a run that either registered as
/// `RunStatus::Failed` in `run_statuses`, or that appears in
/// `loop_guard_causes` (the engine's loop guard stopped it) — regardless of
/// what `run_statuses` says for it. A guard-tripped run can still read
/// `Crashed`, not `Failed`, if the daemon died before `RunFailed` was
/// appended (see the `RunStatus`-filtering lesson in
/// `.autopilot/competitive-waves/interfaces.md`); requiring `Failed` here
/// too would silently drop exactly that run from the correlation this
/// function exists to compute.
#[must_use]
pub fn failed_run_correlated_claims(
    claims: &[MemoryClaim],
    run_statuses: &HashMap<RunId, RunStatus>,
    loop_guard_causes: &HashMap<RunId, EscalationCause>,
) -> Vec<RunCorrelatedClaim> {
    claims
        .iter()
        .filter_map(|claim| {
            let run_id = run_id_from_source(&claim.provenance().source)?;
            let run_status = *run_statuses.get(&run_id)?;
            let loop_guard_cause = loop_guard_causes.get(&run_id).copied();
            if run_status != RunStatus::Failed && loop_guard_cause.is_none() {
                return None;
            }
            Some(RunCorrelatedClaim {
                claim_id: claim.id(),
                run_id,
                run_status,
                loop_guard_cause,
            })
        })
        .collect()
}

/// Run the full audit: staleness plus failed/loop-guard-stopped-run
/// correlation. Read-only end to end — no claim and no run row is ever
/// written, updated, or deleted; pruning is left to a separate, explicit,
/// human-issued command.
///
/// `project_root` anchors every relative `Provenance::source` file path —
/// see [`classify_source`]'s doc.
///
/// **Never** calls `Storage::list_runs`: that method probes every
/// live-status run's daemon pid and durably rewrites a dead one's status to
/// `Crashed` (`storage.rs`, `Storage::list_runs`) — a side effect this
/// read-only command must not have. The raw registry read
/// (`crate::runs::registry::list_runs`) returns the same rows with no
/// probe and no write.
///
/// The registry scan and the per-run event-log scan
/// ([`list_loop_guard_stopped_runs`]) are both narrowed to exactly the runs
/// at least one claim's `transcript:run-<ULID>#turn-N` locator names
/// (`run_id_from_source`) — not the whole registry, which
/// [`list_loop_guard_stopped_runs`]'s doc names as the cost driver for this
/// cross-run question. A claim naming a run absent from that narrowed set
/// (deleted from the registry, or never inserted) is not silently dropped
/// either: it surfaces as a `caveats` entry.
///
/// A run whose per-run event log this function's scan could not open or
/// read is skipped, not fatal — see [`list_loop_guard_stopped_runs`]'s doc
/// — and also surfaces as a `caveats` entry rather than failing the whole
/// report.
///
/// # Errors
/// Returns an error if the run registry itself cannot be read.
pub async fn run_audit(
    claims: &[MemoryClaim],
    storage: &Arc<Storage>,
    project_root: &Path,
) -> Result<AuditReport> {
    let (stale, unverifiable) = stale_claims(claims, project_root);

    let claimed_run_ids: std::collections::HashSet<RunId> = claims
        .iter()
        .filter_map(|claim| run_id_from_source(&claim.provenance().source))
        .collect();

    let all_runs = crate::runs::registry::list_runs(&storage.registry_pool, &RunFilter::default())
        .map_err(|e| PersistenceError::Storage(format!("list runs: {e}")))?;
    let relevant_runs: Vec<RunSummary> = all_runs
        .into_iter()
        .filter(|run| claimed_run_ids.contains(&run.id))
        .collect();
    let run_statuses: HashMap<RunId, RunStatus> = relevant_runs
        .iter()
        .map(|run| (run.id, run.status))
        .collect();

    let (loop_guard_causes, skipped_runs) =
        list_loop_guard_stopped_runs(storage, &relevant_runs).await;

    let mut caveats = Vec::new();
    for skipped in &skipped_runs {
        caveats.push(format!(
            "could not read run {}'s event log for a loop-guard escalation ({}); its claim(s) \
             are still checked for staleness and correlated by registry status alone",
            skipped.run_id, skipped.reason
        ));
    }
    // `claimed_run_ids` is a `HashSet`, whose iteration order is randomized
    // per-process (`RandomState`) — sort before turning it into output, or
    // the same input produces a different `caveats` document on every
    // invocation. `RunId` derives no `Ord` (the `define_id!` macro it comes
    // from is shared by every ID type in `surge-core`; adding one is a
    // wider change than this fix), so sort by the ULID's own string form,
    // which is lexicographically stable and unique per id — sufficient for
    // a deterministic *order*, not meant to imply chronological meaning.
    let mut missing_run_ids: Vec<RunId> = claimed_run_ids
        .iter()
        .copied()
        .filter(|run_id| !run_statuses.contains_key(run_id))
        .collect();
    missing_run_ids.sort_by_key(ToString::to_string);
    for run_id in missing_run_ids {
        caveats.push(format!(
            "a claim names run {run_id}, which is not present in the run registry; it cannot \
             be correlated with a failed or loop-guard-stopped run"
        ));
    }

    let run_correlated = failed_run_correlated_claims(claims, &run_statuses, &loop_guard_causes);

    Ok(AuditReport {
        stale,
        unverifiable,
        run_correlated,
        caveats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use surge_core::memory::{ClaimStatus, Confidence, Provenance};
    use surge_core::run_event::{EscalationCause, EventPayload, VersionedEventPayload};

    /// A project root for tests whose claim sources are already absolute
    /// (so `resolve_against` never actually joins against it) or are
    /// non-file locators (transcript/URL) that never reach a path join at
    /// all — its value is inert for those, deliberately, so the test does
    /// not need a real directory just to satisfy the parameter.
    fn irrelevant_project_root() -> &'static Path {
        Path::new("/surge-audit-test-irrelevant-project-root-does-not-exist")
    }

    fn file_claim(path: &std::path::Path, hash: ContentHash) -> MemoryClaim {
        MemoryClaim::new(
            MemoryClaimId::new(),
            "some claim text",
            Provenance::unverified(path.to_string_lossy(), hash),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap()
    }

    #[test]
    fn stale_claims_flags_content_that_changed_since_capture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, b"original content").unwrap();
        let claim = file_claim(&path, ContentHash::compute(b"original content"));

        std::fs::write(&path, b"changed content").unwrap();

        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].claim_id, claim.id());
        assert_eq!(stale[0].reason, StaleReason::ContentChanged);
        assert!(unverifiable.is_empty());
    }

    #[test]
    fn stale_claims_leaves_unchanged_content_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, b"stable content").unwrap();
        let claim = file_claim(&path, ContentHash::compute(b"stable content"));

        let (stale, unverifiable) = stale_claims(&[claim], irrelevant_project_root());
        assert!(stale.is_empty());
        assert!(unverifiable.is_empty());
    }

    #[test]
    fn stale_claims_flags_a_source_that_no_longer_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.txt");
        std::fs::write(&path, b"will be deleted").unwrap();
        let claim = file_claim(&path, ContentHash::compute(b"will be deleted"));
        std::fs::remove_file(&path).unwrap();

        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].claim_id, claim.id());
        assert_eq!(stale[0].reason, StaleReason::SourceMissing);
        assert!(unverifiable.is_empty());
    }

    #[test]
    fn stale_claims_skips_non_file_locators() {
        let claim = MemoryClaim::from_transcript(
            "the retry budget is 3 attempts",
            "transcript:run-01ARZ3NDEKTSV4RRFFQ69G5FAV#turn-1",
            ContentHash::compute(b"turn 1"),
        );
        let (stale, unverifiable) = stale_claims(&[claim], irrelevant_project_root());
        assert!(stale.is_empty());
        assert!(unverifiable.is_empty());
    }

    /// Regression test for the exact defect a reviewer caught: a URL is an
    /// allowed `Provenance::source` locator per its own doc, but the old
    /// `is_file_source` recognized "file" by negation ("not transcript,
    /// not legacy") and would send it straight into `fs::read`, get
    /// `NotFound`, and misreport a URL as a vanished file. It must land in
    /// `unverifiable`, never in `stale`.
    #[test]
    fn stale_claims_reports_a_url_source_as_unverifiable_not_stale() {
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "documented in the upstream RFC",
            Provenance::unverified(
                "https://example.com/notes.md",
                ContentHash::compute(b"whatever was captured"),
            ),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();

        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert!(
            stale.is_empty(),
            "a URL must never be misreported as a vanished file: {stale:?}"
        );
        assert_eq!(unverifiable.len(), 1);
        assert_eq!(unverifiable[0].claim_id, claim.id());
        assert!(unverifiable[0].reason.contains("URL"));
    }

    /// Regression test for the second defect a reviewer caught: a Windows
    /// drive letter (`C:\...`) is a one-character "scheme" followed by a
    /// path separator, not a URI scheme — this project's CI matrix targets
    /// `windows-latest`, so this is a real path shape, not a hypothetical
    /// one. Misclassifying it as a locator scheme silently stops checking
    /// a real file's staleness (worse than a false positive: it never
    /// shows up anywhere for a human to question). It must go through the
    /// same staleness check as any other file path.
    #[test]
    fn windows_drive_letter_path_is_classified_as_a_file_not_a_url() {
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "documented in notes.md",
            Provenance::unverified(
                r"C:\Users\surge-audit-test\notes.md",
                ContentHash::compute(b"whatever was captured"),
            ),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();

        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert!(
            unverifiable.is_empty(),
            "a drive-letter path must never be treated as an unrecognized/URL locator \
             (that silently stops checking it): {unverifiable:?}"
        );
        // The path does not exist on the test host under either OS's path
        // conventions, so it must come back through the staleness check as
        // missing — not be silently skipped.
        assert_eq!(
            stale.len(),
            1,
            "the drive-letter path must be run through the staleness check, not skipped"
        );
        assert_eq!(stale[0].claim_id, claim.id());
        assert_eq!(stale[0].reason, StaleReason::SourceMissing);

        // The fix's actual construction (a length-2-minimum scheme
        // candidate) never special-cased the backslash form above — it
        // falls out of the same threshold as the forward-slash form
        // (`C:/Users/...`), but until now nothing asserted that second
        // form directly. Pin it down explicitly rather than resting on
        // the backslash case alone.
        let forward_slash_claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "documented in notes.md",
            Provenance::unverified(
                "C:/Users/surge-audit-test/notes.md",
                ContentHash::compute(b"whatever was captured"),
            ),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();
        let (stale, unverifiable) = stale_claims(
            std::slice::from_ref(&forward_slash_claim),
            irrelevant_project_root(),
        );
        assert!(unverifiable.is_empty(), "{unverifiable:?}");
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].reason, StaleReason::SourceMissing);
    }

    /// A source that exists but cannot be read as a file (here: it is
    /// actually a directory, so `fs::read` fails with an error other than
    /// `NotFound`) must be reported, not silently dropped after a
    /// `tracing::warn!`. An audit that quietly failed to check some claims
    /// and still reported green is the defect this whole feature exists to
    /// catch.
    #[test]
    fn stale_claims_reports_an_unreadable_source_as_unverifiable_not_silently() {
        let dir = tempfile::tempdir().unwrap();
        // The source names the directory itself, not a file inside it —
        // `fs::read` on a directory fails (EISDIR), which is not `NotFound`.
        let claim = file_claim(dir.path(), ContentHash::compute(b"irrelevant"));

        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert!(
            stale.is_empty(),
            "an unreadable source is not itself a staleness verdict: {stale:?}"
        );
        assert_eq!(unverifiable.len(), 1);
        assert_eq!(unverifiable[0].claim_id, claim.id());
    }

    /// Regression test for the debt this restart closes: `file:///path`
    /// used to fall into `SourceKind::Url` (reported unverifiable,
    /// staleness never checked) even though it names a path this crate can
    /// read locally without any network fetch. It must be expanded and
    /// checked exactly like a plain path — proven here via content drift,
    /// the same signal `stale_claims_flags_content_that_changed_since_capture`
    /// proves for a plain path.
    #[test]
    fn file_uri_source_is_expanded_to_a_path_and_checked_for_content_drift() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, b"original content").unwrap();
        let uri = format!("file://{}", path.display());
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "some claim text",
            Provenance::unverified(&uri, ContentHash::compute(b"original content")),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();

        // Fresh: hash matches, so untouched.
        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert!(stale.is_empty(), "{stale:?}");
        assert!(
            unverifiable.is_empty(),
            "a `file:` URI must never be reported unverifiable: {unverifiable:?}"
        );

        std::fs::write(&path, b"changed content").unwrap();
        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert_eq!(stale.len(), 1, "{unverifiable:?}");
        assert_eq!(stale[0].claim_id, claim.id());
        assert_eq!(stale[0].reason, StaleReason::ContentChanged);
    }

    /// Regression for the percent-encoding hazard a reviewer measured:
    /// `file:///a%20b.txt` must never be resolved against its still-encoded
    /// text (`a%20b.txt`, a literally wrong filename that would almost
    /// certainly not exist) and reported `SourceMissing` — a false "the
    /// source vanished" for a tool whose output is a delete proposal is
    /// worse than an honest "could not verify". It must land in
    /// `unverifiable`, never in `stale`, even though the real (unencoded)
    /// file exists right where the URI points.
    #[test]
    fn file_uri_with_percent_encoding_is_reported_unverifiable_not_falsely_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a b.txt");
        std::fs::write(&path, b"real content").unwrap();
        let uri = format!("file://{}/a%20b.txt", dir.path().display());
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "some claim text",
            Provenance::unverified(&uri, ContentHash::compute(b"real content")),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();

        let (stale, unverifiable) =
            stale_claims(std::slice::from_ref(&claim), irrelevant_project_root());
        assert!(
            stale.is_empty(),
            "a percent-encoded `file:` URI must never be reported as a vanished source: {stale:?}"
        );
        assert_eq!(unverifiable.len(), 1);
        assert_eq!(unverifiable[0].claim_id, claim.id());
    }

    /// The other half of the same debt, negative case: a made-up scheme
    /// name (`my:`, not a registered network scheme) that does not
    /// currently exist on disk cannot be told apart from a genuine
    /// `scheme://` locator by anything short of fetching it or hardcoding
    /// a list of known scheme names — which is exactly the shape-based
    /// guessing this fix removes, not adds back for a different set of
    /// prefixes. So existence is what decides here too, the same as for
    /// `stale_claims_reports_a_url_source_as_unverifiable_not_stale`
    /// above: not found on disk, so it lands in `unverifiable` — visibly
    /// reported, never silently dropped from both lists — exactly like a
    /// genuine URL would. The positive case (the same shape, but the path
    /// genuinely exists) is
    /// `ambiguous_colon_prefixed_path_that_exists_is_treated_as_a_file_not_a_url`
    /// below, which is where this debt's actual fix shows up.
    #[test]
    fn ambiguous_colon_prefixed_relative_path_that_does_not_exist_is_reported_not_silently_dropped()
    {
        // A real, empty project root — guaranteed not to contain
        // `my:notes/file.md` — rather than reusing `irrelevant_project_root`
        // (which is fine for absolute sources, but this one is relative and
        // the test's whole point is that existence *within the project
        // root* is what decides, so the root must be a real, checkable
        // directory here).
        let project_root = tempfile::tempdir().unwrap();
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "documented in notes.md",
            Provenance::unverified(
                "my:notes/file.md",
                ContentHash::compute(b"whatever was captured"),
            ),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();

        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim), project_root.path());
        assert!(stale.is_empty(), "{stale:?}");
        assert_eq!(
            unverifiable.len(),
            1,
            "an ambiguous, currently-nonexistent locator must still be visible in the report, \
             not silently dropped from both lists"
        );
        assert_eq!(unverifiable[0].claim_id, claim.id());
    }

    /// Same ambiguous shape as above, but the path genuinely exists this
    /// time — existence, not the scheme-like shape, must be what decides it
    /// is a file. The fixture is created **inside a tempdir passed as
    /// `project_root`**, not relative to the test process's own current
    /// directory: `Provenance::source`'s file-path form is project-root
    /// relative (`interfaces.md`, "Контракт локаторов памяти"), and a
    /// reviewer measured that the previous version of this test — which
    /// created the fixture in the crate's real working directory — was
    /// itself the symptom of resolving against the wrong base. Unix-only:
    /// a literal colon inside a path component is not a portable filename
    /// character (NTFS reserves it for alternate data streams), and this
    /// project's CI matrix includes `windows-latest`.
    #[cfg(unix)]
    #[test]
    fn ambiguous_colon_prefixed_path_that_exists_is_treated_as_a_file_not_a_url() {
        let project_root = tempfile::tempdir().unwrap();
        let scheme_like_dir = project_root.path().join("my:notes");
        std::fs::create_dir(&scheme_like_dir).unwrap();
        let path = scheme_like_dir.join("file.md");
        std::fs::write(&path, b"real content").unwrap();

        // Relative to `project_root`, matching the locator contract — not
        // the absolute `path` this test built it from.
        let source = "my:notes/file.md";
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "documented in notes.md",
            Provenance::unverified(source, ContentHash::compute(b"real content")),
            Confidence::Asserted,
            ClaimStatus::Unverified,
        )
        .unwrap();

        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim), project_root.path());
        assert!(unverifiable.is_empty(), "{unverifiable:?}");
        assert!(
            stale.is_empty(),
            "an existing, unchanged ambiguous-shaped path must read as fresh: {stale:?}"
        );

        // Proves the anchor is `project_root`, not this process's `cwd`:
        // the same relative locator, audited against an *unrelated* empty
        // root, must come back through the same branch reporting
        // `unverifiable` (not found there) — the classification depends on
        // which root is passed in, not on wherever the test binary's `cwd`
        // happens to be.
        let other_root = tempfile::tempdir().unwrap();
        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim), other_root.path());
        assert!(stale.is_empty(), "{stale:?}");
        assert_eq!(
            unverifiable.len(),
            1,
            "a different project root must not find the same file: {unverifiable:?}"
        );
    }

    #[test]
    fn failed_run_correlated_claims_matches_a_transcript_sourced_claim() {
        let run_id = RunId::new();
        let claim = MemoryClaim::from_transcript(
            "root cause was a missing await",
            format!("transcript:{run_id}#turn-4"),
            ContentHash::compute(b"turn 4"),
        );
        let mut statuses = HashMap::new();
        statuses.insert(run_id, RunStatus::Failed);

        let findings =
            failed_run_correlated_claims(std::slice::from_ref(&claim), &statuses, &HashMap::new());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].claim_id, claim.id());
        assert_eq!(findings[0].run_id, run_id);
        assert_eq!(findings[0].run_status, RunStatus::Failed);
        assert_eq!(
            findings[0].loop_guard_cause, None,
            "a plain stage failure carries no loop-guard cause"
        );
    }

    #[test]
    fn failed_run_correlated_claims_ignores_a_run_that_is_merely_running_and_never_escalated() {
        let run_id = RunId::new();
        let claim = MemoryClaim::from_transcript(
            "unrelated observation",
            format!("transcript:{run_id}#turn-1"),
            ContentHash::compute(b"turn 1"),
        );
        let mut statuses = HashMap::new();
        statuses.insert(run_id, RunStatus::Running);
        // Not failed and never escalated: this run has produced no signal
        // this correlation exists to surface.
        assert!(failed_run_correlated_claims(&[claim], &statuses, &HashMap::new()).is_empty());
    }

    /// The union half of the fix: a run the loop guard stopped must be
    /// correlated even when its registry status is not `Failed` — a guard
    /// trip that raced a dead daemon can leave the run reading `Crashed`
    /// (see this module's and `list_loop_guard_stopped_runs`'s docs). This
    /// is the pure, no-I/O half of that proof; the full-stack version
    /// (real `Storage`, a real appended `EscalationRequested`) is
    /// `run_audit_correlates_a_claim_with_a_loop_guard_stopped_run_even_when_the_registry_reads_crashed`
    /// below.
    #[test]
    fn failed_run_correlated_claims_includes_a_loop_guard_stopped_run_even_when_its_status_is_not_failed()
     {
        let run_id = RunId::new();
        let claim = MemoryClaim::from_transcript(
            "the retry budget is 3 attempts",
            format!("transcript:{run_id}#turn-2"),
            ContentHash::compute(b"turn 2"),
        );
        let mut statuses = HashMap::new();
        statuses.insert(run_id, RunStatus::Crashed);
        let mut loop_guard_causes = HashMap::new();
        loop_guard_causes.insert(run_id, EscalationCause::LoopGuardRepeatedToolCall);

        let findings = failed_run_correlated_claims(&[claim], &statuses, &loop_guard_causes);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].run_status, RunStatus::Crashed);
        assert_eq!(
            findings[0].loop_guard_cause,
            Some(EscalationCause::LoopGuardRepeatedToolCall)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_audit_flags_a_stale_claim_leaves_a_fresh_one_alone_and_deletes_neither() {
        let store = crate::memory::MemoryStore::in_memory().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("source.txt");
        std::fs::write(&path, b"original").unwrap();
        let stale_claim = file_claim(&path, ContentHash::compute(b"original"));
        std::fs::write(&path, b"drifted").unwrap();
        store.add_claim(&stale_claim).unwrap();

        let fresh_claim = MemoryClaim::from_transcript(
            "unrelated fresh claim",
            "transcript:run-01ARZ3NDEKTSV4RRFFQ69G5FAV#turn-2",
            ContentHash::compute(b"turn 2"),
        );
        store.add_claim(&fresh_claim).unwrap();

        let storage_dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(storage_dir.path()).await.unwrap();

        let claims = store.list_claims().unwrap();
        let report = run_audit(&claims, &storage, irrelevant_project_root())
            .await
            .unwrap();

        assert_eq!(report.stale.len(), 1, "only the drifted claim is stale");
        assert_eq!(report.stale[0].claim_id, stale_claim.id());
        assert!(
            report
                .stale
                .iter()
                .all(|finding| finding.claim_id != fresh_claim.id()),
            "the fresh claim must not be flagged"
        );
        assert!(
            report.run_correlated.is_empty(),
            "no run exists yet to correlate against: {:?}",
            report.run_correlated
        );

        // Auditing must never remove or alter anything: both claims are
        // still present, byte-for-byte, afterward.
        let after = store.list_claims().unwrap();
        assert_eq!(after.len(), 2, "audit must not delete any claim");
        assert!(after.contains(&stale_claim));
        assert!(after.contains(&fresh_claim));
    }

    /// The end-to-end proof for the second half of R21: a claim traced to
    /// a run the loop guard stopped must show up in `run_correlated` with
    /// its cause, going through the real production path —
    /// `Storage::create_run`/`append_event`, the same writer the engine
    /// uses — not a hand-built map. Deliberately sets the run's registry
    /// status to `Crashed` (never `Failed`) to prove the union logic
    /// documented on `failed_run_correlated_claims` and
    /// `list_loop_guard_stopped_runs`: this test goes red if a future edit
    /// reverts to filtering on `RunStatus::Failed` alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_audit_correlates_a_claim_with_a_loop_guard_stopped_run_even_when_the_registry_reads_crashed()
     {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let run_id = RunId::new();
        let writer = storage.create_run(run_id, dir.path(), None).await.unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();
        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::StageEntered { node, attempt: 1 }),
                VersionedEventPayload::new(EventPayload::EscalationRequested {
                    stage: None,
                    reason: "node loop guard: node has run for 0s, past its 0s wall-clock \
                              budget; escalating"
                        .into(),
                    cause: EscalationCause::LoopGuardNodeDeadline,
                }),
            ])
            .await
            .unwrap();
        writer.flush().await.unwrap();
        drop(writer);

        storage
            .set_run_status(&run_id, RunStatus::Crashed, Some(1))
            .await
            .unwrap();

        let claim = MemoryClaim::from_transcript(
            "root cause was the node running past its budget",
            format!("transcript:{run_id}#turn-3"),
            ContentHash::compute(b"turn 3"),
        );

        let report = run_audit(
            std::slice::from_ref(&claim),
            &storage,
            irrelevant_project_root(),
        )
        .await
        .unwrap();

        assert_eq!(
            report.run_correlated.len(),
            1,
            "{:?}",
            report.run_correlated
        );
        let finding = &report.run_correlated[0];
        assert_eq!(finding.claim_id, claim.id());
        assert_eq!(finding.run_id, run_id);
        assert_eq!(
            finding.run_status,
            RunStatus::Crashed,
            "the registry status is deliberately not Failed — the loop-guard cause alone \
             must be what pulls this run into the correlation"
        );
        assert_eq!(
            finding.loop_guard_cause,
            Some(EscalationCause::LoopGuardNodeDeadline)
        );
        assert!(
            report.caveats.is_empty(),
            "the run is registered and its log is readable: {:?}",
            report.caveats
        );
    }

    /// Blocker regression, both halves at once: `run_audit` must never
    /// mutate the registry (a reviewer measured a `Running`/dead-pid row
    /// silently flip to `Crashed` after one `surge memory audit`, via
    /// `Storage::list_runs`'s stale-pid rewrite — a contract break for a
    /// command whose whole premise is "propose, never change anything"),
    /// and it must not waste a scan attempt on a run no claim names (the
    /// registry-scan narrowing this restart adds): an orphaned registry row
    /// with a dead pid and no per-run directory sits alongside a real,
    /// claimed, guard-tripped run — if the orphan were scanned it would
    /// produce a `caveats` entry (its reader can't open), so an empty
    /// `caveats` here also proves the narrowing, not just the no-mutation
    /// half.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_audit_never_mutates_the_registry_and_never_scans_a_run_no_claim_names() {
        use crate::runs::registry;

        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        let claimed_run = RunId::new();
        let writer = storage
            .create_run(claimed_run, dir.path(), None)
            .await
            .unwrap();
        let node = surge_core::keys::NodeKey::try_from("implement").unwrap();
        writer
            .append_events(vec![
                VersionedEventPayload::new(EventPayload::StageEntered { node, attempt: 1 }),
                VersionedEventPayload::new(EventPayload::EscalationRequested {
                    stage: None,
                    reason: "node loop guard: node has run for 0s, past its 0s wall-clock \
                              budget; escalating"
                        .into(),
                    cause: EscalationCause::LoopGuardNodeDeadline,
                }),
            ])
            .await
            .unwrap();
        writer.flush().await.unwrap();
        drop(writer);

        // A registry-only row: `Running` status, an impossibly-high (dead)
        // daemon pid, no per-run directory, and no claim ever names it.
        // `Storage::list_runs` would rewrite this to `Crashed` the instant
        // it is listed; the raw registry read must not.
        let orphaned_run = RunId::new();
        registry::insert_run(
            &storage.registry_pool,
            &registry::RunSummary {
                id: orphaned_run,
                project_path: dir.path().to_path_buf(),
                pipeline_template: None,
                status: RunStatus::Running,
                started_at_ms: 1,
                ended_at_ms: None,
                daemon_pid: Some(i32::MAX),
            },
        )
        .unwrap();

        let claim = MemoryClaim::from_transcript(
            "root cause was the node running past its budget",
            format!("transcript:{claimed_run}#turn-1"),
            ContentHash::compute(b"turn 1"),
        );

        let report = run_audit(
            std::slice::from_ref(&claim),
            &storage,
            irrelevant_project_root(),
        )
        .await
        .unwrap();

        assert_eq!(
            report.run_correlated.len(),
            1,
            "{:?}",
            report.run_correlated
        );
        assert_eq!(report.run_correlated[0].run_id, claimed_run);
        assert!(
            report.caveats.is_empty(),
            "the orphaned run must never even be attempted, since no claim names it: {:?}",
            report.caveats
        );

        let orphaned_after = registry::get_run(&storage.registry_pool, &orphaned_run)
            .unwrap()
            .unwrap();
        assert_eq!(
            orphaned_after.status,
            RunStatus::Running,
            "run_audit must never rewrite registry status, even for a dead-pid row"
        );
        assert_eq!(orphaned_after.ended_at_ms, None);
    }

    /// Regression for the nondeterminism a reviewer measured: `caveats`'s
    /// "run not in the registry" half is built by iterating
    /// `claimed_run_ids`, a `HashSet<RunId>` — `RandomState` reseeds on
    /// every fresh `HashSet` (not merely once per process), so two calls to
    /// `run_audit` over the exact same claims, in the exact same process,
    /// could already emit `caveats` in two different orders before the
    /// fix. A machine consumer of `--format json` must see the same
    /// document for the same input.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_audit_caveats_order_is_deterministic_across_repeated_calls() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(dir.path()).await.unwrap();

        // Three claims naming three different runs, none present in the
        // registry — three "run not in the registry" caveat entries whose
        // relative order is exactly what a `HashSet` iteration would
        // scramble.
        let claims: Vec<MemoryClaim> = (0..3)
            .map(|i| {
                MemoryClaim::from_transcript(
                    format!("claim {i}"),
                    format!("transcript:{}#turn-1", RunId::new()),
                    ContentHash::compute(format!("turn {i}").as_bytes()),
                )
            })
            .collect();

        let first = run_audit(&claims, &storage, irrelevant_project_root())
            .await
            .unwrap()
            .caveats;
        assert_eq!(first.len(), 3, "{first:?}");

        for attempt in 0..4 {
            let repeat = run_audit(&claims, &storage, irrelevant_project_root())
                .await
                .unwrap()
                .caveats;
            assert_eq!(
                repeat, first,
                "attempt {attempt}: the same input must produce the same `caveats` document \
                 every time, not one that depends on HashSet iteration order"
            );
        }
    }
}
