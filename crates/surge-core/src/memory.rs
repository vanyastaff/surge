//! Memory as claims, not notes.
//!
//! A memory entry is a claim of fact with enough provenance to judge whether
//! it should still be believed: where it came from ([`Provenance`]), how much
//! it is worth trusting ([`Confidence`], a three-level tag — never a bool),
//! and whether it has actually been checked since it was recorded
//! ([`ClaimStatus`]). Anything captured from a transcript or conversation
//! starts life explicitly unverified — set at the moment it is captured, not
//! patched in by some later pass.
//!
//! This module owns the *shape* of a claim and its storage-independent
//! constructors. Selecting claims into a context budget, auditing them for
//! staleness, and writing them back at a run boundary are separate concerns
//! owned elsewhere.

use crate::content_hash::ContentHash;
use crate::id::MemoryClaimId;
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use std::str::FromStr;

// ── Confidence ───────────────────────────────────────────────────────

/// How much a [`MemoryClaim`] is worth trusting — a three-level tag, never
/// a boolean.
///
/// Variant declaration order **is** the trust ranking, most-trusted first:
/// `Verified < NameMatched < Asserted`. Selecting claims into a context
/// budget so that direct evidence is admitted ahead of weak recollection
/// relies on this ordering; that selection itself is a separate module's
/// job, but the ranking it selects by is defined here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// The claim was checked by actually running the verification command
    /// against its source, and it still holds.
    Verified,
    /// The referenced name, symbol, or path was found to still match, but
    /// the full verification command was not (re-)run.
    NameMatched,
    /// Recorded as true without any verification step. The starting
    /// confidence for anything pulled out of a transcript or conversation.
    Asserted,
}

impl Confidence {
    /// Stable string form, matching the vocabulary used to describe the
    /// three levels (`verified` / `name_matched` / `asserted`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::NameMatched => "name_matched",
            Self::Asserted => "asserted",
        }
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unrecognized [`Confidence`] string.
#[derive(Debug, Clone, thiserror::Error)]
#[error("unknown Confidence: {0:?}")]
pub struct ParseConfidenceError(pub String);

impl FromStr for Confidence {
    type Err = ParseConfidenceError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "verified" => Self::Verified,
            "name_matched" => Self::NameMatched,
            "asserted" => Self::Asserted,
            other => return Err(ParseConfidenceError(other.to_string())),
        })
    }
}

// ── ClaimStatus ──────────────────────────────────────────────────────

/// Whether a [`MemoryClaim`] has been checked against its source since it
/// was recorded.
///
/// Distinct from [`Confidence`]: confidence ranks how much a claim is worth
/// trusting; status only records whether a check has happened at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    /// Checked against [`Provenance::source`] and found to still hold.
    Verified,
    /// Not yet checked since being recorded. Anything ingested from a
    /// transcript or conversation is captured in this state — never
    /// upgraded after the fact by a later pass.
    Unverified,
}

impl ClaimStatus {
    /// Stable string form (`verified` / `unverified`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Unverified => "unverified",
        }
    }
}

impl fmt::Display for ClaimStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when parsing an unrecognized [`ClaimStatus`] string.
#[derive(Debug, Clone, thiserror::Error)]
#[error("unknown ClaimStatus: {0:?}")]
pub struct ParseClaimStatusError(pub String);

impl FromStr for ClaimStatus {
    type Err = ParseClaimStatusError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "verified" => Self::Verified,
            "unverified" => Self::Unverified,
            other => return Err(ParseClaimStatusError(other.to_string())),
        })
    }
}

// ── Provenance ───────────────────────────────────────────────────────

/// Where a [`MemoryClaim`] came from and how it was last checked.
///
/// `source` and `hash` are always known — a claim always has a locator and
/// a hash of that locator's content at capture time. `verified_by` and
/// `verified_at` are only populated once someone actually re-checks the
/// claim against its source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Path (or other locator — a transcript turn, a URL) the claim was
    /// captured from.
    pub source: String,
    /// Content hash taken at capture time. For a source with independently
    /// fetchable content (a file, a transcript turn) this is the hash of
    /// that content, so later verification can re-fetch `source` and
    /// re-hash it to detect drift. For a locator with no independently
    /// fetchable content of its own — for example a migrated legacy
    /// record, addressed by a synthetic `legacy:<table>:<id>` locator —
    /// it is the hash of the claim's own `text` instead, which is the
    /// closest thing such a source has to reproducible content.
    pub hash: ContentHash,
    /// The command last used to verify the claim against its source, if any.
    pub verified_by: Option<String>,
    /// Unix-ms timestamp of the last verification, if any.
    pub verified_at: Option<u64>,
}

