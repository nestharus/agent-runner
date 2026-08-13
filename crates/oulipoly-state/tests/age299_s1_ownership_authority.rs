use oulipoly_state::continuation::{ContinuationAcceptInput, ContinuationAcceptResult};
use oulipoly_state::migrations;
use oulipoly_state::repositories::ContinuationRepository;
use oulipoly_state::{
    CURRENT_SCHEMA_VERSION, CompletionObligationAdmission, CompletionObligationAdmissionResult,
    CompletionObligationAuthority, HistoricalParentAuthorityClaim, InvocationParentAdmission,
    InvocationStart, ListenerSettlementClass, OwnerLineageRelationship, RecoveryDisposition,
    RunningParentAdmission, SettlementVerifierIdentity, SidecarGenerationState, StateDb,
};
use rusqlite::Connection;
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
        sql.contains("UNIQUE (event_id, owner_invocation_uuid)")
            && sql.contains("REFERENCES invocations(invocation_uuid)")
            && sql.contains("owner_session_id TEXT NOT NULL")
            && sql.contains("expected_sidecar_generation TEXT NOT NULL")
    }));
    assert_eq!(
        ownership_schema(&fresh)
            .iter()
            .filter(|sql| sql.contains("completion event sidecar generation conflict"))
            .count(),
        2,
        "insert and update must preserve one immutable generation per event"
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
fn schema_rejects_mixed_event_generations_without_mutation() {
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

    let insert_error = state
        .connection()
        .execute(
            "INSERT INTO invocation_completion_obligations (
                admission_id, invocation_uuid, event_id, owner_invocation_uuid,
                owner_session_id, expected_sidecar_generation, admitted_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                SECOND_ADMISSION_ID,
                ROOT_UUID,
                EVENT_ID,
                GRANDCHILD_UUID,
                SECOND_OWNER_SESSION_ID,
                "mismatched-generation",
                "2026-08-13T00:02:00Z",
            ],
        )
        .unwrap_err();
    assert!(
        insert_error
            .to_string()
            .contains("completion event sidecar generation conflict"),
        "{insert_error}"
    );
    assert_eq!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap(),
        vec![first_recorded.clone()]
    );

    let second = obligation(
        SECOND_ADMISSION_ID,
        ROOT_UUID,
        EVENT_ID,
        GRANDCHILD_UUID,
        SECOND_OWNER_SESSION_ID,
        GENERATION_ID,
    );
    let CompletionObligationAdmissionResult::Recorded(second_recorded) =
        state.record_completion_obligation(second).unwrap()
    else {
        panic!("matching sibling listener admission must be recorded")
    };
    let expected = vec![first_recorded, second_recorded];

    let update_error = state
        .connection()
        .execute(
            "UPDATE invocation_completion_obligations
             SET expected_sidecar_generation = 'mismatched-generation'
             WHERE admission_id = ?1",
            [SECOND_ADMISSION_ID],
        )
        .unwrap_err();
    assert!(
        update_error
            .to_string()
            .contains("completion event sidecar generation conflict"),
        "{update_error}"
    );
    assert_eq!(
        state
            .completion_obligations_for_invocation(ROOT_UUID)
            .unwrap(),
        expected
    );
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
fn historical_parent_admission_requires_exact_durable_authority() {
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

    let running = InvocationParentAdmission::RequireRunning(RunningParentAdmission);
    let historical = state
        .historical_parent_admission(HistoricalParentAuthorityClaim {
            continuation_id: &continuation.continuation_id,
            parent_invocation_uuid: ROOT_UUID,
            child_invocation_uuid: &continuation.resume.invocation_id,
        })
        .unwrap()
        .expect("exact durable continuation identity authorizes association");
    assert_eq!(historical.parent_invocation_uuid(), ROOT_UUID);
    assert_eq!(
        historical.child_invocation_uuid(),
        continuation.resume.invocation_id
    );
    assert_eq!(historical.continuation_id(), continuation.continuation_id);
    assert_ne!(
        running,
        InvocationParentAdmission::Historical(historical.clone())
    );

    for claim in [
        HistoricalParentAuthorityClaim {
            continuation_id: "resume-shaped-command",
            parent_invocation_uuid: ROOT_UUID,
            child_invocation_uuid: &continuation.resume.invocation_id,
        },
        HistoricalParentAuthorityClaim {
            continuation_id: &continuation.continuation_id,
            parent_invocation_uuid: ROOT_UUID,
            child_invocation_uuid: "bare-parent-env-derived-child",
        },
        HistoricalParentAuthorityClaim {
            continuation_id: &continuation.continuation_id,
            parent_invocation_uuid: CHILD_UUID,
            child_invocation_uuid: &continuation.resume.invocation_id,
        },
    ] {
        assert_eq!(state.historical_parent_admission(claim).unwrap(), None);
    }
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

fn ownership_schema(state: &StateDb) -> Vec<String> {
    let mut statement = state
        .connection()
        .prepare(
            "SELECT sql FROM sqlite_schema
             WHERE name IN (
                'invocation_completion_obligations',
                'idx_invocation_completion_obligations_invocation',
                'trg_invocation_completion_obligations_generation_insert',
                'trg_invocation_completion_obligations_generation_update'
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
