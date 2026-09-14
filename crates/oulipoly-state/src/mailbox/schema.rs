//! Ordered schema evolution for the shared PID sidecar.
//! Each migration step names the bounded entity that owns its schema change;
//! fresh construction and installed-version upgrades are separate paths.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub(super) const CURRENT_VERSION: i64 = 19;
const MAX_SUPPORTED_VERSION: i64 = CURRENT_VERSION;
const SCHEMA_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const SCHEMA_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidecarEntity {
    NamespaceAuthority,
    MailboxDelivery,
    CompletionAuthority,
    RuntimeLifecycle,
    SessionAdmission,
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
    MigrationStep {
        target_version: 5,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_mailbox_delivery_settlement_schema,
    },
    MigrationStep {
        target_version: 6,
        owner: SidecarEntity::SessionAdmission,
        apply: ensure_session_admission_schema,
    },
    MigrationStep {
        target_version: 7,
        owner: SidecarEntity::SessionAdmission,
        apply: ensure_session_admission_launcher_identity_schema,
    },
    MigrationStep {
        target_version: 8,
        owner: SidecarEntity::WakeAndSessionMetadata,
        apply: ensure_wake_sweep_progress_schema,
    },
    MigrationStep {
        target_version: 9,
        owner: SidecarEntity::SessionAdmission,
        apply: ensure_session_admission_scaling_indexes,
    },
    MigrationStep {
        target_version: 10,
        owner: SidecarEntity::PayloadRetention,
        apply: ensure_terminal_history_retention_schema,
    },
    MigrationStep {
        target_version: 11,
        owner: SidecarEntity::PayloadRetention,
        apply: ensure_terminal_payload_lookup_indexes,
    },
    MigrationStep {
        target_version: 12,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_mailbox_delivery_settlement_schema,
    },
    MigrationStep {
        target_version: 13,
        owner: SidecarEntity::MailboxDelivery,
        apply: migrate_headless_observation_fence,
    },
    MigrationStep {
        target_version: 14,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_observation_stop_schema,
    },
    MigrationStep {
        target_version: 15,
        owner: SidecarEntity::MailboxDelivery,
        apply: ensure_delivery_finalization_schema,
    },
    MigrationStep {
        target_version: 16,
        owner: SidecarEntity::MailboxDelivery,
        apply: migrate_receipt_scan,
    },
    MigrationStep {
        target_version: 17,
        owner: SidecarEntity::RuntimeLifecycle,
        apply: migrate_starting_custody,
    },
    MigrationStep {
        target_version: 18,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_completion_continuation,
    },
    MigrationStep {
        target_version: 19,
        owner: SidecarEntity::CompletionAuthority,
        apply: migrate_notification_settlement,
    },
];

fn migrate_notification_settlement(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0019_notification_settlement.sql"))
        .map_err(|error| error.to_string())
}

fn migrate_completion_continuation(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0018_completion_continuation.sql"))
        .map_err(|error| error.to_string())?;
    // Schema, identity and version commit together. Legacy namespace identity,
    // admissions and pending work are not converted into v2 recovery authority.
    conn.execute(
        "INSERT INTO completion_continuation_domain VALUES(1,?1,'main-native-completion-v2')",
        [Uuid::new_v4().to_string()],
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn migrate_starting_custody(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("migrations/0017_starting_custody.sql"))
        .map_err(|error| error.to_string())
}

// Version and fingerprint are one WAL read snapshot. The caller retains the
// namespace fence throughout; a committed current schema needs no SQLite writer.
fn observe_valid_current(conn: &Connection) -> Result<bool, String> {
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("Failed to observe sidecar schema: {e}"))?;
    let version = sidecar_version(&tx)?;
    validate_supported_version(version)?;
    if version == MAX_SUPPORTED_VERSION {
        super::completion_continuation::validate_schema_on(&tx)?;
    }
    tx.commit()
        .map_err(|e| format!("Failed to finish sidecar schema observation: {e}"))?;
    Ok(version == MAX_SUPPORTED_VERSION)
}