impl Provenance {
    /// Provenance for a claim that has never been verified: only the
    /// source and its hash are known.
    #[must_use]
    pub fn unverified(source: impl Into<String>, hash: ContentHash) -> Self {
        Self {
            source: source.into(),
            hash,
            verified_by: None,
            verified_at: None,
        }
    }

    /// Provenance for a claim that has just been verified.
    #[must_use]
    pub fn verified(
        source: impl Into<String>,
        hash: ContentHash,
        verified_by: impl Into<String>,
        verified_at_ms: u64,
    ) -> Self {
        Self {
            source: source.into(),
            hash,
            verified_by: Some(verified_by.into()),
            verified_at: Some(verified_at_ms),
        }
    }

    /// True once both `verified_by` and `verified_at` are populated.
    #[must_use]
    pub fn is_verified(&self) -> bool {
        self.verified_by.is_some() && self.verified_at.is_some()
    }
}

// ── MemoryClaim ──────────────────────────────────────────────────────

/// A single unit of project memory: a claim of fact, with enough provenance
/// to judge whether it should still be believed.
///
/// Fields are private and reached through getters. This is not
/// encapsulation for its own sake: `ClaimStatus::Verified` without a
/// verified [`Provenance`] must be **unrepresentable**, not just
/// discouraged, so nothing — a struct literal, a field assignment, or
/// deserializing untrusted JSON — can build one except through
/// [`MemoryClaim::new`], which checks it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryClaim {
    id: MemoryClaimId,
    text: String,
    provenance: Provenance,
    confidence: Confidence,
    status: ClaimStatus,
}

/// Error returned by [`MemoryClaim::new`] when `status` and `provenance`
/// disagree.
///
/// `ClaimStatus::Verified` is a claim that says "this holds, and here is
/// the proof": it requires `Provenance::verified_by` and
/// `Provenance::verified_at` to both be populated. A claim that reports
/// verified status without recording who checked it and when is an
/// assertion without evidence — exactly what per-claim provenance exists
/// to rule out, so it is rejected at construction rather than trusted by
/// convention.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "ClaimStatus::Verified requires provenance.verified_by and provenance.verified_at to both be set"
)]
pub struct UnprovenVerifiedStatus;

impl MemoryClaim {
    /// Unique identifier.
    #[must_use]
    pub fn id(&self) -> MemoryClaimId {
        self.id
    }

    /// The claim's text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Where it came from and how it was last checked.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// How much to trust it.
    #[must_use]
    pub fn confidence(&self) -> Confidence {
        self.confidence
    }

    /// Whether it has been checked since being recorded.
    #[must_use]
    pub fn status(&self) -> ClaimStatus {
        self.status
    }

    /// Rebuild this claim under a different id, leaving everything else
    /// unchanged. Changing the id can never violate the `Verified`
    /// invariant (it does not touch `status`/`provenance`), so this is
    /// infallible. Used when a deterministic id — one reused from a
    /// migrated legacy row, say — turns out to collide with an unrelated
    /// claim and needs to be replaced rather than silently dropped.
    #[must_use]
    pub fn with_id(mut self, id: MemoryClaimId) -> Self {
        self.id = id;
        self
    }

    /// Whether this claim may be superseded in place by a fresher
    /// write-back about the same subject, rather than left standing next to
    /// a new claim.
    ///
    /// Only ever true for [`ClaimStatus::Unverified`]. A [`ClaimStatus::Verified`]
    /// claim carries a verification command and timestamp
    /// (`Provenance::is_verified`) — that is proof, and background write-back
    /// never silently retires proof just because a fresh, unverified
    /// assertion showed up about the same thing (`.autopilot/competitive-waves/spec.md`
    /// §24, History 20 / R22: "a verified claim is never aged").
    #[must_use]
    pub fn is_aging_eligible(&self) -> bool {
        self.status == ClaimStatus::Unverified
    }

    /// Build a claim that is always `ClaimStatus::Unverified` — infallible
    /// by construction, since the `Verified`-requires-provenance invariant
    /// only constrains `ClaimStatus::Verified`. Not `pub`: external callers
    /// go through [`from_transcript`](Self::from_transcript) (this crate)
    /// or the validating [`new`](Self::new) (everyone else, including
    /// `surge-persistence`'s legacy-row migration, which cannot borrow this
    /// private helper across the crate boundary and instead propagates
    /// `new`'s `Result` with `?` even though it is provably always `Ok`).
    fn unverified(
        id: MemoryClaimId,
        text: impl Into<String>,
        provenance: Provenance,
        confidence: Confidence,
    ) -> Self {
        Self {
            id,
            text: text.into(),
            provenance,
            confidence,
            status: ClaimStatus::Unverified,
        }
    }

