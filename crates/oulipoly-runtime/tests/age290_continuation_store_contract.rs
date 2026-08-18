//! ## Declared roles
//!
//! `orchestration`, `validator`, `accessor`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age290_continuation_store_contract.rs
//!     role: adapter
//!     Translates:
//!       - fresh-continuation-store-behavior-contract
//!       - file-backed-SQLite-test-fixture-contract
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use oulipoly_runtime::fresh_continuation::{
    AcceptDecision, AcceptedContinuation, ArtifactIdentity, ContinuationArtifactSource,
    ContinuationBlock, ContinuationBlockKind, ContinuationEvidence, ContinuationEvidenceValidator,
    ContinuationStore, DefaultContinuationEvidenceValidator, FreshContinuationOutcome,
    FreshContinuationRequest, HistoricalParentAuthorityClaim, InvocationDisposition,
    InvocationOutcome, InvocationParentAdmission, PublishedHandoff, ReservedInvocation,
    ResumeAcceptance, RunDecision, StateDbContinuationStore, ValidatedContinuation,
};
use oulipoly_state::continuation::{ContinuationAcceptInput, ContinuationAcceptResult};
use oulipoly_state::repositories::ContinuationRepository;
use oulipoly_state::{InvocationStart, InvocationStatus, StateDb};
use sha2::{Digest, Sha256};

#[test]
fn accept_reuses_exact_reserved_identities_for_the_same_validated_context() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();

    let first = store.accept(&context).expect("accept continuation");
    let continuation = accepted(&first);

    assert!(!continuation.continuation_id.is_empty());
    assert!(!continuation.resume.invocation_id.is_empty());
    assert!(!continuation.fresh.invocation_id.is_empty());
    assert_eq!(
        continuation.resume.parent_invocation_id,
        context.request().origin_invocation_id
    );
    assert_eq!(
        continuation.fresh.parent_invocation_id,
        continuation.resume.invocation_id
    );

    let repeated = store.accept(&context).expect("repeat accept");

    assert_eq!(repeated, first);
}

#[test]
fn reserved_invocation_identities_are_valid_distinct_uuids() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();

    let continuation = accept(&mut store, &context);

    assert_eq!(
        continuation.resume.parent_invocation_id,
        context.request().origin_invocation_id
    );
    assert_eq!(
        continuation.fresh.parent_invocation_id,
        continuation.resume.invocation_id
    );

    let resume_invocation_id = uuid::Uuid::parse_str(&continuation.resume.invocation_id)
        .expect("reserved resume invocation identity must be a valid UUID");
    let fresh_invocation_id = uuid::Uuid::parse_str(&continuation.fresh.invocation_id)
        .expect("reserved fresh invocation identity must be a valid UUID");

    assert_ne!(resume_invocation_id, fresh_invocation_id);
}

#[test]
fn ordinary_parent_admission_still_defaults_to_require_running() {
    assert!(matches!(
        InvocationParentAdmission::default(),
        InvocationParentAdmission::RequireRunning(_)
    ));
}

#[test]
fn accept_rejects_a_changed_fingerprint_for_the_same_logical_request() {
    let fixture = Fixture::new();
    let context = fixture.context_with_target("request-1", "fresh-model");
    let conflicting = fixture.context_with_target("request-1", "different-model");
    let mut store = fixture.open_store();
    store.accept(&context).expect("initial accept");

    let error = store
        .accept(&conflicting)
        .expect_err("changed fingerprint must conflict");

    assert_eq!(error.kind, ContinuationBlockKind::Conflict);
}

#[test]
fn simultaneous_accepts_converge_on_one_exact_reservation() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let stores = (0..4).map(|_| fixture.open_store()).collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(stores.len()));

    let continuations = std::thread::scope(|scope| {
        let handles = stores
            .into_iter()
            .map(|mut store| {
                let context = context.clone();
                let barrier = Arc::clone(&barrier);
                scope.spawn(move || {
                    barrier.wait();
                    accept(&mut store, &context)
                })
            })
            .collect::<Vec<_>>();

        handles
            .into_iter()
            .map(|handle| handle.join().expect("accept thread"))
            .collect::<Vec<_>>()
    });

    assert!(
        continuations
            .iter()
            .all(|continuation| continuation == &continuations[0])
    );
}

