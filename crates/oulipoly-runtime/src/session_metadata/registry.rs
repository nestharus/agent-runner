//! ## Declared roles
//! accessor, filter, mapper, parser, predicate, validator
//!
//! Push-based transcript-location registry for provider-private storage scans.
//! Metadata lookup hydrates this Oulipoly-owned boundary first, then resolves
//! sessions from registry entries instead of pulling directly from provider
//! private layouts.

use super::SessionStorageType;
use super::locator::{
    LocatedTranscript, LocatorError, LocatorSource, TranscriptLookupMode, UnsupportedStorageReason,
    located, no_locator, single_jsonl_match, validate_jsonl_path,
};
use oulipoly_config::SessionStorage;
use std::fs::{DirEntry, ReadDir};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLocatorEntry {
    pub provider_name: String,
    pub session_id: String,
    pub storage_type: SessionStorageType,
    pub path: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TranscriptLocatorRegistry {
    entries: Vec<TranscriptLocatorEntry>,
}

impl TranscriptLocatorRegistry {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &TranscriptLocatorEntry> {
        self.entries.iter()
    }

    pub fn locate(
        &self,
        provider_name: &str,
        session_id: &str,
        storage_type: SessionStorageType,
        source: LocatorSource,
        mode: TranscriptLookupMode,
    ) -> Result<LocatedTranscript, LocatorError> {
        let matches = self.matching_paths(provider_name, session_id, storage_type);
        let path = single_jsonl_match(source, session_id, matches)?;
        Ok(located(path, storage_type, mode))
    }

    fn insert(&mut self, entry: TranscriptLocatorEntry) {
        if self.contains_entry(&entry) {
            return;
        }
        self.entries.push(entry);
    }

    fn contains_entry(&self, entry: &TranscriptLocatorEntry) -> bool {
        self.entries.iter().any(|candidate| candidate == entry)
    }

    fn matching_paths(
        &self,
        provider_name: &str,
        session_id: &str,
        storage_type: SessionStorageType,
    ) -> Vec<PathBuf> {
        let entries = self.matching_entries(provider_name, session_id, storage_type);
        paths_for_entries(entries)
    }

    fn matching_entries(
        &self,
        provider_name: &str,
        session_id: &str,
        storage_type: SessionStorageType,
    ) -> Vec<&TranscriptLocatorEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.provider_name == provider_name)
            .filter(|entry| entry.session_id == session_id)
            .filter(|entry| entry.storage_type == storage_type)
            .collect()
    }
}

fn paths_for_entries(entries: Vec<&TranscriptLocatorEntry>) -> Vec<PathBuf> {
    entries
        .into_iter()
        .map(|entry| entry.path.clone())
        .collect()
}

pub fn discover_transcript_locator_registry(
    provider_name: &str,
    storage: Option<&SessionStorage>,
) -> Result<TranscriptLocatorRegistry, LocatorError> {
    let mut registry = TranscriptLocatorRegistry::new();
    hydrate_transcript_locator_registry(&mut registry, provider_name, storage)?;
    Ok(registry)
}

pub fn hydrate_transcript_locator_registry(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    storage: Option<&SessionStorage>,
) -> Result<(), LocatorError> {
    match storage {
        Some(SessionStorage::ClaudeCode { projects_dir }) => {
            hydrate_claude_storage(registry, provider_name, projects_dir)
        }
        Some(SessionStorage::Codex { sessions_dir }) => {
            hydrate_codex_storage(registry, provider_name, sessions_dir)
        }
        _ => Err(no_locator()),
    }
}

pub(super) fn registry_for_provider_storage(
    provider_name: &str,
    storage: Option<&SessionStorage>,
) -> Result<TranscriptLocatorRegistry, LocatorError> {
    discover_transcript_locator_registry(provider_name, storage)
}

fn hydrate_claude_storage(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    projects_dir: &Path,
) -> Result<(), LocatorError> {
    let projects_dir = canonical_claude_projects_dir(projects_dir)?;
    let project_entries =
        directory_entries(read_claude_project_entries(&projects_dir)?, &projects_dir)?;
    let project_dirs = project_directory_entries(project_entries);
    for project_dir in project_dirs {
        hydrate_claude_project_dir(registry, provider_name, &project_dir.path())?;
    }
    Ok(())
}

fn canonical_claude_projects_dir(projects_dir: &Path) -> Result<PathBuf, LocatorError> {
    projects_dir
        .canonicalize()
        .map_err(|error| claude_projects_dir_unavailable(projects_dir, error))
}

fn read_claude_project_entries(projects_dir: &Path) -> Result<ReadDir, LocatorError> {
    std::fs::read_dir(projects_dir)
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
        "transcript_registry_dir_read_error: {}: {error}",
        dir.display()
    )
}

