//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (batch delivery and wake-sweep regressions).

use crate::fake_provider::{anchor_admission_provider_script, provider_script};
use crate::fixtures::Fixture;
use crate::liveness::{
    assert_dead_owner_debris_retained, delivered_rows_without_pending_or_claim,
    delivered_single_row_without_error_or_claim, settle_wake_sweep, wait_for_file, wait_until,
    wait_until_with_timeout,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_additional_notifications_remain_queued, assert_age270_invocation,
    assert_live_claim_token, assert_no_wake_claim, assert_pending_handle_without_error,
    assert_pending_mailbox_count, assert_prompt_contains_handle, assert_prompt_file_missing,
    assert_success, assert_xdg_isolated,
};
use crate::wake_claim_setup::{seed_dead_wake_claim, seed_live_wake_claim};
use crate::{MODEL, PROVIDER, SESSION};

fn direct_unconfirmed_invocation(output: &std::process::Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{stdout}");
    let result: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    let mut keys = result
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent_runner_chain_id",
            "agent_runner_invocation_id",
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "provider_name",
            "provider_session_id",
            "status",
            "success",
            "terminal_reason"
        ]
    );
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["error_category"], "resume_completion_unconfirmed");
    assert_eq!(result["terminal_reason"], "resume_completion_unconfirmed");
    assert_eq!(result["provider_name"], PROVIDER);
    assert_eq!(result["provider_session_id"], SESSION);
    assert_eq!(result["agent_runner_invocation_id"], result["id"]);
    result["id"].as_str().unwrap().to_string()
}

fn assert_one_failed_delivery(fixture: &Fixture, session_id: &str) {
    let rows = fixture.mailbox().list_mailbox(session_id, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_some());
    assert_eq!(rows[0].delivery_attempts, 1);
    assert!(rows[0].delivery_error.is_none());
    let invocation_id = rows[0].delivered_by_invocation_uuid.as_deref().unwrap();
    assert_age270_invocation(fixture, invocation_id);
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .legacy_runtime_projection(session_id)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.run_state, "idle");
    assert_eq!(runtime.last_exit_code, Some(0));
    assert_no_wake_claim(fixture, session_id);
}

