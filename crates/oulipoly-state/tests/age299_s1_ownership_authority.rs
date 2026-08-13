use oulipoly_state::continuation::{ContinuationAcceptInput, ContinuationAcceptResult};
use oulipoly_state::migrations;
use oulipoly_state::repositories::ContinuationRepository;
use oulipoly_state::{
    CURRENT_SCHEMA_VERSION, CompletionObligationAdmission, CompletionObligationAdmissionResult,
    CompletionObligationAuthority, InvocationStart, ListenerSettlementClass,
    OwnerLineageRelationship, RecoveryDisposition, SettlementVerifierIdentity,
    SidecarGenerationState, StateDb,
};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};
use std::path::Path;

const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const CHILD_UUID: &str = "22222222-2222-4222-8222-222222222222";
const GRANDCHILD_UUID: &str = "33333333-3333-4333-8333-333333333333";
const EVENT_ID: &str = "ab_age299_s1_event";
const ADMISSION_ID: &str = "admission-age299-s1";
const SECOND_ADMISSION_ID: &str = "admission-age299-s1-second-listener";
const OWNER_SESSION_ID: &str = "session-age299-s1-owner";
const SECOND_OWNER_SESSION_ID: &str = "session-age299-s1-second-owner";
const GENERATION_ID: &str = "44444444-4444-4444-8444-444444444444";
const NULL_ADMISSION_ERROR: &str =
    "NOT NULL constraint failed: invocation_completion_obligations.admission_id";
const APPEND_ONLY_UPDATE_ERROR: &str = "completion obligation is append-only: update forbidden";
const APPEND_ONLY_DELETE_ERROR: &str = "completion obligation is append-only: delete forbidden";
const NON_TEXT_ADMISSION_ID: &str = "malformed-storage-admission";
const NUMERIC_ADMISSION_ID: &str = "299";
const STRING_DECODED_FIELDS: [&str; 7] = [
    "admission_id",
    "invocation_uuid",
    "event_id",
    "owner_invocation_uuid",
    "owner_session_id",
    "expected_sidecar_generation",
    "admitted_at",
];
const COMPLETION_OBLIGATION_INSERT: &str = concat!(
    "INSERT INTO invocation_completion_obligations (",
    "admission_id, invocation_uuid, event_id, owner_invocation_uuid, ",
    "owner_session_id, expected_sidecar_generation, admitted_at",
    ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
);
const COMPLETION_OBLIGATION_REPLACE: &str = concat!(
    "INSERT OR REPLACE INTO invocation_completion_obligations (",
    "admission_id, invocation_uuid, event_id, owner_invocation_uuid, ",
    "owner_session_id, expected_sidecar_generation, admitted_at",
    ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
);

type RawCompletionObligationRow = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
);

#[test]
fn schema_13_migrates_through_ordered_v14_and_preserves_invocation_data() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.db");
    build_schema_13_database(&path);
    let before = invocation_projection(&Connection::open(&path).unwrap());

    let plan = migrations::current_plan_from(13).unwrap();
    assert_eq!(
        plan.iter()
            .map(|migration| (migration.target_version, migration.id))
            .collect::<Vec<_>>(),
        vec![(14, "0014_invocation_completion_obligations")]
    );

    let state = StateDb::open(&path).unwrap();
    assert_eq!(user_version(state.connection()), CURRENT_SCHEMA_VERSION);
    assert_eq!(invocation_projection(state.connection()), before);
    assert!(table_exists(
        state.connection(),
        "invocation_completion_obligations"
    ));
}

