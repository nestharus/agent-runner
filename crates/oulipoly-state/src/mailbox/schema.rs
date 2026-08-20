//! Ordered schema evolution for the shared PID sidecar.
//! Each migration step names the bounded entity that owns its schema change;
//! fresh construction and installed-version upgrades are separate paths.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub(super) const CURRENT_VERSION: i64 = 4;
const SCHEMA_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const SCHEMA_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidecarEntity {
    NamespaceAuthority,
    MailboxDelivery,
    CompletionAuthority,
    RuntimeLifecycle,
    WakeAndSessionMetadata,
    PayloadRetention,
}

struct MigrationStep {
    target_version: i64,
    owner: SidecarEntity,
    apply: fn(&Connection) -> Result<(), String>,
}

const SCHEMA_STEPS: &[MigrationStep] = &[
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::NamespaceAuthority,
        apply: ensure_namespace_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_mailbox_delivery_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::CompletionAuthority,
        apply: ensure_completion_authority_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::RuntimeLifecycle,
        apply: ensure_runtime_lifecycle_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::WakeAndSessionMetadata,
        apply: ensure_wake_and_session_metadata_schema,
    },
    MigrationStep {
        target_version: 2,
        owner: SidecarEntity::PayloadRetention,
        apply: ensure_payload_retention_schema,
    },
    MigrationStep {
        target_version: 3,
        owner: SidecarEntity::WakeAndSessionMetadata,
        apply: ensure_wake_process_identity_schema,
    },
    MigrationStep {
        target_version: 4,
        owner: SidecarEntity::RuntimeLifecycle,
        apply: ensure_runtime_creator_identity_schema,
    },
];

pub(super) fn ensure(conn: &mut Connection) -> Result<(), String> {
    let stored_version = sidecar_version(conn)?;
    validate_supported_version(stored_version)?;
    if stored_version == CURRENT_VERSION {
        return Ok(());
    }

    let deadline = Instant::now() + SCHEMA_LOCK_TIMEOUT;
    loop {
        let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(error)
                if super::sqlite_error_is_contention(&error) && Instant::now() < deadline =>
            {
                std::thread::sleep(SCHEMA_LOCK_RETRY_INTERVAL);
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "Failed to lock PID mailbox sidecar schema migration: {error}"
                ));
            }
        };
        let locked_version = sidecar_version(&tx)?;
        validate_supported_version(locked_version)?;
        if locked_version == CURRENT_VERSION {
            return tx.commit().map_err(|err| {
                format!("Failed to finish PID mailbox sidecar schema check: {err}")
            });
        }

        if locked_version == 0 {
            create_fresh_schema(&tx)?;
        } else {
            upgrade_installed_schema(&tx, locked_version)?;
        }
        tx.pragma_update(None, "user_version", CURRENT_VERSION)
            .map_err(|err| format!("Failed to record PID mailbox sidecar schema version: {err}"))?;
        return tx.commit().map_err(|err| {
            format!("Failed to commit PID mailbox sidecar schema migration: {err}")
        });
    }
}

fn validate_supported_version(version: i64) -> Result<(), String> {
    if (0..=CURRENT_VERSION).contains(&version) {
        return Ok(());
    }
    Err(format!(
        "Unsupported PID mailbox sidecar schema version {version}; expected 0..={CURRENT_VERSION}"
    ))
}

fn create_fresh_schema(conn: &Connection) -> Result<(), String> {
    apply_steps(conn, SCHEMA_STEPS)
}

fn upgrade_installed_schema(conn: &Connection, stored_version: i64) -> Result<(), String> {
    for target_version in (stored_version + 1)..=CURRENT_VERSION {
        let steps = SCHEMA_STEPS
            .iter()
            .filter(|step| step.target_version == target_version)
            .collect::<Vec<_>>();
        if steps.is_empty() {
            return Err(format!(
                "PID mailbox migration manifest has no steps for version {target_version}"
            ));
        }
        for step in steps {
            (step.apply)(conn).map_err(|err| {
                format!(
                    "PID mailbox {:?} migration to version {} failed: {err}",
                    step.owner, step.target_version
                )
            })?;
        }
    }
    Ok(())
}

fn apply_steps(conn: &Connection, steps: &[MigrationStep]) -> Result<(), String> {
    for step in steps {
        (step.apply)(conn).map_err(|err| {
            format!(
                "PID mailbox {:?} fresh-schema step failed: {err}",
                step.owner
            )
        })?;
    }
    Ok(())
}

fn ensure_namespace_schema(conn: &Connection) -> Result<(), String> {
    crate::pid_identity::ensure_identity_schema(conn)?;
    conn.execute_batch(super::mailbox_schema_definition())
        .map_err(|err| format!("Failed to ensure sidecar entity tables: {err}"))?;
    super::ensure_mailbox_sidecar_identity_locked(conn)
}