#[test]
fn malformed_recorded_outcome_blocks_during_accept() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    store
        .begin_resume(&continuation)
        .expect("begin reserved resume");
    store
        .record_resume(
            &continuation,
            &unconfirmed_resume(&continuation.resume.invocation_id),
        )
        .expect("record resume");
    drop(store);

    let state = StateDb::open(&fixture.state_path).expect("open state for corruption check");
    let connection = rusqlite::Connection::open(state.path()).unwrap();
    let constrained = connection.execute(
        "UPDATE fresh_continuations SET resume_outcome_json = '{' WHERE continuation_id = ?1",
        [&continuation.continuation_id],
    );
    assert!(constrained.is_err(), "schema must reject malformed JSON");
    connection
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable corruption injection");
    connection
        .execute(
            "UPDATE fresh_continuations SET resume_outcome_json = '{' WHERE continuation_id = ?1",
            [&continuation.continuation_id],
        )
        .expect("inject malformed durable outcome");
    drop(state);

    let mut reopened = fixture.open_store();
    let error = reopened
        .accept(&context)
        .expect_err("corrupt outcome must block during accept");

    assert_eq!(error.kind, ContinuationBlockKind::AmbiguousState);
}

#[test]
fn invalid_persisted_reservation_identity_blocks_during_accept() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    drop(store);

    let state = StateDb::open(&fixture.state_path).expect("open state for corruption check");
    rusqlite::Connection::open(state.path())
        .unwrap()
        .execute(
            "UPDATE fresh_continuations
                SET resume_invocation_id = 'not-a-uuid',
                    fresh_parent_invocation_id = 'not-a-uuid'
              WHERE continuation_id = ?1",
            [&continuation.continuation_id],
        )
        .expect("inject invalid parent-consistent reservation identity");
    drop(state);

    let mut reopened = fixture.open_store();
    let error = reopened
        .accept(&context)
        .expect_err("invalid durable reservation must block during accept");

    assert_eq!(error.kind, ContinuationBlockKind::AmbiguousState);
}

#[test]
fn fresh_stage_before_resume_completion_blocks_during_accept() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    drop(store);

    let state = StateDb::open(&fixture.state_path).expect("open state for corruption check");
    let connection = rusqlite::Connection::open(state.path()).unwrap();
    let constrained = connection.execute(
        "UPDATE fresh_continuations SET fresh_stage = 'running' WHERE continuation_id = ?1",
        [&continuation.continuation_id],
    );
    assert!(
        constrained.is_err(),
        "schema must reject fresh progress before durable resume completion"
    );
    connection
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable corruption injection");
    connection
        .execute(
            "UPDATE fresh_continuations SET fresh_stage = 'running' WHERE continuation_id = ?1",
            [&continuation.continuation_id],
        )
        .expect("inject impossible stage ordering");
    drop(state);

    let mut reopened = fixture.open_store();
    let error = reopened
        .accept(&context)
        .expect_err("impossible stage order must block during accept");

    assert_eq!(error.kind, ContinuationBlockKind::AmbiguousState);
}

#[test]
fn resume_reservation_is_observed_with_the_same_identity_after_restart() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);

    let first = store
        .begin_resume(&continuation)
        .expect("begin reserved resume");

    assert_eq!(first, RunDecision::Run(continuation.resume.clone()));

    drop(store);
    let mut reopened = fixture.open_store();
    let after_restart = reopened
        .begin_resume(&continuation)
        .expect("observe reserved resume after restart");

    assert_eq!(
        after_restart,
        RunDecision::Observe(continuation.resume.clone())
    );
}