    /// Record a claim pulled from a transcript or conversation.
    ///
    /// Anything ingested this way starts life at `Confidence::Asserted` /
    /// `ClaimStatus::Unverified` — the starting point this constructor
    /// fixes, rather than defaulting or patching one in later.
    #[must_use]
    pub fn from_transcript(
        text: impl Into<String>,
        source: impl Into<String>,
        source_hash: ContentHash,
    ) -> Self {
        Self::unverified(
            MemoryClaimId::new(),
            text,
            Provenance::unverified(source, source_hash),
            Confidence::Asserted,
        )
    }

    /// Construct a claim with fully specified provenance, confidence, and
    /// status — for reconstructing a claim read back from storage, or for
    /// recording one whose provenance is already verified.
    ///
    /// # Errors
    ///
    /// Returns [`UnprovenVerifiedStatus`] if `status` is
    /// `ClaimStatus::Verified` but `provenance` is not
    /// [`Provenance::is_verified`] — see [`UnprovenVerifiedStatus`] for why
    /// that combination is rejected rather than merely discouraged.
    pub fn new(
        id: MemoryClaimId,
        text: impl Into<String>,
        provenance: Provenance,
        confidence: Confidence,
        status: ClaimStatus,
    ) -> Result<Self, UnprovenVerifiedStatus> {
        if status == ClaimStatus::Verified && !provenance.is_verified() {
            return Err(UnprovenVerifiedStatus);
        }
        Ok(Self {
            id,
            text: text.into(),
            provenance,
            confidence,
            status,
        })
    }
}

/// Serde-only mirror of [`MemoryClaim`]'s shape, used solely to deserialize
/// through [`MemoryClaim::new`]. Never exported: constructing a
/// `MemoryClaim` from untrusted JSON must not be able to skip the
/// `Verified`-requires-provenance check any more than a struct literal or a
/// field assignment can (both of which `MemoryClaim`'s private fields
/// already rule out).
#[derive(Deserialize)]
struct RawMemoryClaim {
    id: MemoryClaimId,
    text: String,
    provenance: Provenance,
    confidence: Confidence,
    status: ClaimStatus,
}

impl<'de> Deserialize<'de> for MemoryClaim {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawMemoryClaim::deserialize(deserializer)?;
        MemoryClaim::new(raw.id, raw.text, raw.provenance, raw.confidence, raw.status)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_ranks_verified_ahead_of_name_matched_ahead_of_asserted() {
        assert!(Confidence::Verified < Confidence::NameMatched);
        assert!(Confidence::NameMatched < Confidence::Asserted);
    }

    #[test]
    fn confidence_string_form_matches_the_three_named_levels() {
        assert_eq!(Confidence::Verified.as_str(), "verified");
        assert_eq!(Confidence::NameMatched.as_str(), "name_matched");
        assert_eq!(Confidence::Asserted.as_str(), "asserted");
    }

    #[test]
    fn confidence_roundtrips_through_its_string_form() {
        for c in [
            Confidence::Verified,
            Confidence::NameMatched,
            Confidence::Asserted,
        ] {
            assert_eq!(c.as_str().parse::<Confidence>().unwrap(), c);
        }
    }

    #[test]
    fn claim_status_roundtrips_through_its_string_form() {
        for s in [ClaimStatus::Verified, ClaimStatus::Unverified] {
            assert_eq!(s.as_str().parse::<ClaimStatus>().unwrap(), s);
        }
    }

    #[test]
    fn from_transcript_marks_the_claim_unverified_and_asserted() {
        let hash = ContentHash::compute(b"transcript turn 12");
        let claim = MemoryClaim::from_transcript(
            "the retry budget is 3 attempts",
            "transcript:run-01ABC#turn-12",
            hash,
        );

        assert_eq!(claim.status(), ClaimStatus::Unverified);
        assert_eq!(claim.confidence(), Confidence::Asserted);
        assert!(!claim.provenance().is_verified());
        assert_eq!(claim.provenance().source, "transcript:run-01ABC#turn-12");
        assert_eq!(claim.provenance().hash, hash);
    }

