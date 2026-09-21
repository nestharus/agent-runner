//! Typed live-versus-historical SQLite access barrier.
//!
//! Callers select a repository scope explicitly. Statement classifications are
//! static Rust values and production authorization never parses SQL text.

use crate::diagnostic_recorder::{
    DiagnosticPhase, LiveHistoryBarrierEvidence, PhaseObservation, SpanStart, SqliteAccessClass,
    SqliteDatabaseRole, SqliteEventIdentity, SqlitePathClass, process_recorder,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatementAccess {
    pub query_family: &'static str,
    pub class: SqliteAccessClass,
    pub requires_bound: bool,
}

impl StatementAccess {
    pub const fn live(query_family: &'static str) -> Self {
        Self {
            query_family,
            class: SqliteAccessClass::LiveAuthority,
            requires_bound: false,
        }
    }

    pub const fn bounded_cross_boundary(query_family: &'static str) -> Self {
        Self {
            query_family,
            class: SqliteAccessClass::BoundedCrossBoundary,
            requires_bound: true,
        }
    }

    pub const fn historical(query_family: &'static str) -> Self {
        Self {
            query_family,
            class: SqliteAccessClass::HistoricalDiagnostic,
            requires_bound: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessScope {
    Live {
        trace: &'static str,
        database_role: SqliteDatabaseRole,
    },
    Historical,
}

impl AccessScope {
    pub const fn live(trace: &'static str, database_role: SqliteDatabaseRole) -> Self {
        Self::Live {
            trace,
            database_role,
        }
    }

    pub const fn historical() -> Self {
        Self::Historical
    }

    pub fn authorize(self, statement: StatementAccess, bound: Option<usize>) -> Result<(), String> {
        if statement.requires_bound && bound.is_none_or(|bound| bound == 0) {
            return Err(format!(
                "live_history_barrier: {} requires a positive explicit bound",
                statement.query_family
            ));
        }
        let Self::Live {
            trace,
            database_role,
        } = self
        else {
            return Ok(());
        };
        if statement.class != SqliteAccessClass::HistoricalDiagnostic {
            return Ok(());
        }
        record_blocked_historical_access(trace, database_role, statement);
        Err(format!(
            "live_history_barrier: live trace {trace} cannot access historical query family {}",
            statement.query_family
        ))
    }
}

fn record_blocked_historical_access(
    live_trace: &'static str,
    database_role: SqliteDatabaseRole,
    statement: StatementAccess,
) {
    let start = SpanStart::new("live_history_barrier", "sqlite_access")
        .with_lifecycle_phase(live_trace)
        .with_sqlite_identity(
            SqliteEventIdentity::new(
                database_role,
                SqlitePathClass::ManagedFile,
                statement.query_family,
            )
            .with_access_class(statement.class),
        );
    process_recorder().with_requested_span(start, |span| {
        let _ = span.record(
            DiagnosticPhase::Failed,
            PhaseObservation::not_started()
                .with_live_history_barrier(LiveHistoryBarrierEvidence::blocked(
                    live_trace,
                    statement.class,
                    statement.query_family,
                ))
                .with_cause("historical_access_from_live_trace"),
        );
    });
}

pub(crate) const STATE_LIVE_SUBTREE_CHILDREN: StatementAccess =
    StatementAccess::bounded_cross_boundary("invocations.live_subtree_children");
pub(crate) const MAILBOX_PENDING_DELIVERY: StatementAccess =
    StatementAccess::bounded_cross_boundary("mailbox.pending_delivery");
pub(crate) const MAILBOX_PENDING_SESSIONS: StatementAccess =
    StatementAccess::bounded_cross_boundary("mailbox.pending_sessions");
pub(crate) const MAILBOX_PENDING_EXACT: StatementAccess =
    StatementAccess::live("mailbox.pending_exact");
pub(crate) const MAILBOX_PENDING_COUNT: StatementAccess =
    StatementAccess::live("mailbox.pending_count");
pub(crate) const MAILBOX_FULL_HISTORY: StatementAccess =
    StatementAccess::historical("mailbox.full_listing");
pub(crate) const STATE_FULL_ADMISSION_LEDGER: StatementAccess =
    StatementAccess::historical("completion_continuation.full_admission_ledger");
pub(crate) const SUPERVISOR_PENDING_ATTEMPTS: StatementAccess =
    StatementAccess::bounded_cross_boundary("completion_continuation.pending_for_supervisor");
pub(crate) const TERMINAL_RETENTION_STATS: StatementAccess =
    StatementAccess::historical("terminal_history.retention_stats");
pub(crate) const TERMINAL_RETENTION_PRUNE: StatementAccess =
    StatementAccess::historical("terminal_history.prune");
pub(crate) const TERMINAL_RETENTION_VACUUM: StatementAccess =
    StatementAccess::historical("terminal_history.vacuum");
pub(crate) const DELIVERED_PAYLOAD_COMPACTION_STATS: StatementAccess =
    StatementAccess::historical("terminal_history.payload_compaction_stats");
pub(crate) const DELIVERED_PAYLOAD_COMPACTION: StatementAccess =
    StatementAccess::historical("terminal_history.payload_compaction");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryKind {
    Table,
    Index,
    Statement,
    Maintenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryEntry {
    pub id: &'static str,
    pub kind: InventoryKind,
    pub class: SqliteAccessClass,
}

/// Machine-readable counterpart to `docs/architecture/live-history-access-inventory.md`.
pub const ACCESS_INVENTORY: &[InventoryEntry] = &[
    InventoryEntry {
        id: "table.invocations",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.provider_logical_launches",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.provider_launch_attempts",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.provider_launch_transition_replays",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "table.provider_launch_native_channel_duties",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "table.completed_turns",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.invocation_completion_obligations",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.invocation_completion_continuity",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.completion_authority_continuity",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.invocation_completion_v2_identity",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.mailbox",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.mailbox_delivery_attempts",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.mailbox_delivery_attempt_items",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.completion_event",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.completion_event_listener",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.runtime_generation",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.session_wake_claim",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "table.completion_continuation_owner",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "table.completion_continuation_source",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.completion_continuation_attempt",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.completion_continuation_notification",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "table.completion_continuation_domain",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "table.completion_supervisor_authority",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "table.completion_supervisor_inheritance",
        kind: InventoryKind::Table,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_invocations_running_parent",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_invocations_parent_running_created",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "index.provider_launch_nonterminal",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "index.provider_launch_cancelling",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.provider_launch_native_channel_duty_domain",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.completed_turns_recovery_pending",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_invocation_completion_obligations_legacy",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_invocation_completion_obligations_event",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "index.idx_invocation_completion_continuity_head",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_completion_authority_continuity_head",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_invocation_completion_v2_registration",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "index.idx_invocation_completion_v2_source",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "index.idx_invocation_completion_v2_handle",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "index.idx_mailbox_pending",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "index.idx_mailbox_pending_target",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "index.idx_mailbox_pending_session_live",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "index.idx_mailbox_pending_target_live",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "index.idx_mailbox_deliverable_session_live",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_mailbox_deliverable_target_live",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_mailbox_deliverable_global",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_mailbox_delivery_attempt_unresolved",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_completion_event_listener_session_live",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_completion_event_listener_retirement_pending",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_runtime_generation_live_session",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.completion_continuation_owner_running",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.completion_continuation_attempt_unresolved",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.completion_continuation_attempt_native_runtime",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "index.completion_continuation_source_unaccepted",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "index.idx_mailbox_terminal_retention",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "index.idx_mailbox_delivery_attempt_terminal_retention",
        kind: InventoryKind::Index,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "statement.invocations.live_subtree_children",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "statement.mailbox.pending_delivery",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "statement.mailbox.pending_sessions",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "statement.mailbox.pending_exact",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.mailbox.pending_count",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.mailbox.full_listing",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "statement.mailbox.resolve_unresolved_delivery_attempts",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.completed_turns.recovery_pending",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.completion_continuation.exact_admission",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.completion_continuation.continuity_suffix",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "statement.completion_continuation.legacy_probe",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.provider_launch.native_channel_duty_for_domain",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.supervisor.close_idle_live_obligations",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::LiveAuthority,
    },
    InventoryEntry {
        id: "statement.completion_continuation.pending_for_supervisor",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "statement.completion_continuation.native_runtime_identity",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::BoundedCrossBoundary,
    },
    InventoryEntry {
        id: "maintenance.terminal_history.retention_stats",
        kind: InventoryKind::Maintenance,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "maintenance.terminal_history.payload_compaction",
        kind: InventoryKind::Maintenance,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "maintenance.terminal_history.payload_compaction_stats",
        kind: InventoryKind::Maintenance,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "statement.completion_continuation.full_admission_ledger",
        kind: InventoryKind::Statement,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "maintenance.terminal_history.prune",
        kind: InventoryKind::Maintenance,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
    InventoryEntry {
        id: "maintenance.terminal_history.vacuum",
        kind: InventoryKind::Maintenance,
        class: SqliteAccessClass::HistoricalDiagnostic,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn inventory_ids_are_unique_and_documented() {
        let document = include_str!("../../../docs/architecture/live-history-access-inventory.md");
        let mut ids = HashSet::new();
        for entry in ACCESS_INVENTORY {
            assert!(ids.insert(entry.id), "duplicate inventory id: {}", entry.id);
            assert!(
                document.contains(&format!("`{}`", entry.id)),
                "inventory document is missing {}",
                entry.id
            );
        }
    }

    #[test]
    fn cross_boundary_access_requires_a_positive_bound() {
        let live = AccessScope::live("fixture.live", SqliteDatabaseRole::PidMailbox);
        assert!(live.authorize(MAILBOX_PENDING_DELIVERY, None).is_err());
        assert!(live.authorize(MAILBOX_PENDING_DELIVERY, Some(0)).is_err());
        assert!(live.authorize(MAILBOX_PENDING_DELIVERY, Some(1)).is_ok());
    }
}
