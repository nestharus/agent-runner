//! ## Declared roles
//!
//! `validator`, `orchestration`, `accessor`, `mapper`
//!
//! Provider-neutral expected-versus-observed session authority.
//!
//! Transport adapters supply an authenticated observation. This module alone
//! compares it with launch intent and commits the verified binding.

use oulipoly_state::{ProviderSessionAuthorityCommit, ProviderSessionBinding, StateDb};

#[derive(Debug, Clone, Copy)]
pub struct SessionAuthorityExpectation<'a> {
    pub account_name: &'a str,
    pub provider_session_id: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
pub struct AuthoritativeSessionObservation<'a> {
    pub account_name: &'a str,
    pub provider_session_id: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSessionAuthority {
    account_name: String,
    provider_session_id: String,
}

impl VerifiedSessionAuthority {
    pub fn account_name(&self) -> &str {
        &self.account_name
    }

    pub fn provider_session_id(&self) -> &str {
        &self.provider_session_id
    }
}

pub struct SessionAuthorityCommitRequest<'a> {
    pub state: &'a StateDb,
    pub invocation_row_id: i64,
    pub invocation_uuid: &'a str,
    pub expectation: SessionAuthorityExpectation<'a>,
    pub observation: Option<AuthoritativeSessionObservation<'a>>,
    pub capture_method: &'static str,
    pub resume_input_id: Option<String>,
    pub provider_session_resolved_account: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SessionAuthorityError {
    #[error(
        "authoritative session observation missing for account {account_name} and expected session {provider_session_id}"
    )]
    MissingObservation {
        account_name: String,
        provider_session_id: String,
    },
    #[error("authoritative session observation has an empty account name")]
    EmptyObservedAccount,
    #[error("authoritative session observation has an empty provider session ID")]
    EmptyObservedSession,
    #[error(
        "authoritative session account mismatch: expected {expected_account}, observed {observed_account}"
    )]
    AccountMismatch {
        expected_account: String,
        observed_account: String,
    },
    #[error(
        "authoritative provider session mismatch for account {account_name}: expected {expected_session_id}, observed {observed_session_id}"
    )]
    SessionMismatch {
        account_name: String,
        expected_session_id: String,
        observed_session_id: String,
    },
    #[error("failed to commit provider session authority: {message}")]
    Persistence { message: String },
}

impl SessionAuthorityError {
    pub fn protocol_kind(&self) -> &'static str {
        match self {
            Self::MissingObservation { .. } => "session_identity_observation_missing",
            Self::EmptyObservedAccount => "session_identity_account_empty",
            Self::EmptyObservedSession => "session_identity_empty",
            Self::AccountMismatch { .. } => "session_identity_account_mismatch",
            Self::SessionMismatch { .. } => "session_identity_mismatch",
            Self::Persistence { .. } => "session_identity_commit_failed",
        }
    }
}

pub fn verify_session_authority(
    expectation: SessionAuthorityExpectation<'_>,
    observation: Option<AuthoritativeSessionObservation<'_>>,
) -> Result<Option<VerifiedSessionAuthority>, SessionAuthorityError> {
    let Some(observation) = observation else {
        return match expectation.provider_session_id {
            Some(provider_session_id) => Err(SessionAuthorityError::MissingObservation {
                account_name: expectation.account_name.to_string(),
                provider_session_id: provider_session_id.to_string(),
            }),
            None => Ok(None),
        };
    };
    if observation.account_name.trim().is_empty() {
        return Err(SessionAuthorityError::EmptyObservedAccount);
    }
    if observation.provider_session_id.trim().is_empty() {
        return Err(SessionAuthorityError::EmptyObservedSession);
    }
    if observation.account_name != expectation.account_name {
        return Err(SessionAuthorityError::AccountMismatch {
            expected_account: expectation.account_name.to_string(),
            observed_account: observation.account_name.to_string(),
        });
    }
    if let Some(expected_session_id) = expectation.provider_session_id
        && observation.provider_session_id != expected_session_id
    {
        return Err(SessionAuthorityError::SessionMismatch {
            account_name: expectation.account_name.to_string(),
            expected_session_id: expected_session_id.to_string(),
            observed_session_id: observation.provider_session_id.to_string(),
        });
    }
    Ok(Some(VerifiedSessionAuthority {
        account_name: observation.account_name.to_string(),
        provider_session_id: observation.provider_session_id.to_string(),
    }))
}

