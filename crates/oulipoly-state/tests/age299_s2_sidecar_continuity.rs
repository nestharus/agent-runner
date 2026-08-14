use oulipoly_state::mailbox::{CompletionEventRegistrationInput, MailboxDb};
use oulipoly_state::{CompletionObligationAdmission, InvocationStart, StateDb};
const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const EVENT_ID: &str = "ab_age299_s2_event";
const ADMISSION_ID: &str = "admission-age299-s2";
const OWNER_SESSION_ID: &str = "session-age299-s2-owner";

#[test]
fn each_state_database_path_derives_a_distinct_sidecar_path() {
    let directory = tempfile::tempdir().unwrap();
    let canonical_state = directory.path().join("state.db");
    let alternate_state = directory.path().join("alternate.db");

    assert_eq!(
        MailboxDb::path_for_state_db(&canonical_state),
        directory.path().join("pid-identity.db")
    );
    assert_eq!(
        MailboxDb::path_for_state_db(&alternate_state),
        directory.path().join("alternate.db.pid-identity.db")
    );
    assert_ne!(
        MailboxDb::path_for_state_db(&canonical_state),
        MailboxDb::path_for_state_db(&alternate_state)
    );
}

#[test]
fn sibling_state_databases_cannot_register_into_one_sidecar_authority() {
    let directory = tempfile::tempdir().unwrap();
    let canonical_path = directory.path().join("state.db");
    let alternate_path = directory.path().join("alternate.db");
    let canonical = StateDb::open(&canonical_path).unwrap();
    let mut alternate = StateDb::open(&alternate_path).unwrap();
    let canonical_row_id = start_invocation(&canonical, ROOT_UUID);
    start_invocation(&alternate, ROOT_UUID);

    alternate
        .register_completion_event_with_obligation(
            "alternate-state-admission",
            completion_registration("alternate-state-event", ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap();

    let canonical_sidecar = MailboxDb::path_for_state_db(&canonical_path);
    let alternate_sidecar = MailboxDb::path_for_state_db(&alternate_path);
    assert_ne!(canonical_sidecar, alternate_sidecar);
    assert!(!canonical_sidecar.exists());
    assert!(alternate_sidecar.exists());
    assert!(
        canonical
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        alternate
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap()
            .len(),
        1
    );
    canonical
        .finalize_invocation(canonical_row_id, true, 0, None, None)
        .unwrap();
}

#[test]
fn admitted_completion_authority_refuses_missing_replaced_and_wrong_generation_sidecars() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let mut state = StateDb::open(&state_path).unwrap();
    let invocation_row_id = start_invocation(&state, ROOT_UUID);
    state
        .register_completion_event_with_obligation(
            ADMISSION_ID,
            completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap();
    let matching_generation = MailboxDb::open(&sidecar_path)
        .unwrap()
        .sidecar_generation()
        .unwrap();
    state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap();

    let missing_uuid = "55555555-5555-4555-8555-555555555555";
    let missing_row_id = start_invocation(&state, missing_uuid);
    state
        .record_completion_obligation(obligation(
            "admission-missing-sidecar",
            missing_uuid,
            "event-missing-sidecar",
            missing_uuid,
            "session-missing-sidecar",
            &matching_generation,
        ))
        .unwrap();
    drop(state);
    std::fs::remove_file(&sidecar_path).unwrap();

    let state = StateDb::open(&state_path).unwrap();
    let missing_error = state
        .finalize_invocation(missing_row_id, true, 0, None, None)
        .unwrap_err();
    assert!(
        missing_error.contains("process_integrity"),
        "{missing_error}"
    );
    assert!(missing_error.contains(missing_uuid), "{missing_error}");
    assert!(
        missing_error.contains("sidecar is missing"),
        "{missing_error}"
    );
    assert_running(&state, missing_uuid);

    let replaced_generation = MailboxDb::open(&sidecar_path)
        .unwrap()
        .sidecar_generation()
        .unwrap();
    assert_ne!(replaced_generation, matching_generation);
    let mismatch_error = state
        .finalize_invocation(missing_row_id, true, 0, None, None)
        .unwrap_err();
    assert!(
        mismatch_error.contains("process_integrity"),
        "{mismatch_error}"
    );
    assert!(
        mismatch_error.contains(&matching_generation),
        "{mismatch_error}"
    );
    assert!(
        mismatch_error.contains(&replaced_generation),
        "{mismatch_error}"
    );
}

#[test]
fn admitted_completion_authority_refuses_a_renamed_sidecar_until_exact_authority_returns() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let renamed_sidecar_path = directory.path().join("pid-identity.held");
    let mut state = StateDb::open(&state_path).unwrap();
    let invocation_row_id = start_invocation(&state, ROOT_UUID);
    state
        .register_completion_event_with_obligation(
            ADMISSION_ID,
            completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap();
    std::fs::rename(&sidecar_path, &renamed_sidecar_path).unwrap();

    let error = state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap_err();
    assert!(error.contains("process_integrity"), "{error}");
    assert!(error.contains("sidecar is missing"), "{error}");
    assert_running(&state, ROOT_UUID);

    std::fs::rename(&renamed_sidecar_path, &sidecar_path).unwrap();
    state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap();
}

#[test]
fn admitted_completion_authority_refuses_an_absent_event_in_the_matching_sidecar() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let state = StateDb::open(&state_path).unwrap();
    let invocation_row_id = start_invocation(&state, ROOT_UUID);
    let sidecar_generation = MailboxDb::open(&sidecar_path)
        .unwrap()
        .sidecar_generation()
        .unwrap();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            ROOT_UUID,
            OWNER_SESSION_ID,
            &sidecar_generation,
        ))
        .unwrap();

    let error = state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap_err();
    assert!(error.contains("process_integrity"), "{error}");
    assert!(error.contains(ADMISSION_ID), "{error}");
    assert!(error.contains(EVENT_ID), "{error}");
    assert!(error.contains("event listener is absent"), "{error}");
}

