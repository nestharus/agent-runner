//! ## Declared roles
//! accessor, filter, formatter, mapper, validator
//!
//! AGE-137 Claude storage locator contract for transcript lookup.

use super::{
    LocatedTranscript, LocatorError, LocatorSource, SessionStorageType, TranscriptLocator,
    TranscriptLookupMode, TranscriptRequest, UnsupportedStorageReason, locate_claude_by_content,
    located, no_locator, single_jsonl_match, validate_jsonl_path,
};
use oulipoly_config::SessionStorage;
use std::fs::{DirEntry, ReadDir};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Copy)]
pub struct ClaudeStorageLocator;

impl TranscriptLocator for ClaudeStorageLocator {
    fn locate_jsonl(&self, request: &TranscriptRequest) -> Result<LocatedTranscript, LocatorError> {
        let projects_dir = require_claude_projects_dir(request.storage)?;
        let projects_dir = canonical_claude_projects_dir(projects_dir)?;
        let matches = claude_filename_or_content_matches(&projects_dir, &request.session_id)?;
        let path = single_jsonl_match(LocatorSource::Claude, &request.session_id, matches)?;
        Ok(located(path, SessionStorageType::ClaudeCode, request.mode))
    }
}

fn claude_filename_or_content_matches(
    projects_dir: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>, LocatorError> {
    let filename_matches = collect_claude_transcript_matches(projects_dir, session_id)?;
    if !filename_matches.is_empty() {
        return Ok(filename_matches);
    }
    locate_claude_by_content(projects_dir, session_id)
}

fn require_claude_projects_dir(storage: Option<&SessionStorage>) -> Result<&PathBuf, LocatorError> {
    claude_projects_dir(storage).ok_or_else(no_locator)
}

fn claude_projects_dir(storage: Option<&SessionStorage>) -> Option<&PathBuf> {
    match storage {
        Some(SessionStorage::ClaudeCode { projects_dir }) => Some(projects_dir),
        _ => None,
    }
}

fn canonical_claude_projects_dir(projects_dir: &Path) -> Result<PathBuf, LocatorError> {
    projects_dir
        .canonicalize()
        .map_err(|error| claude_projects_dir_unavailable(projects_dir, error))
}

fn claude_projects_dir_unavailable(path: &Path, error: std::io::Error) -> LocatorError {
    LocatorError::UnsupportedStorage {
        reason: UnsupportedStorageReason::ClaudeProjectsDirUnavailable {
            path: path.to_path_buf(),
            io_error: error.to_string(),
        },
    }
}

fn collect_claude_transcript_matches(
    projects_dir: &Path,
    session_id: &str,
) -> Result<Vec<PathBuf>, LocatorError> {
    let entries = read_claude_project_entries(projects_dir)?;
    let entries = directory_entries(entries, projects_dir)?;
    let directories = project_directory_entries(entries);
    let candidates = transcript_candidates(directories, &claude_transcript_filename(session_id));
    validate_existing_candidates(existing_candidates(candidates))
}

fn read_claude_project_entries(projects_dir: &Path) -> Result<ReadDir, LocatorError> {
    std::fs::read_dir(projects_dir)
        .map_err(|error| claude_projects_dir_unavailable(projects_dir, error))
}

fn directory_entries(entries: ReadDir, projects_dir: &Path) -> Result<Vec<DirEntry>, LocatorError> {
    entries
        .map(|entry| entry.map_err(|error| claude_project_entry_error(projects_dir, error)))
        .collect()
}

fn claude_project_entry_error(projects_dir: &Path, error: std::io::Error) -> LocatorError {
    LocatorError::Io {
        kind: error.kind(),
        path: projects_dir.to_path_buf(),
        message: claude_project_entry_error_message(projects_dir, &error),
    }
}

fn claude_project_entry_error_message(projects_dir: &Path, error: &std::io::Error) -> String {
    format!(
        "claude_projects_dir_read_error: {}: {error}",
        projects_dir.display()
    )
}

fn project_directory_entries(entries: Vec<DirEntry>) -> Vec<DirEntry> {
    entries.into_iter().filter(is_directory_entry).collect()
}

fn is_directory_entry(entry: &DirEntry) -> bool {
    entry.file_type().is_ok_and(|file_type| file_type.is_dir())
}

fn transcript_candidates(entries: Vec<DirEntry>, filename: &str) -> Vec<PathBuf> {
    entries
        .into_iter()
        .map(|entry| transcript_candidate(entry.path(), filename))
        .collect()
}

fn transcript_candidate(project_dir: PathBuf, filename: &str) -> PathBuf {
    project_dir.join(filename)
}

fn claude_transcript_filename(session_id: &str) -> String {
    format!("{session_id}.jsonl")
}

fn existing_candidates(candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    candidates
        .into_iter()
        .filter(|candidate| candidate.is_file())
        .collect()
}

fn validate_existing_candidates(candidates: Vec<PathBuf>) -> Result<Vec<PathBuf>, LocatorError> {
    candidates
        .into_iter()
        .map(|candidate| validate_jsonl_path(candidate, TranscriptLookupMode::RequireExisting))
        .collect()
}
