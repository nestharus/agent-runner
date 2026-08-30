//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (batch delivery and wake-sweep regressions).

use crate::fake_cli::provider_script;
use crate::fixtures::Fixture;
use crate::liveness::{
    assert_dead_owner_debris_retained, delivered_rows_without_pending_or_claim,
    delivered_single_row_without_error_or_claim, settle_wake_sweep, wait_for_file, wait_until,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_additional_notifications_remain_queued, assert_age270_invocation,
    assert_live_claim_token, assert_no_wake_claim, assert_pending_handle_without_error,
    assert_pending_mailbox_count, assert_prompt_contains_handle, assert_prompt_file_missing,
    assert_success, assert_xdg_isolated,
};
use crate::wake_claim_setup::{
    acquire_seed_wake_claim, seed_dead_wake_claim, seed_live_wake_claim,
};
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
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-sweep-reclaim");
    seed_dead_wake_claim(&fixture, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("sweep-reclaimed.txt"));
    assert_prompt_contains_handle(&prompt, "h-sweep-reclaim");
    wait_until("sweep reclaimed dead claim and delivered mailbox", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    assert_one_failed_delivery(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_resurrect_abandoned_transient_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "abandoned-transient-resumed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-abandoned-transient");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "abandoned-transient-resumed.txt");
    assert_pending_handle_without_error(&fixture, SESSION, "h-abandoned-transient");
    assert_no_wake_claim(&fixture, SESSION);
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
    // Idle headless runtime with a dead-owner pending row, but NO session turn /
    // chain -> no durable resume evidence. The session is never auto-woken
    // (anti-resurrection), but automatic terminal reap is withheld because the
    // sweep cannot fence State and mailbox authority atomically.
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-non-resumable-transient");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "non-resumable-transient-resumed.txt");
    assert_dead_owner_debris_retained(&fixture, SESSION);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_retains_dead_owner_session_with_chain_but_no_turns() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "chain-no-turns-resumed.txt"));
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

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "chain-no-turns-resumed.txt");
    assert_dead_owner_debris_retained(&fixture, SESSION);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_delivers_resumable_session_missing_models_dir() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "missing-models-dir-resumed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_without_models_dir(SESSION);
    fixture.seed_mailbox_for(SESSION, "h-missing-models-dir", None);
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("missing-models-dir-resumed.txt"));
    assert_prompt_contains_handle(&prompt, "h-missing-models-dir");
    wait_until("missing models_dir wake delivered", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    assert_one_failed_delivery(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_disturb_live_identity_matched_claim() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "live-claim-not-disturbed.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-live-claim");
    seed_live_wake_claim(&fixture, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "live-claim-not-disturbed.txt");
    assert_live_claim_token(&fixture, SESSION, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_does_not_rewake_consumed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "consumed-not-rewoken.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-consumed");
    fixture.seed_consumed_notification_turn("h-consumed");
    seed_dead_wake_claim(&fixture, "cccccccc-cccc-4ccc-8ccc-cccccccccccc", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "consumed-not-rewoken.txt");
    assert_pending_mailbox_count(&fixture, SESSION, 1);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_retries_twice_unconfirmed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "twice-unconfirmed-retried.txt"));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-unconfirmed");
    fixture.mark_mailbox_unconfirmed_twice(SESSION, "h-unconfirmed");
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_mailbox_list(SESSION);
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
    assert_xdg_isolated(&fixture);
}

pub(crate) fn failed_auto_wake_retains_retry_ownership_during_backoff() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let first_failure = fixture.work_dir.join("first-auto-wake-failed");
    let hook = format!(
        r#"if [ "$WU_D_PROVIDER_RESUME_INDEX" = 2 ]; then
  : > {}
  exit 17
fi"#,
        shell_path(&first_failure),
    );
    fixture.write_provider(&provider_script(
        "",
        &hook,
        "retry-owner-${WU_D_PROVIDER_RESUME_INDEX}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    for index in 0..21 {
        fixture.seed_mailbox(SESSION, &format!("h-retry-owner-{index:02}"));
    }

    let manual_resume = fixture.run_resume_with_retry_base(2_000);
    direct_unconfirmed_invocation(&manual_resume);
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

    let overlapping_sweep = fixture.run_mailbox_list(SESSION);
    assert_success(&overlapping_sweep);
    std::thread::sleep(std::time::Duration::from_millis(250));
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "2",
        "a startup sweep must coalesce with the retry owner"
    );

    wait_until("owned retry renewed and delivered pending mailbox", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 21)
    });
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "3"
    );
    assert_xdg_isolated(&fixture);
}

