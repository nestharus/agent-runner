use super::hash::payload_hash_for_columns;
use crate::deployment::row_version::registry::TableRegistration;
use rusqlite::Row;
use rusqlite::types::Value;

pub fn payload_hash_for_row(
    table: &TableRegistration,
    row: &Row<'_>,
) -> rusqlite::Result<[u8; 32]> {
    let values = table
        .payload_columns
        .iter()
        .map(|column| row.get::<_, Value>(*column).map(Some))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(payload_hash_for_columns(&values))
}
