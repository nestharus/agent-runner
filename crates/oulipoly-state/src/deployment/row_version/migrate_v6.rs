use crate::deployment::row_version::registry::REGISTRY;
use crate::deployment::row_version::triggers_sql::apply::generate_all_triggers;
use rusqlite::Connection;

pub fn apply_v6_row_version(conn: &Connection) -> Result<(), rusqlite::Error> {
    for entry in REGISTRY {
        // 0006 creates this table with row_version already present; the Rust
        // hook only upgrades tracked tables that existed before schema v6.
        if entry.table == "invocation_returned_artifacts" {
            continue;
        }
        if column_exists(conn, entry.table, "row_version")? {
            continue;
        }
        conn.execute_batch(&format!(
            "ALTER TABLE {} ADD COLUMN row_version INTEGER NOT NULL DEFAULT 0;",
            entry.table
        ))?;
    }

    conn.execute_batch(&generate_all_triggers())?;
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for name in columns {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}
