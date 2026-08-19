use serde::{Deserialize, Serialize};

mod cancellation;

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
