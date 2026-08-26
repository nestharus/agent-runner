//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! TEST: proactive wake integration orchestration cases (batch delivery and wake-sweep regressions).

use crate::fake_cli::provider_script;
use crate::fixtures::Fixture;
use crate::liveness::{
    assert_dead_owner_debris_reaped, delivered_rows_without_pending_or_claim,
    delivered_single_row_without_error_or_claim, settle_wake_sweep, wait_for_file, wait_until,
};
use crate::test_guard::integration_test_guard;
use crate::validators::{
    assert_additional_notifications_remain_queued, assert_age270_invocation,
    assert_live_claim_token, assert_no_wake_claim, assert_pending_handle_with_delivery_attempts,
    assert_pending_handle_without_error, assert_pending_mailbox_count,
    assert_prompt_contains_handle, assert_prompt_file_missing, assert_success, assert_xdg_isolated,
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
        .session_runtime(session_id)
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
        "batch-${OULIPOLY_AUTO_WAKE_COUNT:-manual}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, 5);
    for index in 0..25 {
        fixture.seed_mailbox(SESSION, &format!("h-batch-{index:02}"));
    }

    let output = fixture.run_resume();
    let manual_invocation = direct_unconfirmed_invocation(&output);

    let first = wait_for_file(&fixture.prompt_file("batch-manual.txt"));
    let second = wait_for_file(&fixture.prompt_file("batch-6.txt"));
    assert_additional_notifications_remain_queued(&first);
    assert_prompt_contains_handle(&second, "h-batch-20");
    wait_until("batch rows delivered", || {
        delivered_rows_without_pending_or_claim(&fixture, SESSION, 25)
    });
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
    assert!(fixture.prompt_file("batch-manual.txt").exists());
    assert!(fixture.prompt_file("batch-6.txt").exists());
    assert!(!fixture.prompt_file("batch-7.txt").exists());
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

pub(crate) fn wake_sweep_reaps_non_resumable_abandoned_transient_session() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "non-resumable-transient-resumed.txt",
    ));
    // Idle headless runtime with a dead-owner pending row, but NO session turn /
    // chain -> no durable resume evidence. The session is never auto-woken
    // (anti-resurrection) and, being non-resumable, its undeliverable row is reaped.
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-non-resumable-transient");

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "non-resumable-transient-resumed.txt");
    assert_dead_owner_debris_reaped(&fixture, SESSION);
    assert_no_wake_claim(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}

pub(crate) fn wake_sweep_reaps_dead_owner_session_with_chain_but_no_turns() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script("", "", "chain-no-turns-resumed.txt"));
    // A registered chain segment with ZERO produced turns is an empty resume
    // target, not durable work. With a dead owner, it must be reaped (not
    // preserved as if resumable) and never auto-woken.
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
    assert_dead_owner_debris_reaped(&fixture, SESSION);
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

pub(crate) fn wake_sweep_does_not_rewake_twice_unconfirmed_pending_mailbox() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "twice-unconfirmed-not-rewoken.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    fixture.seed_mailbox(SESSION, "h-unconfirmed");
    fixture.mark_mailbox_unconfirmed_twice(SESSION, "h-unconfirmed");
    seed_dead_wake_claim(&fixture, "dddddddd-dddd-4ddd-8ddd-dddddddddddd", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);
    settle_wake_sweep();

    assert_prompt_file_missing(&fixture, "twice-unconfirmed-not-rewoken.txt");
    assert_pending_handle_with_delivery_attempts(&fixture, SESSION, "h-unconfirmed", 2);
    assert_xdg_isolated(&fixture);
}

