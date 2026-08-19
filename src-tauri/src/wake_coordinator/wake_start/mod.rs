//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

mod claim;
mod liveness;

use oulipoly_state::mailbox::{
    MailboxDb, SessionLiveness, SessionMetadataRow, WakeClaimAcquireResult, WakeClaimRow,
};
use uuid::Uuid;

use super::auto_wake_env::{auto_wake_max_for_runtime, emit_auto_wake_cap_reached};
use super::diagnostics::{
    WakeDiagnostic, already_in_flight_diagnostic, auto_wake_cap_diagnostic, spawn_error_diagnostic,
    spawned_wake_diagnostic, storage_error_diagnostic,
};
use super::spawn::spawn_detached_resume;

#[derive(Clone, Copy)]
pub(super) struct StartWakeInput<'a> {
    pub(super) session_id: &'a str,
    pub(super) reason: &'a str,
    pub(super) auto_wake_count: i64,
    pub(super) renew_token: Option<&'a str>,
}

struct WakeStartContext<'a> {
    input: StartWakeInput<'a>,
    db: MailboxDb,
    runtime: Option<SessionMetadataRow>,
    claim: WakeClaimRow,
    auto_wake_max: i64,
}

pub(crate) fn trigger_notify_wake(session_id: &str) -> WakeDiagnostic {
    start_wake_chain(StartWakeInput {
        session_id,
        reason: "notify_idle",
        auto_wake_count: 1,
        renew_token: None,
    })
}

pub(super) fn start_wake_chain(input: StartWakeInput<'_>) -> WakeDiagnostic {
    let claim_token = Uuid::new_v4().to_string();
    let mut context = match prepare_wake_start_context(input, &claim_token) {
        Ok(context) => context,
        Err(diagnostic) => return diagnostic,
    };
    let spawn = spawn_detached_resume(
        context.input.session_id,
        context.runtime.as_ref(),
        &context.claim.claim_token,
        context.input.auto_wake_count,
        context.auto_wake_max,
    );
    wake_spawn_diagnostic(
        &mut context.db,
        context.input,
        context.runtime.as_ref(),
        context.claim,
        spawn,
    )
}

fn prepare_wake_start_context<'a>(
    input: StartWakeInput<'a>,
    claim_token: &str,
) -> Result<WakeStartContext<'a>, WakeDiagnostic> {
    let db = open_wake_mailbox().map_err(storage_error_diagnostic)?;
    prepare_wake_start_context_with_db(input, claim_token, db)
}

fn prepare_wake_start_context_with_db<'a>(
    input: StartWakeInput<'a>,
    claim_token: &str,
    mut db: MailboxDb,
) -> Result<WakeStartContext<'a>, WakeDiagnostic> {
    let runtime =
        session_metadata_for_wake(&db, input.session_id).map_err(storage_error_diagnostic)?;
    let input = normalize_start_wake_input(input, runtime.as_ref());
    super::consumed_completion::reconcile_late_consumed_completions_on(&mut db, input.session_id)
        .map_err(storage_error_diagnostic)?;
    let auto_wake_max = auto_wake_max_for_runtime(runtime.as_ref());
    let liveness = wake_runtime_liveness(&mut db, input.session_id)?;
    cleanup_idle_runtime(&liveness);
    if wake_liveness_busy(&liveness) {
        return Err(busy_diagnostic());
    }
    let claim = acquire_startable_wake_claim(&mut db, input, claim_token, auto_wake_max)?;
    Ok(wake_start_context(input, db, runtime, claim, auto_wake_max))
}

fn wake_start_context<'a>(
    input: StartWakeInput<'a>,
    db: MailboxDb,
    runtime: Option<SessionMetadataRow>,
    claim: WakeClaimRow,
    auto_wake_max: i64,
) -> WakeStartContext<'a> {
    WakeStartContext {
        input,
        db,
        runtime,
        claim,
        auto_wake_max,
    }
}

fn normalize_start_wake_input<'a>(
    input: StartWakeInput<'a>,
    runtime: Option<&SessionMetadataRow>,
) -> StartWakeInput<'a> {
    let persisted_next = runtime
        .map(|runtime| runtime.auto_wake_count.saturating_add(1))
        .unwrap_or(input.auto_wake_count);
    StartWakeInput {
        auto_wake_count: input.auto_wake_count.max(persisted_next).max(1),
        ..input
    }
}

fn start_wake_cap_diagnostic(
    session_id: &str,
    current_count: i64,
    auto_wake_max: i64,
) -> WakeDiagnostic {
    emit_auto_wake_cap_reached(session_id, current_count, auto_wake_max);
    auto_wake_cap_diagnostic(current_count)
}

fn wake_runtime_liveness(
    db: &mut MailboxDb,
    session_id: &str,
) -> Result<liveness::RuntimeLivenessCheck, WakeDiagnostic> {
    liveness::runtime_liveness(db, session_id).map_err(storage_error_diagnostic)
}

fn cleanup_idle_runtime(check: &liveness::RuntimeLivenessCheck) {
    liveness::cleanup_idle_runtime(check);
}

