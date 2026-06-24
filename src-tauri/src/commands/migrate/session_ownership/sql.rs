//! Declared roles: accessor, mapper, validator, predicate

use super::DryRunError;
use super::preflight::{self, IntegrityReport};
use oulipoly_state::CURRENT_SCHEMA_VERSION;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::Path;

const FORWARD_SQL: &str = include_str!("forward.sql");
const ROLLBACK_SQL: &str = include_str!("s11_wu4_restore_session_ownership_preimage.sql");

#[derive(Debug, Clone)]
pub(crate) struct ForwardCounts {
    pub(crate) counts: BTreeMap<String, i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct RollbackCounts {
    pub(crate) counts: BTreeMap<String, i64>,
    pub(crate) mismatches: BTreeMap<String, i64>,
    pub(crate) restored: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct CwdCompleteness {
    pub(crate) missing: i64,
    pub(crate) null: i64,
    pub(crate) non_absolute: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct LiveApplyVerification {
    pub(crate) integrity: IntegrityReport,
    pub(crate) planned: BTreeMap<String, i64>,
    pub(crate) applied: BTreeMap<String, i64>,
    pub(crate) residual_old_owned_rows: i64,
    pub(crate) segment_collision_count: i64,
    pub(crate) turn_collision_count: i64,
    pub(crate) preimage_rows: i64,
}

#[derive(Debug, Clone)]
enum RuntimeCwd {
    Missing,
    Null,
    Value(String),
}

#[derive(Debug, Clone)]
struct DriftProbeResult {
    kind: &'static str,
    exists: bool,
}

pub(crate) fn apply_forward(conn: &Connection) -> Result<ForwardCounts, DryRunError> {
    match conn.execute_batch(FORWARD_SQL) {
        Ok(()) => Ok(ForwardCounts {
            counts: read_key_counts(conn, "s11_wu4_last_run_counts")?,
        }),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err.into())
        }
    }
}

pub(crate) fn apply_forward_live(conn: &Connection) -> Result<ForwardCounts, DryRunError> {
    match conn.execute_batch(FORWARD_SQL) {
        Ok(()) => Ok(ForwardCounts {
            counts: read_key_counts(conn, "s11_wu4_last_run_counts")?,
        }),
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            if is_busy_or_locked(&err) {
                Err(live_busy_error())
            } else {
                Err(err.into())
            }
        }
    }
}

pub(crate) fn apply_rollback(conn: &Connection) -> Result<RollbackCounts, DryRunError> {
    assert_no_rollback_drift(conn)?;
    conn.execute_batch(ROLLBACK_SQL)?;
    let mismatches = preimage_mismatch_counts(conn)?;
    Ok(rollback_counts(
        read_key_counts(conn, "s11_wu4_last_rollback_counts")?,
        mismatches,
    ))
}