#[cfg(target_os = "linux")]
#[test]
fn renewed_followup_claim_survives_old_failed_child_recheck() {
    use sha2::{Digest, Sha256};
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
    let count1_token = fixture.work_dir.join("count1.token");
    let count2_pid = fixture.work_dir.join("count2.pid");
    let count2_token = fixture.work_dir.join("count2.token");
    let held = fixture.work_dir.join("count2.held");
    let hook = format!(
        r#"count="${{OULIPOLY_AUTO_WAKE_COUNT:-manual}}"
token="${{OULIPOLY_AUTO_WAKE_TOKEN:-manual}}"
printf '%s|%s|%s\n' "$count" "$PPID" "$token" >> {ledger}
if [ "$count" = 1 ]; then
  printf '%s' "$PPID" > {count1_pid}
  awk '{{print $22}}' "/proc/$PPID/stat" > {count1_start}
  printf '%s' "$token" > {count1_token}
fi
if [ "$count" = 2 ]; then
  printf '%s' "$PPID" > {count2_pid}
  printf '%s' "$token" > {count2_token}
  : > {held}
  IFS= read -r _ < {fifo}
fi"#,
        ledger = shell_path(&ledger),
        count1_pid = shell_path(&count1_pid),
        count1_start = shell_path(&count1_start),
        count1_token = shell_path(&count1_token),
        count2_pid = shell_path(&count2_pid),
        count2_token = shell_path(&count2_token),
        held = shell_path(&held),
        fifo = shell_path(&fifo),
    );
    fixture.write_provider(&provider_script(
        "",
        &hook,
        "batch-${OULIPOLY_AUTO_WAKE_COUNT:-manual}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime();
    for index in 0..45 {
        fixture.seed_mailbox(SESSION, &format!("h-renewed-{index:02}"));
    }
    let output = fixture.run_resume();
    let manual_invocation = direct_unconfirmed_invocation(&output);
    wait_until("count 2 held", || {
        held.exists() && count2_pid.exists() && count2_token.exists()
    });
    let turn_id = format!("wu-d-delivery-{SESSION}-2");
    let turn_name = Sha256::digest(turn_id.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        + ".jsonl";
    assert!(
        !fixture
            .work_dir
            .join("session-turns")
            .join(turn_name)
            .exists()
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
    let old_token = std::fs::read_to_string(&count1_token).unwrap();
    let renewed_token = std::fs::read_to_string(&count2_token).unwrap();
    let renewed_pid = std::fs::read_to_string(&count2_pid)
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let claim = fixture.mailbox().wake_claim(SESSION).unwrap().unwrap();
    assert_eq!(claim.claim_token, renewed_token);
    assert_ne!(claim.claim_token, old_token);
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
    assert!(fixture.prompt_file("batch-manual.txt").exists());
    assert!(fixture.prompt_file("batch-1.txt").exists());
    assert!(fixture.prompt_file("batch-2.txt").exists());
    assert!(!fixture.prompt_file("batch-3.txt").exists());
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

pub(crate) fn persisted_count_at_five_allows_startup_sweep_delivery() {
    let _guard = integration_test_guard();
    let fixture = Fixture::new();
    fixture.write_provider(&provider_script(
        "",
        "",
        "sweep-count-${OULIPOLY_AUTO_WAKE_COUNT:-missing}.txt",
    ));
    fixture.seed_session_turn();
    fixture.seed_idle_runtime_with_wake_count(SESSION, 5);
    fixture.seed_mailbox_for(SESSION, "h-sweep-count", None);
    seed_dead_wake_claim(&fixture, "abababab-abab-4bab-8bab-abababababab", 601);

    let output = fixture.run_mailbox_list(SESSION);
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("sweep-count-6.txt"));
    assert_prompt_contains_handle(&prompt, "h-sweep-count");
    wait_until("count-five startup sweep delivery", || {
        delivered_single_row_without_error_or_claim(&fixture, SESSION)
    });
    let runtime = fixture.mailbox().session_runtime(SESSION).unwrap().unwrap();
    assert_eq!(runtime.auto_wake_count, 6);
    assert_one_failed_delivery(&fixture, SESSION);
    assert_xdg_isolated(&fixture);
}
