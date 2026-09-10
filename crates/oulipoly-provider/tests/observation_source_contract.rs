//! Embedded Runner contract, not the provider's private schema copy.
pub mod support {
    pub mod contract_matrix;
}
use oulipoly_provider::schemas::SchemaRegistry;
use serde_json::{Value, json};
use support::contract_matrix::{fixtures, non_launch_fixture};

fn response(projection: &str, total: Value, warnings: Value) -> Value {
    let mut value =
        non_launch_fixture(&fixtures(), "session.read_turns", "success_response").clone();
    value["result"]["turn_projection"] = json!(projection);
    value["result"]["source_bytes_examined"] = total;
    value["result"]["warnings"] = warnings;
    value
}

#[test]
fn observation_schema_limits_are_projection_scoped_without_changing_other_providers() {
    let schema = SchemaRegistry::new();
    for projection in ["canonical_ingest", "user_observation"] {
        for warnings in [json!([]), json!(["ordinary provider warning"])] {
            for total in [0, 8_388_608] {
                schema
                    .validate_response(
                        "session.read_turns",
                        &response(projection, json!(total), warnings.clone()),
                    )
                    .unwrap();
            }
            assert!(
                schema
                    .validate_response(
                        "session.read_turns",
                        &response(projection, json!(8_388_609), warnings)
                    )
                    .is_err()
            );
        }
    }
    let declaration = "codex_observation_io_v1:forward=1072;reconstruction=8387536;metadata=134";
    schema
        .validate_response(
            "session.read_turns",
            &response(
                "user_observation",
                json!(8_388_742),
                json!([declaration, "ordinary warning"]),
            ),
        )
        .unwrap();
    let maximum = "codex_observation_io_v1:forward=8388608;reconstruction=8388607;metadata=0";
    schema
        .validate_response(
            "session.read_turns",
            &response("user_observation", json!(16_777_215), json!([maximum])),
        )
        .unwrap();
    for projection in ["canonical_ingest", "user_observation"] {
        for total in [json!(16_777_216), json!(-1), json!(1.5), json!("8388742")] {
            assert!(
                schema
                    .validate_response(
                        "session.read_turns",
                        &response(projection, total, json!([maximum]))
                    )
                    .is_err()
            );
        }
    }
    assert!(
        schema
            .validate_response(
                "session.read_turns",
                &response("canonical_ingest", json!(1), json!([declaration]))
            )
            .is_err()
    );
}

#[test]
fn observation_schema_rejects_hostile_declarations_even_below_ordinary_quota() {
    let schema = SchemaRegistry::new();
    let valid = "codex_observation_io_v1:forward=1;reconstruction=0;metadata=0";
    for warnings in [
        json!([valid, valid]),
        json!([valid, "codex_observation_io_v1"]),
        json!(["codex_observation_io_v1:forward=-1;reconstruction=0;metadata=0"]),
        json!(["codex_observation_io_v1:forward=+1;reconstruction=0;metadata=0"]),
        json!(["codex_observation_io_v1:forward=1.1;reconstruction=0;metadata=0"]),
        json!(["codex_observation_io_v1:forward=1e0;reconstruction=0;metadata=0"]),
        json!(["codex_observation_io_v1:forward= 1;reconstruction=0;metadata=0"]),
        json!(["codex_observation_io_v1:forward=1;reconstruction=0"]),
        json!(["codex_observation_io_v1:forward=1;reconstruction=0;metadata=0\n"]),
        json!(["codex_observation_io_v1:forward=1;reconstruction=0;metadata=0;metadata=0"]),
        json!([
            "codex_observation_io_v1:forward=184467440737095516160;reconstruction=0;metadata=0"
        ]),
    ] {
        assert!(
            schema
                .validate_response(
                    "session.read_turns",
                    &response("user_observation", json!(1), warnings.clone())
                )
                .is_err(),
            "{warnings}"
        );
    }
    let mut unknown = response("user_observation", json!(1), json!([valid]));
    unknown["result"]["observation_permission"] = json!(true);
    assert!(
        schema
            .validate_response("session.read_turns", &unknown)
            .is_err()
    );
    // Schema shape does not prove request-relative sums or u64 range. Those are
    // deliberately exercised through read_turn_page in the paired consumer.
}