#[test]
fn invocation_without_admitted_completion_authority_keeps_existing_absence_semantics() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let state = StateDb::open(&state_path).unwrap();
    let invocation_row_id = start_invocation(&state, ROOT_UUID);
    assert!(!MailboxDb::path_for_state_db(&state_path).exists());

    state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap();
}

#[test]
fn mailbox_sidecar_generation_is_stable_and_rejects_direct_identity_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let sidecar_path = directory.path().join("pid-identity.db");
    let mailbox = MailboxDb::open(&sidecar_path).unwrap();
    let generation = mailbox.sidecar_generation().unwrap();
    let connection = rusqlite::Connection::open(&sidecar_path).unwrap();

    for statement in [
        "UPDATE mailbox_sidecar_identity SET generation_uuid = '55555555-5555-4555-8555-555555555555' WHERE singleton = 1",
        "DELETE FROM mailbox_sidecar_identity WHERE singleton = 1",
        "INSERT OR REPLACE INTO mailbox_sidecar_identity (singleton, generation_uuid, created_at) VALUES (1, '55555555-5555-4555-8555-555555555555', '2026-08-13T00:00:00Z')",
        "INSERT INTO mailbox_sidecar_identity (singleton, generation_uuid, created_at) VALUES (1, '55555555-5555-4555-8555-555555555555', '2026-08-13T00:00:00Z') ON CONFLICT(singleton) DO UPDATE SET generation_uuid = excluded.generation_uuid",
    ] {
        let error = connection.execute(statement, []).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("mailbox sidecar identity is immutable"),
            "{statement}: {error}"
        );
        assert_eq!(mailbox.sidecar_generation().unwrap(), generation);
    }
    drop(mailbox);
    assert_eq!(
        MailboxDb::open(&sidecar_path)
            .unwrap()
            .sidecar_generation()
            .unwrap(),
        generation
    );
}

fn start_invocation(state: &StateDb, invocation_uuid: &str) -> i64 {
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "age299-s2".to_string(),
            provider_name: "test-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap()
}

fn obligation<'a>(
    admission_id: &'a str,
    invocation_uuid: &'a str,
    event_id: &'a str,
    owner_invocation_uuid: &'a str,
    owner_session_id: &'a str,
    expected_sidecar_generation: &'a str,
) -> CompletionObligationAdmission<'a> {
    CompletionObligationAdmission {
        admission_id,
        invocation_uuid,
        event_id,
        owner_invocation_uuid,
        owner_session_id,
        expected_sidecar_generation,
    }
}

fn completion_registration<'a>(
    event_id: &'a str,
    owner_invocation_uuid: &'a str,
    owner_session_id: &'a str,
) -> CompletionEventRegistrationInput<'a> {
    CompletionEventRegistrationInput {
        event_id,
        delivery_mode: "async",
        owner_session_id: Some(owner_session_id),
        owner_invocation_uuid: Some(owner_invocation_uuid),
        state_dir: "/tmp/age299-s2-state",
        meta_path: "/tmp/age299-s2-meta",
        log_path: "/tmp/age299-s2-log",
        rc_path: "/tmp/age299-s2-rc",
    }
}

fn assert_running(state: &StateDb, invocation_uuid: &str) {
    assert_eq!(
        state
            .get_invocation_by_uuid(invocation_uuid)
            .unwrap()
            .unwrap()
            .status
            .as_str(),
        "running"
    );
}
