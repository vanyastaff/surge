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

use crate::runs::registry::{self, RunFilter};
use crate::{PersistenceError, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use serde::Serialize;
use std::collections::HashMap;
use std::io::ErrorKind;
use surge_core::memory::MemoryClaim;
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
    /// Claims that trace back to a run that failed.
    pub run_correlated: Vec<RunCorrelatedClaim>,
    /// Known gaps in this report. Never a fabricated finding — only an
    /// honest statement of what could not be computed. Today this always
    /// names the missing loop-guard ("looping") run signal; see
    /// [`LOOPING_CORRELATION_UNAVAILABLE`].
    pub caveats: Vec<String>,
}

/// Why a run stopped by the loop guard cannot be told apart from any other
/// failed run today.
///
/// No `LoopGuard`/`Verdict` type exists yet (`surge-orchestrator::guard` in
/// `.autopilot/competitive-waves/interfaces.md` is not implemented). The one
/// durable trace of a guard-style stop, `EventPayload::EscalationRequested`,
/// is not indexed in the run registry (only reachable by replaying a run's
/// full event log) and already covers unrelated causes — MCP
/// restart-exhaustion and bootstrap retry-cap exhaustion — neither of which
/// is the tool-call-repeat-plus-node-deadline guard trip this correlation
/// class means. Treating any `EscalationRequested` as "looping" would
/// misclassify those other causes, which is worse than reporting the gap:
/// see this module's docs and the delivery report for `08-memory-audit`.
pub const LOOPING_CORRELATION_UNAVAILABLE: &str = "loop-guard (\"looping\") run correlation is \
    not available: no durable, queryable signal distinguishes a guard-tripped run from any \
    other failure today (no LoopGuard/Verdict type exists; EscalationRequested conflates \
    unrelated escalation causes and is not indexed in the run registry). Only failed-run \
    correlation is included below.";

/// What kind of locator a claim's `Provenance::source` is, as far as this
/// module can tell. `Provenance::source`'s own doc allows a file path, a
/// transcript turn, or a URL; this crate additionally mints
/// `legacy:<table>:<id>` for migrated rows. Only [`SourceKind::FilePath`]
/// has a "current file hash" to compare provenance against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// A path this crate can read from disk and re-hash.
    FilePath,
    /// A transcript-ingest locator (`transcript:run-<ULID>#turn-<N>`).
    /// Never hashed from fetchable content in the first place — not
    /// eligible for staleness, and not a gap in the report either.
    Transcript,
    /// A migrated-legacy locator (`legacy:<table>:<id>`). Same treatment
    /// as `Transcript`.
    Legacy,
    /// A URL (`scheme://...` or `scheme:...`) — an allowed locator per
    /// `Provenance::source`'s doc, but fetching one over the network is
    /// out of scope for a local staleness check.
    Url,
    /// Anything else: not a recognized locator scheme and not shaped like
    /// a file path either. Reported, not guessed at.
    Unrecognized,
}

/// Classify `source` **positively**: a file path is recognized by its own
/// shape (starts with `/`, `./`, or `../`, or simply carries no
/// scheme-shaped prefix), not by process of elimination against
/// `transcript:`/`legacy:` alone. That distinction matters because
/// `Provenance::source`'s doc explicitly allows a URL as a locator too —
/// treating "not transcript, not legacy" as "must be a file" would send a
/// URL straight into `fs::read`, get `NotFound`, and misreport it as a
/// vanished file source it never was.
fn classify_source(source: &str) -> SourceKind {
    if source.starts_with("transcript:") {
        return SourceKind::Transcript;
    }
    if source.starts_with("legacy:") {
        return SourceKind::Legacy;
    }
    if is_file_path(source) {
        return SourceKind::FilePath;
    }
    if parse_uri_scheme(source).is_some() {
        return SourceKind::Url;
    }
    SourceKind::Unrecognized
}

