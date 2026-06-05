//! ## Declared roles
//! orchestration, accessor, filter, mapper, validator, formatter
//!
//! ## Adapter declarations
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/services/lock.rs::session_lock_service_adapter
//!     role: adapter
//!     Translates:
//!       - oulipoly_state.resume_resolution_contract
//!       - oulipoly_config.default_runtime_config_contract
//!       - oulipoly_runtime.session_lock_contract
//!       - host.filesystem_lock_directory_contract
//!
//! Session lock service helper. It owns default config/state/lock resolution
//! for acquire/release requests and preserves the existing DTO error boundary.

use super::dtos::{SessionLockFailure, SessionLockServiceRequest, SessionLockSuccess};
use crate::session_lock::{Lease, LockError, ReleaseReceipt, SessionLock};
use oulipoly_config::{ProvidersConfig, load_models};
use oulipoly_state::repositories::ResumeRepository;
use oulipoly_state::{ChainPreview, ResumeError, StateDb};
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn lock_session(
    request: SessionLockServiceRequest,
) -> Result<SessionLockSuccess, SessionLockFailure> {
    match request {
        SessionLockServiceRequest::Acquire { session_id, ttl_ms } => {
            acquire_session_lock(&session_id, ttl_ms)
        }
        SessionLockServiceRequest::Release { session_id, token } => {
            release_session_lock(&session_id, &token)
        }
    }
}

fn acquire_session_lock(
    session_id: &str,
    ttl_ms: u64,
) -> Result<SessionLockSuccess, SessionLockFailure> {
    let state = open_default_state_for_lock()?;
    let providers_cfg = load_default_providers_config();
    let models = load_default_models_for_lock(&providers_cfg)?;
    reject_recent_ambiguous_resume(&state, session_id).map_err(SessionLockFailure::Resume)?;
    let resolved = <StateDb as ResumeRepository>::resolve_resume(&state, &models, session_id, None)
        .map_err(SessionLockFailure::Resume)?;
    let lock = open_default_session_lock()?;
    let lease = acquire_resolved_session_lease(&lock, &resolved, ttl_ms)?;
    Ok(acquired_session_lock_success(resolved, lease))
}

fn acquired_session_lock_success(
    resolved: oulipoly_state::ResolvedResume,
    lease: Lease,
) -> SessionLockSuccess {
    SessionLockSuccess::Acquired {
        session_id: resolved.active_session_id,
        chain_id: resolved.chain_id,
        provider_name: resolved.active_provider,
        lease,
    }
}

fn open_default_state_for_lock() -> Result<StateDb, SessionLockFailure> {
    StateDb::open_default()
        .map_err(|message| SessionLockFailure::Lock(LockError::Operational { message }))
}

fn load_default_providers_config() -> ProvidersConfig {
    ProvidersConfig::load(&default_config_root().join("providers.toml")).unwrap_or_default()
}

fn load_default_models_for_lock(
    providers_cfg: &ProvidersConfig,
) -> Result<oulipoly_state::ModelStore, SessionLockFailure> {
    load_models(&default_models_dir(), Some(providers_cfg)).map_err(|message| {
        SessionLockFailure::Lock(LockError::Operational {
            message: message.to_string(),
        })
    })
}

fn open_default_session_lock() -> Result<SessionLock, SessionLockFailure> {
    let lock_dir = default_lock_dir().map_err(SessionLockFailure::Lock)?;
    SessionLock::new(&lock_dir).map_err(|err| {
        SessionLockFailure::Lock(LockError::Operational {
            message: format!("failed to open locks: {err}"),
        })
    })
}

fn acquire_resolved_session_lease(
    lock: &SessionLock,
    resolved: &oulipoly_state::ResolvedResume,
    ttl_ms: u64,
) -> Result<Lease, SessionLockFailure> {
    lock.acquire(
        &resolved.active_session_id,
        &resolved.active_provider,
        Duration::from_millis(ttl_ms),
    )
    .map_err(SessionLockFailure::Lock)
}

fn reject_recent_ambiguous_resume(state: &StateDb, session_id: &str) -> Result<(), ResumeError> {
    let previews = resume_previews_for_lock(state, session_id)?;
    let recent_count = recent_resume_preview_count(&previews);
    reject_ambiguous_recent_resume_count(session_id, previews, recent_count)
}

fn resume_previews_for_lock(
    state: &StateDb,
    session_id: &str,
) -> Result<Vec<ChainPreview>, ResumeError> {
    state
        .resume_previews(session_id)
        .map_err(|message| ResumeError::Db { message })
}

fn recent_resume_preview_count(previews: &[ChainPreview]) -> usize {
    let cutoff = recent_resume_preview_cutoff();
    previews
        .iter()
        .filter(|preview| preview.last_used_at >= cutoff)
        .count()
}

fn recent_resume_preview_cutoff() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::hours(24)
}

fn reject_ambiguous_recent_resume_count(
    session_id: &str,
    previews: Vec<ChainPreview>,
    recent_count: usize,
) -> Result<(), ResumeError> {
    if recent_count > 1 {
        return Err(ambiguous_recent_resume_error(session_id, previews));
    }
    Ok(())
}

fn ambiguous_recent_resume_error(session_id: &str, previews: Vec<ChainPreview>) -> ResumeError {
    ResumeError::Ambiguous {
        input: session_id.to_string(),
        previews,
    }
}

fn release_session_lock(
    session_id: &str,
    token: &str,
) -> Result<SessionLockSuccess, SessionLockFailure> {
    // Preserve resume-handshake's state-open gate; release itself does not resolve providers.
    open_default_state_for_lock()?;
    let lock = open_default_session_lock()?;
    let receipt = release_session_lease(&lock, session_id, token)?;
    Ok(SessionLockSuccess::Released { receipt })
}

fn release_session_lease(
    lock: &SessionLock,
    session_id: &str,
    token: &str,
) -> Result<ReleaseReceipt, SessionLockFailure> {
    lock.release(session_id, token)
        .map_err(SessionLockFailure::Lock)
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

fn default_lock_dir() -> Result<PathBuf, LockError> {
    oulipoly_state::paths::data_dir()
        .map(|dir| dir.join("locks"))
        .map_err(|_| LockError::Operational {
            message: "Could not determine data directory".to_string(),
        })
}
