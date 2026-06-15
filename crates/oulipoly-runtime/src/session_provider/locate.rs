use super::types::SessionProviderError;
use crate::session_metadata::{LocatedTranscript, SessionStorageType, TranscriptLookupMode};
use oulipoly_config::{ScriptSessionStorageType, SessionStorage};
use oulipoly_provider::generated::SessionLocateTranscriptResult;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn map_locate_result(
    result: SessionLocateTranscriptResult,
    mode: TranscriptLookupMode,
) -> Result<LocatedTranscript, SessionProviderError> {
    let facts = validate_locate_result(result, mode)?;
    Ok(located_transcript_from_facts(facts, mode))
}

struct ValidLocateFacts {
    path: PathBuf,
    format_id: Option<String>,
}

fn validate_locate_result(
    result: SessionLocateTranscriptResult,
    mode: TranscriptLookupMode,
) -> Result<ValidLocateFacts, SessionProviderError> {
    require_located(result.located)?;
    let path = validate_provider_path(require_located_path(result.path)?, mode)?;
    require_existing_observed(result.require_existing_observed, mode)?;
    Ok(ValidLocateFacts {
        path,
        format_id: result.format_id,
    })
}

fn require_located(located: bool) -> Result<(), SessionProviderError> {
    if located {
        Ok(())
    } else {
        Err(SessionProviderError::new(
            "session_locate_missing",
            "provider did not locate a transcript",
        ))
    }
}

fn located_transcript_from_facts(
    facts: ValidLocateFacts,
    mode: TranscriptLookupMode,
) -> LocatedTranscript {
    LocatedTranscript {
        path: facts.path,
        storage_classification: map_format_id(facts.format_id.as_deref()),
        require_existing_observed: matches!(mode, TranscriptLookupMode::RequireExisting),
    }
}

fn require_located_path(path: Option<String>) -> Result<PathBuf, SessionProviderError> {
    let Some(path) = path else {
        return Err(SessionProviderError::new(
            "session_locate_missing_path",
            "provider returned located=true without path",
        ));
    };
    if path.is_empty() {
        return Err(SessionProviderError::new(
            "session_locate_empty_path",
            "provider returned an empty transcript path",
        ));
    }
    Ok(PathBuf::from(path))
}

fn require_existing_observed(
    observed: Option<bool>,
    mode: TranscriptLookupMode,
) -> Result<(), SessionProviderError> {
    if matches!(mode, TranscriptLookupMode::RequireExisting) && observed != Some(true) {
        return Err(SessionProviderError::new(
            "session_locate_require_existing_unobserved",
            "provider did not report require_existing observation",
        ));
    }
    Ok(())
}

fn validate_provider_path(
    path: PathBuf,
    mode: TranscriptLookupMode,
) -> Result<PathBuf, SessionProviderError> {
    validate_absolute_provider_path(&path)?;
    if provider_path_exists(&path) {
        return canonicalize_provider_path(&path);
    }
    validate_missing_provider_path(&path, mode)?;
    Ok(path)
}

fn validate_absolute_provider_path(path: &Path) -> Result<(), SessionProviderError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(invalid_provider_relative_path(path))
    }
}

fn provider_path_exists(path: &Path) -> bool {
    path.exists()
}

fn canonicalize_provider_path(path: &Path) -> Result<PathBuf, SessionProviderError> {
    path.canonicalize()
        .map_err(|err| invalid_provider_canonicalize_path(path, err))
}

fn validate_missing_provider_path(
    path: &Path,
    mode: TranscriptLookupMode,
) -> Result<(), SessionProviderError> {
    if matches!(mode, TranscriptLookupMode::AllowMissing) {
        return Ok(());
    }
    Err(invalid_provider_missing_path(path))
}

fn invalid_provider_relative_path(path: &Path) -> SessionProviderError {
    SessionProviderError::new(
        "session_locate_invalid_path",
        format!("provider returned relative path {}", path.display()),
    )
}

fn invalid_provider_canonicalize_path(path: &Path, error: std::io::Error) -> SessionProviderError {
    SessionProviderError::new(
        "session_locate_invalid_path",
        format!("failed to canonicalize {}: {error}", path.display()),
    )
}

fn invalid_provider_missing_path(path: &Path) -> SessionProviderError {
    SessionProviderError::new(
        "session_locate_invalid_path",
        format!("provider returned missing path {}", path.display()),
    )
}

fn map_format_id(format_id: Option<&str>) -> SessionStorageType {
    map_parsed_format_id(parse_format_id(format_id))
}

fn parse_format_id(format_id: Option<&str>) -> Option<ScriptSessionStorageType> {
    let format_id = format_id?;
    serde_json::from_value::<ScriptSessionStorageType>(Value::String(format_id.to_string())).ok()
}

fn map_parsed_format_id(storage_type: Option<ScriptSessionStorageType>) -> SessionStorageType {
    match storage_type {
        Some(storage_type) => SessionStorageType::from(&Some(SessionStorage::Script {
            cwd_script: String::new(),
            transcript_script: None,
            storage_type: Some(storage_type),
        })),
        None => SessionStorageType::Other,
    }
}
