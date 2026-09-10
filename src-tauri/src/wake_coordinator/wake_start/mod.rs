//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator`

mod liveness;

use oulipoly_state::mailbox::{
    MailboxDb, SessionLiveness, SessionMetadataRow, WakeClaimAcquireResult, WakeClaimRow,
};
use uuid::Uuid;

use super::diagnostics::{
    WakeDiagnostic, already_in_flight_diagnostic, spawn_error_diagnostic, spawned_wake_diagnostic,
    storage_error_diagnostic,
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
    );
    wake_spawn_diagnostic(&mut context.db, context.input, context.claim, spawn)
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
    if let Some(stop) = db
        .mailbox_observation_stop(input.session_id)
        .map_err(storage_error_diagnostic)?
    {
        let mut diagnostic = WakeDiagnostic::status("observation_stopped");
        diagnostic.message = Some(format!("stop_id={}: {}", stop.stop_id, stop.error));
        return Err(diagnostic);
    }
    let runtime =
        session_metadata_for_wake(&db, input.session_id).map_err(storage_error_diagnostic)?;
    let input = normalize_start_wake_input(input, runtime.as_ref());
    super::consumed_completion::reconcile_late_consumed_completions_on(&mut db, input.session_id)
        .map_err(storage_error_diagnostic)?;
    let liveness = wake_runtime_liveness(&mut db, input.session_id)?;
    cleanup_idle_runtime(&liveness);
    if wake_liveness_busy(&liveness) {
        return Err(busy_diagnostic());
    }
    let claim = acquire_startable_wake_claim(&mut db, input, claim_token)?;
    Ok(wake_start_context(input, db, runtime, claim))
}

fn wake_start_context<'a>(
    input: StartWakeInput<'a>,
    db: MailboxDb,
    runtime: Option<SessionMetadataRow>,
    claim: WakeClaimRow,
) -> WakeStartContext<'a> {
    WakeStartContext {
        input,
        db,
        runtime,
        claim,
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
) -> Result<WakeClaimRow, WakeDiagnostic> {
    let claim_result = super::wake_claim::acquire_wake_claim(db, input, claim_token)
        .map_err(storage_error_diagnostic)?;
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
    claim: WakeClaimRow,
    spawn: Result<i64, String>,
) -> WakeDiagnostic {
    match spawn {
        Ok(wake_pid) => successful_wake_spawn_diagnostic(db, input, claim, wake_pid),
        Err(err) => failed_wake_spawn_diagnostic(db, input, claim, err),
    }
}

