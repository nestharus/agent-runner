//! SDK-owned synthetic specimens at Runner's base admission boundaries.
//! Selection controls exercise request-local offers and typed capabilities,
//! not adapter advertisement/emission. Stream admission does not establish
//! prompt attestation, complete output delivery, or page-reader coherence.

use oulipoly_provider::generated as dto;
use oulipoly_provider::schemas::{SchemaRegistry, SchemaValidationError};
use oulipoly_provider::stream::LaunchJsonlReader;
use serde_json::{Value, json};

fn specimens() -> Value {
    serde_json::from_str(agent_provider_contract::fixtures::HOST_EXTENSIONS_V1_JSON).unwrap()
}

// Ordered fixture edits are test data, not a host protocol or semantic verifier.
fn edited(mut value: Value, edits: &Value) -> Value {
    for edit in edits.as_array().unwrap() {
        let (parent, key) = edit["path"].as_str().unwrap().rsplit_once('/').unwrap();
        let parent = value.pointer_mut(parent).unwrap();
        if let Some(array) = parent.as_array_mut() {
            let index = key.parse::<usize>().unwrap();
            if edit["remove"] == true {
                array.remove(index);
            } else {
                array[index] = edit["value"].clone();
            }
        } else {
            let object = parent.as_object_mut().unwrap();
            if edit["remove"] == true {
                assert!(object.remove(key).is_some());
            } else {
                object.insert(key.to_owned(), edit["value"].clone());
            }
        }
    }
    value
}

fn ndjson(events: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    for event in events.as_array().unwrap() {
        serde_json::to_writer(&mut bytes, event).unwrap();
        bytes.push(b'\n');
    }
    bytes
}

fn admit(
    registry: &SchemaRegistry,
    target: &str,
    value: &Value,
) -> Result<(), SchemaValidationError> {
    match target {
        "/describe_request" => registry.validate_request("describe", value),
        "/describe_response" => registry.validate_response("describe", value),
        "/launch_request" => registry.validate_request("launch", value),
        "/pages_request" => registry.validate_request("session.read_turns", value),
        "/pages_response" => registry.validate_response("session.read_turns", value),
        "/launch_events/0" | "/launch_events/4" => registry.validate_launch_event("marker", value),
        _ => panic!("unknown specimen target {target}"),
    }
}

#[test]
fn paired_valid_shapes_reach_runner_admission_and_stream_retention() {
    let fixture = specimens();
    let valid = &fixture["valid"];
    let registry = SchemaRegistry::new();
    for target in [
        "/describe_request",
        "/describe_response",
        "/launch_request",
        "/pages_request",
        "/pages_response",
    ] {
        admit(&registry, target, valid.pointer(target).unwrap()).unwrap();
        println!("valid{target}: base schema admitted");
    }
    let describe: dto::DescribeResponse =
        serde_json::from_value(valid["describe_response"].clone()).unwrap();
    assert_eq!(
        describe.result.capabilities.additional["future_extension"],
        json!({"opaque": true})
    );
    let launch: dto::LaunchRequest =
        serde_json::from_value(valid["launch_request"].clone()).unwrap();
    let pages: dto::SessionReadTurnsResponse =
        serde_json::from_value(valid["pages_response"].clone()).unwrap();
    assert_eq!(
        serde_json::to_value(launch.clone()).unwrap(),
        valid["launch_request"]
    );
    assert_eq!(
        serde_json::to_value(pages).unwrap(),
        valid["pages_response"]
    );
    let result = LaunchJsonlReader::new(&launch.request_id)
        .read(ndjson(&valid["launch_events"]).as_slice())
        .unwrap();
    assert_eq!(
        result.events.len(),
        valid["launch_events"].as_array().unwrap().len()
    );
    assert_eq!(result.exit.seq, 6);
    assert_eq!(result.stdout_bytes(), b"Hello\n");
    assert_eq!(result.stderr_bytes(), b"Warning\n");
    for (name, index) in [
        (dto::PROMPT_ACCEPTED_MARKER_V1, 0),
        (dto::LAUNCH_OUTPUT_COMPLETE_MARKER_V1, 4),
    ] {
        assert_eq!(
            result.retained_marker_value(name),
            Some(&valid["launch_events"][index]["value"])
        );
    }
    println!("valid/launch_events: Runner parser admitted and retained specimen bytes/markers");
}

