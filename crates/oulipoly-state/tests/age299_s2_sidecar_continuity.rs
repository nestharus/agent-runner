use oulipoly_state::mailbox::{CompletionEventRegistrationInput, MailboxDb};
use oulipoly_state::migrations;
use oulipoly_state::{
    CURRENT_SCHEMA_VERSION, CompletionContinuityRecoveryState, CompletionObligationAdmission,
    CompletionRegistrationAuthority, InvocationStart, ProviderSessionBinding, StateDb,
};
const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const EVENT_ID: &str = "ab_age299_s2_event";
const ADMISSION_ID: &str = "admission-age299-s2";
const OWNER_SESSION_ID: &str = "session-age299-s2-owner";

#[test]
fn base_s1_schema_14_without_capability_upgrades_but_cannot_admit_new_authority() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    build_base_s1_schema_14(&state_path, false);

    let mut state = StateDb::open(&state_path).unwrap();

    assert_eq!(state_user_version(&state), CURRENT_SCHEMA_VERSION);
    assert_eq!(
        state.completion_continuity_recovery_state().unwrap(),
        CompletionContinuityRecoveryState::Ready
    );
    let authority = fixture_authority();
    let error = state
        .register_completion_event_with_authority(
            &authority,
            ADMISSION_ID,
            completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap_err();
    assert!(error.contains("no caller-bound"), "{error}");
    assert_eq!(
        count_state_rows(&state, "invocation_completion_continuity"),
        0
    );
    assert!(!sidecar_path.exists());
}

#[test]
fn base_s1_schema_14_obligations_require_typed_operator_recovery_before_registration() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    build_base_s1_schema_14(&state_path, true);

    let mut state = StateDb::open(&state_path).unwrap();

    assert_eq!(state_user_version(&state), CURRENT_SCHEMA_VERSION);
    assert_eq!(
        state.completion_continuity_recovery_state().unwrap(),
        CompletionContinuityRecoveryState::OperatorRecoveryRequired {
            unproven_obligation_count: 1,
        }
    );
    assert!(state.get_invocation_by_uuid(ROOT_UUID).unwrap().is_some());
    let error = state
        .register_completion_event_with_authority(
            &fixture_authority(),
            "post-upgrade-admission",
            completion_registration("post-upgrade-event", ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap_err();
    assert!(
        error.contains("completion_continuity_recovery=operator_recovery_required"),
        "{error}"
    );
    assert!(error.contains("agents migrate --rebuild"), "{error}");
    assert!(!sidecar_path.exists());
    assert_eq!(
        count_state_rows(&state, "invocation_completion_obligations"),
        1
    );
    assert_eq!(
        count_state_rows(&state, "invocation_completion_continuity"),
        0
    );
}

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
    let (alternate_row_id, alternate_authority) =
        start_authorized_invocation(&alternate, ROOT_UUID, OWNER_SESSION_ID);
    assert!(alternate_row_id > 0);

    alternate
        .register_completion_event_with_authority(
            &alternate_authority,
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
fn completion_admission_rejects_foreign_capability_and_fabricated_session() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let mut state = StateDb::open(&state_path).unwrap();
    let (_, owner_authority) = start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
    let foreign_uuid = "33333333-3333-4333-8333-333333333333";
    let (_, foreign_authority) =
        start_authorized_invocation(&state, foreign_uuid, "foreign-session");

    let foreign_error = state
        .register_completion_event_with_authority(
            &foreign_authority,
            "foreign-capability-admission",
            completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap_err();
    assert!(foreign_error.contains("not authorized"), "{foreign_error}");

    let session_error = state
        .register_completion_event_with_authority(
            &owner_authority,
            "fabricated-session-admission",
            completion_registration(EVENT_ID, ROOT_UUID, "fabricated-session"),
        )
        .unwrap_err();
    assert!(
        session_error.contains("not the authoritative session"),
        "{session_error}"
    );
    assert!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap()
            .is_empty()
    );
    assert!(!MailboxDb::path_for_state_db(&state_path).exists());
}

#[test]
fn caller_bound_capability_preserves_only_exact_immutable_replay() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let mut state = StateDb::open(&state_path).unwrap();
    let (_, authority) = start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
    let registration = completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID);

    state
        .register_completion_event_with_authority(&authority, ADMISSION_ID, registration)
        .unwrap();
    state
        .register_completion_event_with_authority(&authority, ADMISSION_ID, registration)
        .unwrap();

    assert_eq!(
        count_state_rows(&state, "invocation_completion_obligations"),
        1
    );
    assert_eq!(
        count_state_rows(&state, "invocation_completion_continuity"),
        1
    );
    let stored_digest: String = state
        .connection()
        .query_row(
            "SELECT completion_registration_capability_digest
             FROM invocations
             WHERE invocation_uuid = ?1",
            [ROOT_UUID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored_digest.len(), 64);
    assert_ne!(stored_digest, authority.process_environment_value());
    assert!(!format!("{authority:?}").contains(authority.process_environment_value()));
}

