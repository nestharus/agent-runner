use super::super::registry::TableRegistration;

/// Generates the per-table row_version trigger shape.
///
/// SQLite does not support assigning to `NEW.row_version` portably in a
/// `BEFORE` trigger, so the migration hook and this generator use guarded
/// `AFTER INSERT`/`AFTER UPDATE` triggers. The guards make old-writer inserts
/// initialize row_version and old-writer updates advance it without double
/// bumping explicit row_version updates.
pub fn generate_triggers_for_table(table: &TableRegistration) -> String {
    format!(
        "\
CREATE TRIGGER IF NOT EXISTS trg_{table}_row_version_insert
AFTER INSERT ON {table}
FOR EACH ROW
WHEN NEW.row_version = 0
BEGIN
  UPDATE {table}
     SET row_version = 1
   WHERE rowid = NEW.rowid;
END;

CREATE TRIGGER IF NOT EXISTS trg_{table}_row_version_update
AFTER UPDATE ON {table}
FOR EACH ROW
WHEN NEW.row_version = OLD.row_version
BEGIN
  UPDATE {table}
     SET row_version = OLD.row_version + 1
   WHERE rowid = OLD.rowid;
END;
",
        table = table.table
    )
}
