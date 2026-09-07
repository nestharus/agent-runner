use super::ingest::ingest_one_canonical_turn_page;
use super::types::{
    SessionProviderError, SessionProviderIdentity, SessionTurnIngestQuantumRequest,
};
use crate::provider_registry::ProviderRegistry;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use oulipoly_provider::client::CancellationToken;
use oulipoly_state::{
    SessionTurnIngestStream, SessionTurnIngestStreamKey, SessionTurnStreamProjection, StateDb,
};
use std::path::Path;
use std::time::Duration;

const DEFAULT_MAX_TURNS: u64 = 128;
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 256 * 1024;
const DEFAULT_MAX_SOURCE_BYTES: u64 = 1024 * 1024;
const DEFAULT_MAX_INLINE_BODY_BYTES: u64 = 64 * 1024;
const DEFAULT_PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_LEASE_DURATION: ChronoDuration = ChronoDuration::seconds(75);
const MAX_RETRY_BACKOFF_SECS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTurnIngestQuantumOutcome {
    Idle,
    Applied {
        key: SessionTurnIngestStreamKey,
        inserted_turns: u64,
        duplicate_turns: u64,
        checkpoint_generation: u64,
    },
    RetryScheduled {
        key: SessionTurnIngestStreamKey,
        error: String,
    },
    Unsupported {
        key: SessionTurnIngestStreamKey,
        error: String,
    },
    Quarantined {
        key: SessionTurnIngestStreamKey,
        error: String,
    },
}

pub struct SessionTurnIngestDriverRequest<'a> {
    pub state: &'a StateDb,
    pub registry: &'a ProviderRegistry,
    pub lease_owner: &'a str,
    pub effective_cwd: Option<&'a Path>,
    pub cancellation: &'a CancellationToken,
    pub now: DateTime<Utc>,
}

pub fn run_one_session_turn_ingest_quantum(
    request: SessionTurnIngestDriverRequest<'_>,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let lease_expires_at = request.now + DEFAULT_LEASE_DURATION;
    let Some(stream) = request
        .state
        .lease_ready_session_turn_ingest_stream(
            SessionTurnStreamProjection::CanonicalIngest,
            request.lease_owner,
            request.now,
            lease_expires_at,
        )
        .map_err(state_error)?
    else {
        return Ok(SessionTurnIngestQuantumOutcome::Idle);
    };
    run_leased_session_turn_ingest_quantum(request, stream)
}

pub fn run_session_turn_ingest_quantum_for_key(
    request: SessionTurnIngestDriverRequest<'_>,
    key: &SessionTurnIngestStreamKey,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let lease_expires_at = request.now + DEFAULT_LEASE_DURATION;
    let Some(stream) = request
        .state
        .lease_session_turn_ingest_stream(key, request.lease_owner, request.now, lease_expires_at)
        .map_err(state_error)?
    else {
        return Ok(SessionTurnIngestQuantumOutcome::Idle);
    };
    run_leased_session_turn_ingest_quantum(request, stream)
}

fn run_leased_session_turn_ingest_quantum(
    request: SessionTurnIngestDriverRequest<'_>,
    stream: SessionTurnIngestStream,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let identity = SessionProviderIdentity {
        model_name: String::new(),
        provider_name: stream.key.provider_name.clone(),
        provider_instance_id: Some(stream.key.provider_instance_id.clone()),
        settings_id: stream.key.settings_id.clone(),
    };
    match ingest_one_canonical_turn_page(SessionTurnIngestQuantumRequest {
        state: request.state,
        registry: request.registry,
        lease_owner: request.lease_owner,
        identity,
        session_id: &stream.key.session_id,
        effective_cwd: request.effective_cwd,
        cancellation: request.cancellation,
        timeout: DEFAULT_PAGE_TIMEOUT,
        max_turns: DEFAULT_MAX_TURNS,
        max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        max_source_bytes: DEFAULT_MAX_SOURCE_BYTES,
        max_inline_body_bytes: DEFAULT_MAX_INLINE_BODY_BYTES,
    }) {
        Ok(outcome) => Ok(SessionTurnIngestQuantumOutcome::Applied {
            key: stream.key,
            inserted_turns: outcome.inserted_turns,
            duplicate_turns: outcome.duplicate_turns,
            checkpoint_generation: outcome.checkpoint_generation,
        }),
        Err(error) => dispose_failed_quantum(request, stream, error),
    }
}