fn wake_liveness_busy(check: &liveness::RuntimeLivenessCheck) -> bool {
    check.liveness == SessionLiveness::Busy
}

fn busy_diagnostic() -> WakeDiagnostic {
    WakeDiagnostic::status("busy")
}

fn acquire_startable_wake_claim(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim_token: &str,
    auto_wake_max: i64,
) -> Result<WakeClaimRow, WakeDiagnostic> {
    let claim_result = claim::acquire_wake_claim(db, input, claim_token, auto_wake_max)
        .map_err(storage_error_diagnostic)?;
    wake_claim_to_start(input.session_id, claim_result)
}

fn wake_claim_to_start(
    session_id: &str,
    result: WakeClaimAcquireResult,
) -> Result<WakeClaimRow, WakeDiagnostic> {
    match result {
        WakeClaimAcquireResult::Acquired(claim) => Ok(claim),
        WakeClaimAcquireResult::NoPending => Err(WakeDiagnostic::status("no_pending")),
        WakeClaimAcquireResult::Busy => Err(WakeDiagnostic::status("busy")),
        WakeClaimAcquireResult::CapReached {
            current_count,
            max_count,
        } => Err(start_wake_cap_diagnostic(
            session_id,
            current_count,
            max_count,
        )),
        WakeClaimAcquireResult::AlreadyInFlight(claim) => Err(already_in_flight_diagnostic(claim)),
    }
}

fn wake_spawn_diagnostic(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    runtime: Option<&SessionMetadataRow>,
    claim: WakeClaimRow,
    spawn: Result<i64, String>,
) -> WakeDiagnostic {
    match spawn {
        Ok(wake_pid) => successful_wake_spawn_diagnostic(db, input, runtime, claim, wake_pid),
        Err(err) => failed_wake_spawn_diagnostic(db, input, claim, err),
    }
}

fn successful_wake_spawn_diagnostic(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    runtime: Option<&SessionMetadataRow>,
    claim: WakeClaimRow,
    wake_pid: i64,
) -> WakeDiagnostic {
    record_wake_pid_or_warn(db, input.session_id, &claim.claim_token, wake_pid, runtime);
    spawned_wake_diagnostic(claim.claim_token, wake_pid, input.auto_wake_count)
}

fn failed_wake_spawn_diagnostic(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim: WakeClaimRow,
    err: String,
) -> WakeDiagnostic {
    let _ = db
        .wake_sessions()
        .release_wake_claim(input.session_id, &claim.claim_token);
    spawn_error_diagnostic(claim.claim_token, input.auto_wake_count, err)
}

fn open_wake_mailbox() -> Result<MailboxDb, String> {
    MailboxDb::open_default()
}

fn session_metadata_for_wake(
    db: &MailboxDb,
    session_id: &str,
) -> Result<Option<SessionMetadataRow>, String> {
    db.wake_session_reader().session_metadata(session_id)
}

fn record_wake_pid_or_warn(
    db: &mut MailboxDb,
    session_id: &str,
    claim_token: &str,
    wake_pid: i64,
    runtime: Option<&SessionMetadataRow>,
) {
    let (provider_name, model_name) = wake_pid_identity_names(runtime);
    if let Err(err) = db.wake_sessions().record_wake_claim_pid_identity(
        session_id,
        claim_token,
        wake_pid,
        provider_name,
        model_name,
    ) {
        warn_wake_pid_record_failed(session_id, claim_token, err);
    }
}

fn wake_pid_identity_names(runtime: Option<&SessionMetadataRow>) -> (Option<&str>, Option<&str>) {
    (
        runtime.and_then(|row| row.provider_name.as_deref()),
        runtime.and_then(|row| row.model_name.as_deref()),
    )
}

fn warn_wake_pid_record_failed(session_id: &str, claim_token: &str, err: String) {
    tracing::warn!(session_id, claim_token, "Failed to record wake PID: {err}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake_coordinator::consumed_completion::ConsumedCompletionFixture;

    #[test]
    fn wake_start_reconciles_late_consumption_before_claim() {
        let fixture = ConsumedCompletionFixture::new();
        fixture.mark_consumed();

        let diagnostic = match prepare_wake_start_context_with_db(
            StartWakeInput {
                session_id: ConsumedCompletionFixture::SESSION_ID,
                reason: "test",
                auto_wake_count: 1,
                renew_token: None,
            },
            "claim-token",
            fixture.mailbox(),
        ) {
            Err(diagnostic) => diagnostic,
            Ok(_) => panic!("consumed completion must stop wake preparation"),
        };

        assert_eq!(diagnostic.status, "no_pending");
        assert!(!diagnostic.attempted);
        let db = fixture.mailbox();
        assert!(
            db.list_pending(ConsumedCompletionFixture::SESSION_ID)
                .unwrap()
                .is_empty()
        );
        assert!(
            db.wake_session_reader()
                .wake_claim(ConsumedCompletionFixture::SESSION_ID)
                .unwrap()
                .is_none()
        );
        let listener = db
            .completion_event_listeners(ConsumedCompletionFixture::EVENT_ID)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            listener.acknowledgement_reason.as_deref(),
            Some("consumed_in_call")
        );
    }
}