pub(crate) fn apply_rollback_live(conn: &Connection) -> Result<RollbackCounts, DryRunError> {
    assert_no_rollback_drift(conn)?;
    match conn.execute_batch(ROLLBACK_SQL) {
        Ok(()) => {
            let mismatches = preimage_mismatch_counts(conn)?;
            Ok(rollback_counts(
                read_key_counts(conn, "s11_wu4_last_rollback_counts")?,
                mismatches,
            ))
        }
        Err(err) if is_busy_or_locked(&err) => Err(live_busy_error()),
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn verify_live_apply(
    conn: &Connection,
    before: &IntegrityReport,
    forward: &ForwardCounts,
) -> Result<LiveApplyVerification, DryRunError> {
    let integrity = preflight::inspect_integrity(conn)?;
    validate_live_integrity(before, &integrity)?;
    let planned = planned_apply_counts(forward);
    let applied = if planned.values().all(|value| *value == 0) {
        planned.clone()
    } else {
        applied_apply_counts(conn)?
    };
    validate_count_match(&planned, &applied)?;
    let residual_old_owned_rows = residual_old_owner_count(conn)?;
    if residual_old_owned_rows != 0 {
        return Err(DryRunError::new(format!(
            "post-apply verification failed: residual old-owned rows: {residual_old_owned_rows}"
        )));
    }
    let segment_collision_count = segment_collision_count(conn)?;
    let turn_collision_count = turn_collision_count(conn)?;
    if segment_collision_count != 0 || turn_collision_count != 0 {
        return Err(DryRunError::new(format!(
            "post-apply verification failed: remaining collisions: segments={segment_collision_count}, turns={turn_collision_count}"
        )));
    }
    let preimage_rows = preimage_row_count(conn)?;
    let planned_any = planned.values().any(|value| *value != 0);
    if planned_any && last_run_preimage_plan_count(conn)? == 0 {
        return Err(DryRunError::new(
            "post-apply verification failed: current preimage plan is empty",
        ));
    }
    Ok(LiveApplyVerification {
        integrity,
        planned,
        applied,
        residual_old_owned_rows,
        segment_collision_count,
        turn_collision_count,
        preimage_rows,
    })
}

pub(crate) fn require_preimage_rows(conn: &Connection) -> Result<i64, DryRunError> {
    let rows = preimage_row_count(conn).map_err(|err| {
        DryRunError::new(format!(
            "nothing to roll back: preimage table missing or unreadable: {err}"
        ))
    })?;
    if rows == 0 {
        return Err(DryRunError::new(
            "nothing to roll back: preimage table is empty",
        ));
    }
    Ok(rows)
}

pub(crate) fn verify_rollback_restored(
    conn: &Connection,
) -> Result<BTreeMap<String, i64>, DryRunError> {
    let mismatches = preimage_mismatch_counts(conn)?;
    if !all_zero(&mismatches) {
        return Err(DryRunError::new(format!(
            "rollback verification failed: mismatches remain: {mismatches:?}"
        )));
    }
    Ok(mismatches)
}

pub(crate) fn drop_preimage_artifacts(conn: &Connection) -> Result<(), DryRunError> {
    conn.execute_batch("DROP TABLE IF EXISTS s11_wu4_restore_session_ownership_preimage;")?;
    Ok(())
}

pub(crate) fn cwd_completeness(
    state: &Connection,
    mailbox_copy: Option<&Path>,
) -> Result<CwdCompleteness, DryRunError> {
    let active_sessions = migrated_active_sessions(state)?;
    let runtime_cwds = runtime_cwds(mailbox_copy, &active_sessions)?;
    Ok(cwd_completeness_counts(&runtime_cwds))
}

fn assert_no_rollback_drift(conn: &Connection) -> Result<(), DryRunError> {
    validate_no_rollback_drift(read_rollback_drift(conn)?)
}

fn read_rollback_drift(conn: &Connection) -> Result<Vec<DriftProbeResult>, DryRunError> {
    rollback_drift_probes()
        .iter()
        .map(|(kind, sql)| {
            Ok(DriftProbeResult {
                kind,
                exists: conn.query_row(sql, [], |row| row.get(0))?,
            })
        })
        .collect()
}

fn rollback_drift_probes() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "chain drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_chains c ON c.chain_id = p.chain_id
                WHERE p.entity_kind = 'chain' AND c.model_name <> p.new_model_name
             )",
        ),
        (
            "segment drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_chain_segments s ON s.id = p.segment_id
                WHERE p.entity_kind = 'segment' AND s.provider_name <> p.new_provider_name
             )",
        ),
        (
            "turn drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_turns t ON t.id = p.turn_row_id
                WHERE p.entity_kind = 'turn' AND t.provider_name <> p.new_provider_name
             )",
        ),
        (
            "invocation missing before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                WHERE p.entity_kind = 'invocation'
                  AND NOT EXISTS (
                      SELECT 1 FROM invocations i WHERE i.id = CAST(p.row_pk AS INTEGER)
                  )
             )",
        ),
        (
            "invocation model drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN invocations i ON i.id = CAST(p.row_pk AS INTEGER)
                WHERE p.entity_kind = 'invocation'
                  AND NOT (i.model_name IS p.new_model_name)
             )",
        ),
        (
            "invocation provider drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN invocations i ON i.id = CAST(p.row_pk AS INTEGER)
                WHERE p.entity_kind = 'invocation'
                  AND NOT (i.provider_name IS p.new_provider_name)
             )",
        ),
        (
            "deleted segment id drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_chain_segments s ON s.id = p.segment_id
                WHERE p.entity_kind = 'segment_delete'
             )",
        ),
        (
            "deleted turn id drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_turns t ON t.id = p.turn_row_id
                WHERE p.entity_kind = 'turn_delete'
             )",
        ),
        (
            "segment merge survivor drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                JOIN session_chain_segments s ON s.id = p.segment_id
                WHERE p.entity_kind = 'segment_merge_survivor'
                  AND (
                       NOT (s.chain_id IS p.chain_id)
                    OR s.provider_name <> p.new_provider_name
                    OR NOT (s.session_id IS p.session_id)
                    OR NOT (s.started_at IS p.new_started_at)
                    OR NOT (s.ended_at IS p.new_ended_at)
                    OR NOT (s.last_turn_id IS p.new_last_turn_id)
                    OR NOT (s.transition_reason IS p.new_transition_reason)
                  )
             )",
        ),
        (
            "segment merge survivor missing before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                WHERE p.entity_kind = 'segment_merge_survivor'
                  AND NOT EXISTS (
                      SELECT 1 FROM session_chain_segments s WHERE s.id = p.segment_id
                  )
             )",
        ),
        (
            "segment full-row preimage drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                WHERE p.entity_kind IN ('segment_delete', 'segment_merge_survivor')
                  AND (
                         p.segment_id IS NULL
                      OR p.chain_id IS NULL
                      OR p.old_provider_name IS NULL
                      OR p.session_id IS NULL
                      OR p.segment_started_at IS NULL
                      OR p.segment_transition_reason IS NULL
                      OR (p.entity_kind = 'segment_merge_survivor'
                          AND (p.new_provider_name IS NULL
                               OR p.new_started_at IS NULL
                               OR p.new_transition_reason IS NULL))
                  )
             )",
        ),
        (
            "turn full-row preimage drift before rollback",
            "SELECT EXISTS(
                SELECT 1 FROM s11_wu4_restore_session_ownership_preimage p
                WHERE p.entity_kind = 'turn_delete'
                  AND (
                         p.turn_row_id IS NULL
                      OR p.old_provider_name IS NULL
                      OR p.session_id IS NULL
                      OR p.turn_id IS NULL
                      OR p.turn_timestamp IS NULL
                      OR p.turn_role IS NULL
                      OR p.turn_is_sidechain IS NULL
                      OR p.turn_is_compaction_boundary IS NULL
                      OR p.turn_ingested_at IS NULL
                  )
             )",
        ),
    ]
}

fn validate_no_rollback_drift(results: Vec<DriftProbeResult>) -> Result<(), DryRunError> {
    for result in results {
        validate_drift_probe(result)?;
    }
    Ok(())
}

fn validate_drift_probe(result: DriftProbeResult) -> Result<(), DryRunError> {
    if result.exists {
        return Err(DryRunError::new(result.kind));
    }
    Ok(())
}

fn preimage_mismatch_counts(conn: &Connection) -> Result<BTreeMap<String, i64>, DryRunError> {
    Ok(mismatch_count_map(mismatch_count_rows(conn)?))
}

fn mismatch_count_rows(conn: &Connection) -> Result<Vec<(&'static str, i64)>, DryRunError> {
    Ok(vec![
        ("chain_mismatch", read_chain_mismatch_count(conn)?),
        ("segment_mismatch", read_segment_mismatch_count(conn)?),
        ("turn_mismatch", read_turn_mismatch_count(conn)?),
        ("invocation_mismatch", read_invocation_mismatch_count(conn)?),
        (
            "segment_delete_mismatch",
            read_segment_delete_mismatch_count(conn)?,
        ),
        (
            "turn_delete_mismatch",
            read_turn_delete_mismatch_count(conn)?,
        ),
        (
            "segment_merge_survivor_mismatch",
            read_segment_merge_survivor_mismatch_count(conn)?,
        ),
    ])
}

fn mismatch_count_map(rows: Vec<(&'static str, i64)>) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for (key, value) in rows {
        counts.insert(key.to_string(), value);
    }
    counts
}

