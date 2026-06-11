use super::{InvocationRecord, InvocationStatus, StateDb, sqlite};
use chrono::{DateTime, Utc};

impl StateDb {
    pub fn record_legacy_resume_input_session_id(
        &self,
        id: i64,
        resume_input_id: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE invocations
                 SET session_id = ?1
                 WHERE id = ?2 AND session_capture_method = 'resumed'",
                sqlite::params![resume_input_id, id],
            )
            .map_err(|e| {
                format!("Failed to update legacy resume session_id for invocation {id}: {e}")
            })?;
        Ok(())
    }

    pub fn update_resume_acceptance(
        &self,
        id: i64,
        status: &str,
        evidence: Option<&str>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE invocations
                 SET resume_acceptance_status = ?1,
                     resume_acceptance_evidence = ?2
                 WHERE id = ?3",
                sqlite::params![status, evidence, id],
            )
            .map_err(|e| format!("Failed to update resume acceptance for invocation {id}: {e}"))?;
        Ok(())
    }

    pub fn get_invocation_by_uuid(&self, uuid: &str) -> Result<Option<InvocationRecord>, String> {
        let sql = Self::invocation_record_select_sql(&self.conn, "WHERE invocation_uuid = ?1")?;
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare invocation lookup: {e}"))?;

        let result = stmt.query_row(sqlite::params![uuid], Self::map_invocation_row);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(sqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to query invocation: {e}")),
        }
    }

    pub fn list_invocation_children(
        &self,
        parent_id: i64,
    ) -> Result<Vec<InvocationRecord>, String> {
        let sql = Self::invocation_record_select_sql(
            &self.conn,
            "WHERE parent_invocation_id = ?1
             ORDER BY created_at, id",
        )?;
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| format!("Failed to prepare invocation child lookup: {e}"))?;

        let rows = stmt
            .query_map(sqlite::params![parent_id], Self::map_invocation_row)
            .map_err(|e| format!("Failed to query invocation children: {e}"))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to map invocation children: {e}"))
    }

    fn map_invocation_row(row: &sqlite::Row<'_>) -> sqlite::Result<InvocationRecord> {
        let created_at_raw: String = row.get(19)?;
        let finished_at_raw: Option<String> = row.get(20)?;
        let status_raw: String = row.get(6)?;
        let created_at = Self::strict_rfc3339_at(&created_at_raw, 18)?;
        let finished_at = Self::optional_strict_rfc3339_at(finished_at_raw, 19)?;
        let status = Self::parse_invocation_status_at(&status_raw, 6)?;

        Ok(InvocationRecord {
            id: row.get(0)?,
            invocation_uuid: row.get(1)?,
            model_name: row.get(2)?,
            provider_name: row.get(3)?,
            provider_index: row.get::<_, i64>(4)? as usize,
            parent_invocation_id: row.get(5)?,
            status,
            success: row.get::<_, Option<i64>>(7)?.map(|value| value != 0),
            exit_code: row.get(8)?,
            error_category: row.get(9)?,
            terminal_reason: row.get(10)?,
            session_id: row.get(11)?,
            session_capture_method: row.get(12)?,
            provider_session_id: row.get(13)?,
            resume_input_id: row.get(14)?,
            provider_session_capture_method: row.get(15)?,
            provider_session_resolved_account: row.get(16)?,
            resume_acceptance_status: row.get(17)?,
            resume_acceptance_evidence: row.get(18)?,
            created_at,
            finished_at,
        })
    }

    fn optional_strict_rfc3339_at(
        raw: Option<String>,
        column_index: usize,
    ) -> sqlite::Result<Option<DateTime<Utc>>> {
        raw.map(|s| Self::strict_rfc3339_at(&s, column_index))
            .transpose()
    }

    fn parse_invocation_status_at(
        raw: &str,
        column_index: usize,
    ) -> sqlite::Result<InvocationStatus> {
        raw.parse::<InvocationStatus>().map_err(|_| {
            sqlite::Error::FromSqlConversionFailure(
                column_index,
                sqlite::Type::Text,
                format!("Unknown invocation status: {raw}").into(),
            )
        })
    }
}
