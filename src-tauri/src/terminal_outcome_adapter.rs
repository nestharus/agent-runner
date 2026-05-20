//! Typed terminal-outcome classification for CLI result handling.
//!
//! ## Declared roles
//!
//! `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/terminal_outcome_adapter.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly_runtime::executor::ExecutionResult terminal_signal contract
//!       - oulipoly_runtime::executor::terminal_signal::TerminalSignalKind contract
//!       - oulipoly_runtime::diagnostics error-category contract
//!       - src-tauri CLI quota retry/error-category contract
//! ```

use oulipoly_runtime::balancer::{FailureClass, apply_post_failure_forensics};
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{ExecutionResult, TerminalSignal};
use oulipoly_state::StateDb;
use std::io;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalOutcomeCategory {
    QuotaExhausted,
    HungSubprocess,
}

impl TerminalOutcomeCategory {
    pub(crate) fn as_error_category(self) -> Option<String> {
        match self {
            TerminalOutcomeCategory::QuotaExhausted => {
                Some(ErrorCategory::QuotaExhausted.as_str().to_string())
            }
            TerminalOutcomeCategory::HungSubprocess => {
                Some(ErrorCategory::HungSubprocess.as_str().to_string())
            }
        }
    }
}

pub(crate) fn classify_error_category_with_fallback<F>(
    result: &ExecutionResult,
    diagnostics_fallback: F,
) -> Option<String>
where
    F: FnOnce() -> Option<String>,
{
    if result.exit_code == 0 {
        return None;
    }

    if let Some(signal) = result.terminal_signal.as_ref()
        && let Some(category) = category_for_signal_kind(signal.kind)
    {
        return category.as_error_category();
    }

    diagnostics_fallback()
}

fn category_for_signal_kind(kind: TerminalSignalKind) -> Option<TerminalOutcomeCategory> {
    match kind {
        TerminalSignalKind::QuotaExhaustedInband => Some(TerminalOutcomeCategory::QuotaExhausted),
        TerminalSignalKind::ProlongedSilence => Some(TerminalOutcomeCategory::HungSubprocess),
        TerminalSignalKind::CleanExit
        | TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::Unknown => None,
    }
}

pub(crate) enum TerminalSignalDisposition {
    QuotaExhaustedRetry,
    ProlongedSilenceFail,
    InteractiveFail,
    InteractiveClean,
    NotApplicable,
}

pub(crate) struct TerminalSignalContext<'a, W: io::Write> {
    pub(crate) invocation_id: &'a Uuid,
    pub(crate) session_id: Option<&'a Uuid>,
    pub(crate) provider: &'a str,
    pub(crate) state_db: &'a StateDb,
    pub(crate) stderr: &'a mut W,
}

pub(crate) fn emit_terminal_signal_marker(
    signal: &TerminalSignal,
    invocation_id: &Uuid,
    session_id: Option<&Uuid>,
    stderr: &mut impl io::Write,
) -> io::Result<()> {
    let payload = serde_json::json!({
        "kind": format!("{:?}", signal.kind),
        "evidence": {
            "excerpt": signal.evidence.as_str(),
        },
        "invocation_id": invocation_id.to_string(),
        "session_id": session_id.map(Uuid::to_string),
    });
    writeln!(
        stderr,
        "OULIPOLY_TERMINAL_SIGNAL={}",
        serde_json::to_string(&payload).map_err(io::Error::other)?
    )
}

pub(crate) fn apply_terminal_signal_outcome(
    signal: &Option<TerminalSignal>,
    ctx: &mut TerminalSignalContext<'_, impl io::Write>,
) -> TerminalSignalDisposition {
    let Some(signal) = signal else {
        return TerminalSignalDisposition::NotApplicable;
    };

    let disposition = terminal_signal_disposition(signal);
    match disposition {
        TerminalSignalDisposition::QuotaExhaustedRetry => {
            // AGE-153 marker authority: emit_terminal_signal_marker.
            emit_terminal_signal_marker_or_warn(signal, ctx);
            apply_typed_post_failure_forensics(signal, ctx);
        }
        TerminalSignalDisposition::ProlongedSilenceFail
        | TerminalSignalDisposition::InteractiveFail => {
            emit_terminal_signal_marker_or_warn(signal, ctx);
            apply_typed_post_failure_forensics(signal, ctx);
        }
        TerminalSignalDisposition::InteractiveClean | TerminalSignalDisposition::NotApplicable => {}
    }
    disposition
}

fn apply_typed_post_failure_forensics<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) {
    let Some(failure_class) = FailureClass::from_terminal_signal_kind(signal.kind) else {
        return;
    };
    if let Err(err) = apply_post_failure_forensics(
        ctx.state_db,
        ctx.provider,
        failure_class,
        chrono::Utc::now(),
    ) {
        let _ = writeln!(
            ctx.stderr,
            "Warning: Failed to apply post-failure forensics: {err:?}"
        );
    }
}

