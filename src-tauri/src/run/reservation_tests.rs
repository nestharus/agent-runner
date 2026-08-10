use std::path::Path;

use oulipoly_runtime::fresh_continuation::ReservedInvocation;
use oulipoly_state::{InvocationStart, StateDb};
use uuid::Uuid;

use super::reservation::ReservedRun;

const PARENT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const RESERVED_UUID: &str = "22222222-2222-4222-8222-222222222222";
const MISSING_PARENT_UUID: &str = "33333333-3333-4333-8333-333333333333";

struct ReservationFixture {
    state: StateDb,
    reservation: ReservedInvocation,
    parent_row_id: i64,
}

fn reservation_fixture() -> ReservationFixture {
    let state = StateDb::open(Path::new(":memory:")).unwrap();
    let parent_row_id = start_parent(&state, PARENT_UUID);
    let reservation = ReservedInvocation {
        invocation_id: RESERVED_UUID.to_string(),
        parent_invocation_id: PARENT_UUID.to_string(),
    };

    ReservationFixture {
        state,
        reservation,
        parent_row_id,
    }
}

fn start_parent(state: &StateDb, invocation_uuid: &str) -> i64 {
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap()
}

#[test]
fn reserved_run_preserves_reserved_identity_exact_parent_and_single_attempt() {
    let fixture = reservation_fixture();

    let result: Result<ReservedRun, String> =
        ReservedRun::resolve(&fixture.state, &fixture.reservation);
    let plan = result.unwrap();
    let invocation_id: &str = plan.invocation_id();
    let parent_invocation_row_id: i64 = plan.parent_invocation_row_id();
    let max_attempts: usize = plan.max_attempts();

    assert_eq!(invocation_id, RESERVED_UUID);
    assert_eq!(parent_invocation_row_id, fixture.parent_row_id);
    assert_eq!(max_attempts, 1);
}

#[test]
fn reserved_run_supplies_the_exact_identity_to_both_concrete_adapters() {
    let fixture = reservation_fixture();
    let result: Result<ReservedRun, String> =
        ReservedRun::resolve(&fixture.state, &fixture.reservation);
    let plan = result.unwrap();
    let provider_name = "selected-provider";

    let resume_id = super::resume::composite_invocation_id(provider_name, Some(&plan));
    let balancing_id = super::balancing::composite_invocation_id(provider_name, Some(&plan));

    assert_eq!(resume_id.source, provider_name);
    assert_eq!(resume_id.id, RESERVED_UUID);
    assert_eq!(balancing_id.source, provider_name);
    assert_eq!(balancing_id.id, RESERVED_UUID);
}

#[test]
fn reserved_run_rejects_a_missing_exact_parent_instead_of_using_another_row() {
    let state = StateDb::open(Path::new(":memory:")).unwrap();
    start_parent(&state, PARENT_UUID);
    let reservation = ReservedInvocation {
        invocation_id: RESERVED_UUID.to_string(),
        parent_invocation_id: MISSING_PARENT_UUID.to_string(),
    };

    let result: Result<ReservedRun, String> = ReservedRun::resolve(&state, &reservation);

    assert!(result.is_err());
}

#[test]
fn resume_without_a_reservation_mints_distinct_valid_uuids() {
    let first = super::resume::composite_invocation_id("selected-provider", None);
    let second = super::resume::composite_invocation_id("selected-provider", None);

    assert!(Uuid::parse_str(&first.id).is_ok());
    assert!(Uuid::parse_str(&second.id).is_ok());
    assert_ne!(first.id, second.id);
}

#[test]
fn balancing_without_a_reservation_mints_distinct_valid_uuids() {
    let first = super::balancing::composite_invocation_id("selected-provider", None);
    let second = super::balancing::composite_invocation_id("selected-provider", None);

    assert!(Uuid::parse_str(&first.id).is_ok());
    assert!(Uuid::parse_str(&second.id).is_ok());
    assert_ne!(first.id, second.id);
}