#[test]
fn fresh_and_migrated_v14_have_the_same_completion_obligation_schema() {
    let directory = tempfile::tempdir().unwrap();
    let fresh_path = directory.path().join("fresh.db");
    let migrated_path = directory.path().join("migrated.db");

    let fresh = StateDb::open(&fresh_path).unwrap();
    build_schema_13_database(&migrated_path);
    let migrated = StateDb::open(&migrated_path).unwrap();

    assert_eq!(
        ownership_schema(&fresh),
        ownership_schema(&migrated),
        "fresh and migrated databases must expose identical table, index, and constraint SQL"
    );
    assert!(ownership_schema(&fresh).iter().any(|sql| {
        sql.contains("admission_id ANY NOT NULL PRIMARY KEY")
            && sql.contains("UNIQUE (event_id, owner_invocation_uuid)")
            && sql.contains("REFERENCES invocations(invocation_uuid)")
            && sql.ends_with(") STRICT")
    }));
    let table_sql = ownership_schema(&fresh)
        .into_iter()
        .find(|sql| sql.starts_with("CREATE TABLE invocation_completion_obligations"))
        .unwrap();
    for field in STRING_DECODED_FIELDS {
        assert!(
            table_sql.contains(&format!("CONSTRAINT completion_obligation_{field}_text"))
                && table_sql.contains(&format!("typeof({field}) = 'text'")),
            "missing storage-class contract for {field}: {table_sql}"
        );
    }
    let schema = ownership_schema(&fresh);
    assert_eq!(
        schema
            .iter()
            .filter(|sql| sql.contains("completion event sidecar generation conflict"))
            .count(),
        1,
        "inserts must preserve one immutable generation per event"
    );
    assert_eq!(
        schema
            .iter()
            .filter(|sql| sql.contains(APPEND_ONLY_UPDATE_ERROR))
            .count(),
        1,
        "every direct update must be rejected"
    );
    assert_eq!(
        schema
            .iter()
            .filter(|sql| sql.contains(APPEND_ONLY_DELETE_ERROR))
            .count(),
        1,
        "every direct delete must be rejected"
    );
}

#[test]
fn absent_obligation_and_exact_admitted_expectation_are_distinct() {
    let state = state_with_root();
    assert_eq!(
        state.completion_obligation_authority(ADMISSION_ID).unwrap(),
        CompletionObligationAuthority::NoAdmittedObligation
    );
    let invalid = state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            ROOT_UUID,
            " ",
            GENERATION_ID,
        ))
        .unwrap_err();
    assert_eq!(
        invalid.to_string(),
        "invalid ownership identity: owner_session_id"
    );

    let recorded = state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            ROOT_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();
    let CompletionObligationAdmissionResult::Recorded(expectation) = recorded else {
        panic!("first admission must record the expectation")
    };
    assert_eq!(expectation.admission_id, ADMISSION_ID);
    assert_eq!(expectation.event_id, EVENT_ID);
    assert_eq!(expectation.owner_invocation_uuid, ROOT_UUID);
    assert_eq!(expectation.owner_session_id, OWNER_SESSION_ID);
    assert_eq!(expectation.expected_sidecar_generation, GENERATION_ID);
    assert_eq!(
        state.completion_obligation_authority(ADMISSION_ID).unwrap(),
        CompletionObligationAuthority::Admitted(expectation)
    );
}

#[test]
fn empty_table_rejects_null_admission_identity_for_every_insert_form() {
    let state = state_with_lineage();

    assert_null_admission_insert_forms_are_rejected(
        &state,
        "null-admission-empty-event",
        CHILD_UUID,
        "null-admission-empty-session",
    );
}

#[test]
fn singleton_listener_rejects_null_admission_identity_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_null_admission_insert_forms_are_rejected(
        &state,
        "null-admission-singleton-event",
        GRANDCHILD_UUID,
        "null-admission-singleton-session",
    );
    assert_null_admission_conflict_actions_are_rejected(
        &state,
        EVENT_ID,
        CHILD_UUID,
        "null-admission-singleton-replacement",
    );
}

#[test]
fn multi_listener_event_rejects_null_admission_identity_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();
    state
        .record_completion_obligation(obligation(
            SECOND_ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            GRANDCHILD_UUID,
            SECOND_OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_null_admission_insert_forms_are_rejected(
        &state,
        "null-admission-multi-event",
        ROOT_UUID,
        "null-admission-multi-session",
    );
    assert_null_admission_conflict_actions_are_rejected(
        &state,
        EVENT_ID,
        GRANDCHILD_UUID,
        "null-admission-multi-replacement",
    );
}

#[test]
fn empty_table_rejects_non_text_storage_for_every_string_projection() {
    let state = state_with_lineage();

    assert_non_text_insert_forms_are_rejected(
        &state,
        "malformed-empty-event",
        CHILD_UUID,
        "malformed-empty-session",
    );
}

#[test]
fn singleton_listener_rejects_non_text_storage_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_non_text_insert_forms_are_rejected(
        &state,
        "malformed-singleton-event",
        GRANDCHILD_UUID,
        "malformed-singleton-session",
    );
    assert_non_text_conflict_actions_are_rejected(
        &state,
        ADMISSION_ID,
        EVENT_ID,
        CHILD_UUID,
        OWNER_SESSION_ID,
    );
    assert_equivalent_blob_admission_is_rejected(&state, ADMISSION_ID);
}