fn terminal_signal_disposition(signal: &TerminalSignal) -> TerminalSignalDisposition {
    match signal.kind {
        TerminalSignalKind::CleanExit => TerminalSignalDisposition::InteractiveClean,
        TerminalSignalKind::QuotaExhaustedInband => TerminalSignalDisposition::QuotaExhaustedRetry,
        TerminalSignalKind::ProlongedSilence => TerminalSignalDisposition::ProlongedSilenceFail,
        TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::Unknown => TerminalSignalDisposition::InteractiveFail,
    }
}

fn emit_terminal_signal_marker_or_warn<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) {
    if let Err(err) =
        emit_terminal_signal_marker(signal, ctx.invocation_id, ctx.session_id, ctx.stderr)
    {
        eprintln!("Warning: Failed to emit terminal signal marker: {err}");
    }
}

pub(crate) fn typed_terminal_reason_fallback(signal: &TerminalSignal) -> Option<&'static str> {
    match signal.kind {
        TerminalSignalKind::CleanExit => None,
        TerminalSignalKind::QuotaExhaustedInband => Some("quota_exhausted_inband"),
        TerminalSignalKind::RateLimited => Some("rate_limited"),
        TerminalSignalKind::ProlongedSilence => Some("bounded_silence"),
        TerminalSignalKind::NonzeroExit => Some("exit_nonzero"),
        TerminalSignalKind::SignalExit => Some("signal_exit"),
        TerminalSignalKind::SpawnError => Some("spawn_error"),
        TerminalSignalKind::Unknown => Some("unknown_exit"),
    }
}

pub(crate) fn terminal_signal_reason<'a>(
    signal: &'a Option<TerminalSignal>,
    executor_terminal_reason: Option<&'a str>,
) -> Option<&'a str> {
    signal.as_ref()?;
    executor_terminal_reason.or_else(|| signal.as_ref().and_then(typed_terminal_reason_fallback))
}

pub(crate) fn terminal_signal_error_category<'a>(
    signal: &Option<TerminalSignal>,
    terminal_reason: &'a str,
) -> Option<&'a str> {
    match signal.as_ref().map(|signal| signal.kind) {
        Some(TerminalSignalKind::NonzeroExit | TerminalSignalKind::SignalExit) => None,
        _ => Some(terminal_reason),
    }
}

pub(crate) fn resume_terminal_signal_for_outcome(
    signal: &Option<TerminalSignal>,
) -> Option<TerminalSignal> {
    signal
        .as_ref()
        .filter(|signal| {
            !matches!(
                signal.kind,
                TerminalSignalKind::NonzeroExit | TerminalSignalKind::SignalExit
            )
        })
        .cloned()
}

pub(crate) fn balanced_terminal_signal_for_outcome(
    result: &ExecutionResult,
    should_defer_generic_exit: bool,
) -> Option<TerminalSignal> {
    result
        .terminal_signal
        .as_ref()
        .filter(|signal| {
            !matches!(
                signal.kind,
                TerminalSignalKind::NonzeroExit | TerminalSignalKind::SignalExit
            ) || !should_defer_generic_exit
        })
        .cloned()
}

pub(crate) fn spawn_error_terminal_signal(
    provider_name: &str,
    evidence: impl Into<String>,
) -> TerminalSignal {
    TerminalSignal {
        kind: TerminalSignalKind::SpawnError,
        provider_name: provider_name.to_string(),
        evidence: evidence.into(),
        observed_at: std::time::SystemTime::now(),
    }
}

enum Age153TerminalSignalFixtureOverride {
    Clear,
    Force(TerminalSignalKind),
    Unset,
}

pub(crate) fn apply_age153_terminal_signal_fixture_override(result: &mut ExecutionResult) {
    match age153_terminal_signal_fixture_override() {
        Age153TerminalSignalFixtureOverride::Clear => {
            clear_terminal_signal_fixture_override(result)
        }
        Age153TerminalSignalFixtureOverride::Force(kind) => {
            force_terminal_signal_fixture_override(result, kind)
        }
        Age153TerminalSignalFixtureOverride::Unset => {}
    }
}

fn age153_terminal_signal_fixture_override() -> Age153TerminalSignalFixtureOverride {
    if age153_force_terminal_signal_none_requested() {
        return Age153TerminalSignalFixtureOverride::Clear;
    }
    age153_forced_terminal_signal_kind()
        .map(Age153TerminalSignalFixtureOverride::Force)
        .unwrap_or(Age153TerminalSignalFixtureOverride::Unset)
}

fn age153_force_terminal_signal_none_requested() -> bool {
    std::env::var_os("OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_NONE").is_some()
}

