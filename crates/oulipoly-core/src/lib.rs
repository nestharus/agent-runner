use serde::{Deserialize, Serialize};

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
            Self::Initial => "initial",
            Self::Manual => "manual",
            Self::QuotaThreshold => "quota_threshold",
            Self::Exhausted => "exhausted",
            Self::Imported => "imported",
        }
    }
}