pub(crate) fn persisted_count_at_five_allows_turn_end_followup_wake() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "batch-${WU_D_PROVIDER_RESUME_INDEX}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, 5);
    for index in 0..25 {
        fixture.seed_mailbox(SESSION, &format!("h-batch-{index:02}"));
    }

    let output = fixture.run_resume();
    let manual_invocation = direct_unconfirmed_invocation(&output);

    let first = wait_for_file(&fixture.prompt_file("batch-1.txt"));
    assert_additional_notifications_remain_queued(&first);
    wait_until("batch rows delivered", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 25)
    });
    let second = wait_for_file(&fixture.prompt_file("batch-2.txt"));
    assert_prompt_contains_handle(&second, "h-batch-20");
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(
        rows.iter()
            .all(|row| row.delivery_attempts == 1 && row.delivery_error.is_none())
    );
    let mut groups = std::collections::BTreeMap::new();
    for row in &rows {
        *groups
            .entry(row.delivered_by_invocation_uuid.clone().unwrap())
            .or_insert(0usize) += 1;
    }
    assert_eq!(groups.len(), 2);
    assert_eq!(groups.get(&manual_invocation), Some(&20));
    let followup = groups
        .iter()
        .find(|(id, _)| *id != &manual_invocation)
        .unwrap();
    assert_eq!(*followup.1, 5);
    assert_age270_invocation(&fixture, &manual_invocation);
    assert_age270_invocation(&fixture, followup.0);
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.auto_wake_count, 6);
    assert!(!fixture.prompt_file("batch-3.txt").exists());
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_reclaims_dead_claim_and_delivers_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "sweep-reclaimed.txt"));
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-sweep-reclaim");
    seed_dead_wake_claim(&fixture, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", 601);

    let output = fixture.run_startup_recovery(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("sweep-reclaimed.txt"));
    assert_prompt_contains_handle(&prompt, "h-sweep-reclaim");
    wait_until("sweep reclaimed dead claim and delivered mailbox", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    assert_one_failed_delivery(&fixture, SESSION);
    fixture.assert_recovery_drained(SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn native_bound_session_automatically_delivers_after_historical_owner_death() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "abandoned-transient-resumed.txt"));
    fixture.establish_recovery_parent("77777777-7777-4777-8777-777777777777");
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-abandoned-transient");

    fixture.seed_recovery_control();
    let output = fixture.run_startup_recovery("77777777-7777-4777-8777-777777777777");
    assert_success(&output);
    fixture.assert_recovery_control();

    // AGE360 user decision supersedes the former abandoned-transient negative
    // for genuinely bound native recipients only. A finished invocation and
    // historical mailbox-owner death are not an explicit session stop.
    let prompt = wait_for_file(&fixture.prompt_file("abandoned-transient-resumed.txt"));
    assert_eq!(prompt.matches("handle: h-abandoned-transient").count(), 1);
    assert!(!prompt.contains("h-positive-control"));
    wait_until("native-bound historical notification delivered", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 1)
    });
    assert_one_failed_delivery(&fixture, SESSION);
    fixture.assert_recovery_drained(SESSION);
    assert_exact_native_delivery(&fixture, SESSION, "h-abandoned-transient");
    assert_exact_native_delivery(
        &fixture,
        "77777777-7777-4777-8777-777777777777",
        "h-positive-control",
    );
    let before = invocation_count(&fixture);
    assert_success(&fixture.run_startup_recovery(SESSION));
    settle_wake_sweep();
    assert_eq!(invocation_count(&fixture), before);
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "2"
    );
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_retains_non_resumable_abandoned_transient_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "non-resumable-transient-resumed.txt",
    ));
    fixture.establish_recovery_parent("77777777-7777-4777-8777-777777777777");
    // Idle headless runtime with a dead-owner pending row, but NO session turn /
    // chain -> no durable resume evidence. The session is never auto-woken
    // (anti-resurrection), but automatic terminal reap is withheld because the
    // sweep cannot fence State and mailbox authority atomically.
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-non-resumable-transient");

    fixture.seed_recovery_control();
    let output = fixture.run_startup_recovery("77777777-7777-4777-8777-777777777777");
    assert_success(&output);
    fixture.assert_recovery_control();

    assert_prompt_file_missing(&fixture, "non-resumable-transient-resumed.txt");
    assert_dead_owner_debris_retained(&fixture, SESSION);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_retains_dead_owner_session_with_chain_but_no_turns() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "chain-no-turns-resumed.txt"));
    fixture.establish_recovery_parent("77777777-7777-4777-8777-777777777777");
    // A registered chain segment with ZERO produced turns is an empty resume
    // target, not durable work. With a dead owner it is never auto-woken, but
    // remains pending for an explicitly fenced operator disposition.
    fixture.seed_active_chain_for(
        "33333333-3333-4333-8333-333333333333",
        PROVIDER,
        SESSION,
        MODEL,
    );
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-chain-no-turns");

    fixture.seed_recovery_control();
    let output = fixture.run_startup_recovery("77777777-7777-4777-8777-777777777777");
    assert_success(&output);
    fixture.assert_recovery_control();

    assert_prompt_file_missing(&fixture, "chain-no-turns-resumed.txt");
    assert_dead_owner_debris_retained(&fixture, SESSION);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_delivers_resumable_session_missing_models_dir() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "missing-models-dir-resumed.txt"));
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_without_models_dir(SESSION);
    fixture.seed_mailbox_for(SESSION, "h-missing-models-dir", None);
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_startup_recovery(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("missing-models-dir-resumed.txt"));
    assert_prompt_contains_handle(&prompt, "h-missing-models-dir");
    wait_until("missing models_dir wake delivered", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    assert_one_failed_delivery(&fixture, SESSION);
    fixture.assert_recovery_drained(SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_disturb_live_identity_matched_claim() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "live-claim-not-disturbed.txt"));
    fixture.establish_recovery_parent("77777777-7777-4777-8777-777777777777");
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-live-claim");
    seed_live_wake_claim(&fixture, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");

    fixture.seed_recovery_control();
    let output = fixture.run_startup_recovery("77777777-7777-4777-8777-777777777777");
    assert_success(&output);
    fixture.assert_recovery_control();

    assert_prompt_file_missing(&fixture, "live-claim-not-disturbed.txt");
    assert_live_claim_token(&fixture, SESSION, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_treat_pre_anchor_prose_as_consumption() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "pre-anchor-prose-retried.txt"));
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-consumed");
    fixture.seed_consumed_notification_turn("h-consumed");
    seed_dead_wake_claim(&fixture, "cccccccc-cccc-4ccc-8ccc-cccccccccccc", 601);

    let output = fixture.run_startup_recovery(SESSION);
    assert_success(&output);
    // The stored prose predates the delivery anchor and has no exact nonce.
    // It cannot suppress this row; only the new provider-observed submission
    // may settle it (sweep/consumed.rs and the AGE347 observation contract).
    let prompt = wait_for_file(&fixture.prompt_file("pre-anchor-prose-retried.txt"));
    assert_prompt_contains_handle(&prompt, "h-consumed");
    wait_until("fresh post-anchor evidence settles the pending row", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    let row = fixture
        .mailbox()
        .list_mailbox(SESSION, true)
        .unwrap()
        .remove(0);
    assert_eq!(row.delivery_attempts, 1);
    assert!(row.delivered_by_invocation_uuid.is_some());
    assert_pending_mailbox_count(&fixture, SESSION, 0);
    fixture.assert_recovery_drained(SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_retries_twice_unconfirmed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "twice-unconfirmed-retried.txt"));
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-unconfirmed");
    fixture.mark_mailbox_unconfirmed_twice(SESSION, "h-unconfirmed");
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_startup_recovery(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("twice-unconfirmed-retried.txt"));
    assert_prompt_contains_handle(&prompt, "h-unconfirmed");
    wait_until("twice-unconfirmed mailbox retried and delivered", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    let row = fixture
        .mailbox()
        .list_mailbox(SESSION, true)
        .unwrap()
        .remove(0);
    assert_eq!(row.delivery_attempts, 3);
    assert_age270_invocation(
        &fixture,
        row.delivered_by_invocation_uuid.as_deref().unwrap(),
    );
    fixture.assert_recovery_drained(SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn failed_auto_wake_retains_retry_ownership_during_backoff() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let first_failure = fixture.work_dir.join("first-auto-wake-failed");
    let hook = format!(
        r#"if [ "$WU_D_ANCHOR_INDEX" = 2 ]; then
  : > {}
  exit 17
fi"#,
        shell_path(&first_failure),
    );
    fixture.write_provider(&anchor_admission_provider_script(
        &hook,
        "",
        "retry-owner-${WU_D_PROVIDER_RESUME_INDEX}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    for index in 0..21 {
        fixture.seed_mailbox(SESSION, &format!("h-retry-owner-{index:02}"));
    }

    let manual_resume = fixture.run_resume_with_retry_base(2_000);
    let original_parent = direct_unconfirmed_invocation(&manual_resume);
    wait_until("first automatic wake failed and entered backoff", || {
        first_failure.exists()
            && crate::liveness::runtime_is_idle(&fixture, SESSION)
            && invocation_count(&fixture) == 2
    });
    std::thread::sleep(std::time::Duration::from_millis(250));

    let claim = fixture
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .expect("failed automatic wake must retain retry ownership during backoff");
    assert_eq!(claim.auto_wake_count, 1);
    assert!(claim.wake_pid.is_some());

    // A rejected native allocation is diagnostic history, not a replacement
    // parent. This samples the real failure while its retry owner still holds.
    let metadata = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(
        metadata.invocation_uuid.as_deref(),
        Some(original_parent.as_str())
    );
    let failed_uuid: String = fixture
        .state()
        .connection()
        .query_row(
            "SELECT invocation_uuid FROM invocations WHERE error_category='guard_drop'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let failed = fixture
        .state()
        .get_invocation_by_uuid(&failed_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(failed.status, oulipoly_state::InvocationStatus::Failed);
    assert_eq!(failed.success, Some(false));
    assert_eq!(failed.exit_code, Some(-1));
    assert!(failed.session_id.is_none() && failed.provider_session_id.is_none());
    let parent = fixture
        .state()
        .get_invocation_by_uuid(&original_parent)
        .unwrap()
        .unwrap();
    assert_eq!(parent.provider_session_id.as_deref(), Some(SESSION));
    assert_eq!(failed.parent_invocation_id, Some(parent.id));
    assert_rejected_anchor_attempts(&fixture, 1, 1);
    println!("retry backoff metadata={metadata:?} failed={failed:?}");

    let overlapping_sweep = fixture.run_mailbox_list(SESSION);
    assert_success(&overlapping_sweep);
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "1",
        "inspection must not disturb the live retry owner; rejected anchor never launched"
    );

    wait_until("owned retry renewed and delivered pending mailbox", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 21)
    });
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "2"
    );
    assert_rejected_anchor_attempts(&fixture, 1, 1);
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(
        rows[..20]
            .iter()
            .all(|row| row.delivered_by_invocation_uuid.as_deref()
                == Some(original_parent.as_str())
                && row.delivery_attempts == 1)
    );
    let retry_uuid = rows[20].delivered_by_invocation_uuid.as_deref().unwrap();
    assert_ne!(retry_uuid, failed_uuid);
    assert_ne!(retry_uuid, original_parent);
    assert_age270_invocation(&fixture, retry_uuid);
    let retry = fixture
        .state()
        .get_invocation_by_uuid(retry_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(retry.provider_session_id.as_deref(), Some(SESSION));
    assert_eq!(retry.parent_invocation_id, Some(parent.id));
    assert_eq!(invocation_count(&fixture), 3);
    assert_retry_parent_custody_drained(&fixture, retry_uuid);
    let metadata = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(metadata.auto_wake_count, 2);
    let selected = fixture
        .state()
        .get_invocation_by_uuid(metadata.invocation_uuid.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(selected.provider_session_id.as_deref(), Some(SESSION));
    assert_ne!(selected.invocation_uuid, failed_uuid);
    println!("retry recovered metadata={metadata:?} bound={retry:?}");
    assert_xdg_isolated(&fixture);
}

pub(crate) fn maximum_chronology_and_delivery_attempts_stay_eligible_across_rechecks() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let first_failure = fixture.work_dir.join("maximum-chronology-first-failure");
    let recovery_started = fixture.work_dir.join("maximum-recovery-started");
    let release_recovery = fixture.work_dir.join("maximum-release-recovery");
    let hook = format!(
        r#"if [ "$WU_D_ANCHOR_INDEX" = 1 ]; then
  : > {first_failure}
  exit 17
fi
if [ "$WU_D_ANCHOR_INDEX" = 2 ]; then
  : > {recovery_started}
  while [ ! -e {release_recovery} ]; do sleep 0.01; done
fi"#,
        first_failure = shell_path(&first_failure),
        recovery_started = shell_path(&recovery_started),
        release_recovery = shell_path(&release_recovery),
    );
    // Recovery must answer, not merely receipt the notification. Anchor
    // failures still happen before launch; other scenarios remain receipt-only.
    fixture.write_provider(
        &anchor_admission_provider_script(
            &hook,
            "",
            "maximum-chronology-${WU_D_PROVIDER_RESUME_INDEX}.txt",
        )
        .replace(
            "SUCCESSFUL_ASSISTANT = False",
            "SUCCESSFUL_ASSISTANT = True",
        ),
    );
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, i64::MAX);
    for index in 0..21 {
        fixture.seed_mailbox(SESSION, &format!("h-maximum-chronology-{index:02}"));
    }
    fixture
        .sidecar_conn()
        .execute(
            "UPDATE mailbox
             SET delivery_attempts = ?2
             WHERE seq IN (
                 SELECT seq FROM mailbox
                 WHERE session_id = ?1
                 ORDER BY enqueued_at, seq
                 LIMIT 20
             )",
            rusqlite::params![SESSION, i64::MAX - 1],
        )
        .unwrap();

    // A real completed native parent is selected; startup admission elects the
    // independent driver. Historical count is input, never launch authority.
    assert_success(&fixture.run_startup_recovery(SESSION));
    wait_until(
        "maximum-count failure reached real anchor admission",
        || first_failure.exists(),
    );
    // Hold only the recovery anchor so failure/cadence evidence is checked
    // before the first valid answer. Both waits share the original 45s budget.
    let recovery_deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    wait_until_with_timeout(
        "maximum chronology recovery held before submission",
        recovery_deadline.saturating_duration_since(std::time::Instant::now()),
        || recovery_started.exists(),
    );
    assert_rejected_anchor_attempts(&fixture, 1, 20);
    let pending = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert_eq!(pending.len(), 21);
    assert!(
        pending
            .iter()
            .all(|row| row.delivered_at.is_none() && row.delivery_error.is_none())
    );
    assert!(
        pending[..20]
            .iter()
            .all(|row| row.delivery_attempts == i64::MAX - 1)
    );
    assert_eq!(pending[20].delivery_attempts, 0);
    assert!(
        !fixture
            .work_dir
            .join("provider-resume-sequence.txt")
            .exists()
    );
    let intervals = native_failed_retry_intervals_ms(&fixture, 1);
    assert_eq!(intervals.len(), 1);
    assert_retry_interval_ms(intervals[0], 30_000);
    println!("maximum-chronology finished-to-created retry ms={intervals:?}");
    std::fs::write(&release_recovery, "release\n").unwrap();
    wait_until_with_timeout(
        "maximum chronology retry delivered all pending rows",
        recovery_deadline.saturating_duration_since(std::time::Instant::now()),
        || delivered_rows_without_pending_or_claim(&fixture, SESSION, 21),
    );
    assert_recovery_batch_prompts(&fixture, "maximum-chronology", "h-maximum-chronology");
    assert_native_retry_custody(&fixture, 1, 2);
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 21);
    assert!(
        rows[..20]
            .iter()
            .all(|row| row.delivery_attempts == i64::MAX),
        "rejected anchor is not a delivery attempt; successful delivery saturates at i64::MAX"
    );
    assert_eq!(rows[20].delivery_attempts, 1);
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.auto_wake_count, i64::MAX);
    assert_rejected_anchor_attempts(&fixture, 1, 20);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn repeated_failed_wakes_keep_oldest_batch_owned_past_terminal_budget() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let attempt_ledger = fixture.work_dir.join("persistent-failure-attempts.txt");
    let seventh_started = fixture.work_dir.join("persistent-failure-seventh-started");
    let release_seventh = fixture.work_dir.join("persistent-failure-release-seventh");
    let hook = format!(
        r#"python3 - {attempt_ledger} "$WU_D_ANCHOR_INDEX" <<'PY'
import sys
import time

with open(sys.argv[1], "a", encoding="utf-8") as out:
    out.write(f"{{sys.argv[2]}} {{time.monotonic_ns()}}\n")
PY
if [ "$WU_D_ANCHOR_INDEX" -le 6 ]; then
  exit 17
fi
if [ "$WU_D_ANCHOR_INDEX" = 7 ]; then
  : > {seventh_started}
  while [ ! -e {release_seventh} ]; do sleep 0.01; done
fi"#,
        attempt_ledger = shell_path(&attempt_ledger),
        seventh_started = shell_path(&seventh_started),
        release_seventh = shell_path(&release_seventh),
    );
    // Recovery must answer, not merely receipt the notification. Anchor
    // failures still happen before launch; other scenarios remain receipt-only.
    fixture.write_provider(
        &anchor_admission_provider_script(
            &hook,
            "",
            "persistent-failure-${WU_D_PROVIDER_RESUME_INDEX}.txt",
        )
        .replace(
            "SUCCESSFUL_ASSISTANT = False",
            "SUCCESSFUL_ASSISTANT = True",
        ),
    );
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, 0);
    for index in 0..21 {
        fixture.seed_mailbox(SESSION, &format!("h-persistent-failure-{index:02}"));
    }
    assert_success(&fixture.run_startup_recovery(SESSION));
    wait_until_with_timeout(
        "seventh production retry reached pre-submission anchor admission",
        std::time::Duration::from_secs(120),
        || seventh_started.exists(),
    );

    let retained_claim = fixture
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .expect("six consecutive failures must retain one claim for a seventh attempt");
    assert!(!retained_claim.claim_token.is_empty());
    assert_eq!(retained_claim.auto_wake_count, 7);
    assert!(retained_claim.wake_pid.is_some());
    let pending = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(
        pending[..20]
            .iter()
            .all(|row| row.delivery_attempts == 0 && row.delivery_error.is_none())
    );
    assert_rejected_anchor_attempts(&fixture, 6, 20);
    let conn = fixture.sidecar_conn();
    let newer_memberships: i64 = conn.query_row(
        "SELECT COUNT(*) FROM mailbox_delivery_attempt_items i JOIN mailbox m ON m.seq=i.mailbox_seq WHERE m.handle='h-persistent-failure-20'",
        [], |row| row.get(0)).unwrap();
    assert_eq!(
        newer_memberships, 0,
        "newer work cannot displace the held oldest batch"
    );
    let claim_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM session_wake_claim WHERE session_id=?1",
            [SESSION],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(claim_count, 1);

    assert!(
        !fixture
            .work_dir
            .join("provider-resume-sequence.txt")
            .exists(),
        "six rejected admissions and held seventh must cause zero semantic submissions"
    );
    assert_eq!(pending[20].delivery_attempts, 0);
    assert!(pending[20].delivery_error.is_none());
    assert!(pending.iter().all(|row| row.delivered_at.is_none()));

    // Rejection unwinds the invocation guard before the failed-wake recheck
    // sleeps. Anchor entry precedes that unwinding; timing from there charges
    // observation/teardown to backoff. The next invocation starts after renewal.
    // This bracket still includes recheck/spawn overhead, not just the sleep.
    let retry_intervals_ms = native_failed_retry_intervals_ms(&fixture, 6);
    eprintln!("production finished-to-created retry intervals (ms): {retry_intervals_ms:?}");
    for (elapsed_ms, expected_ms) in retry_intervals_ms
        .iter()
        .zip([1_000_i64, 2_000, 4_000, 8_000, 16_000, 30_000])
    {
        assert_retry_interval_ms(*elapsed_ms, expected_ms);
    }

    std::fs::write(&release_seventh, "release\n").unwrap();
    wait_until(
        "persistent failure lifecycle delivered oldest and newer work",
        || delivered_rows_without_pending_or_claim(&fixture, SESSION, 21),
    );
    assert_recovery_batch_prompts(&fixture, "persistent-failure", "h-persistent-failure");
    let oldest = wait_for_file(&fixture.prompt_file("persistent-failure-1.txt"));
    assert_prompt_contains_handle(&oldest, "h-persistent-failure-00");
    assert_prompt_contains_handle(&oldest, "h-persistent-failure-19");
    assert!(!oldest.contains("h-persistent-failure-20"), "{oldest}");
    let newer = wait_for_file(&fixture.prompt_file("persistent-failure-2.txt"));
    assert_prompt_contains_handle(&newer, "h-persistent-failure-20");
    assert!(!newer.contains("h-persistent-failure-00"), "{newer}");

    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(rows[..20].iter().all(|row| row.delivery_attempts == 1));
    assert_eq!(rows[20].delivery_attempts, 1);
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "2"
    );
    let attempts = std::fs::read_to_string(&attempt_ledger)
        .unwrap()
        .lines()
        .map(|line| {
            let (index, timestamp) = line.split_once(' ').unwrap();
            (
                index.parse::<usize>().unwrap(),
                timestamp.parse::<u128>().unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        attempts.iter().map(|entry| entry.0).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    let anchor_intervals_ms = attempts[..7]
        .windows(2)
        .map(|pair| (pair[1].1 - pair[0].1) / 1_000_000)
        .collect::<Vec<_>>();
    eprintln!("anchor-entry intervals including active work (ms): {anchor_intervals_ms:?}");
    assert_native_retry_custody(&fixture, 6, 2);
    assert_xdg_isolated(&fixture);
}

fn native_failed_retry_intervals_ms(fixture: &Fixture, failures: usize) -> Vec<i64> {
    let state = fixture.state();
    let connection = state.connection();
    let mut statement = connection
        .prepare("SELECT created_at, finished_at, status FROM invocations ORDER BY id")
        .unwrap();
    let invocations = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        invocations.len(),
        failures + 2,
        "recovery held before answer/newer launch"
    );
    assert_eq!(
        invocations[0].2, "succeeded",
        "genuine initial native parent"
    );
    invocations[1..failures + 2]
        .windows(2)
        .map(|pair| {
            assert_eq!(pair[0].2, "failed");
            finished_to_created_ms(
                pair[0].1.as_deref().expect("rejected invocation finished"),
                &pair[1].0,
            )
        })
        .collect()
}

fn finished_to_created_ms(finished_at: &str, next_created_at: &str) -> i64 {
    let finished = chrono::DateTime::parse_from_rfc3339(finished_at).unwrap();
    let next_created = chrono::DateTime::parse_from_rfc3339(next_created_at).unwrap();
    next_created
        .signed_duration_since(finished)
        .num_milliseconds()
}

fn assert_retry_interval_ms(elapsed_ms: i64, expected_ms: i64) {
    assert!(
        elapsed_ms >= expected_ms - 100 && elapsed_ms <= expected_ms + 1_500,
        "production retry did not follow the selected exponential cadence: \
         expected_ms={expected_ms}, elapsed_ms={elapsed_ms}"
    );
}

#[test]
fn retry_timestamp_oracle_excludes_active_attempt_duration() {
    let created = "2026-09-09T10:00:00+00:00";
    let finished = "2026-09-09T10:00:03+00:00";
    let next_created = "2026-09-09T10:00:04+00:00";
    assert_eq!(finished_to_created_ms(created, next_created), 4_000);
    assert_eq!(finished_to_created_ms(finished, next_created), 1_000);
    assert_eq!(finished_to_created_ms(next_created, finished), -1_000);
    assert_retry_interval_ms(finished_to_created_ms(finished, next_created), 1_000);
}

#[test]
#[should_panic(expected = "production retry did not follow the selected exponential cadence")]
fn retry_timestamp_oracle_rejects_early_retry() {
    assert_retry_interval_ms(899, 1_000);
}

#[test]
#[should_panic(expected = "production retry did not follow the selected exponential cadence")]
fn retry_timestamp_oracle_rejects_late_retry() {
    assert_retry_interval_ms(2_501, 1_000);
}

#[cfg(target_os = "linux")]
#[test]
fn renewed_followup_claim_survives_old_failed_child_recheck() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let fifo = fixture.work_dir.join("renewed-release.fifo");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let ledger = fixture.work_dir.join("renewed-ledger.txt");
    let count1_pid = fixture.work_dir.join("count1.pid");
    let count1_start = fixture.work_dir.join("count1.start");
    let count2_pid = fixture.work_dir.join("count2.pid");
    let held = fixture.work_dir.join("count2.held");
    let hook = format!(
        r#"index="$WU_D_PROVIDER_RESUME_INDEX"
label=manual
if [ "$index" -gt 1 ]; then
  label=$((index - 1))
fi
# Join this provider invocation to the generation creator, not a fixed
# ancestor depth (published proxy and custodian are not wake runners).
wake_pid="$(python3 - <<'PYOWNER'
import json, os, pathlib, sqlite3
invocation = json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"]
path = pathlib.Path(os.environ["OULIPOLY_DATA_DIR"]) / "pid-identity.db"
with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True) as db:
    rows = db.execute("SELECT creator_identity_os_pid, creator_identity_os_boot_id, creator_identity_os_pid_starttime_ticks FROM runtime_generation WHERE spawn_invocation_uuid = ?", (invocation,)).fetchall()
assert len(rows) == 1, rows
pid, boot, start = rows[0]
assert pathlib.Path("/proc/sys/kernel/random/boot_id").read_text().strip() == boot
stat = pathlib.Path(f"/proc/{{pid}}/stat").read_text().rsplit(") ", 1)[1].split()
assert int(stat[19]) == start
assert os.path.samefile(f"/proc/{{pid}}/exe", os.environ["AGENT_BASH_AGENT_RUNNER_BIN"])
print(pid)
PYOWNER
)" || exit 92
printf '%s|%s\n' "$label" "$wake_pid" >> {ledger}
if [ "$index" = 2 ]; then
  printf '%s' "$wake_pid" > {count1_pid}
  awk '{{print $22}}' "/proc/$wake_pid/stat" > {count1_start}