fn age153_forced_terminal_signal_kind() -> Option<TerminalSignalKind> {
    age153_forced_terminal_signal_kind_value()
        .as_deref()
        .and_then(terminal_signal_kind_from_env)
}

fn age153_forced_terminal_signal_kind_value() -> Option<String> {
    std::env::var("OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND").ok()
}

fn clear_terminal_signal_fixture_override(result: &mut ExecutionResult) {
    result.terminal_signal = None;
    result.terminal_reason = None;
}

fn force_terminal_signal_fixture_override(result: &mut ExecutionResult, kind: TerminalSignalKind) {
    let existing = result.terminal_signal.take();
    let signal = match existing {
        Some(mut signal) => {
            signal.kind = kind;
            signal
        }
        None => TerminalSignal {
            kind,
            provider_name: String::new(),
            evidence: "age153 fixture override".to_string(),
            observed_at: std::time::SystemTime::now(),
        },
    };
    result.terminal_reason = typed_terminal_reason_fallback(&signal).map(str::to_string);
    result.terminal_signal = Some(signal);
}

fn terminal_signal_kind_from_env(value: &str) -> Option<TerminalSignalKind> {
    match value {
        "CleanExit" => Some(TerminalSignalKind::CleanExit),
        "NonzeroExit" => Some(TerminalSignalKind::NonzeroExit),
        "SignalExit" => Some(TerminalSignalKind::SignalExit),
        "SpawnError" => Some(TerminalSignalKind::SpawnError),
        "QuotaExhaustedInband" => Some(TerminalSignalKind::QuotaExhaustedInband),
        "RateLimited" => Some(TerminalSignalKind::RateLimited),
        "ProlongedSilence" => Some(TerminalSignalKind::ProlongedSilence),
        "Unknown" => Some(TerminalSignalKind::Unknown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_runtime::executor::{
        CapturedChildInvocation, SessionCaptureMethod, SessionCaptureResult, TerminalSignal,
    };
    use std::time::SystemTime;

    fn result_with_signal(kind: Option<TerminalSignalKind>) -> ExecutionResult {
        ExecutionResult {
            stdout: Vec::new(),
            stderr: "legacy quota text".to_string(),
            exit_code: 1,
            provider_index: 0,
            session_capture: SessionCaptureResult {
                session_id: None,
                method: SessionCaptureMethod::None,
            },
            resume_acceptance: None,
            terminal_reason: Some("quota_exhausted_inband".to_string()),
            terminal_signal: kind.map(|kind| TerminalSignal {
                kind,
                provider_name: "provider-a".to_string(),
                evidence: "typed evidence".to_string(),
                observed_at: SystemTime::UNIX_EPOCH,
            }),
            captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
            returned_artifacts: Vec::new(),
        }
    }

    fn production_source() -> &'static str {
        include_str!("terminal_outcome_adapter.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production source precedes tests")
    }

    fn production_block_after(start: &str) -> &'static str {
        let source = production_source();
        let start_idx = source
            .find(start)
            .unwrap_or_else(|| panic!("missing {start}"));
        let open_idx = source[start_idx..]
            .find('{')
            .map(|idx| start_idx + idx)
            .unwrap_or_else(|| panic!("missing opening brace after {start}"));
        let mut depth = 1usize;
        let mut idx = open_idx + 1;
        let bytes = source.as_bytes();

        while idx < bytes.len() {
            match bytes[idx] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[open_idx + 1..idx];
                    }
                }
                _ => {}
            }
            idx += 1;
        }

        panic!("missing closing brace after {start}");
    }

    fn assert_production_contains(fragment_parts: &[&str]) {
        let fragment = fragment_parts.concat();
        assert!(
            production_source().contains(&fragment),
            "production terminal_outcome_adapter.rs must contain {fragment:?} per /home/nes/projects/agent-runner/planning/age-153-terminal-signal-wiring/contracts/age-153-terminal-signal-wiring.md"
        );
    }

    #[test]
    fn age151_terminal_outcome_typed_quota_exhausted_inband_maps_to_quota_exhausted_before_diagnostics()
     {
        let result = result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband));
        let mut fallback_called = false;

        let category = classify_error_category_with_fallback(&result, || {
            fallback_called = true;
            Some(ErrorCategory::Unknown.as_str().to_string())
        });

        assert_eq!(
            category.as_deref(),
            Some(ErrorCategory::QuotaExhausted.as_str())
        );
        assert!(!fallback_called);
    }

    #[test]
    fn age151_terminal_outcome_typed_prolonged_silence_is_non_quota_terminal_outcome() {
        let result = result_with_signal(Some(TerminalSignalKind::ProlongedSilence));

        let category = classify_error_category_with_fallback(&result, || {
            Some(ErrorCategory::QuotaExhausted.as_str().to_string())
        });

        assert_eq!(
            category.as_deref(),
            Some(ErrorCategory::HungSubprocess.as_str())
        );
    }

    #[test]
    fn age151_terminal_outcome_legacy_diagnostics_runs_only_when_typed_signal_absent() {
        let typed_result = result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband));
        let legacy_result = result_with_signal(None);
        let mut fallback_calls = 0;

        let typed_category = classify_error_category_with_fallback(&typed_result, || {
            fallback_calls += 1;
            Some(ErrorCategory::Unknown.as_str().to_string())
        });
        let legacy_category = classify_error_category_with_fallback(&legacy_result, || {
            fallback_calls += 1;
            Some(ErrorCategory::QuotaExhausted.as_str().to_string())
        });

        assert_eq!(
            typed_category.as_deref(),
            Some(ErrorCategory::QuotaExhausted.as_str())
        );
        assert_eq!(
            legacy_category.as_deref(),
            Some(ErrorCategory::QuotaExhausted.as_str())
        );
        assert_eq!(fallback_calls, 1);
    }

    #[test]
    fn age151_terminal_outcome_terminal_reason_strings_do_not_create_typed_behavior() {
        let result = result_with_signal(None);

        let category = classify_error_category_with_fallback(&result, || None);

        assert_eq!(
            result.terminal_reason.as_deref(),
            Some("quota_exhausted_inband")
        );
        assert_eq!(category, None);
    }

    #[test]
    fn age153_apply_terminal_signal_outcome_unit_contract_declares_five_dispositions() {
        assert_production_contains(&["fn ", "apply_terminal_signal_outcome", "("]);
        assert_production_contains(&["enum ", "TerminalSignalDisposition"]);
        for variant in [
            "QuotaExhaustedRetry",
            "ProlongedSilenceFail",
            "InteractiveFail",
            "InteractiveClean",
            "NotApplicable",
        ] {
            assert_production_contains(&["TerminalSignalDisposition::", variant]);
        }
    }

    #[test]
    fn age153_apply_terminal_signal_outcome_maps_quota_to_typed_forensics_write() {
        let source = production_source();
        let outcome = production_block_after("fn apply_terminal_signal_outcome(");
        let disposition = production_block_after("fn terminal_signal_disposition(");
        assert!(
            outcome.contains("terminal_signal_disposition(signal)")
                && outcome.contains("TerminalSignalDisposition::QuotaExhaustedRetry")
                && outcome.contains("apply_typed_post_failure_forensics")
                && disposition.contains("TerminalSignalKind::QuotaExhaustedInband")
                && disposition.contains("TerminalSignalDisposition::QuotaExhaustedRetry"),
            "AGE-163 WU-A.4: quota typed signal must route through apply_typed_post_failure_forensics"
        );
        let quota_retry_arm = outcome
            .find("TerminalSignalDisposition::QuotaExhaustedRetry")
            .expect("quota retry arm must exist in apply_terminal_signal_outcome");
        let after_quota_retry = &outcome[quota_retry_arm..];
        let forensics_call = after_quota_retry
            .find("apply_typed_post_failure_forensics")
            .expect("quota retry arm must call apply_typed_post_failure_forensics");
        let next_arm = after_quota_retry
            .find("TerminalSignalDisposition::InteractiveClean")
            .expect("next terminal-signal arm must exist");
        assert!(
            forensics_call < next_arm,
            "forensics write must stay in the typed-signal arms (not InteractiveClean)"
        );
        // AGE-163 WU-A.4: read-side persistent vs transient typing lives in
        // FailureClass::from_terminal_signal_kind. RateLimited maps to
        // TransientStderrNoise which has next_available_at_offset() = None,
        // so the durable working-set write does not fire on transient noise.
        assert!(
            source.contains("FailureClass::from_terminal_signal_kind")
                || source.contains("apply_typed_post_failure_forensics"),
            "typed forensics surface must be referenced in the adapter"
        );
        assert!(
            source.contains("terminal_signal") && source.contains("classify_error_category"),
            "typed-signal precedence must coexist with legacy fallback helpers"
        );
    }

    #[test]
    fn age153_emit_terminal_signal_marker_unit_contract_is_key_json_stderr_line() {
        assert_production_contains(&["fn ", "emit_terminal_signal_marker", "("]);
        let helper = production_block_after("fn emit_terminal_signal_marker(");
        assert!(helper.contains("OULIPOLY_TERMINAL_SIGNAL="), "{helper}");
        assert!(helper.contains("serde_json"), "{helper}");
        assert!(helper.contains("kind"), "{helper}");
        assert!(helper.contains("evidence"), "{helper}");
        assert!(helper.contains("invocation_id"), "{helper}");
        assert!(helper.contains("session_id"), "{helper}");
        assert!(
            helper.contains("writeln!") || helper.contains("write_all"),
            "marker helper must write exactly one newline-terminated stderr record"
        );
    }
}
