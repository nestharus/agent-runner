#![cfg(unix)]

mod fixtures;

use agent_runner_lib::session_replace::{
    CanonicalRecord, CanonicalToProviderRenderer, ClaudeCodeRenderer, CodexSessionRenderer,
    ReplaceError, ReplaceReceipt, run_import_replace,
};
use fixtures::initiative_06_import_replace::*;
use rusqlite::Connection;
use std::fs;

/// Risk: T1 — valid Claude stdin replacement may write canonical bytes instead of provider-native bytes.
/// Level: CLI integration.
/// Source: contract §7 T-valid-replace; proposal §9.1 Valid stdin replace; A1, A3, A5.
/// Observable: exit 0; receipt fields are populated; transcript is Claude-native; export semantics match imported canonical records.
/// Residual: does not exhaust every Claude content variant.
#[test]
fn t1_valid_replace_claude_stdin_emits_receipt_and_provider_native_transcript() {
    assert_public_session_replace_contract_types_are_reachable();
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "valid",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    let receipt = assert_success(&output);
    assert_receipt_shape(
        &receipt,
        &prepared.session_id,
        &prepared.provider_name,
        "claude_code",
        &prepared.jsonl_path,
    );
    let transcript = String::from_utf8(fs::read(&prepared.jsonl_path).unwrap()).unwrap();
    assert!(transcript.contains("\"sessionId\""), "{transcript}");
    assert!(
        transcript.contains("\"uuid\":\"valid-turn-1\""),
        "{transcript}"
    );
    assert!(!transcript.contains("\"provider_name\""), "{transcript}");
    assert!(
        !transcript.contains("\"unsupported_record\""),
        "{transcript}"
    );
    assert_receipt_postimage_matches_export(&prepared.fixture, &prepared.session_id, &receipt);
    assert_export_semantics_match_canonical(&prepared.fixture, &prepared.session_id, &input);
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

/// Risk: T2 — Codex rendering may be accidentally treated as unsupported or written in Claude shape.
/// Level: CLI integration.
/// Source: contract §7 T-codex-replace; proposal §9.1 Postimage round-trip; A3, A5.
/// Observable: exit 0; receipt storage_type is codex_session; transcript contains Codex rollout records.
/// Residual: does not cover Codex compaction records.
#[test]
fn t2_codex_replace_writes_codex_rollout_jsonl() {
    let prepared = prepared_codex_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "codex",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    let receipt = assert_success(&output);
    assert_receipt_shape(
        &receipt,
        &prepared.session_id,
        &prepared.provider_name,
        "codex_session",
        &prepared.jsonl_path,
    );
    let transcript = String::from_utf8(fs::read(&prepared.jsonl_path).unwrap()).unwrap();
    assert!(
        transcript.contains("\"type\":\"response_item\""),
        "{transcript}"
    );
    assert!(transcript.contains("codex assistant"), "{transcript}");
    assert!(!transcript.contains("\"provider_name\""), "{transcript}");
    assert_receipt_postimage_matches_export(&prepared.fixture, &prepared.session_id, &receipt);
    assert_export_semantics_match_canonical(&prepared.fixture, &prepared.session_id, &input);
}

/// Risk: T3 — --from-file may bypass the stdin validator or use a different mutation path.
/// Level: CLI integration.
/// Source: contract §7 T-from-file; proposal §9.1 Valid --from-file replace; A9.
/// Observable: exit 0; receipt shape and final transcript semantics match file input.
/// Residual: does not cover unreadable file permissions.
#[test]
fn t3_from_file_is_equivalent_to_stdin_after_bytes_are_loaded() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "from-file",
    );
    let input_path = prepared
        .fixture
        .stage_jsonl("packed-canonical.jsonl", &input);

    let output =
        prepared
            .fixture
            .run_import_replace_from_file(&prepared.session_id, &input_path, &[]);

    let receipt = assert_success(&output);
    assert_receipt_shape(
        &receipt,
        &prepared.session_id,
        &prepared.provider_name,
        "claude_code",
        &prepared.jsonl_path,
    );
    assert_export_semantics_match_canonical(&prepared.fixture, &prepared.session_id, &input);
}

