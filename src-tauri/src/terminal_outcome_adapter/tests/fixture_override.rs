use super::{result_with_signal, signal};
use crate::terminal_outcome_adapter::fixture_override::{
    age153_forced_terminal_signal_token, force_terminal_signal_fixture_override,
    reset_age153_force_terminal_signal_sequence, terminal_signal_kind_from_env,
};
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;

#[test]
fn age153_fixture_parser_accepts_maybe_quota_exhausted_kind() {
    let parsed = terminal_signal_kind_from_env("MaybeQuotaExhausted");
    assert_eq!(parsed, Some(TerminalSignalKind::MaybeQuotaExhausted));

    let mut result = result_with_signal(None);
    force_terminal_signal_fixture_override(&mut result, TerminalSignalKind::MaybeQuotaExhausted);

    assert_eq!(
        result.terminal_signal.as_ref().map(|signal| signal.kind),
        Some(TerminalSignalKind::MaybeQuotaExhausted)
    );
    assert_eq!(
        result.terminal_reason.as_deref(),
        Some("maybe_quota_exhausted")
    );
}

#[test]
fn fixture_override_accepts_maybe_kind() {
    assert_eq!(
        terminal_signal_kind_from_env("MaybeQuotaExhausted"),
        Some(TerminalSignalKind::MaybeQuotaExhausted)
    );
}

#[test]
fn age153_forced_terminal_signal_token_rotates_sequence() {
    reset_age153_force_terminal_signal_sequence();

    let first =
        age153_forced_terminal_signal_token("MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited");
    let second =
        age153_forced_terminal_signal_token("MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited");
    let third =
        age153_forced_terminal_signal_token("MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited");
    let fourth =
        age153_forced_terminal_signal_token("MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited");

    assert_eq!(first, "MaybeQuotaExhausted");
    assert_eq!(second, "QuotaExhaustedInband");
    assert_eq!(third, "RateLimited");
    assert_eq!(fourth, "MaybeQuotaExhausted");
}

#[test]
fn forced_override_preserves_existing_signal_evidence() {
    let mut result = result_with_signal(Some(TerminalSignalKind::Unknown));
    let original = signal(TerminalSignalKind::Unknown).evidence;
    if let Some(signal) = result.terminal_signal.as_mut() {
        signal.evidence = original.clone();
    }

    force_terminal_signal_fixture_override(&mut result, TerminalSignalKind::QuotaExhaustedInband);

    let signal = result.terminal_signal.expect("forced signal");
    assert_eq!(signal.kind, TerminalSignalKind::QuotaExhaustedInband);
    assert_eq!(signal.evidence, original);
}
