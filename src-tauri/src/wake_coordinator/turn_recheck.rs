//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`

use super::auto_wake_env::{
    AutoWakeEnv, auto_wake_cap_reached, auto_wake_max_for_session, current_auto_wake,
    current_auto_wake_count, emit_auto_wake_cap_reached, release_current_auto_wake_claim,
    sleep_before_failed_auto_wake_retry,
};
use super::diagnostics::{WakeDiagnostic, auto_wake_cap_diagnostic, storage_error_diagnostic};
use super::idle::mark_session_idle_after_turn;
use super::wake_start::{StartWakeInput, start_wake_chain};
use oulipoly_state::mailbox::MailboxDb;

pub(crate) fn mark_successful_turn_idle_and_recheck(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: i32,
) -> Result<WakeDiagnostic, String> {
    mark_session_idle_after_turn(session_id, invocation_uuid, Some(exit_code))?;
    Ok(successful_turn_recheck(session_id))
}

pub(crate) fn recheck_after_failed_auto_wake(session_id: &str) -> WakeDiagnostic {
    let Some(auto_wake) = current_auto_wake() else {
        return WakeDiagnostic::status("not_auto_wake");
    };
    release_current_auto_wake_claim(session_id, Some(&auto_wake));
    failed_auto_wake_recheck(session_id, &auto_wake)
}

fn successful_turn_recheck(session_id: &str) -> WakeDiagnostic {
    trigger_turn_end_recheck(session_id)
}

fn trigger_turn_end_recheck(session_id: &str) -> WakeDiagnostic {
    let pending_count = match turn_end_pending_count(session_id) {
        Ok(count) => count,
        Err(err) => return storage_error_diagnostic(err),
    };
    let auto_wake = current_auto_wake();
    let auto_wake_max = match auto_wake_max_for_session(session_id) {
        Ok(value) => value,
        Err(err) => return storage_error_diagnostic(err),
    };
    let current_count = current_auto_wake_count(auto_wake.as_ref());
    apply_turn_end_recheck_decision(
        session_id,
        auto_wake.as_ref(),
        turn_end_recheck_decision(
            session_id,
            RecheckConditions {
                no_pending: no_pending(pending_count),
                cap_reached: auto_wake_cap_reached(current_count, auto_wake_max),
            },
            current_count,
            auto_wake_max,
            auto_wake.as_ref(),
        ),
    )
}

fn failed_auto_wake_recheck(session_id: &str, auto_wake: &AutoWakeEnv) -> WakeDiagnostic {
    let pending_count = match turn_end_pending_count(session_id) {
        Ok(count) => count,
        Err(err) => return storage_error_diagnostic(err),
    };
    let auto_wake_max = match auto_wake_max_for_session(session_id) {
        Ok(value) => value,
        Err(err) => return storage_error_diagnostic(err),
    };
    apply_failed_auto_wake_recheck_decision(
        session_id,
        auto_wake,
        failed_auto_wake_recheck_decision(
            session_id,
            RecheckConditions {
                no_pending: no_pending(pending_count),
                cap_reached: auto_wake_cap_reached(auto_wake.count, auto_wake_max),
            },
            auto_wake,
            auto_wake_max,
        ),
    )
}

enum TurnEndRecheckDecision<'a> {
    NoPending,
    CapReached { current_count: i64, max_count: i64 },
    Start(StartWakeInput<'a>),
}

struct RecheckConditions {
    no_pending: bool,
    cap_reached: bool,
}

fn turn_end_recheck_decision<'a>(
    session_id: &'a str,
    conditions: RecheckConditions,
    current_count: i64,
    auto_wake_max: i64,
    auto_wake: Option<&'a AutoWakeEnv>,
) -> TurnEndRecheckDecision<'a> {
    if conditions.no_pending {
        return TurnEndRecheckDecision::NoPending;
    }
    let max_count = auto_wake_max;
    if conditions.cap_reached {
        return TurnEndRecheckDecision::CapReached {
            current_count,
            max_count,
        };
    }
    TurnEndRecheckDecision::Start(turn_end_start_wake_input(
        session_id,
        current_count,
        auto_wake,
    ))
}