#[test]
fn multi_listener_event_rejects_non_text_storage_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();
    state
        .record_completion_obligation(obligation(
            SECOND_ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            GRANDCHILD_UUID,
            SECOND_OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_non_text_insert_forms_are_rejected(
        &state,
        "malformed-multi-event",
        ROOT_UUID,
        "malformed-multi-session",
    );
    assert_non_text_conflict_actions_are_rejected(
        &state,
        SECOND_ADMISSION_ID,
        EVENT_ID,
        GRANDCHILD_UUID,
        SECOND_OWNER_SESSION_ID,
    );
}

#[test]
fn direct_inserts_allow_sibling_listeners_only_for_the_admitted_generation() {
    let state = state_with_lineage();
    let first = obligation(
        ADMISSION_ID,
        ROOT_UUID,
        EVENT_ID,
        CHILD_UUID,
        OWNER_SESSION_ID,
        GENERATION_ID,
    );
    let CompletionObligationAdmissionResult::Recorded(first_recorded) =
        state.record_completion_obligation(first).unwrap()
    else {
        panic!("first listener admission must be recorded")
    };

    let before = raw_completion_obligation_rows(&state);
    let mismatched_sibling = obligation(
        SECOND_ADMISSION_ID,
        ROOT_UUID,
        EVENT_ID,
        GRANDCHILD_UUID,
        SECOND_OWNER_SESSION_ID,
        "mismatched-generation",
    );
    let insert_error =
        direct_insert_completion_obligation(&state, mismatched_sibling, "2026-08-13T00:02:00Z")
            .unwrap_err();
    assert!(
        insert_error
            .to_string()
            .contains("completion event sidecar generation conflict"),
        "{insert_error}"
    );
    assert_eq!(raw_completion_obligation_rows(&state), before);

    let second = obligation(
        SECOND_ADMISSION_ID,
        ROOT_UUID,
        EVENT_ID,
        GRANDCHILD_UUID,
        SECOND_OWNER_SESSION_ID,
        GENERATION_ID,
    );
    direct_insert_completion_obligation(&state, second, "9999-12-31T23:59:59Z").unwrap();
    let CompletionObligationAdmissionResult::Replay(second_recorded) =
        state.record_completion_obligation(second).unwrap()
    else {
        panic!("the directly inserted sibling must replay exactly through the API")
    };
    assert_eq!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap(),
        vec![first_recorded, second_recorded]
    );
}

#[test]
fn singleton_listener_rejects_every_direct_update_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_every_direct_update_is_rejected(&state, ADMISSION_ID);
}

#[test]
fn multi_listener_event_rejects_every_direct_update_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();
    state
        .record_completion_obligation(obligation(
            SECOND_ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            GRANDCHILD_UUID,
            SECOND_OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_every_direct_update_is_rejected(&state, ADMISSION_ID);
}

#[test]
fn singleton_listener_rejects_direct_delete_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_direct_delete_is_rejected(&state, ADMISSION_ID);
}

#[test]
fn multi_listener_event_rejects_direct_delete_without_mutation() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();
    state
        .record_completion_obligation(obligation(
            SECOND_ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            GRANDCHILD_UUID,
            SECOND_OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();

    assert_direct_delete_is_rejected(&state, ADMISSION_ID);
}

#[test]
fn replace_cannot_bypass_append_only_identity() {
    let state = state_with_lineage();
    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();
    let before = raw_completion_obligation_rows(&state);

    let error = state
        .connection()
        .execute(
            "INSERT OR REPLACE INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                ADMISSION_ID,
                ROOT_UUID,
                "replacement-event",
                GRANDCHILD_UUID,
                "replacement-session",
                "replacement-generation",
                "2026-08-13T00:04:00Z",
            ],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("completion obligation immutable identity conflict"),
        "{error}"
    );
    assert_eq!(raw_completion_obligation_rows(&state), before);
}

