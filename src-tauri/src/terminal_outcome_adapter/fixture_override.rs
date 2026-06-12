//! AGE-153 terminal-signal fixture override parsing and application.
//!
//! ## Declared roles
//!
//! `parser`, `mapper`, `validator`

use crate::terminal_outcome_adapter::typed_terminal_reason_fallback;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{ExecutionResult, TerminalSignal};
use std::sync::atomic::{AtomicUsize, Ordering};

static AGE153_FORCE_TERMINAL_SIGNAL_SEQUENCE_INDEX: AtomicUsize = AtomicUsize::new(0);

enum Age153TerminalSignalFixtureOverride {
    Clear,
    Force(TerminalSignalKind),
    Unset,
}

pub fn apply_age153_terminal_signal_fixture_override(result: &mut ExecutionResult) {
    apply_age153_terminal_signal_fixture_override_to_fields(
        &mut result.terminal_signal,
        &mut result.terminal_reason,
    );
}

pub fn apply_age153_terminal_signal_fixture_override_to_fields(
    terminal_signal: &mut Option<TerminalSignal>,
    terminal_reason: &mut Option<String>,
) {
    match age153_terminal_signal_fixture_override() {
        Age153TerminalSignalFixtureOverride::Clear => {
            clear_terminal_signal_fixture_override(terminal_signal, terminal_reason)
        }
        Age153TerminalSignalFixtureOverride::Force(kind) => {
            force_terminal_signal_fixture_override_fields(terminal_signal, terminal_reason, kind)
        }
        Age153TerminalSignalFixtureOverride::Unset => {}
    }
}

fn age153_terminal_signal_fixture_override() -> Age153TerminalSignalFixtureOverride {
    if age153_force_terminal_signal_none_requested() {
        return Age153TerminalSignalFixtureOverride::Clear;
    }
    age153_forced_terminal_signal_override()
}

fn age153_force_terminal_signal_none_requested() -> bool {
    std::env::var_os("OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_NONE").is_some()
}

fn is_clear_or_none_fixture_token(token: &str) -> bool {
    matches!(token, "None" | "Clear")
}

fn age153_fixture_override_from_kind(
    kind: Option<TerminalSignalKind>,
) -> Age153TerminalSignalFixtureOverride {
    kind.map(Age153TerminalSignalFixtureOverride::Force)
        .unwrap_or(Age153TerminalSignalFixtureOverride::Unset)
}

fn age153_forced_terminal_signal_override() -> Age153TerminalSignalFixtureOverride {
    let Some(value) = age153_forced_terminal_signal_kind_value() else {
        return Age153TerminalSignalFixtureOverride::Unset;
    };
    let token = age153_forced_terminal_signal_token(&value);
    if is_clear_or_none_fixture_token(token) {
        return Age153TerminalSignalFixtureOverride::Clear;
    }
    age153_fixture_override_from_kind(terminal_signal_kind_from_env(token))
}

fn age153_forced_terminal_signal_kind_value() -> Option<String> {
    std::env::var("OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND").ok()
}

fn parse_fixture_tokens(value: &str) -> Vec<&str> {
    value
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect()
}

fn select_token_by_sequence_index<'a>(all_tokens: &[&'a str], fallback: &'a str) -> &'a str {
    if all_tokens.is_empty() {
        return fallback;
    }
    let index = AGE153_FORCE_TERMINAL_SIGNAL_SEQUENCE_INDEX.fetch_add(1, Ordering::Relaxed);
    let len = all_tokens.len();
    all_tokens.get(index % len).copied().unwrap_or(fallback)
}

pub(super) fn age153_forced_terminal_signal_token(value: &str) -> &str {
    let all_tokens = parse_fixture_tokens(value);
    select_token_by_sequence_index(&all_tokens, value)
}

fn clear_terminal_signal_fixture_override(
    terminal_signal: &mut Option<TerminalSignal>,
    terminal_reason: &mut Option<String>,
) {
    *terminal_signal = None;
    *terminal_reason = None;
}

#[cfg(test)]
pub(super) fn force_terminal_signal_fixture_override(
    result: &mut ExecutionResult,
    kind: TerminalSignalKind,
) {
    force_terminal_signal_fixture_override_fields(
        &mut result.terminal_signal,
        &mut result.terminal_reason,
        kind,
    );
}

fn build_forced_terminal_signal(
    existing: Option<TerminalSignal>,
    kind: TerminalSignalKind,
) -> TerminalSignal {
    match existing {
        Some(signal) => forced_existing_terminal_signal(signal, kind),
        None => forced_new_terminal_signal(kind),
    }
}

fn forced_existing_terminal_signal(
    mut signal: TerminalSignal,
    kind: TerminalSignalKind,
) -> TerminalSignal {
    signal.kind = kind;
    signal
}

fn forced_new_terminal_signal(kind: TerminalSignalKind) -> TerminalSignal {
    TerminalSignal {
        kind,
        provider_name: String::new(),
        evidence: "age153 fixture override".to_string(),
        observed_at: std::time::SystemTime::now(),
    }
}

fn force_terminal_signal_fixture_override_fields(
    terminal_signal: &mut Option<TerminalSignal>,
    terminal_reason: &mut Option<String>,
    kind: TerminalSignalKind,
) {
    let signal = build_forced_terminal_signal(terminal_signal.take(), kind);
    *terminal_reason = typed_terminal_reason_fallback(&signal).map(str::to_string);
    *terminal_signal = Some(signal);
}

pub(super) fn terminal_signal_kind_from_env(value: &str) -> Option<TerminalSignalKind> {
    match value {
        "CleanExit" => Some(TerminalSignalKind::CleanExit),
        "NonzeroExit" => Some(TerminalSignalKind::NonzeroExit),
        "SignalExit" => Some(TerminalSignalKind::SignalExit),
        "SpawnError" => Some(TerminalSignalKind::SpawnError),
        "QuotaExhaustedInband" => Some(TerminalSignalKind::QuotaExhaustedInband),
        "MaybeQuotaExhausted" => Some(TerminalSignalKind::MaybeQuotaExhausted),
        "RateLimited" => Some(TerminalSignalKind::RateLimited),
        "ProlongedSilence" => Some(TerminalSignalKind::ProlongedSilence),
        "Unknown" => Some(TerminalSignalKind::Unknown),
        _ => None,
    }
}

#[cfg(test)]
pub(super) fn reset_age153_force_terminal_signal_sequence() {
    AGE153_FORCE_TERMINAL_SIGNAL_SEQUENCE_INDEX.store(0, Ordering::Relaxed);
}
