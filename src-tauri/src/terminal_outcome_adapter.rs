//! Typed terminal-outcome classification for CLI result handling.
//!
//! ## Declared roles
//!
//! `mapper`, `formatter`, `parser`, `predicate`, `orchestration`, `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/terminal_outcome_adapter.rs
//!     role: adapter
//!     Translates:
//!       - ExecutionResult.terminal_signal
//!       - oulipoly_runtime::executor::ExecutionResult terminal_signal contract
//!       - oulipoly_runtime::executor::terminal_signal::TerminalSignalKind contract
//!       - oulipoly_runtime::diagnostics error-category contract
//!       - src-tauri CLI quota retry/error-category contract
//!       - AGE-153 forced terminal-signal fixture contract
//! intrinsic_surface_declarations:
//!   - component: src-tauri/src/terminal_outcome_adapter.rs
//!     role: intrinsic-surface
//!     Domain: Tauri terminal-signal disposition
//!     Owns:
//!       - full TerminalSignalKind disposition vocabulary
//!       - terminal marker vocabulary
//!       - terminal error-category projection
//!       - external-cancelled projection hook
//! ```

use oulipoly_runtime::balancer::{FailureClass, apply_post_failure_forensics};
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{ExecutionResult, TerminalSignal};
use oulipoly_state::StateDb;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

static AGE153_FORCE_TERMINAL_SIGNAL_SEQUENCE_INDEX: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOutcomeCategory {
    QuotaExhausted,
    HungSubprocess,
}

impl TerminalOutcomeCategory {
    pub fn as_error_category(self) -> Option<String> {
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

pub fn classify_error_category_with_fallback<F>(
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
        | TerminalSignalKind::MaybeQuotaExhausted
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::Unknown => None,
    }
}

pub enum TerminalSignalDisposition {
    QuotaExhaustedRetry,
    MaybeQuotaVerify,
    ProlongedSilenceFail,
    InteractiveFail,
    InteractiveClean,
    NotApplicable,
}

pub struct TerminalSignalContext<'a, W: io::Write> {
    pub invocation_id: &'a Uuid,
    pub session_id: Option<&'a Uuid>,
    pub provider: &'a str,
    pub state_db: &'a StateDb,
    pub stderr: &'a mut W,
}

pub fn emit_terminal_signal_marker(
    signal: &TerminalSignal,
    invocation_id: &Uuid,
    session_id: Option<&Uuid>,
    stderr: &mut impl io::Write,
) -> io::Result<()> {
    let payload = terminal_signal_marker_payload(signal, invocation_id, session_id);
    write_terminal_signal_marker(stderr, &payload)
}

fn terminal_signal_marker_payload(
    signal: &TerminalSignal,
    invocation_id: &Uuid,
    session_id: Option<&Uuid>,
) -> serde_json::Value {
    serde_json::json!({
        "kind": format!("{:?}", signal.kind),
        "evidence": {
            "excerpt": signal.evidence.as_str(),
        },
        "invocation_id": invocation_id.to_string(),
        "session_id": session_id.map(Uuid::to_string),
    })
}

fn write_terminal_signal_marker(
    stderr: &mut impl io::Write,
    payload: &serde_json::Value,
) -> io::Result<()> {
    writeln!(
        stderr,
        "OULIPOLY_TERMINAL_SIGNAL={}",
        serde_json::to_string(&payload).map_err(io::Error::other)?
    )
}

pub fn apply_terminal_signal_outcome(
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
            // AGE-163 forensics write `next_available_at` for the routing
            // working-set predicate; the legacy `exhausted_at` column is the
            // immediate durable mark that AGE-166 tests + the orchestration
            // loop's confirm path also rely on.
            apply_typed_post_failure_forensics(signal, ctx);
            mark_provider_exhausted(ctx.state_db, ctx.provider);
        }
        TerminalSignalDisposition::MaybeQuotaVerify => {
            // AGE-166: maybe-quota emits the marker but does NOT mark exhausted.
            // Confirmation (second consecutive zero-turn) routes through
            // `confirm_maybe_quota_exhausted` which marks the provider exhausted.
            emit_terminal_signal_marker_or_warn(signal, ctx);
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

/// AGE-166: confirm a previously-tentative `MaybeQuotaExhausted` signal.
///
/// Called by the orchestration loop on the second consecutive zero-turn for
/// the same `(provider, session_id)` key. Emits the marker, marks the provider
/// exhausted, and returns the canonical `quota_exhausted` error-category string.
pub fn confirm_maybe_quota_exhausted<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) -> &'static str {
    emit_terminal_signal_marker_or_warn(signal, ctx);
    mark_provider_exhausted(ctx.state_db, ctx.provider);
    ErrorCategory::QuotaExhausted.as_str()
}

