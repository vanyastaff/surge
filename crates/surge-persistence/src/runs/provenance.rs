//! SQLite-only adapter for optional qualified semantic origins.
use surge_core::execution_recovery::semantic_position::InheritedProvenance;
pub(crate) fn read(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<InheritedProvenance>> {
    row.get::<_, Option<String>>(index)?
        .map(|json| {
            serde_json::from_str(&json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()
}
