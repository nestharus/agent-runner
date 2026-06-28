use super::types::{
    SessionProviderEnumerateEntry, SessionProviderEnumerateResult, SessionProviderEnumerateSource,
    SessionProviderError,
};
use oulipoly_provider::generated::SessionEnumerateResult as ProviderEnumerateResult;
use std::path::PathBuf;

pub(super) fn map_enumerate_result(
    result: ProviderEnumerateResult,
) -> Result<SessionProviderEnumerateResult, SessionProviderError> {
    let sessions = result
        .sessions
        .into_iter()
        .map(|entry| {
            Ok(SessionProviderEnumerateEntry {
                provider_session_id: entry.provider_session_id,
                title: non_empty_optional(entry.title),
                cwd: map_optional_cwd(entry.cwd)?,
                created_unix_ms: entry.created_unix_ms,
                updated_unix_ms: entry.updated_unix_ms,
                turn_count: entry.turn_count,
                source: SessionProviderEnumerateSource {
                    kind: entry.source.kind,
                    detail: non_empty_optional(entry.source.detail),
                },
            })
        })
        .collect::<Result<Vec<_>, SessionProviderError>>()?;

    Ok(SessionProviderEnumerateResult {
        sessions,
        complete: result.complete,
        next_cursor: non_empty_optional(result.next_cursor),
        warnings: result.warnings,
    })
}

fn map_optional_cwd(cwd: Option<String>) -> Result<Option<PathBuf>, SessionProviderError> {
    let Some(cwd) = non_empty_optional(cwd) else {
        return Ok(None);
    };
    let path = PathBuf::from(cwd);
    if path.is_absolute() {
        Ok(Some(path))
    } else {
        Err(SessionProviderError::new(
            "session_enumerate_invalid_cwd",
            "provider returned a relative session cwd",
        ))
    }
}

fn non_empty_optional(input: Option<String>) -> Option<String> {
    input.filter(|value| !value.is_empty())
}
