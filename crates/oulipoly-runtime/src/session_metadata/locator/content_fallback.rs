//! ## Declared roles
//! accessor, filter, mapper, parser, predicate, validator
//!
//! AGE-157 content-fallback scan for transcript lookup parity with the
//! `scripts/claude-code-locate-transcript` and `scripts/codex-locate-transcript`
//! shell locators. When filename-derived lookup misses, the scripts defensively
//! parse each JSONL file for a session-id record; these helpers port that
//! behavior into the Rust locator surfaces (provider trait impls + push-based
//! registry resolver).
//!
//! Deliberate divergence from the scripts: the recursive walk is bounded to
//! `MAX_SCAN_DEPTH` (matches the existing codex hydration limit) so an
//! adversarially deep tree cannot induce unbounded I/O. Per-file I/O errors
//! during the fallback are silently skipped, mirroring the scripts'
//! `except OSError: return False` shape.

use super::{LocatorError, TranscriptLookupMode, validate_jsonl_path};
use std::fs::{DirEntry, File, ReadDir};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const MAX_SCAN_DEPTH: usize = 4;

pub(in crate::session_metadata) fn locate_claude_by_content(
    projects_dir: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>, LocatorError> {
    scan_for_content_matches(
        projects_dir,
        session_id,
        is_jsonl_candidate_claude,
        line_indicates_claude_session,
    )
}

pub(in crate::session_metadata) fn locate_codex_by_content(
    sessions_dir: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>, LocatorError> {
    scan_for_content_matches(
        sessions_dir,
        session_id,
        is_jsonl_candidate_codex,
        line_indicates_codex_session,
    )
}

fn scan_for_content_matches(
    root: &Path,
    session_id: &str,
    filename_filter: fn(&str) -> bool,
    line_predicate: fn(&serde_json::Value, &str) -> bool,
) -> Result<Vec<PathBuf>, LocatorError> {
    let mut matches = Vec::new();
    scan_at_depth(
        root,
        session_id,
        filename_filter,
        line_predicate,
        0,
        &mut matches,
    )?;
    Ok(matches)
}

fn scan_at_depth(
    dir: &Path,
    session_id: &str,
    filename_filter: fn(&str) -> bool,
    line_predicate: fn(&serde_json::Value, &str) -> bool,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), LocatorError> {
    if !within_scan_depth(depth) {
        return Ok(());
    }
    let entries = directory_entries(read_directory(dir)?, dir)?;
    for entry in entries {
        visit_entry(
            entry,
            session_id,
            filename_filter,
            line_predicate,
            depth,
            matches,
        )?;
    }
    Ok(())
}

fn within_scan_depth(depth: usize) -> bool {
    depth <= MAX_SCAN_DEPTH
}

fn visit_entry(
    entry: DirEntry,
    session_id: &str,
    filename_filter: fn(&str) -> bool,
    line_predicate: fn(&serde_json::Value, &str) -> bool,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), LocatorError> {
    if is_directory_entry(&entry) {
        return scan_at_depth(
            &entry.path(),
            session_id,
            filename_filter,
            line_predicate,
            depth + 1,
            matches,
        );
    }
    if is_candidate_file_entry(&entry, filename_filter)
        && jsonl_file_matches_session(&entry.path(), session_id, line_predicate)
    {
        matches.push(validate_jsonl_path(
            entry.path(),
            TranscriptLookupMode::RequireExisting,
        )?);
    }
    Ok(())
}

fn read_directory(dir: &Path) -> Result<ReadDir, LocatorError> {
    std::fs::read_dir(dir).map_err(|error| directory_read_error(dir, error))
}

fn directory_entries(entries: ReadDir, dir: &Path) -> Result<Vec<DirEntry>, LocatorError> {
    entries
        .map(|entry| entry.map_err(|error| directory_read_error(dir, error)))
        .collect()
}

fn directory_read_error(dir: &Path, error: std::io::Error) -> LocatorError {
    LocatorError::Io {
        kind: error.kind(),
        path: dir.to_path_buf(),
        message: directory_read_error_message(dir, &error),
    }
}

fn directory_read_error_message(dir: &Path, error: &std::io::Error) -> String {
    format!(
        "transcript_content_fallback_dir_read_error: {}: {error}",
        dir.display()
    )
}

fn is_directory_entry(entry: &DirEntry) -> bool {
    entry.file_type().is_ok_and(|file_type| file_type.is_dir())
}

fn is_candidate_file_entry(entry: &DirEntry, filename_filter: fn(&str) -> bool) -> bool {
    entry.file_type().is_ok_and(|file_type| file_type.is_file())
        && file_name_passes(entry, filename_filter)
}

fn file_name_passes(entry: &DirEntry, filename_filter: fn(&str) -> bool) -> bool {
    entry
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(filename_filter)
}

fn is_jsonl_candidate_claude(name: &str) -> bool {
    name.ends_with(".jsonl")
}

fn is_jsonl_candidate_codex(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

fn jsonl_file_matches_session(
    path: &Path,
    session_id: &str,
    line_predicate: fn(&serde_json::Value, &str) -> bool,
) -> bool {
    let Some(reader) = open_jsonl_reader(path) else {
        return false;
    };
    any_line_matches(reader, session_id, line_predicate)
}

fn open_jsonl_reader(path: &Path) -> Option<BufReader<File>> {
    File::open(path).ok().map(BufReader::new)
}

fn any_line_matches(
    reader: BufReader<File>,
    session_id: &str,
    line_predicate: fn(&serde_json::Value, &str) -> bool,
) -> bool {
    reader
        .lines()
        .map_while(Result::ok)
        .any(|line| line_matches(&line, session_id, line_predicate))
}

fn line_matches(
    line: &str,
    session_id: &str,
    line_predicate: fn(&serde_json::Value, &str) -> bool,
) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    parse_json_line(trimmed).is_some_and(|value| line_predicate(&value, session_id))
}

fn parse_json_line(line: &str) -> Option<serde_json::Value> {
    serde_json::from_str(line).ok()
}

fn line_indicates_claude_session(value: &serde_json::Value, session_id: &str) -> bool {
    value
        .get("sessionId")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|recorded| recorded == session_id)
}

fn line_indicates_codex_session(value: &serde_json::Value, session_id: &str) -> bool {
    if !is_codex_session_meta_record(value) {
        return false;
    }
    codex_session_meta_payload_id(value).is_some_and(|recorded| recorded == session_id)
}

fn is_codex_session_meta_record(value: &serde_json::Value) -> bool {
    value
        .get("type")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| kind == "session_meta")
}

fn codex_session_meta_payload_id(value: &serde_json::Value) -> Option<&str> {
    value
        .get("payload")
        .and_then(|payload| payload.get("id"))
        .and_then(serde_json::Value::as_str)
}
