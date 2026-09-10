//! Decodes inherited automatic-wake process context and child-admission markers.
//!
//! ## Declared roles
//!
//! `accessor`, `parser`, `predicate`

use oulipoly_core::AutoWakeEnvironmentVariable;

use super::constants::DEFAULT_AUTO_WAKE_RETRY_BASE_MS;

pub(super) struct AutoWakeEnv {
    pub(super) token: String,
    /// Chronology and cadence input, never an eligibility or exhaustion budget.
    pub(super) chronological_attempt_count: i64,
    pub(super) retry_base_milliseconds: u64,
}

pub(super) struct AutoWakeChildMarker {
    expected_session: String,
    claim_token: String,
}

impl AutoWakeChildMarker {
    pub(super) fn matches_session(&self, session_id: &str) -> bool {
        self.expected_session == session_id && !self.claim_token.is_empty()
    }

    pub(super) fn claim_token(&self) -> &str {
        &self.claim_token
    }
}

pub(crate) fn is_auto_wake_invocation() -> bool {
    auto_wake_marker_present()
}

pub(super) fn auto_wake_marker_present() -> bool {
    std::env::var_os(AutoWakeEnvironmentVariable::MARKER.name()).is_some()
}

pub(super) fn current_auto_wake_child_marker() -> AutoWakeChildMarker {
    AutoWakeChildMarker {
        expected_session: std::env::var(AutoWakeEnvironmentVariable::SESSION_ID.name())
            .unwrap_or_default(),
        claim_token: std::env::var(AutoWakeEnvironmentVariable::CLAIM_TOKEN.name())
            .unwrap_or_default(),
    }
}

pub(super) fn current_auto_wake() -> Option<AutoWakeEnv> {
    if !auto_wake_marker_present() {
        return None;
    }
    let token = std::env::var(AutoWakeEnvironmentVariable::CLAIM_TOKEN.name()).ok()?;
    let chronological_attempt_count = std::env::var(AutoWakeEnvironmentVariable::COUNT.name())
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let retry_base_milliseconds =
        std::env::var(AutoWakeEnvironmentVariable::RETRY_BASE_MILLISECONDS.name())
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_AUTO_WAKE_RETRY_BASE_MS);
    Some(AutoWakeEnv {
        token,
        chronological_attempt_count,
        retry_base_milliseconds,
    })
}
