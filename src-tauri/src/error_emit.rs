//! ## Declared roles
//!
//! `formatter`, `mapper`, `validator`

use crate::json_error::emit_json_error;
use agent_runner_lib::effective_provider_for_model_provider;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_runtime::executor;
use oulipoly_runtime::session_lock::LockError;
use oulipoly_state::StateDb;

pub(crate) fn emit_resume_resolution_error(err: oulipoly_state::ResumeError) -> i32 {
    let disposition = resume_resolution_error_disposition(err);
    emit_resume_resolution_error_payload(disposition.payload);
    disposition.exit_code
}

struct ResumeResolutionErrorDisposition {
    exit_code: i32,
    payload: ResumeResolutionErrorPayload,
}

fn resume_resolution_error_disposition(
    err: oulipoly_state::ResumeError,
) -> ResumeResolutionErrorDisposition {
    let exit_code = resume_resolution_error_exit_code(&err);
    let payload = resume_resolution_error_payload(err);
    resume_resolution_error_disposition_from_parts(exit_code, payload)
}

fn resume_resolution_error_disposition_from_parts(
    exit_code: i32,
    payload: ResumeResolutionErrorPayload,
) -> ResumeResolutionErrorDisposition {
    ResumeResolutionErrorDisposition { exit_code, payload }
}

fn resume_resolution_error_exit_code(err: &oulipoly_state::ResumeError) -> i32 {
    match err {
        oulipoly_state::ResumeError::InvalidUuid { .. } => 2,
        oulipoly_state::ResumeError::NoChainFound { .. } => 10,
        oulipoly_state::ResumeError::WrongIdKind { .. } => 10,
        oulipoly_state::ResumeError::Ambiguous { .. } => 11,
        oulipoly_state::ResumeError::UnknownModel { .. } => 12,
        oulipoly_state::ResumeError::ProviderModelMismatch { .. } => 12,
        oulipoly_state::ResumeError::ActiveSegmentMissing { .. } => 12,
        oulipoly_state::ResumeError::ProviderNotConfigured { .. } => 12,
        oulipoly_state::ResumeError::ProviderMissingResume { .. } => 12,
        oulipoly_state::ResumeError::Db { .. } => 1,
    }
}

enum ResumeResolutionErrorPayload {
    JsonError { code: &'static str, message: String },
    WrongIdKind(serde_json::Value),
}

fn resume_resolution_error_payload(
    err: oulipoly_state::ResumeError,
) -> ResumeResolutionErrorPayload {
    use oulipoly_state::ResumeError;
    match err {
        ResumeError::InvalidUuid { input } => ResumeResolutionErrorPayload::JsonError {
            code: "invalid-session-id",
            message: invalid_session_uuid_message(&input),
        },
        ResumeError::NoChainFound { input } => ResumeResolutionErrorPayload::JsonError {
            code: "session-not-found",
            message: no_chain_found_message(&input),
        },
        ResumeError::WrongIdKind {
            input,
            provider_session_id,
            agent_runner_invocation_id,
            chain_id,
            provider_name,
            ..
        } => ResumeResolutionErrorPayload::WrongIdKind(wrong_id_kind_payload(
            &input,
            &provider_session_id,
            &agent_runner_invocation_id,
            &chain_id,
            &provider_name,
        )),
        ResumeError::Ambiguous { input, .. } => ResumeResolutionErrorPayload::JsonError {
            code: "ambiguous-session",
            message: ambiguous_session_message(&input),
        },
        ResumeError::UnknownModel { model_name } => ResumeResolutionErrorPayload::JsonError {
            code: "model-resolution-failed",
            message: unknown_model_message(&model_name),
        },
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            ..
        } => ResumeResolutionErrorPayload::JsonError {
            code: "model-resolution-failed",
            message: provider_model_mismatch_message(&model_name, &active_provider),
        },
        ResumeError::ActiveSegmentMissing { chain_id } => ResumeResolutionErrorPayload::JsonError {
            code: "model-resolution-failed",
            message: active_segment_missing_message(&chain_id),
        },
        ResumeError::ProviderNotConfigured { provider } => {
            ResumeResolutionErrorPayload::JsonError {
                code: "model-resolution-failed",
                message: provider_not_configured_message(&provider),
            }
        }
        ResumeError::ProviderMissingResume { provider_name } => {
            ResumeResolutionErrorPayload::JsonError {
                code: "model-resolution-failed",
                message: provider_missing_resume_message(&provider_name),
            }
        }
        ResumeError::Db { message } => ResumeResolutionErrorPayload::JsonError {
            code: "operational-error",
            message,
        },
    }
}