#[test]
fn fresh_waits_for_the_exact_resume_and_is_observed_with_the_same_identity_after_restart() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    store
        .begin_resume(&continuation)
        .expect("begin reserved resume");
    let mut wrong_resume = unconfirmed_resume(&continuation.resume.invocation_id);
    wrong_resume.invocation_id = "another-resume-invocation".to_string();

    let mismatch = store
        .record_resume(&continuation, &wrong_resume)
        .expect_err("mismatched resume identity must conflict");
    let premature_fresh = store
        .begin_fresh(&continuation)
        .expect_err("fresh cannot begin before the reserved resume is recorded");

    assert_eq!(mismatch.kind, ContinuationBlockKind::Conflict);
    assert_eq!(premature_fresh.kind, ContinuationBlockKind::AmbiguousState);

    let resume = unconfirmed_resume(&continuation.resume.invocation_id);
    store
        .record_resume(&continuation, &resume)
        .expect("record exact resume outcome");
    let first_fresh = store
        .begin_fresh(&continuation)
        .expect("begin reserved fresh invocation");

    assert_eq!(first_fresh, RunDecision::Run(continuation.fresh.clone()));

    drop(store);
    let mut reopened = fixture.open_store();
    let after_restart = reopened
        .begin_fresh(&continuation)
        .expect("observe reserved fresh invocation after restart");

    assert_eq!(
        after_restart,
        RunDecision::Observe(continuation.fresh.clone())
    );
}

#[test]
fn fresh_rejects_a_recorded_resume_that_does_not_meet_the_trigger() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    store
        .begin_resume(&continuation)
        .expect("begin reserved resume");
    let resume = successful_resume(&continuation.resume.invocation_id);
    store
        .record_resume(&continuation, &resume)
        .expect("record successful resume outcome");

    let error = store
        .begin_fresh(&continuation)
        .expect_err("successful resume must not trigger a fresh continuation");

    assert_eq!(error.kind, ContinuationBlockKind::Conflict);
}

#[test]
fn outcome_replays_are_idempotent_only_for_the_same_exact_value() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    store
        .begin_resume(&continuation)
        .expect("begin reserved resume");
    let resume = unconfirmed_resume(&continuation.resume.invocation_id);

    store
        .record_resume(&continuation, &resume)
        .expect("record exact resume outcome");
    store
        .record_resume(&continuation, &resume)
        .expect("repeat exact resume outcome");
    let mut conflicting_resume = resume.clone();
    conflicting_resume.session_id = Some("different-session".to_string());
    let resume_conflict = store
        .record_resume(&continuation, &conflicting_resume)
        .expect_err("different resume replay must conflict");

    assert_eq!(resume_conflict.kind, ContinuationBlockKind::Conflict);

    store
        .begin_fresh(&continuation)
        .expect("begin reserved fresh invocation");
    let fresh = successful_fresh(&continuation.fresh.invocation_id);
    store
        .record_fresh(&continuation, &fresh)
        .expect("record exact fresh outcome");
    store
        .record_fresh(&continuation, &fresh)
        .expect("repeat exact fresh outcome");
    let mut conflicting_fresh = fresh.clone();
    conflicting_fresh.physical_exit_code = 9;
    let fresh_conflict = store
        .record_fresh(&continuation, &conflicting_fresh)
        .expect_err("different fresh replay must conflict");

    assert_eq!(fresh_conflict.kind, ContinuationBlockKind::Conflict);
}

#[test]
fn successful_fresh_handoff_is_replayed_exactly_after_restart() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let handoff = fixture.handoff();
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    store
        .begin_resume(&continuation)
        .expect("begin reserved resume");
    let resume = unconfirmed_resume(&continuation.resume.invocation_id);
    store
        .record_resume(&continuation, &resume)
        .expect("record exact resume outcome");
    store
        .begin_fresh(&continuation)
        .expect("begin reserved fresh invocation");
    let fresh = successful_fresh(&continuation.fresh.invocation_id);
    store
        .record_fresh(&continuation, &fresh)
        .expect("record exact fresh outcome");

    let terminal = store
        .finish(&continuation, &handoff)
        .expect("finish successful continuation");

    assert_eq!(
        terminal,
        FreshContinuationOutcome::Continued {
            continuation_id: continuation.continuation_id.clone(),
            resume: resume.clone(),
            fresh: fresh.clone(),
            handoff: handoff.clone(),
        }
    );

    drop(store);
    let mut reopened = fixture.open_store();
    let replay = reopened.accept(&context).expect("replay after restart");

    assert_eq!(replay, AcceptDecision::Replay(Box::new(terminal)));
}

