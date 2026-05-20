//! ## Declared roles
//! accessor, filter, formatter, mapper, predicate, validator
//!
//! AGE-137 Codex storage locator contract for rollout transcript lookup.

use super::{
    LocatedTranscript, LocatorError, LocatorSource, SessionStorageType, TranscriptLocator,
    TranscriptLookupMode, TranscriptRequest, UnsupportedStorageReason, locate_codex_by_content,
    located, no_locator, single_jsonl_match, validate_jsonl_path,
};
use oulipoly_config::SessionStorage;
use std::fs::{DirEntry, ReadDir};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy)]
pub struct CodexStorageLocator;

impl TranscriptLocator for CodexStorageLocator {
    fn locate_jsonl(&self, request: &TranscriptRequest) -> Result<LocatedTranscript, LocatorError> {
        let sessions_dir = require_codex_sessions_dir(request.storage)?;
        let sessions_dir = canonical_codex_sessions_dir(sessions_dir)?;
        let matches = codex_filename_or_content_matches(&sessions_dir, &request.session_id)?;
        let path = single_jsonl_match(LocatorSource::Codex, &request.session_id, matches)?;
        Ok(located(
            path,
            SessionStorageType::CodexSession,
            request.mode,
        ))
    }
}

fn codex_filename_or_content_matches(
    sessions_dir: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>, LocatorError> {
    let filename_matches = collect_codex_rollout_matches(sessions_dir, session_id)?;
    if !filename_matches.is_empty() {
        return Ok(filename_matches);
    }
    locate_codex_by_content(sessions_dir, session_id)
}

fn require_codex_sessions_dir(storage: Option<&SessionStorage>) -> Result<&PathBuf, LocatorError> {
    codex_sessions_dir(storage).ok_or_else(no_locator)
}

fn codex_sessions_dir(storage: Option<&SessionStorage>) -> Option<&PathBuf> {
    match storage {
        Some(SessionStorage::Codex { sessions_dir }) => Some(sessions_dir),
        _ => None,
    }
}

fn canonical_codex_sessions_dir(sessions_dir: &Path) -> Result<PathBuf, LocatorError> {
    sessions_dir
        .canonicalize()
        .map_err(|error| codex_sessions_dir_unavailable(sessions_dir, error))
}

fn codex_sessions_dir_unavailable(path: &Path, error: std::io::Error) -> LocatorError {
    LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::CodexSessionsDirUnavailable {
            path: path.to_path_buf(),
            io_error: error.to_string(),
        },
    }
}

fn collect_codex_rollout_matches(
    sessions_dir: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>, LocatorError> {
    let mut matches = Vec::new();
    collect_codex_rollout_matches_at_depth(sessions_dir, session_id, 0, &mut matches)?;
    Ok(matches)
}

fn collect_codex_rollout_matches_at_depth(
    dir: &Path,
    session_id: &str,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), LocatorError> {
    if !within_codex_scan_depth(depth) {
        return Ok(());
    }
    let entries = codex_directory_entries(read_codex_directory(dir)?, dir)?;
    for entry in entries {
        collect_codex_entry(entry, session_id, depth, matches)?;
    }
    Ok(())
}

fn within_codex_scan_depth(depth: usize) -> bool {
    depth <= 4
}

fn read_codex_directory(dir: &Path) -> Result<ReadDir, LocatorError> {
    std::fs::read_dir(dir).map_err(|error| codex_directory_read_error(dir, error))
}

fn codex_directory_entries(entries: ReadDir, dir: &Path) -> Result<Vec<DirEntry>, LocatorError> {
    entries
        .map(|entry| entry.map_err(|error| codex_directory_read_error(dir, error)))
        .collect()
}

fn codex_directory_read_error(dir: &Path, error: std::io::Error) -> LocatorError {
    LocatorError::Io {
        kind: error.kind(),
        path: dir.to_path_buf(),
        message: codex_directory_read_error_message(dir, &error),
    }
}

fn codex_directory_read_error_message(dir: &Path, error: &std::io::Error) -> String {
    format!("codex_sessions_dir_read_error: {}: {error}", dir.display())
}

fn collect_codex_entry(
    entry: DirEntry,
    session_id: &str,
    depth: usize,
    matches: &mut Vec<PathBuf>,
) -> Result<(), LocatorError> {
    if is_directory_entry(&entry) {
        return collect_codex_rollout_matches_at_depth(
            &entry.path(),
            session_id,
            next_depth(depth),
            matches,
        );
    }
    if is_rollout_file_entry(&entry, session_id) {
        matches.push(validated_rollout_path(entry.path())?);
    }
    Ok(())
}

fn next_depth(depth: usize) -> usize {
    depth + 1
}

fn is_directory_entry(entry: &DirEntry) -> bool {
    entry.file_type().is_ok_and(|file_type| file_type.is_dir())
}

fn is_rollout_file_entry(entry: &DirEntry, session_id: &str) -> bool {
    entry.file_type().is_ok_and(|file_type| file_type.is_file())
        && is_codex_rollout_match(&entry.path(), session_id)
}

fn is_codex_rollout_match(path: &Path, session_id: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| rollout_filename_matches_session(name, session_id))
}

fn rollout_filename_matches_session(name: &str, session_id: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl") && name.contains(session_id)
}

fn validated_rollout_path(path: PathBuf) -> Result<PathBuf, LocatorError> {
    validate_jsonl_path(path, TranscriptLookupMode::RequireExisting)
}