fi
if [ "$index" = 3 ]; then
  printf '%s' "$wake_pid" > {count2_pid}
  : > {held}
  IFS= read -r _ < {fifo}
fi"#,
        ledger = shell_path(&ledger),
        count1_pid = shell_path(&count1_pid),
        count1_start = shell_path(&count1_start),
        count2_pid = shell_path(&count2_pid),
        held = shell_path(&held),
        fifo = shell_path(&fifo),
    );
    fixture.write_provider(&provider_script(
        "",
        &hook,
        "batch-${WU_D_PROVIDER_RESUME_INDEX}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    for index in 0..45 {
        fixture.seed_mailbox(SESSION, &format!("h-renewed-{index:02}"));
    }
    let output = fixture.run_resume();
    let manual_invocation = direct_unconfirmed_invocation(&output);
    wait_until("count 2 held", || held.exists() && count2_pid.exists());
    assert_eq!(
        std::fs::read_dir(fixture.work_dir.join("session-turns"))
            .unwrap()
            .count(),
        2
    );
    assert_eq!(fixture.mailbox().list_pending(SESSION).unwrap().len(), 5);
    let old_pid = std::fs::read_to_string(&count1_pid)
        .unwrap()
        .parse::<u32>()
        .unwrap();
    let old_start = std::fs::read_to_string(&count1_start)
        .unwrap()
        .trim()
        .to_string();
    wait_until("count 1 process identity gone", || {
        process_start(old_pid).as_deref() != Some(old_start.as_str())
    });
    let renewed_pid = std::fs::read_to_string(&count2_pid)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let claim = fixture
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .unwrap();
    assert!(!claim.claim_token.is_empty());
    assert_eq!(claim.wake_pid, Some(renewed_pid));
    assert_eq!(claim.auto_wake_count, 2);
    eprintln!(
        "renewed claim: old_runner={old_pid} old_start={old_start} identity_gone=true renewed_runner={renewed_pid} claim_pid={:?} auto_wake_count=2",
        claim.wake_pid
    );
    assert_eq!(invocation_count(&fixture), 3);
    let ledger_lines = std::fs::read_to_string(&ledger)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(ledger_lines.len(), 3, "{ledger_lines:?}");
    assert_eq!(
        ledger_lines
            .iter()
            .filter(|line| line.starts_with("manual|"))
            .count(),
        1
    );
    assert_eq!(
        ledger_lines
            .iter()
            .filter(|line| line.starts_with("1|"))
            .count(),
        1
    );
    assert_eq!(
        ledger_lines
            .iter()
            .filter(|line| line.starts_with("2|"))
            .count(),
        1
    );
    assert!(fixture.prompt_file("batch-1.txt").exists());
    assert!(fixture.prompt_file("batch-2.txt").exists());
    assert!(fixture.prompt_file("batch-3.txt").exists());
    assert!(!fixture.prompt_file("batch-4.txt").exists());
    std::fs::write(&fifo, "release\n").unwrap();
    wait_until("renewed delivery settled", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 45)
    });
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(
        rows.iter()
            .all(|row| row.delivery_attempts == 1 && row.delivery_error.is_none())
    );
    let first = rows[0].delivered_by_invocation_uuid.clone().unwrap();
    let second = rows[20].delivered_by_invocation_uuid.clone().unwrap();
    let third = rows[40].delivered_by_invocation_uuid.clone().unwrap();
    assert_eq!(first, manual_invocation);
    assert_ne!(first, second);
    assert_ne!(second, third);
    assert!(
        rows[..20]
            .iter()
            .all(|row| row.delivered_by_invocation_uuid.as_deref() == Some(first.as_str()))
    );
    assert!(
        rows[20..40]
            .iter()
            .all(|row| row.delivered_by_invocation_uuid.as_deref() == Some(second.as_str()))
    );
    assert!(
        rows[40..]
            .iter()
            .all(|row| row.delivered_by_invocation_uuid.as_deref() == Some(third.as_str()))
    );
    for id in [&first, &second, &third] {
        assert_age270_invocation(&fixture, id);
    }
    assert_eq!(invocation_count(&fixture), 3);
    eprintln!(
        "renewed settlement: rows=45 batches=20/20/5 delivery_attempts=1 errors=0 pending=0 claim=none invocations=3"
    );
}