fn read_chain_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chains c ON c.chain_id = p.chain_id
         WHERE p.entity_kind = 'chain' AND c.model_name <> p.old_model_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn validate_live_integrity(
    before: &IntegrityReport,
    after: &IntegrityReport,
) -> Result<(), DryRunError> {
    if after.quick_check != "ok" {
        return Err(DryRunError::new(format!(
            "post-apply quick_check failed: {}",
            after.quick_check
        )));
    }
    if after.user_version != before.user_version
        || after.user_version != i64::from(CURRENT_SCHEMA_VERSION)
    {
        return Err(DryRunError::new(format!(
            "post-apply user_version mismatch: before {}, after {}, expected {}",
            before.user_version, after.user_version, CURRENT_SCHEMA_VERSION
        )));
    }
    Ok(())
}

fn planned_apply_counts(forward: &ForwardCounts) -> BTreeMap<String, i64> {
    [
        "chain_model_updates_to_apply",
        "segment_provider_updates_to_apply",
        "turn_provider_updates_to_apply",
        "invocation_identity_updates_to_apply",
        "segment_rows_merged_away",
        "turn_rows_deduped_away",
        "segment_merge_survivors_updated",
    ]
    .into_iter()
    .map(|key| (key.to_string(), *forward.counts.get(key).unwrap_or(&0)))
    .collect()
}

fn applied_apply_counts(conn: &Connection) -> Result<BTreeMap<String, i64>, DryRunError> {
    Ok(BTreeMap::from([
        (
            "chain_model_updates_to_apply".to_string(),
            applied_chain_count(conn)?,
        ),
        (
            "segment_provider_updates_to_apply".to_string(),
            applied_segment_count(conn)?,
        ),
        (
            "turn_provider_updates_to_apply".to_string(),
            applied_turn_count(conn)?,
        ),
        (
            "invocation_identity_updates_to_apply".to_string(),
            applied_invocation_count(conn)?,
        ),
        (
            "segment_rows_merged_away".to_string(),
            applied_segment_delete_count(conn)?,
        ),
        (
            "turn_rows_deduped_away".to_string(),
            applied_turn_delete_count(conn)?,
        ),
        (
            "segment_merge_survivors_updated".to_string(),
            applied_segment_merge_survivor_count(conn)?,
        ),
    ]))
}

fn validate_count_match(
    planned: &BTreeMap<String, i64>,
    applied: &BTreeMap<String, i64>,
) -> Result<(), DryRunError> {
    for (key, planned_value) in planned {
        let applied_value = applied.get(key).copied().unwrap_or_default();
        if applied_value != *planned_value {
            return Err(DryRunError::new(format!(
                "post-apply count mismatch for {key}: planned {planned_value}, applied {applied_value}"
            )));
        }
    }
    Ok(())
}

fn applied_chain_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_last_run_preimage_plan plan
         JOIN s11_wu4_restore_session_ownership_preimage p
           ON p.entity_kind = plan.entity_kind
          AND p.row_pk = plan.row_pk
          AND p.entity_kind = 'chain'
         JOIN session_chains c ON c.chain_id = p.chain_id
         WHERE p.old_model_name <> p.new_model_name
           AND c.model_name = p.new_model_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn applied_segment_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_last_run_preimage_plan plan
         JOIN s11_wu4_restore_session_ownership_preimage p
           ON p.entity_kind = plan.entity_kind
          AND p.row_pk = plan.row_pk
          AND p.entity_kind = 'segment'
         JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE p.old_provider_name <> p.new_provider_name
           AND s.provider_name = p.new_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn applied_turn_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_last_run_preimage_plan plan
         JOIN s11_wu4_restore_session_ownership_preimage p
           ON p.entity_kind = plan.entity_kind
          AND p.row_pk = plan.row_pk
          AND p.entity_kind = 'turn'
         JOIN session_turns t ON t.id = p.turn_row_id
         WHERE p.old_provider_name <> p.new_provider_name
           AND t.provider_name = p.new_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn applied_invocation_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_last_run_preimage_plan plan
         JOIN s11_wu4_restore_session_ownership_preimage p
           ON p.entity_kind = plan.entity_kind
          AND p.row_pk = plan.row_pk
          AND p.entity_kind = 'invocation'
         JOIN invocations i ON i.id = CAST(p.row_pk AS INTEGER)
         WHERE (NOT (p.old_model_name IS p.new_model_name)
                OR NOT (p.old_provider_name IS p.new_provider_name))
           AND i.model_name IS p.new_model_name
           AND i.provider_name IS p.new_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn residual_old_owner_count(conn: &Connection) -> Result<i64, DryRunError> {
    Ok(residual_chain_old_owner_count(conn)?
        + residual_segment_old_owner_count(conn)?
        + residual_turn_old_owner_count(conn)?
        + residual_invocation_old_owner_count(conn)?)
}