fn dispose_failed_quantum(
    request: SessionTurnIngestDriverRequest<'_>,
    stream: SessionTurnIngestStream,
    error: SessionProviderError,
) -> Result<SessionTurnIngestQuantumOutcome, SessionProviderError> {
    let error_token = disposition_token(&error).to_string();
    if stream_is_quarantined(request.state, &stream.key)? {
        return Ok(SessionTurnIngestQuantumOutcome::Quarantined {
            key: stream.key,
            error: error_token,
        });
    }
    if unsupported_error(&error) {
        request
            .state
            .mark_session_turn_ingest_unsupported(
                &stream.key,
                request.lease_owner,
                stream.checkpoint_generation,
                &error_token,
            )
            .map_err(state_error)?;
        return Ok(SessionTurnIngestQuantumOutcome::Unsupported {
            key: stream.key,
            error: error_token,
        });
    }
    if permanent_protocol_error(&error) {
        request
            .state
            .quarantine_session_turn_ingest_stream(
                &stream.key,
                request.lease_owner,
                stream.checkpoint_generation,
                &error_token,
            )
            .map_err(state_error)?;
        return Ok(SessionTurnIngestQuantumOutcome::Quarantined {
            key: stream.key,
            error: error_token,
        });
    }
    let retry_at = request.now + retry_backoff(stream.retry_count);
    request
        .state
        .retry_session_turn_ingest_stream(
            &stream.key,
            request.lease_owner,
            stream.checkpoint_generation,
            retry_at,
            &error_token,
        )
        .map_err(state_error)?;
    Ok(SessionTurnIngestQuantumOutcome::RetryScheduled {
        key: stream.key,
        error: error_token,
    })
}

fn stream_is_quarantined(
    state: &StateDb,
    key: &SessionTurnIngestStreamKey,
) -> Result<bool, SessionProviderError> {
    state
        .session_turn_ingest_stream(key)
        .map(|stream| stream.is_some_and(|stream| stream.status == "quarantined"))
        .map_err(state_error)
}

fn unsupported_error(error: &SessionProviderError) -> bool {
    matches!(
        error.token(),
        "session_capability_missing"
            | "session_turn_pages_capability_missing"
            | "session_turn_page_budget_too_small"
            | "session_turn_record_ceiling_exceeded"
            | "codex_rollout_capacity"
    )
}

fn permanent_protocol_error(error: &SessionProviderError) -> bool {
    matches!(
        error.token(),
        "session_turn_page_token_stale"
            | "invalid_session_read_turns_params"
            | "provider_page_response_budget_exceeded"
            | "provider_page_identity_mismatch"
            | "provider_page_turn_count_invalid"
            | "provider_page_source_budget_exceeded"
            | "provider_page_warning_bound_exceeded"
            | "provider_page_sequence_mismatch"
            | "provider_page_snapshot_mismatch"
            | "provider_page_completion_tokens_invalid"
            | "provider_page_scan_progress_invalid"
            | "provider_page_empty_without_progress"
            | "provider_page_scan_token_unchanged"
            | "provider_page_tail_anchor_invalid"
            | "provider_page_duplicate_turn_id"
            | "provider_page_turn_sequence_mismatch"
            | "provider_page_turn_timestamp_invalid"
            | "provider_page_inline_body_missing"
            | "provider_page_inline_body_invalid"
            | "provider_page_inline_body_metadata_mismatch"
            | "provider_page_canonical_text_digest_mismatch"
            | "provider_page_body_state_invalid"
            | "provider_page_digest_invalid"
            | "provider_page_field_bound_invalid"
            | "provider_page_digest_failed"
            | "session_turn_stream_identity_mismatch"
            | "session_turn_stream_projection_mismatch"
            | "session_turn_checkpoint_invalid"
    )
}