#[test]
fn t_unrelated_session_unchanged_after_replace() {
    let fixture = ImportReplaceFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let workspace_root = fixture.root().join("workspace");
    let path_a = fixture.stage_claude_jsonl(
        &projects_dir,
        &workspace_root,
        SESSION_A,
        &format!(
            "{}\n{}\n",
            claude_seed_line(SESSION_A, "old-turn-1", "user", "old user", 0),
            claude_seed_line(SESSION_A, "old-turn-2", "assistant", "old assistant", 1)
        ),
    );
    let path_b = fixture.stage_claude_jsonl(
        &projects_dir,
        &workspace_root,
        SESSION_B,
        &format!(
            "{}\n{}\n",
            claude_seed_line(SESSION_B, "b-turn-1", "user", "b user", 0),
            claude_seed_line(SESSION_B, "b-turn-2", "assistant", "b assistant", 1)
        ),
    );
    fixture.write_model(MODEL, &[CLAUDE_PROVIDER]);
    fixture.write_provider(
        CLAUDE_PROVIDER,
        StorageKind::ClaudeCode {
            projects_dir: &projects_dir,
        },
    );
    fixture.write_sessions_with_locator_body(
        CLAUDE_PROVIDER,
        &format!(
            "case \"$SESSION_ID\" in\n  \"{SESSION_A}\") printf '%s\\n' {:?} ;;\n  \"{SESSION_B}\") printf '%s\\n' {:?} ;;\n  *) exit 1 ;;\nesac",
            path_a.to_string_lossy(),
            path_b.to_string_lossy()
        ),
    );
    fixture.seed_active_chain(
        CHAIN_A,
        CLAUDE_PROVIDER,
        SESSION_A,
        MODEL,
        "2026-04-17T08:00:00Z",
    );
    fixture.seed_active_chain(
        CHAIN_B,
        CLAUDE_PROVIDER,
        SESSION_B,
        MODEL,
        "2026-04-17T08:05:00Z",
    );
    fixture.seed_turns_with_metadata(CLAUDE_PROVIDER, SESSION_A, &path_a);
    fixture.seed_turns_with_metadata(CLAUDE_PROVIDER, SESSION_B, &path_b);
    let before_b = fixture.turn_rows(CLAUDE_PROVIDER, SESSION_B);
    let before_segment_id = fixture.active_segment_id(CHAIN_A);
    let before_last_used = fixture.chain_last_used_at(CHAIN_A);
    let input = canonical_jsonl(SESSION_A, CLAUDE_PROVIDER, &path_a, "unrelated");

    let output = fixture.run_import_replace(SESSION_A, &input, &[]);

    assert_success(&output);
    assert_eq!(fixture.turn_rows(CLAUDE_PROVIDER, SESSION_B), before_b);
    assert_eq!(fixture.active_segment_id(CHAIN_A), before_segment_id);
    assert_ne!(fixture.chain_last_used_at(CHAIN_A), before_last_used);
    assert_eq!(fixture.chain_last_used_at(CHAIN_A), "2026-04-17T09:00:01Z");
    assert_eq!(
        fixture.segment_state(CHAIN_A)["last_turn_id"],
        "unrelated-turn-2"
    );
}

/// Risk: T4 — preimage protection may compare the wrong hash domain or run outside the lock.
/// Level: CLI integration.
/// Source: contract §7 T-preimage-match; proposal §5 and §6 hash details; A4.
/// Observable: current canonical export hash succeeds when supplied through --preimage-sha256.
/// Residual: does not prove TOCTOU protection against non-cooperating external writers.
#[test]
fn t4_preimage_match_succeeds_with_current_canonical_export_hash() {
    let prepared = prepared_claude_replace_fixture();
    let before_export = prepared.fixture.run_export(&prepared.session_id);
    assert_eq!(before_export.status.code(), Some(0), "{before_export:?}");
    let preimage = sha256sum_bytes(&before_export.stdout);
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "preimage-match",
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &["--preimage-sha256", preimage.as_str()],
    );

    let receipt = assert_success(&output);
    assert_eq!(receipt["preimage_sha256"], preimage);
}

/// Risk: T5 — preimage mismatch may mutate the transcript before refusing stale input.
/// Level: CLI integration.
/// Source: contract §7 T-preimage-mismatch; proposal §5 exit 15; A4, A8.
/// Observable: exit 15 preimage-mismatch; transcript and DB rows are unchanged; recovery artifacts remain.
/// Residual: journal content schema is checked by recovery tests, not fully decoded here.
#[test]
fn t5_preimage_mismatch_exits_15_without_transcript_or_db_mutation() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "preimage-mismatch",
    );
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &[
            "--preimage-sha256",
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        ],
    );

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    assert_json_error(&output, "preimage-mismatch");
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
    assert!(
        prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists()
    );
    assert!(
        prepared
            .fixture
            .canonical_records_path(&prepared.session_id)
            .exists()
    );
}

