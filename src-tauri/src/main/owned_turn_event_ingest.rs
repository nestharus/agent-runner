//! ## Declared roles
//! parser, mapper, orchestration, accessor, formatter, predicate
//!
//! Provider JSONL ingestion for the Oulipoly-owned turn/event boundary.
//!
//! ## Adapter declarations
//! adapter_declarations:
//!   - component: src-tauri/src/main/owned_turn_event_ingest.rs
//!     role: adapter
//!     Translates:
//!       - provider JSONL compact-summary transcript fields
//!       - provider transcript storage path fallback
//!       - oulipoly_state::OwnedTurnEventRow
//!       - oulipoly_state::StateDb owned turn/event facade

use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_state::{CompactSummaryEvidence, OwnedTurnEventRow, StateDb};
use serde_json::Value;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};

pub(crate) type IoError = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderCompactRecord {
    turn_uuid: String,
    is_compaction_boundary: bool,
    summary_metadata_json: Option<String>,
}

pub(crate) fn parse_provider_jsonl_compact_summary_records(
    jsonl: &Path,
) -> Result<Vec<ProviderCompactRecord>, IoError> {
    read_compaction_jsonl_lines(jsonl).map(|lines| {
        lines
            .iter()
            .filter_map(|line| parse_provider_compact_record(line))
            .collect()
    })
}

pub(crate) fn map_provider_record_to_owned_turn_event_row(
    record: &ProviderCompactRecord,
    session_id: &str,
) -> OwnedTurnEventRow {
    OwnedTurnEventRow {
        session_id: session_id.to_string(),
        turn_uuid: record.turn_uuid.clone(),
        is_compaction_boundary: record.is_compaction_boundary,
        summary_metadata_json: record.summary_metadata_json.clone(),
    }
}

pub(crate) fn ingest_owned_turn_event_rows(
    state: &StateDb,
    _provider_name: &str,
    session_id: &str,
    jsonl: &Path,
) -> Result<usize, IoError> {
    let records = parse_provider_jsonl_compact_summary_records(jsonl)?;
    let rows: Vec<OwnedTurnEventRow> = records
        .iter()
        .map(|record| map_provider_record_to_owned_turn_event_row(record, session_id))
        .collect();
    state.insert_owned_turn_event_rows(&rows)
}

#[cfg(test)]
pub(crate) fn flag_compaction_boundaries_from_jsonl(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    path: &Path,
) -> Result<u64, IoError> {
    ingest_owned_turn_event_rows(state, provider_name, session_id, path)?;
    let evidence = state.compact_summary_evidence(session_id)?;
    flag_compaction_boundaries_from_evidence(state, provider_name, &evidence)
}

pub(crate) fn flag_compaction_boundaries_from_evidence(
    state: &StateDb,
    provider_name: &str,
    evidence: &CompactSummaryEvidence,
) -> Result<u64, IoError> {
    let mut flagged = 0u64;
    for turn_uuid in &evidence.compact_turn_uuids {
        if flag_compaction_boundary(state, provider_name, &evidence.session_id, turn_uuid)? {
            flagged += 1;
        }
    }
    Ok(flagged)
}

pub(crate) fn existing_storage_compaction_source_path(
    provider_name: &str,
    session_id: &str,
    models: &HashMap<String, ModelConfig>,
) -> Option<PathBuf> {
    existing_owned_turn_event_source_path(storage_transcript_path(
        matching_storage_providers(provider_name, models),
        session_id,
    )?)
}

pub(crate) fn compact_summary_turn_uuid(obj: &Value) -> Option<String> {
    if !is_compact_summary_json(obj) {
        return None;
    }
    raw_provider_turn_uuid(obj).map(string_from_str)
}

fn parse_provider_compact_record(line: &str) -> Option<ProviderCompactRecord> {
    let obj = parse_compaction_json_line(line)?;
    map_provider_json_to_compact_record(&obj)
}

fn map_provider_json_to_compact_record(obj: &Value) -> Option<ProviderCompactRecord> {
    let compact_turn_id = compact_summary_turn_uuid(obj);
    map_provider_json_parts_to_compact_record(obj, compact_turn_id)
}

fn map_provider_json_parts_to_compact_record(
    obj: &Value,
    compact_turn_id: Option<String>,
) -> Option<ProviderCompactRecord> {
    Some(ProviderCompactRecord {
        turn_uuid: provider_turn_uuid(obj, compact_turn_id)?,
        is_compaction_boundary: is_compact_summary_json(obj),
        summary_metadata_json: compact_summary_metadata_json(obj),
    })
}

fn provider_turn_uuid(obj: &Value, compact_turn_id: Option<String>) -> Option<String> {
    compact_turn_id.or_else(|| raw_provider_turn_uuid(obj).map(string_from_str))
}

fn matching_storage_providers<'a>(
    provider_name: &str,
    models: &'a HashMap<String, ModelConfig>,
) -> Vec<&'a ProviderConfig> {
    models
        .values()
        .flat_map(|model| model.providers.iter())
        .filter(|provider| provider.name == provider_name)
        .collect()
}

fn storage_transcript_path(providers: Vec<&ProviderConfig>, session_id: &str) -> Option<PathBuf> {
    providers.into_iter().find_map(|provider| {
        oulipoly_runtime::migration::find_claude_source_from_storage(provider, session_id)
    })
}

fn existing_owned_turn_event_source_path(path: PathBuf) -> Option<PathBuf> {
    if path.exists() { Some(path) } else { None }
}

fn compact_summary_metadata_json(obj: &Value) -> Option<String> {
    serde_json::to_string(obj).ok()
}

fn read_compaction_jsonl_lines(path: &Path) -> Result<Vec<String>, IoError> {
    let file = open_compaction_source(path)?;
    collect_compaction_jsonl_lines(path, std::io::BufReader::new(file).lines())
}

fn collect_compaction_jsonl_lines<I>(path: &Path, lines: I) -> Result<Vec<String>, IoError>
where
    I: Iterator<Item = Result<String, std::io::Error>>,
{
    lines
        .map(|line| line.map_err(|e| format_compaction_source_line_error(path, e)))
        .collect()
}

fn open_compaction_source(path: &Path) -> Result<std::fs::File, IoError> {
    std::fs::File::open(path)
        .map_err(|e| format!("Failed to open compaction source {}: {e}", path.display()))
}

fn format_compaction_source_line_error(path: &Path, error: std::io::Error) -> IoError {
    format!(
        "Failed to read compaction source line from {}: {error}",
        path.display()
    )
}

fn parse_compaction_json_line(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line).ok()
}

fn is_compact_summary_json(obj: &Value) -> bool {
    compact_summary_bool(obj, "isCompactSummary")
        || compact_summary_bool(obj, "is_compaction_boundary")
}

fn compact_summary_bool(obj: &Value, key: &str) -> bool {
    obj.get(key).and_then(|value| value.as_bool()) == Some(true)
}

fn raw_provider_turn_uuid(obj: &Value) -> Option<&str> {
    obj.get("uuid")
        .or_else(|| obj.get("turn_id"))
        .and_then(|value| value.as_str())
}

fn flag_compaction_boundary(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
    turn_id: &str,
) -> Result<bool, IoError> {
    state.flag_compaction_boundary(provider_name, session_id, turn_id)
}

fn string_from_str(value: &str) -> String {
    value.to_string()
}
