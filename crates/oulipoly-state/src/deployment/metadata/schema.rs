pub const COORDINATOR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SchemaError {
    Sql(String),
}

impl From<rusqlite::Error> for SchemaError {
    fn from(err: rusqlite::Error) -> Self {
        SchemaError::Sql(err.to_string())
    }
}

pub fn ensure_coordinator_schema(conn: &mut rusqlite::Connection) -> Result<(), SchemaError> {
    let tx = conn.transaction()?;

    tx.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS deployments (
            deployment_id BLOB PRIMARY KEY NOT NULL,
            from_schema_version INTEGER NOT NULL,
            to_schema_version INTEGER NOT NULL,
            phase TEXT NOT NULL,
            started_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            notes TEXT
        );

        CREATE TABLE IF NOT EXISTS primary_pointer (
            pointer_id INTEGER PRIMARY KEY CHECK (pointer_id = 1),
            schema_version INTEGER NOT NULL,
            deployment_id BLOB,
            role TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS queue_state (
            deployment_id BLOB PRIMARY KEY NOT NULL,
            direction TEXT NOT NULL,
            activation_state TEXT NOT NULL,
            last_sequence INTEGER NOT NULL DEFAULT 0,
            last_acked_sequence INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS import_watermarks (
            deployment_id BLOB NOT NULL,
            table_name TEXT NOT NULL,
            last_pk_json TEXT,
            last_seen_row_version INTEGER NOT NULL DEFAULT 0,
            completed_pass INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (deployment_id, table_name)
        );

        CREATE TABLE IF NOT EXISTS retention_state (
            deployment_id BLOB PRIMARY KEY NOT NULL,
            retention_started_at TEXT,
            retention_completed_at TEXT,
            reverse_dual_write_active INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS hook_outbox (
            hook_id BLOB PRIMARY KEY NOT NULL,
            deployment_id BLOB,
            hook_kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            enqueued_at TEXT NOT NULL,
            delivered_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_hook_outbox_undelivered
            ON hook_outbox(delivered_at) WHERE delivered_at IS NULL;
        PRAGMA user_version = 1;
        ",
    )?;

    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // component_slug: deployment-metadata-schema
    #[test]
    fn coordinator_schema_creation_is_idempotent_and_sets_user_version() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();

        ensure_coordinator_schema(&mut conn).expect("first schema creation succeeds");
        ensure_coordinator_schema(&mut conn).expect("second schema creation is idempotent");

        let user_version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(user_version, COORDINATOR_SCHEMA_VERSION);
    }
}