fn warn_mark_provider_exhausted_failed(e: impl std::fmt::Display) {
    eprintln!("Warning: Failed to mark provider exhausted: {e}");
}

fn mark_provider_exhausted(state: &StateDb, provider_name: &str) {
    state
        .mark_exhausted(provider_name)
        .unwrap_or_else(warn_mark_provider_exhausted_failed);
}

fn terminal_signal_disposition(signal: &TerminalSignal) -> TerminalSignalDisposition {
    match signal.kind {
        TerminalSignalKind::CleanExit => TerminalSignalDisposition::InteractiveClean,
        TerminalSignalKind::QuotaExhaustedInband => TerminalSignalDisposition::QuotaExhaustedRetry,
        TerminalSignalKind::MaybeQuotaExhausted => TerminalSignalDisposition::MaybeQuotaVerify,
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

pub fn typed_terminal_reason_fallback(signal: &TerminalSignal) -> Option<&'static str> {
    oulipoly_runtime::diagnostics::canonical_terminal_reason_for_kind(signal.kind)
}

pub fn terminal_signal_reason<'a>(
    signal: &Option<TerminalSignal>,
    executor_terminal_reason: Option<&'a str>,
) -> Option<&'a str> {
    signal.as_ref()?;
    executor_terminal_reason.or_else(|| signal.as_ref().and_then(typed_terminal_reason_fallback))
}

pub fn terminal_signal_error_category<'a>(
    signal: &Option<TerminalSignal>,
    terminal_reason: &'a str,
) -> Option<&'a str> {
    match signal.as_ref().map(|signal| signal.kind) {
        Some(
            TerminalSignalKind::NonzeroExit
            | TerminalSignalKind::SignalExit
            | TerminalSignalKind::MaybeQuotaExhausted,
        ) => None,
        _ => Some(terminal_reason),
    }
}