fn invocation_count(fixture: &Fixture) -> i64 {
    fixture
        .state()
        .connection()
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

fn shell_path(path: &std::path::Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
fn process_start(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let tail = stat.rsplit_once(") ")?.1;
    tail.split_whitespace().nth(19).map(str::to_string)
}

pub(crate) fn maximum_persisted_count_allows_startup_sweep_delivery() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let started = fixture.work_dir.join("maximum-sweep-started");
    let release = fixture.work_dir.join("maximum-sweep-release");
    let hook = format!(
        ": > {started}\nwhile [ ! -e {release} ]; do sleep 0.01; done",
        started = shell_path(&started),
        release = shell_path(&release),
    );
    fixture.write_provider(&provider_script("", &hook, "maximum-sweep-count.txt"));
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, i64::MAX);
    fixture.seed_mailbox_for(SESSION, "h-sweep-count", None);
    let stale_token = "abababab-abab-4bab-8bab-abababababab";
    seed_dead_wake_claim(&fixture, stale_token, 601);

    let output = fixture.run_startup_recovery(SESSION);
    assert_success(&output);

    wait_for_file(&started);
    let claim = fixture
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .expect("maximum chronology sweep must acquire a replacement claim");
    assert_ne!(claim.claim_token, stale_token);
    assert_eq!(claim.auto_wake_count, i64::MAX);
    assert!(claim.wake_pid.is_some());
    let prompt = wait_for_file(&fixture.prompt_file("maximum-sweep-count.txt"));
    assert_prompt_contains_handle(&prompt, "h-sweep-count");
    std::fs::write(&release, "release\n").unwrap();
    wait_until("maximum chronology startup sweep delivery", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.auto_wake_count, i64::MAX);
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "1"
    );
    assert_one_failed_delivery(&fixture, SESSION);
    fixture.assert_recovery_drained(SESSION);
    assert_xdg_isolated(&fixture);
}

