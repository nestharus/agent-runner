use agent_runner_lib::setup::setup_brain_host::{
    SetupBrainHost, decode_setup_brain_turn_result,
    test_support::{
        BrainAction, BrainFixtureMode, BrainInvalidOutput, SetupBrainFlowHarness, SetupBrainMessage,
    },
};
use oulipoly_config::ProviderImplementationRef;
use oulipoly_config::app::SetupBrainConfig;
use oulipoly_provider::generated::SetupBrainTurnResult;
use oulipoly_setup::actions::AgentAction;
use oulipoly_setup::schemas::AGENT_TURN_SCHEMA;
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn configured_neutral_setup_brain_describes_gates_dispatches_json_actions_without_legacy() {
    let outcome = SetupBrainFlowHarness::configured("fixture-setup-brain")
        .with_settings_id("brain-default")
        .with_mode(BrainFixtureMode::JsonActions {
            conversation_id: None,
            actions: vec![
                BrainAction::Status {
                    message: "neutral setup ready".into(),
                },
                BrainAction::Complete {
                    summary: "neutral setup complete".into(),
                    items: vec!["fixture-setup-model".into()],
                },
            ],
            done: false,
        })
        .run();

    assert_eq!(outcome.provider_calls(), ["describe", "setup_brain.turn"]);
    assert_eq!(outcome.legacy_fallback_invocations(), 0);

    let request = outcome.only_turn_request();
    assert_eq!(request.artifact_id(), "fixture-setup-brain");
    assert_eq!(request.settings_id(), Some("brain-default"));
    assert_eq!(request.conversation_id(), None);
    assert_eq!(request.operation(), "setup_brain.turn");
    assert!(request.host_context().get("setup").is_some());
    assert!(request.response_schema().contains("\"actions\""));
    assert!(request.allowed_tools().contains(&"run_command".to_string()));

    assert_eq!(
        outcome.events(),
        [
            "progress:Agent turn 1/25...",
            "status:Thinking...",
            "status:neutral setup ready",
            "complete:neutral setup complete"
        ]
    );
    assert_eq!(outcome.executed_actions(), ["Status", "Complete"]);
    assert_eq!(outcome.terminal_outcome(), Some("success"));
    assert_eq!(outcome.memory_turns()[0].turn_number, 1);
    assert_eq!(
        outcome.memory_turns()[0].user_message,
        "Analyze the system state and begin setup."
    );
    assert_eq!(
        outcome.memory_turns()[0].actions_summary,
        "2 actions processed"
    );
}

#[test]
fn production_setup_brain_host_invokes_artifact_and_preserves_conversation_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let calls_path = temp.path().join("brain-calls.jsonl");
    let script_path = temp.path().join("fixture-setup-brain.py");
    let script = format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import sys

calls = pathlib.Path({calls_path:?})
request = json.load(sys.stdin)
operation = sys.argv[1]
with calls.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"operation": operation, "request": request}}) + "\n")

def envelope(result):
    return {{"contract": request["contract"], "request_id": request["request_id"], "ok": True, "result": result}}

if operation == "describe":
    result = {{
        "provider_id": "fixture-setup-brain",
        "display_name": "Fixture Setup Brain",
        "contract_versions": [request["contract"]],
        "preferred_contract": request["contract"],
        "capabilities": {{
            "launch": False,
            "policy": False,
            "quota": False,
            "session": False,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": True,
            "setup": True,
            "migration": False
        }}
    }}
elif operation == "setup_brain.turn":
    params = request["params"]
    if "conversation_id" in params:
        actions = [{{"type": "complete", "summary": "artifact complete", "items": ["fixture-setup-model"]}}]
        done = True
        conversation_id = "conv-neutral-2"
    else:
        actions = [{{"type": "status", "message": "artifact first turn"}}]
        done = False
        conversation_id = "conv-neutral-1"
    result = {{
        "conversation_id": conversation_id,
        "message": {{
            "content_type": "json",
            "json": {{"actions": actions, "done": done}}
        }},
        "markers": []
    }}
else:
    print(json.dumps({{"contract": request["contract"], "request_id": request["request_id"], "ok": False, "error": {{"code": "unsupported", "category": "unsupported", "message": "unsupported operation", "retryable": False}}}}))
    sys.exit(0)