/// Risk: T6 — import-replace may ignore an existing pause-handshake-compatible lease.
/// Level: CLI integration.
/// Source: contract §7 T-busy; proposal §8 Lock behavior; A6.
/// Observable: exit 13 session-busy; only staging scratch is cleaned and no per-session journal is published.
/// Residual: does not prove every non-cooperating provider process is detected.
#[test]
fn t6_busy_lock_exits_13_and_does_not_publish_session_journal() {
    let prepared = prepared_claude_replace_fixture();
    prepared
        .fixture
        .write_active_lock(&prepared.provider_name, &prepared.session_id);
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "busy",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(13), "{output:?}");
    assert_json_error(&output, "session-busy");
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

/// Risk: T7 — a crash after transcript rename but before DB commit may leave derived state stale forever.
/// Level: subprocess crash-recovery integration.
/// Source: contract §7 T-recovery-rename-only; proposal §6 Startup recovery; A8.
/// Observable: SIGKILL after the after-rename test hook; the next import-replace command first rebuilds session_turns from canonical_records_path and deletes journal files.
/// Residual: relies on the Step 6c test hook documented in the fixture rather than a real power loss.
#[test]
fn t7_recovery_rename_only_rebuilds_db_from_journal_attached_canonical_records() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "recover",
    );
    let child = prepared.fixture.spawn_import_replace(
        &prepared.session_id,
        &input,
        &[],
        &[(TEST_HOOK_ENV, TEST_BLOCK_AFTER_RENAME)],
    );

    let killed = kill_after_test_hook(child, TEST_BLOCK_AFTER_RENAME);
    assert_ne!(killed.status.code(), Some(0), "{killed:?}");
    assert!(
        prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists()
    );
    assert_eq!(
        fs::read_to_string(
            prepared
                .fixture
                .canonical_records_path(&prepared.session_id)
        )
        .unwrap(),
        input
    );

    let recovery_trigger =
        prepared
            .fixture
            .run_import_replace(&prepared.session_id, "{not-json", &[]);
    assert_eq!(
        recovery_trigger.status.code(),
        Some(15),
        "{recovery_trigger:?}"
    );
    assert_json_error(&recovery_trigger, "invalid-input-transcript");
    let rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    assert_eq!(
        rows.iter()
            .map(|row| row.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["recover-turn-1", "recover-turn-2"]
    );
    assert_eq!(
        prepared.fixture.segment_state(&prepared.chain_id)["last_turn_id"],
        "recover-turn-2"
    );
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

/// Risk: T8 — recovery may silently accept an ambiguous transcript hash after a crash.
/// Level: subprocess crash-recovery integration.
/// Source: contract §7 T-recovery-ambiguous-hash; proposal §6 Startup recovery ambiguous case; A8.
/// Observable: after SIGKILL and manual transcript corruption, startup moves journal to quarantine and leaves DB unchanged.
/// Residual: log wording is not asserted beyond filesystem quarantine.
#[test]
fn t8_recovery_ambiguous_hash_quarantines_journal_and_leaves_db_unchanged() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "ambiguous",
    );
    let before_rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    let child = prepared.fixture.spawn_import_replace(
        &prepared.session_id,
        &input,
        &[],
        &[(TEST_HOOK_ENV, TEST_BLOCK_AFTER_RENAME)],
    );
    let _ = kill_after_test_hook(child, TEST_BLOCK_AFTER_RENAME);
    fs::write(&prepared.jsonl_path, b"{\"corrupt\":\"manual edit\"}\n").unwrap();

    let _ = prepared.fixture.run_recovery_trigger();

    assert!(
        !prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists()
    );
    let quarantine_files = fs::read_dir(prepared.fixture.quarantine_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert!(
        quarantine_files.iter().any(|path| path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(&prepared.session_id)),
        "{quarantine_files:?}"
    );
    assert!(
        prepared
            .fixture
            .canonical_records_path(&prepared.session_id)
            .exists()
    );
    let after_rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    assert_eq!(after_rows, before_rows);
}