// Read-only proof from real admission failures, never injected submission state.
fn assert_rejected_anchor_attempts(fixture: &Fixture, expected: i64, batch_size: i64) {
    let conn = fixture.sidecar_conn();
    let rejected: i64 = conn.query_row(
        "SELECT count(*) FROM mailbox_delivery_attempts a
         WHERE observation_error LIKE '%offline_anchor_unavailable%'
           AND headless_submission_state = 'prepared' AND submission_started_at IS NULL
           AND observation_anchor_token IS NULL AND observation_confirmed_at IS NULL
           AND (SELECT count(*) FROM mailbox_delivery_attempt_items i WHERE i.attempt_id=a.attempt_id)=?1",
        [batch_size], |row| row.get(0)).unwrap();
    assert_eq!(
        rejected, expected,
        "only the prepared-before-CAS path proves non-submission"
    );
    assert_rejected_logical_launches(fixture, expected);
}

#[test]
fn failed_provider_exit_and_empty_observation_retain_uncertainty_across_rechecks() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "exit 17", "failed-after-cas.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-failed-after-cas");
    let output = fixture.run_resume();
    assert_eq!(output.status.code(), Some(17), "{output:?}");
    let attempt: String = fixture.sidecar_conn().query_row(
        "SELECT attempt_id FROM mailbox_delivery_attempts WHERE submission_started_at IS NOT NULL AND resolved_at IS NULL",
        [], |row| row.get(0)).unwrap();
    let invocations_before = invocation_count(&fixture);
    for _ in 0..3 {
        assert_success(&fixture.run_startup_recovery(SESSION)); // fresh Runner process
        settle_wake_sweep();
        assert_eq!(
            std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
            "1"
        );
        assert!(!fixture.work_dir.join("session-turns").exists());
        let retained: i64 = fixture.sidecar_conn().query_row(
            "SELECT count(*) FROM mailbox_delivery_attempts WHERE attempt_id=?1
             AND headless_submission_state='possible' AND submission_started_at IS NOT NULL
             AND resolved_at IS NULL AND observation_confirmed_at IS NULL AND acknowledged_at IS NULL",
            [&attempt], |row| row.get(0)).unwrap();
        assert_eq!(retained, 1);
        let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].delivered_at.is_none());
        assert_eq!(rows[0].delivery_attempts, 1);
    }
    assert_eq!(invocation_count(&fixture), invocations_before);
    // Capture only already-observed native custody. Uncertainty permits no
    // provider replay, but the independent owner can attempt a preflight and
    // refuse it before publishing any invocation/runtime generation.
    assert_observed_uncertainty_custody_drained(&fixture);
    assert_eq!(invocation_count(&fixture), invocations_before);
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "1"
    );
    let retained: i64 = fixture
        .sidecar_conn()
        .query_row(
            "SELECT count(*) FROM mailbox_delivery_attempts WHERE attempt_id=?1
         AND headless_submission_state='possible' AND submission_started_at IS NOT NULL
         AND resolved_at IS NULL AND observation_confirmed_at IS NULL AND acknowledged_at IS NULL",
            [&attempt],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, 1);
    println!(
        "uncertainty retained attempt={attempt} invocations={invocations_before}; no provider replay"
    );
    assert_xdg_isolated(&fixture);
}

