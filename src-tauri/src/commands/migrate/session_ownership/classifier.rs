//! Declared role: mapper, filter, accessor, validator, orchestration

use super::DryRunError;
use super::target_resolution::TargetResolution;
use oulipoly_config::ProviderConfig;
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader};

#[derive(Debug, Clone)]
pub(crate) struct CandidateSegment {
    pub(crate) chain_id: String,
    pub(crate) segment_id: i64,
    pub(crate) old_provider_name: String,
    pub(crate) session_id: String,
    pub(crate) new_provider_name: String,
    pub(crate) issue52_unregistered: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Candidates {
    pub(crate) candidate_chains: i64,
    pub(crate) candidate_segments: i64,
    pub(crate) eligible_segments: i64,
    pub(crate) blocked_segments: i64,
    pub(crate) issue52_unregistered_segments: i64,
    pub(crate) segment_rows_merged_away: i64,
    pub(crate) turn_rows_deduped_away: i64,
    pub(crate) segment_merge_survivors_updated: i64,
    pub(crate) source_chains: Vec<SourceChainCandidate>,
    pub(crate) segments: Vec<CandidateSegment>,
    pub(crate) segment_merge_groups: Vec<SegmentMergeGroup>,
    pub(crate) segment_merge_deletes: Vec<SegmentMergeDelete>,
    pub(crate) turn_remaps: Vec<SessionTurnRemap>,
    pub(crate) turn_dedup_deletes: Vec<TurnDedupDelete>,
}

#[derive(Debug, Clone)]
pub(crate) struct SourceChainCandidate {
    chain_id: String,
    is_orphaned: bool,
    target_model_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct CorrectivePlan {
    pub(crate) rows: Vec<CorrectivePlanRow>,
}

#[derive(Debug, Clone)]
pub(crate) struct CorrectivePlanRow {
    pub(crate) chain_id: String,
    pub(crate) old_model_name: String,
    pub(crate) new_model_name: String,
    pub(crate) evidence_source: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SegmentMergeGroup {
    pub(crate) survivor_segment_id: i64,
    pub(crate) expected_chain_id: String,
    pub(crate) expected_provider_name: String,
    pub(crate) expected_session_id: String,
    pub(crate) expected_started_at: String,
    pub(crate) expected_ended_at: Option<String>,
    pub(crate) expected_last_turn_id: Option<String>,
    pub(crate) expected_transition_reason: String,
    pub(crate) merged_started_at: String,
    pub(crate) merged_ended_at: Option<String>,
    pub(crate) merged_last_turn_id: Option<String>,
    pub(crate) merged_transition_reason: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SegmentMergeDelete {
    pub(crate) segment_id: i64,
    pub(crate) survivor_segment_id: i64,
    pub(crate) expected_chain_id: String,
    pub(crate) expected_provider_name: String,
    pub(crate) expected_session_id: String,
    pub(crate) expected_started_at: String,
    pub(crate) expected_ended_at: Option<String>,
    pub(crate) expected_last_turn_id: Option<String>,
    pub(crate) expected_transition_reason: String,
}

#[derive(Debug, Clone)]
pub(crate) struct TurnDedupDelete {
    pub(crate) loser_turn_row_id: i64,
    pub(crate) winner_turn_row_id: i64,
    pub(crate) old_provider_name: String,
    pub(crate) new_provider_name: String,
    pub(crate) session_id: String,
    pub(crate) turn_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionTurnRemap {
    pub(crate) turn_row_id: i64,
    pub(crate) session_id: String,
    pub(crate) old_provider_name: String,
    pub(crate) new_provider_name: String,
    pub(crate) turn_id: String,
}

#[derive(Debug, Clone)]
struct SourceSegment {
    chain_id: String,
    segment_id: i64,
    old_provider_name: String,
    session_id: String,
}

#[derive(Debug, Clone)]
struct SegmentRow {
    id: i64,
    chain_id: String,
    provider_name: String,
    session_id: String,
    started_at: String,
    ended_at: Option<String>,
    last_turn_id: Option<String>,
    transition_reason: String,
}

#[derive(Debug, Clone)]
struct TurnContent {
    turn_id: String,
    timestamp: String,
    role: String,
    body: Option<String>,
}

impl TurnContent {
    fn intrinsic_content_key(&self) -> (&str, &str, Option<&str>) {
        (
            self.role.as_str(),
            self.timestamp.as_str(),
            self.body.as_deref(),
        )
    }
}

#[derive(Debug, Clone)]
struct TurnRow {
    id: i64,
    old_provider_name: String,
    new_provider_name: String,
    session_id: String,
    content: TurnContent,
}

#[derive(Debug)]
struct SegmentPartitions {
    registered: Vec<SourceSegment>,
    unregistered: Vec<SourceSegment>,
}

#[derive(Debug)]
struct SqlInputRows<'a> {
    migration_params: Vec<(&'static str, String)>,
    provider_inventory: Vec<&'a str>,
    provider_ref_model_names: Vec<&'a str>,
    source_chains: Vec<SourceChainInputRow<'a>>,
    provider_aliases: Vec<ProviderAliasRow<'a>>,
    segment_merge_groups: &'a [SegmentMergeGroup],
    segment_merge_deletes: &'a [SegmentMergeDelete],
    turn_remaps: &'a [SessionTurnRemap],
    turn_dedup_deletes: &'a [TurnDedupDelete],
}

#[derive(Debug)]
struct SourceChainInputRow<'a> {
    chain_id: &'a str,
    is_orphaned: bool,
    target_model_name: &'a str,
}

#[derive(Debug)]
struct ProviderAliasRow<'a> {
    old_provider_name: &'a str,
    new_provider_name: &'a str,
}

pub(crate) fn classify(
    conn: &Connection,
    target: &TargetResolution,
) -> Result<Candidates, DryRunError> {
    let source_chains = source_chain_candidates(conn, target)?;
    let source_segments = read_source_segments(conn, &source_chains)?;
    let partitions = partition_segments_by_inventory(source_segments, &provider_inventory(target));
    let segments = candidate_segments(partitions, &target.canonical_provider_name);
    let (segment_merge_groups, segment_merge_deletes) = build_segment_merge_plan(conn, &segments)?;
    let turn_remaps = build_session_turn_remaps(conn, &segments, target)?;
    let turn_dedup_deletes = build_turn_dedup_plan(conn, &turn_remaps)?;
    Ok(candidates_from_segments(
        source_chains,
        segments,
        segment_merge_groups,
        segment_merge_deletes,
        turn_remaps,
        turn_dedup_deletes,
    ))
}

pub(crate) fn populate_sql_inputs(
    conn: &mut Connection,
    target: &TargetResolution,
    candidates: &Candidates,
) -> Result<(), DryRunError> {
    let tx = conn.transaction()?;
    create_sql_input_tables(&tx)?;
    write_sql_input_rows(&tx, &sql_input_rows(target, candidates))?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn populate_sql_inputs_temp(
    conn: &Connection,
    target: &TargetResolution,
    candidates: &Candidates,
) -> Result<(), DryRunError> {
    create_temp_sql_input_tables(conn)?;
    write_sql_input_rows(conn, &sql_input_rows(target, candidates))
}

pub(crate) fn build_corrective_plan(
    conn: &Connection,
    target: &TargetResolution,
) -> Result<CorrectivePlan, DryRunError> {
    let has_original_preimage = table_exists(conn, "s11_wu4_restore_session_ownership_preimage")?;
    let mut rows = if has_original_preimage {
        corrective_primary_plan_rows(conn, target)?
    } else {
        corrective_fallback_plan_rows(conn, target)?
    };
    let planned_chain_ids = rows
        .iter()
        .map(|row| row.chain_id.clone())
        .collect::<BTreeSet<_>>();
    rows.extend(corrective_transcript_plan_rows(
        conn,
        target,
        &target.transcript_source_provider,
        has_original_preimage,
        &planned_chain_ids,
    )?);
    Ok(CorrectivePlan { rows })
}

pub(crate) fn populate_corrective_plan_temp(
    conn: &Connection,
    plan: &CorrectivePlan,
) -> Result<(), DryRunError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS temp.s11_m2c_model_corrective_plan;
         CREATE TEMP TABLE s11_m2c_model_corrective_plan (
             chain_id TEXT PRIMARY KEY,
             old_model_name TEXT NOT NULL,
             new_model_name TEXT NOT NULL,
             evidence_source TEXT NOT NULL
         );",
    )?;
    for row in &plan.rows {
        conn.execute(
            "INSERT INTO s11_m2c_model_corrective_plan(chain_id, old_model_name, new_model_name, evidence_source)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                row.chain_id,
                row.old_model_name,
                row.new_model_name,
                row.evidence_source
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn cleanup_corrective_plan_temp(conn: &Connection) {
    let _ = conn.execute_batch("DROP TABLE IF EXISTS temp.s11_m2c_model_corrective_plan;");
}

pub(crate) fn cleanup_temp_sql_inputs(conn: &Connection) {
    let _ = conn.execute_batch(
        "DROP TABLE IF EXISTS temp.s11_wu4_migration_params;
          DROP TABLE IF EXISTS temp.s11_wu4_original_target_provider_inventory;
          DROP TABLE IF EXISTS temp.s11_wu4_target_provider_inventory;
          DROP TABLE IF EXISTS temp.s11_wu4_provider_ref_model_names;
          DROP TABLE IF EXISTS temp.s11_wu4_provider_aliases;
          DROP TABLE IF EXISTS temp.s11_wu4_source_chain_candidates;
          DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_groups;
          DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_deletes;
          DROP TABLE IF EXISTS temp.s11_wu4_session_turn_remaps;
          DROP TABLE IF EXISTS temp.s11_wu4_turn_dedup_deletes;",
    );
}

fn create_sql_input_tables(conn: &Connection) -> Result<(), DryRunError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS s11_wu4_migration_params;
          DROP TABLE IF EXISTS s11_wu4_original_target_provider_inventory;
          DROP TABLE IF EXISTS s11_wu4_target_provider_inventory;
          DROP TABLE IF EXISTS s11_wu4_provider_ref_model_names;
          DROP TABLE IF EXISTS s11_wu4_provider_aliases;
          DROP TABLE IF EXISTS s11_wu4_source_chain_candidates;
          DROP TABLE IF EXISTS s11_wu4_segment_merge_groups;
          DROP TABLE IF EXISTS s11_wu4_segment_merge_deletes;
          DROP TABLE IF EXISTS s11_wu4_session_turn_remaps;
          DROP TABLE IF EXISTS s11_wu4_turn_dedup_deletes;
          CREATE TABLE s11_wu4_migration_params (key TEXT PRIMARY KEY, value TEXT NOT NULL);
          CREATE TABLE s11_wu4_original_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'dry-run');
          CREATE TABLE s11_wu4_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'dry-run');
          CREATE TABLE s11_wu4_provider_ref_model_names (model_name TEXT PRIMARY KEY);
          CREATE TABLE s11_wu4_provider_aliases (old_provider_name TEXT PRIMARY KEY, new_provider_name TEXT NOT NULL, reason TEXT NOT NULL);
          CREATE TABLE s11_wu4_source_chain_candidates (
              chain_id TEXT PRIMARY KEY,
              evidence TEXT NOT NULL,
              target_model_name TEXT NOT NULL,
              is_orphaned INTEGER NOT NULL CHECK(is_orphaned IN (0, 1))
          );
          CREATE TABLE s11_wu4_segment_merge_groups (
              survivor_segment_id INTEGER PRIMARY KEY,
              expected_chain_id TEXT NOT NULL,
              expected_provider_name TEXT NOT NULL,
              expected_session_id TEXT NOT NULL,
              expected_started_at TEXT NOT NULL,
              expected_ended_at TEXT,
              expected_last_turn_id TEXT,
              expected_transition_reason TEXT NOT NULL,
              merged_started_at TEXT NOT NULL,
              merged_ended_at TEXT,
              merged_last_turn_id TEXT,
              merged_transition_reason TEXT NOT NULL
          );
           CREATE TABLE s11_wu4_segment_merge_deletes (
              segment_id INTEGER PRIMARY KEY,
              survivor_segment_id INTEGER NOT NULL,
              expected_chain_id TEXT NOT NULL,
              expected_provider_name TEXT NOT NULL,
              expected_session_id TEXT NOT NULL,
              expected_started_at TEXT NOT NULL,
              expected_ended_at TEXT,
              expected_last_turn_id TEXT,
               expected_transition_reason TEXT NOT NULL
           );
           CREATE TABLE s11_wu4_session_turn_remaps (
               turn_row_id INTEGER PRIMARY KEY,
               session_id TEXT NOT NULL,
               old_provider_name TEXT NOT NULL,
               new_provider_name TEXT NOT NULL,
               turn_id TEXT NOT NULL
           );
           CREATE TABLE s11_wu4_turn_dedup_deletes (
              loser_turn_row_id INTEGER PRIMARY KEY,
              winner_turn_row_id INTEGER NOT NULL,
              old_provider_name TEXT NOT NULL,
              new_provider_name TEXT NOT NULL,
              session_id TEXT NOT NULL,
              turn_id TEXT NOT NULL
         );",
    )
    .map_err(Into::into)
}

fn create_temp_sql_input_tables(conn: &Connection) -> Result<(), DryRunError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS temp.s11_wu4_migration_params;
          DROP TABLE IF EXISTS temp.s11_wu4_original_target_provider_inventory;
          DROP TABLE IF EXISTS temp.s11_wu4_target_provider_inventory;
          DROP TABLE IF EXISTS temp.s11_wu4_provider_ref_model_names;
          DROP TABLE IF EXISTS temp.s11_wu4_provider_aliases;
          DROP TABLE IF EXISTS temp.s11_wu4_source_chain_candidates;
          DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_groups;
          DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_deletes;
          DROP TABLE IF EXISTS temp.s11_wu4_session_turn_remaps;
          DROP TABLE IF EXISTS temp.s11_wu4_turn_dedup_deletes;
          CREATE TEMP TABLE s11_wu4_migration_params (key TEXT PRIMARY KEY, value TEXT NOT NULL);
          CREATE TEMP TABLE s11_wu4_original_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'live');
          CREATE TEMP TABLE s11_wu4_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'live');
          CREATE TEMP TABLE s11_wu4_provider_ref_model_names (model_name TEXT PRIMARY KEY);
          CREATE TEMP TABLE s11_wu4_provider_aliases (old_provider_name TEXT PRIMARY KEY, new_provider_name TEXT NOT NULL, reason TEXT NOT NULL);
          CREATE TEMP TABLE s11_wu4_source_chain_candidates (
              chain_id TEXT PRIMARY KEY,
              evidence TEXT NOT NULL,
              target_model_name TEXT NOT NULL,
              is_orphaned INTEGER NOT NULL CHECK(is_orphaned IN (0, 1))
          );
          CREATE TEMP TABLE s11_wu4_segment_merge_groups (
              survivor_segment_id INTEGER PRIMARY KEY,
              expected_chain_id TEXT NOT NULL,
              expected_provider_name TEXT NOT NULL,
              expected_session_id TEXT NOT NULL,
              expected_started_at TEXT NOT NULL,
              expected_ended_at TEXT,
              expected_last_turn_id TEXT,
              expected_transition_reason TEXT NOT NULL,
              merged_started_at TEXT NOT NULL,
              merged_ended_at TEXT,
              merged_last_turn_id TEXT,
              merged_transition_reason TEXT NOT NULL
          );
           CREATE TEMP TABLE s11_wu4_segment_merge_deletes (
              segment_id INTEGER PRIMARY KEY,
              survivor_segment_id INTEGER NOT NULL,
              expected_chain_id TEXT NOT NULL,
              expected_provider_name TEXT NOT NULL,
              expected_session_id TEXT NOT NULL,
              expected_started_at TEXT NOT NULL,
              expected_ended_at TEXT,
              expected_last_turn_id TEXT,
               expected_transition_reason TEXT NOT NULL
           );
           CREATE TEMP TABLE s11_wu4_session_turn_remaps (
               turn_row_id INTEGER PRIMARY KEY,
               session_id TEXT NOT NULL,
               old_provider_name TEXT NOT NULL,
               new_provider_name TEXT NOT NULL,
               turn_id TEXT NOT NULL
           );
           CREATE TEMP TABLE s11_wu4_turn_dedup_deletes (
              loser_turn_row_id INTEGER PRIMARY KEY,
              winner_turn_row_id INTEGER NOT NULL,
              old_provider_name TEXT NOT NULL,
              new_provider_name TEXT NOT NULL,
              session_id TEXT NOT NULL,
              turn_id TEXT NOT NULL
         );",
    )
    .map_err(Into::into)
}