#[test]
fn recovery_deletes_orphan_canonical_records_without_pending_journal() {
    let prepared = prepared_claude_replace_fixture();
    let before_rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    let orphan = prepared
        .fixture
        .canonical_records_path(&prepared.session_id);
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(
        &orphan,
        canonical_jsonl(
            &prepared.session_id,
            &prepared.provider_name,
            &prepared.jsonl_path,
            "orphan",
        ),
    )
    .unwrap();

    let recovery_trigger = prepared.fixture.run_recovery_trigger();

    assert_eq!(
        recovery_trigger.status.code(),
        Some(0),
        "{recovery_trigger:?}"
    );
    assert!(!orphan.exists());
    assert_eq!(
        prepared
            .fixture
            .turn_rows(&prepared.provider_name, &prepared.session_id),
        before_rows
    );
}

#[test]
fn recovery_keeps_orphan_canonical_records_while_session_lock_is_live() {
    let prepared = prepared_claude_replace_fixture();
    let before_rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    let orphan = prepared
        .fixture
        .canonical_records_path(&prepared.session_id);
    fs::create_dir_all(orphan.parent().unwrap()).unwrap();
    fs::write(
        &orphan,
        canonical_jsonl(
            &prepared.session_id,
            &prepared.provider_name,
            &prepared.jsonl_path,
            "live-orphan",
        ),
    )
    .unwrap();
    prepared
        .fixture
        .write_active_lock(&prepared.provider_name, &prepared.session_id);

    let recovery_trigger = prepared.fixture.run_recovery_trigger();

    assert_eq!(
        recovery_trigger.status.code(),
        Some(0),
        "{recovery_trigger:?}"
    );
    assert!(orphan.exists());
    fs::remove_file(
        prepared
            .fixture
            .lock_path(&prepared.provider_name, &prepared.session_id),
    )
    .unwrap();

    let recovery_trigger = prepared.fixture.run_recovery_trigger();

    assert_eq!(
        recovery_trigger.status.code(),
        Some(0),
        "{recovery_trigger:?}"
    );
    assert!(!orphan.exists());
    assert_eq!(
        prepared
            .fixture
            .turn_rows(&prepared.provider_name, &prepared.session_id),
        before_rows
    );
}

/// Risk: T9 — two import-replace processes may both pass the lock gate and publish colliding journal files.
/// Level: subprocess concurrency integration.
/// Source: contract §7 T-concurrent-import-replace; proposal §9.1 concurrent row; A6, A8.
/// Observable: exactly one subprocess exits 0 and the other exits 13; final transcript/export matches the winner; no journal pollution remains.
/// Residual: uses a lock-hold test hook to make scheduler timing deterministic.
#[test]
fn t9_concurrent_import_replace_allows_exactly_one_winner() {
    let prepared = prepared_claude_replace_fixture();
    let input_a = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "winner-a",
    );
    let input_b = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "winner-b",
    );
    let mut first = prepared.fixture.spawn_import_replace(
        &prepared.session_id,
        &input_a,
        &[],
        &[(TEST_HOOK_ENV, TEST_SLEEP_AFTER_LOCK_MS)],
    );
    wait_for_test_hook_line(&mut first, TEST_SLEEP_AFTER_LOCK_MS);
    let second = prepared
        .fixture
        .spawn_import_replace(&prepared.session_id, &input_b, &[], &[]);

    let first_output = first.wait_with_output().unwrap();
    let second_output = second.wait_with_output().unwrap();
    let outputs = vec![first_output, second_output];
    assert_eq!(
        outputs
            .iter()
            .filter(|output| output.status.code() == Some(0))
            .count(),
        1,
        "{outputs:?}"
    );
    assert_eq!(
        outputs
            .iter()
            .filter(|output| output.status.code() == Some(13))
            .count(),
        1,
        "{outputs:?}"
    );
    for output in outputs
        .iter()
        .filter(|output| output.status.code() == Some(13))
    {
        assert_json_error(output, "session-busy");
    }
    let success = outputs
        .iter()
        .find(|output| output.status.code() == Some(0))
        .unwrap();
    assert_success_allowing_test_hook_stderr(success);
    assert_export_semantics_match_canonical(&prepared.fixture, &prepared.session_id, &input_a);
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

