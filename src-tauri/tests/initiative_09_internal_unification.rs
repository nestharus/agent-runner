#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06_import_replace::*;
use oulipoly_config::{ProvidersConfig, SessionsConfig, load_models};
use oulipoly_runtime::session_lock::{self, LockError, SessionLock};
use oulipoly_runtime::session_metadata::locate_session_metadata;
use rusqlite::params;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Risk: T-cross-module-lock-visibility — import-replace may not see leases acquired through the public pause-handshake path.
/// Level: CLI integration.
/// Source: proposal §4 T-cross-module-lock-visibility; D1-D4, D8.
/// Observable: pause-handshake acquires a lease; immediate import-replace exits 13 session-busy with expires_at populated; resume-handshake releases the token.
/// Residual: does not prove visibility for non-CLI in-process lock holders beyond the shared on-disk lease shape.
#[test]
fn t_cross_module_lock_visibility() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "cross-module",
    );
    let pause = prepared.fixture.run_pause_handshake(&prepared.session_id);
    let pause_receipt = assert_success(&pause);
    let token = pause_receipt["token"].as_str().unwrap().to_string();

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);
    let resume = prepared
        .fixture
        .run_resume_handshake(&prepared.session_id, &token);

    assert_success(&resume);
    assert_eq!(output.status.code(), Some(13), "{output:?}");
    let error = assert_json_error(&output, "session-busy");
    assert!(
        error["error"]["expires_at"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "{error}"
    );
}

/// Risk: T-busy-token-hash-preserved — lifting import-replace to public session_lock may drop or reformat the busy token hash.
/// Level: CLI integration.
/// Source: proposal §4 T-busy-token-hash-preserved; D7.
/// Observable: import-replace exits 13 while a pause-handshake lease is live, and stderr JSON error.token is a non-empty 64-character SHA-256 hex string.
/// Residual: validates the user-visible hash shape, not the raw pause token value hidden on disk.
#[test]
fn t_busy_token_hash_preserved() {
    let prepared = prepared_claude_replace_fixture();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "busy-token",
    );
    let pause = prepared.fixture.run_pause_handshake(&prepared.session_id);
    let pause_receipt = assert_success(&pause);
    let token = pause_receipt["token"].as_str().unwrap().to_string();

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);
    let resume = prepared
        .fixture
        .run_resume_handshake(&prepared.session_id, &token);

    assert_success(&resume);
    assert_eq!(output.status.code(), Some(13), "{output:?}");
    let error = assert_json_error(&output, "session-busy");
    let token_hash = error["error"]["token"].as_str().unwrap();
    assert_hash_hex(token_hash);
    let expected_hash = sha256sum_bytes(token.as_bytes());
    assert_eq!(
        token_hash, expected_hash,
        "busy JSON token must equal sha256(live pause token)"
    );
}

/// Risk: T-error-path-release — a post-acquire import-replace failure may leave a stuck session lease.
/// Level: CLI integration.
/// Source: proposal §4 T-error-path-release; D3.
/// Observable: forced fail-postimage-verification exits 1; a fresh immediate import-replace on the same session does not exit 13.
/// Residual: covers the forced postimage-verification path, not every possible OS-level write/fsync failure.
#[test]
fn t_error_path_release() {
    let prepared = prepared_claude_replace_fixture();
    let failing_input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "forced-failure",
    );
    let failed = prepared
        .fixture
        .spawn_import_replace(
            &prepared.session_id,
            &failing_input,
            &[],
            &[(TEST_HOOK_ENV, TEST_FAIL_POSTIMAGE_VERIFY)],
        )
        .wait_with_output()
        .unwrap();

    assert_eq!(failed.status.code(), Some(1), "{failed:?}");
    assert_json_error(&failed, "operational-error");

    let fresh_input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "fresh-after-failure",
    );
    let fresh = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &fresh_input, &[]);

    assert_ne!(fresh.status.code(), Some(13), "{fresh:?}");
    assert_success_allowing_test_hook_stderr(&fresh);
}