fn turn_end_start_wake_input<'a>(
    session_id: &'a str,
    current_count: i64,
    auto_wake: Option<&'a AutoWakeEnv>,
) -> StartWakeInput<'a> {
    StartWakeInput {
        session_id,
        reason: "turn_end_recheck",
        auto_wake_count: current_count + 1,
        renew_token: auto_wake.map(|wake| wake.token.as_str()),
    }
}

fn apply_turn_end_recheck_decision(
    session_id: &str,
    auto_wake: Option<&AutoWakeEnv>,
    decision: TurnEndRecheckDecision<'_>,
) -> WakeDiagnostic {
    match decision {
        TurnEndRecheckDecision::NoPending => {
            release_current_auto_wake_claim(session_id, auto_wake);
            no_pending_diagnostic()
        }
        TurnEndRecheckDecision::CapReached {
            current_count,
            max_count,
        } => {
            release_current_auto_wake_claim(session_id, auto_wake);
            cap_reached_diagnostic(session_id, current_count, max_count)
        }
        TurnEndRecheckDecision::Start(input) => start_wake_chain(input),
    }
}

enum FailedAutoWakeRecheckDecision<'a> {
    NoPending,
    CapReached { current_count: i64, max_count: i64 },
    Retry(StartWakeInput<'a>),
}

fn failed_auto_wake_recheck_decision<'a>(
    session_id: &'a str,
    conditions: RecheckConditions,
    auto_wake: &'a AutoWakeEnv,
    auto_wake_max: i64,
) -> FailedAutoWakeRecheckDecision<'a> {
    if conditions.no_pending {
        return FailedAutoWakeRecheckDecision::NoPending;
    }
    let max_count = auto_wake_max;
    if conditions.cap_reached {
        return FailedAutoWakeRecheckDecision::CapReached {
            current_count: auto_wake.count,
            max_count,
        };
    }
    FailedAutoWakeRecheckDecision::Retry(failed_auto_wake_retry_input(session_id, auto_wake))
}

fn failed_auto_wake_retry_input<'a>(
    session_id: &'a str,
    auto_wake: &'a AutoWakeEnv,
) -> StartWakeInput<'a> {
    StartWakeInput {
        session_id,
        reason: "wake_failure_retry",
        auto_wake_count: auto_wake.count + 1,
        renew_token: None,
    }
}

fn apply_failed_auto_wake_recheck_decision(
    session_id: &str,
    auto_wake: &AutoWakeEnv,
    decision: FailedAutoWakeRecheckDecision<'_>,
) -> WakeDiagnostic {
    match decision {
        FailedAutoWakeRecheckDecision::NoPending => no_pending_diagnostic(),
        FailedAutoWakeRecheckDecision::CapReached {
            current_count,
            max_count,
        } => cap_reached_diagnostic(session_id, current_count, max_count),
        FailedAutoWakeRecheckDecision::Retry(input) => {
            sleep_before_failed_auto_wake_retry(auto_wake.count);
            start_wake_chain(input)
        }
    }
}

fn turn_end_pending_count(session_id: &str) -> Result<usize, String> {
    let mut db = MailboxDb::open_default()?;
    turn_end_pending_count_on(&mut db, session_id)
}

fn turn_end_pending_count_on(db: &mut MailboxDb, session_id: &str) -> Result<usize, String> {
    super::consumed_completion::reconcile_late_consumed_completions_on(db, session_id)?;
    crate::mailbox_delivery::deliverable_pending_count_on(db, session_id)
}

fn no_pending_diagnostic() -> WakeDiagnostic {
    WakeDiagnostic::status("no_pending")
}

fn cap_reached_diagnostic(session_id: &str, current_count: i64, max_count: i64) -> WakeDiagnostic {
    emit_auto_wake_cap_reached(session_id, current_count, max_count);
    auto_wake_cap_diagnostic(current_count)
}

fn no_pending(pending_count: usize) -> bool {
    pending_count == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake_coordinator::consumed_completion::ConsumedCompletionFixture;

    #[test]
    fn turn_end_pending_count_reconciles_late_consumption() {
        let fixture = ConsumedCompletionFixture::new();
        fixture.mark_consumed();
        let mut db = fixture.mailbox();

        assert_eq!(
            turn_end_pending_count_on(&mut db, ConsumedCompletionFixture::SESSION_ID).unwrap(),
            0
        );
        assert!(
            db.list_pending(ConsumedCompletionFixture::SESSION_ID)
                .unwrap()
                .is_empty()
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
