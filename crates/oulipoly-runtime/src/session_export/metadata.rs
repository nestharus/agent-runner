use super::{ExportError, ExportSessionMetadata, SessionStorageType};
use oulipoly_config::{
    ProvidersConfig, ScriptSessionStorageType, SessionStorage, SessionsConfig, load_models,
};
use oulipoly_state::{ResumeError, StateDb};
use std::path::PathBuf;

pub fn resolve_export_session_metadata(
    session_id: &str,
) -> Result<ExportSessionMetadata, ExportError> {
    let state = StateDb::open_default().map_err(|message| ExportError::Operational { message })?;
    let config_root = default_config_root();
    let providers_path = config_root.join("providers.toml");
    let sessions_path = config_root.join("sessions.toml");
    let providers_cfg = ProvidersConfig::load(&providers_path).unwrap_or_default();
    let models_dir = default_models_dir();
    let models = load_models(&models_dir, Some(&providers_cfg))
        .map_err(|message| ExportError::Operational { message })?;
    let sessions_cfg = SessionsConfig::load(&sessions_path).unwrap_or_default();

    uuid::Uuid::parse_str(session_id).map_err(|_| ExportError::InvalidSessionId {
        input: session_id.to_string(),
    })?;

    let previews = state
        .resume_previews(session_id)
        .map_err(|message| ExportError::Operational { message })?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(24);
    let recent_count = previews
        .iter()
        .filter(|preview| preview.last_used_at >= cutoff)
        .count();
    if recent_count > 1 {
        return Err(ExportError::AmbiguousSession {
            input: session_id.to_string(),
        });
    }

    let resolved = state
        .resolve_resume(&models, session_id, None)
        .map_err(resume_error_to_export_error)?;

    let provider_entry = providers_cfg
        .get(&resolved.active_provider)
        .ok_or_else(|| ExportError::UnsupportedStorage {
            provider_name: resolved.active_provider.clone(),
            reason: "provider is missing from providers.toml".to_string(),
        })?;
    let storage_type = match provider_entry.session_storage.as_ref() {
        Some(SessionStorage::ClaudeCode { .. }) => SessionStorageType::ClaudeCode,
        Some(SessionStorage::Codex { .. }) => SessionStorageType::CodexSession,
        Some(SessionStorage::Script {
            storage_type: Some(ScriptSessionStorageType::ClaudeCode),
            ..
        }) => SessionStorageType::ClaudeCode,
        Some(SessionStorage::Script {
            storage_type: Some(ScriptSessionStorageType::CodexSession),
            ..
        }) => SessionStorageType::CodexSession,
        Some(SessionStorage::Script { .. }) => {
            return Err(ExportError::UnsupportedStorage {
                provider_name: resolved.active_provider,
                reason: "script session_storage does not declare a canonical export format"
                    .to_string(),
            });
        }
        None => {
            return Err(ExportError::UnsupportedStorage {
                provider_name: resolved.active_provider,
                reason: "provider has no session_storage configuration".to_string(),
            });
        }
    };

    let jsonl_path = crate::session_metadata::resolve_jsonl_path_for_provider_allow_missing(
        &sessions_cfg,
        provider_entry.session_storage.as_ref(),
        &resolved.active_provider,
        &resolved.active_session_id,
    )
    .map_err(metadata_error_to_export_error)?;

    Ok(ExportSessionMetadata {
        session_id: resolved.active_session_id,
        chain_id: resolved.chain_id,
        provider_name: resolved.active_provider,
        storage_type,
        jsonl_path,
    })
}

fn resume_error_to_export_error(err: ResumeError) -> ExportError {
    match err {
        ResumeError::InvalidUuid { input } => ExportError::InvalidSessionId { input },
        ResumeError::NoChainFound { input } => ExportError::SessionNotFound { input },
        ResumeError::WrongIdKind { input, .. } => ExportError::SessionNotFound { input },
        ResumeError::Ambiguous { input, .. } => ExportError::AmbiguousSession { input },
        ResumeError::ProviderModelMismatch {
            active_provider, ..
        } => ExportError::UnsupportedStorage {
            provider_name: active_provider,
            reason: "session owner is not in the resolved model provider pool".to_string(),
        },
        ResumeError::UnknownModel { model_name } => ExportError::Operational {
            message: format!("unknown model referenced by session chain: {model_name}"),
        },
        ResumeError::ActiveSegmentMissing { chain_id } => ExportError::Operational {
            message: format!("no active segment found for chain {chain_id}"),
        },
        ResumeError::ProviderNotConfigured { provider } => ExportError::UnsupportedStorage {
            provider_name: provider,
            reason: "session owner provider is not configured".to_string(),
        },
        ResumeError::ProviderMissingResume { provider_name } => ExportError::UnsupportedStorage {
            provider_name,
            reason: "session owner provider has no resume configuration".to_string(),
        },
        ResumeError::Db { message } => ExportError::Operational { message },
    }
}

fn metadata_error_to_export_error(err: crate::session_metadata::MetadataError) -> ExportError {
    match err {
        crate::session_metadata::MetadataError::InvalidSessionId { input } => {
            ExportError::InvalidSessionId { input }
        }
        crate::session_metadata::MetadataError::SessionNotFound { input } => {
            ExportError::SessionNotFound { input }
        }
        crate::session_metadata::MetadataError::AmbiguousSession { input } => {
            ExportError::AmbiguousSession { input }
        }
        crate::session_metadata::MetadataError::UnsupportedStorage {
            provider_name,
            reason,
        } => ExportError::UnsupportedStorage {
            provider_name,
            reason,
        },
        crate::session_metadata::MetadataError::Operational { message } => {
            ExportError::Operational { message }
        }
    }
}

fn default_config_root() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_models_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner").join("models"))
        .unwrap_or_else(|| PathBuf::from("models"))
}