#[test]
fn event_listeners_are_independent_idempotent_obligations_with_one_generation() {
    let state = state_with_lineage();
    let first = obligation(
        ADMISSION_ID,
        ROOT_UUID,
        EVENT_ID,
        CHILD_UUID,
        OWNER_SESSION_ID,
        GENERATION_ID,
    );
    let second = obligation(
        SECOND_ADMISSION_ID,
        ROOT_UUID,
        EVENT_ID,
        GRANDCHILD_UUID,
        SECOND_OWNER_SESSION_ID,
        GENERATION_ID,
    );
    let CompletionObligationAdmissionResult::Recorded(first_recorded) =
        state.record_completion_obligation(first).unwrap()
    else {
        panic!("first listener admission must be recorded")
    };

    let mismatched_listener = obligation(
        SECOND_ADMISSION_ID,
        ROOT_UUID,
        EVENT_ID,
        GRANDCHILD_UUID,
        SECOND_OWNER_SESSION_ID,
        "55555555-5555-4555-8555-555555555555",
    );
    assert_immutable_conflict_without_mutation(
        &state,
        mismatched_listener,
        std::slice::from_ref(&first_recorded),
    );

    let CompletionObligationAdmissionResult::Recorded(second_recorded) =
        state.record_completion_obligation(second).unwrap()
    else {
        panic!("sibling listener with the event generation must be recorded")
    };
    let expected = vec![first_recorded.clone(), second_recorded.clone()];
    assert_eq!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap(),
        expected,
        "both descendant-owned listeners must remain queryable in the root scope"
    );

    for (input, recorded) in [(first, first_recorded), (second, second_recorded)] {
        let CompletionObligationAdmissionResult::Replay(replayed) =
            state.record_completion_obligation(input).unwrap()
        else {
            panic!("each exact listener replay must be classified as replay")
        };
        assert_eq!(replayed, recorded);
    }

    for conflict in [
        obligation(
            ADMISSION_ID,
            ROOT_UUID,
            "ab_conflicting_event",
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ),
        obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            "changed-owner-session",
            GENERATION_ID,
        ),
        obligation(
            ADMISSION_ID,
            CHILD_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ),
        obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            "55555555-5555-4555-8555-555555555555",
        ),
        obligation(
            "other-admission",
            ROOT_UUID,
            EVENT_ID,
            CHILD_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ),
    ] {
        assert_immutable_conflict_without_mutation(&state, conflict, &expected);
    }
}

#[test]
fn sidecar_generation_states_preserve_absent_missing_matching_and_mismatched() {
    let state = state_with_root();
    let absent = state.completion_obligation_authority(ADMISSION_ID).unwrap();
    assert_eq!(
        absent.sidecar_generation_state(None),
        SidecarGenerationState::NoAdmittedObligation
    );

    state
        .record_completion_obligation(obligation(
            ADMISSION_ID,
            ROOT_UUID,
            EVENT_ID,
            ROOT_UUID,
            OWNER_SESSION_ID,
            GENERATION_ID,
        ))
        .unwrap();
    let admitted = state.completion_obligation_authority(ADMISSION_ID).unwrap();
    assert_eq!(
        admitted.sidecar_generation_state(None),
        SidecarGenerationState::ExpectedButUnobserved {
            expected: GENERATION_ID.to_string()
        }
    );
    assert_eq!(
        admitted.sidecar_generation_state(Some(GENERATION_ID)),
        SidecarGenerationState::Matching {
            expected: GENERATION_ID.to_string(),
            observed: GENERATION_ID.to_string(),
        }
    );
    assert_eq!(
        admitted.sidecar_generation_state(Some("replacement-generation")),
        SidecarGenerationState::Mismatched {
            expected: GENERATION_ID.to_string(),
            observed: "replacement-generation".to_string(),
        }
    );
}

#[test]
fn listener_settlement_and_recovery_authorities_remain_distinct() {
    let verifier = SettlementVerifierIdentity::new("transport:delivery-1").unwrap();
    let classes = [
        ListenerSettlementClass::PendingOrUnsettled,
        ListenerSettlementClass::VerifiedTransportDelivery {
            verifier: verifier.clone(),
        },
        ListenerSettlementClass::ExactOwnerConsumption {
            verifier: verifier.clone(),
        },
        ListenerSettlementClass::ManualOrAdminAcknowledgement {
            verifier: verifier.clone(),
        },
        ListenerSettlementClass::ExplicitAbandonment {
            verifier: verifier.clone(),
        },
        ListenerSettlementClass::ExplicitWaiver {
            verifier: verifier.clone(),
        },
        ListenerSettlementClass::UnknownOrInvalidAuthority {
            verifier: Some(verifier.clone()),
        },
    ];
    for (index, class) in classes.iter().enumerate() {
        assert!(
            classes.iter().skip(index + 1).all(|other| class != other),
            "settlement authority classes must not collapse: {class:?}"
        );
    }
    assert_eq!(verifier.as_str(), "transport:delivery-1");
    assert_ne!(
        RecoveryDisposition::Abandoned {
            authority: verifier.clone()
        },
        RecoveryDisposition::Waived {
            authority: verifier
        }
    );
}