fn write_sql_input_rows(conn: &Connection, rows: &SqlInputRows<'_>) -> Result<(), DryRunError> {
    for (key, value) in &rows.migration_params {
        conn.execute(
            "INSERT INTO s11_wu4_migration_params(key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }
    for provider_name in &rows.provider_inventory {
        conn.execute(
            "INSERT INTO s11_wu4_original_target_provider_inventory(provider_name, source) VALUES (?1, 'config')",
            [provider_name],
        )?;
        conn.execute(
            "INSERT INTO s11_wu4_target_provider_inventory(provider_name, source) VALUES (?1, 'config')",
            [provider_name],
        )?;
    }
    for model_name in &rows.provider_ref_model_names {
        conn.execute(
            "INSERT INTO s11_wu4_provider_ref_model_names(model_name) VALUES (?1)",
            [model_name],
        )?;
    }
    for source in &rows.source_chains {
        conn.execute(
            "INSERT INTO s11_wu4_source_chain_candidates(chain_id, evidence, target_model_name, is_orphaned) VALUES (?1, 'copied-state', ?2, ?3)",
            params![
                source.chain_id,
                source.target_model_name,
                i64::from(source.is_orphaned)
            ],
        )?;
    }
    for alias in &rows.provider_aliases {
        conn.execute(
            "INSERT OR IGNORE INTO s11_wu4_provider_aliases(old_provider_name, new_provider_name, reason) VALUES (?1, ?2, 'canonical-remap')",
            params![alias.old_provider_name, alias.new_provider_name],
        )?;
    }
    for group in rows.segment_merge_groups {
        conn.execute(
            "INSERT INTO s11_wu4_segment_merge_groups(
                 survivor_segment_id, expected_chain_id, expected_provider_name,
                 expected_session_id, expected_started_at, expected_ended_at,
                 expected_last_turn_id, expected_transition_reason, merged_started_at,
                 merged_ended_at, merged_last_turn_id, merged_transition_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                group.survivor_segment_id,
                group.expected_chain_id,
                group.expected_provider_name,
                group.expected_session_id,
                group.expected_started_at,
                group.expected_ended_at,
                group.expected_last_turn_id,
                group.expected_transition_reason,
                group.merged_started_at,
                group.merged_ended_at,
                group.merged_last_turn_id,
                group.merged_transition_reason,
            ],
        )?;
    }
    for delete in rows.segment_merge_deletes {
        conn.execute(
            "INSERT INTO s11_wu4_segment_merge_deletes(
                 segment_id, survivor_segment_id, expected_chain_id,
                 expected_provider_name, expected_session_id, expected_started_at,
                 expected_ended_at, expected_last_turn_id, expected_transition_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                delete.segment_id,
                delete.survivor_segment_id,
                delete.expected_chain_id,
                delete.expected_provider_name,
                delete.expected_session_id,
                delete.expected_started_at,
                delete.expected_ended_at,
                delete.expected_last_turn_id,
                delete.expected_transition_reason,
            ],
        )?;
    }
    for remap in rows.turn_remaps {
        conn.execute(
            "INSERT INTO s11_wu4_session_turn_remaps(
                 turn_row_id, session_id, old_provider_name, new_provider_name, turn_id
              ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                remap.turn_row_id,
                remap.session_id,
                remap.old_provider_name,
                remap.new_provider_name,
                remap.turn_id,
            ],
        )?;
    }
    for delete in rows.turn_dedup_deletes {
        conn.execute(
            "INSERT INTO s11_wu4_turn_dedup_deletes(
                 loser_turn_row_id, winner_turn_row_id, old_provider_name,
                 new_provider_name, session_id, turn_id
              ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                delete.loser_turn_row_id,
                delete.winner_turn_row_id,
                delete.old_provider_name,
                delete.new_provider_name,
                delete.session_id,
                delete.turn_id,
            ],
        )?;
    }
    Ok(())
}

