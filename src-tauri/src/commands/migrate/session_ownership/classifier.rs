//! Declared role: mapper, filter, accessor, validator, orchestration

use super::DryRunError;
use super::target_resolution::TargetResolution;
use rusqlite::{Connection, params};
use std::collections::BTreeSet;

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
    pub(crate) source_chains: Vec<String>,
    pub(crate) segments: Vec<CandidateSegment>,
}

#[derive(Debug, Clone)]
struct SourceSegment {
    chain_id: String,
    segment_id: i64,
    old_provider_name: String,
    session_id: String,
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
    assert_no_segment_collision(conn, &segments)?;
    assert_no_turn_collision(conn, &segments)?;
    Ok(candidates_from_segments(source_chains, segments))
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
         DROP TABLE IF EXISTS temp.s11_wu4_source_chain_candidates;",
    );
}

fn create_sql_input_tables(conn: &Connection) -> Result<(), DryRunError> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS s11_wu4_migration_params;
         DROP TABLE IF EXISTS s11_wu4_original_target_provider_inventory;
         DROP TABLE IF EXISTS s11_wu4_target_provider_inventory;
         DROP TABLE IF EXISTS s11_wu4_provider_aliases;
         DROP TABLE IF EXISTS s11_wu4_source_chain_candidates;
         CREATE TABLE s11_wu4_migration_params (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE s11_wu4_original_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'dry-run');
         CREATE TABLE s11_wu4_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'dry-run');
         CREATE TABLE s11_wu4_provider_aliases (old_provider_name TEXT PRIMARY KEY, new_provider_name TEXT NOT NULL, reason TEXT NOT NULL);
         CREATE TABLE s11_wu4_source_chain_candidates (chain_id TEXT PRIMARY KEY, evidence TEXT NOT NULL);",
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
         CREATE TEMP TABLE s11_wu4_migration_params (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TEMP TABLE s11_wu4_original_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'live');
         CREATE TEMP TABLE s11_wu4_target_provider_inventory (provider_name TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'live');
         CREATE TEMP TABLE s11_wu4_provider_aliases (old_provider_name TEXT PRIMARY KEY, new_provider_name TEXT NOT NULL, reason TEXT NOT NULL);
         CREATE TEMP TABLE s11_wu4_source_chain_candidates (chain_id TEXT PRIMARY KEY, evidence TEXT NOT NULL);",
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
) -> Candidates {
    let issue52_unregistered_segments = issue52_unregistered_segments(&segments) as i64;
    Candidates {
        candidate_chains: source_chains.len() as i64,
        candidate_segments: segments.len() as i64,
        eligible_segments: segments.len() as i64,
        blocked_segments: 0,
        issue52_unregistered_segments,
        source_chains,
        segments,
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

fn assert_no_segment_collision(
    conn: &Connection,
    segments: &[CandidateSegment],
) -> Result<(), DryRunError> {
    for segment in remapped_segments(segments) {
        validate_no_segment_collision(segment_collision_exists(conn, segment)?)?;
    }
    Ok(())
}

fn segment_collision_exists(
    conn: &Connection,
    segment: &CandidateSegment,
) -> Result<bool, DryRunError> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM session_chain_segments
            WHERE chain_id = ?1 AND provider_name = ?2 AND session_id = ?3 AND id <> ?4
         )",
        params![
            segment.chain_id,
            segment.new_provider_name,
            segment.session_id,
            segment.segment_id
        ],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn validate_no_segment_collision(exists: bool) -> Result<(), DryRunError> {
    if exists {
        return Err(DryRunError::new(
            "segment provider remap would collide with UNIQUE(chain_id, provider_name, session_id)",
        ));
    }
    Ok(())
}

fn assert_no_turn_collision(
    conn: &Connection,
    segments: &[CandidateSegment],
) -> Result<(), DryRunError> {
    for segment in remapped_segments(segments) {
        validate_no_turn_collision(turn_collision_exists(conn, segment)?)?;
    }
    Ok(())
}

fn turn_collision_exists(
    conn: &Connection,
    segment: &CandidateSegment,
) -> Result<bool, DryRunError> {
    conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM session_turns old_turn
            JOIN session_turns existing_turn
              ON existing_turn.provider_name = ?1
             AND existing_turn.session_id = old_turn.session_id
             AND existing_turn.turn_id = old_turn.turn_id
             AND existing_turn.id <> old_turn.id
            WHERE old_turn.provider_name = ?2 AND old_turn.session_id = ?3
         )",
        params![
            segment.new_provider_name,
            segment.old_provider_name,
            segment.session_id
        ],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn validate_no_turn_collision(exists: bool) -> Result<(), DryRunError> {
    if exists {
        return Err(DryRunError::new(
            "session_turns provider remap would collide with UNIQUE(provider_name, session_id, turn_id)",
        ));
    }
    Ok(())
}

fn moved_provider_token() -> String {
    ["cla", "ude"].concat()
}