// Join the delivered row to the actual resumed invocation and native activation,
// not merely to a fake-provider receipt or a prompt file that can be overwritten.
fn assert_exact_native_delivery(fixture: &Fixture, session: &str, handle: &str) {
    let rows = fixture.mailbox().list_mailbox(session, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].handle, handle);
    let id = rows[0].delivered_by_invocation_uuid.as_deref().unwrap();
    let invocation = fixture.state().get_invocation_by_uuid(id).unwrap().unwrap();
    assert_eq!(invocation.provider_session_id.as_deref(), Some(session));
    assert_eq!(invocation.provider_name.as_deref(), Some(PROVIDER));
    assert_age270_invocation(fixture, id);
    let count: i64 = fixture
        .state()
        .connection()
        .query_row(
            "SELECT count(*) FROM invocations WHERE provider_session_id=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2, "one genuine initial invocation and one resume");
    let activations: i64 = fixture
        .sidecar_conn()
        .query_row(
            "SELECT count(*) FROM completion_continuation_attempt WHERE session_id=?1",
            [session],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(activations, 1, "no duplicate native execution");
    let bound: String = fixture.sidecar_conn().query_row(
        "SELECT spawn_invocation_uuid FROM completion_continuation_attempt WHERE session_id=?1 AND phase='drained' AND integrated=1",
        [session], |row| row.get(0)).unwrap();
    assert_eq!(
        bound, id,
        "delivery identity must match actual drained launcher"
    );
    println!("exact native delivery session={session} handle={handle} invocation={id}");
}

#[test]
fn native_bound_pause_retains_legacy_notification_with_functioning_recipient() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "paused-native.txt"));
    fixture.establish_recovery_parent("77777777-7777-4777-8777-777777777777");
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture
        .mailbox()
        .set_notifications_paused(SESSION, true)
        .unwrap();
    fixture.seed_mailbox(SESSION, "h-paused-native");
    fixture.seed_recovery_control();
    assert_success(&fixture.run_startup_recovery("77777777-7777-4777-8777-777777777777"));
    fixture.assert_recovery_control();
    assert_prompt_file_missing(&fixture, "paused-native.txt");
    assert_pending_handle_without_error(&fixture, SESSION, "h-paused-native");
    assert_no_wake_claim(&fixture, SESSION);
    assert!(fixture.mailbox().notifications_paused(SESSION).unwrap());
    assert_eq!(invocation_count(&fixture), 3);
    // Existing explicit mailbox resume, not TTL0 handshake, clears this pause.
    let mut command = std::process::Command::new(crate::parse::runner_bin());
    command.args(["mailbox", "resume", "--session-id", SESSION, "--json"]);
    assert_success(&fixture.run(command));
    wait_until("explicit unpause delivers native recipient", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 1)
    });
    fixture.assert_recovery_drained(SESSION);
    assert_exact_native_delivery(&fixture, SESSION, "h-paused-native");
    assert_xdg_isolated(&fixture);
}

