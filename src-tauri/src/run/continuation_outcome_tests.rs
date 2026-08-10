use std::path::Path;

use oulipoly_runtime::fresh_continuation::{
    InvocationDisposition, InvocationOutcome, ReservedInvocation, ResumeAcceptance,
};
use oulipoly_state::{InvocationStart, ProviderSessionBinding, StateDb};

const PARENT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const RESERVED_UUID: &str = "22222222-2222-4222-8222-222222222222";
const MISSING_UUID: &str = "33333333-3333-4333-8333-333333333333";
const OTHER_UUID: &str = "44444444-4444-4444-8444-444444444444";
const ORIGIN_SESSION_ID: &str = "origin-session";
const FRESH_SESSION_ID: &str = "fresh-session";
const DIFFERENT_SESSION_ID: &str = "different-session";
const UNCONFIRMED: &str = "resume_completion_unconfirmed";

struct OutcomeFixture {
    state: StateDb,
    reservation: ReservedInvocation,
    parent_row_id: i64,
}

fn outcome_fixture(reserved_uuid: &str) -> OutcomeFixture {
    let state = StateDb::open(Path::new(":memory:")).unwrap();
    let parent_row_id = start_invocation(&state, PARENT_UUID, None);
    let reservation = ReservedInvocation {
        invocation_id: reserved_uuid.to_string(),
        parent_invocation_id: PARENT_UUID.to_string(),
    };

    OutcomeFixture {
        state,
        reservation,
        parent_row_id,
    }
}

fn start_invocation(state: &StateDb, invocation_uuid: &str, parent: Option<i64>) -> i64 {
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: parent,
        })
        .unwrap()
}

fn start_reserved_invocation(fixture: &OutcomeFixture) -> i64 {
    start_invocation(
        &fixture.state,
        &fixture.reservation.invocation_id,
        Some(fixture.parent_row_id),
    )
}

fn bind_provider_session(
    state: &StateDb,
    row_id: i64,
    session_id: &str,
    capture_method: &'static str,
    resume_input_id: Option<&str>,
) {
    state
        .bind_invocation_provider_session_start(
            row_id,
            &ProviderSessionBinding {
                provider_session_id: session_id.to_string(),
                capture_method,
                resume_input_id: resume_input_id.map(str::to_string),
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
}

fn finalize_success(state: &StateDb, row_id: i64) {
    state
        .finalize_invocation(row_id, true, 0, None, None)
        .unwrap();
}

#[test]
fn finalized_failed_resume_maps_exact_terminal_outcome() {
    let fixture = outcome_fixture(RESERVED_UUID);
    let row_id = start_reserved_invocation(&fixture);
    bind_provider_session(
        &fixture.state,
        row_id,
        ORIGIN_SESSION_ID,
        "resumed",
        Some(ORIGIN_SESSION_ID),
    );
    fixture
        .state
        .update_resume_acceptance(row_id, "accepted", Some("matched origin session"))
        .unwrap();
    fixture
        .state
        .finalize_invocation(row_id, false, 0, Some(UNCONFIRMED), Some(UNCONFIRMED))
        .unwrap();

    let outcome = super::continuation_outcome::observe_resume_outcome(
        &fixture.state,
        &fixture.reservation,
        ORIGIN_SESSION_ID,
    )
    .unwrap();

    assert_eq!(
        outcome,
        InvocationOutcome {
            invocation_id: RESERVED_UUID.to_string(),
            session_id: Some(ORIGIN_SESSION_ID.to_string()),
            physical_exit_code: 0,
            acceptance: ResumeAcceptance::Accepted,
            disposition: InvocationDisposition::Failed {
                error_category: UNCONFIRMED.to_string(),
                terminal_reason: UNCONFIRMED.to_string(),
            },
        }
    );
}

#[test]
fn finalized_successful_fresh_maps_exact_terminal_outcome() {
    let fixture = outcome_fixture(RESERVED_UUID);
    let row_id = start_reserved_invocation(&fixture);
    bind_provider_session(
        &fixture.state,
        row_id,
        FRESH_SESSION_ID,
        "forced_flag_verified",
        None,
    );
    finalize_success(&fixture.state, row_id);

    let outcome =
        super::continuation_outcome::observe_fresh_outcome(&fixture.state, &fixture.reservation)
            .unwrap();

    assert_eq!(
        outcome,
        InvocationOutcome {
            invocation_id: RESERVED_UUID.to_string(),
            session_id: Some(FRESH_SESSION_ID.to_string()),
            physical_exit_code: 0,
            acceptance: ResumeAcceptance::NotApplicable,
            disposition: InvocationDisposition::Succeeded,
        }
    );
}

#[test]
fn observation_rejects_missing_exact_reserved_uuid_despite_newer_terminal_row() {
    let fixture = outcome_fixture(MISSING_UUID);
    let newer_row_id = start_invocation(&fixture.state, OTHER_UUID, Some(fixture.parent_row_id));
    bind_provider_session(
        &fixture.state,
        newer_row_id,
        FRESH_SESSION_ID,
        "forced_flag_verified",
        None,
    );
    finalize_success(&fixture.state, newer_row_id);

    let result =
        super::continuation_outcome::observe_fresh_outcome(&fixture.state, &fixture.reservation);

    assert!(result.is_err());
}

#[test]
fn observation_rejects_exact_running_row() {
    let fixture = outcome_fixture(RESERVED_UUID);
    let row_id = start_reserved_invocation(&fixture);
    bind_provider_session(
        &fixture.state,
        row_id,
        FRESH_SESSION_ID,
        "forced_flag_verified",
        None,
    );

    let result =
        super::continuation_outcome::observe_fresh_outcome(&fixture.state, &fixture.reservation);

    assert!(result.is_err());
}

#[test]
fn observation_rejects_exact_reserved_uuid_attached_to_different_parent() {
    let fixture = outcome_fixture(RESERVED_UUID);
    let different_parent_row_id = start_invocation(&fixture.state, OTHER_UUID, None);
    let row_id = start_invocation(
        &fixture.state,
        &fixture.reservation.invocation_id,
        Some(different_parent_row_id),
    );
    bind_provider_session(
        &fixture.state,
        row_id,
        FRESH_SESSION_ID,
        "forced_flag_verified",
        None,
    );
    finalize_success(&fixture.state, row_id);

    let result =
        super::continuation_outcome::observe_fresh_outcome(&fixture.state, &fixture.reservation);

    assert!(result.is_err());
}

#[test]
fn resume_observation_rejects_provider_session_mismatch() {
    let fixture = outcome_fixture(RESERVED_UUID);
    let row_id = start_reserved_invocation(&fixture);
    bind_provider_session(
        &fixture.state,
        row_id,
        DIFFERENT_SESSION_ID,
        "resumed",
        Some(ORIGIN_SESSION_ID),
    );
    fixture
        .state
        .update_resume_acceptance(row_id, "accepted", Some("fixture acceptance"))
        .unwrap();
    finalize_success(&fixture.state, row_id);

    let result = super::continuation_outcome::observe_resume_outcome(
        &fixture.state,
        &fixture.reservation,
        ORIGIN_SESSION_ID,
    );

    assert!(result.is_err());
}

#[test]
fn fresh_observation_rejects_success_without_captured_provider_session() {
    let fixture = outcome_fixture(RESERVED_UUID);
    let row_id = start_reserved_invocation(&fixture);
    finalize_success(&fixture.state, row_id);

    let result =
        super::continuation_outcome::observe_fresh_outcome(&fixture.state, &fixture.reservation);

    assert!(result.is_err());
}