// Only fixed host-recognized tokens may cross into durable state/UI. Provider
// messages and arbitrary provider code strings are not diagnostic authority.
fn disposition_token(error: &SessionProviderError) -> &str {
    if unsupported_error(error) || permanent_protocol_error(error) {
        return error.token();
    }
    match error.token() {
        "provider_process_timeout"
        | "session_turn_page_io"
        | "session_turn_page_state_failed"
        | "session_provider_describe_unavailable" => error.token(),
        _ => "provider_page_failed",
    }
}

fn retry_backoff(retry_count: u64) -> ChronoDuration {
    let exponent = retry_count.min(8) as u32;
    let seconds = 1_i64
        .checked_shl(exponent)
        .unwrap_or(MAX_RETRY_BACKOFF_SECS)
        .min(MAX_RETRY_BACKOFF_SECS);
    ChronoDuration::seconds(seconds)
}

fn state_error(error: String) -> SessionProviderError {
    SessionProviderError::new("session_turn_page_state_failed", error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_provider::SessionProviderError;

    #[test]
    fn deterministic_page_conflicts_are_permanent() {
        for token in [
            "provider_page_identity_mismatch",
            "provider_page_sequence_mismatch",
            "provider_page_snapshot_mismatch",
            "provider_page_duplicate_turn_id",
            "provider_page_inline_body_metadata_mismatch",
            "provider_page_canonical_text_digest_mismatch",
        ] {
            assert!(permanent_protocol_error(&SessionProviderError::new(
                token, token
            )));
        }
        for token in ["provider_page_failed", "provider_process_timeout"] {
            assert!(!permanent_protocol_error(&SessionProviderError::new(
                token, token
            )));
        }
    }

    #[test]
    fn age343_disposition_retains_checkpoint_and_never_persists_provider_text() {
        use crate::provider_registry::ProviderRegistryOptions;
        use oulipoly_config::ProvidersConfig;
        for (token, expected_status, expected_error) in [
            (
                "codex_rollout_capacity",
                "unsupported",
                "codex_rollout_capacity",
            ),
            (
                "session_turn_page_budget_too_small",
                "unsupported",
                "session_turn_page_budget_too_small",
            ),
            (
                "session_turn_record_ceiling_exceeded",
                "unsupported",
                "session_turn_record_ceiling_exceeded",
            ),
            (
                "session_turn_page_token_stale",
                "quarantined",
                "session_turn_page_token_stale",
            ),
            (
                "invalid_session_read_turns_params",
                "quarantined",
                "invalid_session_read_turns_params",
            ),
            ("session_turn_page_io", "retry_wait", "session_turn_page_io"),
            (
                "private/provider/body/token",
                "retry_wait",
                "provider_page_failed",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let state = StateDb::open(&temp.path().join("state.db")).unwrap();
            let registry = ProviderRegistry::from_configs(
                &[],
                &ProvidersConfig::default(),
                ProviderRegistryOptions::default(),
            )
            .unwrap();
            let key = SessionTurnIngestStreamKey {
                provider_name: "codex".into(),
                provider_instance_id: "instance".into(),
                settings_id: "codex".into(),
                session_id: "synthetic".into(),
                projection: SessionTurnStreamProjection::CanonicalIngest,
            };
            let strategy = oulipoly_config::ResumeStrategy {
                kind: oulipoly_config::ResumeKind::Subcommand,
                flag: None,
                subcommand: Some(vec!["resume".into()]),
            };
            let resume_before =
                crate::executor::cli::compose_resume_args(&strategy, &key.session_id).unwrap();
            state.enqueue_session_turn_ingest_stream(&key).unwrap();
            let now = Utc::now();
            let stream = state
                .lease_session_turn_ingest_stream(
                    &key,
                    "test",
                    now,
                    now + ChronoDuration::seconds(75),
                )
                .unwrap()
                .unwrap();
            use oulipoly_state::{
                SessionTurnPageApply, SessionTurnPageBodyState, SessionTurnPageTurnIngest,
            };
            let prefix = SessionTurnPageApply {
                key: key.clone(),
                lease_owner: "test".into(),
                expected_generation: 0,
                request_token_sha256: "1".repeat(64),
                snapshot_id: "snapshot".into(),
                page_index: 0,
                page_start_sequence: 0,
                page_turn_count: 1,
                scan_progress: false,
                snapshot_complete: false,
                next_page_token: Some("exact-failed-boundary".into()),
                resume_token: None,
                page_digest: "2".repeat(64),
                turns: vec![SessionTurnPageTurnIngest {
                    session_id: key.session_id.clone(),
                    turn_id: "prefix".into(),
                    snapshot_sequence: 0,
                    timestamp: now,
                    role: "user".into(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body_state: SessionTurnPageBodyState::Absent,
                    body: None,
                    body_bytes: None,
                    body_sha256: None,
                    canonical_text_sha256: None,
                    canonical_text_digest_verified: false,
                }],
            };
            assert_eq!(stream.checkpoint_generation, 0);
            state.apply_session_turn_page(&prefix).unwrap();
            let stream = state
                .lease_session_turn_ingest_stream(
                    &key,
                    "test",
                    now + ChronoDuration::seconds(1),
                    now + ChronoDuration::seconds(75),
                )
                .unwrap()
                .unwrap();
            // Reacquired lease permits replay without duplicating prefix effects.
            assert!(state.apply_session_turn_page(&prefix).unwrap().replayed);
            assert_eq!(stream.checkpoint_generation, 1);
            let stream = state
                .lease_session_turn_ingest_stream(
                    &key,
                    "test",
                    now + ChronoDuration::seconds(1),
                    now + ChronoDuration::seconds(75),
                )
                .unwrap()
                .unwrap();
            let before = stream.clone();
            assert_eq!(before.committed_turn_count, 1);
            assert_eq!(
                before.next_page_token.as_deref(),
                Some("exact-failed-boundary")
            );
            let cancellation = CancellationToken::new();
            let outcome = dispose_failed_quantum(
                SessionTurnIngestDriverRequest {
                    state: &state,
                    registry: &registry,
                    lease_owner: "test",
                    effective_cwd: None,
                    cancellation: &cancellation,
                    now,
                },
                stream,
                SessionProviderError::new(token, "private diagnostic body"),
            )
            .unwrap();
            let after = state.session_turn_ingest_stream(&key).unwrap().unwrap();
            assert_eq!(after.status, expected_status);
            let resume_after =
                crate::executor::cli::compose_resume_args(&strategy, &after.key.session_id)
                    .unwrap();
            assert_eq!(resume_before, resume_after);
            assert_eq!(resume_after, vec!["resume", "synthetic"]);
            let conn = rusqlite::Connection::open(temp.path().join("state.db")).unwrap();
            let stored: String = conn
                .query_row(
                    "SELECT last_error FROM session_turn_ingest_streams",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(stored, expected_error);
            assert_eq!(after.checkpoint_generation, before.checkpoint_generation);
            assert_eq!(after.snapshot_id, before.snapshot_id);
            assert_eq!(after.next_page_token, before.next_page_token);
            assert_eq!(after.after_token, before.after_token);
            assert_eq!(after.committed_turn_count, before.committed_turn_count);
            assert_eq!(after.expected_page_index, before.expected_page_index);
            assert_eq!(after.expected_turn_sequence, before.expected_turn_sequence);
            assert_eq!(after.lease_owner, None);
            assert!(!format!("{outcome:?}").contains("private"));
            // Existing state semantics count every worker failure, not just retries.
            assert_eq!(after.retry_count, before.retry_count + 1);
            if expected_status != "retry_wait" {
                assert!(
                    state
                        .lease_ready_session_turn_ingest_stream(
                            SessionTurnStreamProjection::CanonicalIngest,
                            "next",
                            now + ChronoDuration::days(1),
                            now + ChronoDuration::days(2)
                        )
                        .unwrap()
                        .is_none()
                );
            } else {
                assert!(
                    state
                        .lease_session_turn_ingest_stream(
                            &key,
                            "retry",
                            now + ChronoDuration::minutes(10),
                            now + ChronoDuration::minutes(11)
                        )
                        .unwrap()
                        .is_some()
                );
            }
        }
    }
}