    #[test]
    fn provenance_verified_populates_command_and_timestamp() {
        let hash = ContentHash::compute(b"src/lib.rs");
        let provenance = Provenance::verified("src/lib.rs", hash, "cargo test", 1_700_000_000_000);

        assert!(provenance.is_verified());
        assert_eq!(provenance.verified_by.as_deref(), Some("cargo test"));
        assert_eq!(provenance.verified_at, Some(1_700_000_000_000));
    }

    #[test]
    fn memory_claim_json_roundtrip_preserves_every_field() {
        let hash = ContentHash::compute(b"src/lib.rs");
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "the retry budget is 3 attempts",
            Provenance::verified("src/lib.rs", hash, "cargo test", 1_700_000_000_000),
            Confidence::Verified,
            ClaimStatus::Verified,
        )
        .unwrap();

        let json = serde_json::to_string(&claim).unwrap();
        let parsed: MemoryClaim = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, claim);
    }

    #[test]
    fn deserializing_verified_status_without_verified_provenance_fails() {
        // Hand-built JSON standing in for untrusted input: `status:
        // "verified"` but no `verified_by`/`verified_at` — exactly what a
        // struct literal or direct field assignment could otherwise produce
        // now that `MemoryClaim`'s fields are private. Deserializing must
        // reject it via the same check `new` applies, not build it anyway.
        let json = serde_json::json!({
            // Bare ULID form (no `claim-` prefix): that prefix only exists
            // in `MemoryClaimId`'s `Display`/`FromStr` impls for CLI/human
            // use — its derived `Deserialize` goes through `Ulid`'s serde
            // impl, which parses the bare canonical string.
            "id": MemoryClaimId::new().as_ulid().to_string(),
            "text": "the retry budget is 3 attempts",
            "provenance": {
                "source": "src/lib.rs",
                "hash": ContentHash::compute(b"src/lib.rs").to_string(),
                "verified_by": null,
                "verified_at": null,
            },
            "confidence": "verified",
            "status": "verified",
        });

        let result: std::result::Result<MemoryClaim, _> = serde_json::from_value(json);
        let err = result.expect_err("an unproven `verified` status must not deserialize");
        // Assert the *reason*, not just that some error occurred: this must
        // fail because `MemoryClaim::new`'s `UnprovenVerifiedStatus` check
        // rejected it via `serde::de::Error::custom`, not because of an
        // unrelated deserialization problem (a renamed field, a reshaped
        // hash format) that would leave this test green for the wrong
        // reason.
        assert!(
            err.to_string()
                .contains(&UnprovenVerifiedStatus.to_string()),
            "expected the UnprovenVerifiedStatus reason in the deserialize error, got: {err}"
        );
    }

    #[test]
    fn new_rejects_verified_status_without_verified_provenance() {
        let hash = ContentHash::compute(b"src/lib.rs");
        let result = MemoryClaim::new(
            MemoryClaimId::new(),
            "the retry budget is 3 attempts",
            Provenance::unverified("src/lib.rs", hash),
            Confidence::Verified,
            ClaimStatus::Verified,
        );

        assert_eq!(result, Err(UnprovenVerifiedStatus));
    }

    #[test]
    fn new_accepts_verified_status_when_provenance_is_verified() {
        let hash = ContentHash::compute(b"src/lib.rs");
        let result = MemoryClaim::new(
            MemoryClaimId::new(),
            "the retry budget is 3 attempts",
            Provenance::verified("src/lib.rs", hash, "cargo test", 1_700_000_000_000),
            Confidence::Verified,
            ClaimStatus::Verified,
        );

        assert!(result.is_ok());
    }

    #[test]
    fn unverified_claim_is_aging_eligible() {
        let claim = MemoryClaim::from_transcript(
            "the retry budget is 3 attempts",
            "transcript:run-01ARZ3NDEKTSV4RRFFQ69G5FAV#turn-1",
            ContentHash::compute(b"turn 1"),
        );

        assert!(claim.is_aging_eligible());
    }

    #[test]
    fn verified_claim_is_never_aging_eligible() {
        let hash = ContentHash::compute(b"src/lib.rs");
        let claim = MemoryClaim::new(
            MemoryClaimId::new(),
            "the retry budget is 3 attempts",
            Provenance::verified("src/lib.rs", hash, "cargo test", 1_700_000_000_000),
            Confidence::Verified,
            ClaimStatus::Verified,
        )
        .unwrap();

        assert!(
            !claim.is_aging_eligible(),
            "a verified claim must never be reported eligible for aging/replacement"
        );
    }
}
