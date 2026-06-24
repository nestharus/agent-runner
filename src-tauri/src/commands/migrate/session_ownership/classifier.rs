//! Declared role: mapper, filter, accessor, validator, orchestration

use super::target_resolution::TargetResolution;
use super::DryRunError;
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, BTreeSet};

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
    pub(crate) source_chains: Vec<String>,
    pub(crate) segments: Vec<CandidateSegment>,
    pub(crate) segment_merge_groups: Vec<SegmentMergeGroup>,
    pub(crate) segment_merge_deletes: Vec<SegmentMergeDelete>,
    pub(crate) turn_dedup_deletes: Vec<TurnDedupDelete>,
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
    source_chains: Vec<&'a str>,
    provider_aliases: Vec<ProviderAliasRow<'a>>,
    segment_merge_groups: &'a [SegmentMergeGroup],
    segment_merge_deletes: &'a [SegmentMergeDelete],
    turn_dedup_deletes: &'a [TurnDedupDelete],
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
    let source_chains = source_chain_candidates(conn, &target.model_name)?;
    let source_segments = read_source_segments(conn, &source_chains)?;
    let partitions = partition_segments_by_inventory(source_segments, &provider_inventory(target));
    let segments = candidate_segments(partitions, &target.canonical_provider_name);
    let (segment_merge_groups, segment_merge_deletes) = build_segment_merge_plan(conn, &segments)?;
    let turn_dedup_deletes = build_turn_dedup_plan(conn, &segments)?;
    Ok(candidates_from_segments(
        source_chains,
        segments,
        segment_merge_groups,
        segment_merge_deletes,
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

pub(crate) fn cleanup_temp_sql_inputs(conn: &Connection) {
    let _ = conn.execute_batch(
        "DROP TABLE IF EXISTS temp.s11_wu4_migration_params;
         DROP TABLE IF EXISTS temp.s11_wu4_original_target_provider_inventory;
         DROP TABLE IF EXISTS temp.s11_wu4_target_provider_inventory;
         DROP TABLE IF EXISTS temp.s11_wu4_provider_aliases;
         DROP TABLE IF EXISTS temp.s11_wu4_source_chain_candidates;
         DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_groups;
         DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_deletes;
         DROP TABLE IF EXISTS temp.s11_wu4_turn_dedup_deletes;",
    );
}

fn create_sql_input_tables(conn: &Connection) -> Result<(), DryRunError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS s11_wu4_migration_params;
         DROP TABLE IF EXISTS s11_wu4_original_target_provider_inventory;
         DROP TABLE IF EXISTS s11_wu4_target_provider_inventory;
         DROP TABLE IF EXISTS s11_wu4_provider_aliases;
         DROP TABLE IF EXISTS s11_wu4_source_chain_candidates;
         DROP TABLE IF EXISTS s11_wu4_segment_merge_groups;
         DROP TABLE IF EXISTS s11_wu4_segment_merge_deletes;
         DROP TABLE IF EXISTS s11_wu4_turn_dedup_deletes;
         CREATE TABLE s11_wu4_migration_params (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE s11_wu4_original_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'dry-run');
         CREATE TABLE s11_wu4_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'dry-run');
         CREATE TABLE s11_wu4_provider_aliases (old_provider_name TEXT PRIMARY KEY, new_provider_name TEXT NOT NULL, reason TEXT NOT NULL);
         CREATE TABLE s11_wu4_source_chain_candidates (chain_id TEXT PRIMARY KEY, evidence TEXT NOT NULL);
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
         DROP TABLE IF EXISTS temp.s11_wu4_provider_aliases;
         DROP TABLE IF EXISTS temp.s11_wu4_source_chain_candidates;
         DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_groups;
         DROP TABLE IF EXISTS temp.s11_wu4_segment_merge_deletes;
         DROP TABLE IF EXISTS temp.s11_wu4_turn_dedup_deletes;
         CREATE TEMP TABLE s11_wu4_migration_params (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TEMP TABLE s11_wu4_original_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'live');
         CREATE TEMP TABLE s11_wu4_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'live');
         CREATE TEMP TABLE s11_wu4_provider_aliases (old_provider_name TEXT PRIMARY KEY, new_provider_name TEXT NOT NULL, reason TEXT NOT NULL);
         CREATE TEMP TABLE s11_wu4_source_chain_candidates (chain_id TEXT PRIMARY KEY, evidence TEXT NOT NULL);
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
    for chain_id in &rows.source_chains {
        conn.execute(
            "INSERT INTO s11_wu4_source_chain_candidates(chain_id, evidence) VALUES (?1, 'copied-state')",
            [chain_id],
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
        source_chains: source_chain_rows(candidates),
        provider_aliases: provider_alias_rows(&remapped_segments(&candidates.segments)),
        segment_merge_groups: &candidates.segment_merge_groups,
        segment_merge_deletes: &candidates.segment_merge_deletes,
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
        ("migration_id", "s11-m2-session-ownership".to_string()),
    ]
}

fn provider_inventory_rows(target: &TargetResolution) -> Vec<&str> {
    target.inventory.iter().map(String::as_str).collect()
}