#[test]
fn failed_fresh_outcome_preserves_both_results_and_is_replayed_exactly() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let handoff = fixture.handoff();
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    store
        .begin_resume(&continuation)
        .expect("begin reserved resume");
    let resume = unconfirmed_resume(&continuation.resume.invocation_id);
    store
        .record_resume(&continuation, &resume)
        .expect("record exact resume outcome");
    store
        .begin_fresh(&continuation)
        .expect("begin reserved fresh invocation");
    let fresh = failed_fresh(&continuation.fresh.invocation_id);
    store
        .record_fresh(&continuation, &fresh)
        .expect("record exact fresh failure");

    let terminal = store
        .finish(&continuation, &handoff)
        .expect("finish failed continuation");

    assert!(matches!(
        &terminal,
        FreshContinuationOutcome::Failed {
            continuation_id,
            resume: actual_resume,
            fresh: Some(actual_fresh),
            reason,
            ..
        } if continuation_id.as_str() == continuation.continuation_id.as_str()
            && actual_resume == &resume
            && actual_fresh == &fresh
            && reason.kind == ContinuationBlockKind::InvocationFailed
    ));

    drop(store);
    let mut reopened = fixture.open_store();
    let replay = reopened.accept(&context).expect("replay after restart");

    assert_eq!(replay, AcceptDecision::Replay(Box::new(terminal)));
}