#[test]
fn exact_owner_and_recursive_descendant_owner_are_distinct() {
    let state = state_with_lineage();
    assert_eq!(
        state
            .owner_lineage_relationship(ROOT_UUID, ROOT_UUID)
            .unwrap(),
        OwnerLineageRelationship::ExactOwner
    );
    assert_eq!(
        state
            .owner_lineage_relationship(ROOT_UUID, CHILD_UUID)
            .unwrap(),
        OwnerLineageRelationship::RecursiveDescendant { depth: 1 }
    );
    assert_eq!(
        state
            .owner_lineage_relationship(ROOT_UUID, GRANDCHILD_UUID)
            .unwrap(),
        OwnerLineageRelationship::RecursiveDescendant { depth: 2 }
    );
    assert_eq!(
        state
            .owner_lineage_relationship("missing", ROOT_UUID)
            .unwrap(),
        OwnerLineageRelationship::UnknownOrInvalidAuthority
    );
}

#[test]
fn raw_continuation_acceptance_and_direct_tuple_have_no_historical_authority_api() {
    let mut state = state_with_root();
    let accepted = state
        .accept_continuation(&ContinuationAcceptInput {
            logical_request_key: "age299-s1-historical-authority".to_string(),
            fingerprint: "validated-fingerprint".to_string(),
            origin_invocation_id: ROOT_UUID.to_string(),
        })
        .unwrap();
    let ContinuationAcceptResult::Accepted(continuation) = accepted else {
        panic!("new continuation must be accepted")
    };
    assert_eq!(continuation.resume.parent_invocation_id, ROOT_UUID);

    let exact_tuple: (String, String, String, String, String) = state
        .connection()
        .query_row(
            "SELECT continuation_id, validated_fingerprint,
                    resume_parent_invocation_id, resume_invocation_id,
                    fresh_invocation_id
             FROM fresh_continuations
             WHERE continuation_id = ?1",
            [&continuation.continuation_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(exact_tuple.0, continuation.continuation_id);
    assert_eq!(exact_tuple.1, "validated-fingerprint");
    assert_eq!(exact_tuple.2, ROOT_UUID);
    assert_eq!(exact_tuple.3, continuation.resume.invocation_id);
    assert_eq!(exact_tuple.4, continuation.fresh.invocation_id);
}

fn build_schema_13_database(path: &Path) {
    let mut connection = Connection::open(path).unwrap();
    let plan = migrations::plan(0, 13).unwrap();
    migrations::run_with_db_path(&mut connection, &plan, path.to_path_buf()).unwrap();
    assert_eq!(user_version(&connection), 13);
    connection
        .execute(
            "INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, success, exit_code, error_category, terminal_reason,
                created_at, finished_at
             ) VALUES (?1, 'model', 'provider', 0, 'succeeded', 1, 0, NULL,
                       'completed_before_v14', '2026-08-13T00:00:00Z',
                       '2026-08-13T00:01:00Z')",
            [ROOT_UUID],
        )
        .unwrap();
}

fn state_with_root() -> StateDb {
    let state = StateDb::open(Path::new(":memory:")).unwrap();
    state
        .start_invocation(&invocation(ROOT_UUID, None))
        .unwrap();
    state
}

fn state_with_lineage() -> StateDb {
    let state = state_with_root();
    let root_id = state.get_invocation_by_uuid(ROOT_UUID).unwrap().unwrap().id;
    state
        .start_invocation(&invocation(CHILD_UUID, Some(root_id)))
        .unwrap();
    let child_id = state
        .get_invocation_by_uuid(CHILD_UUID)
        .unwrap()
        .unwrap()
        .id;
    state
        .start_invocation(&invocation(GRANDCHILD_UUID, Some(child_id)))
        .unwrap();
    state
}

fn invocation(invocation_uuid: &str, parent_invocation_id: Option<i64>) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation_uuid.to_string(),
        model_name: "age299-s1".to_string(),
        provider_name: "test-provider".to_string(),
        provider_index: 0,
        parent_invocation_id,
    }
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

