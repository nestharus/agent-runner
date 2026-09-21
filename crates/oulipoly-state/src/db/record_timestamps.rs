//! Explicit, historical-only repair path for established terminal timestamps.
//!
//! Ordinary lifecycle writers are protected by schema triggers. A repair is a
//! separate transaction that first appends its durable audit authorization and
//! then changes the timestamp and retention projection atomically.

use super::*;
use crate::live_history::STATE_TIMESTAMP_REPAIR;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTimestampRecordFamily {
    Invocation,
    ProviderLogicalLaunch,
    ProviderLaunchAttempt,
    CompletedTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateTerminalTimestampRepair<'a> {
    pub family: StateTimestampRecordFamily,
    pub record_key: &'a str,
    pub new_timestamp: &'a str,
    pub actor: &'a str,
    pub reason: &'a str,
}

struct RepairTarget {
    family: &'static str,
    table: &'static str,
    key_column: &'static str,
    terminal_column: &'static str,
    terminal_predicate: &'static str,
}

impl StateTimestampRecordFamily {
    fn target(self) -> RepairTarget {
        match self {
            Self::Invocation => RepairTarget {
                family: "invocation",
                table: "invocations",
                key_column: "invocation_uuid",
                terminal_column: "finished_at",
                terminal_predicate: "status IN ('succeeded','failed')",
            },
            Self::ProviderLogicalLaunch => RepairTarget {
                family: "provider_logical_launch",
                table: "provider_logical_launches",
                key_column: "logical_launch_id",
                terminal_column: "finished_at",
                terminal_predicate: "status IN ('succeeded','failed','cancelled')",
            },
            Self::ProviderLaunchAttempt => RepairTarget {
                family: "provider_launch_attempt",
                table: "provider_launch_attempts",
                key_column: "attempt_id",
                terminal_column: "finished_at",
                terminal_predicate: "status IN ('superseded','succeeded','failed','cancelled')",
            },
            Self::CompletedTurn => RepairTarget {
                family: "completed_turn",
                table: "completed_turns",
                key_column: "invocation_uuid",
                terminal_column: "closed_at",
                terminal_predicate: "recovery_pending=0",
            },
        }
    }
}

impl StateDb {
    pub fn repair_terminal_timestamp(
        &mut self,
        request: StateTerminalTimestampRepair<'_>,
    ) -> Result<String, String> {
        self.access_scope.authorize(STATE_TIMESTAMP_REPAIR, None)?;
        if request.record_key.is_empty() {
            return Err("record timestamp repair key must not be empty".into());
        }
        if request.actor.trim().is_empty() || request.reason.trim().is_empty() {
            return Err("record timestamp repair requires actor and reason".into());
        }
        DateTime::parse_from_rfc3339(request.new_timestamp)
            .map_err(|_| "record timestamp repair requires an RFC3339 timestamp".to_string())?;

        let target = request.family.target();
        let tx = self
            .conn
            .transaction_with_behavior(sqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("Failed to start record timestamp repair: {error}"))?;
        let lookup = format!(
            "SELECT {terminal} FROM {table} WHERE {key}=?1 AND {predicate}",
            terminal = target.terminal_column,
            table = target.table,
            key = target.key_column,
            predicate = target.terminal_predicate,
        );
        let old_value = tx
            .query_row(&lookup, [request.record_key], |row| {
                row.get::<_, Option<String>>(0)
            })
            .optional()
            .map_err(|error| format!("Failed to read terminal timestamp for repair: {error}"))?
            .flatten()
            .ok_or_else(|| {
                "record timestamp repair requires an established terminal record".to_string()
            })?;
        if old_value == request.new_timestamp {
            return Err("record timestamp repair must change the terminal timestamp".into());
        }

        let repair_id = Uuid::new_v4().to_string();
        let repaired_at = Self::current_rfc3339_timestamp();
        tx.execute(
            "INSERT INTO record_timestamp_repairs(
                repair_id,record_family,record_key,field_name,old_value,new_value,
                actor,reason,repaired_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            sqlite::params![
                repair_id,
                target.family,
                request.record_key,
                target.terminal_column,
                old_value,
                request.new_timestamp,
                request.actor,
                request.reason,
                repaired_at,
            ],
        )
        .map_err(|error| format!("Failed to append record timestamp repair audit: {error}"))?;
        let update = format!(
            "UPDATE {table}
             SET {terminal}=?1,
                 retention_eligible_at=CASE
                   WHEN created_at IS NOT NULL AND julianday(?1)>=julianday(created_at)
                   THEN ?1 ELSE NULL END,
                 retention_status=CASE
                   WHEN created_at IS NULL OR julianday(?1) IS NULL OR julianday(created_at) IS NULL
                   THEN 'legacy_unknown'
                   WHEN julianday(?1)<julianday(created_at) THEN 'clock_anomaly'
                   ELSE 'eligible' END
             WHERE {key}=?2 AND {predicate}",
            table = target.table,
            terminal = target.terminal_column,
            key = target.key_column,
            predicate = target.terminal_predicate,
        );
        let changed = tx
            .execute(
                &update,
                sqlite::params![request.new_timestamp, request.record_key],
            )
            .map_err(|error| format!("Failed to apply record timestamp repair: {error}"))?;
        if changed != 1 {
            return Err("record timestamp repair target changed concurrently".into());
        }
        tx.commit()
            .map_err(|error| format!("Failed to commit record timestamp repair: {error}"))?;
        Ok(repair_id)
    }
}