print(json.dumps(envelope(result)))
"#,
        calls_path = calls_path.to_string_lossy()
    );
    fs::write(&script_path, script).expect("write fake setup brain");
    let mut permissions = fs::metadata(&script_path)
        .expect("metadata for fake setup brain")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).expect("chmod fake setup brain");

    let config = SetupBrainConfig {
        artifact: ProviderImplementationRef {
            path: None,
            crate_name: None,
            version: None,
            binary: None,
            script: Some(script_path.to_string_lossy().into_owned()),
        },
        settings_id: Some("brain-default".into()),
    };
    let mut host = SetupBrainHost::new(
        config,
        "neutral system prompt".into(),
        json!({"setup": {"detect": {"installed": true}}}),
    )
    .expect("setup brain host");

    let first = host
        .send_turn("first message", AGENT_TURN_SCHEMA)
        .expect("first setup brain turn");
    let second = host
        .send_turn("second message", AGENT_TURN_SCHEMA)
        .expect("second setup brain turn");

    assert!(!first.done);
    assert_eq!(first.actions.len(), 1);
    assert!(matches!(first.actions[0], AgentAction::Status { .. }));
    assert!(second.done);
    assert!(matches!(second.actions[0], AgentAction::Complete { .. }));

    let records: Vec<Value> = fs::read_to_string(calls_path)
        .expect("read setup brain calls")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json call record"))
        .collect();
    assert_eq!(
        records
            .iter()
            .map(|record| record["operation"].as_str().expect("operation"))
            .collect::<Vec<_>>(),
        ["describe", "setup_brain.turn", "setup_brain.turn"]
    );
    assert_eq!(
        records[1].pointer("/request/params/settings_id"),
        Some(&json!("brain-default"))
    );
    assert_eq!(
        records[1].pointer("/request/params/context/setup/detect/installed"),
        Some(&json!(true))
    );
    assert!(
        records[1]
            .pointer("/request/params/conversation_id")
            .is_none()
    );
    assert_eq!(
        records[2].pointer("/request/params/conversation_id"),
        Some(&json!("conv-neutral-1"))
    );
    assert!(
        records[1]
            .pointer("/request/params/response_schema/properties/actions")
            .is_some()
    );
    assert!(
        records[1]
            .pointer("/request/params/allowed_tools")
            .and_then(Value::as_array)
            .expect("allowed tools")
            .iter()
            .any(|tool| tool == "run_command")
    );
}

#[test]
fn no_config_setup_flow_uses_neutral_legacy_fallback_and_records_no_provider_turn() {
    let outcome = SetupBrainFlowHarness::without_setup_brain_config()
        .with_legacy_fallback_actions(vec![BrainAction::Complete {
            summary: "fallback complete".into(),
            items: vec!["fixture-setup-model".into()],
        }])
        .run();

    assert_eq!(outcome.provider_calls(), [] as [&str; 0]);
    assert_eq!(outcome.turn_request_count(), 0);
    assert_eq!(outcome.legacy_fallback_invocations(), 1);
    assert_eq!(outcome.terminal_outcome(), Some("success"));
}

#[test]
fn missing_setup_brain_capability_prevents_turn_dispatch() {
    let outcome = SetupBrainFlowHarness::configured("fixture-setup-brain")
        .with_describe_capability("setup_brain", false)
        .run();

    assert_eq!(outcome.provider_calls(), ["describe"]);
    assert_eq!(outcome.turn_request_count(), 0);
    assert_eq!(outcome.legacy_fallback_invocations(), 0);
    assert_eq!(outcome.error_kind(), Some("missing_setup_brain_capability"));
    assert_eq!(outcome.error_operation(), Some("describe"));
    assert_eq!(outcome.terminal_outcome(), Some("agent_error"));
}

#[test]
fn provider_error_reports_recoverable_dispatch_error_with_operation_context() {
    let outcome = SetupBrainFlowHarness::configured("fixture-setup-brain")
        .with_mode(BrainFixtureMode::ProviderError {
            operation: "setup_brain.turn".into(),
            message: "fixture provider failure".into(),
        })
        .run();

    assert_eq!(outcome.provider_calls(), ["describe", "setup_brain.turn"]);
    assert_eq!(outcome.legacy_fallback_invocations(), 0);
    assert_eq!(outcome.error_kind(), Some("setup_brain_provider_error"));
    assert_eq!(outcome.error_operation(), Some("setup_brain.turn"));
    assert_eq!(outcome.recoverable_errors(), ["setup_brain_provider_error"]);
    assert_eq!(outcome.executed_actions(), [] as [&str; 0]);
    assert_eq!(outcome.terminal_outcome(), Some("agent_error"));
}