#[test]
fn native_bound_observation_stop_retains_legacy_notification_with_functioning_recipient() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "stopped-native.txt"));
    fixture.establish_recovery_parent("77777777-7777-4777-8777-777777777777");
    fixture.establish_recovery_parent(SESSION);
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-stopped-native");
    // Historical unresolved observation is input, never native custody authority.
    let mut mailbox = fixture.mailbox();
    let row = mailbox.list_pending(SESSION).unwrap().remove(0);
    mailbox
        .register_headless_delivery_attempt(
            "historical-stop-attempt",
            SESSION,
            None,
            "historical-stop-owner",
            &[row.seq],
            0,
        )
        .unwrap();
    mailbox
        .stop_mailbox_observation(
            SESSION,
            "historical-stop-attempt",
            "capacity",
            "explicit retained observation failure",
        )
        .unwrap();
    let stop = mailbox.mailbox_observation_stop(SESSION).unwrap().unwrap();
    fixture.seed_recovery_control();
    assert_success(&fixture.run_startup_recovery("77777777-7777-4777-8777-777777777777"));
    fixture.assert_recovery_control();
    assert_prompt_file_missing(&fixture, "stopped-native.txt");
    assert_pending_handle_without_error(&fixture, SESSION, "h-stopped-native");
    assert_no_wake_claim(&fixture, SESSION);
    let retained = mailbox.mailbox_observation_stop(SESSION).unwrap().unwrap();
    assert_eq!(retained.stop_id, stop.stop_id);
    assert_eq!(retained.error, stop.error);
    assert_eq!(invocation_count(&fixture), 3);
    // Pausing/resuming notifications must not silently clear an observation stop.
    let mut command = std::process::Command::new(crate::parse::runner_bin());
    command.args(["mailbox", "resume", "--session-id", SESSION, "--json"]);
    assert_success(&fixture.run(command));
    assert_eq!(
        mailbox
            .mailbox_observation_stop(SESSION)
            .unwrap()
            .unwrap()
            .stop_id,
        stop.stop_id
    );
    assert_prompt_file_missing(&fixture, "stopped-native.txt");
    // Explicit resolution of this historical fixture stop is a separate action.
    mailbox
        .rearm_mailbox_observation(
            SESSION,
            &stop.stop_id,
            "fixture observation capacity available; historical prepared attempt never submitted",
        )
        .unwrap();
    assert_success(&fixture.run_startup_recovery(SESSION));
    wait_until("explicitly rearmed native recipient delivers", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 1)
    });
    fixture.assert_recovery_drained(SESSION);
    assert_exact_native_delivery(&fixture, SESSION, "h-stopped-native");
    assert_xdg_isolated(&fixture);
}