fn assert_immutable_conflict_without_mutation(
    state: &StateDb,
    conflict: CompletionObligationAdmission<'_>,
    expected: &[oulipoly_state::CompletionObligationExpectation],
) {
    let error = state.record_completion_obligation(conflict).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("conflicts with immutable admission"),
        "{error}"
    );
    assert_eq!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap(),
        expected
    );
}

fn assert_every_direct_update_is_rejected(state: &StateDb, admission_id: &str) {
    for (field, replacement) in [
        ("admission_id", "replacement-admission"),
        ("invocation_uuid", CHILD_UUID),
        ("event_id", "replacement-event"),
        ("owner_invocation_uuid", GRANDCHILD_UUID),
        ("owner_session_id", "replacement-owner-session"),
        (
            "expected_sidecar_generation",
            "55555555-5555-4555-8555-555555555555",
        ),
        ("admitted_at", "2026-08-13T00:05:00Z"),
    ] {
        let before = raw_completion_obligation_rows(state);
        let error = state
            .connection()
            .execute(
                &format!(
                    "UPDATE invocation_completion_obligations
                     SET {field} = ?1 WHERE admission_id = ?2"
                ),
                rusqlite::params![replacement, admission_id],
            )
            .unwrap_err();
        assert!(
            error.to_string().contains(APPEND_ONLY_UPDATE_ERROR),
            "{field}: {error}"
        );
        assert_eq!(
            raw_completion_obligation_rows(state),
            before,
            "rejected update of {field} changed a prior row"
        );
    }
}

fn assert_direct_delete_is_rejected(state: &StateDb, admission_id: &str) {
    let before = raw_completion_obligation_rows(state);
    let error = state
        .connection()
        .execute(
            "DELETE FROM invocation_completion_obligations WHERE admission_id = ?1",
            [admission_id],
        )
        .unwrap_err();
    assert!(
        error.to_string().contains(APPEND_ONLY_DELETE_ERROR),
        "{error}"
    );
    assert_eq!(
        raw_completion_obligation_rows(state),
        before,
        "rejected delete changed a prior row"
    );
}

fn assert_null_admission_insert_forms_are_rejected(
    state: &StateDb,
    event_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
) {
    for (operation, statement) in [
        (
            "ordinary insert",
            "INSERT INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6)",
        ),
        (
            "insert or replace",
            "INSERT OR REPLACE INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6)",
        ),
        (
            "upsert",
            "INSERT INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (admission_id) DO UPDATE SET event_id = excluded.event_id",
        ),
    ] {
        let before = raw_completion_obligation_rows(state);
        let error = state
            .connection()
            .execute(
                statement,
                rusqlite::params![
                    ROOT_UUID,
                    event_id,
                    owner_invocation_uuid,
                    owner_session_id,
                    GENERATION_ID,
                    "2026-08-13T00:06:00Z",
                ],
            )
            .unwrap_err();
        assert!(
            error.to_string().contains(NULL_ADMISSION_ERROR),
            "{operation}: {error}"
        );
        assert_eq!(
            raw_completion_obligation_rows(state),
            before,
            "rejected {operation} with NULL admission identity changed prior rows"
        );
    }
}

fn assert_null_admission_conflict_actions_are_rejected(
    state: &StateDb,
    event_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
) {
    for (operation, statement) in [
        (
            "conflicting insert or replace",
            "INSERT OR REPLACE INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6)",
        ),
        (
            "conflicting upsert",
            "INSERT INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (NULL, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (event_id, owner_invocation_uuid)
             DO UPDATE SET admission_id = excluded.admission_id",
        ),
    ] {
        let before = raw_completion_obligation_rows(state);
        let error = state
            .connection()
            .execute(
                statement,
                rusqlite::params![
                    ROOT_UUID,
                    event_id,
                    owner_invocation_uuid,
                    owner_session_id,
                    GENERATION_ID,
                    "2026-08-13T00:07:00Z",
                ],
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("completion obligation immutable identity conflict"),
            "{operation}: {error}"
        );
        assert_eq!(
            raw_completion_obligation_rows(state),
            before,
            "rejected {operation} with NULL admission identity changed prior rows"
        );
    }
}

