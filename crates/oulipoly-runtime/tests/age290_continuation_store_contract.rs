use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use oulipoly_runtime::fresh_continuation::{
    AcceptDecision, AcceptedContinuation, ArtifactIdentity, ContinuationBlockKind,
    ContinuationEvidence, ContinuationStore, FreshContinuationOutcome, FreshContinuationRequest,
    InvocationDisposition, InvocationOutcome, PublishedHandoff, ResumeAcceptance, RunDecision,
    StateDbContinuationStore, ValidatedContinuation,
};
use oulipoly_state::StateDb;

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
        context.request.origin_invocation_id
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
        context.request.origin_invocation_id
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
fn accept_rejects_a_changed_fingerprint_for_the_same_logical_request() {
    let fixture = Fixture::new();
    let context = fixture.context("fingerprint-1");
    let mut conflicting = context.clone();
    conflicting.fingerprint = "fingerprint-2".to_string();
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
    let constrained = state.connection().execute(
        "UPDATE fresh_continuations SET resume_outcome_json = '{' WHERE continuation_id = ?1",
        [&continuation.continuation_id],
    );
    assert!(constrained.is_err(), "schema must reject malformed JSON");
    state
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable corruption injection");
    state
        .connection()
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
    state
        .connection()
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
    let constrained = state.connection().execute(
        "UPDATE fresh_continuations SET fresh_stage = 'running' WHERE continuation_id = ?1",
        [&continuation.continuation_id],
    );
    assert!(
        constrained.is_err(),
        "schema must reject fresh progress before durable resume completion"
    );
    state
        .connection()
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .expect("enable corruption injection");
    state
        .connection()
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

    fn context(&self, fingerprint: &str) -> ValidatedContinuation {
        ValidatedContinuation {
            request: FreshContinuationRequest {
                question_id: "question-1".to_string(),
                origin_invocation_id: "origin-invocation".to_string(),
                origin_session_id: "origin-session".to_string(),
                planning_root: self.planning_root.clone(),
                worktree: self.worktree.clone(),
                last_successful_boundary: "verified".to_string(),
                active_blocked_boundary: "apply".to_string(),
                target_model: "fresh-model".to_string(),
                evidence: ContinuationEvidence {
                    question: artifact(&self.planning_root, "question.json"),
                    answer: artifact(&self.planning_root, "answer.json"),
                    session_graph: artifact(&self.planning_root, "graph.json"),
                    origin_trace: artifact(&self.planning_root, "trace.json"),
                    ticket_snapshot: artifact(&self.planning_root, "ticket.md"),
                },
            },
            fingerprint: fingerprint.to_string(),
        }
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

fn artifact(root: &Path, name: &str) -> ArtifactIdentity {
    ArtifactIdentity {
        path: root.join(name),
        sha256: format!("{name}-sha256"),
    }
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
