//! ## Declared roles
//! orchestration, accessor, filter, mapper, predicate, formatter
//!
//! Session-window matching helper. It owns provider-session candidate lookup,
//! effective-cwd disambiguation, and no-match warning emission.
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/services/session_window.rs::session_window_selection
//!     role: intrinsic-surface
//!     Domain: invocation-window-session-selection
//!     Owns:
//!       - invocation finished-at window
//!       - provider-session candidate enumeration
//!       - effective-cwd comparison
//!       - workspace-root normalization
//!       - no-match warning
//!       - first-candidate fallback policy

use oulipoly_config::ProvidersConfig;
use oulipoly_state::StateDb;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) fn find_session_for_invocation_window(
    state: &StateDb,
    providers_cfg: Option<&ProvidersConfig>,
    provider_name: &str,
    started_at: &chrono::DateTime<chrono::Utc>,
    finished_at: &chrono::DateTime<chrono::Utc>,
    effective_cwd: Option<&Path>,
    stderr: &mut dyn Write,
) -> Result<Option<String>, String> {
    let candidates = session_window_candidates(state, provider_name, started_at, finished_at)?;
    if candidates.len() <= 1 {
        return Ok(first_candidate(candidates));
    }

    let Some(effective_cwd) = effective_cwd else {
        return Ok(first_candidate(candidates));
    };
    let Some(session_storage) = session_storage_for_provider(providers_cfg, provider_name) else {
        return Ok(first_candidate(candidates));
    };

    select_candidate_by_workspace(
        candidates,
        session_storage,
        provider_name,
        effective_cwd,
        stderr,
    )
}

fn select_candidate_by_workspace(
    candidates: Vec<String>,
    session_storage: &oulipoly_config::SessionStorage,
    provider_name: &str,
    effective_cwd: &Path,
    stderr: &mut dyn Write,
) -> Result<Option<String>, String> {
    let expected_cwd = comparable_workspace_path(effective_cwd);
    let resolved = resolved_workspace_candidates(&candidates, session_storage, provider_name);
    match select_resolved_workspace_candidate(&resolved, &expected_cwd) {
        WorkspaceCandidateSelection::Matched(session_id) => Ok(Some(session_id)),
        WorkspaceCandidateSelection::NoMatchWithResolved => {
            write_session_window_no_match_warning(stderr, provider_name, effective_cwd)?;
            Ok(None)
        }
        WorkspaceCandidateSelection::NoResolvedCandidates => Ok(first_candidate(candidates)),
    }
}

fn resolved_workspace_candidates(
    candidates: &[String],
    session_storage: &oulipoly_config::SessionStorage,
    provider_name: &str,
) -> Vec<(String, PathBuf)> {
    let results = candidate_workspace_results(candidates, session_storage, provider_name);
    let successful = successful_candidate_workspace_results(results);
    workspace_candidate_tuples(successful)
}

fn workspace_matched_candidate(
    resolved: &[(String, PathBuf)],
    expected_cwd: &Path,
) -> Option<String> {
    matched_workspace_candidate(resolved, expected_cwd).map(workspace_candidate_session_id)
}

enum WorkspaceCandidateSelection {
    Matched(String),
    NoMatchWithResolved,
    NoResolvedCandidates,
}

fn select_resolved_workspace_candidate(
    resolved: &[(String, PathBuf)],
    expected_cwd: &Path,
) -> WorkspaceCandidateSelection {
    if let Some(session_id) = workspace_matched_candidate(resolved, expected_cwd) {
        return WorkspaceCandidateSelection::Matched(session_id);
    }
    resolved_workspace_candidate_miss(resolved)
}

fn resolved_workspace_candidate_miss(
    resolved: &[(String, PathBuf)],
) -> WorkspaceCandidateSelection {
    if has_resolved_workspace_candidates(resolved) {
        return WorkspaceCandidateSelection::NoMatchWithResolved;
    }
    WorkspaceCandidateSelection::NoResolvedCandidates
}

fn candidate_workspace_results(
    candidates: &[String],
    session_storage: &oulipoly_config::SessionStorage,
    provider_name: &str,
) -> Vec<(
    String,
    Result<PathBuf, crate::session_metadata::MetadataError>,
)> {
    candidates
        .iter()
        .map(|session_id| {
            (
                session_id.clone(),
                resolve_candidate_workspace_root(session_storage, provider_name, session_id),
            )
        })
        .collect()
}

fn successful_candidate_workspace_results(
    results: Vec<(
        String,
        Result<PathBuf, crate::session_metadata::MetadataError>,
    )>,
) -> Vec<(
    String,
    Result<PathBuf, crate::session_metadata::MetadataError>,
)> {
    results
        .into_iter()
        .filter(|(_, result)| result.is_ok())
        .collect()
}

fn workspace_candidate_tuples(
    results: Vec<(
        String,
        Result<PathBuf, crate::session_metadata::MetadataError>,
    )>,
) -> Vec<(String, PathBuf)> {
    results
        .into_iter()
        .map(|(session_id, result)| {
            (
                session_id,
                result.expect("workspace candidates were filtered to successful results"),
            )
        })
        .collect()
}

fn matched_workspace_candidate<'a>(
    resolved: &'a [(String, PathBuf)],
    expected_cwd: &Path,
) -> Option<&'a (String, PathBuf)> {
    resolved
        .iter()
        .find(|(_, workspace_root)| workspace_matches_expected_cwd(workspace_root, expected_cwd))
}

fn workspace_candidate_session_id(candidate: &(String, PathBuf)) -> String {
    candidate.0.clone()
}

fn has_resolved_workspace_candidates(resolved: &[(String, PathBuf)]) -> bool {
    !resolved.is_empty()
}

fn first_candidate(candidates: Vec<String>) -> Option<String> {
    candidates.into_iter().next()
}

fn session_window_candidates(
    state: &StateDb,
    provider_name: &str,
    started_at: &chrono::DateTime<chrono::Utc>,
    finished_at: &chrono::DateTime<chrono::Utc>,
) -> Result<Vec<String>, String> {
    state.find_sessions_for_invocation_window(provider_name, started_at, finished_at)
}

fn session_storage_for_provider<'a>(
    providers_cfg: Option<&'a ProvidersConfig>,
    provider_name: &str,
) -> Option<&'a oulipoly_config::SessionStorage> {
    providers_cfg
        .and_then(|providers| providers.get(provider_name))
        .and_then(|entry| entry.session_storage.as_ref())
}

fn resolve_candidate_workspace_root(
    session_storage: &oulipoly_config::SessionStorage,
    provider_name: &str,
    session_id: &str,
) -> Result<PathBuf, crate::session_metadata::MetadataError> {
    crate::session_metadata::resolve_workspace_root_for_provider_session(
        Some(session_storage),
        provider_name,
        session_id,
    )
}

fn workspace_matches_expected_cwd(workspace_root: &Path, expected_cwd: &Path) -> bool {
    comparable_workspace_path(workspace_root) == expected_cwd
}

fn write_session_window_no_match_warning(
    stderr: &mut dyn Write,
    provider_name: &str,
    effective_cwd: &Path,
) -> Result<(), String> {
    writeln!(
        stderr,
        "Warning: no in-window session for provider {provider_name} matched cwd {}; not emitting inferred session id",
        effective_cwd.display()
    )
    .map_err(|err| format!("Failed to write session ingest warning: {err}"))
}

fn comparable_workspace_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
