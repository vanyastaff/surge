//! Run-id resolution for operators typing a short suffix.

use std::sync::Arc;

use surge_core::RunId;
use surge_persistence::runs::Storage;

use crate::operator::error::{MIN_SUFFIX_LEN, OperatorError, RunIdError};

/// How many matching runs are fetched to report an ambiguity; the reported
/// count is capped here.
const AMBIGUITY_CAP: usize = 50;

/// Resolve a run id, accepting the full ULID or a unique short suffix (as shown
/// by `surge inbox`).
///
/// Guards: an empty or too-short suffix is rejected. The suffix is matched by
/// the database against every run, so a unique match is trustworthy — there is
/// no scan window a colliding run could hide beyond.
///
/// # Errors
/// Returns [`OperatorError::RunId`] for an unusable or non-unique id and
/// [`OperatorError::ListRuns`] if the registry cannot be listed.
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
    let matches = storage
        .find_run_ids_by_suffix(value, AMBIGUITY_CAP)
        .await
        .map_err(OperatorError::ListRuns)?;
    match matches.as_slice() {
        [one] => Ok(*one),
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