/// Risk: T-active-segment-id-flows — public session_metadata resolution may lose the active segment identity needed by import-replace DB writes.
/// Level: component.
/// Source: proposal §4 T-active-segment-id-flows; D5, D6.
/// Observable: pre-replace locate_session_metadata.active_segment_id equals the active row id; post-replace last_turn_id changes on that same id; session_chains.last_used_at changes for metadata.chain_id.
/// Residual: focuses on the selected active segment and does not cover ambiguous multi-chain resolution.
#[test]
fn t_active_segment_id_flows() {
    let _path_guard = scripts_path_lock().lock().unwrap();
    let old_path = std::env::var_os("PATH");
    let scripts_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("scripts");
    let path = std::env::join_paths(
        std::iter::once(scripts_dir)
            .chain(std::env::split_paths(&old_path.clone().unwrap_or_default())),
    )
    .unwrap();
    unsafe {
        std::env::set_var("PATH", path);
    }

    let prepared = prepared_claude_replace_fixture();
    let state = prepared.fixture.open_db();
    let models = load_models(prepared.fixture.models_dir(), None).unwrap();
    let providers = ProvidersConfig::load(&prepared.fixture.providers_path()).unwrap();
    let sessions = SessionsConfig::load(&prepared.fixture.sessions_path()).unwrap();
    let metadata =
        locate_session_metadata(&state, &models, &providers, &sessions, &prepared.session_id)
            .unwrap();
    let conn = prepared.fixture.conn();
    let active_segment_id = conn
        .query_row(
            "SELECT id FROM session_chain_segments
             WHERE session_id = ?1 AND ended_at IS NULL",
            params![prepared.session_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(
        state
            .active_segment_id_for_chain_provider_session(
                &metadata.chain_id,
                &prepared.provider_name,
                &prepared.session_id,
            )
            .unwrap(),
        Some(active_segment_id)
    );
    assert_eq!(
        state
            .active_segment_id_for_chain_provider_session(
                &metadata.chain_id,
                "other-provider",
                &prepared.session_id,
            )
            .unwrap(),
        None
    );
    assert_eq!(metadata.active_segment_id, active_segment_id);

    let chain_id = metadata.chain_id.clone();
    let before_last_used = prepared.fixture.chain_last_used_at(&chain_id);
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "active-segment",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_success(&output);
    let post_last_turn = conn
        .query_row(
            "SELECT last_turn_id FROM session_chain_segments WHERE id = ?1",
            params![active_segment_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap();
    assert_eq!(post_last_turn.as_deref(), Some("active-segment-turn-2"));
    let after_last_used = prepared.fixture.chain_last_used_at(&chain_id);
    assert_ne!(after_last_used, before_last_used);
    assert_eq!(after_last_used, "2026-04-17T09:00:01Z");

    match old_path {
        Some(path) => unsafe {
            std::env::set_var("PATH", path);
        },
        None => unsafe {
            std::env::remove_var("PATH");
        },
    }
}

#[test]
fn session_replace_active_segment_unchanged_under_contract() {
    t_active_segment_id_flows();
}

fn scripts_path_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Risk: T-any-active-for-session-public — recovery may lack a public way to detect active session leases after internal lock deletion.
/// Level: component.
/// Source: proposal §4 T-any-active-for-session-public; D4.
/// Observable: any_active_for_session returns false for missing lock dir and missing lease file, true for an active lease, and false for an expired lease.
/// Residual: does not cover malformed lock JSON, which remains an operational-error path.
#[test]
fn t_any_active_for_session_public() {
    let dir = tempfile::tempdir().unwrap();
    let missing_lock_dir = dir.path().join("missing-locks");
    assert!(!session_lock::any_active_for_session(&missing_lock_dir, SESSION_A).unwrap());

    let lock_dir = dir.path().join("locks");
    let lock = SessionLock::new(&lock_dir).unwrap();
    assert!(!session_lock::any_active_for_session(&lock_dir, SESSION_A).unwrap());

    let _active = lock
        .acquire(SESSION_A, CLAUDE_PROVIDER, Duration::from_secs(60))
        .unwrap();
    assert!(session_lock::any_active_for_session(&lock_dir, SESSION_A).unwrap());
    match lock
        .acquire(SESSION_A, CLAUDE_PROVIDER, Duration::from_secs(60))
        .unwrap_err()
    {
        LockError::Busy { token_hash, .. } => {
            let token_hash = token_hash.unwrap();
            let token_hash = token_hash.strip_prefix("sha256:").unwrap_or(&token_hash);
            assert_hash_hex(token_hash);
        }
        err => panic!("expected busy lock error, got {err:?}"),
    }

    let _expired = lock
        .acquire(SESSION_B, CLAUDE_PROVIDER, Duration::from_millis(1))
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(!session_lock::any_active_for_session(&lock_dir, SESSION_B).unwrap());
}