#[test]
fn validated_store_acceptance_binds_historical_authority_to_every_exact_identity() {
    let fixture = Fixture::new();
    let context = fixture.context("request-1");
    let different_context = fixture.context("request-2");
    let state = StateDb::open(&fixture.state_path).expect("open origin state");
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: context.request().origin_invocation_id.clone(),
            model_name: "model".to_string(),
            provider_name: "provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .expect("seed origin invocation");
    let origin_row = state
        .get_invocation_by_uuid(&context.request().origin_invocation_id)
        .expect("query origin invocation")
        .expect("origin invocation row");
    state
        .finalize_invocation(origin_row.id, true, 0, None, None)
        .expect("historical authority must not require a running origin");
    assert_eq!(
        state
            .get_invocation_by_uuid(&context.request().origin_invocation_id)
            .unwrap()
            .unwrap()
            .status,
        InvocationStatus::Succeeded
    );
    drop(state);
    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);

    assert_exact_historical_authority(&store, &continuation, &continuation.resume);
    assert_eq!(
        store
            .historical_parent_admission(
                &continuation,
                HistoricalParentAuthorityClaim {
                    continuation_id: &continuation.continuation_id,
                    parent_invocation_uuid: &continuation.fresh.parent_invocation_id,
                    child_invocation_uuid: &continuation.fresh.invocation_id,
                },
            )
            .expect("fresh authority before parent exists"),
        None
    );
    let state = StateDb::open(&fixture.state_path).expect("open state for resume parent");
    let origin_row = state
        .get_invocation_by_uuid(&context.request().origin_invocation_id)
        .unwrap()
        .unwrap();
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: continuation.resume.invocation_id.clone(),
            model_name: "model".to_string(),
            provider_name: "provider".to_string(),
            provider_index: 0,
            parent_invocation_id: Some(origin_row.id),
        })
        .expect("seed exact resume parent for fresh reservation");
    drop(state);
    assert_exact_historical_authority(&store, &continuation, &continuation.fresh);

    let wrong_claims = [
        HistoricalParentAuthorityClaim {
            continuation_id: "wrong-continuation",
            parent_invocation_uuid: &continuation.resume.parent_invocation_id,
            child_invocation_uuid: &continuation.resume.invocation_id,
        },
        HistoricalParentAuthorityClaim {
            continuation_id: &continuation.continuation_id,
            parent_invocation_uuid: "wrong-parent",
            child_invocation_uuid: &continuation.resume.invocation_id,
        },
        HistoricalParentAuthorityClaim {
            continuation_id: &continuation.continuation_id,
            parent_invocation_uuid: &continuation.resume.parent_invocation_id,
            child_invocation_uuid: "wrong-child",
        },
        HistoricalParentAuthorityClaim {
            continuation_id: &continuation.continuation_id,
            parent_invocation_uuid: &continuation.resume.parent_invocation_id,
            child_invocation_uuid: &continuation.fresh.invocation_id,
        },
        HistoricalParentAuthorityClaim {
            continuation_id: &continuation.continuation_id,
            parent_invocation_uuid: &continuation.fresh.parent_invocation_id,
            child_invocation_uuid: &continuation.resume.invocation_id,
        },
    ];
    for claim in wrong_claims {
        assert_eq!(
            store
                .historical_parent_admission(&continuation, claim)
                .expect("historical authority lookup"),
            None
        );
    }

    let replayed_with_different_validation = AcceptedContinuation::without_historical_authority(
        continuation.continuation_id.clone(),
        different_context.clone(),
        continuation.resume.clone(),
        continuation.fresh.clone(),
    );
    assert_eq!(
        store
            .historical_parent_admission(
                &replayed_with_different_validation,
                HistoricalParentAuthorityClaim {
                    continuation_id: &continuation.continuation_id,
                    parent_invocation_uuid: &continuation.resume.parent_invocation_id,
                    child_invocation_uuid: &continuation.resume.invocation_id,
                },
            )
            .expect("historical authority lookup"),
        None
    );

    let exact_claim = HistoricalParentAuthorityClaim {
        continuation_id: &continuation.continuation_id,
        parent_invocation_uuid: &continuation.resume.parent_invocation_id,
        child_invocation_uuid: &continuation.resume.invocation_id,
    };
    let mut wrong_validated_request = continuation.clone();
    wrong_validated_request.context = different_context;
    let mut wrong_accepted_continuation = continuation.clone();
    wrong_accepted_continuation.continuation_id = "cross-request-continuation".to_string();
    let mut wrong_resume_reservation = continuation.clone();
    wrong_resume_reservation.resume.invocation_id = "cross-request-resume".to_string();
    let mut wrong_fresh_reservation = continuation.clone();
    wrong_fresh_reservation.fresh.invocation_id = "cross-request-fresh".to_string();
    for rebound in [
        wrong_validated_request,
        wrong_accepted_continuation,
        wrong_resume_reservation,
        wrong_fresh_reservation,
    ] {
        assert_eq!(
            store
                .historical_parent_admission(&rebound, exact_claim)
                .expect("cross-binding authority lookup"),
            None
        );
    }
}