fn ensure_mailbox_delivery_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_mailbox_columns(conn)?;
    super::ensure_mailbox_target_index(conn)?;
    super::ensure_mailbox_delivery_owner_index(conn)
}

fn ensure_completion_authority_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "INSERT OR IGNORE INTO completion_authority_materialization_summary (
            invocation_uuid, materialized_count, authority_ordinal,
            sidecar_generation, continuity_digest
         )
         SELECT head.invocation_uuid,
                (SELECT COUNT(*)
                 FROM completion_authority_continuity AS counted
                 WHERE counted.invocation_uuid = head.invocation_uuid),
                head.authority_ordinal, head.sidecar_generation, head.continuity_digest
         FROM completion_authority_continuity AS head
         WHERE head.authority_ordinal = (
            SELECT MAX(candidate.authority_ordinal)
            FROM completion_authority_continuity AS candidate
            WHERE candidate.invocation_uuid = head.invocation_uuid
         )
         AND (
            SELECT COUNT(*)
            FROM completion_authority_continuity AS counted
            WHERE counted.invocation_uuid = head.invocation_uuid
         ) = (
            SELECT COUNT(*)
            FROM completion_authority_continuity AS continuity
            JOIN completion_event AS event
              ON event.event_id = continuity.event_id
             AND event.kind = 'agent_bash_complete'
            JOIN completion_event_listener AS listener
              ON listener.event_id = continuity.event_id
             AND listener.listener_id = continuity.owner_invocation_uuid
             AND listener.owner_invocation_uuid = continuity.owner_invocation_uuid
             AND listener.session_id = continuity.owner_session_id
            WHERE continuity.invocation_uuid = head.invocation_uuid
         );",
    )
    .map_err(|err| format!("Failed to backfill completion materialization summary: {err}"))
}

fn ensure_runtime_lifecycle_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_runtime_generation_columns(conn)?;
    promote_legacy_runtime_authorities(conn)
}

fn ensure_wake_and_session_metadata_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_session_runtime_columns(conn)
}

fn ensure_wake_process_identity_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_wake_claim_process_identity_columns(conn)
}

fn ensure_runtime_creator_identity_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_runtime_generation_columns(conn)?;
    super::settle_unverifiable_runtime_generations(conn)
}

fn ensure_payload_retention_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_mailbox_compaction_index(conn)
}

#[derive(Debug)]
struct LegacyRuntimeAuthority {
    session_id: String,
    mode: String,
    invocation_uuid: String,
    provider_name: String,
    model_name: Option<String>,
    pty_control_path: Option<String>,
    updated_at: String,
    os_pid: i64,
    os_boot_id: String,
    os_pid_starttime_ticks: i64,
    turn_started_at: Option<String>,
    models_dir: Option<String>,
    effective_cwd: Option<String>,
}

fn promote_legacy_runtime_authorities(conn: &Connection) -> Result<(), String> {
    settle_incomplete_legacy_runtime_authorities(conn)?;
    let rows = read_complete_legacy_runtime_authorities(conn)?;
    for row in rows {
        promote_legacy_runtime_authority(conn, &row)?;
    }
    Ok(())
}

fn settle_incomplete_legacy_runtime_authorities(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "UPDATE session_runtime
         SET run_state = 'idle',
             pty_control_path = NULL,
             running_invocation_uuid = NULL,
             running_os_pid = NULL,
             running_os_boot_id = NULL,
             running_os_pid_starttime_ticks = NULL,
             turn_ended_at = COALESCE(turn_ended_at, updated_at)
         WHERE run_state = 'running'
           AND (running_invocation_uuid IS NULL
             OR running_os_pid IS NULL
             OR running_os_boot_id IS NULL
             OR running_os_pid_starttime_ticks IS NULL)",
        [],
    )
    .map_err(|err| format!("Failed to settle incomplete legacy runtime authority: {err}"))?;
    Ok(())
}

