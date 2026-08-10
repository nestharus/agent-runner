//! Typed persistence contract for durable fresh continuations.
//!
//! ## Declared roles
//!
//! `accessor`, `formatter`

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationAcceptInput {
    pub logical_request_key: String,
    pub fingerprint: String,
    pub origin_invocation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationReservation {
    pub invocation_id: String,
    pub parent_invocation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinuationRecord {
    pub logical_request_key: String,
    pub continuation_id: String,
    pub fingerprint: String,
    pub resume: ContinuationReservation,
    pub fresh: ContinuationReservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContinuationResumeAcceptance {
    #[serde(rename = "Accepted")]
    Accepted,
    #[serde(rename = "Rejected")]
    Rejected,
    #[serde(rename = "Unconfirmed")]
    Unconfirmed,
    #[serde(rename = "NotApplicable")]
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContinuationInvocationDisposition {
    #[serde(rename = "Succeeded")]
    Succeeded,
    #[serde(rename = "Failed")]
    Failed {
        error_category: String,
        terminal_reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationInvocationOutcome {
    pub invocation_id: String,
    pub session_id: Option<String>,
    pub physical_exit_code: i32,
    pub acceptance: ContinuationResumeAcceptance,
    pub disposition: ContinuationInvocationDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuationPublishedHandoff {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContinuationTerminalOutcome {
    Continued {
        continuation_id: String,
        resume: ContinuationInvocationOutcome,
        fresh: ContinuationInvocationOutcome,
        handoff: ContinuationPublishedHandoff,
    },
    Failed {
        continuation_id: String,
        resume: ContinuationInvocationOutcome,
        fresh: ContinuationInvocationOutcome,
        handoff: ContinuationPublishedHandoff,
        reason: String,
    },
}

impl ContinuationTerminalOutcome {
    pub(crate) fn handoff(&self) -> &ContinuationPublishedHandoff {
        match self {
            Self::Continued { handoff, .. } | Self::Failed { handoff, .. } => handoff,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationAcceptResult {
    Accepted(ContinuationRecord),
    Replay(ContinuationTerminalOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationRunDecision {
    Run(ContinuationReservation),
    Observe(ContinuationReservation),
    Terminal(Box<ContinuationTerminalOutcome>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContinuationRepositoryError {
    Conflict(String),
    AmbiguousState(String),
    Persistence(String),
}

impl fmt::Display for ContinuationRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(message)
            | Self::AmbiguousState(message)
            | Self::Persistence(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ContinuationRepositoryError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_invocation_outcome_json_shape_is_stable() {
        let succeeded = ContinuationInvocationOutcome {
            invocation_id: "resume-invocation".to_string(),
            session_id: Some("session-1".to_string()),
            physical_exit_code: 0,
            acceptance: ContinuationResumeAcceptance::Accepted,
            disposition: ContinuationInvocationDisposition::Succeeded,
        };
        let succeeded_json = r#"{"invocation_id":"resume-invocation","session_id":"session-1","physical_exit_code":0,"acceptance":"Accepted","disposition":"Succeeded"}"#;
        assert_eq!(serde_json::to_string(&succeeded).unwrap(), succeeded_json);
        assert_eq!(
            serde_json::from_str::<ContinuationInvocationOutcome>(succeeded_json).unwrap(),
            succeeded
        );

        let failed = ContinuationInvocationOutcome {
            invocation_id: "fresh-invocation".to_string(),
            session_id: None,
            physical_exit_code: 1,
            acceptance: ContinuationResumeAcceptance::NotApplicable,
            disposition: ContinuationInvocationDisposition::Failed {
                error_category: "invocation".to_string(),
                terminal_reason: "process exited".to_string(),
            },
        };
        let failed_json = r#"{"invocation_id":"fresh-invocation","session_id":null,"physical_exit_code":1,"acceptance":"NotApplicable","disposition":{"Failed":{"error_category":"invocation","terminal_reason":"process exited"}}}"#;
        assert_eq!(serde_json::to_string(&failed).unwrap(), failed_json);
        assert_eq!(
            serde_json::from_str::<ContinuationInvocationOutcome>(failed_json).unwrap(),
            failed
        );
    }
}
