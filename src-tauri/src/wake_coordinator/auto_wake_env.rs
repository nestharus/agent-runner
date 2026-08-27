//! ## Declared roles
//!
//! `accessor`, `mapper`, `parser`, `predicate`

use oulipoly_core::AutoWakeEnvironmentVariable;

use super::constants::DEFAULT_AUTO_WAKE_RETRY_BASE_MS;

pub(super) struct AutoWakeEnv {
    pub(super) token: String,
    pub(super) count: i64,
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
        expected_session: auto_wake_expected_session(),
        claim_token: auto_wake_child_claim_token(),
    }
}

fn auto_wake_expected_session() -> String {
    std::env::var(AutoWakeEnvironmentVariable::SESSION_ID.name()).unwrap_or_default()
}

fn auto_wake_child_claim_token() -> String {
    std::env::var(AutoWakeEnvironmentVariable::CLAIM_TOKEN.name()).unwrap_or_default()
}

pub(super) fn current_auto_wake_count(auto_wake: Option<&AutoWakeEnv>) -> i64 {
    auto_wake.map(|wake| wake.count).unwrap_or(0)
}

pub(super) fn current_auto_wake() -> Option<AutoWakeEnv> {
    auto_wake_marker_present()
        .then(current_auto_wake_env)
        .flatten()
}

fn current_auto_wake_env() -> Option<AutoWakeEnv> {
    Some(AutoWakeEnv {
        token: auto_wake_token()?,
        count: auto_wake_count(),
        retry_base_milliseconds: auto_wake_retry_base_milliseconds(),
    })
}

fn auto_wake_count() -> i64 {
    parse_auto_wake_count(auto_wake_count_value())
}

fn auto_wake_token() -> Option<String> {
    std::env::var(AutoWakeEnvironmentVariable::CLAIM_TOKEN.name()).ok()
}

fn auto_wake_count_value() -> Option<String> {
    std::env::var(AutoWakeEnvironmentVariable::COUNT.name()).ok()
}

fn parse_auto_wake_count(value: Option<String>) -> i64 {
    value.and_then(|value| value.parse().ok()).unwrap_or(1)
}

fn auto_wake_retry_base_milliseconds() -> u64 {
    parsed_auto_wake_retry_base_milliseconds()
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_AUTO_WAKE_RETRY_BASE_MS)
}

fn parsed_auto_wake_retry_base_milliseconds() -> Option<u64> {
    parse_auto_wake_retry_base_milliseconds(auto_wake_retry_base_milliseconds_text())
}

fn auto_wake_retry_base_milliseconds_text() -> Option<String> {
    std::env::var(AutoWakeEnvironmentVariable::RETRY_BASE_MILLISECONDS.name()).ok()
}

fn parse_auto_wake_retry_base_milliseconds(value: Option<String>) -> Option<u64> {
    value.and_then(|value| value.parse().ok())
}
