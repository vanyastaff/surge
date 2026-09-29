//! Run-id resolution for operators typing a short suffix.

use std::sync::Arc;

use surge_core::RunId;
use surge_persistence::runs::Storage;
use surge_persistence::runs::registry::RunFilter;

use crate::operator::error::{MIN_SUFFIX_LEN, OperatorError, RunIdError, SUFFIX_SCAN_LIMIT};

/// Resolve a run id, accepting the full ULID or a unique short suffix (as shown
/// by `surge inbox`).
///
/// Guards: an empty or too-short suffix is rejected (`ends_with("")` would
/// match every run); if the run scan hits its ceiling, an otherwise-unique
/// match is treated as ambiguous rather than trusted, since a colliding run
/// could sit beyond the window.
///
/// # Errors
/// Returns [`OperatorError::RunId`] for an unusable or non-unique id and
/// [`OperatorError::ListRunsForIdMatch`] if the registry cannot be listed.
pub async fn resolve_run_id(storage: &Arc<Storage>, value: &str) -> Result<RunId, OperatorError> {
    let value = value.trim();
    if let Ok(id) = value.parse::<RunId>() {
        return Ok(id);
    }
    if value.len() < MIN_SUFFIX_LEN {
        return Err(RunIdError::TooShort {
            value: value.to_owned(),
        }
        .into());
    }
    let runs = storage
        .list_runs(RunFilter {
            status: None,
            project_path: None,
            limit: Some(SUFFIX_SCAN_LIMIT),
        })
        .await
        .map_err(OperatorError::ListRunsForIdMatch)?;
    let truncated = runs.len() >= SUFFIX_SCAN_LIMIT;
    let matches: Vec<RunId> = runs
        .iter()
        .filter(|r| r.id.to_string().ends_with(value))
        .map(|r| r.id)
        .collect();
    match matches.as_slice() {
        [one] if !truncated => Ok(*one),
        [_one] => Err(RunIdError::PossiblyAmbiguous {
            value: value.to_owned(),
        }
        .into()),
        [] => Err(RunIdError::NotFound {
            value: value.to_owned(),
        }
        .into()),
        many => Err(RunIdError::Ambiguous {
            value: value.to_owned(),
            count: many.len(),
        }
        .into()),
    }
}