fn sql_input_rows<'a>(
    target: &'a TargetResolution,
    candidates: &'a Candidates,
) -> SqlInputRows<'a> {
    SqlInputRows {
        migration_params: migration_param_rows(target),
        provider_inventory: provider_inventory_rows(target),
        provider_ref_model_names: provider_ref_model_name_rows(target),
        source_chains: source_chain_rows(candidates),
        provider_aliases: provider_alias_rows(&remapped_segments(&candidates.segments)),
        segment_merge_groups: &candidates.segment_merge_groups,
        segment_merge_deletes: &candidates.segment_merge_deletes,
        turn_remaps: &candidates.turn_remaps,
        turn_dedup_deletes: &candidates.turn_dedup_deletes,
    }
}

fn migration_param_rows(target: &TargetResolution) -> Vec<(&'static str, String)> {
    vec![
        ("target_model_name", target.model_name.clone()),
        (
            "canonical_provider_name",
            target.canonical_provider_name.clone(),
        ),
        ("moved_provider_like_pattern", target_provider_pattern()),
        ("migration_id", "s11-m2-session-ownership".to_string()),
    ]
}

fn provider_inventory_rows(target: &TargetResolution) -> Vec<&str> {
    target.inventory.iter().map(String::as_str).collect()
}

fn provider_ref_model_name_rows(target: &TargetResolution) -> Vec<&str> {
    target
        .moved_family_provider_ref_models
        .iter()
        .map(String::as_str)
        .collect()
}

fn source_chain_rows(candidates: &Candidates) -> Vec<SourceChainInputRow<'_>> {
    candidates
        .source_chains
        .iter()
        .map(|source| SourceChainInputRow {
            chain_id: source.chain_id.as_str(),
            is_orphaned: source.is_orphaned,
            target_model_name: source.target_model_name.as_str(),
        })
        .collect()
}

fn provider_alias_rows<'a>(segments: &[&'a CandidateSegment]) -> Vec<ProviderAliasRow<'a>> {
    segments
        .iter()
        .map(|segment| ProviderAliasRow {
            old_provider_name: segment.old_provider_name.as_str(),
            new_provider_name: segment.new_provider_name.as_str(),
        })
        .collect()
}

fn source_chain_candidates(
    conn: &Connection,
    target: &TargetResolution,
) -> Result<Vec<SourceChainCandidate>, DryRunError> {
    let rows = read_source_chain_candidates(conn, &target_provider_pattern())?;
    rows.into_iter()
        .map(|(chain_id, model_name)| {
            let is_orphaned = !target
                .moved_family_provider_ref_models
                .contains(&model_name);
            let target_model_name = if is_orphaned {
                let segment_session_ids = read_segment_session_ids_for_chain(conn, &chain_id)?;
                // transcript fallback: not required - DB evidence + deterministic fallback cover the tests.
                infer_chain_target_model(
                    conn,
                    &segment_session_ids,
                    &target.moved_family_provider_ref_models,
                )?
                .unwrap_or_else(|| target.model_name.clone())
            } else {
                model_name
            };
            Ok(SourceChainCandidate {
                chain_id,
                is_orphaned,
                target_model_name,
            })
        })
        .collect()
}

