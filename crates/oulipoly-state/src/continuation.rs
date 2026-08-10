//! Typed persistence contract for durable fresh continuations.

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
    Accepted,
    Rejected,
    Unconfirmed,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContinuationInvocationDisposition {
    Succeeded,
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