#[test]
fn invalid_provider_outputs_execute_zero_actions_after_invalid_output() {
    let cases = [
        (
            "malformed-output",
            BrainInvalidOutput::MalformedProviderOutput,
            "setup_brain_protocol_error",
        ),
        (
            "mismatched-request",
            BrainInvalidOutput::MismatchedRequestId,
            "setup_brain_protocol_error",
        ),
        (
            "schema-invalid-success",
            BrainInvalidOutput::SchemaInvalidSuccess,
            "setup_brain_protocol_error",
        ),
        (
            "non-json-message",
            BrainInvalidOutput::Message(SetupBrainMessage {
                content_type: "text".into(),
                json: None,
            }),
            "invalid_setup_brain_message",
        ),
        (
            "missing-message-json",
            BrainInvalidOutput::Message(SetupBrainMessage {
                content_type: "json".into(),
                json: None,
            }),
            "invalid_setup_brain_message",
        ),
        (
            "invalid-action-json",
            BrainInvalidOutput::InvalidActionJson {
                raw_action: r#"{"type":"run_command","command":17}"#.into(),
            },
            "invalid_setup_brain_action_json",
        ),
    ];

    for (case_name, invalid_output, expected_kind) in cases {
        let outcome = SetupBrainFlowHarness::configured("fixture-setup-brain")
            .with_mode(BrainFixtureMode::InvalidOutput(invalid_output))
            .run();

        assert_eq!(outcome.provider_calls(), ["describe", "setup_brain.turn"]);
        assert_eq!(
            outcome.error_kind(),
            Some(expected_kind),
            "{case_name} should map to the stable setup brain error"
        );
        assert_eq!(
            outcome.executed_actions(),
            [] as [&str; 0],
            "{case_name} must not execute actions after invalid provider output"
        );
        assert_eq!(outcome.terminal_outcome(), Some("agent_error"));
    }
}

#[test]
fn production_decoder_separates_top_level_schema_invalid_from_action_json_invalid() {
    let schema_invalid = match decode_setup_brain_turn_result(SetupBrainTurnResult {
        conversation_id: "conv-neutral-1".into(),
        message: json!({
            "content_type": "json",
            "json": {
                "actions": [],
                "done": "not-a-boolean"
            }
        }),
        markers: vec![],
    }) {
        Ok(_) => panic!("top-level setup turn schema failure must be protocol error"),
        Err(error) => error,
    };
    assert_eq!(schema_invalid.kind, "setup_brain_protocol_error");

    let invalid_action = match decode_setup_brain_turn_result(SetupBrainTurnResult {
        conversation_id: "conv-neutral-1".into(),
        message: json!({
            "content_type": "json",
            "json": {
                "actions": [{"type": "run_command", "command": 17}],
                "done": false
            }
        }),
        markers: vec![],
    }) {
        Ok(_) => panic!("invalid action payload must keep the action JSON error"),
        Err(error) => error,
    };
    assert_eq!(invalid_action.kind, "invalid_setup_brain_action_json");
}

#[test]
fn multi_turn_continuity_sends_returned_conversation_id() {
    let outcome = SetupBrainFlowHarness::configured("fixture-setup-brain")
        .with_mode(BrainFixtureMode::TwoTurnContinuity {
            returned_conversation_id: "conv-neutral-1".into(),
            first_actions: vec![BrainAction::Status {
                message: "first turn".into(),
            }],
            second_actions: vec![BrainAction::Complete {
                summary: "second turn".into(),
                items: vec!["fixture-setup-model".into()],
            }],
        })
        .run();

    let requests = outcome.turn_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].conversation_id(), None);
    assert_eq!(requests[1].conversation_id(), Some("conv-neutral-1"));
    assert_eq!(
        outcome.provider_calls(),
        ["describe", "setup_brain.turn", "setup_brain.turn"]
    );
    assert_eq!(outcome.legacy_continuity_token_reads(), 0);
    assert_eq!(outcome.terminal_outcome(), Some("success"));
}