pub fn resume_terminal_signal_for_outcome(
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

pub fn balanced_terminal_signal_for_outcome(
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

pub fn spawn_error_terminal_signal(
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

fn age153_forced_terminal_signal_token(value: &str) -> &str {
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
fn force_terminal_signal_fixture_override(result: &mut ExecutionResult, kind: TerminalSignalKind) {
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

fn terminal_signal_kind_from_env(value: &str) -> Option<TerminalSignalKind> {
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
            submitted_user_turn: None,
            captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
            returned_artifacts: Vec::new(),
        }
    }

    fn signal(kind: TerminalSignalKind) -> TerminalSignal {
        TerminalSignal {
            kind,
            provider_name: "provider-a".to_string(),
            evidence: "provider_session_id=session-1 baseline_assistant_turns=0 current_assistant_turns=0 new_assistant_turns=0".to_string(),
            observed_at: SystemTime::UNIX_EPOCH,
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
        let open_idx = production_block_open_index(source, start);
        let close_idx = production_block_close_index(source, open_idx, start);
        &source[open_idx + 1..close_idx]
    }

    fn production_block_open_index(source: &str, start: &str) -> usize {
        let start_idx = production_block_start_index(source, start);
        source[start_idx..]
            .find('{')
            .map(|idx| start_idx + idx)
            .unwrap_or_else(|| panic!("missing opening brace after {start}"))
    }

    fn production_block_start_index(source: &str, start: &str) -> usize {
        source
            .find(start)
            .unwrap_or_else(|| panic!("missing {start}"))
    }

    fn production_block_close_index(source: &str, open_idx: usize, start: &str) -> usize {
        let mut depth = 1usize;
        let mut idx = open_idx + 1;
        let bytes = source.as_bytes();

        while idx < bytes.len() {
            match bytes[idx] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return idx;
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
        let payload_helper = production_block_after("fn terminal_signal_marker_payload(");
        let write_helper = production_block_after("fn write_terminal_signal_marker(");
        assert!(
            write_helper.contains("OULIPOLY_TERMINAL_SIGNAL="),
            "{write_helper}"
        );
        assert!(write_helper.contains("serde_json"), "{write_helper}");
        assert!(payload_helper.contains("kind"), "{payload_helper}");
        assert!(payload_helper.contains("evidence"), "{payload_helper}");
        assert!(payload_helper.contains("invocation_id"), "{payload_helper}");
        assert!(payload_helper.contains("session_id"), "{payload_helper}");
        assert!(
            write_helper.contains("writeln!") || write_helper.contains("write_all"),
            "marker helper must write exactly one newline-terminated stderr record"
        );
    }

    #[test]
    fn terminal_outcome_adapter_maybe_quota_verify_disposition_non_durable() {
        let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        db.upsert_quota_refresh("provider-a", &[]).unwrap();
        let invocation_id = Uuid::nil();
        let maybe = signal(TerminalSignalKind::MaybeQuotaExhausted);
        let mut stderr = Vec::new();
        let mut ctx = TerminalSignalContext {
            invocation_id: &invocation_id,
            session_id: None,
            provider: "provider-a",
            state_db: &db,
            stderr: &mut stderr,
        };

        let disposition = apply_terminal_signal_outcome(&Some(maybe), &mut ctx);
        let category = classify_error_category_with_fallback(
            &result_with_signal(Some(TerminalSignalKind::MaybeQuotaExhausted)),
            || None,
        );

        assert!(matches!(
            disposition,
            TerminalSignalDisposition::MaybeQuotaVerify
        ));
        assert_eq!(category, None);
        assert_eq!(
            db.get_quota("provider-a").unwrap().unwrap().exhausted_at,
            None
        );
        let marker = String::from_utf8(stderr).unwrap();
        assert!(marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
        assert!(
            marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
            "{marker}"
        );
    }

    struct CancelledTerminalFixture {
        db: StateDb,
        invocation_id: Uuid,
        signal: TerminalSignal,
        stderr: Vec<u8>,
    }

    fn cancelled_terminal_fixture() -> CancelledTerminalFixture {
        let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        db.upsert_quota_refresh("provider-a", &[]).unwrap();
        CancelledTerminalFixture {
            db,
            invocation_id: Uuid::nil(),
            signal: cancelled_terminal_signal(),
            stderr: Vec::new(),
        }
    }

    fn cancelled_terminal_signal() -> TerminalSignal {
        let mut cancelled = signal(TerminalSignalKind::Unknown);
        cancelled.evidence = "terminal-classify-cancelled".to_string();
        cancelled
    }

    fn apply_cancelled_terminal_outcome(
        fixture: &mut CancelledTerminalFixture,
    ) -> TerminalSignalDisposition {
        let mut ctx = TerminalSignalContext {
            invocation_id: &fixture.invocation_id,
            session_id: None,
            provider: "provider-a",
            state_db: &fixture.db,
            stderr: &mut fixture.stderr,
        };
        apply_terminal_signal_outcome(&Some(fixture.signal.clone()), &mut ctx)
    }

    fn cancelled_terminal_reason() -> &'static str {
        terminal_signal_reason(&Some(cancelled_terminal_signal()), Some("cancelled"))
            .expect("reason")
    }

    fn cancelled_terminal_category(terminal_reason: &str) -> Option<&str> {
        terminal_signal_error_category(&Some(signal(TerminalSignalKind::Unknown)), terminal_reason)
    }

    fn assert_cancelled_terminal_outcome(
        fixture: &CancelledTerminalFixture,
        disposition: TerminalSignalDisposition,
        terminal_reason: &str,
        category: Option<&str>,
    ) {
        assert!(matches!(
            disposition,
            TerminalSignalDisposition::InteractiveFail
        ));
        assert_eq!(terminal_reason, "cancelled");
        assert_eq!(category, Some("cancelled"));
        assert_eq!(
            fixture
                .db
                .get_quota("provider-a")
                .unwrap()
                .unwrap()
                .exhausted_at,
            None
        );
        let marker = String::from_utf8(fixture.stderr.clone()).unwrap();
        assert!(marker.contains("OULIPOLY_TERMINAL_SIGNAL="), "{marker}");
        assert!(marker.contains("terminal-classify-cancelled"), "{marker}");
    }

    #[test]
    fn terminal_outcome_adapter_cancelled_reason_is_nondurable_interactive_failure() {
        let mut fixture = cancelled_terminal_fixture();
        let disposition = apply_cancelled_terminal_outcome(&mut fixture);
        let terminal_reason = cancelled_terminal_reason();
        let category = cancelled_terminal_category(terminal_reason);

        assert_cancelled_terminal_outcome(&fixture, disposition, terminal_reason, category);
    }

    #[test]
    fn terminal_outcome_adapter_confirmed_exhaustion_writes_durable() {
        let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        db.upsert_quota_refresh("provider-a", &[]).unwrap();
        let invocation_id = Uuid::nil();
        let maybe = signal(TerminalSignalKind::MaybeQuotaExhausted);
        let mut stderr = Vec::new();
        let mut ctx = TerminalSignalContext {
            invocation_id: &invocation_id,
            session_id: None,
            provider: "provider-a",
            state_db: &db,
            stderr: &mut stderr,
        };

        let category = confirm_maybe_quota_exhausted(&maybe, &mut ctx);

        assert_eq!(category, ErrorCategory::QuotaExhausted.as_str());
        assert!(
            db.get_quota("provider-a")
                .unwrap()
                .unwrap()
                .exhausted_at
                .is_some()
        );
        let marker = String::from_utf8(stderr).unwrap();
        assert!(
            marker.contains("\"kind\":\"MaybeQuotaExhausted\""),
            "{marker}"
        );
    }

    #[test]
    fn typed_terminal_reason_fallback_returns_maybe_quota_exhausted_for_new_kind() {
        let maybe = signal(TerminalSignalKind::MaybeQuotaExhausted);
        let quota = signal(TerminalSignalKind::QuotaExhaustedInband);

        assert_eq!(
            typed_terminal_reason_fallback(&maybe),
            Some("maybe_quota_exhausted")
        );
        assert_eq!(
            typed_terminal_reason_fallback(&quota),
            Some("quota_exhausted_inband")
        );
    }

    #[test]
    fn age153_fixture_parser_accepts_maybe_quota_exhausted_kind() {
        let parsed = terminal_signal_kind_from_env("MaybeQuotaExhausted");
        assert_eq!(parsed, Some(TerminalSignalKind::MaybeQuotaExhausted));

        let mut result = result_with_signal(None);
        force_terminal_signal_fixture_override(
            &mut result,
            TerminalSignalKind::MaybeQuotaExhausted,
        );

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
        AGE153_FORCE_TERMINAL_SIGNAL_SEQUENCE_INDEX.store(0, Ordering::Relaxed);

        let first = age153_forced_terminal_signal_token(
            "MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited",
        );
        let second = age153_forced_terminal_signal_token(
            "MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited",
        );
        let third = age153_forced_terminal_signal_token(
            "MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited",
        );
        let fourth = age153_forced_terminal_signal_token(
            "MaybeQuotaExhausted,QuotaExhaustedInband,RateLimited",
        );

        assert_eq!(first, "MaybeQuotaExhausted");
        assert_eq!(second, "QuotaExhaustedInband");
        assert_eq!(third, "RateLimited");
        assert_eq!(fourth, "MaybeQuotaExhausted");
    }

    #[test]
    fn quota_exhausted_inband_semantics_regression() {
        let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        db.upsert_quota_refresh("provider-a", &[]).unwrap();
        let invocation_id = Uuid::nil();
        let quota = signal(TerminalSignalKind::QuotaExhaustedInband);
        let mut stderr = Vec::new();
        let mut ctx = TerminalSignalContext {
            invocation_id: &invocation_id,
            session_id: None,
            provider: "provider-a",
            state_db: &db,
            stderr: &mut stderr,
        };

        let disposition = apply_terminal_signal_outcome(&Some(quota.clone()), &mut ctx);
        let category = classify_error_category_with_fallback(
            &result_with_signal(Some(TerminalSignalKind::QuotaExhaustedInband)),
            || Some(ErrorCategory::Unknown.as_str().to_string()),
        );
        let terminal_reason = terminal_signal_reason(&Some(quota.clone()), None);
        let terminal_error_category =
            terminal_signal_error_category(&Some(quota), terminal_reason.unwrap());

        assert!(matches!(
            disposition,
            TerminalSignalDisposition::QuotaExhaustedRetry
        ));
        assert_eq!(
            category.as_deref(),
            Some(ErrorCategory::QuotaExhausted.as_str())
        );
        assert!(
            db.get_quota("provider-a")
                .unwrap()
                .unwrap()
                .exhausted_at
                .is_some()
        );
        assert_eq!(terminal_reason, Some("quota_exhausted_inband"));
        assert_eq!(terminal_error_category, Some("quota_exhausted_inband"));
        let marker = String::from_utf8(stderr).unwrap();
        assert!(
            marker.contains("\"kind\":\"QuotaExhaustedInband\""),
            "{marker}"
        );
    }
}