pub(crate) fn infer_chain_target_model(
    conn: &Connection,
    segment_session_ids: &[String],
    moved_family_provider_ref_models: &BTreeSet<String>,
) -> Result<Option<String>, DryRunError> {
    infer_chain_target_model_excluding_rewritten(
        conn,
        segment_session_ids,
        moved_family_provider_ref_models,
        false,
    )
}

pub(crate) fn infer_chain_target_model_from_transcript(
    provider_config: &ProviderConfig,
    segment_session_ids: &[String],
    moved_family_provider_ref_models: &BTreeSet<String>,
    backfill_default_model_name: &str,
) -> Option<String> {
    let mut evidence: BTreeMap<String, i64> = BTreeMap::new();
    for session_id in segment_session_ids {
        let Some(path) = oulipoly_runtime::migration::find_session_source_from_storage(
            provider_config,
            session_id,
        ) else {
            continue;
        };
        let Ok(file) = File::open(path) else {
            continue;
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if value.get("type").and_then(|value| value.as_str()) != Some("assistant") {
                continue;
            }
            let Some(model_name) = value
                .get("message")
                .and_then(|message| message.get("model"))
                .and_then(|model| model.as_str())
            else {
                continue;
            };
            if model_name == "<synthetic>" {
                continue;
            }
            if let Some(inventory_model) =
                transcript_inventory_model(model_name, moved_family_provider_ref_models)
            {
                *evidence.entry(inventory_model.to_string()).or_insert(0) += 1;
            }
        }
    }
    evidence
        .into_iter()
        .max_by(|(left_model, left_count), (right_model, right_count)| {
            left_count
                .cmp(right_count)
                .then_with(|| right_model.cmp(left_model))
        })
        .and_then(|(model, _)| (model != backfill_default_model_name).then_some(model))
}

fn transcript_inventory_model<'a>(
    transcript_model: &str,
    moved_family_provider_ref_models: &'a BTreeSet<String>,
) -> Option<&'a str> {
    let mut best = None;
    for inventory_model in moved_family_provider_ref_models {
        let inventory_model_name = inventory_model.as_str();
        let matches_inventory = transcript_model == inventory_model_name
            || transcript_model
                .strip_prefix(inventory_model_name)
                .is_some_and(|suffix| suffix.starts_with('-'));
        let is_longest_match = best
            .as_ref()
            .map(|current: &&String| inventory_model.len() > current.len())
            .unwrap_or(true);
        if matches_inventory && is_longest_match {
            best = Some(inventory_model);
        }
    }
    best.map(String::as_str)
}

fn corrective_primary_plan_rows(
    conn: &Connection,
    target: &TargetResolution,
) -> Result<Vec<CorrectivePlanRow>, DryRunError> {
    let mut stmt = conn.prepare(
        "SELECT p.chain_id, p.new_model_name
         FROM s11_wu4_restore_session_ownership_preimage p
         JOIN session_chains c ON c.chain_id = p.chain_id
         WHERE p.entity_kind = 'chain'
           AND p.old_model_name = '<unknown>'
           AND c.model_name = p.new_model_name
         ORDER BY p.chain_id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (chain_id, backfill_default_model_name) = row?;
        let segment_session_ids = read_segment_session_ids_for_chain(conn, &chain_id)?;
        let inferred = infer_chain_target_model_excluding_rewritten(
            conn,
            &segment_session_ids,
            &target.moved_family_provider_ref_models,
            true,
        )?;
        Ok(inferred
            .filter(|model_name| model_name != &backfill_default_model_name)
            .map(|new_model_name| CorrectivePlanRow {
                chain_id,
                old_model_name: backfill_default_model_name,
                new_model_name,
                evidence_source: "original-preimage-db-evidence".to_string(),
            }))
    })
    .filter_map(|row| match row {
        Ok(Some(value)) => Some(Ok(value)),
        Ok(None) => None,
        Err(err) => Some(Err(err)),
    })
    .collect()
}

