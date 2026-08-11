//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

mod claim;
mod liveness;

use oulipoly_state::mailbox::{
    GenerationStorageError, MailboxDb, SessionGenerationProjection, SessionLiveness,
    SessionRuntimeRow, WakeClaimAcquireResult, WakeClaimRow,
};
use uuid::Uuid;

use super::auto_wake_env::{
    auto_wake_cap_reached, auto_wake_max_for_runtime, emit_auto_wake_cap_reached,
};
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
    runtime: Option<SessionRuntimeRow>,
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
    if generation_authority_blocks_wake(&db, input.session_id)? {
        return Err(busy_diagnostic());
    }
    let runtime =
        session_runtime_for_wake(&db, input.session_id).map_err(storage_error_diagnostic)?;
    let input = normalize_start_wake_input(input, runtime.as_ref());
    validate_wake_has_deliverable_pending(&db, input.session_id)?;
    let auto_wake_max = auto_wake_max_for_runtime(runtime.as_ref());
    validate_start_wake_cap(input, auto_wake_max)?;
    let liveness = wake_runtime_liveness(&mut db, input.session_id, runtime.as_ref())?;
    cleanup_idle_runtime(runtime.as_ref(), liveness);
    if wake_liveness_busy(liveness) {
        return Err(busy_diagnostic());
    }
    super::consumed_completion::reconcile_late_consumed_completions_on(&mut db, input.session_id)
        .map_err(storage_error_diagnostic)?;
    let claim = acquire_startable_wake_claim(&mut db, input, claim_token)?;
    Ok(wake_start_context(input, db, runtime, claim, auto_wake_max))
}

fn validate_wake_has_deliverable_pending(
    db: &MailboxDb,
    session_id: &str,
) -> Result<(), WakeDiagnostic> {
    match crate::mailbox_delivery::deliverable_pending_count_on(db, session_id) {
        Ok(0) => Err(WakeDiagnostic::status("no_pending")),
        Ok(_) => Ok(()),
        Err(err) => Err(storage_error_diagnostic(err)),
    }
}

fn generation_authority_blocks_wake(
    db: &MailboxDb,
    session_id: &str,
) -> Result<bool, WakeDiagnostic> {
    let projection =
        session_generation_projection(db, session_id).map_err(generation_storage_diagnostic)?;
    Ok(generation_projection_blocks_wake(projection))
}

fn session_generation_projection(
    db: &MailboxDb,
    session_id: &str,
) -> Result<SessionGenerationProjection, GenerationStorageError> {
    db.session_generation_projection(session_id)
}

fn generation_storage_diagnostic(err: GenerationStorageError) -> WakeDiagnostic {
    storage_error_diagnostic(err.to_string())
}

fn generation_projection_blocks_wake(projection: SessionGenerationProjection) -> bool {
    !matches!(projection, SessionGenerationProjection::None)
}

fn wake_start_context<'a>(
    input: StartWakeInput<'a>,
    db: MailboxDb,
    runtime: Option<SessionRuntimeRow>,
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
    runtime: Option<&SessionRuntimeRow>,
) -> StartWakeInput<'a> {
    let persisted_next = runtime
        .map(|runtime| runtime.auto_wake_count.saturating_add(1))
        .unwrap_or(input.auto_wake_count);
    StartWakeInput {
        auto_wake_count: input.auto_wake_count.max(persisted_next).max(1),
        ..input
    }
}

fn start_wake_input_reached_cap(input: StartWakeInput<'_>, auto_wake_max: i64) -> bool {
    auto_wake_cap_reached(input.auto_wake_count.saturating_sub(1), auto_wake_max)
}

fn validate_start_wake_cap(
    input: StartWakeInput<'_>,
    auto_wake_max: i64,
) -> Result<(), WakeDiagnostic> {
    if !start_wake_input_reached_cap(input, auto_wake_max) {
        return Ok(());
    }
    let current_count = input.auto_wake_count.saturating_sub(1);
    Err(start_wake_cap_diagnostic(
        input.session_id,
        current_count,
        auto_wake_max,
    ))
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
    runtime: Option<&SessionRuntimeRow>,
) -> Result<Option<SessionLiveness>, WakeDiagnostic> {
    liveness::pty_runtime_liveness(db, session_id, runtime).map_err(storage_error_diagnostic)
}

fn cleanup_idle_runtime(runtime: Option<&SessionRuntimeRow>, liveness: Option<SessionLiveness>) {
    liveness::cleanup_idle_runtime(runtime, liveness);
}

fn wake_liveness_busy(liveness: Option<SessionLiveness>) -> bool {
    liveness::optional_pty_liveness_is_busy(liveness)
}

fn busy_diagnostic() -> WakeDiagnostic {
    WakeDiagnostic::status("busy")
}

fn acquire_startable_wake_claim(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim_token: &str,
) -> Result<WakeClaimRow, WakeDiagnostic> {
    let claim_result =
        claim::acquire_wake_claim(db, input, claim_token).map_err(storage_error_diagnostic)?;
    wake_claim_to_start(claim_result)
}

fn wake_claim_to_start(result: WakeClaimAcquireResult) -> Result<WakeClaimRow, WakeDiagnostic> {
    match result {
        WakeClaimAcquireResult::Acquired(claim) => Ok(claim),
        WakeClaimAcquireResult::NoPending => Err(WakeDiagnostic::status("no_pending")),
        WakeClaimAcquireResult::Busy => Err(WakeDiagnostic::status("busy")),
        WakeClaimAcquireResult::AlreadyInFlight(claim) => Err(already_in_flight_diagnostic(claim)),
    }
}

fn wake_spawn_diagnostic(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    runtime: Option<&SessionRuntimeRow>,
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
    runtime: Option<&SessionRuntimeRow>,
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
    let _ = db.release_wake_claim(input.session_id, Some(&claim.claim_token));
    spawn_error_diagnostic(claim.claim_token, input.auto_wake_count, err)
}

fn open_wake_mailbox() -> Result<MailboxDb, String> {
    MailboxDb::open_default()
}

fn session_runtime_for_wake(
    db: &MailboxDb,
    session_id: &str,
) -> Result<Option<SessionRuntimeRow>, String> {
    db.session_runtime(session_id)
}

fn record_wake_pid_or_warn(
    db: &mut MailboxDb,
    session_id: &str,
    claim_token: &str,
    wake_pid: i64,
    runtime: Option<&SessionRuntimeRow>,
) {
    let (provider_name, model_name) = wake_pid_identity_names(runtime);
    if let Err(err) = db.record_wake_claim_pid_identity(
        session_id,
        claim_token,
        wake_pid,
        provider_name,
        model_name,
    ) {
        warn_wake_pid_record_failed(session_id, claim_token, err);
    }
}

fn wake_pid_identity_names(runtime: Option<&SessionRuntimeRow>) -> (Option<&str>, Option<&str>) {
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
            db.wake_claim(ConsumedCompletionFixture::SESSION_ID)
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