fn source_chain_rows(candidates: &Candidates) -> Vec<&str> {
    candidates
        .source_chains
        .iter()
        .map(String::as_str)
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
    target_model: &str,
) -> Result<Vec<String>, DryRunError> {
    read_source_chain_candidates(conn, target_model, &target_provider_pattern())
}

fn read_source_chain_candidates(
    conn: &Connection,
    target_model: &str,
    provider_pattern: &str,
) -> Result<Vec<String>, DryRunError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT c.chain_id
         FROM session_chains c
         LEFT JOIN session_chain_segments s ON s.chain_id = c.chain_id
         WHERE c.model_name <> ?1
           AND (lower(c.model_name) LIKE ?2 OR lower(s.provider_name) LIKE ?2)
         ORDER BY c.chain_id",
    )?;
    let rows = stmt.query_map(params![target_model, provider_pattern], |row| row.get(0))?;
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

fn read_source_segments(
    conn: &Connection,
    source_chains: &[String],
) -> Result<Vec<SourceSegment>, DryRunError> {
    let mut segments = Vec::new();
    for chain_id in source_chains {
        segments.extend(read_source_segments_for_chain(conn, chain_id)?);
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
    source_chains: Vec<String>,
    segments: Vec<CandidateSegment>,
    segment_merge_groups: Vec<SegmentMergeGroup>,
    segment_merge_deletes: Vec<SegmentMergeDelete>,
    turn_dedup_deletes: Vec<TurnDedupDelete>,
) -> Candidates {
    let issue52_unregistered_segments = issue52_unregistered_segments(&segments) as i64;
    let segment_rows_merged_away = segment_merge_deletes.len() as i64;
    let turn_rows_deduped_away = turn_dedup_deletes.len() as i64;
    let segment_merge_survivors_updated = segment_merge_groups.len() as i64;
    Candidates {
        candidate_chains: source_chains.len() as i64,
        candidate_segments: segments.len() as i64,
        eligible_segments: segments.len() as i64,
        blocked_segments: 0,
        issue52_unregistered_segments,
        segment_rows_merged_away,
        turn_rows_deduped_away,
        segment_merge_survivors_updated,
        source_chains,
        segments,
        segment_merge_groups,
        segment_merge_deletes,
        turn_dedup_deletes,
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
        let earliest = rows
            .first()
            .expect("colliding segment group is nonempty after filter");
        let latest = rows
            .last()
            .expect("colliding segment group is nonempty after filter");
        let latest_id = latest.id;
        merge_groups.push(SegmentMergeGroup {
            survivor_segment_id: latest_id,
            expected_chain_id: latest.chain_id.clone(),
            expected_provider_name: latest.provider_name.clone(),
            expected_session_id: latest.session_id.clone(),
            expected_started_at: latest.started_at.clone(),
            expected_ended_at: latest.ended_at.clone(),
            expected_last_turn_id: latest.last_turn_id.clone(),
            expected_transition_reason: latest.transition_reason.clone(),
            merged_started_at: earliest.started_at.clone(),
            merged_ended_at: latest.ended_at.clone(),
            merged_last_turn_id: latest.last_turn_id.clone(),
            merged_transition_reason: latest.transition_reason.clone(),
        });
        for row in rows {
            if row.id != latest_id {
                merge_deletes.push(SegmentMergeDelete {
                    segment_id: row.id,
                    survivor_segment_id: latest_id,
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
    segments: &[CandidateSegment],
) -> Result<Vec<TurnDedupDelete>, DryRunError> {
    let turns = read_candidate_turn_rows(conn, segments)?;
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

fn read_candidate_turn_rows(
    conn: &Connection,
    segments: &[CandidateSegment],
) -> Result<Vec<TurnRow>, DryRunError> {
    let mut owner_remaps = BTreeMap::new();
    for segment in segments {
        owner_remaps.insert(
            (
                segment.old_provider_name.as_str(),
                segment.session_id.as_str(),
            ),
            segment.new_provider_name.as_str(),
        );
    }
    let mut owner_stmt = conn.prepare(
        "SELECT id, turn_id, timestamp, role, body
         FROM session_turns
         WHERE provider_name = ?1 AND session_id = ?2
         ORDER BY id",
    )?;
    let mut canonical_stmt = conn.prepare(
        "SELECT id, turn_id, timestamp, role, body
         FROM session_turns
         WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3
         ORDER BY id",
    )?;
    let mut rows = BTreeMap::new();
    for ((old_provider_name, session_id), new_provider_name) in owner_remaps {
        let turn_rows = owner_stmt.query_map(params![old_provider_name, session_id], |row| {
            read_turn_row(row, old_provider_name, new_provider_name, session_id)
        })?;
        for turn in turn_rows.collect::<Result<Vec<_>, _>>()? {
            if old_provider_name != new_provider_name {
                let canonical_rows = canonical_stmt.query_map(
                    params![new_provider_name, session_id, turn.content.turn_id.as_str()],
                    |row| read_turn_row(row, new_provider_name, new_provider_name, session_id),
                )?;
                for canonical in canonical_rows.collect::<Result<Vec<_>, _>>()? {
                    rows.insert(canonical.id, canonical);
                }
            }
            rows.insert(turn.id, turn);
        }
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