#[test]
fn admitted_completion_authority_refuses_missing_replaced_and_wrong_generation_sidecars() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let mut state = StateDb::open(&state_path).unwrap();
    let (invocation_row_id, authority) =
        start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
    state
        .register_completion_event_with_authority(
            &authority,
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
    let (missing_row_id, missing_authority) =
        start_authorized_invocation(&state, missing_uuid, "session-missing-sidecar");
    state
        .register_completion_event_with_authority(
            &missing_authority,
            "admission-missing-sidecar",
            completion_registration(
                "event-missing-sidecar",
                missing_uuid,
                "session-missing-sidecar",
            ),
        )
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
    assert_running(&state, missing_uuid);
}

#[test]
fn admitted_completion_authority_refuses_a_renamed_sidecar_until_exact_authority_returns() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let renamed_sidecar_path = directory.path().join("pid-identity.held");
    let mut state = StateDb::open(&state_path).unwrap();
    let (invocation_row_id, authority) =
        start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
    state
        .register_completion_event_with_authority(
            &authority,
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
fn finalization_rejects_a_post_a_same_generation_snapshot_after_b_is_admitted() {
    const B_UUID: &str = "22222222-2222-4222-8222-222222222222";
    const B_SESSION_ID: &str = "session-age299-s2-b";
    const B_EVENT_ID: &str = "ab_age299_s2_event_b";
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let post_a_snapshot = directory.path().join("pid-identity.post-a");
    let post_b_snapshot = directory.path().join("pid-identity.post-b");
    let mut state = StateDb::open(&state_path).unwrap();
    let (a_row_id, a_authority) = start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
    let (_, b_authority) = start_authorized_invocation(&state, B_UUID, B_SESSION_ID);

    state
        .register_completion_event_with_authority(
            &a_authority,
            ADMISSION_ID,
            completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap();
    let post_a_bytes = std::fs::read(&sidecar_path).unwrap();
    std::fs::write(&post_a_snapshot, &post_a_bytes).unwrap();
    let post_a_facts = sidecar_invocation_facts(&sidecar_path, ROOT_UUID);
    assert_eq!(post_a_facts.1, 1);
    assert_eq!(post_a_facts.2, 1);
    assert_eq!(post_a_facts.3, 1);

    state
        .register_completion_event_with_authority(
            &b_authority,
            "admission-age299-s2-b",
            completion_registration(B_EVENT_ID, B_UUID, B_SESSION_ID),
        )
        .unwrap();
    let post_b_bytes = std::fs::read(&sidecar_path).unwrap();
    std::fs::write(&post_b_snapshot, &post_b_bytes).unwrap();
    let post_b_facts = sidecar_invocation_facts(&sidecar_path, ROOT_UUID);
    assert_eq!(post_b_facts.0, post_a_facts.0);
    assert_eq!(post_b_facts.1, 2);
    assert_eq!(&post_b_facts.2.., &post_a_facts.2..);
    assert_eq!(
        count_state_rows(&state, "invocation_completion_continuity"),
        2
    );

    std::fs::remove_file(&sidecar_path).unwrap();
    std::fs::write(&sidecar_path, &post_a_bytes).unwrap();
    assert_eq!(std::fs::read(&sidecar_path).unwrap(), post_a_bytes);
    assert_eq!(
        sidecar_invocation_facts(&sidecar_path, ROOT_UUID),
        post_a_facts
    );

    let error = state
        .finalize_invocation(a_row_id, true, 0, None, None)
        .unwrap_err();

    assert!(error.contains("process_integrity"), "{error}");
    assert!(
        error.contains("no exact matching State/sidecar continuity proof"),
        "{error}"
    );
    assert_running(&state, ROOT_UUID);
    assert_eq!(
        state
            .completion_obligations_for_invocation(B_UUID)
            .unwrap()
            .len(),
        1
    );

    std::fs::remove_file(&sidecar_path).unwrap();
    std::fs::rename(&post_b_snapshot, &sidecar_path).unwrap();
    assert_eq!(std::fs::read(&sidecar_path).unwrap(), post_b_bytes);
    state
        .finalize_invocation(a_row_id, true, 0, None, None)
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
    insert_completion_obligation(
        &state,
        obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            ROOT_UUID,
            OWNER_SESSION_ID,
            &sidecar_generation,
        ),
    );

    let error = state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap_err();
    assert!(error.contains("process_integrity"), "{error}");
    assert!(
        error.contains("exact State continuity and materialization proof"),
        "{error}"
    );
    assert_running(&state, ROOT_UUID);
}

#[test]
fn admitted_completion_authority_refuses_present_event_with_wrong_owner_or_session() {
    for (wrong_owner, wrong_session) in [(true, false), (false, true)] {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        let (invocation_row_id, authority) =
            start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
        state
            .register_completion_event_with_authority(
                &authority,
                ADMISSION_ID,
                completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
            )
            .unwrap();
        replace_admitted_listener(
            &sidecar_path,
            if wrong_owner {
                "99999999-9999-4999-8999-999999999999"
            } else {
                ROOT_UUID
            },
            if wrong_session {
                "wrong-session-age299-s2"
            } else {
                OWNER_SESSION_ID
            },
        );

        let error = state
            .finalize_invocation(invocation_row_id, true, 0, None, None)
            .unwrap_err();

        assert!(error.contains("process_integrity"), "{error}");
        assert!(
            error.contains("exact matching State/sidecar continuity proof"),
            "{error}"
        );
        assert_running(&state, ROOT_UUID);
    }
}

#[test]
fn finalization_refuses_missing_mismatched_or_drifted_materialization_summaries() {
    for mutation in [
        "sidecar_missing",
        "sidecar_count_mismatch",
        "sidecar_digest_mismatch",
        "state_missing",
        "state_count_mismatch",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let sidecar_path = MailboxDb::path_for_state_db(&state_path);
        let mut state = StateDb::open(&state_path).unwrap();
        let (invocation_row_id, authority) =
            start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
        state
            .register_completion_event_with_authority(
                &authority,
                ADMISSION_ID,
                completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
            )
            .unwrap();
        let target = if mutation.starts_with("sidecar") {
            &sidecar_path
        } else {
            &state_path
        };
        let connection = rusqlite::Connection::open(target).unwrap();
        let statement = match mutation {
            "sidecar_missing" => {
                "DELETE FROM completion_authority_materialization_summary"
            }
            "sidecar_count_mismatch" => {
                "UPDATE completion_authority_materialization_summary
                 SET materialized_count = materialized_count + 1"
            }
            "sidecar_digest_mismatch" => {
                "UPDATE completion_authority_materialization_summary
                 SET continuity_digest = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'"
            }
            "state_missing" => "DELETE FROM invocation_completion_materialization_summary",
            "state_count_mismatch" => {
                "UPDATE invocation_completion_materialization_summary
                 SET materialized_count = materialized_count + 1"
            }
            _ => unreachable!(),
        };
        connection.execute(statement, []).unwrap();
        drop(connection);

        let error = state
            .finalize_invocation(invocation_row_id, true, 0, None, None)
            .unwrap_err();

        assert!(error.contains("process_integrity"), "{mutation}: {error}");
        assert_running(&state, ROOT_UUID);
    }
}

#[test]
fn schema_17_upgrade_backfills_only_an_exact_proven_materialization_summary() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let mut state = StateDb::open(&state_path).unwrap();
    let (invocation_row_id, authority) =
        start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
    state
        .register_completion_event_with_authority(
            &authority,
            ADMISSION_ID,
            completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap();
    drop(state);
    let connection = rusqlite::Connection::open(&state_path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER trg_invocation_completion_materialization_summary_continuity_insert;
             DROP TABLE invocation_completion_materialization_summary;
             DROP INDEX idx_invocations_parent_running_created;
             PRAGMA user_version = 17;",
        )
        .unwrap();
    drop(connection);

    state = StateDb::open(&state_path).unwrap();
    let summary: (i64, i64, String) = state
        .connection()
        .query_row(
            "SELECT materialized_count, authority_ordinal, continuity_digest
             FROM invocation_completion_materialization_summary
             WHERE invocation_uuid = ?1",
            [ROOT_UUID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(summary.0, 1);
    assert_eq!(summary.1, 1);
    assert_eq!(summary.2.len(), 64);
    state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap();
}

#[test]
fn sidecar_repair_does_not_synthesize_summary_from_malformed_listener_identity() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let mut state = StateDb::open(&state_path).unwrap();
    let (invocation_row_id, authority) =
        start_authorized_invocation(&state, ROOT_UUID, OWNER_SESSION_ID);
    state
        .register_completion_event_with_authority(
            &authority,
            ADMISSION_ID,
            completion_registration(EVENT_ID, ROOT_UUID, OWNER_SESSION_ID),
        )
        .unwrap();
    let connection = rusqlite::Connection::open(&sidecar_path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER trg_completion_event_listener_identity_update;
             DROP TRIGGER trg_completion_authority_materialization_summary_insert;
             DELETE FROM completion_authority_materialization_summary;
             UPDATE completion_event_listener SET session_id = 'malformed-session';",
        )
        .unwrap();
    drop(connection);
    drop(MailboxDb::open(&sidecar_path).unwrap());
    let summary_count: i64 = rusqlite::Connection::open(&sidecar_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM completion_authority_materialization_summary",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(summary_count, 0);

    let error = state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap_err();
    assert!(error.contains("process_integrity"), "{error}");
    assert_running(&state, ROOT_UUID);
}

#[test]
fn migrated_recovery_obligation_refuses_matching_listener_without_continuity_proof() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("state.db");
    let sidecar_path = MailboxDb::path_for_state_db(&state_path);
    let sidecar = MailboxDb::open(&sidecar_path).unwrap();
    let generation = sidecar.sidecar_generation().unwrap();
    drop(sidecar);
    insert_sidecar_event_listener(&sidecar_path, "base-s1-event", ROOT_UUID, "base-s1-session");
    build_base_s1_schema_14_with_obligation_generation(&state_path, &generation);
    let state = StateDb::open(&state_path).unwrap();
    let invocation_row_id = state.get_invocation_by_uuid(ROOT_UUID).unwrap().unwrap().id;

    let error = state
        .finalize_invocation(invocation_row_id, true, 0, None, None)
        .unwrap_err();

    assert!(
        error.contains("completion_continuity_recovery=operator_recovery_required"),
        "{error}"
    );
    assert_running(&state, ROOT_UUID);
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

fn start_authorized_invocation(
    state: &StateDb,
    invocation_uuid: &str,
    session_id: &str,
) -> (i64, CompletionRegistrationAuthority) {
    let start = state
        .start_invocation_with_completion_registration_authority(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "age299-s2".to_string(),
            provider_name: "test-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            start.invocation_row_id,
            &ProviderSessionBinding {
                provider_session_id: session_id.to_string(),
                capture_method: "fixture",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    (
        start.invocation_row_id,
        start.completion_registration_authority,
    )
}

fn fixture_authority() -> CompletionRegistrationAuthority {
    CompletionRegistrationAuthority::from_process_environment_value("11".repeat(32)).unwrap()
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

fn insert_completion_obligation(state: &StateDb, input: CompletionObligationAdmission<'_>) {
    let mut connection = rusqlite::Connection::open(state.path()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    migrations::run_with_db_path(&mut connection, &[], state.path().to_path_buf()).unwrap();
    connection
        .execute(
            "INSERT INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                input.admission_id,
                input.invocation_uuid,
                input.event_id,
                input.owner_invocation_uuid,
                input.owner_session_id,
                input.expected_sidecar_generation,
                "2026-08-13T00:00:00Z",
            ],
        )
        .unwrap();
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
    let invocation = state
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(invocation.status.as_str(), "running");
    assert_eq!(invocation.success, None);
    assert_eq!(invocation.exit_code, None);
    assert_eq!(invocation.error_category, None);
    assert_eq!(invocation.terminal_reason, None);
    assert_eq!(invocation.finished_at, None);
    assert!(
        !state
            .path()
            .parent()
            .unwrap()
            .join("invocations")
            .join(format!("{invocation_uuid}.result"))
            .exists()
    );
}

fn build_base_s1_schema_14(path: &std::path::Path, with_obligation: bool) {
    build_base_s1_schema_14_with_optional_generation(
        path,
        with_obligation.then_some("44444444-4444-4444-8444-444444444444"),
    );
}

fn build_base_s1_schema_14_with_obligation_generation(
    path: &std::path::Path,
    sidecar_generation: &str,
) {
    build_base_s1_schema_14_with_optional_generation(path, Some(sidecar_generation));
}

fn build_base_s1_schema_14_with_optional_generation(
    path: &std::path::Path,
    sidecar_generation: Option<&str>,
) {
    let mut connection = rusqlite::Connection::open(path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    let plan = migrations::plan(0, 14).unwrap();
    migrations::run_with_db_path(&mut connection, &plan, path.to_path_buf()).unwrap();
    assert_eq!(user_version(&connection), 14);
    assert!(!table_exists(
        &connection,
        "invocation_completion_continuity"
    ));
    connection
        .execute(
            "INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, created_at
             ) VALUES (?1, 'age299-s2', 'test-provider', 0, 'running', '2026-08-14T00:00:00Z')",
            [ROOT_UUID],
        )
        .unwrap();
    if let Some(sidecar_generation) = sidecar_generation {
        insert_base_s1_obligation(&connection, sidecar_generation);
    }
}

fn insert_base_s1_obligation(connection: &rusqlite::Connection, sidecar_generation: &str) {
    connection
        .execute(
            "INSERT INTO invocation_completion_obligations (
                    admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                    owner_session_id, expected_sidecar_generation, admitted_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, '2026-08-14T00:00:01Z')",
            rusqlite::params![
                "base-s1-admission",
                ROOT_UUID,
                "base-s1-event",
                ROOT_UUID,
                "base-s1-session",
                sidecar_generation,
            ],
        )
        .unwrap();
}

fn replace_admitted_listener(
    sidecar_path: &std::path::Path,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
) {
    let connection = rusqlite::Connection::open(sidecar_path).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
             DROP TRIGGER trg_completion_authority_continuity_delete;
             DROP TRIGGER trg_completion_event_listener_continuity_delete;
             DELETE FROM completion_authority_continuity;
             DELETE FROM completion_event_listener;
             DELETE FROM completion_event;",
        )
        .unwrap();
    drop(connection);
    insert_sidecar_event_listener(
        sidecar_path,
        EVENT_ID,
        owner_invocation_uuid,
        owner_session_id,
    );
}

fn insert_sidecar_event_listener(
    sidecar_path: &std::path::Path,
    event_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
) {
    let connection = rusqlite::Connection::open(sidecar_path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection
        .execute(
            "INSERT INTO completion_event (
                event_id, kind, state, delivery_mode, state_dir, meta_path,
                log_path, rc_path, created_at
             ) VALUES (?1, 'agent_bash_complete', 'pending', 'async', ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                event_id,
                "/tmp/age299-s2-state",
                "/tmp/age299-s2-meta",
                "/tmp/age299-s2-log",
                "/tmp/age299-s2-rc",
                "2026-08-14T00:00:00Z",
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO completion_event_listener (
                event_id, listener_id, session_id, owner_invocation_uuid,
                active, created_at
             ) VALUES (?1, ?2, ?3, ?2, 1, ?4)",
            rusqlite::params![
                event_id,
                owner_invocation_uuid,
                owner_session_id,
                "2026-08-14T00:00:00Z",
            ],
        )
        .unwrap();
}

fn user_version(connection: &rusqlite::Connection) -> i32 {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn table_exists(connection: &rusqlite::Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT EXISTS (
                SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1
             )",
            [table],
            |row| row.get(0),
        )
        .unwrap()
}

fn state_user_version(state: &StateDb) -> i32 {
    state
        .connection()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn count_state_rows(state: &StateDb, table: &str) -> i64 {
    state
        .connection()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn sidecar_invocation_facts(
    path: &std::path::Path,
    invocation_uuid: &str,
) -> (String, i64, i64, i64) {
    let connection = rusqlite::Connection::open(path).unwrap();
    let generation = connection
        .query_row(
            "SELECT generation_uuid FROM mailbox_sidecar_identity WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let continuity_count = connection
        .query_row(
            "SELECT COUNT(*) FROM completion_authority_continuity",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event_listener_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM completion_event e
             JOIN completion_event_listener l ON l.event_id = e.event_id
             WHERE l.owner_invocation_uuid = ?1",
            [invocation_uuid],
            |row| row.get(0),
        )
        .unwrap();
    let summary_count = connection
        .query_row(
            "SELECT materialized_count
             FROM completion_authority_materialization_summary
             WHERE invocation_uuid = ?1",
            [invocation_uuid],
            |row| row.get(0),
        )
        .unwrap();
    (
        generation,
        continuity_count,
        event_listener_count,
        summary_count,
    )
}
