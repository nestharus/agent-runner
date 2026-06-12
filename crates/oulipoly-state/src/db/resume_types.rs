//! ## Declared roles
//!
//! - mapper
//!
//! Role set: { mapper }
//!
//! Resume public mappers and private query row mappers.
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/resume_types.rs
//!     role: intrinsic-surface
//!     Domain: provider-session-id-grammar
//!     Owns:
//!       - OpenCode provider session ID prefix `ses_`
//!       - OpenCode provider session ID minimum alphanumeric suffix length accepted by StateDb resume input validation
//!       - StateDb opaque provider-session resume identity DTO boundary
//!   - component: crates/oulipoly-state/src/db/resume_types.rs
//!     role: intrinsic-surface
//!     Domain: resume-dto-boundary
//!     Owns:
//!       - ModelStore alias over std::collections::HashMap and ModelConfig
//!       - ResolvedResume, ResumeError, ChainPreview, TurnPreview, and row DTO fields
//!       - chrono DateTime/Utc timestamp fields carried by resume previews
//! ```

use chrono::{DateTime, Utc};
use oulipoly_config::ModelConfig;

pub(super) const OPENCODE_SESSION_PREFIX: &str = "ses_";
pub(super) const OPENCODE_SESSION_MIN_SUFFIX_LEN: usize = 3;

pub type ModelStore = std::collections::HashMap<String, ModelConfig>;

#[derive(Debug, Clone)]
pub struct ResolvedResume {
    pub chain_id: String,
    pub model_name: Option<String>,
    pub model: Option<ModelConfig>,
    pub active_provider: String,
    pub active_session_id: String,
}

#[derive(Debug, Clone)]
pub enum ResumeError {
    InvalidUuid {
        input: String,
    },
    NoChainFound {
        input: String,
    },
    WrongIdKind {
        input: String,
        input_kind: WrongIdKindInput,
        provider_session_id: Option<String>,
        agent_runner_invocation_id: String,
        chain_id: Option<String>,
        provider_name: Option<String>,
    },
    Ambiguous {
        input: String,
        previews: Vec<ChainPreview>,
    },
    ProviderModelMismatch {
        model_name: String,
        active_provider: String,
        suggestions: Vec<String>,
    },
    ProviderNotConfigured {
        provider: String,
    },
    UnknownModel {
        model_name: String,
    },
    ActiveSegmentMissing {
        chain_id: String,
    },
    ProviderMissingResume {
        provider_name: String,
    },
    Db {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrongIdKindInput {
    AgentRunnerInvocationId,
}

#[derive(Debug, Clone)]
pub struct ChainPreview {
    pub chain_id: String,
    pub last_used_at: DateTime<Utc>,
    pub active_provider: String,
    pub active_session_id: String,
    pub turn_count: usize,
    pub recent_turns: Vec<TurnPreview>,
}

#[derive(Debug, Clone)]
pub struct TurnPreview {
    pub role: String,
    pub timestamp: DateTime<Utc>,
    pub snippet: Option<String>,
}

pub(super) struct WrongIdKindInvocationMatch {
    pub(super) invocation_uuid: String,
    pub(super) provider_name: Option<String>,
    pub(super) provider_session_id: Option<String>,
    pub(super) chain_id: Option<String>,
}

pub(super) struct WrongIdKindInvocationRow {
    pub(super) invocation_uuid: String,
    pub(super) provider_name: Option<String>,
    pub(super) provider_session_id: Option<String>,
}

pub(super) struct RecentTurnRow {
    pub(super) role: String,
    pub(super) timestamp_raw: String,
}

pub(super) struct ParsedTurnPreviewTimestamp {
    pub(super) role: String,
    pub(super) timestamp: DateTime<Utc>,
}

pub(super) struct ResumeChainCandidate {
    pub(super) chain_id: String,
    pub(super) last_used_at: DateTime<Utc>,
    pub(super) latest_segment_started_at: DateTime<Utc>,
}