fn successful_wake_spawn_diagnostic(
    db: &mut MailboxDb,
    input: StartWakeInput<'_>,
    claim: WakeClaimRow,
    wake_pid: i64,
) -> WakeDiagnostic {
    record_wake_pid_or_warn(db, input.session_id, &claim.claim_token, wake_pid);
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

fn record_wake_pid_or_warn(db: &mut MailboxDb, session_id: &str, claim_token: &str, wake_pid: i64) {
    if let Err(err) =
        db.wake_sessions()
            .record_wake_claim_pid_identity(session_id, claim_token, wake_pid)
    {
        warn_wake_pid_record_failed(session_id, claim_token, err);
    }
}

fn warn_wake_pid_record_failed(session_id: &str, claim_token: &str, err: String) {
    tracing::warn!(session_id, claim_token, "Failed to record wake PID: {err}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake_coordinator::consumed_completion::ConsumedCompletionFixture;
    use oulipoly_state::mailbox::{SessionMetadataUpsert, WakeClaimRequest};

    #[test]
    fn unpause_concurrent_sleeping_requests_admit_only_one_owner_without_replay() {
        let fixture = ConsumedCompletionFixture::new();
        let session = ConsumedCompletionFixture::SESSION_ID;
        fixture
            .mailbox()
            .set_notifications_paused(session, true)
            .unwrap();
        let barrier = std::sync::Barrier::new(2);
        let statuses = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|index| {
                    let fixture = &fixture;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let mut db = fixture.mailbox();
                        db.set_notifications_paused(session, false).unwrap();
                        barrier.wait();
                        match prepare_wake_start_context_with_db(
                            StartWakeInput {
                                session_id: session,
                                reason: "notify_idle",
                                auto_wake_count: 1,
                                renew_token: None,
                            },
                            &format!("unpause-owner-{index}"),
                            db,
                        ) {
                            Ok(_) => "acquired".to_string(),
                            Err(diagnostic) => diagnostic.status,
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(
            statuses
                .iter()
                .filter(|status| *status == "acquired")
                .count(),
            1,
            "{statuses:?}"
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| *status == "already_in_flight")
                .count(),
            1,
            "{statuses:?}"
        );
        let mut db = fixture.mailbox();
        let claim = db
            .wake_session_reader()
            .wake_claim(session)
            .unwrap()
            .unwrap();
        let pending = db.list_pending(session).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delivery_attempts, 0);
        db.acknowledge_range(session, pending[0].seq, pending[0].seq, "test-consumer")
            .unwrap();
        db.wake_sessions()
            .release_wake_claim(session, &claim.claim_token)
            .unwrap();
        let diagnostic = match prepare_wake_start_context_with_db(
            StartWakeInput {
                session_id: session,
                reason: "notify_idle",
                auto_wake_count: 1,
                renew_token: None,
            },
            "after-settlement",
            db,
        ) {
            Ok(_) => panic!("settled work must not launch"),
            Err(diagnostic) => diagnostic,
        };
        assert_eq!(diagnostic.status, "no_pending");
        assert!(
            fixture
                .mailbox()
                .wake_session_reader()
                .wake_claim(session)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn stopped_observation_rejects_notify_retry_and_restart_sweep_without_claim_takeover() {
        let fixture = ConsumedCompletionFixture::new();
        let session = ConsumedCompletionFixture::SESSION_ID;
        let mut db = fixture.mailbox();
        let rows = db.list_pending(session).unwrap();
        let seq = rows[0].seq;
        db.register_headless_delivery_attempt(
            "stopped-attempt",
            session,
            None,
            "native",
            &[seq],
            0,
        )
        .unwrap();
        let claim = db
            .wake_sessions()
            .try_acquire_startable_wake_claim(
                WakeClaimRequest {
                    session_id: session,
                    claim_token: "existing-owner",
                    reason: "notify_idle",
                    auto_wake_count: 1,
                    wake_invocation_uuid: None,
                    stale_after_seconds: 600,
                },
                None,
            )
            .unwrap();
        assert!(matches!(claim, WakeClaimAcquireResult::Acquired(_)));
        db.stop_mailbox_observation(
            session,
            "stopped-attempt",
            "session_turn_staging_capacity_exceeded",
            "capacity exhausted",
        )
        .unwrap();
        let stop = db.mailbox_observation_stop(session).unwrap().unwrap();
        db.set_notifications_paused(session, true).unwrap();
        db.set_notifications_paused(session, false).unwrap();
        assert_eq!(
            db.mailbox_observation_stop(session)
                .unwrap()
                .unwrap()
                .stop_id,
            stop.stop_id
        );
        drop(db);
        for reason in [
            "notify_idle",
            "wake_failure_retry",
            "process_start",
            "maintenance_tick",
        ] {
            let diagnostic = match prepare_wake_start_context_with_db(
                StartWakeInput {
                    session_id: session,
                    reason,
                    auto_wake_count: 2,
                    renew_token: Some("existing-owner"),
                },
                "replacement-owner",
                fixture.mailbox(),
            ) {
                Err(diagnostic) => diagnostic,
                Ok(_) => panic!("fixed failure must not reach spawn"),
            };
            assert_eq!(diagnostic.status, "observation_stopped");
            assert!(!diagnostic.attempted);
            assert!(diagnostic.message.unwrap().contains(&stop.stop_id));
            let mut db = fixture.mailbox();
            let denied = db
                .wake_sessions()
                .try_acquire_startable_wake_claim(
                    WakeClaimRequest {
                        session_id: session,
                        claim_token: "replacement-owner",
                        reason,
                        auto_wake_count: 2,
                        wake_invocation_uuid: None,
                        stale_after_seconds: 0,
                    },
                    Some("existing-owner"),
                )
                .unwrap();
            assert!(matches!(denied, WakeClaimAcquireResult::NoPending));
            assert_eq!(
                db.wake_session_reader()
                    .wake_claim(session)
                    .unwrap()
                    .unwrap()
                    .claim_token,
                "existing-owner"
            );
            assert_eq!(db.list_pending(session).unwrap().len(), 1);
            assert_eq!(
                db.completion_event_listeners(ConsumedCompletionFixture::EVENT_ID)
                    .unwrap()[0]
                    .acknowledgement_reason,
                None
            );
        }
        let mut db = fixture.mailbox();
        db.rearm_mailbox_observation(session, &stop.stop_id, "capacity restored and validated")
            .unwrap();
        // Rearm neither steals the live claim nor acknowledges the listener.
        assert_eq!(
            db.wake_session_reader()
                .wake_claim(session)
                .unwrap()
                .unwrap()
                .claim_token,
            "existing-owner"
        );
        assert!(
            !db.wake_sessions()
                .release_wake_claim(session, "wrong-owner")
                .unwrap()
        );
        assert!(
            db.wake_sessions()
                .release_wake_claim(session, "existing-owner")
                .unwrap()
        );
        let context = prepare_wake_start_context_with_db(
            StartWakeInput {
                session_id: session,
                reason: "notify_idle",
                auto_wake_count: 2,
                renew_token: None,
            },
            "rearmed-owner",
            db,
        )
        .unwrap_or_else(|d| panic!("rearm rejected: {}", d.status));
        assert_eq!(context.claim.claim_token, "rearmed-owner");
    }

    #[test]
    fn maximum_persisted_count_acquires_exact_wake_claim() {
        let fixture = ConsumedCompletionFixture::new();
        let mut db = fixture.mailbox();
        db.wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: ConsumedCompletionFixture::SESSION_ID,
                mode: "headless",
                invocation_uuid: Some(ConsumedCompletionFixture::INVOCATION_UUID),
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture-model"),
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        let seeded = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id: ConsumedCompletionFixture::SESSION_ID,
                claim_token: "seed-count-token",
                reason: "fixture",
                auto_wake_count: i64::MAX,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(seeded, WakeClaimAcquireResult::Acquired(_)));
        db.wake_sessions()
            .release_wake_claim(ConsumedCompletionFixture::SESSION_ID, "seed-count-token")
            .unwrap();

        let context = prepare_wake_start_context_with_db(
            StartWakeInput {
                session_id: ConsumedCompletionFixture::SESSION_ID,
                reason: "notify_idle",
                auto_wake_count: 1,
                renew_token: None,
            },
            "exact-new-claim-token",
            db,
        )
        .unwrap_or_else(|diagnostic| {
            panic!(
                "pending work at maximum chronology must acquire a claim, got {}",
                diagnostic.status
            )
        });

        assert_eq!(context.input.auto_wake_count, i64::MAX);
        assert_eq!(context.claim.claim_token, "exact-new-claim-token");
        assert_eq!(
            context
                .db
                .wake_session_reader()
                .wake_claim(ConsumedCompletionFixture::SESSION_ID)
                .unwrap()
                .unwrap()
                .claim_token,
            "exact-new-claim-token"
        );
    }

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