#[test]
fn durable_continuation_id_rewrite_cannot_replace_validated_store_provenance() {
    let fixture = Fixture::new();
    let context = fixture.context("continuation-id-rewrite");
    let origin = StateDb::open(&fixture.state_path).expect("open origin state");
    origin
        .start_invocation(&InvocationStart {
            invocation_uuid: context.request().origin_invocation_id.clone(),
            model_name: "model".to_string(),
            provider_name: "provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .expect("seed origin invocation");
    drop(origin);

    let mut store = fixture.open_store();
    let continuation = accept(&mut store, &context);
    let original_id = continuation.continuation_id.clone();
    let replacement_id = "replacement-continuation-id";
    let writer = StateDb::open(&fixture.state_path).expect("open separate public writer");
    let connection = rusqlite::Connection::open(writer.path()).unwrap();
    let before = durable_continuation_non_id_fields(&writer, &original_id);

    let updated = connection
        .execute(
            "UPDATE fresh_continuations SET continuation_id = ?1 WHERE continuation_id = ?2",
            [replacement_id, &original_id],
        )
        .expect("rewrite only the durable continuation identity");
    assert_eq!(updated, 1);
    assert_eq!(
        durable_continuation_non_id_fields(&writer, replacement_id),
        before,
        "continuation ID rewrite changed non-ID durable fields"
    );

    let replacement_claim = HistoricalParentAuthorityClaim {
        continuation_id: replacement_id,
        parent_invocation_uuid: &continuation.resume.parent_invocation_id,
        child_invocation_uuid: &continuation.resume.invocation_id,
    };
    assert_eq!(
        store
            .historical_parent_admission(&continuation, replacement_claim)
            .expect("replacement claim lookup"),
        None,
        "a durable rewrite must not mint authority for the replacement identity"
    );
    let original_claim = HistoricalParentAuthorityClaim {
        continuation_id: &original_id,
        parent_invocation_uuid: &continuation.resume.parent_invocation_id,
        child_invocation_uuid: &continuation.resume.invocation_id,
    };
    assert_eq!(
        store
            .historical_parent_admission(&continuation, original_claim)
            .expect("original claim lookup while durable identity is absent"),
        None
    );

    let restored = connection
        .execute(
            "UPDATE fresh_continuations SET continuation_id = ?1 WHERE continuation_id = ?2",
            [&original_id, replacement_id],
        )
        .expect("restore the accepted durable continuation identity");
    assert_eq!(restored, 1);
    assert_eq!(
        durable_continuation_non_id_fields(&writer, &original_id),
        before,
        "continuation ID restoration changed non-ID durable fields"
    );

    let admission = store
        .historical_parent_admission(&continuation, original_claim)
        .expect("restored original claim lookup")
        .expect("restored provenance-bound durable identity must authorize");
    assert_eq!(admission.continuation_id(), original_id);
    assert_eq!(
        admission.parent_invocation_uuid(),
        continuation.resume.parent_invocation_id
    );
    assert_eq!(
        admission.child_invocation_uuid(),
        continuation.resume.invocation_id
    );
}

fn assert_exact_historical_authority(
    store: &StateDbContinuationStore,
    continuation: &AcceptedContinuation,
    reservation: &ReservedInvocation,
) {
    let authority = store
        .historical_parent_admission(
            continuation,
            HistoricalParentAuthorityClaim {
                continuation_id: &continuation.continuation_id,
                parent_invocation_uuid: &reservation.parent_invocation_id,
                child_invocation_uuid: &reservation.invocation_id,
            },
        )
        .expect("historical authority lookup")
        .expect("validated exact reservation must authorize its parent association");
    assert_eq!(
        authority.parent_invocation_uuid(),
        reservation.parent_invocation_id
    );
    assert_eq!(authority.child_invocation_uuid(), reservation.invocation_id);
    assert_eq!(authority.continuation_id(), continuation.continuation_id);
}

#[test]
fn raw_repository_acceptance_and_exact_durable_tuple_cannot_mint_historical_authority() {
    let fixture = Fixture::new();
    let context = fixture.context("request-1");
    let mut state = StateDb::open(&fixture.state_path).expect("open raw state repository");
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: context.request().origin_invocation_id.clone(),
            model_name: "model".to_string(),
            provider_name: "provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .expect("seed origin invocation");
    let raw = state
        .accept_continuation(&ContinuationAcceptInput {
            logical_request_key: "caller-chosen-logical-key".to_string(),
            fingerprint: context.fingerprint().to_string(),
            origin_invocation_id: context.request().origin_invocation_id.clone(),
        })
        .expect("raw continuation acceptance");
    let ContinuationAcceptResult::Accepted(record) = raw else {
        panic!("new raw continuation must be accepted")
    };
    let exact_tuple: (String, String, String, String, String, String, String) = state
        .connection()
        .query_row(
            "SELECT logical_request_key, validated_fingerprint, continuation_id,
                    resume_invocation_id, resume_parent_invocation_id,
                    fresh_invocation_id, fresh_parent_invocation_id
             FROM fresh_continuations WHERE continuation_id = ?1",
            [&record.continuation_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .expect("read exact durable tuple");
    assert_eq!(exact_tuple.0, "caller-chosen-logical-key");
    assert_eq!(exact_tuple.1, context.fingerprint());
    assert_eq!(exact_tuple.2, record.continuation_id);
    assert_eq!(exact_tuple.3, record.resume.invocation_id);
    assert_eq!(exact_tuple.4, record.resume.parent_invocation_id);
    assert_eq!(exact_tuple.5, record.fresh.invocation_id);
    assert_eq!(exact_tuple.6, record.fresh.parent_invocation_id);
    drop(state);

    let forged = AcceptedContinuation::without_historical_authority(
        record.continuation_id.clone(),
        context,
        ReservedInvocation {
            invocation_id: record.resume.invocation_id.clone(),
            parent_invocation_id: record.resume.parent_invocation_id.clone(),
        },
        ReservedInvocation {
            invocation_id: record.fresh.invocation_id.clone(),
            parent_invocation_id: record.fresh.parent_invocation_id.clone(),
        },
    );
    let store = fixture.open_store();
    assert_eq!(
        store
            .historical_parent_admission(
                &forged,
                HistoricalParentAuthorityClaim {
                    continuation_id: &record.continuation_id,
                    parent_invocation_uuid: &record.resume.parent_invocation_id,
                    child_invocation_uuid: &record.resume.invocation_id,
                },
            )
            .expect("historical authority lookup"),
        None
    );
}

struct Fixture {
    _tempdir: tempfile::TempDir,
    state_path: PathBuf,
    planning_root: PathBuf,
    worktree: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tempdir = tempfile::tempdir().expect("temporary state directory");
        let planning_root = tempdir.path().join("planning");
        let worktree = tempdir.path().join("worktree");
        std::fs::create_dir_all(&planning_root).expect("planning root");
        std::fs::create_dir_all(&worktree).expect("worktree");
        Self {
            state_path: tempdir.path().join("state.db"),
            _tempdir: tempdir,
            planning_root,
            worktree,
        }
    }

    fn open_store(&self) -> StateDbContinuationStore {
        let state = StateDb::open(&self.state_path).expect("open temporary state.db");
        StateDbContinuationStore::new(state)
    }

    fn context(&self, question_id: &str) -> ValidatedContinuation {
        self.context_with_target(question_id, "fresh-model")
    }

    fn context_with_target(&self, question_id: &str, target_model: &str) -> ValidatedContinuation {
        let files = evidence_files(
            &self.planning_root,
            &self.worktree,
            question_id,
            "origin-invocation",
        );
        let request = FreshContinuationRequest {
            question_id: question_id.to_string(),
            origin_invocation_id: "origin-invocation".to_string(),
            origin_session_id: "origin-session".to_string(),
            planning_root: self.planning_root.clone(),
            worktree: self.worktree.clone(),
            last_successful_boundary: "verified".to_string(),
            active_blocked_boundary: "apply".to_string(),
            target_model: target_model.to_string(),
            evidence: ContinuationEvidence {
                question: artifact(&files, &self.planning_root, "question.json"),
                answer: artifact(&files, &self.planning_root, "answer.json"),
                session_graph: artifact(&files, &self.planning_root, "graph.json"),
                origin_trace: artifact(&files, &self.planning_root, "trace.json"),
                ticket_snapshot: artifact(&files, &self.planning_root, "ticket.md"),
            },
        };
        DefaultContinuationEvidenceValidator::new(ArtifactSourceFake { files })
            .validate(&request)
            .expect("production evidence validation")
    }

    fn handoff(&self) -> PublishedHandoff {
        PublishedHandoff {
            path: self.planning_root.join("continuation.json"),
            sha256: "handoff-sha256".to_string(),
        }
    }
}

fn accept(
    store: &mut StateDbContinuationStore,
    context: &ValidatedContinuation,
) -> AcceptedContinuation {
    let decision = store.accept(context).expect("accept continuation");
    accepted(&decision).clone()
}

fn accepted(decision: &AcceptDecision) -> &AcceptedContinuation {
    match decision {
        AcceptDecision::Accepted(continuation) => continuation,
        AcceptDecision::Replay(_) => panic!("expected a newly accepted continuation"),
    }
}

struct ArtifactSourceFake {
    files: std::collections::HashMap<PathBuf, Vec<u8>>,
}

impl ContinuationArtifactSource for ArtifactSourceFake {
    fn read(&mut self, artifact: &ArtifactIdentity) -> Result<Vec<u8>, ContinuationBlock> {
        self.files
            .get(&artifact.path)
            .cloned()
            .ok_or_else(|| ContinuationBlock {
                kind: ContinuationBlockKind::InvalidEvidence,
                message: "artifact is missing".to_string(),
            })
    }
}

fn evidence_files(
    root: &Path,
    worktree: &Path,
    question_id: &str,
    origin_invocation_id: &str,
) -> std::collections::HashMap<PathBuf, Vec<u8>> {
    let graph_path = root.join("graph.json");
    std::collections::HashMap::from([
        (
            root.join("question.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "kind": "agent_question",
                "question_id": question_id,
                "origin": {
                    "invocation_uuid": origin_invocation_id,
                    "session_id": "origin-session",
                    "worktree_path": worktree,
                },
                "state_refs": {"session_graph_manifest": graph_path},
            }))
            .unwrap(),
        ),
        (
            root.join("answer.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "kind": "agent_answer",
                "question_id": question_id,
                "answered_by": "user-via-root-orchestrator",
                "continuation_plan": {"session_graph_manifest": graph_path},
            }))
            .unwrap(),
        ),
        (
            graph_path,
            serde_json::to_vec(&serde_json::json!({
                "root_invocation_uuid": origin_invocation_id,
                "invocation_ids": [origin_invocation_id],
                "session_ids": ["origin-session"],
                "question_ids": [question_id],
            }))
            .unwrap(),
        ),
        (
            root.join("trace.json"),
            serde_json::to_vec(&serde_json::json!({
                "root": {
                    "invocation": {"id": origin_invocation_id},
                    "session": {"provider_session_id": "origin-session"},
                },
            }))
            .unwrap(),
        ),
        (root.join("ticket.md"), b"ticket snapshot".to_vec()),
    ])
}

