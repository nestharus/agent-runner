//! Coordinates the distinct terminal-attempt and failed-automatic-wake rechecks.
//!
//! ## Declared roles
//!
//! `mapper`, `orchestration`

use super::auto_wake_env::{AutoWakeEnv, current_auto_wake};
use super::diagnostics::{WakeDiagnostic, storage_error_diagnostic};
use super::idle::mark_session_idle_after_turn;
use super::retry_cadence::sleep_before_failed_auto_wake_retry;
use super::wake_claim::release_current_auto_wake_claim;
use super::wake_start::{StartWakeInput, start_wake_chain};
use oulipoly_state::{StateDb, mailbox::MailboxDb};

pub(crate) fn mark_terminal_attempt_idle_and_recheck(
    session_id: &str,
    invocation_uuid: &str,
    exit_code: i32,
) -> Result<WakeDiagnostic, String> {
    mark_session_idle_after_turn(session_id, invocation_uuid, Some(exit_code))?;
    Ok(terminal_attempt_recheck(session_id))
}

fn terminal_attempt_recheck(session_id: &str) -> WakeDiagnostic {
    let pending_count = match turn_end_pending_count(session_id) {
        Ok(count) => count,
        Err(err) => return storage_error_diagnostic(err),
    };
    let auto_wake = current_auto_wake();
    if pending_count == 0 {
        release_current_auto_wake_claim(session_id, auto_wake.as_ref());
        return WakeDiagnostic::status("no_pending");
    }
    start_wake_chain(StartWakeInput {
        session_id,
        reason: "turn_end_recheck",
        auto_wake_count: following_auto_wake_chronology_count(auto_wake.as_ref()),
        renew_token: auto_wake.as_ref().map(|wake| wake.token.as_str()),
    })
}

pub(crate) fn recheck_after_failed_auto_wake(session_id: &str) -> WakeDiagnostic {
    let Some(auto_wake) = current_auto_wake() else {
        return WakeDiagnostic::status("not_auto_wake");
    };
    let pending_count = match turn_end_pending_count(session_id) {
        Ok(count) => count,
        Err(err) => {
            release_current_auto_wake_claim(session_id, Some(&auto_wake));
            return storage_error_diagnostic(err);
        }
    };
    if pending_count == 0 {
        release_current_auto_wake_claim(session_id, Some(&auto_wake));
        return WakeDiagnostic::status("no_pending");
    }
    sleep_before_failed_auto_wake_retry(&auto_wake);
    let diagnostic = start_wake_chain(StartWakeInput {
        session_id,
        reason: "wake_failure_retry",
        auto_wake_count: following_auto_wake_chronology_count(Some(&auto_wake)),
        renew_token: Some(&auto_wake.token),
    });
    release_current_auto_wake_claim(session_id, Some(&auto_wake));
    diagnostic
}

fn following_auto_wake_chronology_count(auto_wake: Option<&AutoWakeEnv>) -> i64 {
    auto_wake
        .map(|wake| wake.chronological_attempt_count)
        .unwrap_or(0)
        + 1
}

fn turn_end_pending_count(session_id: &str) -> Result<usize, String> {
    let Some(mut db) = MailboxDb::open_default_if_exists()? else {
        return Ok(0);
    };
    let state = StateDb::open_default()?;
    turn_end_pending_count_on(&mut db, &state, session_id)
}

fn turn_end_pending_count_on(
    db: &mut MailboxDb,
    state: &StateDb,
    session_id: &str,
) -> Result<usize, String> {
    super::consumed_completion::reconcile_late_consumed_completions_on(db, session_id)?;
    crate::mailbox_delivery::deliverable_pending_count_on(db, state, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake_coordinator::consumed_completion::ConsumedCompletionFixture;

    #[test]
    fn terminal_attempt_recheck_ignores_former_cap_at_five() {
        let auto_wake = AutoWakeEnv {
            token: "turn-end-token".to_string(),
            chronological_attempt_count: 5,
            retry_base_milliseconds: 1_000,
        };

        assert_eq!(following_auto_wake_chronology_count(Some(&auto_wake)), 6);
    }

    #[test]
    fn failed_auto_wake_recheck_ignores_former_cap_at_five() {
        let auto_wake = AutoWakeEnv {
            token: "failed-wake-token".to_string(),
            chronological_attempt_count: 5,
            retry_base_milliseconds: 1_000,
        };

        assert_eq!(following_auto_wake_chronology_count(Some(&auto_wake)), 6);
    }

    #[test]
    fn turn_end_pending_count_reconciles_late_consumption() {
        let fixture = ConsumedCompletionFixture::new();
        fixture.mark_consumed();
        let mut db = fixture.mailbox();
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();

        assert_eq!(
            turn_end_pending_count_on(&mut db, &state, ConsumedCompletionFixture::SESSION_ID)
                .unwrap(),
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
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();

        assert_eq!(
            turn_end_pending_count_on(&mut db, &state, ConsumedCompletionFixture::SESSION_ID)
                .unwrap(),
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