pub(crate) fn maximum_chronology_stays_eligible_across_failed_and_terminal_rechecks() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let first_failure = fixture.work_dir.join("maximum-chronology-first-failure");
    let hook = format!(
        r#"if [ "$WU_D_PROVIDER_RESUME_INDEX" = 1 ]; then
  : > {}
  exit 17
fi"#,
        shell_path(&first_failure),
    );
    fixture.write_provider(&provider_script(
        "",
        &hook,
        "maximum-chronology-${WU_D_PROVIDER_RESUME_INDEX}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, i64::MAX);
    for index in 0..21 {
        fixture.seed_mailbox(SESSION, &format!("h-maximum-chronology-{index:02}"));
    }
    let claim_token = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
    acquire_seed_wake_claim(&fixture, claim_token);
    fixture
        .sidecar_conn()
        .execute(
            "UPDATE session_wake_claim SET auto_wake_count = ?2 WHERE session_id = ?1",
            rusqlite::params![SESSION, i64::MAX],
        )
        .unwrap();

    let first = fixture.run_auto_wake_resume(claim_token, i64::MAX, 1);
    assert!(
        first_failure.exists(),
        "maximum-count failure path was not reached"
    );
    assert!(
        !String::from_utf8_lossy(&first.stderr).contains("attempt to add with overflow"),
        "maximum chronology overflowed before retry renewal: {first:?}"
    );

    wait_until(
        "maximum chronology retry delivered all pending rows",
        || delivered_rows_without_pending_or_claim(&fixture, SESSION, 21),
    );
    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 21);
    assert!(rows[..20].iter().all(|row| row.delivery_attempts == 2));
    assert_eq!(rows[20].delivery_attempts, 1);
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    assert_eq!(runtime.auto_wake_count, i64::MAX);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn repeated_failed_wakes_keep_oldest_batch_owned_past_terminal_budget() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    let attempt_ledger = fixture.work_dir.join("persistent-failure-attempts.txt");
    let fourth_started = fixture.work_dir.join("persistent-failure-fourth-started");
    let release_fourth = fixture.work_dir.join("persistent-failure-release-fourth");
    let hook = format!(
        r#"python3 - {attempt_ledger} "$WU_D_PROVIDER_RESUME_INDEX" <<'PY'
import sys
import time

with open(sys.argv[1], "a", encoding="utf-8") as out:
    out.write(f"{{sys.argv[2]}} {{time.monotonic_ns()}}\n")
PY
if [ "$WU_D_PROVIDER_RESUME_INDEX" -le 3 ]; then
  exit 17
fi
if [ "$WU_D_PROVIDER_RESUME_INDEX" = 4 ]; then
  : > {fourth_started}
  while [ ! -e {release_fourth} ]; do sleep 0.01; done
fi"#,
        attempt_ledger = shell_path(&attempt_ledger),
        fourth_started = shell_path(&fourth_started),
        release_fourth = shell_path(&release_fourth),
    );
    fixture.write_provider(&provider_script(
        "",
        &hook,
        "persistent-failure-${WU_D_PROVIDER_RESUME_INDEX}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, i64::MAX);
    for index in 0..21 {
        fixture.seed_mailbox(SESSION, &format!("h-persistent-failure-{index:02}"));
    }
    let claim_token = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    acquire_seed_wake_claim(&fixture, claim_token);
    fixture
        .sidecar_conn()
        .execute(
            "UPDATE session_wake_claim SET auto_wake_count = ?2 WHERE session_id = ?1",
            rusqlite::params![SESSION, i64::MAX],
        )
        .unwrap();

    let first = fixture.run_auto_wake_resume(claim_token, i64::MAX, 1);
    assert_eq!(first.status.code(), Some(17), "{first:?}");
    wait_for_file(&fourth_started);

    let retained_claim = fixture
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .expect("three consecutive failures must retain one claim for a fourth attempt");
    assert_ne!(retained_claim.claim_token, claim_token);
    assert_eq!(retained_claim.auto_wake_count, i64::MAX);
    assert!(retained_claim.wake_pid.is_some());
    let pending = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(pending[..20].iter().all(|row| {
        row.delivery_attempts == 3 && row.delivery_error.as_deref() == Some("exit_nonzero")
    }));
    assert_eq!(pending[20].delivery_attempts, 0);
    assert!(pending[20].delivery_error.is_none());
    assert!(pending.iter().all(|row| row.delivered_at.is_none()));

    std::fs::write(&release_fourth, "release\n").unwrap();
    wait_until(
        "persistent failure lifecycle delivered oldest and newer work",
        || delivered_rows_without_pending_or_claim(&fixture, SESSION, 21),
    );
    for index in 1..=4 {
        let prompt =
            wait_for_file(&fixture.prompt_file(&format!("persistent-failure-{index}.txt")));
        assert_prompt_contains_handle(&prompt, "h-persistent-failure-00");
        assert_prompt_contains_handle(&prompt, "h-persistent-failure-19");
        assert!(!prompt.contains("h-persistent-failure-20"), "{prompt}");
    }
    let newer = wait_for_file(&fixture.prompt_file("persistent-failure-5.txt"));
    assert_prompt_contains_handle(&newer, "h-persistent-failure-20");

    let rows = fixture.mailbox().list_mailbox(SESSION, true).unwrap();
    assert!(rows[..20].iter().all(|row| row.delivery_attempts == 4));
    assert_eq!(rows[20].delivery_attempts, 1);
    assert_eq!(
        std::fs::read_to_string(fixture.work_dir.join("provider-resume-sequence.txt")).unwrap(),
        "5"
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
        [1, 2, 3, 4, 5]
    );
    for pair in attempts[..4].windows(2) {
        let elapsed_ms = (pair[1].1 - pair[0].1) / 1_000_000;
        assert!(
            (900..=10_000).contains(&elapsed_ms),
            "maximum chronology retry escaped its bounded cadence: {attempts:?}"
        );
    }
    assert_xdg_isolated(&fixture);
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
printf '%s|%s\n' "$label" "$PPID" >> {ledger}
if [ "$index" = 2 ]; then
  printf '%s' "$PPID" > {count1_pid}
  awk '{{print $22}}' "/proc/$PPID/stat" > {count1_start}
fi
if [ "$index" = 3 ]; then
  printf '%s' "$PPID" > {count2_pid}
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
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, i64::MAX);
    fixture.seed_mailbox_for(SESSION, "h-sweep-count", None);
    let stale_token = "abababab-abab-4bab-8bab-abababababab";
    seed_dead_wake_claim(&fixture, stale_token, 601);

    let output = fixture.run_mailbox_list(SESSION);
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
    assert_xdg_isolated(&fixture);
}