fn read_complete_legacy_runtime_authorities(
    conn: &Connection,
) -> Result<Vec<LegacyRuntimeAuthority>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT session_id, mode, running_invocation_uuid,
                    COALESCE(provider_name, 'legacy-unknown'), model_name,
                    pty_control_path, updated_at, running_os_pid,
                    running_os_boot_id, running_os_pid_starttime_ticks,
                    turn_started_at, models_dir, effective_cwd
             FROM session_runtime
             WHERE run_state = 'running'
               AND running_invocation_uuid IS NOT NULL
               AND running_os_pid IS NOT NULL
               AND running_os_boot_id IS NOT NULL
               AND running_os_pid_starttime_ticks IS NOT NULL",
        )
        .map_err(|err| format!("Failed to prepare legacy runtime promotion: {err}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(LegacyRuntimeAuthority {
                session_id: row.get(0)?,
                mode: row.get(1)?,
                invocation_uuid: row.get(2)?,
                provider_name: row.get(3)?,
                model_name: row.get(4)?,
                pty_control_path: row.get(5)?,
                updated_at: row.get(6)?,
                os_pid: row.get(7)?,
                os_boot_id: row.get(8)?,
                os_pid_starttime_ticks: row.get(9)?,
                turn_started_at: row.get(10)?,
                models_dir: row.get(11)?,
                effective_cwd: row.get(12)?,
            })
        })
        .map_err(|err| format!("Failed to query legacy runtime promotion rows: {err}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| format!("Failed to read legacy runtime promotion row: {err}"))
}

fn promote_legacy_runtime_authority(
    conn: &Connection,
    legacy: &LegacyRuntimeAuthority,
) -> Result<(), String> {
    if let Some((generation_uuid, generation_session_id, lifecycle_state, exit_code)) = conn
        .query_row(
            "SELECT generation_uuid, session_id, lifecycle_state, exit_code
             FROM runtime_generation
             WHERE identity_os_pid = ?1
               AND identity_os_boot_id = ?2
               AND identity_os_pid_starttime_ticks = ?3",
            params![
                legacy.os_pid,
                &legacy.os_boot_id,
                legacy.os_pid_starttime_ticks
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i32>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|err| format!("Failed to match promoted runtime identity: {err}"))?
    {
        attach_promoted_runtime_session(conn, &generation_uuid, generation_session_id, legacy)?;
        if lifecycle_state == "exited" {
            settle_legacy_runtime_projection(conn, legacy, exit_code)?;
        }
        return Ok(());
    }
    if session_has_nonterminal_generation(conn, &legacy.session_id)? {
        return Ok(());
    }

    let created_at = legacy
        .turn_started_at
        .as_deref()
        .unwrap_or(&legacy.updated_at);
    conn.execute(
        "INSERT INTO runtime_generation (
            generation_uuid, lifecycle_state, spawn_invocation_uuid, session_id,
            runtime_mode, provider_name, model_name, pty_control_path, models_dir,
            effective_cwd, spawned_os_pid, identity_os_pid, identity_os_boot_id,
            identity_os_pid_starttime_ticks, created_at, running_at
         ) VALUES (?1, 'running', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                   ?10, ?10, ?11, ?12, ?13, ?13)",
        params![
            Uuid::new_v4().to_string(),
            &legacy.invocation_uuid,
            &legacy.session_id,
            &legacy.mode,
            &legacy.provider_name,
            &legacy.model_name,
            &legacy.pty_control_path,
            &legacy.models_dir,
            &legacy.effective_cwd,
            legacy.os_pid,
            &legacy.os_boot_id,
            legacy.os_pid_starttime_ticks,
            created_at,
        ],
    )
    .map_err(|err| format!("Failed to promote legacy runtime authority: {err}"))?;
    Ok(())
}

fn settle_legacy_runtime_projection(
    conn: &Connection,
    legacy: &LegacyRuntimeAuthority,
    exit_code: Option<i32>,
) -> Result<(), String> {
    conn.execute(
        "UPDATE session_runtime
         SET run_state = 'idle',
             pty_control_path = NULL,
             running_invocation_uuid = NULL,
             running_os_pid = NULL,
             running_os_boot_id = NULL,
             running_os_pid_starttime_ticks = NULL,
             turn_ended_at = COALESCE(turn_ended_at, updated_at),
             last_exit_code = ?2
         WHERE session_id = ?1",
        params![&legacy.session_id, exit_code],
    )
    .map_err(|err| format!("Failed to settle promoted terminal runtime projection: {err}"))?;
    Ok(())
}

fn attach_promoted_runtime_session(
    conn: &Connection,
    generation_uuid: &str,
    generation_session_id: Option<String>,
    legacy: &LegacyRuntimeAuthority,
) -> Result<(), String> {
    match generation_session_id {
        Some(session_id) if session_id == legacy.session_id => Ok(()),
        Some(session_id) => Err(format!(
            "Legacy runtime identity for session {} already belongs to session {session_id}",
            legacy.session_id
        )),
        None => {
            conn.execute(
                "UPDATE runtime_generation SET session_id = ?2 WHERE generation_uuid = ?1",
                params![generation_uuid, &legacy.session_id],
            )
            .map_err(|err| format!("Failed to attach promoted runtime session: {err}"))?;
            Ok(())
        }
    }
}

fn session_has_nonterminal_generation(conn: &Connection, session_id: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM runtime_generation
            WHERE session_id = ?1 AND lifecycle_state != 'exited'
         )",
        params![session_id],
        |row| row.get(0),
    )
    .map_err(|err| format!("Failed to inspect promoted runtime session: {err}"))
}

fn sidecar_version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|err| format!("Failed to read PID mailbox sidecar schema version: {err}"))
}
