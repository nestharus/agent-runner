use serde::{Deserialize, Serialize};

mod auto_wake_environment;
mod cancellation;

pub use auto_wake_environment::AutoWakeEnvironmentVariable;
pub use cancellation::{CancellationRegistration, CancellationToken};

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
