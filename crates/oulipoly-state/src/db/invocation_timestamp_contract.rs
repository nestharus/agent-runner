//! Idempotent installation of the invocation timestamp and retention contract.
//!
//! Numbered migration 27 and current-schema repair/rebuild routes share this
//! installer so every writable current database has the same projection,
//! immutability, audited-repair, and retention-index guarantees.

use super::*;

const INSTALL_SAVEPOINT: &str = "age371_invocation_timestamp_contract";

impl StateDb {
    pub(crate) fn install_invocation_timestamp_contract(
        conn: &sqlite::Connection,
    ) -> sqlite::Result<()> {
        conn.execute_batch(&format!("SAVEPOINT {INSTALL_SAVEPOINT};"))?;
        match Self::install_invocation_timestamp_contract_inner(conn) {
            Ok(()) => conn.execute_batch(&format!("RELEASE {INSTALL_SAVEPOINT};")),
            Err(error) => {
                let _ = conn.execute_batch(&format!(
                    "ROLLBACK TO {INSTALL_SAVEPOINT}; RELEASE {INSTALL_SAVEPOINT};"
                ));
                Err(error)
            }
        }
    }

    fn install_invocation_timestamp_contract_inner(
        conn: &sqlite::Connection,
    ) -> sqlite::Result<()> {
        let columns = Self::invocation_timestamp_columns(conn)?;
        let lifecycle_was_missing = !columns
            .iter()
            .any(|column| column == "lifecycle_updated_at");
        let eligibility_was_missing = !columns
            .iter()
            .any(|column| column == "retention_eligible_at");
        let status_was_missing = !columns.iter().any(|column| column == "retention_status");

        if lifecycle_was_missing {
            conn.execute_batch("ALTER TABLE invocations ADD COLUMN lifecycle_updated_at TEXT;")?;
        }
        if eligibility_was_missing {
            conn.execute_batch("ALTER TABLE invocations ADD COLUMN retention_eligible_at TEXT;")?;
        }
        if status_was_missing {
            conn.execute_batch(
                "ALTER TABLE invocations ADD COLUMN retention_status TEXT NOT NULL
                 DEFAULT 'legacy_unknown' CHECK (retention_status IN
                 ('pending', 'eligible', 'legacy_unknown', 'clock_anomaly'));",
            )?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS record_timestamp_repairs (
                repair_id TEXT PRIMARY KEY,
                record_family TEXT NOT NULL CHECK (record_family IN (
                    'invocation',
                    'provider_logical_launch',
                    'provider_launch_attempt',
                    'completed_turn'
                )),
                record_key TEXT NOT NULL,
                field_name TEXT NOT NULL CHECK (field_name IN ('finished_at', 'closed_at')),
                old_value TEXT,
                new_value TEXT NOT NULL,
                actor TEXT NOT NULL CHECK (trim(actor) <> ''),
                reason TEXT NOT NULL CHECK (trim(reason) <> ''),
                repaired_at TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_record_timestamp_repairs_record
                ON record_timestamp_repairs(record_family, record_key, repaired_at);
             CREATE TRIGGER IF NOT EXISTS record_timestamp_repairs_append_only_update
             BEFORE UPDATE ON record_timestamp_repairs
             BEGIN
                SELECT RAISE(ABORT, 'record timestamp repair audit is append-only');
             END;
             CREATE TRIGGER IF NOT EXISTS record_timestamp_repairs_append_only_delete
             BEFORE DELETE ON record_timestamp_repairs
             BEGIN
                SELECT RAISE(ABORT, 'record timestamp repair audit is append-only');
             END;",
        )?;

        if lifecycle_was_missing {
            conn.execute_batch(
                "UPDATE invocations
                 SET lifecycle_updated_at = CASE
                    WHEN status = 'running' THEN created_at ELSE finished_at END;",
            )?;
        }
        if status_was_missing {
            conn.execute_batch(
                "UPDATE invocations
                 SET retention_status = CASE
                    WHEN status = 'running' THEN 'pending'
                    WHEN status IN ('succeeded', 'failed')
                     AND finished_at IS NOT NULL
                     AND finished_at <> created_at
                     AND julianday(finished_at) < julianday(created_at)
                    THEN 'clock_anomaly'
                    WHEN status IN ('succeeded', 'failed')
                     AND finished_at IS NOT NULL
                     AND finished_at <> created_at
                     AND julianday(finished_at) >= julianday(created_at)
                    THEN 'eligible'
                    ELSE 'legacy_unknown' END;",
            )?;
        }
        if status_was_missing || eligibility_was_missing {
            conn.execute_batch(
                "UPDATE invocations
                 SET retention_eligible_at = CASE
                    WHEN retention_status = 'eligible'
                     AND status IN ('succeeded', 'failed')
                     AND finished_at IS NOT NULL
                     AND finished_at <> created_at
                     AND julianday(finished_at) >= julianday(created_at)
                    THEN finished_at ELSE NULL END;",
            )?;
        }

        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_invocations_retention_eligible
                ON invocations(retention_eligible_at, id)
                WHERE retention_status = 'eligible' AND retention_eligible_at IS NOT NULL;

             CREATE TRIGGER IF NOT EXISTS invocations_timestamp_after_insert
             AFTER INSERT ON invocations
             BEGIN
                UPDATE invocations
                SET lifecycle_updated_at = CASE
                        WHEN NEW.status = 'running' THEN NEW.created_at ELSE NEW.finished_at END,
                    retention_eligible_at = CASE
                        WHEN NEW.status IN ('succeeded', 'failed')
                         AND NEW.finished_at IS NOT NULL
                         AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
                        THEN NEW.finished_at ELSE NULL END,
                    retention_status = CASE
                        WHEN NEW.status = 'running' THEN 'pending'
                        WHEN NEW.status IN ('succeeded', 'failed')
                         AND NEW.finished_at IS NOT NULL
                         AND julianday(NEW.finished_at) < julianday(NEW.created_at)
                        THEN 'clock_anomaly'
                        WHEN NEW.status IN ('succeeded', 'failed')
                         AND NEW.finished_at IS NOT NULL
                         AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
                        THEN 'eligible'
                        ELSE 'legacy_unknown' END
                WHERE id = NEW.id;
             END;

             CREATE TRIGGER IF NOT EXISTS invocations_timestamp_after_terminal
             AFTER UPDATE OF status, finished_at ON invocations
             WHEN NEW.status IN ('succeeded', 'failed', 'legacy')
              AND (OLD.status IS NOT NEW.status OR OLD.finished_at IS NOT NEW.finished_at)
             BEGIN
                UPDATE invocations
                SET lifecycle_updated_at = NEW.finished_at,
                    retention_eligible_at = CASE
                        WHEN NEW.status IN ('succeeded', 'failed')
                         AND NEW.finished_at IS NOT NULL
                         AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
                        THEN NEW.finished_at ELSE NULL END,
                    retention_status = CASE
                        WHEN NEW.status IN ('succeeded', 'failed')
                         AND NEW.finished_at IS NOT NULL
                         AND julianday(NEW.finished_at) < julianday(NEW.created_at)
                        THEN 'clock_anomaly'
                        WHEN NEW.status IN ('succeeded', 'failed')
                         AND NEW.finished_at IS NOT NULL
                         AND julianday(NEW.finished_at) >= julianday(NEW.created_at)
                        THEN 'eligible'
                        ELSE 'legacy_unknown' END
                WHERE id = NEW.id;
             END;

             CREATE TRIGGER IF NOT EXISTS invocations_created_at_immutable
             BEFORE UPDATE OF created_at ON invocations
             WHEN NEW.created_at IS NOT OLD.created_at
             BEGIN
                SELECT RAISE(ABORT, 'invocation creation timestamp is immutable');
             END;

             CREATE TRIGGER IF NOT EXISTS invocations_finished_at_immutable
             BEFORE UPDATE OF finished_at ON invocations
             WHEN OLD.finished_at IS NOT NULL
              AND NEW.finished_at IS NOT OLD.finished_at
              AND NOT EXISTS (
                  SELECT 1 FROM record_timestamp_repairs repair
                  WHERE repair.record_family = 'invocation'
                    AND repair.record_key = OLD.invocation_uuid
                    AND repair.field_name = 'finished_at'
                    AND repair.old_value IS OLD.finished_at
                    AND repair.new_value = NEW.finished_at
                    AND NOT EXISTS (
                        SELECT 1 FROM record_timestamp_repairs newer
                        WHERE newer.record_family = repair.record_family
                          AND newer.record_key = repair.record_key
                          AND newer.field_name = repair.field_name
                          AND newer.rowid > repair.rowid
                    )
              )
             BEGIN
                SELECT RAISE(ABORT, 'invocation terminal timestamp is immutable');
             END;

             CREATE TRIGGER IF NOT EXISTS invocations_terminal_reopen_forbidden
             BEFORE UPDATE OF status ON invocations
             WHEN OLD.status IN ('succeeded', 'failed', 'legacy')
              AND NEW.status IS NOT OLD.status
             BEGIN
                SELECT RAISE(ABORT, 'invocation terminal state cannot reopen');
             END;",
        )
    }

    fn invocation_timestamp_columns(conn: &sqlite::Connection) -> sqlite::Result<Vec<String>> {
        let mut statement = conn.prepare("PRAGMA table_info(invocations)")?;
        statement
            .query_map([], |row| row.get(1))?
            .collect::<sqlite::Result<Vec<_>>>()
    }
}
