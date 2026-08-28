//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (wake-sweep backlog regressions).

use crate::SESSION;
use crate::fake_cli::provider_script;
use crate::fixtures::Fixture;
use crate::liveness::{
    backlog_recovered_and_debris_retained, delivered_rows_without_pending_or_claim, wait_for_file,
    wait_until,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_age270_invocation, assert_dead_owner_prompts_missing, assert_prompt_contains_handle,
    assert_success, assert_xdg_isolated,
};
use crate::wake_claim_setup::seed_dead_wake_claim;

pub(crate) fn wake_sweep_retries_twice_unconfirmed_oldest_with_newer_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "oldest-after-unconfirmed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-unconfirmed-old");
    fixture.seed_mailbox(SESSION, "h-newer");
    fixture.mark_mailbox_unconfirmed_twice(SESSION, "h-unconfirmed-old");
    seed_dead_wake_claim(&fixture, "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("oldest-after-unconfirmed.txt"));
    assert_prompt_contains_handle(&prompt, "h-unconfirmed-old");
    assert_prompt_contains_handle(&prompt, "h-newer");
    wait_until("oldest and newer mailbox delivered in FIFO order", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 2)
    });
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    let old = rows
        .iter()
        .find(|row| row.handle == "h-unconfirmed-old")
        .unwrap();
    assert!(old.delivered_at.is_some());
    assert_eq!(old.delivery_attempts, 3);
    assert!(old.delivery_error.is_none());
    let newer = rows.iter().find(|row| row.handle == "h-newer").unwrap();
    assert!(newer.delivered_at.is_some());
    assert_eq!(newer.delivery_attempts, 1);
    assert!(newer.delivery_error.is_none());
    assert_eq!(
        old.delivered_by_invocation_uuid, newer.delivered_by_invocation_uuid,
        "the oldest row must remain in the selected FIFO batch"
    );
    assert_age270_invocation(
        &fixture,
        old.delivered_by_invocation_uuid.as_deref().unwrap(),
    );
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_backlog_recovers_recent_leak_and_retains_dead_owner_debris() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        r#"printf '%s' "$last" > "$work/backlog-$resume.txt""#,
        "backlog-any.txt",
    ));

    let dead_sessions = fixture.seed_dead_owner_backlog();

    let idle_session = "11111111-1111-4111-8111-000000000001";
    fixture.seed_resumable_backlog_session(
        "22222222-2222-4222-8222-000000000001",
        idle_session,
        "turn-idle-backlog",
        "h-idle-resumable-backlog",
        "eeee0000-0000-4000-8000-000000000001",
        Some(3_600),
    );

    let recent_session = "11111111-1111-4111-8111-000000000002";
    fixture.seed_resumable_backlog_session(
        "22222222-2222-4222-8222-000000000002",
        recent_session,
        "turn-recent-backlog",
        "h-recent-leak-backlog",
        "eeee0000-0000-4000-8000-000000000002",
        None,
    );

    let output = fixture.run_mailbox_list(recent_session);
    assert_success(&output);

    let idle_prompt = wait_for_file(&fixture.prompt_file(&format!("backlog-{idle_session}.txt")));
    assert_prompt_contains_handle(&idle_prompt, "h-idle-resumable-backlog");
    let recent_prompt =
        wait_for_file(&fixture.prompt_file(&format!("backlog-{recent_session}.txt")));
    assert_prompt_contains_handle(&recent_prompt, "h-recent-leak-backlog");

    wait_until(
        "backlog recoverable sessions delivered and debris retained",
        || {
            backlog_recovered_and_debris_retained(
                &fixture,
                idle_session,
                recent_session,
                &dead_sessions,
            )
        },
    );
    assert_dead_owner_prompts_missing(&fixture, &dead_sessions);
    for session_id in [idle_session, recent_session] {
        let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].delivered_at.is_some());
        assert_eq!(rows[0].delivery_attempts, 1);
        assert!(rows[0].delivery_error.is_none());
        assert_age270_invocation(
            &fixture,
            rows[0].delivered_by_invocation_uuid.as_deref().unwrap(),
        );
        assert!(
            fixture
                .mailbox()
                .wake_session_reader()
                .wake_claim(session_id)
                .unwrap()
                .is_none()
        );
    }
    for session_id in &dead_sessions {
        let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].delivered_at.is_none());
        assert_eq!(rows[0].delivery_attempts, 0);
        assert!(rows[0].delivery_error.is_none());
        assert!(
            fixture
                .mailbox()
                .wake_session_reader()
                .wake_claim(session_id)
                .unwrap()
                .is_some()
        );
    }
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_eventually_reaches_startable_session_between_paused_edges() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "middle-session.txt"));

    seed_paused_sessions(&fixture, "old", 0..128);

    let middle_session = "11111111-1111-4111-8111-000000000129";
    fixture.seed_resumable_backlog_session(
        "22222222-2222-4222-8222-000000000129",
        middle_session,
        "turn-middle-session",
        "h-middle-session",
        "eeee0000-0000-4000-8000-000000000129",
        None,
    );

    seed_paused_sessions(&fixture, "new", 130..258);

    for _ in 0..3 {
        let output = fixture.run_mailbox_list(middle_session);
        assert_success(&output);
    }

    let prompt = wait_for_file(&fixture.prompt_file("middle-session.txt"));
    assert_prompt_contains_handle(&prompt, "h-middle-session");
    wait_until("middle backlog session delivered", || {
        delivered_rows_without_pending_or_claim(&fixture, middle_session, 1)
    });
    assert_xdg_isolated(&fixture);
}

fn seed_paused_sessions(fixture: &Fixture, cohort: &str, indexes: std::ops::Range<usize>) {
    for index in indexes {
        let session_id = format!("33333333-3333-4333-8333-{index:012}");
        fixture.seed_mailbox_for(&session_id, &format!("h-{cohort}-paused-{index}"), None);
        fixture
            .mailbox()
            .set_notifications_paused(&session_id, true)
            .unwrap();
    }
}
