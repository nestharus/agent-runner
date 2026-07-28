use std::path::Path;

use crate::ScratchpadError;

pub(super) fn install_store_aliases(db_path: &Path) -> Result<(), ScratchpadError> {
    let conn = rusqlite::Connection::open(db_path).map_err(ScratchpadError::Database)?;
    conn.execute_batch(
        r#"
        CREATE VIEW IF NOT EXISTS artifacts AS
            SELECT * FROM artifact_versions;

        CREATE TRIGGER IF NOT EXISTS artifacts_update_created_at
        INSTEAD OF UPDATE OF created_at ON artifacts
        BEGIN
            UPDATE artifact_versions
               SET created_at = NEW.created_at
             WHERE workflow_run_id = OLD.workflow_run_id
               AND artifact_name = OLD.artifact_name
               AND version = OLD.version;
        END;
        "#,
    )
    .map_err(ScratchpadError::Database)?;
    Ok(())
}