fn emit_resume_resolution_error_payload(payload: ResumeResolutionErrorPayload) {
    match payload {
        ResumeResolutionErrorPayload::JsonError { code, message } => {
            emit_json_error(code, message);
        }
        ResumeResolutionErrorPayload::WrongIdKind(payload) => {
            emit_wrong_id_kind_error(&payload);
        }
    }
}

pub(crate) fn emit_lock_error(err: LockError) -> i32 {
    let disposition = lock_error_disposition(err);
    emit_lock_error_payload(disposition.payload);
    disposition.exit_code
}

struct LockErrorDisposition {
    exit_code: i32,
    payload: LockErrorPayload,
}

fn lock_error_disposition(err: LockError) -> LockErrorDisposition {
    let exit_code = lock_error_exit_code(&err);
    let payload = lock_error_payload(err);
    lock_error_disposition_from_parts(exit_code, payload)
}

fn lock_error_disposition_from_parts(
    exit_code: i32,
    payload: LockErrorPayload,
) -> LockErrorDisposition {
    LockErrorDisposition { exit_code, payload }
}

fn lock_error_exit_code(err: &LockError) -> i32 {
    match err {
        LockError::Busy { .. } => 13,
        LockError::TokenInvalid => 16,
        LockError::LockExpired => 17,
        LockError::Operational { .. } => 1,
    }
}

struct LockErrorPayload {
    code: &'static str,
    message: String,
}

fn lock_error_payload(err: LockError) -> LockErrorPayload {
    match err {
        LockError::Busy { expires_at, .. } => LockErrorPayload {
            code: "session-busy",
            message: session_busy_message(&expires_at),
        },
        LockError::TokenInvalid => LockErrorPayload {
            code: "lock-token-invalid",
            message: "pause token is invalid for this session".to_string(),
        },
        LockError::LockExpired => LockErrorPayload {
            code: "lock-expired",
            message: "pause lock is absent or expired without release evidence".to_string(),
        },
        LockError::Operational { message } => LockErrorPayload {
            code: "operational-error",
            message,
        },
    }
}

fn emit_lock_error_payload(payload: LockErrorPayload) {
    emit_json_error(payload.code, payload.message);
}

pub(crate) fn effective_model_for_execution(
    model: &ModelConfig,
    provider_index: usize,
    providers_cfg: &ProvidersConfig,
) -> Result<(ProviderConfig, PromptMode), String> {
    effective_provider_for_model_provider(model, provider_index, providers_cfg)
}

pub(crate) fn emit_unknown_diagnostic(
    state: &StateDb,
    provider_name: &str,
    provider_index: usize,
    result: &executor::ExecutionResult,
    retry_rotation_disposition: &str,
) {
    let payload = unknown_diagnostic_payload(UnknownDiagnosticPayloadInput {
        provider_name,
        provider_index,
        account_window_state: unknown_account_window_state_payload(state, provider_name),
        exit_code: result.exit_code,
        retry_rotation_disposition,
        stderr_excerpt: unknown_diagnostic_stderr_excerpt(result),
    });
    emit_unknown_diagnostic_payload(&payload);
}

struct UnknownDiagnosticPayloadInput<'a> {
    provider_name: &'a str,
    provider_index: usize,
    account_window_state: serde_json::Value,
    exit_code: i32,
    retry_rotation_disposition: &'a str,
    stderr_excerpt: String,
}

fn unknown_account_window_state_payload(state: &StateDb, provider_name: &str) -> serde_json::Value {
    crate::diagnostics_payloads::account_window_state_payload(state, provider_name)
}

fn unknown_diagnostic_stderr_excerpt(result: &executor::ExecutionResult) -> String {
    crate::redaction::redacted_stderr_excerpt(&result.stderr)
}