fn artifact(
    files: &std::collections::HashMap<PathBuf, Vec<u8>>,
    root: &Path,
    name: &str,
) -> ArtifactIdentity {
    let path = root.join(name);
    ArtifactIdentity {
        sha256: format!("{:x}", Sha256::digest(files.get(&path).unwrap())),
        path,
    }
}

type DurableContinuationNonIdFields = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn durable_continuation_non_id_fields(
    state: &StateDb,
    continuation_id: &str,
) -> DurableContinuationNonIdFields {
    state
        .connection()
        .query_row(
            "SELECT logical_request_key, validated_fingerprint,
                    resume_invocation_id, resume_parent_invocation_id,
                    resume_stage, resume_outcome_json,
                    fresh_invocation_id, fresh_parent_invocation_id,
                    fresh_stage, fresh_outcome_json,
                    handoff_json, terminal_outcome_json
             FROM fresh_continuations
             WHERE continuation_id = ?1",
            [continuation_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            },
        )
        .expect("read exact durable continuation non-ID fields")
}

fn unconfirmed_resume(invocation_id: &str) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: invocation_id.to_string(),
        session_id: Some("origin-session".to_string()),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::Accepted,
        disposition: InvocationDisposition::Failed {
            error_category: "resume_completion_unconfirmed".to_string(),
            terminal_reason: "resume_completion_unconfirmed".to_string(),
        },
    }
}

fn successful_resume(invocation_id: &str) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: invocation_id.to_string(),
        session_id: Some("origin-session".to_string()),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::Accepted,
        disposition: InvocationDisposition::Succeeded,
    }
}

fn successful_fresh(invocation_id: &str) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: invocation_id.to_string(),
        session_id: Some("fresh-session".to_string()),
        physical_exit_code: 0,
        acceptance: ResumeAcceptance::NotApplicable,
        disposition: InvocationDisposition::Succeeded,
    }
}

fn failed_fresh(invocation_id: &str) -> InvocationOutcome {
    InvocationOutcome {
        invocation_id: invocation_id.to_string(),
        session_id: Some("fresh-session".to_string()),
        physical_exit_code: 9,
        acceptance: ResumeAcceptance::NotApplicable,
        disposition: InvocationDisposition::Failed {
            error_category: "fresh_provider_failed".to_string(),
            terminal_reason: "fresh_provider_failed".to_string(),
        },
    }
}