/// Risk: T10 — handled input errors may partially mutate transcript or state.
/// Level: CLI integration.
/// Source: contract §7 T-readonly-on-error; proposal §5 exit guarantees; A3, A4.
/// Observable: preimage mismatch leaves transcript bytes and session_turns exactly unchanged.
/// Residual: post-rename operational errors are covered by T11 and recovery tests.
#[test]
fn t10_readonly_on_error_keeps_transcript_and_db_unchanged() {
    let prepared = prepared_claude_replace_fixture();
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "readonly-error",
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &[
            "--preimage-sha256",
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
        ],
    );

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
}

/// Risk: T11 — verification failures may delete the only durable recovery signal.
/// Level: CLI integration with fault injection.
/// Source: contract §7 T-no-deletion-before-verify; proposal §4 success flow steps 10-11; A8.
/// Observable: injected postimage verification failure exits 1 and leaves pending journal plus canonical_records_path.
/// Residual: exact stderr message is intentionally less stable than the exit code and artifact contract.
#[test]
fn t11_no_deletion_before_verify_leaves_recovery_artifacts_on_postimage_failure() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "verify-fail",
    );

    let output = prepared
        .fixture
        .spawn_import_replace(
            &prepared.session_id,
            &input,
            &[],
            &[(TEST_HOOK_ENV, TEST_FAIL_POSTIMAGE_VERIFY)],
        )
        .wait_with_output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_json_error(&output, "operational-error");
    assert!(
        prepared
            .fixture
            .pending_journal_path(&prepared.session_id)
            .exists()
    );
    assert!(
        prepared
            .fixture
            .canonical_records_path(&prepared.session_id)
            .exists()
    );
    let rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    assert_eq!(
        rows.iter()
            .map(|row| row.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["old-turn-1", "old-turn-2"]
    );
}

/// Risk: T12 — provider storage type `other` may be mutated with a guessed renderer.
/// Level: CLI integration.
/// Source: contract §7 T-unsupported-storage-other; proposal §3 renderer contract; A5.
/// Observable: exit 12 unsupported-storage; no transcript, DB, lock, or journal mutation.
/// Residual: future renderer support should deliberately change this expectation.
#[test]
fn t12_unsupported_storage_other_exits_12_before_mutation() {
    let prepared = prepared_other_storage_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "other",
    );
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(12), "{output:?}");
    assert_json_error(&output, "unsupported-storage");
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
}

/// Risk: T13 — malformed canonical JSONL may be accepted or reported without line context.
/// Level: CLI integration.
/// Source: contract §7 T-malformed-input-record; proposal §3 validation rules; A3.
/// Observable: exit 15 invalid-input-transcript; stderr carries line number when the bad record is line-local.
/// Residual: exact validation message text is not asserted.
#[test]
fn t13_malformed_input_record_exits_15_with_line_number() {
    let prepared = prepared_claude_replace_fixture();

    let output =
        prepared
            .fixture
            .run_import_replace(&prepared.session_id, malformed_canonical_jsonl(), &[]);

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let json = assert_json_error(&output, "invalid-input-transcript");
    assert!(
        json["error"]["line"].as_u64() == Some(1) || json["line"].as_u64() == Some(1),
        "{json}"
    );
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

#[test]
fn invalid_timestamp_record_exits_15_before_mutation() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "bad-timestamp",
    )
    .replacen("2026-04-17T09:00:00Z", "not-a-timestamp", 1);

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let json = assert_json_error(&output, "invalid-input-transcript");
    assert!(
        json["error"]["line"].as_u64() == Some(1) || json["line"].as_u64() == Some(1),
        "{json}"
    );
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

#[test]
fn t_session_id_mismatch_in_input() {
    let prepared = prepared_claude_replace_fixture();
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    let input = canonical_jsonl(
        SESSION_B,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "session-mismatch",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let json = assert_json_error(&output, "invalid-input-transcript");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("session/provider"),
        "{json}"
    );
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
    assert_no_replace_journal_pollution(&prepared.fixture, SESSION_B);
}

