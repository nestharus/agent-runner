//! Terminal-signal disposition and side-effect application.
//!
//! ## Declared roles
//!
//! `orchestration`, `mapper`, `formatter`, `validator`

use crate::terminal_outcome_adapter::marker::emit_terminal_signal_marker;
use oulipoly_runtime::balancer::{FailureClass, apply_post_failure_forensics};
use oulipoly_runtime::diagnostics::ErrorCategory;
use oulipoly_runtime::executor::TerminalSignal;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_state::StateDb;
use std::io;
use uuid::Uuid;

#[derive(Clone, Copy)]
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

pub fn apply_terminal_signal_outcome(
    signal: &Option<TerminalSignal>,
    ctx: &mut TerminalSignalContext<'_, impl io::Write>,
) -> TerminalSignalDisposition {
    let Some(signal) = signal else {
        return TerminalSignalDisposition::NotApplicable;
    };

    let disposition = terminal_signal_disposition(signal);
    // AGE-153 marker authority remains emit_terminal_signal_marker via the side-effect helpers.
    match disposition {
        TerminalSignalDisposition::QuotaExhaustedRetry => {
            apply_typed_post_failure_forensics_quota_retry_side_effects(signal, ctx)
        }
        TerminalSignalDisposition::MaybeQuotaVerify => {
            apply_maybe_quota_verify_side_effects(signal, ctx);
        }
        TerminalSignalDisposition::ProlongedSilenceFail
        | TerminalSignalDisposition::InteractiveFail => {
            apply_typed_post_failure_forensics_terminal_failure_side_effects(signal, ctx);
        }
        TerminalSignalDisposition::InteractiveClean => {
            emit_terminal_signal_marker_or_warn(signal, ctx)
        }
        TerminalSignalDisposition::NotApplicable => {}
    }
    disposition
}

fn apply_typed_post_failure_forensics_quota_retry_side_effects<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) {
    // AGE-153 marker authority: emit_terminal_signal_marker.
    emit_terminal_signal_marker_or_warn(signal, ctx);
    // AGE-163 forensics write `next_available_at` for the routing
    // working-set predicate; the legacy `exhausted_at` column is the
    // immediate durable mark that AGE-166 tests + the orchestration
    // loop's confirm path also rely on.
    apply_typed_post_failure_forensics(signal, ctx);
    mark_provider_exhausted(ctx.state_db, ctx.provider);
}

fn apply_maybe_quota_verify_side_effects<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) {
    // AGE-166: maybe-quota emits the marker but does NOT mark exhausted.
    // Confirmation (second consecutive zero-turn) routes through
    // `confirm_maybe_quota_exhausted` which marks the provider exhausted.
    emit_terminal_signal_marker_or_warn(signal, ctx);
}

fn apply_typed_post_failure_forensics_terminal_failure_side_effects<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) {
    emit_terminal_signal_marker_or_warn(signal, ctx);
    apply_typed_post_failure_forensics(signal, ctx);
}

fn apply_typed_post_failure_forensics<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) {
    let Some(failure_class) = terminal_signal_failure_class(signal) else {
        return;
    };
    persist_typed_post_failure_forensics_or_warn(ctx, failure_class);
}

fn terminal_signal_failure_class(signal: &TerminalSignal) -> Option<FailureClass> {
    FailureClass::from_terminal_signal_kind(signal.kind)
}

fn persist_typed_post_failure_forensics_or_warn<W: io::Write>(
    ctx: &mut TerminalSignalContext<'_, W>,
    failure_class: FailureClass,
) {
    if let Err(err) = persist_typed_post_failure_forensics(ctx, failure_class) {
        warn_typed_post_failure_forensics_failed(ctx.stderr, err);
    }
}

fn persist_typed_post_failure_forensics<W: io::Write>(
    ctx: &TerminalSignalContext<'_, W>,
    failure_class: FailureClass,
) -> Result<(), oulipoly_runtime::migration::MigrationError> {
    apply_post_failure_forensics(
        ctx.state_db,
        ctx.provider,
        failure_class,
        chrono::Utc::now(),
    )
}

fn warn_typed_post_failure_forensics_failed(
    stderr: &mut impl io::Write,
    err: oulipoly_runtime::migration::MigrationError,
) {
    let _ = writeln!(
        stderr,
        "Warning: Failed to apply post-failure forensics: {err:?}"
    );
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
    apply_maybe_quota_confirmation_side_effects(signal, ctx);
    confirmed_maybe_quota_error_category()
}

fn apply_maybe_quota_confirmation_side_effects<W: io::Write>(
    signal: &TerminalSignal,
    ctx: &mut TerminalSignalContext<'_, W>,
) {
    emit_terminal_signal_marker_or_warn(signal, ctx);
    mark_provider_exhausted(ctx.state_db, ctx.provider);
}

fn confirmed_maybe_quota_error_category() -> &'static str {
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
        TerminalSignalKind::ProviderStorageContention => {
            TerminalSignalDisposition::QuotaExhaustedRetry
        }
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