fn unknown_diagnostic_payload(input: UnknownDiagnosticPayloadInput<'_>) -> serde_json::Value {
    serde_json::json!({
        "error_category": "unknown",
        "provider": input.provider_name,
        "provider_index": input.provider_index,
        "account_window_state": input.account_window_state,
        "exit_code": input.exit_code,
        "retry_rotation_disposition": input.retry_rotation_disposition,
        "stderr_excerpt": input.stderr_excerpt,
    })
}

fn emit_unknown_diagnostic_payload(payload: &serde_json::Value) {
    match serde_json::to_string(payload) {
        Ok(json) => eprintln!("OULIPOLY_UNKNOWN_DIAGNOSTIC={json}"),
        Err(err) => eprintln!("Warning: Failed to serialize unknown diagnostic: {err}"),
    }
}

fn invalid_session_uuid_message(input: &str) -> String {
    format!("invalid session UUID: {input}")
}

fn no_chain_found_message(input: &str) -> String {
    format!("no session found matching {input}")
}

fn wrong_id_kind_message(input: &str) -> String {
    format!("wrong id kind: {input} is an agent-runner invocation id")
}

fn wrong_id_kind_payload(
    input: &str,
    provider_session_id: &Option<String>,
    agent_runner_invocation_id: &str,
    chain_id: &Option<String>,
    provider_name: &Option<String>,
) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "code": "wrong-id-kind",
            "message": wrong_id_kind_message(input),
            "input": input,
            "agent_runner_invocation_id": agent_runner_invocation_id,
            "provider_session_id": provider_session_id,
            "agent_runner_chain_id": chain_id,
            "provider_name": provider_name,
        }
    })
}

fn emit_wrong_id_kind_error(payload: &serde_json::Value) {
    eprintln!("{payload}");
}

fn ambiguous_session_message(input: &str) -> String {
    format!("session {input} resolves to multiple chains")
}

fn unknown_model_message(model_name: &str) -> String {
    format!("unknown model for session: {model_name}")
}

fn provider_model_mismatch_message(model_name: &str, active_provider: &str) -> String {
    format!("model {model_name} does not include active provider {active_provider}")
}

fn active_segment_missing_message(chain_id: &str) -> String {
    format!("no active segment found for chain {chain_id}")
}

fn provider_not_configured_message(provider: &str) -> String {
    format!("provider {provider} is not configured")
}

fn provider_missing_resume_message(provider_name: &str) -> String {
    format!("provider {provider_name} has no resume configuration")
}

fn session_busy_message(expires_at: &str) -> String {
    format!("session is paused until {expires_at}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_resolution_error_exit_codes_are_preserved() {
        assert_eq!(
            emit_resume_resolution_error(oulipoly_state::ResumeError::InvalidUuid {
                input: "bad".to_string(),
            }),
            2
        );
        assert_eq!(
            emit_resume_resolution_error(oulipoly_state::ResumeError::NoChainFound {
                input: "x".to_string(),
            }),
            10
        );
        assert_eq!(
            emit_resume_resolution_error(oulipoly_state::ResumeError::Ambiguous {
                input: "x".to_string(),
                previews: Vec::new(),
            }),
            11
        );
        assert_eq!(
            emit_resume_resolution_error(oulipoly_state::ResumeError::UnknownModel {
                model_name: "m".to_string(),
            }),
            12
        );
        assert_eq!(
            emit_resume_resolution_error(oulipoly_state::ResumeError::Db {
                message: "boom".to_string(),
            }),
            1
        );
    }

    #[test]
    fn lock_error_exit_codes_are_preserved() {
        assert_eq!(
            emit_lock_error(LockError::Busy {
                expires_at: "2026-01-01T00:00:00Z".to_string(),
                token_hash: None,
            }),
            13
        );
        assert_eq!(emit_lock_error(LockError::TokenInvalid), 16);
        assert_eq!(emit_lock_error(LockError::LockExpired), 17);
        assert_eq!(
            emit_lock_error(LockError::Operational {
                message: "boom".to_string(),
            }),
            1
        );
    }
}