#[test]
fn t_provider_name_mismatch_in_input() {
    let prepared = prepared_claude_replace_fixture();
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    let input = canonical_jsonl(
        &prepared.session_id,
        CODEX_PROVIDER,
        &prepared.jsonl_path,
        "provider-mismatch",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let json = assert_json_error(&output, "invalid-input-transcript");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("session/provider"),
        "{json}"
    );
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

#[test]
fn t_unsupported_record_class() {
    let prepared = prepared_claude_replace_fixture();
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    let input = unsupported_record_only_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let json = assert_json_error(&output, "invalid-input-transcript");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unsupported record class"),
        "{json}"
    );
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

#[test]
fn t_schema_incompatible_exit_14() {
    let fixture = ImportReplaceFixture::new();
    fs::create_dir_all(fixture.db_path().parent().unwrap()).unwrap();
    Connection::open(fixture.db_path()).unwrap();
    let before_db = fs::read(fixture.db_path()).unwrap();
    let input = canonical_jsonl(
        SESSION_A,
        CLAUDE_PROVIDER,
        &fixture.root().join("schema-incompatible.jsonl"),
        "schema",
    );

    let output = fixture.run_import_replace(SESSION_A, &input, &[]);

    assert_eq!(output.status.code(), Some(14), "{output:?}");
    assert_json_error(&output, "schema-incompatible");
    assert_eq!(fs::read(fixture.db_path()).unwrap(), before_db);
    assert!(!fixture.replace_journal_dir().exists());
}

#[test]
fn t_empty_input_exits_15_before_mutation() {
    assert_invalid_input_has_no_mutation(|_| Vec::new());
}

#[test]
fn t_blank_line_input_exits_15_before_mutation() {
    let json = assert_invalid_input_has_no_mutation(|prepared| {
        canonical_jsonl(
            &prepared.session_id,
            &prepared.provider_name,
            &prepared.jsonl_path,
            "blank-line",
        )
        .replacen('\n', "\n\n", 1)
        .into_bytes()
    });
    assert_eq!(json["error"]["line"].as_u64(), Some(2), "{json}");
}

#[test]
fn t_missing_required_canonical_field_exits_15_before_mutation() {
    assert_invalid_input_has_no_mutation(|prepared| {
        let input = canonical_jsonl(
            &prepared.session_id,
            &prepared.provider_name,
            &prepared.jsonl_path,
            "missing-field",
        );
        let mut records = normalize_jsonl(&input);
        records[0].as_object_mut().unwrap().remove("turn_id");
        (records
            .into_iter()
            .map(|record| serde_json::to_string(&record).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n")
            .into_bytes()
    });
}

#[test]
fn t_non_utf8_stdin_exits_15_before_mutation() {
    let json = assert_invalid_input_has_no_mutation(|_| vec![0xff, 0xfe, b'\n']);
    assert!(
        json["error"]["message"].as_str().unwrap().contains("utf-8"),
        "{json}"
    );
}

#[test]
fn malformed_preimage_sha_exits_2_as_invalid_argument() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "bad-preimage-arg",
    );

    let output = prepared.fixture.run_import_replace(
        &prepared.session_id,
        &input,
        &["--preimage-sha256", "abc"],
    );

    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_json_error(&output, "invalid-argument");
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
}

/// Risk: T14 — import may preserve or invent fields absent from v1 CanonicalRecord.
/// Level: DB integration.
/// Source: contract §7 T-field-loss-explicit; proposal §7 DB consistency update; A7.
/// Observable: imported rows have canonical fields populated and parent_turn_id NULL, sidechain 0, compaction 0.
/// Residual: future canonical schema extensions may intentionally preserve these fields.
#[test]
fn t14_field_loss_is_explicit_for_reinserted_session_turns() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "loss",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);
    assert_success(&output);

    let rows = prepared
        .fixture
        .turn_rows(&prepared.provider_name, &prepared.session_id);
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in rows {
        assert!(row.turn_id.starts_with("loss-turn-"), "{row:?}");
        assert_eq!(row.parent_turn_id, None, "{row:?}");
        assert_eq!(row.is_sidechain, 0, "{row:?}");
        assert_eq!(row.is_compaction_boundary, 0, "{row:?}");
    }
}