#[test]
fn paired_selection_controls_reach_runner_host_context_checks() {
    let fixture = specimens();
    let registry = SchemaRegistry::new();
    for case in fixture["selection"].as_array().unwrap() {
        let extension = case["extension"].as_str().unwrap();
        let mut request = fixture["valid"]["describe_request"].clone();
        if case["host_env"].is_null() {
            request["host"].as_object_mut().unwrap().remove("env");
        } else {
            request["host"]["env"] = case["host_env"].clone();
        }
        let mut response = fixture["valid"]["describe_response"].clone();
        let caps = response["result"]["capabilities"].as_object_mut().unwrap();
        for key in [
            "prompt_acceptance_v1",
            "launch_output_v1",
            "session_turn_pages_v1",
        ] {
            caps.remove(key);
        }
        caps.extend(case["capabilities"].as_object().unwrap().clone());
        // Absent/false and unoffered true capabilities are valid wire shapes.
        registry.validate_request("describe", &request).unwrap();
        registry.validate_response("describe", &response).unwrap();
        let request: dto::DescribeRequest = serde_json::from_value(request).unwrap();
        let response: dto::DescribeResponse = serde_json::from_value(response).unwrap();
        let caps = &response.result.capabilities;
        let (offered, advertised) = match extension {
            "prompt_acceptance" => (
                dto::host_requested_prompt_acceptance_v1(&request.host),
                caps.prompt_acceptance_v1,
            ),
            "launch_output" => (
                dto::host_requested_launch_output_v1(&request.host),
                caps.launch_output_v1,
            ),
            "session_turn_pages" => (
                dto::host_requested_session_turn_pages_v1(&request.host),
                caps.session_turn_pages_v1,
            ),
            _ => panic!("unknown specimen extension {extension}"),
        };
        assert_eq!(
            offered && advertised == Some(true),
            case["expect_selected"].as_bool().unwrap(),
            "{case}"
        );
        // Eligibility under the fixture's support declaration; no provider ran.
        let supports_v1 = case["provider_supported"]
            .as_array()
            .unwrap()
            .contains(&json!(1));
        assert_eq!(
            offered && supports_v1,
            case["expect_advertised"].as_bool().unwrap(),
            "{case}"
        );
        println!(
            "selection/{extension}/{}: request-local offer, typed capability and support eligibility checked",
            case["name"].as_str().unwrap()
        );
    }
}

#[test]
fn paired_invalid_shapes_fail_runner_base_admission() {
    let fixture = specimens();
    let registry = SchemaRegistry::new();
    for case in fixture["invalid_admission"].as_array().unwrap() {
        let target = case["target"].as_str().unwrap();
        let value = edited(
            fixture["valid"].pointer(target).unwrap().clone(),
            &case["edits"],
        );
        assert!(admit(&registry, target, &value).is_err(), "{case}");
        if let Some(index) = target.strip_prefix("/launch_events/") {
            let mut events = fixture["valid"]["launch_events"].clone();
            events[index.parse::<usize>().unwrap()] = value;
            let error = LaunchJsonlReader::new("launch-host-extensions")
                .read(ndjson(&events).as_slice())
                .unwrap_err();
            assert_eq!(error.transport_kind(), "schema_invalid_event", "{case}");
        }
        println!(
            "invalid_admission/{}: rejected at Runner schema boundary{}",
            case["name"].as_str().unwrap(),
            if target.starts_with("/launch_events/") {
                " and stream reader"
            } else {
                ""
            }
        );
    }
}

#[test]
fn paired_invalid_streams_fail_runner_launch_reader() {
    let fixture = specimens();
    for case in fixture["invalid_streams"].as_array().unwrap() {
        let events = edited(fixture["valid"]["launch_events"].clone(), &case["edits"]);
        let error = LaunchJsonlReader::new("launch-host-extensions")
            .read(ndjson(&events).as_slice())
            .unwrap_err();
        let expected = match case["expect"].as_str().unwrap() {
            "request_id" => "mismatched_request_id",
            "sequence" => "skipped_seq",
            "schema" => "schema_invalid_event",
            "base64" => "invalid_base64",
            "missing_exit" => "missing_final_exit",
            "after_exit" => "event_after_exit",
            value => panic!("unknown specimen stream outcome {value}"),
        };
        assert_eq!(error.transport_kind(), expected, "{case}");
        println!(
            "invalid_streams/{}: rejected by Runner reader ({expected})",
            case["name"].as_str().unwrap()
        );
    }
}

#[test]
fn paired_semantic_limits_remain_admitted_by_runner_wire_reader() {
    let fixture = specimens();
    for case in fixture["semantic_limits"].as_array().unwrap() {
        let events = edited(fixture["valid"]["launch_events"].clone(), &case["edits"]);
        assert_ne!(events, fixture["valid"]["launch_events"], "{case}");
        LaunchJsonlReader::new("launch-host-extensions")
            .read(ndjson(&events).as_slice())
            .unwrap();
        println!(
            "semantic_limits/{}: wire reader admitted; host semantic approval and provider emission untested",
            case["name"].as_str().unwrap()
        );
    }
    // A producer/reader coherence violation is also outside schema admission.
    let mut page = fixture["valid"]["pages_response"].clone();
    page["result"]["scan_progress"] = json!(true);
    SchemaRegistry::new()
        .validate_response("session.read_turns", &page)
        .unwrap();
    println!(
        "semantic_limits/page_scan_progress: base schema admitted; page-reader coherence untested"
    );
}