pub fn commit_session_authority(
    request: SessionAuthorityCommitRequest<'_>,
) -> Result<Option<VerifiedSessionAuthority>, SessionAuthorityError> {
    let verified = verify_session_authority(request.expectation, request.observation)?;
    let Some(verified) = verified else {
        return Ok(None);
    };
    request
        .state
        .commit_invocation_provider_session_authority(
            request.invocation_row_id,
            &ProviderSessionAuthorityCommit {
                invocation_uuid: request.invocation_uuid,
                provider_name: verified.account_name(),
                binding: &ProviderSessionBinding {
                    provider_session_id: verified.provider_session_id().to_string(),
                    capture_method: request.capture_method,
                    resume_input_id: request.resume_input_id,
                    provider_session_resolved_account: request.provider_session_resolved_account,
                },
            },
        )
        .map_err(|message| SessionAuthorityError::Persistence { message })?;
    Ok(Some(verified))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::{InvocationStart, InvocationStatus};

    const INVOCATION_UUID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
    const ACCOUNT: &str = "account-a";
    const SESSION: &str = "session-a";

    #[test]
    fn exact_observation_commits_binding_and_chain_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDb::open(&temp.path().join("state.db")).unwrap();
        let row_id = running_invocation(&state);

        let verified = commit_session_authority(SessionAuthorityCommitRequest {
            state: &state,
            invocation_row_id: row_id,
            invocation_uuid: INVOCATION_UUID,
            expectation: expectation(Some(SESSION)),
            observation: Some(observation(SESSION)),
            capture_method: "external_provider_launch",
            resume_input_id: Some("requested-session".to_string()),
            provider_session_resolved_account: None,
        })
        .unwrap()
        .unwrap();

        assert_eq!(verified.account_name(), ACCOUNT);
        assert_eq!(verified.provider_session_id(), SESSION);
        let row = state
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Running);
        assert_eq!(row.provider_session_id.as_deref(), Some(SESSION));
        assert_eq!(
            row.provider_session_capture_method.as_deref(),
            Some("external_provider_launch")
        );
        assert_eq!(row.resume_input_id.as_deref(), Some("requested-session"));
        assert!(
            state
                .chain_id_for_segment(ACCOUNT, SESSION)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn mismatch_and_missing_observation_commit_nothing() {
        for (observation, expected_kind) in [
            (
                Some(observation("other-session")),
                "session_identity_mismatch",
            ),
            (None, "session_identity_observation_missing"),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let state = StateDb::open(&temp.path().join("state.db")).unwrap();
            let row_id = running_invocation(&state);
            let error = commit_session_authority(SessionAuthorityCommitRequest {
                state: &state,
                invocation_row_id: row_id,
                invocation_uuid: INVOCATION_UUID,
                expectation: expectation(Some(SESSION)),
                observation,
                capture_method: "external_provider_launch",
                resume_input_id: None,
                provider_session_resolved_account: None,
            })
            .unwrap_err();

            assert_eq!(error.protocol_kind(), expected_kind);
            let row = state
                .get_invocation_by_uuid(INVOCATION_UUID)
                .unwrap()
                .unwrap();
            assert_eq!(row.provider_session_id, None);
            assert_eq!(row.provider_session_capture_method, None);
            assert_eq!(state.chain_id_for_segment(ACCOUNT, SESSION).unwrap(), None);
        }
    }

    #[test]
    fn invocation_identity_failure_rolls_back_binding_and_chain() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDb::open(&temp.path().join("state.db")).unwrap();
        let row_id = running_invocation(&state);

        let error = commit_session_authority(SessionAuthorityCommitRequest {
            state: &state,
            invocation_row_id: row_id,
            invocation_uuid: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
            expectation: expectation(Some(SESSION)),
            observation: Some(observation(SESSION)),
            capture_method: "external_provider_launch",
            resume_input_id: None,
            provider_session_resolved_account: None,
        })
        .unwrap_err();

        assert_eq!(error.protocol_kind(), "session_identity_commit_failed");
        let row = state
            .get_invocation_by_uuid(INVOCATION_UUID)
            .unwrap()
            .unwrap();
        assert_eq!(row.provider_session_id, None);
        assert_eq!(row.provider_session_capture_method, None);
        assert_eq!(state.chain_id_for_segment(ACCOUNT, SESSION).unwrap(), None);
    }

    fn running_invocation(state: &StateDb) -> i64 {
        state
            .start_invocation(&InvocationStart {
                invocation_uuid: INVOCATION_UUID.to_string(),
                model_name: "model-a".to_string(),
                provider_name: ACCOUNT.to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap()
    }

    fn expectation(provider_session_id: Option<&str>) -> SessionAuthorityExpectation<'_> {
        SessionAuthorityExpectation {
            account_name: ACCOUNT,
            provider_session_id,
        }
    }

    fn observation(provider_session_id: &str) -> AuthoritativeSessionObservation<'_> {
        AuthoritativeSessionObservation {
            account_name: ACCOUNT,
            provider_session_id,
        }
    }
}