fn project_directory_entries(entries: Vec<DirEntry>) -> Vec<DirEntry> {
    entries.into_iter().filter(is_directory_entry).collect()
}

fn hydrate_claude_project_dir(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    project_dir: &Path,
) -> Result<(), LocatorError> {
    let entries = directory_entries(
        std::fs::read_dir(project_dir).map_err(|error| directory_read_error(project_dir, error))?,
        project_dir,
    )?;
    for entry in transcript_file_entries(entries) {
        ingest_claude_transcript_file(registry, provider_name, entry.path())?;
    }
    Ok(())
}

fn transcript_file_entries(entries: Vec<DirEntry>) -> Vec<DirEntry> {
    entries.into_iter().filter(is_file_entry).collect()
}

fn ingest_claude_transcript_file(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    path: PathBuf,
) -> Result<(), LocatorError> {
    let Some(session_id) = claude_session_id_from_path(&path) else {
        return Ok(());
    };
    let path = validate_jsonl_path(path, TranscriptLookupMode::RequireExisting)?;
    registry.insert(transcript_locator_entry(
        provider_name,
        &session_id,
        SessionStorageType::ClaudeCode,
        path,
    ));
    Ok(())
}

fn claude_session_id_from_path(path: &Path) -> Option<String> {
    if !has_jsonl_extension(path) {
        return None;
    }
    path.file_stem()?.to_str().map(str::to_string)
}

fn hydrate_codex_storage(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    sessions_dir: &Path,
) -> Result<(), LocatorError> {
    let sessions_dir = canonical_codex_sessions_dir(sessions_dir)?;
    hydrate_codex_storage_at_depth(registry, provider_name, &sessions_dir, 0)
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

fn hydrate_codex_storage_at_depth(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    dir: &Path,
    depth: usize,
) -> Result<(), LocatorError> {
    if !within_codex_scan_depth(depth) {
        return Ok(());
    }
    let entries = directory_entries(read_codex_directory(dir)?, dir)?;
    for entry in entries {
        hydrate_codex_entry(registry, provider_name, entry, depth)?;
    }
    Ok(())
}

fn within_codex_scan_depth(depth: usize) -> bool {
    depth <= 4
}

fn read_codex_directory(dir: &Path) -> Result<ReadDir, LocatorError> {
    std::fs::read_dir(dir).map_err(|error| codex_directory_read_error(dir, error))
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

fn hydrate_codex_entry(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    entry: DirEntry,
    depth: usize,
) -> Result<(), LocatorError> {
    if is_directory_entry(&entry) {
        return hydrate_codex_storage_at_depth(registry, provider_name, &entry.path(), depth + 1);
    }
    if is_file_entry(&entry) {
        ingest_codex_rollout_file(registry, provider_name, entry.path())?;
    }
    Ok(())
}

fn ingest_codex_rollout_file(
    registry: &mut TranscriptLocatorRegistry,
    provider_name: &str,
    path: PathBuf,
) -> Result<(), LocatorError> {
    let Some(session_id) = codex_session_id_from_path(&path) else {
        return Ok(());
    };
    let path = validate_jsonl_path(path, TranscriptLookupMode::RequireExisting)?;
    registry.insert(transcript_locator_entry(
        provider_name,
        &session_id,
        SessionStorageType::CodexSession,
        path,
    ));
    Ok(())
}

fn codex_session_id_from_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if !is_rollout_jsonl_name(name) {
        return None;
    }
    first_uuid_substring(name)
}

fn is_rollout_jsonl_name(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

fn first_uuid_substring(input: &str) -> Option<String> {
    input
        .char_indices()
        .filter_map(|(index, _)| uuid_window(input, index))
        .find_map(parse_uuid_window)
}

fn uuid_window(input: &str, index: usize) -> Option<&str> {
    let end = index.checked_add(36)?;
    input.get(index..end)
}

fn parse_uuid_window(candidate: &str) -> Option<String> {
    Uuid::parse_str(candidate).ok().map(|uuid| uuid.to_string())
}

fn is_directory_entry(entry: &DirEntry) -> bool {
    entry.file_type().is_ok_and(|file_type| file_type.is_dir())
}

fn is_file_entry(entry: &DirEntry) -> bool {
    entry.file_type().is_ok_and(|file_type| file_type.is_file())
}

fn has_jsonl_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "jsonl")
}

fn transcript_locator_entry(
    provider_name: &str,
    session_id: &str,
    storage_type: SessionStorageType,
    path: PathBuf,
) -> TranscriptLocatorEntry {
    TranscriptLocatorEntry {
        provider_name: provider_name.to_string(),
        session_id: session_id.to_string(),
        storage_type,
        path,
    }
}