fn corrective_fallback_plan_rows(
    conn: &Connection,
    target: &TargetResolution,
) -> Result<Vec<CorrectivePlanRow>, DryRunError> {
    let rows = read_source_chain_candidates(conn, &target_provider_pattern())?;
    rows.into_iter()
        .filter(|(_, model_name)| model_name == &target.model_name)
        .map(|(chain_id, _)| {
            let segment_session_ids = read_segment_session_ids_for_chain(conn, &chain_id)?;
            Ok(unique_different_in_family_model(
                conn,
                &segment_session_ids,
                &target.moved_family_provider_ref_models,
                &target.model_name,
            )?
            .map(|new_model_name| CorrectivePlanRow {
                chain_id,
                old_model_name: target.model_name.clone(),
                new_model_name,
                evidence_source: "fallback-single-db-evidence".to_string(),
            }))
        })
        .filter_map(|row| match row {
            Ok(Some(value)) => Some(Ok(value)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        })
        .collect()
}

fn corrective_transcript_plan_rows(
    conn: &Connection,
    target: &TargetResolution,
    provider_config: &ProviderConfig,
    has_original_preimage: bool,
    planned_chain_ids: &BTreeSet<String>,
) -> Result<Vec<CorrectivePlanRow>, DryRunError> {
    let candidates = corrective_transcript_candidate_rows(conn, target, has_original_preimage)?;
    candidates
        .into_iter()
        .filter(|(chain_id, _)| !planned_chain_ids.contains(chain_id))
        .map(|(chain_id, backfill_default_model_name)| {
            let segment_session_ids = read_segment_session_ids_for_chain(conn, &chain_id)?;
            if has_different_in_family_evidence(
                conn,
                &segment_session_ids,
                &target.moved_family_provider_ref_models,
                &backfill_default_model_name,
                has_original_preimage,
            )? {
                return Ok(None);
            }
            Ok(infer_chain_target_model_from_transcript(
                provider_config,
                &segment_session_ids,
                &target.moved_family_provider_ref_models,
                &backfill_default_model_name,
            )
            .map(|new_model_name| CorrectivePlanRow {
                chain_id,
                old_model_name: backfill_default_model_name,
                new_model_name,
                evidence_source: "transcript".to_string(),
            }))
        })
        .filter_map(|row| match row {
            Ok(Some(value)) => Some(Ok(value)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        })
        .collect()
}

fn corrective_transcript_candidate_rows(
    conn: &Connection,
    target: &TargetResolution,
    has_original_preimage: bool,
) -> Result<Vec<(String, String)>, DryRunError> {
    if has_original_preimage {
        let mut stmt = conn.prepare(
            "SELECT p.chain_id, p.new_model_name
             FROM s11_wu4_restore_session_ownership_preimage p
             JOIN session_chains c ON c.chain_id = p.chain_id
             WHERE p.entity_kind = 'chain'
               AND p.old_model_name = '<unknown>'
               AND c.model_name = p.new_model_name
             ORDER BY p.chain_id",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<_, _>>().map_err(Into::into)
    } else {
        Ok(
            read_source_chain_candidates(conn, &target_provider_pattern())?
                .into_iter()
                .filter(|(_, model_name)| model_name == &target.model_name)
                .collect(),
        )
    }
}

fn has_different_in_family_evidence(
    conn: &Connection,
    segment_session_ids: &[String],
    moved_family_provider_ref_models: &BTreeSet<String>,
    default_model_name: &str,
    exclude_original_invocation_preimage: bool,
) -> Result<bool, DryRunError> {
    let exclude_preimage = exclude_original_invocation_preimage
        && table_exists(conn, "s11_wu4_restore_session_ownership_preimage")?;
    let mut stmt = conn.prepare(
        "SELECT id, model_name
         FROM invocations
         WHERE COALESCE(provider_session_id, session_id) = ?1
         ORDER BY id",
    )?;
    for session_id in segment_session_ids {
        let rows = stmt.query_map([session_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        for row in rows {
            let (invocation_id, model_name) = row?;
            if exclude_preimage && original_invocation_preimage_contains(conn, invocation_id)? {
                continue;
            }
            let Some(model_name) = model_name else {
                continue;
            };
            if model_name != default_model_name
                && moved_family_provider_ref_models.contains(&model_name)
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn unique_different_in_family_model(
    conn: &Connection,
    segment_session_ids: &[String],
    moved_family_provider_ref_models: &BTreeSet<String>,
    default_model_name: &str,
) -> Result<Option<String>, DryRunError> {
    let mut models = BTreeSet::new();
    let mut stmt = conn.prepare(
        "SELECT model_name
         FROM invocations
         WHERE COALESCE(provider_session_id, session_id) = ?1
         ORDER BY id",
    )?;
    for session_id in segment_session_ids {
        let rows = stmt.query_map([session_id], |row| row.get::<_, Option<String>>(0))?;
        for row in rows {
            let Some(model_name) = row? else {
                continue;
            };
            if model_name != default_model_name
                && moved_family_provider_ref_models.contains(&model_name)
            {
                models.insert(model_name);
            }
        }
    }
    if models.len() == 1 {
        Ok(models.into_iter().next())
    } else {
        Ok(None)
    }
}

fn infer_chain_target_model_excluding_rewritten(
    conn: &Connection,
    segment_session_ids: &[String],
    moved_family_provider_ref_models: &BTreeSet<String>,
    exclude_original_invocation_preimage: bool,
) -> Result<Option<String>, DryRunError> {
    let mut evidence: BTreeMap<String, (i64, String)> = BTreeMap::new();
    let exclude_preimage = exclude_original_invocation_preimage
        && table_exists(conn, "s11_wu4_restore_session_ownership_preimage")?;
    let mut stmt = conn.prepare(
        "SELECT id, model_name, created_at
         FROM invocations
         WHERE COALESCE(provider_session_id, session_id) = ?1
         ORDER BY id",
    )?;
    for session_id in segment_session_ids {
        let rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (invocation_id, model_name, created_at) = row?;
            if exclude_preimage && original_invocation_preimage_contains(conn, invocation_id)? {
                continue;
            }
            let Some(model_name) = model_name else {
                continue;
            };
            if !moved_family_provider_ref_models.contains(&model_name) {
                continue;
            }
            let entry = evidence.entry(model_name).or_insert((0, String::new()));
            entry.0 += 1;
            if created_at > entry.1 {
                entry.1 = created_at;
            }
        }
    }
    Ok(evidence
        .into_iter()
        .max_by(
            |(left_model, (left_count, left_latest)),
             (right_model, (right_count, right_latest))| {
                left_count
                    .cmp(right_count)
                    .then(left_latest.cmp(right_latest))
                    .then_with(|| right_model.cmp(left_model))
            },
        )
        .map(|(model, _)| model))
}

fn original_invocation_preimage_contains(
    conn: &Connection,
    invocation_id: i64,
) -> Result<bool, DryRunError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM s11_wu4_restore_session_ownership_preimage
             WHERE entity_kind = 'invocation' AND row_pk = CAST(?1 AS TEXT)
         )",
        [invocation_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_segment_session_ids_for_chain(
    conn: &Connection,
    chain_id: &str,
) -> Result<Vec<String>, DryRunError> {
    let mut stmt = conn
        .prepare("SELECT session_id FROM session_chain_segments WHERE chain_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([chain_id], |row| row.get(0))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn read_source_chain_candidates(
    conn: &Connection,
    provider_pattern: &str,
) -> Result<Vec<(String, String)>, DryRunError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT c.chain_id, c.model_name
         FROM session_chains c
         LEFT JOIN session_chain_segments s ON s.chain_id = c.chain_id
         WHERE lower(c.model_name) LIKE ?1 OR lower(s.provider_name) LIKE ?1
         ORDER BY c.chain_id",
    )?;
    let rows = stmt.query_map([provider_pattern], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn target_provider_pattern() -> String {
    contains_pattern(&moved_provider_token())
}

fn contains_pattern(value: &str) -> String {
    let mut pattern = String::with_capacity(value.len() + 2);
    pattern.push('%');
    pattern.push_str(value);
    pattern.push('%');
    pattern
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, DryRunError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
         )",
        [table_name],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_source_segments(
    conn: &Connection,
    source_chains: &[SourceChainCandidate],
) -> Result<Vec<SourceSegment>, DryRunError> {
    let mut segments = Vec::new();
    for source in source_chains {
        segments.extend(read_source_segments_for_chain(conn, &source.chain_id)?);
    }
    Ok(segments)
}

fn read_source_segments_for_chain(
    conn: &Connection,
    chain_id: &str,
) -> Result<Vec<SourceSegment>, DryRunError> {
    let mut stmt = conn.prepare(
        "SELECT id, provider_name, session_id FROM session_chain_segments WHERE chain_id = ?1",
    )?;
    let rows = stmt.query_map([chain_id], |row| {
        Ok(SourceSegment {
            chain_id: chain_id.to_string(),
            segment_id: row.get(0)?,
            old_provider_name: row.get(1)?,
            session_id: row.get(2)?,
        })
    })?;
    rows.collect::<Result<_, _>>().map_err(Into::into)
}

fn provider_inventory(target: &TargetResolution) -> BTreeSet<String> {
    target.inventory.iter().cloned().collect()
}

fn partition_segments_by_inventory(
    segments: Vec<SourceSegment>,
    inventory: &BTreeSet<String>,
) -> SegmentPartitions {
    let mut registered = Vec::new();
    let mut unregistered = Vec::new();
    for segment in segments {
        if inventory.contains(&segment.old_provider_name) {
            registered.push(segment);
        } else {
            unregistered.push(segment);
        }
    }
    SegmentPartitions {
        registered,
        unregistered,
    }
}

fn candidate_segments(
    partitions: SegmentPartitions,
    canonical_provider_name: &str,
) -> Vec<CandidateSegment> {
    let mut segments = registered_candidate_segments(partitions.registered);
    segments.extend(unregistered_candidate_segments(
        partitions.unregistered,
        canonical_provider_name,
    ));
    segments
}

fn registered_candidate_segments(segments: Vec<SourceSegment>) -> Vec<CandidateSegment> {
    segments
        .into_iter()
        .map(|segment| candidate_segment(segment, None))
        .collect()
}

fn unregistered_candidate_segments(
    segments: Vec<SourceSegment>,
    canonical_provider_name: &str,
) -> Vec<CandidateSegment> {
    segments
        .into_iter()
        .map(|segment| candidate_segment(segment, Some(canonical_provider_name)))
        .collect()
}

fn candidate_segment(
    segment: SourceSegment,
    canonical_provider_name: Option<&str>,
) -> CandidateSegment {
    let new_provider_name = canonical_provider_name
        .map(str::to_string)
        .unwrap_or_else(|| segment.old_provider_name.clone());
    CandidateSegment {
        chain_id: segment.chain_id,
        segment_id: segment.segment_id,
        old_provider_name: segment.old_provider_name,
        session_id: segment.session_id,
        new_provider_name,
        issue52_unregistered: canonical_provider_name.is_some(),
    }
}

fn candidates_from_segments(
    source_chains: Vec<SourceChainCandidate>,
    segments: Vec<CandidateSegment>,
    segment_merge_groups: Vec<SegmentMergeGroup>,
    segment_merge_deletes: Vec<SegmentMergeDelete>,
    turn_remaps: Vec<SessionTurnRemap>,
    turn_dedup_deletes: Vec<TurnDedupDelete>,
) -> Candidates {
    let actionable = actionable_candidate_counts(&source_chains, &segments);
    let issue52_unregistered_segments = issue52_unregistered_segments(&segments) as i64;
    let segment_rows_merged_away = segment_merge_deletes.len() as i64;
    let turn_rows_deduped_away = turn_dedup_deletes.len() as i64;
    let segment_merge_survivors_updated = segment_merge_groups.len() as i64;
    Candidates {
        candidate_chains: actionable.chains,
        candidate_segments: actionable.segments,
        eligible_segments: actionable.segments,
        blocked_segments: 0,
        issue52_unregistered_segments,
        segment_rows_merged_away,
        turn_rows_deduped_away,
        segment_merge_survivors_updated,
        source_chains,
        segments,
        segment_merge_groups,
        segment_merge_deletes,
        turn_remaps,
        turn_dedup_deletes,
    }
}

struct ActionableCandidateCounts {
    chains: i64,
    segments: i64,
}

fn actionable_candidate_counts(
    source_chains: &[SourceChainCandidate],
    segments: &[CandidateSegment],
) -> ActionableCandidateCounts {
    let orphaned_chains: BTreeSet<&str> = source_chains
        .iter()
        .filter(|source| source.is_orphaned)
        .map(|source| source.chain_id.as_str())
        .collect();
    let mut chains = BTreeSet::new();
    let mut segment_count = 0;
    for segment in segments {
        if orphaned_chains.contains(segment.chain_id.as_str())
            || segment.old_provider_name != segment.new_provider_name
        {
            chains.insert(segment.chain_id.as_str());
            segment_count += 1;
        }
    }
    ActionableCandidateCounts {
        chains: chains.len() as i64,
        segments: segment_count,
    }
}

fn issue52_unregistered_segments(segments: &[CandidateSegment]) -> usize {
    segments
        .iter()
        .filter(|segment| segment.issue52_unregistered)
        .count()
}

fn remapped_segments(segments: &[CandidateSegment]) -> Vec<&CandidateSegment> {
    segments
        .iter()
        .filter(|segment| segment.old_provider_name != segment.new_provider_name)
        .collect()
}

fn build_segment_merge_plan(
    conn: &Connection,
    segments: &[CandidateSegment],
) -> Result<(Vec<SegmentMergeGroup>, Vec<SegmentMergeDelete>), DryRunError> {
    let segment_rows = read_segment_rows(conn, segments)?;
    let mut groups: BTreeMap<(&str, &str, &str), Vec<&CandidateSegment>> = BTreeMap::new();
    for segment in segments {
        groups
            .entry((
                segment.chain_id.as_str(),
                segment.new_provider_name.as_str(),
                segment.session_id.as_str(),
            ))
            .or_default()
            .push(segment);
    }

    let mut merge_groups = Vec::new();
    let mut merge_deletes = Vec::new();
    for grouped_segments in groups.values().filter(|group| group.len() > 1) {
        let mut rows = grouped_segments
            .iter()
            .map(|segment| {
                segment_rows
                    .get(&segment.segment_id)
                    .ok_or_else(|| DryRunError::new("segment merge plan referenced missing row"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        rows.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then(left.id.cmp(&right.id))
        });
        let earliest = select_segment_merge_start(&rows);
        let survivor = select_segment_merge_survivor(&rows);
        let tail = select_segment_merge_tail(&rows);
        let merged_ended_at = if rows.iter().any(|row| row.ended_at.is_none()) {
            None
        } else {
            tail.ended_at.clone()
        };
        let survivor_id = survivor.id;
        merge_groups.push(SegmentMergeGroup {
            survivor_segment_id: survivor_id,
            expected_chain_id: survivor.chain_id.clone(),
            expected_provider_name: survivor.provider_name.clone(),
            expected_session_id: survivor.session_id.clone(),
            expected_started_at: survivor.started_at.clone(),
            expected_ended_at: survivor.ended_at.clone(),
            expected_last_turn_id: survivor.last_turn_id.clone(),
            expected_transition_reason: survivor.transition_reason.clone(),
            merged_started_at: earliest.started_at.clone(),
            merged_ended_at,
            merged_last_turn_id: tail.last_turn_id.clone(),
            merged_transition_reason: survivor.transition_reason.clone(),
        });
        for row in rows {
            if row.id != survivor_id {
                merge_deletes.push(SegmentMergeDelete {
                    segment_id: row.id,
                    survivor_segment_id: survivor_id,
                    expected_chain_id: row.chain_id.clone(),
                    expected_provider_name: row.provider_name.clone(),
                    expected_session_id: row.session_id.clone(),
                    expected_started_at: row.started_at.clone(),
                    expected_ended_at: row.ended_at.clone(),
                    expected_last_turn_id: row.last_turn_id.clone(),
                    expected_transition_reason: row.transition_reason.clone(),
                });
            }
        }
    }
    Ok((merge_groups, merge_deletes))
}

fn select_segment_merge_survivor<'a>(rows: &[&'a SegmentRow]) -> &'a SegmentRow {
    rows.iter()
        .copied()
        .max_by(|left, right| {
            left.ended_at
                .is_none()
                .cmp(&right.ended_at.is_none())
                .then_with(|| compare_segment_latest(left, right))
        })
        .expect("colliding segment group is nonempty after filter")
}

fn select_segment_merge_start<'a>(rows: &[&'a SegmentRow]) -> &'a SegmentRow {
    rows.iter()
        .copied()
        .min_by(|left, right| compare_segment_latest(left, right))
        .expect("colliding segment group is nonempty after filter")
}

fn select_segment_merge_tail<'a>(rows: &[&'a SegmentRow]) -> &'a SegmentRow {
    rows.iter()
        .copied()
        .max_by(|left, right| compare_segment_latest(left, right))
        .expect("colliding segment group is nonempty after filter")
}

fn compare_segment_latest(left: &SegmentRow, right: &SegmentRow) -> std::cmp::Ordering {
    left.started_at
        .cmp(&right.started_at)
        .then(left.id.cmp(&right.id))
}

fn read_segment_rows(
    conn: &Connection,
    segments: &[CandidateSegment],
) -> Result<BTreeMap<i64, SegmentRow>, DryRunError> {
    let mut rows = BTreeMap::new();
    let mut stmt = conn.prepare(
        "SELECT id, chain_id, provider_name, session_id, started_at, ended_at,
                last_turn_id, transition_reason
         FROM session_chain_segments
         WHERE id = ?1",
    )?;
    for segment in segments {
        let row = stmt.query_row([segment.segment_id], |row| {
            Ok(SegmentRow {
                id: row.get(0)?,
                chain_id: row.get(1)?,
                provider_name: row.get(2)?,
                session_id: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
                last_turn_id: row.get(6)?,
                transition_reason: row.get(7)?,
            })
        })?;
        rows.insert(row.id, row);
    }
    Ok(rows)
}

fn build_turn_dedup_plan(
    conn: &Connection,
    turn_remaps: &[SessionTurnRemap],
) -> Result<Vec<TurnDedupDelete>, DryRunError> {
    let turns = read_candidate_turn_rows(conn, turn_remaps)?;
    let mut groups: BTreeMap<(&str, &str, &str), Vec<&TurnRow>> = BTreeMap::new();
    for turn in &turns {
        groups
            .entry((
                turn.new_provider_name.as_str(),
                turn.session_id.as_str(),
                turn.content.turn_id.as_str(),
            ))
            .or_default()
            .push(turn);
    }

    let mut deletes = Vec::new();
    for group in groups.values().filter(|group| group.len() > 1) {
        let first = group[0].content.intrinsic_content_key();
        if group
            .iter()
            .any(|turn| turn.content.intrinsic_content_key() != first)
        {
            return Err(DryRunError::new(format!(
                "divergent session_turns collision for session_id={} turn_id={}",
                group[0].session_id, group[0].content.turn_id
            )));
        }
        let winner = group
            .iter()
            .min_by_key(|turn| turn.id)
            .expect("colliding turn group is nonempty after filter");
        for loser in group.iter().filter(|turn| turn.id != winner.id) {
            deletes.push(TurnDedupDelete {
                loser_turn_row_id: loser.id,
                winner_turn_row_id: winner.id,
                old_provider_name: loser.old_provider_name.clone(),
                new_provider_name: loser.new_provider_name.clone(),
                session_id: loser.session_id.clone(),
                turn_id: loser.content.turn_id.clone(),
            });
        }
    }
    Ok(deletes)
}

fn build_session_turn_remaps(
    conn: &Connection,
    segments: &[CandidateSegment],
    target: &TargetResolution,
) -> Result<Vec<SessionTurnRemap>, DryRunError> {
    let migrated_sessions: BTreeSet<&str> = segments
        .iter()
        .filter(|segment| segment.new_provider_name == target.canonical_provider_name)
        .map(|segment| segment.session_id.as_str())
        .collect();
    let inventory = provider_inventory(target);
    let provider_pattern = target_provider_pattern();
    let mut stmt = conn.prepare(
        "SELECT id, provider_name, session_id, turn_id
         FROM session_turns
         WHERE session_id = ?1
           AND lower(provider_name) LIKE ?2
         ORDER BY id",
    )?;
    let mut remaps = BTreeMap::new();
    for session_id in migrated_sessions {
        let rows = stmt.query_map(params![session_id, provider_pattern.as_str()], |row| {
            Ok(SessionTurnRemap {
                turn_row_id: row.get(0)?,
                old_provider_name: row.get(1)?,
                session_id: row.get(2)?,
                turn_id: row.get(3)?,
                new_provider_name: target.canonical_provider_name.clone(),
            })
        })?;
        for remap in rows.collect::<Result<Vec<_>, _>>()? {
            if remap.old_provider_name != target.canonical_provider_name
                && !inventory.contains(&remap.old_provider_name)
            {
                remaps.insert(remap.turn_row_id, remap);
            }
        }
    }
    Ok(remaps.into_values().collect())
}

fn read_candidate_turn_rows(
    conn: &Connection,
    turn_remaps: &[SessionTurnRemap],
) -> Result<Vec<TurnRow>, DryRunError> {
    let mut remap_stmt = conn.prepare(
        "SELECT id, turn_id, timestamp, role, body
         FROM session_turns
         WHERE id = ?1
         ORDER BY id",
    )?;
    let mut canonical_stmt = conn.prepare(
        "SELECT id, turn_id, timestamp, role, body
         FROM session_turns
         WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3
         ORDER BY id",
    )?;
    let mut rows = BTreeMap::new();
    for remap in turn_remaps {
        let turn = remap_stmt.query_row([remap.turn_row_id], |row| {
            read_turn_row(
                row,
                remap.old_provider_name.as_str(),
                remap.new_provider_name.as_str(),
                remap.session_id.as_str(),
            )
        })?;
        let canonical_rows = canonical_stmt.query_map(
            params![
                remap.new_provider_name.as_str(),
                remap.session_id.as_str(),
                remap.turn_id.as_str()
            ],
            |row| {
                read_turn_row(
                    row,
                    remap.new_provider_name.as_str(),
                    remap.new_provider_name.as_str(),
                    remap.session_id.as_str(),
                )
            },
        )?;
        for canonical in canonical_rows.collect::<Result<Vec<_>, _>>()? {
            rows.insert(canonical.id, canonical);
        }
        rows.insert(turn.id, turn);
    }
    Ok(rows.into_values().collect())
}

fn read_turn_row(
    row: &rusqlite::Row<'_>,
    old_provider_name: &str,
    new_provider_name: &str,
    session_id: &str,
) -> Result<TurnRow, rusqlite::Error> {
    Ok(TurnRow {
        id: row.get(0)?,
        old_provider_name: old_provider_name.to_string(),
        new_provider_name: new_provider_name.to_string(),
        session_id: session_id.to_string(),
        content: TurnContent {
            turn_id: row.get(1)?,
            timestamp: row.get(2)?,
            role: row.get(3)?,
            body: row.get(4)?,
        },
    })
}

fn moved_provider_token() -> String {
    ["cla", "ude"].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_config::{ProviderConfig, SessionStorage};
    use std::fs;
    use std::path::{Path, PathBuf};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn fixture_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY,
                model_name TEXT,
                provider_name TEXT,
                session_id TEXT,
                provider_session_id TEXT,
                created_at TEXT NOT NULL
            );",
        )
        .unwrap();
        conn
    }

    fn provider_ref_models() -> BTreeSet<String> {
        [model_name("aaa"), model_name("mmm"), model_name("zzz")]
            .into_iter()
            .collect()
    }

    fn provider_token() -> String {
        ["cla", "ude"].concat()
    }

    fn model_name(prefix: &str) -> String {
        format!("{prefix}-ref-{}", provider_token())
    }

    fn shadow_model_name() -> String {
        format!("shadow-{}", provider_token())
    }

    fn transcript_provider(projects_dir: PathBuf) -> ProviderConfig {
        let mut provider =
            ProviderConfig::new(format!("agent-runner-{}", provider_token()), vec![]);
        provider.session_storage = Some(transcript_storage(projects_dir));
        provider
    }

    fn transcript_storage(projects_dir: PathBuf) -> SessionStorage {
        toml::from_str(&format!(
            "kind = {:?}\nprojects_dir = {:?}\n",
            format!("{}_code", provider_token()),
            projects_dir
        ))
        .unwrap()
    }

    fn assistant_model_line(model: &str) -> String {
        format!(r#"{{"type":"assistant","message":{{"model":{model:?}}}}}"#)
    }

    fn write_transcript(projects_dir: &Path, session_id: &str, lines: &[String]) -> PathBuf {
        let project_dir = projects_dir.join("synthetic-project");
        fs::create_dir_all(&project_dir).unwrap();
        let path = project_dir.join(format!("{session_id}.jsonl"));
        fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
        path
    }

    fn infer_from_transcript(
        provider: &ProviderConfig,
        segments: &[&str],
        models: &BTreeSet<String>,
        backfill_default_model_name: &str,
    ) -> Option<String> {
        let segment_session_ids = segments
            .iter()
            .map(|session_id| (*session_id).to_string())
            .collect::<Vec<_>>();
        infer_chain_target_model_from_transcript(
            provider,
            &segment_session_ids,
            models,
            backfill_default_model_name,
        )
    }

    fn insert_invocation(
        conn: &Connection,
        model_name: Option<&str>,
        session_id: &str,
        provider_session_id: Option<&str>,
        created_at: &str,
    ) {
        conn.execute(
            "INSERT INTO invocations
             (model_name, provider_name, session_id, provider_session_id, created_at)
             VALUES (?1, 'provider-fixture', ?2, ?3, ?4)",
            params![model_name, session_id, provider_session_id, created_at],
        )
        .unwrap();
    }

    fn infer(conn: &Connection, segments: &[&str]) -> Option<String> {
        let segment_session_ids = segments
            .iter()
            .map(|session_id| (*session_id).to_string())
            .collect::<Vec<_>>();
        infer_chain_target_model(conn, &segment_session_ids, &provider_ref_models()).unwrap()
    }

    #[test]
    fn infer_chain_target_model_from_transcript_dominant_middle_model_wins() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let target = model_name("aaa");
        let middle = model_name("mmm");
        write_transcript(
            &projects_dir,
            "segment-a",
            &[
                assistant_model_line(&middle),
                assistant_model_line(&middle),
                assistant_model_line(&target),
            ],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a"], &models, &target),
            Some(middle)
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_dominant_last_model_wins() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let target = model_name("aaa");
        let last = model_name("zzz");
        write_transcript(
            &projects_dir,
            "segment-a",
            &[
                assistant_model_line(&target),
                assistant_model_line(&last),
                assistant_model_line(&last),
            ],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a"], &models, &target),
            Some(last)
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_only_default_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let target = model_name("aaa");
        write_transcript(
            &projects_dir,
            "segment-a",
            &[assistant_model_line(&target), assistant_model_line(&target)],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a"], &models, &target),
            None
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_uses_recorded_backfill_default() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let global_default = models.iter().next().unwrap().clone();
        let recorded_default = model_name("mmm");
        write_transcript(
            &projects_dir,
            "segment-global-default",
            &[
                assistant_model_line(&global_default),
                assistant_model_line(&global_default),
                assistant_model_line(&recorded_default),
            ],
        );
        write_transcript(
            &projects_dir,
            "segment-recorded-default",
            &[assistant_model_line(&recorded_default)],
        );

        assert_eq!(
            infer_from_transcript(
                &provider,
                &["segment-global-default"],
                &models,
                &recorded_default,
            ),
            Some(global_default)
        );
        assert_eq!(
            infer_from_transcript(
                &provider,
                &["segment-recorded-default"],
                &models,
                &recorded_default,
            ),
            None
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_missing_synthetic_and_absent_models_return_none() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let default_model = models.iter().next().unwrap().clone();
        write_transcript(
            &projects_dir,
            "synthetic-only",
            &[assistant_model_line("<synthetic>")],
        );
        write_transcript(
            &projects_dir,
            "absent-only",
            &[
                r#"{"type":"assistant","message":{}}"#.to_string(),
                r#"{"type":"assistant","message":{"model":42}}"#.to_string(),
                format!(
                    r#"{{"type":"user","message":{{"model":{:?}}}}}"#,
                    model_name("mmm")
                ),
            ],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["missing"], &models, &default_model),
            None
        );
        assert_eq!(
            infer_from_transcript(&provider, &["synthetic-only"], &models, &default_model),
            None
        );
        assert_eq!(
            infer_from_transcript(&provider, &["absent-only"], &models, &default_model),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn infer_chain_target_model_from_transcript_unreadable_file_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let default_model = models.iter().next().unwrap().clone();
        let middle = model_name("mmm");
        let path = write_transcript(&projects_dir, "segment-a", &[assistant_model_line(&middle)]);
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&path, permissions).unwrap();

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a"], &models, &default_model),
            None
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_tie_uses_lexicographically_smallest() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let target = model_name("aaa");
        let middle = model_name("mmm");
        let last = model_name("zzz");
        write_transcript(
            &projects_dir,
            "segment-a",
            &[assistant_model_line(&last), assistant_model_line(&middle)],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a"], &models, &target),
            Some(middle)
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_middle_dominates_default_noise() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let target = model_name("aaa");
        let middle = model_name("mmm");
        write_transcript(
            &projects_dir,
            "segment-a",
            &[
                assistant_model_line(&middle),
                assistant_model_line(&middle),
                assistant_model_line(&middle),
                assistant_model_line(&middle),
                assistant_model_line(&middle),
                assistant_model_line(&target),
                assistant_model_line(&target),
            ],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a"], &models, &target),
            Some(middle)
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_prefix_matches_inventory_model() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let default_model = models.iter().next().unwrap().clone();
        let middle = model_name("mmm");
        let prefixed_middle = format!("{middle}-4-8");
        write_transcript(
            &projects_dir,
            "segment-a",
            &[assistant_model_line(&prefixed_middle)],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a"], &models, &default_model),
            Some(middle)
        );
    }

    #[test]
    fn infer_chain_target_model_from_transcript_aggregates_multiple_segment_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let projects_dir = temp.path().join("projects");
        let provider = transcript_provider(projects_dir.clone());
        let models = provider_ref_models();
        let target = model_name("aaa");
        let middle = model_name("mmm");
        write_transcript(&projects_dir, "segment-a", &[assistant_model_line(&middle)]);
        write_transcript(
            &projects_dir,
            "segment-b",
            &[assistant_model_line(&middle), assistant_model_line(&target)],
        );

        assert_eq!(
            infer_from_transcript(&provider, &["segment-a", "segment-b"], &models, &target),
            Some(middle)
        );
    }

    #[test]
    fn infer_chain_target_model_dominant_middle_model_wins() {
        let conn = fixture_conn();
        let middle = model_name("mmm");
        let last = model_name("zzz");
        insert_invocation(
            &conn,
            Some(&middle),
            "segment-a",
            None,
            "2026-06-20T10:00:00Z",
        );
        insert_invocation(
            &conn,
            Some(&middle),
            "segment-a",
            None,
            "2026-06-20T10:01:00Z",
        );
        insert_invocation(
            &conn,
            Some(&last),
            "segment-a",
            None,
            "2026-06-20T10:02:00Z",
        );

        assert_eq!(infer(&conn, &["segment-a"]), Some(middle));
    }

    #[test]
    fn infer_chain_target_model_dominant_last_model_wins() {
        let conn = fixture_conn();
        let target = model_name("aaa");
        let last = model_name("zzz");
        insert_invocation(
            &conn,
            Some(&target),
            "segment-a",
            None,
            "2026-06-20T10:00:00Z",
        );
        insert_invocation(
            &conn,
            Some(&last),
            "segment-a",
            None,
            "2026-06-20T10:01:00Z",
        );
        insert_invocation(
            &conn,
            Some(&last),
            "segment-a",
            None,
            "2026-06-20T10:02:00Z",
        );

        assert_eq!(infer(&conn, &["segment-a"]), Some(last));
    }

    #[test]
    fn infer_chain_target_model_returns_none_without_in_family_evidence() {
        let conn = fixture_conn();
        insert_invocation(
            &conn,
            Some("<unknown>"),
            "segment-a",
            None,
            "2026-06-20T10:00:00Z",
        );
        insert_invocation(
            &conn,
            Some(&shadow_model_name()),
            "segment-a",
            None,
            "2026-06-20T10:01:00Z",
        );
        insert_invocation(&conn, None, "segment-a", None, "2026-06-20T10:02:00Z");
        insert_invocation(
            &conn,
            Some(&model_name("mmm")),
            "other",
            None,
            "2026-06-20T10:03:00Z",
        );

        assert_eq!(infer(&conn, &["segment-a"]), None);
    }

    #[test]
    fn infer_chain_target_model_count_tie_uses_latest_created_at() {
        let conn = fixture_conn();
        let middle = model_name("mmm");
        let last = model_name("zzz");
        insert_invocation(
            &conn,
            Some(&middle),
            "segment-a",
            None,
            "2026-06-20T10:05:00Z",
        );
        insert_invocation(
            &conn,
            Some(&last),
            "segment-a",
            None,
            "2026-06-20T10:04:00Z",
        );

        assert_eq!(infer(&conn, &["segment-a"]), Some(middle));
    }

    #[test]
    fn infer_chain_target_model_exact_tie_uses_lexicographically_smallest() {
        let conn = fixture_conn();
        let middle = model_name("mmm");
        let last = model_name("zzz");
        insert_invocation(
            &conn,
            Some(&last),
            "segment-a",
            None,
            "2026-06-20T10:05:00Z",
        );
        insert_invocation(
            &conn,
            Some(&middle),
            "segment-a",
            None,
            "2026-06-20T10:05:00Z",
        );

        assert_eq!(infer(&conn, &["segment-a"]), Some(middle));
    }

    #[test]
    fn infer_chain_target_model_ignores_unknown_null_and_out_of_family_models() {
        let conn = fixture_conn();
        let middle = model_name("mmm");
        insert_invocation(
            &conn,
            Some("<unknown>"),
            "segment-a",
            None,
            "2026-06-20T10:00:00Z",
        );
        insert_invocation(&conn, None, "segment-a", None, "2026-06-20T10:01:00Z");
        insert_invocation(
            &conn,
            Some(&shadow_model_name()),
            "segment-a",
            None,
            "2026-06-20T10:02:00Z",
        );
        insert_invocation(
            &conn,
            Some(&middle),
            "segment-a",
            None,
            "2026-06-20T10:03:00Z",
        );

        assert_eq!(infer(&conn, &["segment-a"]), Some(middle));
    }

    #[test]
    fn infer_chain_target_model_joins_by_provider_session_id_when_present() {
        let conn = fixture_conn();
        let middle = model_name("mmm");
        insert_invocation(
            &conn,
            Some(&middle),
            "non-matching-session-id",
            Some("segment-provider-id"),
            "2026-06-20T10:00:00Z",
        );

        assert_eq!(infer(&conn, &["segment-provider-id"]), Some(middle));
    }

    #[test]
    fn infer_chain_target_model_falls_back_to_session_id_when_provider_session_id_is_null() {
        let conn = fixture_conn();
        let last = model_name("zzz");
        insert_invocation(
            &conn,
            Some(&last),
            "segment-session-id",
            None,
            "2026-06-20T10:00:00Z",
        );

        assert_eq!(infer(&conn, &["segment-session-id"]), Some(last));
    }
}