fn assert_non_text_insert_forms_are_rejected(
    state: &StateDb,
    event_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
) {
    for field in STRING_DECODED_FIELDS {
        for malformed in malformed_storage_values() {
            let admission_id = match &malformed {
                Value::Blob(_) if field == "admission_id" => NON_TEXT_ADMISSION_ID,
                Value::Integer(_) if field == "admission_id" => NUMERIC_ADMISSION_ID,
                _ => NON_TEXT_ADMISSION_ID,
            };
            let mut values = completion_obligation_values(
                admission_id,
                event_id,
                owner_invocation_uuid,
                owner_session_id,
                "2026-08-13T00:08:00Z",
            );
            values[field_index(field)] = malformed.clone();

            for (operation, statement) in non_conflicting_insert_statements() {
                assert_malformed_statement_is_rejected(
                    state,
                    field,
                    operation,
                    statement,
                    &values,
                    &format!("completion_obligation_{field}_text"),
                );
            }
        }
    }
}

fn assert_non_text_conflict_actions_are_rejected(
    state: &StateDb,
    admission_id: &str,
    event_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
) {
    for field in STRING_DECODED_FIELDS {
        for malformed in malformed_storage_values() {
            let mut values = completion_obligation_values(
                admission_id,
                event_id,
                owner_invocation_uuid,
                owner_session_id,
                "2026-08-13T00:09:00Z",
            );
            values[field_index(field)] = malformed;

            for (operation, statement) in conflicting_insert_statements() {
                assert_malformed_statement_is_rejected(
                    state,
                    field,
                    operation,
                    statement,
                    &values,
                    "completion obligation immutable identity conflict",
                );
            }
        }
    }
}

fn assert_equivalent_blob_admission_is_rejected(state: &StateDb, text_admission_id: &str) {
    let before = raw_completion_obligation_rows(state);
    let authority_before = state
        .completion_obligation_authority(text_admission_id)
        .unwrap();
    let values = [
        Value::Blob(text_admission_id.as_bytes().to_vec()),
        Value::Text(ROOT_UUID.to_string()),
        Value::Text("equivalent-blob-event".to_string()),
        Value::Text(GRANDCHILD_UUID.to_string()),
        Value::Text("equivalent-blob-session".to_string()),
        Value::Text(GENERATION_ID.to_string()),
        Value::Text("2026-08-13T00:10:00Z".to_string()),
    ];

    let error = state
        .connection()
        .execute(
            COMPLETION_OBLIGATION_INSERT,
            params_from_iter(values.iter()),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("completion_obligation_admission_id_text"),
        "equivalent BLOB admission identity: {error}"
    );
    assert_eq!(raw_completion_obligation_rows(state), before);
    assert_eq!(
        state
            .completion_obligation_authority(text_admission_id)
            .unwrap(),
        authority_before,
        "BLOB and equivalent TEXT admission identities must not coexist"
    );
    assert_eq!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap()
            .len(),
        before.len(),
        "equivalent BLOB rejection must preserve typed invocation lookup"
    );
}

fn assert_malformed_statement_is_rejected(
    state: &StateDb,
    field: &str,
    operation: &str,
    statement: &str,
    values: &[Value],
    expected_error: &str,
) {
    let before = raw_completion_obligation_rows(state);
    let typed_before = state
        .completion_obligations_for_invocation(ROOT_UUID)
        .unwrap();
    let blob_text_before = state
        .completion_obligation_authority(NON_TEXT_ADMISSION_ID)
        .unwrap();
    let numeric_text_before = state
        .completion_obligation_authority(NUMERIC_ADMISSION_ID)
        .unwrap();

    let error = state
        .connection()
        .execute(statement, params_from_iter(values.iter()))
        .unwrap_err();
    assert!(
        error.to_string().contains(expected_error),
        "{operation} with non-text {field}: {error}"
    );
    assert_eq!(
        raw_completion_obligation_rows(state),
        before,
        "rejected {operation} with non-text {field} changed the complete prior table"
    );
    assert_eq!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap(),
        typed_before,
        "rejected {operation} with non-text {field} changed typed invocation projection"
    );
    for expectation in typed_before {
        assert_eq!(
            state
                .completion_obligation_authority(&expectation.admission_id)
                .unwrap(),
            CompletionObligationAuthority::Admitted(expectation),
            "rejected {operation} with non-text {field} changed typed admission lookup"
        );
    }
    assert_eq!(
        state
            .completion_obligation_authority(NON_TEXT_ADMISSION_ID)
            .unwrap(),
        blob_text_before,
        "BLOB and equivalent TEXT admission identities must not coexist"
    );
    assert_eq!(
        state
            .completion_obligation_authority(NUMERIC_ADMISSION_ID)
            .unwrap(),
        numeric_text_before,
        "numeric and equivalent TEXT admission identities must not coexist"
    );
}

