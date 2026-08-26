use serde::{Deserialize, Serialize};

mod auto_wake_environment;
mod cancellation;

pub use auto_wake_environment::{
    AUTO_WAKE_COUNT_ENV, AUTO_WAKE_ENV, AUTO_WAKE_RETRY_BASE_MS_ENV, AUTO_WAKE_SESSION_ID_ENV,
    AUTO_WAKE_TOKEN_ENV, RUNNER_PRIVATE_AUTO_WAKE_ENV_NAMES,
};
pub use cancellation::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionReason {
    Initial,
    Manual,
    QuotaThreshold,
    Exhausted,
    Imported,
}

impl TransitionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            TransitionReason::Initial => "initial",
            TransitionReason::Manual => "manual",
            TransitionReason::QuotaThreshold => "quota_threshold",
            TransitionReason::Exhausted => "exhausted",
            TransitionReason::Imported => "imported",
        }
    }
}
