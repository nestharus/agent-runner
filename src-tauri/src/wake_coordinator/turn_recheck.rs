//! ## Declared roles
//!
//! `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`

use super::auto_wake_env::{
    AutoWakeEnv, current_auto_wake, current_auto_wake_count, release_current_auto_wake_claim,
    sleep_before_failed_auto_wake_retry,
};
use super::diagnostics::{WakeDiagnostic, storage_error_diagnostic};
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
    let current_count = current_auto_wake_count(auto_wake.as_ref());
    apply_turn_end_recheck_decision(
        session_id,
        auto_wake.as_ref(),
        turn_end_recheck_decision(
            session_id,
            no_pending(pending_count),
            current_count,
            auto_wake.as_ref(),
        ),
    )
}

fn failed_auto_wake_recheck(session_id: &str, auto_wake: &AutoWakeEnv) -> WakeDiagnostic {
    let pending_count = match turn_end_pending_count(session_id) {
        Ok(count) => count,
        Err(err) => return storage_error_diagnostic(err),
    };
    apply_failed_auto_wake_recheck_decision(
        auto_wake,
        failed_auto_wake_recheck_decision(session_id, no_pending(pending_count), auto_wake),
    )
}

enum TurnEndRecheckDecision<'a> {
    NoPending,
    Start(StartWakeInput<'a>),
}

fn turn_end_recheck_decision<'a>(
    session_id: &'a str,
    no_pending: bool,
    current_count: i64,
    auto_wake: Option<&'a AutoWakeEnv>,
) -> TurnEndRecheckDecision<'a> {
    if no_pending {
        return TurnEndRecheckDecision::NoPending;
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
        TurnEndRecheckDecision::Start(input) => start_wake_chain(input),
    }
}

enum FailedAutoWakeRecheckDecision<'a> {
    NoPending,
    Retry(StartWakeInput<'a>),
}

fn failed_auto_wake_recheck_decision<'a>(
    session_id: &'a str,
    no_pending: bool,
    auto_wake: &'a AutoWakeEnv,
) -> FailedAutoWakeRecheckDecision<'a> {
    if no_pending {
        return FailedAutoWakeRecheckDecision::NoPending;
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
    auto_wake: &AutoWakeEnv,
    decision: FailedAutoWakeRecheckDecision<'_>,
) -> WakeDiagnostic {
    match decision {
        FailedAutoWakeRecheckDecision::NoPending => no_pending_diagnostic(),
        FailedAutoWakeRecheckDecision::Retry(input) => {
            sleep_before_failed_auto_wake_retry(auto_wake.count);
            start_wake_chain(input)
        }
    }
}

fn turn_end_pending_count(session_id: &str) -> Result<usize, String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(0);
    };
    turn_end_pending_count_on(&mut db, session_id)
}

fn turn_end_pending_count_on(db: &mut MailboxDb, session_id: &str) -> Result<usize, String> {
    super::consumed_completion::reconcile_late_consumed_completions_on(db, session_id)?;
    crate::mailbox_delivery::deliverable_pending_count_on(db, session_id)
}

fn no_pending_diagnostic() -> WakeDiagnostic {
    WakeDiagnostic::status("no_pending")
}

fn no_pending(pending_count: usize) -> bool {
    pending_count == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake_coordinator::consumed_completion::ConsumedCompletionFixture;

    #[test]
    fn successful_turn_recheck_ignores_former_cap_at_five() {
        let auto_wake = AutoWakeEnv {
            token: "turn-end-token".to_string(),
            count: 5,
        };

        let decision =
            turn_end_recheck_decision("session-a", false, auto_wake.count, Some(&auto_wake));

        let TurnEndRecheckDecision::Start(input) = decision else {
            panic!("pending work above the former cap must start a follow-up wake");
        };
        assert_eq!(input.auto_wake_count, 6);
        assert_eq!(input.renew_token, Some("turn-end-token"));
    }

    #[test]
    fn failed_auto_wake_recheck_ignores_former_cap_at_five() {
        let auto_wake = AutoWakeEnv {
            token: "failed-wake-token".to_string(),
            count: 5,
        };

        let decision = failed_auto_wake_recheck_decision("session-a", false, &auto_wake);

        let FailedAutoWakeRecheckDecision::Retry(input) = decision else {
            panic!("pending work above the former cap must schedule a bounded retry");
        };
        assert_eq!(input.auto_wake_count, 6);
        assert!(input.renew_token.is_none());
    }

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

    #[test]
    fn turn_end_pending_count_keeps_unconsumed_completion_pending() {
        let fixture = ConsumedCompletionFixture::new();
        let mut db = fixture.mailbox();

        assert_eq!(
            turn_end_pending_count_on(&mut db, ConsumedCompletionFixture::SESSION_ID).unwrap(),
            1
        );
        let listener = db
            .completion_event_listeners(ConsumedCompletionFixture::EVENT_ID)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(listener.acknowledgement_reason, None);
    }
}