fn malformed_storage_values() -> [Value; 2] {
    [
        Value::Blob(NON_TEXT_ADMISSION_ID.as_bytes().to_vec()),
        Value::Integer(299),
    ]
}

fn completion_obligation_values(
    admission_id: &str,
    event_id: &str,
    owner_invocation_uuid: &str,
    owner_session_id: &str,
    admitted_at: &str,
) -> Vec<Value> {
    [
        admission_id,
        ROOT_UUID,
        event_id,
        owner_invocation_uuid,
        owner_session_id,
        GENERATION_ID,
        admitted_at,
    ]
    .into_iter()
    .map(|value| Value::Text(value.to_string()))
    .collect()
}

fn field_index(field: &str) -> usize {
    STRING_DECODED_FIELDS
        .iter()
        .position(|candidate| *candidate == field)
        .unwrap()
}

fn non_conflicting_insert_statements() -> [(&'static str, &'static str); 3] {
    [
        ("ordinary insert", COMPLETION_OBLIGATION_INSERT),
        ("insert or replace", COMPLETION_OBLIGATION_REPLACE),
        (
            "admission-key upsert",
            concat!(
                "INSERT INTO invocation_completion_obligations (",
                "admission_id, invocation_uuid, event_id, owner_invocation_uuid, ",
                "owner_session_id, expected_sidecar_generation, admitted_at",
                ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ",
                "ON CONFLICT (admission_id) DO UPDATE SET event_id = excluded.event_id"
            ),
        ),
    ]
}

fn conflicting_insert_statements() -> [(&'static str, &'static str); 2] {
    [
        (
            "conflicting insert or replace",
            COMPLETION_OBLIGATION_REPLACE,
        ),
        (
            "listener-key upsert",
            concat!(
                "INSERT INTO invocation_completion_obligations (",
                "admission_id, invocation_uuid, event_id, owner_invocation_uuid, ",
                "owner_session_id, expected_sidecar_generation, admitted_at",
                ") VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ",
                "ON CONFLICT (event_id, owner_invocation_uuid) ",
                "DO UPDATE SET admission_id = excluded.admission_id"
            ),
        ),
    ]
}

fn direct_insert_completion_obligation(
    state: &StateDb,
    input: CompletionObligationAdmission<'_>,
    admitted_at: &str,
) -> rusqlite::Result<usize> {
    state.connection().execute(
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
            admitted_at,
        ],
    )
}

fn raw_completion_obligation_rows(state: &StateDb) -> Vec<RawCompletionObligationRow> {
    let mut statement = state
        .connection()
        .prepare(
            "SELECT CAST(admission_id AS BLOB), CAST(invocation_uuid AS BLOB),
                    CAST(event_id AS BLOB), CAST(owner_invocation_uuid AS BLOB),
                    CAST(owner_session_id AS BLOB),
                    CAST(expected_sidecar_generation AS BLOB),
                    CAST(admitted_at AS BLOB)
             FROM invocation_completion_obligations
             ORDER BY admission_id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn ownership_schema(state: &StateDb) -> Vec<String> {
    let mut statement = state
        .connection()
        .prepare(
            "SELECT sql FROM sqlite_schema
             WHERE name IN (
                 'invocation_completion_obligations',
                 'idx_invocation_completion_obligations_invocation',
                 'trg_invocation_completion_obligations_generation_insert',
                 'trg_invocation_completion_obligations_append_only_update',
                 'trg_invocation_completion_obligations_append_only_delete'
             )
             ORDER BY type, name",
        )
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn invocation_projection(connection: &Connection) -> Vec<(String, String, Option<i64>, String)> {
    let mut statement = connection
        .prepare(
            "SELECT invocation_uuid, status, success, terminal_reason
             FROM invocations ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn table_exists(connection: &Connection, table: &str) -> bool {
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

fn user_version(connection: &Connection) -> i32 {
    connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}