/// Positive file-path check: an absolute path, an explicit relative
/// marker, or (falling through) anything that does not open with a
/// URI-scheme-shaped prefix (see [`parse_uri_scheme`]).
fn is_file_path(source: &str) -> bool {
    if source.starts_with('/') || source.starts_with("./") || source.starts_with("../") {
        return true;
    }
    parse_uri_scheme(source).is_none()
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
#[must_use]
pub fn stale_claims(claims: &[MemoryClaim]) -> (Vec<StaleClaim>, Vec<UnverifiableClaim>) {
    let mut stale = Vec::new();
    let mut unverifiable = Vec::new();

    for claim in claims {
        let source = &claim.provenance().source;
        match classify_source(source) {
            SourceKind::Transcript | SourceKind::Legacy => {},
            SourceKind::Url => unverifiable.push(UnverifiableClaim {
                claim_id: claim.id(),
                source: source.clone(),
                reason: "source is a URL; staleness cannot be checked without fetching it over \
                         the network, which is out of scope for this audit"
                    .to_string(),
            }),
            SourceKind::Unrecognized => unverifiable.push(UnverifiableClaim {
                claim_id: claim.id(),
                source: source.clone(),
                reason: "source form is not recognized as a file path, URL, transcript locator, \
                         or legacy locator; staleness cannot be checked"
                    .to_string(),
            }),
            SourceKind::FilePath => match std::fs::read(source) {
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
                    // Some other I/O failure (permissions, a source that
                    // turned out to be a directory, ...) is not itself a
                    // staleness verdict either way, but it must not
                    // vanish either: an audit that silently failed to
                    // check half its claims and reported green is the
                    // same defect this whole feature exists to catch.
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

/// Flag claims that trace back to one of `failed_runs`. Looping/guard-tripped
/// correlation is deliberately not attempted here — see
/// [`LOOPING_CORRELATION_UNAVAILABLE`].
#[must_use]
pub fn failed_run_correlated_claims(
    claims: &[MemoryClaim],
    failed_runs: &HashMap<RunId, RunStatus>,
) -> Vec<RunCorrelatedClaim> {
    claims
        .iter()
        .filter_map(|claim| {
            let run_id = run_id_from_source(&claim.provenance().source)?;
            let run_status = *failed_runs.get(&run_id)?;
            Some(RunCorrelatedClaim {
                claim_id: claim.id(),
                run_id,
                run_status,
            })
        })
        .collect()
}

/// Run the full audit: staleness plus failed-run correlation, against the
/// live run registry. Read-only end to end — no claim and no run row is
/// ever written, updated, or deleted; pruning is left to a separate,
/// explicit, human-issued command.
///
/// # Errors
/// Returns an error if the run registry cannot be queried.
pub fn run_audit(
    claims: &[MemoryClaim],
    registry_pool: &Pool<SqliteConnectionManager>,
) -> Result<AuditReport> {
    let (stale, unverifiable) = stale_claims(claims);

    let failed_runs = registry::list_runs(
        registry_pool,
        &RunFilter {
            status: Some(RunStatus::Failed),
            project_path: None,
            limit: None,
        },
    )
    .map_err(|e| PersistenceError::Storage(format!("list failed runs: {e}")))?;
    let failed_by_id: HashMap<RunId, RunStatus> = failed_runs
        .into_iter()
        .map(|run| (run.id, run.status))
        .collect();

    let run_correlated = failed_run_correlated_claims(claims, &failed_by_id);

    Ok(AuditReport {
        stale,
        unverifiable,
        run_correlated,
        caveats: vec![LOOPING_CORRELATION_UNAVAILABLE.to_string()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::SystemClock;
    use crate::runs::registry::open_registry_pool;
    use surge_core::memory::{ClaimStatus, Confidence, Provenance};

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

        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim));
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

        let (stale, unverifiable) = stale_claims(&[claim]);
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

        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim));
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
        let (stale, unverifiable) = stale_claims(&[claim]);
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

        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim));
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

        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim));
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

        let (stale, unverifiable) = stale_claims(std::slice::from_ref(&claim));
        assert!(
            stale.is_empty(),
            "an unreadable source is not itself a staleness verdict: {stale:?}"
        );
        assert_eq!(unverifiable.len(), 1);
        assert_eq!(unverifiable[0].claim_id, claim.id());
    }

    #[test]
    fn failed_run_correlated_claims_matches_a_transcript_sourced_claim() {
        let run_id = RunId::new();
        let claim = MemoryClaim::from_transcript(
            "root cause was a missing await",
            format!("transcript:{run_id}#turn-4"),
            ContentHash::compute(b"turn 4"),
        );
        let mut failed = HashMap::new();
        failed.insert(run_id, RunStatus::Failed);

        let findings = failed_run_correlated_claims(std::slice::from_ref(&claim), &failed);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].claim_id, claim.id());
        assert_eq!(findings[0].run_id, run_id);
        assert_eq!(findings[0].run_status, RunStatus::Failed);
    }

    #[test]
    fn failed_run_correlated_claims_ignores_a_run_not_in_the_failed_set() {
        let run_id = RunId::new();
        let claim = MemoryClaim::from_transcript(
            "unrelated observation",
            format!("transcript:{run_id}#turn-1"),
            ContentHash::compute(b"turn 1"),
        );
        // Empty map: `run_id` never failed (or was never observed at all).
        assert!(failed_run_correlated_claims(&[claim], &HashMap::new()).is_empty());
    }

    #[test]
    fn run_audit_flags_a_stale_claim_leaves_a_fresh_one_alone_and_deletes_neither() {
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

        let registry_dir = tempfile::tempdir().unwrap();
        let pool = open_registry_pool(registry_dir.path(), &SystemClock).unwrap();

        let claims = store.list_claims().unwrap();
        let report = run_audit(&claims, &pool).unwrap();

        assert_eq!(report.stale.len(), 1, "only the drifted claim is stale");
        assert_eq!(report.stale[0].claim_id, stale_claim.id());
        assert!(
            report
                .stale
                .iter()
                .all(|finding| finding.claim_id != fresh_claim.id()),
            "the fresh claim must not be flagged"
        );

        // Auditing must never remove or alter anything: both claims are
        // still present, byte-for-byte, afterward.
        let after = store.list_claims().unwrap();
        assert_eq!(after.len(), 2, "audit must not delete any claim");
        assert!(after.contains(&stale_claim));
        assert!(after.contains(&fresh_claim));
    }
}