// Ongoing uncertainty is not domain quiescence. Snapshot actual custody IDs,
// retain their physical results, and do not demand a launch or a runtime binding
// merely to satisfy a test helper. Later owner attempts remain outside this
// bounded drain witness; the test independently rechecks no provider replay.
fn assert_observed_uncertainty_custody_drained(fixture: &Fixture) {
    let conn = fixture.sidecar_conn();
    let mut stmt = conn
        .prepare("SELECT attempt_id FROM completion_continuation_attempt WHERE session_id=?1")
        .unwrap();
    let ids = stmt
        .query_map([SESSION], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    println!("observed uncertainty custody IDs={ids:?}; empty is admissible");
    for id in ids {
        wait_until("observed preflight custody physically drained", || {
            fixture.sidecar_conn().query_row(
                "SELECT phase='drained' AND integrated=1 AND drain_receipt LIKE '%ECHILD%' FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&id], |row| row.get::<_, bool>(0)).unwrap()
        });
        let (custodian, launcher, generation, invocation, receipt): (String, String, Option<String>, Option<String>, String) =
            fixture.sidecar_conn().query_row(
                "SELECT custodian_identity,launcher_identity,runtime_generation_uuid,spawn_invocation_uuid,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).unwrap();
        for identity in [&custodian, &launcher] {
            let actor: serde_json::Value = serde_json::from_str(identity).unwrap();
            assert_ne!(actor["pid"].as_u64(), Some(u64::from(std::process::id())));
        }
        assert!(
            generation.is_none(),
            "possible submission must block before runtime launch"
        );
        assert!(
            invocation.is_none(),
            "possible submission must not create a replay invocation"
        );
        println!(
            "observed preflight drain attempt={id} custodian={custodian} launcher={launcher} receipt={receipt}"
        );
    }
}

fn assert_retry_parent_custody_drained(fixture: &Fixture, retry_uuid: &str) {
    assert_rejected_logical_launches(fixture, 1);
    assert_terminal_logical_launch(fixture, retry_uuid, "failed");

    wait_until("both original retry custodians drained", || {
        fixture.sidecar_conn().query_row(
            "SELECT COUNT(*)=2 AND SUM(phase='drained' AND integrated=1 AND drain_receipt LIKE '%ECHILD%')=2 FROM completion_continuation_attempt WHERE session_id=?1",
            [SESSION], |row| row.get::<_, bool>(0)).unwrap()
    });
    let conn = fixture.sidecar_conn();
    let mut statement = conn.prepare(
        "SELECT attempt_id,custodian_identity,launcher_identity,runtime_generation_uuid,spawn_invocation_uuid,drain_receipt FROM completion_continuation_attempt WHERE session_id=?1").unwrap();
    let rows = statement
        .query_map([SESSION], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows.iter()
            .filter(|row| row.3.is_none() && row.4.is_none())
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row.3.is_some() && row.4.as_deref() == Some(retry_uuid))
            .count(),
        1
    );
    for row in rows {
        for identity in [&row.1, &row.2] {
            let actor: serde_json::Value = serde_json::from_str(identity).unwrap();
            assert_ne!(actor["pid"].as_u64(), Some(u64::from(std::process::id())));
        }
        println!("original retry custody drain={row:?}");
    }
}

// Unlike successful-runtime-only helpers, pre-anchor rejection has no runtime.
// Exact original activations must still be independently drained, not ACKed.
fn assert_native_retry_custody(fixture: &Fixture, rejected: i64, launched: i64) {
    wait_until("all exact retry activations physically drained", || {
        fixture.sidecar_conn().query_row(
            "SELECT COUNT(*)=?2 AND SUM(phase='drained' AND integrated=1 AND drain_receipt LIKE '%ECHILD%')=?2 FROM completion_continuation_attempt WHERE session_id=?1",
            rusqlite::params![SESSION, rejected + launched], |r| r.get::<_, bool>(0)).unwrap()
    });
    let conn = fixture.sidecar_conn();
    let counts: (i64, i64) = conn.query_row(
        "SELECT SUM(runtime_generation_uuid IS NULL AND spawn_invocation_uuid IS NULL), SUM(runtime_generation_uuid IS NOT NULL AND spawn_invocation_uuid IS NOT NULL) FROM completion_continuation_attempt WHERE session_id=?1",
        [SESSION], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(counts, (rejected, launched));
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 21);
    let oldest = rows[0].delivered_by_invocation_uuid.as_deref().unwrap();
    let newer = rows[20].delivered_by_invocation_uuid.as_deref().unwrap();
    assert_ne!(oldest, newer);
    assert!(
        rows[..20]
            .iter()
            .all(|row| row.delivered_by_invocation_uuid.as_deref() == Some(oldest))
    );
    let launched_ids = conn.prepare("SELECT spawn_invocation_uuid FROM completion_continuation_attempt WHERE session_id=?1 AND spawn_invocation_uuid IS NOT NULL ORDER BY spawn_invocation_uuid")
        .unwrap().query_map([SESSION], |r| r.get::<_, String>(0)).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    let mut delivery_ids = vec![oldest.to_owned(), newer.to_owned()];
    delivery_ids.sort();
    assert_eq!(launched_ids, delivery_ids);
    assert_eq!(invocation_count(fixture), rejected + launched + 1);
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "2"
    );
    let state = fixture.state();
    let invocations = state.connection();
    let parent: (i64, String) = invocations.query_row(
        "SELECT id,invocation_uuid FROM invocations WHERE parent_invocation_id IS NULL AND status='succeeded' AND provider_session_id=?1",
        [SESSION], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let rejected_children: i64 = invocations.query_row(
        "SELECT COUNT(*) FROM invocations WHERE parent_invocation_id=?1 AND session_id IS NULL AND provider_session_id IS NULL AND status='failed' AND error_category='guard_drop'",
        [parent.0], |r| r.get(0)).unwrap();
    assert_eq!(rejected_children, rejected);
    assert_rejected_logical_launches(fixture, rejected);
    for id in &delivery_ids {
        assert_terminal_logical_launch(fixture, id, "succeeded");
    }

    for row in fixture.mailbox().list_mailbox(SESSION, true).unwrap() {
        let id = row.delivered_by_invocation_uuid.as_deref().unwrap();
        let child = state.get_invocation_by_uuid(id).unwrap().unwrap();
        assert_eq!(child.status, oulipoly_state::InvocationStatus::Succeeded);
        assert_eq!(child.success, Some(true));
        assert_eq!(child.exit_code, Some(0));
        assert!(child.error_category.is_none());
        assert_ne!(
            child.terminal_reason.as_deref(),
            Some("resume_completion_unconfirmed")
        );
        assert!(child.finished_at.is_some());
        assert_eq!(child.parent_invocation_id, Some(parent.0));
        assert!(row.delivered_at.is_some() && row.delivery_error.is_none());
        println!(
            "genuine recovered delivery seq={} handle={} invocation={id} outcome={child:?}",
            row.seq, row.handle
        );
        assert_eq!(child.provider_session_id.as_deref(), Some(SESSION));
    }
    println!(
        "genuine retry parent={parent:?}; rejected={rejected}, successful answering launches={launched}"
    );

    let mut stmt = conn.prepare("SELECT attempt_id,custodian_identity,launcher_identity,drain_receipt FROM completion_continuation_attempt WHERE session_id=?1").unwrap();
    for row in stmt
        .query_map([SESSION], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
    {
        let row = row.unwrap();
        for identity in [&row.1, &row.2] {
            let actor: serde_json::Value = serde_json::from_str(identity).unwrap();
            assert_ne!(actor["pid"].as_u64(), Some(u64::from(std::process::id())));
        }
        println!("native retry original drain={row:?}");
    }
}

// Exact batch membership at the provider boundary complements durable delivery IDs.
fn assert_recovery_batch_prompts(fixture: &Fixture, file_prefix: &str, handle_prefix: &str) {
    let oldest = wait_for_file(&fixture.prompt_file(&format!("{file_prefix}-1.txt")));
    let newer = wait_for_file(&fixture.prompt_file(&format!("{file_prefix}-2.txt")));
    for index in 0..21 {
        let handle = format!("handle: {handle_prefix}-{index:02}");
        assert_eq!(oldest.matches(&handle).count(), usize::from(index < 20));
        assert_eq!(newer.matches(&handle).count(), usize::from(index == 20));
    }
    assert!(
        !fixture
            .prompt_file(&format!("{file_prefix}-3.txt"))
            .exists()
    );
}

// Root-selected truthful lifecycle: exact failed-anchor invocations settle their
// own logical/attempt history; no synthetic runtime/endpoint or custody proof.
fn assert_rejected_logical_launches(fixture: &Fixture, expected: i64) {
    let ids = fixture
        .sidecar_conn()
        .prepare(
            "SELECT delivery_invocation_uuid FROM mailbox_delivery_attempts
         WHERE session_id=?1 AND observation_error LIKE '%offline_anchor_unavailable%'
           AND headless_submission_state='prepared' AND submission_started_at IS NULL
         ORDER BY created_at",
        )
        .unwrap()
        .query_map([SESSION], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(ids.len() as i64, expected);
    for id in ids {
        assert_terminal_logical_launch(fixture, &id, "failed");
        let state = fixture.state();
        let truthful: bool = state.connection().query_row(
            "SELECT i.status='failed' AND i.success=0 AND i.exit_code=-1
               AND i.terminal_reason='guard_drop' AND i.finished_at IS NOT NULL
               AND a.terminal_code='guard_drop' AND l.terminal_code='guard_drop'
               AND a.actor_custody_state='not_started' AND a.return_channel_state='not_created'
               AND a.endpoint_family IS NULL AND a.settings_id IS NULL
               AND a.provider_instance_id IS NULL AND a.endpoint_identity_sha256 IS NULL
               AND a.effect_incapable_at IS NULL AND a.actor_settlement_sha256 IS NULL
               AND a.runtime_settlement_sha256 IS NULL AND a.return_channel_settlement_sha256 IS NULL
               AND a.provider_session_observed+a.prompt_accepted+a.assistant_response_observed
                   +a.captured_child_count+a.returned_artifact_count+a.mailbox_submission_accepted=0
             FROM invocations i JOIN provider_launch_attempts a ON a.invocation_id=i.id
             JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id
             WHERE i.invocation_uuid=?1", [&id], |r| r.get(0)).unwrap();
        assert!(
            truthful,
            "failed anchor must not invent execution/custody: {id}"
        );
    }
}

fn assert_terminal_logical_launch(fixture: &Fixture, invocation_uuid: &str, status: &str) {
    let state = fixture.state();
    let row: (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
    ) = state
        .connection()
        .query_row(
            "SELECT l.logical_launch_id,a.attempt_id,l.status,a.status,l.finished_at,a.finished_at,
                l.terminal_code,a.terminal_code
         FROM provider_logical_launches l JOIN provider_launch_attempts a
           ON a.logical_launch_id=l.logical_launch_id AND a.attempt_id=l.current_attempt_id
           AND a.owner_epoch=l.owner_epoch
         JOIN invocations i ON i.id=a.invocation_id AND i.invocation_uuid=a.invocation_uuid
         WHERE i.invocation_uuid=?1 AND l.cancel_requested_at IS NULL
           AND l.terminal_code=COALESCE(i.terminal_reason,'native_completed')",
            [invocation_uuid],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row.2, status);
    assert_eq!(row.3, status);
    assert_eq!(row.4, row.5);
    assert_eq!(row.6, row.7);
    println!("terminal logical readback invocation={invocation_uuid} row={row:?}");
}
