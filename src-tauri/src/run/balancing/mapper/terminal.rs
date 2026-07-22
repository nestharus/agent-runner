use oulipoly_runtime::executor;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_state::StateDb;
use uuid::Uuid;

use crate::terminal_outcome_adapter::{TerminalSignalContext, spawn_error_terminal_signal};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::run::balancing) enum TerminalSignalBranch {
    QuotaExhaustedRetry,
    MaybeQuotaVerify,
    ProlongedSilenceFail,
    InteractiveFail,
    CompletedAttempt,
}

pub(in crate::run::balancing) fn terminal_signal_branch(
    signal: &Option<executor::TerminalSignal>,
    recovered_generic_nonzero: bool,
) -> TerminalSignalBranch {
    let Some(signal) = signal else {
        return TerminalSignalBranch::CompletedAttempt;
    };
    if recovered_generic_nonzero && signal.kind == TerminalSignalKind::NonzeroExit {
        return TerminalSignalBranch::CompletedAttempt;
    }
    match signal.kind {
        TerminalSignalKind::QuotaExhaustedInband => TerminalSignalBranch::QuotaExhaustedRetry,
        TerminalSignalKind::ProviderStorageContention => TerminalSignalBranch::QuotaExhaustedRetry,
        TerminalSignalKind::MaybeQuotaExhausted => TerminalSignalBranch::MaybeQuotaVerify,
        TerminalSignalKind::ProlongedSilence => TerminalSignalBranch::ProlongedSilenceFail,
        TerminalSignalKind::NonzeroExit
        | TerminalSignalKind::SignalExit
        | TerminalSignalKind::SpawnError
        | TerminalSignalKind::RateLimited
        | TerminalSignalKind::Unknown => TerminalSignalBranch::InteractiveFail,
        TerminalSignalKind::CleanExit => TerminalSignalBranch::CompletedAttempt,
    }
}

pub(in crate::run::balancing) fn spawn_error_signal(
    provider_name: &str,
    error: String,
) -> executor::TerminalSignal {
    spawn_error_terminal_signal(provider_name, error)
}

pub(in crate::run::balancing) struct TerminalSignalContextIds {
    pub(in crate::run::balancing) invocation_uuid: Uuid,
    pub(in crate::run::balancing) session_uuid: Option<Uuid>,
}

pub(in crate::run::balancing) fn terminal_signal_context_ids(
    invocation_id: &str,
    provider_session_id: Option<&str>,
) -> TerminalSignalContextIds {
    TerminalSignalContextIds {
        invocation_uuid: super::super::parser::parse_invocation_uuid(invocation_id),
        session_uuid: crate::dispatch::provider_session_marker_uuid(provider_session_id),
    }
}

pub(in crate::run::balancing) struct TerminalSignalContextInput<'a, W: std::io::Write> {
    pub(in crate::run::balancing) ids: &'a TerminalSignalContextIds,
    pub(in crate::run::balancing) provider_name: &'a str,
    pub(in crate::run::balancing) state: &'a StateDb,
    pub(in crate::run::balancing) stderr: &'a mut W,
}

pub(in crate::run::balancing) fn terminal_signal_context<'a, W: std::io::Write>(
    input: TerminalSignalContextInput<'a, W>,
) -> TerminalSignalContext<'a, W> {
    TerminalSignalContext {
        invocation_id: &input.ids.invocation_uuid,
        session_id: input.ids.session_uuid.as_ref(),
        provider: input.provider_name,
        state_db: input.state,
        stderr: input.stderr,
    }
}

pub(in crate::run::balancing) fn terminal_signal_context_for_attempt<'a, W: std::io::Write>(
    ids: &'a TerminalSignalContextIds,
    provider_name: &'a str,
    state: &'a StateDb,
    stderr: &'a mut W,
) -> TerminalSignalContext<'a, W> {
    terminal_signal_context(TerminalSignalContextInput {
        ids,
        provider_name,
        state,
        stderr,
    })
}