fn residual_chain_old_owner_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chains c ON c.chain_id = p.chain_id
         WHERE p.entity_kind = 'chain'
           AND p.old_model_name <> p.new_model_name
           AND c.model_name = p.old_model_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn residual_segment_old_owner_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE p.entity_kind = 'segment'
           AND p.old_provider_name <> p.new_provider_name
           AND s.provider_name = p.old_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn residual_turn_old_owner_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "WITH params AS (
             SELECT
                 (SELECT value FROM s11_wu4_forward_migration_params WHERE key = 'canonical_provider_name') AS canonical_provider_name,
                 (SELECT value FROM s11_wu4_forward_migration_params WHERE key = 'moved_provider_like_pattern') AS moved_provider_like_pattern
         ), migrated_sessions AS (
             SELECT DISTINCT candidate.session_id
             FROM s11_wu4_candidate_segments candidate
             CROSS JOIN params
             WHERE candidate.new_provider_name = params.canonical_provider_name
         )
         SELECT COUNT(*)
         FROM migrated_sessions migrated
         JOIN session_turns t ON t.session_id = migrated.session_id
         CROSS JOIN params
         LEFT JOIN s11_wu4_forward_target_provider_inventory inventory
           ON inventory.provider_name = t.provider_name
         WHERE lower(t.provider_name) LIKE params.moved_provider_like_pattern
           AND t.provider_name <> params.canonical_provider_name
           AND inventory.provider_name IS NULL",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn residual_invocation_old_owner_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "WITH params AS (
             SELECT
                 (SELECT value FROM s11_wu4_forward_migration_params WHERE key = 'canonical_provider_name') AS canonical_provider_name,
                 (SELECT value FROM s11_wu4_forward_migration_params WHERE key = 'moved_provider_like_pattern') AS moved_provider_like_pattern
         ), migrated_invocations AS (
             SELECT DISTINCT
                 i.id,
                 i.model_name,
                 i.provider_name,
                 CASE
                     WHEN candidate.is_orphaned = 1 THEN candidate.target_model_name
                     ELSE candidate.old_model_name
                 END AS target_chain_model_name
             FROM invocations i
             JOIN s11_wu4_candidate_segments candidate
               ON COALESCE(i.provider_session_id, i.session_id) = candidate.session_id
         )
         SELECT
             (SELECT COUNT(*)
              FROM s11_wu4_restore_session_ownership_preimage p
              JOIN invocations i ON i.id = CAST(p.row_pk AS INTEGER)
              WHERE p.entity_kind = 'invocation'
                AND ((NOT (p.old_model_name IS p.new_model_name)
                      AND i.model_name IS p.old_model_name)
                     OR (NOT (p.old_provider_name IS p.new_provider_name)
                         AND i.provider_name IS p.old_provider_name)))
           + (SELECT COUNT(*)
              FROM migrated_invocations migrated
              CROSS JOIN params
              LEFT JOIN s11_wu4_forward_target_provider_inventory inventory
                ON inventory.provider_name = migrated.provider_name
              WHERE migrated.provider_name IS NOT NULL
                AND lower(migrated.provider_name) LIKE params.moved_provider_like_pattern
                AND migrated.provider_name <> params.canonical_provider_name
                AND inventory.provider_name IS NULL)
           + (SELECT COUNT(*)
              FROM migrated_invocations migrated
              LEFT JOIN s11_wu4_forward_provider_ref_model_names provider_ref
                ON provider_ref.model_name = migrated.model_name
              WHERE migrated.model_name <> '<unknown>'
                AND provider_ref.model_name IS NULL
                AND migrated.model_name <> migrated.target_chain_model_name)",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn last_run_preimage_plan_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_last_run_preimage_plan",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn segment_collision_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM (
             SELECT chain_id, provider_name, session_id
             FROM session_chain_segments
             GROUP BY chain_id, provider_name, session_id
             HAVING COUNT(*) > 1
         )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn turn_collision_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM (
             SELECT provider_name, session_id, turn_id
             FROM session_turns
             GROUP BY provider_name, session_id, turn_id
             HAVING COUNT(*) > 1
         )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn preimage_row_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn is_busy_or_locked(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(error, _)
            if matches!(
                error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

fn live_busy_error() -> DryRunError {
    DryRunError::new(
        "live state DB is busy; stop the runner first and retry migrate-session-ownership",
    )
}

fn read_segment_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE p.entity_kind = 'segment' AND s.provider_name <> p.old_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_turn_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_turns t ON t.id = p.turn_row_id
         WHERE p.entity_kind = 'turn' AND t.provider_name <> p.old_provider_name",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_invocation_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*) FROM s11_wu4_restore_session_ownership_preimage p
         LEFT JOIN invocations i ON i.id = CAST(p.row_pk AS INTEGER)
         WHERE p.entity_kind = 'invocation'
           AND (
                i.id IS NULL
             OR NOT (i.model_name IS p.old_model_name)
             OR NOT (i.provider_name IS p.old_provider_name)
           )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn applied_segment_delete_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_last_run_preimage_plan plan
         JOIN s11_wu4_restore_session_ownership_preimage p
           ON p.entity_kind = plan.entity_kind
          AND p.row_pk = plan.row_pk
          AND p.entity_kind = 'segment_delete'
         LEFT JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE s.id IS NULL",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn applied_turn_delete_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_last_run_preimage_plan plan
         JOIN s11_wu4_restore_session_ownership_preimage p
           ON p.entity_kind = plan.entity_kind
          AND p.row_pk = plan.row_pk
          AND p.entity_kind = 'turn_delete'
         LEFT JOIN session_turns t ON t.id = p.turn_row_id
         WHERE t.id IS NULL",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn applied_segment_merge_survivor_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_last_run_preimage_plan plan
         JOIN s11_wu4_restore_session_ownership_preimage p
           ON p.entity_kind = plan.entity_kind
          AND p.row_pk = plan.row_pk
          AND p.entity_kind = 'segment_merge_survivor'
         JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE s.provider_name = p.new_provider_name
           AND s.started_at IS p.new_started_at
           AND s.ended_at IS p.new_ended_at
           AND s.last_turn_id IS p.new_last_turn_id
           AND s.transition_reason IS p.new_transition_reason",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_segment_delete_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         LEFT JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE p.entity_kind = 'segment_delete'
           AND (
                s.id IS NULL
             OR s.chain_id <> p.chain_id
             OR s.provider_name <> p.old_provider_name
             OR s.session_id <> p.session_id
             OR s.started_at <> p.segment_started_at
             OR NOT (s.ended_at IS p.segment_ended_at)
             OR NOT (s.last_turn_id IS p.segment_last_turn_id)
             OR s.transition_reason <> p.segment_transition_reason
           )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_turn_delete_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         LEFT JOIN session_turns t ON t.id = p.turn_row_id
         WHERE p.entity_kind = 'turn_delete'
           AND (
                t.id IS NULL
             OR t.provider_name <> p.old_provider_name
             OR t.session_id <> p.session_id
             OR t.turn_id <> p.turn_id
             OR t.timestamp <> p.turn_timestamp
             OR t.role <> p.turn_role
             OR NOT (t.parent_turn_id IS p.turn_parent_turn_id)
             OR t.is_sidechain <> p.turn_is_sidechain
             OR t.is_compaction_boundary <> p.turn_is_compaction_boundary
             OR NOT (t.source_file IS p.turn_source_file)
             OR t.ingested_at <> p.turn_ingested_at
             OR NOT (t.body IS p.turn_body)
           )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_segment_merge_survivor_mismatch_count(conn: &Connection) -> Result<i64, DryRunError> {
    conn.query_row(
        "SELECT COUNT(*)
         FROM s11_wu4_restore_session_ownership_preimage p
         LEFT JOIN session_chain_segments s ON s.id = p.segment_id
         WHERE p.entity_kind = 'segment_merge_survivor'
           AND (
                s.id IS NULL
             OR s.chain_id <> p.chain_id
             OR s.provider_name <> p.old_provider_name
             OR s.session_id <> p.session_id
             OR s.started_at <> p.segment_started_at
             OR NOT (s.ended_at IS p.segment_ended_at)
             OR NOT (s.last_turn_id IS p.segment_last_turn_id)
             OR s.transition_reason <> p.segment_transition_reason
           )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn rollback_counts(
    counts: BTreeMap<String, i64>,
    mismatches: BTreeMap<String, i64>,
) -> RollbackCounts {
    RollbackCounts {
        restored: all_zero(&mismatches),
        counts,
        mismatches,
    }
}

fn all_zero(counts: &BTreeMap<String, i64>) -> bool {
    counts.values().all(|value| *value == 0)
}

fn read_key_counts(conn: &Connection, table: &str) -> Result<BTreeMap<String, i64>, DryRunError> {
    let mut stmt = conn.prepare(&format!("SELECT key, value FROM {table}"))?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn migrated_active_sessions(state: &Connection) -> Result<Vec<String>, DryRunError> {
    let mut stmt = state.prepare(
        "SELECT DISTINCT segment.session_id
         FROM session_chain_segments segment
         JOIN s11_wu4_restore_session_ownership_preimage preimage
           ON preimage.entity_kind = 'chain'
          AND preimage.chain_id = segment.chain_id
         WHERE segment.ended_at IS NULL
         ORDER BY segment.session_id",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn runtime_cwds(
    mailbox_copy: Option<&Path>,
    session_ids: &[String],
) -> Result<Vec<RuntimeCwd>, DryRunError> {
    let Some(mailbox_copy) = mailbox_copy else {
        return Ok(missing_runtime_cwds(session_ids));
    };
    read_runtime_cwds(mailbox_copy, session_ids)
}

fn missing_runtime_cwds(session_ids: &[String]) -> Vec<RuntimeCwd> {
    session_ids.iter().map(|_| RuntimeCwd::Missing).collect()
}

fn read_runtime_cwds(
    mailbox_copy: &Path,
    session_ids: &[String],
) -> Result<Vec<RuntimeCwd>, DryRunError> {
    let mailbox = Connection::open(mailbox_copy)?;
    session_ids
        .iter()
        .map(|session_id| read_runtime_cwd(&mailbox, session_id))
        .collect()
}

fn read_runtime_cwd(mailbox: &Connection, session_id: &str) -> Result<RuntimeCwd, DryRunError> {
    let cwd: Option<Option<String>> = mailbox
        .query_row(
            "SELECT effective_cwd FROM session_runtime WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(runtime_cwd(cwd))
}

fn runtime_cwd(cwd: Option<Option<String>>) -> RuntimeCwd {
    match cwd {
        None => RuntimeCwd::Missing,
        Some(None) => RuntimeCwd::Null,
        Some(Some(path)) => RuntimeCwd::Value(path),
    }
}

fn cwd_completeness_counts(cwds: &[RuntimeCwd]) -> CwdCompleteness {
    let mut missing = 0;
    let mut null = 0;
    let mut non_absolute = 0;
    for cwd in cwds {
        match cwd {
            RuntimeCwd::Missing => missing += 1,
            RuntimeCwd::Null => null += 1,
            RuntimeCwd::Value(path) if !is_absolute_path(path) => non_absolute += 1,
            RuntimeCwd::Value(_) => {}
        }
    }
    CwdCompleteness {
        missing,
        null,
        non_absolute,
    }
}

fn is_absolute_path(path: &str) -> bool {
    Path::new(path).is_absolute()
}

trait OptionalRow<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalRow<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::migrate::session_ownership::{classifier, target_resolution};
    use oulipoly_state::StateDb;
    use rusqlite::params;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::Path;

    const FIXED_TS: &str = "2026-06-20T10:00:00Z";

    #[test]
    fn session_ownership_sql_stale_turn_dedup_plan_rolls_back_whole_forward_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let models_dir = dir.path().join("models");
        fs::create_dir_all(&models_dir).unwrap();
        write_target_config(&models_dir);

        let _ = StateDb::open(&state_path).unwrap();
        let mut conn = Connection::open(&state_path).unwrap();
        seed_byte_identical_turn_collision(&conn);

        let target = target_resolution::resolve_target(Some(&models_dir)).unwrap();
        let candidates = classifier::classify(&conn, &target).unwrap();
        assert_eq!(candidates.turn_rows_deduped_away, 1);
        let loser_id = candidates.turn_dedup_deletes[0].loser_turn_row_id;
        classifier::populate_sql_inputs(&mut conn, &target, &candidates).unwrap();
        conn.execute(
            "UPDATE session_turns SET body = 'stale-plan-body' WHERE id = ?1",
            [loser_id],
        )
        .unwrap();
        let before_forward = full_db_snapshot(&conn);

        let err = apply_forward(&conn).expect_err("stale plan must fail closed");

        assert!(
            err.to_string().contains("s11_wu4_forward_guard") || err.to_string().contains("UNIQUE"),
            "unexpected stale-plan error: {err}"
        );
        assert_eq!(full_db_snapshot(&conn), before_forward);
        assert!(!table_exists(
            &conn,
            "s11_wu4_restore_session_ownership_preimage"
        ));
        assert!(!table_exists(&conn, "s11_wu4_last_run_counts"));
    }

    #[test]
    fn session_ownership_sql_stale_segment_merge_plan_rolls_back_whole_forward_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let models_dir = dir.path().join("models");
        fs::create_dir_all(&models_dir).unwrap();
        write_target_config(&models_dir);

        let _ = StateDb::open(&state_path).unwrap();
        let mut conn = Connection::open(&state_path).unwrap();
        seed_segment_merge_collision(&conn);

        let target = target_resolution::resolve_target(Some(&models_dir)).unwrap();
        let candidates = classifier::classify(&conn, &target).unwrap();
        assert_eq!(candidates.segment_rows_merged_away, 1);
        assert_eq!(candidates.segment_merge_survivors_updated, 1);
        let loser_id = candidates.segment_merge_deletes[0].segment_id;
        classifier::populate_sql_inputs(&mut conn, &target, &candidates).unwrap();
        conn.execute(
            "UPDATE session_chain_segments SET started_at = '2026-06-20T11:00:00Z' WHERE id = ?1",
            [loser_id],
        )
        .unwrap();
        let before_forward = full_db_snapshot(&conn);

        let err = apply_forward(&conn).expect_err("stale segment plan must fail closed");

        assert_forward_guard_error(&err);
        assert_eq!(full_db_snapshot(&conn), before_forward);
        assert!(!table_exists(
            &conn,
            "s11_wu4_restore_session_ownership_preimage"
        ));
        assert!(!table_exists(&conn, "s11_wu4_last_run_counts"));
    }

    #[test]
    fn session_ownership_sql_turn_dedup_loser_provider_drift_rolls_back_whole_forward_transaction()
    {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let models_dir = dir.path().join("models");
        fs::create_dir_all(&models_dir).unwrap();
        write_target_config(&models_dir);

        let _ = StateDb::open(&state_path).unwrap();
        let mut conn = Connection::open(&state_path).unwrap();
        seed_byte_identical_turn_collision(&conn);

        let target = target_resolution::resolve_target(Some(&models_dir)).unwrap();
        let candidates = classifier::classify(&conn, &target).unwrap();
        assert_eq!(candidates.turn_rows_deduped_away, 1);
        let loser_id = candidates.turn_dedup_deletes[0].loser_turn_row_id;
        let third_provider = "acct-third-neutral";
        classifier::populate_sql_inputs(&mut conn, &target, &candidates).unwrap();
        conn.execute(
            "UPDATE session_turns SET provider_name = ?1 WHERE id = ?2",
            params![third_provider, loser_id],
        )
        .unwrap();
        let before_forward = full_db_snapshot(&conn);

        let err = apply_forward(&conn).expect_err("provider-drifted loser must fail closed");

        assert_forward_guard_error(&err);
        assert_eq!(full_db_snapshot(&conn), before_forward);
        assert!(turn_row_exists_with_provider(
            &conn,
            loser_id,
            third_provider
        ));
        assert!(!table_exists(
            &conn,
            "s11_wu4_restore_session_ownership_preimage"
        ));
        assert!(!table_exists(&conn, "s11_wu4_last_run_counts"));
    }

    #[test]
    fn session_ownership_sql_turn_dedup_loser_ingested_at_drift_is_tolerated() {
        let (_dir, mut conn, target) = setup_sql_turn_collision_fixture(|conn| {
            seed_byte_identical_turn_collision(conn);
        });
        let candidates = classifier::classify(&conn, &target).unwrap();
        assert_eq!(candidates.turn_rows_deduped_away, 1);
        let loser_id = candidates.turn_dedup_deletes[0].loser_turn_row_id;
        classifier::populate_sql_inputs(&mut conn, &target, &candidates).unwrap();
        conn.execute(
            "UPDATE session_turns SET ingested_at = '2026-06-20T10:30:00Z' WHERE id = ?1",
            [loser_id],
        )
        .unwrap();

        let counts = apply_forward(&conn).expect("metadata-only loser drift should be tolerated");

        assert_eq!(counts.counts["turn_rows_deduped_away"], 1);
        assert!(!turn_row_exists(&conn, loser_id));
    }

    #[test]
    fn session_ownership_sql_turn_dedup_loser_parent_turn_id_drift_is_tolerated() {
        let (_dir, mut conn, target) = setup_sql_turn_collision_fixture(|conn| {
            seed_byte_identical_turn_collision(conn);
        });
        let candidates = classifier::classify(&conn, &target).unwrap();
        assert_eq!(candidates.turn_rows_deduped_away, 1);
        let loser_id = candidates.turn_dedup_deletes[0].loser_turn_row_id;
        classifier::populate_sql_inputs(&mut conn, &target, &candidates).unwrap();
        conn.execute(
            "UPDATE session_turns SET parent_turn_id = 'metadata-drift-parent' WHERE id = ?1",
            [loser_id],
        )
        .unwrap();

        let counts = apply_forward(&conn).expect("metadata-only loser drift should be tolerated");

        assert_eq!(counts.counts["turn_rows_deduped_away"], 1);
        assert!(!turn_row_exists(&conn, loser_id));
    }

    #[test]
    fn session_ownership_sql_stale_turn_dedup_plan_role_drift_rolls_back_whole_forward_transaction()
    {
        assert_intrinsic_loser_drift_rolls_back("role", "user");
    }

    #[test]
    fn session_ownership_sql_stale_turn_dedup_plan_timestamp_drift_rolls_back_whole_forward_transaction()
     {
        assert_intrinsic_loser_drift_rolls_back("timestamp", "2026-06-20T10:30:00Z");
    }

    #[test]
    fn session_ownership_sql_classify_ingested_at_only_collision_produces_dedup_plan() {
        let (_dir, conn, target) = setup_sql_turn_collision_fixture(|conn| {
            seed_byte_identical_turn_collision(conn);
            conn.execute(
                "UPDATE session_turns SET ingested_at = '2026-06-20T10:30:00Z'
                 WHERE id = (SELECT MAX(id) FROM session_turns)",
                [],
            )
            .unwrap();
        });

        let candidates = classifier::classify(&conn, &target)
            .expect("metadata-only turn collision should produce a dedup plan");

        assert_eq!(candidates.turn_rows_deduped_away, 1);
        assert_eq!(candidates.turn_dedup_deletes.len(), 1);
    }

    #[test]
    fn s11_m2c_perf_explain_query_plan_searches_preimage_by_index() {
        let (_dir, mut conn, target) = setup_sql_turn_collision_fixture(|conn| {
            seed_byte_identical_turn_collision(conn);
            seed_segment_merge_collision(conn);
        });
        conn.execute_batch("PRAGMA automatic_index = OFF;").unwrap();
        let candidates = classifier::classify(&conn, &target).unwrap();
        assert_eq!(candidates.segment_rows_merged_away, 1);
        assert_eq!(candidates.segment_merge_survivors_updated, 1);
        assert_eq!(candidates.turn_rows_deduped_away, 1);
        classifier::populate_sql_inputs(&mut conn, &target, &candidates).unwrap();

        let counts = apply_forward(&conn).expect("forward fixture should apply");
        assert_eq!(counts.counts["segment_rows_merged_away"], 1);
        assert_eq!(counts.counts["turn_rows_deduped_away"], 1);
        assert_preimage_entity_kinds(
            &conn,
            &[
                "chain",
                "segment",
                "segment_delete",
                "segment_merge_survivor",
                "turn",
                "turn_delete",
            ],
        );

        let mut failures = Vec::new();
        assert_preimage_plan_searches(
            &conn,
            &mut failures,
            "forward turn UPDATE",
            sql_statement_containing(
                FORWARD_SQL,
                "forward turn UPDATE",
                &[
                    "UPDATE session_turns",
                    "preimage.new_provider_name",
                    "preimage.entity_kind = 'turn'",
                    "preimage.turn_row_id = session_turns.id",
                ],
            ),
            "idx_s11_wu4_preimage_kind_turn",
        );
        assert_preimage_plan_searches(
            &conn,
            &mut failures,
            "forward segment UPDATE",
            sql_statement_containing(
                FORWARD_SQL,
                "forward segment UPDATE",
                &[
                    "UPDATE session_chain_segments",
                    "SET provider_name = (",
                    "preimage.entity_kind = 'segment'",
                    "preimage.segment_id = session_chain_segments.id",
                ],
            ),
            "idx_s11_wu4_preimage_kind_segment",
        );
        assert_preimage_plan_searches(
            &conn,
            &mut failures,
            "forward chain UPDATE",
            sql_statement_containing(
                FORWARD_SQL,
                "forward chain UPDATE",
                &[
                    "UPDATE session_chains",
                    "preimage.new_model_name",
                    "preimage.entity_kind = 'chain'",
                    "preimage.chain_id = session_chains.chain_id",
                ],
            ),
            "idx_s11_wu4_preimage_kind_chain",
        );

        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_s11_wu4_preimage_kind_turn;
             DROP INDEX IF EXISTS idx_s11_wu4_preimage_kind_segment;
             DROP INDEX IF EXISTS idx_s11_wu4_preimage_kind_chain;",
        )
        .unwrap();
        conn.execute_batch(ROLLBACK_SQL)
            .expect("rollback fixture should restore");

        assert_preimage_plan_searches(
            &conn,
            &mut failures,
            "rollback segment_merge_survivor UPDATE",
            sql_statement_containing(
                ROLLBACK_SQL,
                "rollback segment_merge_survivor UPDATE",
                &[
                    "UPDATE session_chain_segments",
                    "SET chain_id = (",
                    "preimage.entity_kind = 'segment_merge_survivor'",
                    "preimage.segment_id = session_chain_segments.id",
                ],
            ),
            "idx_s11_wu4_preimage_kind_segment",
        );
        assert_preimage_plan_searches(
            &conn,
            &mut failures,
            "rollback turn UPDATE",
            sql_statement_containing(
                ROLLBACK_SQL,
                "rollback turn UPDATE",
                &[
                    "UPDATE session_turns",
                    "preimage.old_provider_name",
                    "preimage.entity_kind = 'turn'",
                    "preimage.turn_row_id = session_turns.id",
                ],
            ),
            "idx_s11_wu4_preimage_kind_turn",
        );
        assert_preimage_plan_searches(
            &conn,
            &mut failures,
            "rollback segment UPDATE",
            sql_statement_containing(
                ROLLBACK_SQL,
                "rollback segment UPDATE",
                &[
                    "UPDATE session_chain_segments",
                    "SET provider_name = (",
                    "preimage.old_provider_name",
                    "preimage.entity_kind = 'segment'",
                    "preimage.segment_id = session_chain_segments.id",
                ],
            ),
            "idx_s11_wu4_preimage_kind_segment",
        );
        assert_preimage_plan_searches(
            &conn,
            &mut failures,
            "rollback chain UPDATE",
            sql_statement_containing(
                ROLLBACK_SQL,
                "rollback chain UPDATE",
                &[
                    "UPDATE session_chains",
                    "preimage.old_model_name",
                    "preimage.entity_kind = 'chain'",
                    "preimage.chain_id = session_chains.chain_id",
                ],
            ),
            "idx_s11_wu4_preimage_kind_chain",
        );

        assert!(
            failures.is_empty(),
            "preimage hot statements must use committed preimage indexes:\n{}",
            failures.join("\n\n")
        );
    }

    fn assert_preimage_entity_kinds(conn: &Connection, expected: &[&str]) {
        let mut stmt = conn
            .prepare(
                "SELECT entity_kind
                 FROM s11_wu4_restore_session_ownership_preimage
                 GROUP BY entity_kind
                 ORDER BY entity_kind",
            )
            .unwrap();
        let actual = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(actual, expected);
    }

    fn sql_statement_containing<'a>(batch: &'a str, label: &str, needles: &[&str]) -> &'a str {
        batch
            .split(';')
            .map(str::trim)
            .find(|statement| {
                !statement.is_empty() && needles.iter().all(|needle| statement.contains(needle))
            })
            .unwrap_or_else(|| panic!("missing SQL statement for {label}"))
    }

    fn assert_preimage_plan_searches(
        conn: &Connection,
        failures: &mut Vec<String>,
        label: &str,
        statement: &str,
        expected_index: &str,
    ) {
        let details = explain_query_plan(conn, statement);
        let preimage_details: Vec<_> = details
            .iter()
            .filter(|detail| {
                detail.contains("s11_wu4_restore_session_ownership_preimage")
                    || detail.contains("preimage")
            })
            .collect();
        if preimage_details.is_empty()
            || preimage_details
                .iter()
                .any(|detail| detail.contains("SCAN"))
            || preimage_details.iter().any(|detail| {
                detail.contains("SEARCH")
                    && !detail.contains(expected_index)
                    && !detail.contains("USING INTEGER PRIMARY KEY")
            })
        {
            failures.push(format!(
                "{label} expected SEARCH with {expected_index}, got:\n{}",
                details
                    .iter()
                    .map(|detail| format!("  {detail}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
    }

    fn explain_query_plan(conn: &Connection, statement: &str) -> Vec<String> {
        let sql = format!("EXPLAIN QUERY PLAN {statement}");
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn write_target_config(models_dir: &Path) {
        let token = moved_provider_token();
        let target_binary = format!("agent-runner-{token}");
        let model_name = format!("target-{token}");
        let canonical = format!("acct-main-{token}");
        let accepted = format!("acct-accepted-{token}");
        fs::write(
            models_dir.join(format!("{model_name}.toml")),
            format!(
                "provider = {{ binary = {target_binary:?} }}\n\n[[providers]]\nname = {canonical:?}\nargs = []\n\n[[providers]]\nname = {accepted:?}\nargs = []\n"
            ),
        )
        .unwrap();
        fs::write(
            models_dir.parent().unwrap().join("providers.toml"),
            format!(
                "[{canonical}]\ncommand = \"/bin/true\"\nargs = []\nprompt_mode = \"arg\"\n\n[{accepted}]\ncommand = \"/bin/true\"\nargs = []\nprompt_mode = \"arg\"\n"
            ),
        )
        .unwrap();
    }

    fn seed_byte_identical_turn_collision(conn: &Connection) {
        let token = moved_provider_token();
        let source_model = format!("legacy-{token}-model");
        let canonical = format!("acct-main-{token}");
        let unregistered = format!("acct-unreg-{token}");
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES ('chain-stale-plan', ?1, ?1, ?2)",
            params![FIXED_TS, source_model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
             (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
             VALUES ('chain-stale-plan', ?1, 'session-stale', ?2, ?2, 'turn-stale', 'manual')",
            params![unregistered, FIXED_TS],
        )
        .unwrap();
        for provider in [unregistered, canonical] {
            conn.execute(
                "INSERT INTO session_turns
                 (provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
                  is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, 'session-stale', 'turn-stale', ?2, 'assistant', 'parent',
                         1, 0, 'stale.jsonl', ?2, 'identical-body')",
                params![provider, FIXED_TS],
            )
            .unwrap();
        }
    }

    fn seed_segment_merge_collision(conn: &Connection) {
        let token = moved_provider_token();
        let source_model = format!("legacy-{token}-model");
        let canonical = format!("acct-main-{token}");
        let unregistered = format!("acct-unreg-{token}");
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES ('chain-segment-stale-plan', ?1, ?1, ?2)",
            params![FIXED_TS, source_model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
             (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
             VALUES ('chain-segment-stale-plan', ?1, 'session-segment-stale', ?2, ?2, 'turn-earlier', 'manual')",
            params![unregistered, FIXED_TS],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
             (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
             VALUES ('chain-segment-stale-plan', ?1, 'session-segment-stale', '2026-06-20T10:05:00Z', NULL, 'turn-latest', 'quota_threshold')",
            params![canonical],
        )
        .unwrap();
    }

    fn setup_sql_turn_collision_fixture(
        seed: impl FnOnce(&Connection),
    ) -> (
        tempfile::TempDir,
        Connection,
        target_resolution::TargetResolution,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state.db");
        let models_dir = dir.path().join("models");
        fs::create_dir_all(&models_dir).unwrap();
        write_target_config(&models_dir);

        let _ = StateDb::open(&state_path).unwrap();
        let conn = Connection::open(&state_path).unwrap();
        seed(&conn);
        let target = target_resolution::resolve_target(Some(&models_dir)).unwrap();
        (dir, conn, target)
    }

    fn assert_intrinsic_loser_drift_rolls_back(column: &str, value: &str) {
        let (_dir, mut conn, target) = setup_sql_turn_collision_fixture(|conn| {
            seed_byte_identical_turn_collision(conn);
        });
        let candidates = classifier::classify(&conn, &target).unwrap();
        assert_eq!(candidates.turn_rows_deduped_away, 1);
        let loser_id = candidates.turn_dedup_deletes[0].loser_turn_row_id;
        classifier::populate_sql_inputs(&mut conn, &target, &candidates).unwrap();
        conn.execute(
            &format!(
                "UPDATE session_turns SET {} = ?1 WHERE id = ?2",
                quote_ident(column)
            ),
            params![value, loser_id],
        )
        .unwrap();
        let before_forward = full_db_snapshot(&conn);

        let err = apply_forward(&conn).expect_err("intrinsic loser drift must fail closed");

        assert_forward_guard_error(&err);
        assert_eq!(full_db_snapshot(&conn), before_forward);
        assert!(turn_row_exists(&conn, loser_id));
        assert!(!table_exists(
            &conn,
            "s11_wu4_restore_session_ownership_preimage"
        ));
        assert!(!table_exists(&conn, "s11_wu4_last_run_counts"));
    }

    fn assert_forward_guard_error(err: &DryRunError) {
        assert!(
            err.to_string().contains("s11_wu4_forward_guard") || err.to_string().contains("UNIQUE"),
            "unexpected stale-plan error: {err}"
        );
    }

    fn turn_row_exists_with_provider(conn: &Connection, id: i64, provider_name: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_turns WHERE id = ?1 AND provider_name = ?2)",
            params![id, provider_name],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn turn_row_exists(conn: &Connection, id: i64) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_turns WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn full_db_snapshot(conn: &Connection) -> BTreeMap<String, Vec<String>> {
        let mut stmt = conn
            .prepare(
                "SELECT name FROM sqlite_master
                 WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                 ORDER BY name",
            )
            .unwrap();
        let table_names = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        table_names
            .into_iter()
            .map(|table| {
                let rows = snapshot_table(conn, &table);
                (table, rows)
            })
            .collect()
    }

    fn snapshot_table(conn: &Connection, table: &str) -> Vec<String> {
        let columns = table_columns(conn, table);
        let mut parts = vec!["'rowid=' || quote(rowid)".to_string()];
        parts.extend(columns.iter().map(|column| {
            format!(
                "{} || quote({})",
                quote(format!("|{column}=").as_str()),
                quote_ident(column)
            )
        }));
        let sql = format!(
            "SELECT {} FROM {} ORDER BY rowid",
            parts.join(" || "),
            quote_ident(table)
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
        let sql = format!("PRAGMA table_info({})", quote_ident(table));
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }

    fn quote_ident(value: &str) -> String {
        format!("\"{}\"", value.replace('"', "\"\""))
    }

    fn table_exists(conn: &Connection, table: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn moved_provider_token() -> String {
        ["cla", "ude"].concat()
    }
}