/// Risk: T15 — resolver errors may collapse into generic operational failures.
/// Level: CLI integration.
/// Source: contract §7 T-resolver-error-mapping; proposal §5 exit codes; A2, A5.
/// Observable: not-found maps to 10, ambiguous maps to 11, unsupported storage maps to 12.
/// Residual: model-resolution subcodes are not fully enumerated here.
#[test]
fn t15_resolver_error_mapping_covers_10_11_and_12() {
    let missing = missing_uuid_fixture();
    let missing_input = canonical_jsonl(
        SESSION_B,
        CLAUDE_PROVIDER,
        &missing.root().join("missing.jsonl"),
        "missing",
    );
    let missing_output = missing.run_import_replace(SESSION_B, &missing_input, &[]);
    assert_eq!(missing_output.status.code(), Some(10), "{missing_output:?}");
    assert_json_error(&missing_output, "session-not-found");

    let ambiguous = ambiguous_session_fixture();
    let ambiguous_input = canonical_jsonl(
        SESSION_A,
        CLAUDE_PROVIDER,
        &ambiguous.root().join("ambiguous.jsonl"),
        "ambiguous",
    );
    let ambiguous_output = ambiguous.run_import_replace(SESSION_A, &ambiguous_input, &[]);
    assert_eq!(
        ambiguous_output.status.code(),
        Some(11),
        "{ambiguous_output:?}"
    );
    assert_json_error(&ambiguous_output, "ambiguous-session");

    let unsupported = prepared_other_storage_fixture();
    let unsupported_input = canonical_jsonl(
        &unsupported.session_id,
        &unsupported.provider_name,
        &unsupported.jsonl_path,
        "unsupported",
    );
    let unsupported_output =
        unsupported
            .fixture
            .run_import_replace(&unsupported.session_id, &unsupported_input, &[]);
    assert_eq!(
        unsupported_output.status.code(),
        Some(12),
        "{unsupported_output:?}"
    );
    assert_json_error(&unsupported_output, "unsupported-storage");
}

/// Risk: T16 — a well-formed UUID with no chain may accidentally initialize or mutate session state.
/// Level: CLI integration.
/// Source: contract §7 T-session-not-found; proposal §5 exit 10; A2.
/// Observable: exit 10 session-not-found and no replace_journal directory is created.
/// Residual: invalid UUID handling is covered by sibling locate/pause tests.
#[test]
fn t16_session_not_found_exits_10_for_well_formed_uuid_with_no_chain() {
    let fixture = missing_uuid_fixture();
    let input = canonical_jsonl(
        SESSION_B,
        CLAUDE_PROVIDER,
        &fixture.root().join("missing.jsonl"),
        "missing",
    );

    let output = fixture.run_import_replace(SESSION_B, &input, &[]);

    assert_eq!(output.status.code(), Some(10), "{output:?}");
    assert_json_error(&output, "session-not-found");
    assert!(
        !fixture.pending_journal_path(SESSION_B).exists(),
        "not-found must not publish a pending journal"
    );
}

fn assert_receipt_postimage_matches_export(
    fixture: &ImportReplaceFixture,
    session_id: &str,
    receipt: &serde_json::Value,
) {
    let export = fixture.run_export(session_id);
    assert_eq!(export.status.code(), Some(0), "{export:?}");
    assert_eq!(
        receipt["postimage_sha256"].as_str().unwrap(),
        sha256sum_bytes(&export.stdout)
    );
}

fn assert_invalid_input_has_no_mutation(
    build_input: impl FnOnce(&PreparedReplace) -> Vec<u8>,
) -> serde_json::Value {
    let prepared = prepared_claude_replace_fixture();
    let before = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    let input = build_input(&prepared);

    let output = prepared
        .fixture
        .run_import_replace_bytes(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(15), "{output:?}");
    let json = assert_json_error(&output, "invalid-input-transcript");
    let after = prepared.fixture.mutation_snapshot(
        &prepared.jsonl_path,
        &prepared.provider_name,
        &prepared.session_id,
    );
    assert_eq!(after.transcript_bytes, before.transcript_bytes);
    assert_eq!(after.turn_rows, before.turn_rows);
    assert_no_replace_journal_pollution(&prepared.fixture, &prepared.session_id);
    json
}

fn claude_seed_line(
    session_id: &str,
    turn_id: &str,
    role: &str,
    message: &str,
    offset: i64,
) -> String {
    serde_json::json!({
        "sessionId": session_id,
        "type": role,
        "uuid": turn_id,
        "timestamp": format!("2026-04-17T08:00:{offset:02}Z"),
        "message": message,
    })
    .to_string()
}

fn assert_public_session_replace_contract_types_are_reachable() {
    fn assert_renderer<T: CanonicalToProviderRenderer>() {}
    assert_renderer::<ClaudeCodeRenderer>();
    assert_renderer::<CodexSessionRenderer>();
    let _ = std::mem::size_of::<CanonicalRecord>();
    let _ = std::mem::size_of::<ReplaceReceipt>();
    let _ = std::mem::size_of::<ReplaceError>();
    let _runner: fn(
        &str,
        Option<&std::path::Path>,
        Option<&str>,
    ) -> Result<ReplaceReceipt, ReplaceError> = run_import_replace;
}
