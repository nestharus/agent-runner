use std::time::Duration;

use oulipoly_state::InvocationStart;

use super::{PipelineFixture, poll_first_running_uuid};

#[test]
fn age158_poll_first_running_uuid_returns_first_running_row_without_model_filter() {
    let fixture = PipelineFixture::with_script_body("exit 0");
    let db = fixture.open_db();
    let first_uuid = "11111111-1111-4111-8111-111111111111".to_string();
    let second_uuid = "22222222-2222-4222-8222-222222222222".to_string();

    db.start_invocation(&InvocationStart {
        invocation_uuid: first_uuid.clone(),
        model_name: "unrelated-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
    .unwrap();
    db.start_invocation(&InvocationStart {
        invocation_uuid: second_uuid,
        model_name: "fixture".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
    .unwrap();

    assert_eq!(
        poll_first_running_uuid(&fixture, Duration::from_millis(100)),
        Some(first_uuid)
    );
}