pub(super) fn ensure(conn: &mut Connection) -> Result<(), String> {
    if observe_valid_current(conn)? {
        return Ok(());
    }
    let deadline = Instant::now() + SCHEMA_LOCK_TIMEOUT;
    loop {
        // Reobserve on every retry, including after a busy handler used the
        // remaining deadline. Another opener's committed migration is enough.
        if observe_valid_current(conn)? {
            return Ok(());
        }
        #[cfg(test)]
        after_stale_observation();
        let tx = match rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(error) if super::sqlite_error_is_contention(&error) => {
                // The failed BEGIN retained no transaction. Reobserve even at
                // the deadline, without extending writer patience.
                if observe_valid_current(conn)? {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(format!(
                        "Failed to lock PID mailbox sidecar schema migration: {error}"
                    ));
                }
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
        if locked_version >= CURRENT_VERSION {
            // Do not serialize fingerprint work under a stale writer lease.
            tx.commit().map_err(|err| {
                format!("Failed to finish PID mailbox sidecar schema check: {err}")
            })?;
            continue;
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

#[cfg(test)]
thread_local! {
    static STALE_OBSERVATION_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = Default::default();
}
#[cfg(test)]
fn after_stale_observation() {
    STALE_OBSERVATION_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

fn validate_supported_version(version: i64) -> Result<(), String> {
    if (0..=MAX_SUPPORTED_VERSION).contains(&version) {
        return Ok(());
    }
    Err(format!(
        "Unsupported PID mailbox sidecar schema version {version}; expected 0..={MAX_SUPPORTED_VERSION}"
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

fn ensure_mailbox_delivery_settlement_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_mailbox_delivery_attempt_columns(conn)
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

fn ensure_session_admission_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(super::session_admission_schema_definition())
        .map_err(|err| format!("Failed to ensure session admission queue: {err}"))
}

fn ensure_session_admission_launcher_identity_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_session_admission_launcher_identity_schema(conn)
}

fn ensure_session_admission_scaling_indexes(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(super::session_admission_scaling_indexes_definition())
        .map_err(|err| format!("Failed to ensure session admission scaling indexes: {err}"))
}

fn ensure_terminal_history_retention_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_terminal_history_retention_schema(conn)
}

fn ensure_terminal_payload_lookup_indexes(conn: &Connection) -> Result<(), String> {
    super::ensure_terminal_payload_lookup_indexes(conn)
}

fn ensure_wake_and_session_metadata_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_session_runtime_columns(conn)
}

fn ensure_wake_process_identity_schema(conn: &Connection) -> Result<(), String> {
    super::ensure_wake_claim_process_identity_columns(conn)
}

fn ensure_wake_sweep_progress_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(super::wake_sweep_progress_schema_definition())
        .map_err(|err| format!("Failed to ensure wake sweep progress schema: {err}"))
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

fn migrate_headless_observation_fence(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0013_headless_observation_fence.sql"))
        .map_err(|err| format!("Failed to migrate headless observation fence: {err}"))
}

fn ensure_observation_stop_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0014_observation_stop.sql"))
        .map_err(|err| format!("Failed to create observation stop history: {err}"))
}

fn ensure_delivery_finalization_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0015_delivery_finalization.sql"))
        .map_err(|err| format!("Failed to create delivery finalization references: {err}"))
}

fn migrate_receipt_scan(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(include_str!("0016_receipt_scan.sql"))
        .map_err(|err| format!("Failed to migrate receipt scan: {err}"))
}

/// Existing older-schema fixtures must remove newer objects as well as lower
/// user_version; this helper is never available to production migration code.
#[cfg(test)]
pub(super) fn remove_continuation_schema_for_legacy_fixture(conn: &Connection) {
    conn.execute_batch(
        "DROP TRIGGER completion_continuation_notification_ack;
        DROP TABLE completion_continuation_notification;
        DROP TABLE completion_continuation_attempt;
        DROP TABLE completion_continuation_source;
        DROP TABLE completion_continuation_context;
        DROP TABLE completion_continuation_owner;
        DROP TABLE completion_continuation_domain;
        DROP TRIGGER completion_continuation_claim_delete;
        DROP TRIGGER completion_continuation_claim_replace;",
    )
    .unwrap();
}

#[cfg(test)]
mod contention_tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn stale_reader_validates_committed_schema_with_unrelated_writer_held() {
        stale_reader_case("current");
    }

    #[test]
    fn stale_reader_rejects_uncommitted_corrupt_and_unsupported_schema() {
        for case in ["uncommitted", "corrupt", "unsupported"] {
            stale_reader_case(case);
        }
    }

    fn stale_reader_case(case: &str) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let reader_path = path.clone();
        let (observed_tx, observed_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            STALE_OBSERVATION_HOOK.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    observed_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }));
            });
            // Real opener retains its ordinary shared namespace fence.
            let result = super::super::MailboxDb::open(&reader_path);
            done_tx.send(result.map(|_| ())).unwrap();
        });
        observed_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        let mut writer = Connection::open(&path).unwrap();
        if case != "uncommitted" {
            let migrated = super::super::MailboxDb::open(&path).unwrap();
            drop(migrated);
            match case {
                "corrupt" => writer
                    .execute_batch("DROP TABLE completion_continuation_notification")
                    .unwrap(),
                "unsupported" => writer
                    .pragma_update(None, "user_version", CURRENT_VERSION + 1)
                    .unwrap(),
                _ => (),
            }
        }
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        if case == "uncommitted" {
            create_fresh_schema(&tx).unwrap();
            tx.pragma_update(None, "user_version", CURRENT_VERSION)
                .unwrap();
        }
        resume_tx.send(()).unwrap();
        // Completion is required BEFORE releasing this writer, not inferred
        // from elapsed sleep. The uncommitted migration must exhaust the
        // existing bounded lock policy rather than observe dirty schema.
        let result = done_rx.recv_timeout(Duration::from_secs(20)).unwrap();
        match case {
            "current" => result.unwrap(),
            "uncommitted" => assert!(
                result
                    .unwrap_err()
                    .contains("Failed to lock PID mailbox sidecar schema migration")
            ),
            "unsupported" => assert!(
                result
                    .unwrap_err()
                    .contains("Unsupported PID mailbox sidecar schema version")
            ),
            "corrupt" => assert!(result.is_err(), "invalid fingerprint was accepted"),
            _ => unreachable!(),
        }
        tx.rollback().unwrap();
        reader.join().unwrap();
    }
}
