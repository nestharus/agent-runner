use agent_runner_lib::setup::flow::SetupFlow;
use oulipoly_config::{ProviderImplementationRef, app::SetupBrainConfig};
use oulipoly_setup::actions::{Action, AgentAction, AgentTurnResult, MemoryEdgeSpec, SetupEvent};
use oulipoly_setup::memory::MemoryGraph;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;

#[test]
fn action_executor_contract_list_remains_in_setup_loop() {
    let flow = read_source("src-tauri/src/setup/flow.rs");
    let executor_body = source_between(
        &flow,
        "async fn process_agent_turn_result(",
        "fn execute_allowlisted(",
    );

    for action in [
        "AgentAction::Status",
        "AgentAction::RunCommand",
        "AgentAction::WriteConfig",
        "AgentAction::TestIntegration",
        "AgentAction::AskUser",
        "AgentAction::SyncSkill",
        "AgentAction::SyncMcp",
        "AgentAction::UpdateMemory",
        "AgentAction::Complete",
    ] {
        assert_contains("setup action executor", executor_body, action);
    }

    assert_ordered(
        executor_body,
        &[
            "AgentAction::Status",
            "AgentAction::RunCommand",
            "AgentAction::WriteConfig",
            "AgentAction::TestIntegration",
            "AgentAction::AskUser",
            "AgentAction::SyncSkill",
            "AgentAction::SyncMcp",
            "AgentAction::UpdateMemory",
            "AgentAction::Complete",
        ],
    );
}

#[test]
fn action_executor_behaves_for_events_feedback_memory_and_terminal_completion() {
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let event_sink = events.clone();
    let channel = Channel::new(move |body| {
        match body {
            InvokeResponseBody::Json(json) => {
                event_sink
                    .lock()
                    .expect("events mutex")
                    .push(serde_json::from_str(&json).expect("setup event json"));
            }
            InvokeResponseBody::Raw(_) => panic!("setup events must be JSON"),
        }
        Ok(())
    });
    let (_tx, rx) = mpsc::channel(1);
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("state.db");
    let memory = MemoryGraph::open(&db_path).expect("memory graph");
    memory.create_session("session-1").expect("session");
    let mut flow =
        SetupFlow::new_with_setup_brain(channel, rx, memory, "session-1".to_string(), None);

    let result = AgentTurnResult {
        actions: vec![
            AgentAction::Status {
                message: "neutral status".into(),
            },
            AgentAction::RunCommand {
                command: "fixture-denied".into(),
                args: vec![],
                description: "try denied command".into(),
            },
            AgentAction::UpdateMemory {
                node_type: "tool".into(),
                label: "fixture-tool".into(),
                data: "{}".into(),
                edges: vec![MemoryEdgeSpec {
                    target_label: "fixture-target".into(),
                    edge_type: "relates".into(),
                }],
            },
            AgentAction::Complete {
                summary: "executor complete".into(),
                items: vec!["fixture-item".into()],
            },
        ],
        done: false,
    };

    let feedback = tauri::async_runtime::block_on(flow.process_agent_turn_result_for_test(
        &result,
        1,
        "initial message",
    ));

    assert_eq!(feedback, None);
    let events = events.lock().expect("events mutex").clone();
    assert_eq!(
        events[0].pointer("/event"),
        Some(&Value::String("status".into()))
    );
    assert_eq!(
        events[0].pointer("/data/message"),
        Some(&Value::String("neutral status".into()))
    );
    assert_eq!(
        events[1].pointer("/event"),
        Some(&Value::String("status".into()))
    );
    assert_eq!(
        events[1].pointer("/data/message"),
        Some(&Value::String("try denied command".into()))
    );
    assert_eq!(
        events[2].pointer("/event"),
        Some(&Value::String("error".into()))
    );
    assert_eq!(
        events[2].pointer("/data/message"),
        Some(&Value::String(
            "Command 'fixture-denied' is not in the allowlist".into()
        ))
    );
    assert_eq!(
        events[3].pointer("/event"),
        Some(&Value::String("complete".into()))
    );
    assert_eq!(
        events[3].pointer("/data/summary"),
        Some(&Value::String("executor complete".into()))
    );

    drop(flow);
    let memory = MemoryGraph::open(&db_path).expect("memory graph");
    let node = memory
        .get_node("tool:fixture-tool")
        .expect("memory lookup")
        .expect("memory node");
    assert_eq!(node.node_type, "tool");
    assert_eq!(node.label, "fixture-tool");
    assert_eq!(node.data, "{}");
}

#[test]
fn action_executor_behaves_for_allowed_commands_writes_and_test_results() {
    let _env_guard = EnvGuard::set_home(tempfile::tempdir().expect("home tempdir"));
    let (events, channel, rx, memory, session_id, _temp) = test_flow_parts("session-allowed");
    let mut flow = SetupFlow::new_with_setup_brain(channel, rx, memory, session_id, None);

    let result = AgentTurnResult {
        actions: vec![
            AgentAction::RunCommand {
                command: "bash".into(),
                args: vec!["-lc".into(), "printf command-ok".into()],
                description: "run neutral command".into(),
            },
            AgentAction::WriteConfig {
                path: "~/.config/oulipoly-agent-runner/executor-fixture.txt".into(),
                content: "written-ok".into(),
                description: "write neutral config".into(),
            },
            AgentAction::WriteConfig {
                path: "/tmp/oulipoly-denied-fixture.txt".into(),
                content: "denied".into(),
                description: "reject neutral config".into(),
            },
            AgentAction::TestIntegration {
                model_name: "fixture-model".into(),
                command: "bash".into(),
                args: vec!["-lc".into(), "printf test-ok".into()],
            },
            AgentAction::TestIntegration {
                model_name: "fixture-model-fail".into(),
                command: "bash".into(),
                args: vec!["-lc".into(), "printf test-fail >&2; exit 7".into()],
            },
        ],
        done: false,
    };

    let feedback = tauri::async_runtime::block_on(flow.process_agent_turn_result_for_test(
        &result,
        1,
        "initial message",
    ))
    .expect("feedback");

    assert!(feedback.contains("Command `bash -lc printf command-ok` completed (exit 0)"));
    assert!(feedback.contains("stdout: command-ok"));
    assert!(
        feedback.contains("Config written: ~/.config/oulipoly-agent-runner/executor-fixture.txt")
    );
    assert!(feedback.contains("Failed to write config: Write path '/tmp/oulipoly-denied-fixture.txt' is not in allowed directories"));
    assert!(feedback.contains("Test for fixture-model: PASS (exit 0). Output: test-ok"));
    assert!(feedback.contains("Test for fixture-model-fail: FAIL (exit 7). Output: test-fail"));

    let events = events.lock().expect("events mutex").clone();
    assert!(events.iter().any(|event| {
        event.pointer("/data/content/type") == Some(&Value::String("command_output".into()))
            && event.pointer("/data/content/stdout") == Some(&Value::String("command-ok".into()))
    }));
    assert!(events.iter().any(|event| {
        event.pointer("/data/content/type") == Some(&Value::String("config_written".into()))
    }));
    assert!(events.iter().any(|event| {
        event.pointer("/data/content/type") == Some(&Value::String("test_result".into()))
            && event.pointer("/data/content/success") == Some(&Value::Bool(true))
            && event.pointer("/data/content/output") == Some(&Value::String("test-ok".into()))
    }));
    assert!(events.iter().any(|event| {
        event.pointer("/data/content/type") == Some(&Value::String("test_result".into()))
            && event.pointer("/data/content/success") == Some(&Value::Bool(false))
            && event.pointer("/data/content/output") == Some(&Value::String("test-fail".into()))
    }));

    let written_path = dirs::home_dir()
        .expect("home dir")
        .join(".config/oulipoly-agent-runner/executor-fixture.txt");
    assert_eq!(
        std::fs::read_to_string(written_path).expect("written fixture"),
        "written-ok"
    );
}

#[test]
fn configured_setup_loop_behaves_for_done_true_terminal_outcome() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("state.db");
    let script_path = temp.path().join("fixture-setup-brain.py");
    let script = r#"#!/usr/bin/env python3
import json
import sys

request = json.load(sys.stdin)
operation = sys.argv[1]

def envelope(result):
    return {"contract": request["contract"], "request_id": request["request_id"], "ok": True, "result": result}

if operation == "describe":
    result = {
        "provider_id": "fixture-setup-brain",
        "display_name": "Fixture Setup Brain",
        "contract_versions": [request["contract"]],
        "preferred_contract": request["contract"],
        "capabilities": {
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
        }
    }
    print(json.dumps(envelope(result)))
elif operation == "setup_brain.turn":
    result = {
        "conversation_id": "conv-neutral-done",
        "message": {
            "content_type": "json",
            "json": {"actions": [], "done": True}
        },
        "markers": []
    }
    print(json.dumps(envelope(result)))
else:
    print(json.dumps({
        "contract": request["contract"],
        "request_id": request["request_id"],
        "ok": False,
        "error": {
            "code": "unsupported",
            "category": "unsupported",
            "message": "unsupported operation",
            "retryable": False
        }
    }))
"#;
    fs::write(&script_path, script).expect("write fake setup brain");
    let mut permissions = fs::metadata(&script_path)
        .expect("metadata for fake setup brain")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&script_path, permissions).expect("chmod fake setup brain");

    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let event_sink = events.clone();
    let channel = Channel::new(move |body| {
        match body {
            InvokeResponseBody::Json(json) => {
                event_sink
                    .lock()
                    .expect("events mutex")
                    .push(serde_json::from_str(&json).expect("setup event json"));
            }
            InvokeResponseBody::Raw(_) => panic!("setup events must be JSON"),
        }
        Ok(())
    });
    let (_tx, rx) = mpsc::channel(1);
    let memory = MemoryGraph::open(&db_path).expect("memory graph");
    let flow = SetupFlow::new_with_setup_brain(
        channel,
        rx,
        memory,
        "session-done".to_string(),
        Some(SetupBrainConfig {
            artifact: ProviderImplementationRef {
                path: None,
                crate_name: None,
                version: None,
                binary: None,
                script: Some(script_path.to_string_lossy().into_owned()),
            },
            settings_id: Some("brain-default".into()),
        }),
    );

    tauri::async_runtime::block_on(flow.run_for_cli("fixture-cli"));

    let events = events.lock().expect("events mutex").clone();
    assert!(events.iter().any(|event| {
        event.pointer("/event") == Some(&Value::String("progress".into()))
            && event.pointer("/data/message") == Some(&Value::String("Agent turn 1/25...".into()))
    }));
    assert_eq!(
        setup_session_outcome(&db_path, "session-done"),
        Some("done".into())
    );
}

#[test]
fn action_executor_behaves_for_user_response_and_sync_failures() {
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let event_sink = events.clone();
    let channel = Channel::new(move |body| {
        match body {
            InvokeResponseBody::Json(json) => {
                event_sink
                    .lock()
                    .expect("events mutex")
                    .push(serde_json::from_str(&json).expect("setup event json"));
            }
            InvokeResponseBody::Raw(_) => panic!("setup events must be JSON"),
        }
        Ok(())
    });
    let (tx, rx) = mpsc::channel(1);
    tx.blocking_send(oulipoly_setup::actions::UserResponse::Skip {
        reason: Some("fixture skip".into()),
    })
    .expect("send user response");
    let temp = tempfile::tempdir().expect("tempdir");
    let memory = MemoryGraph::open(&temp.path().join("state.db")).expect("memory graph");
    memory.create_session("session-user").expect("session");
    let mut flow =
        SetupFlow::new_with_setup_brain(channel, rx, memory, "session-user".to_string(), None);

    let result = AgentTurnResult {
        actions: vec![
            AgentAction::AskUser {
                action: Action::Confirm {
                    title: "Confirm neutral action".into(),
                    message: "Continue?".into(),
                    confirm_id: "fixture-confirm".into(),
                    confirm_label: None,
                    cancel_label: None,
                },
            },
            AgentAction::SyncSkill {
                source_cli: "fixture-source".into(),
                target_cli: "fixture-target".into(),
                skill_name: "fixture-skill".into(),
            },
            AgentAction::SyncMcp {
                source_cli: "fixture-source".into(),
                target_cli: "fixture-target".into(),
                mcp_name: "fixture-mcp".into(),
                config: "{}".into(),
            },
        ],
        done: false,
    };

    let feedback = tauri::async_runtime::block_on(flow.process_agent_turn_result_for_test(
        &result,
        1,
        "initial message",
    ))
    .expect("feedback");

    assert!(feedback.contains("User responded:"));
    assert!(feedback.contains("fixture skip"));
    assert!(feedback.contains("Failed to sync skill: Source CLI has no skills directory"));
    assert!(feedback.contains("Failed to sync MCP: No MCP config path for fixture-target"));
    let events = events.lock().expect("events mutex").clone();
    assert!(events.iter().any(|event| {
        event.pointer("/event") == Some(&Value::String("need_input".into()))
            && event.pointer("/data/action/type") == Some(&Value::String("confirm".into()))
    }));
}

#[test]
fn action_executor_behaves_for_user_cancel_terminal_branch() {
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let event_sink = events.clone();
    let channel = Channel::new(move |body| {
        match body {
            InvokeResponseBody::Json(json) => {
                event_sink
                    .lock()
                    .expect("events mutex")
                    .push(serde_json::from_str(&json).expect("setup event json"));
            }
            InvokeResponseBody::Raw(_) => panic!("setup events must be JSON"),
        }
        Ok(())
    });
    let (tx, rx) = mpsc::channel(1);
    tx.blocking_send(oulipoly_setup::actions::UserResponse::Cancel)
        .expect("send cancel");
    let temp = tempfile::tempdir().expect("tempdir");
    let memory = MemoryGraph::open(&temp.path().join("state.db")).expect("memory graph");
    memory.create_session("session-cancel").expect("session");
    let mut flow =
        SetupFlow::new_with_setup_brain(channel, rx, memory, "session-cancel".to_string(), None);

    let result = AgentTurnResult {
        actions: vec![AgentAction::AskUser {
            action: Action::Confirm {
                title: "Confirm neutral action".into(),
                message: "Continue?".into(),
                confirm_id: "fixture-confirm".into(),
                confirm_label: None,
                cancel_label: None,
            },
        }],
        done: false,
    };

    let feedback = tauri::async_runtime::block_on(flow.process_agent_turn_result_for_test(
        &result,
        1,
        "initial message",
    ));

    assert_eq!(feedback, None);
    let events = events.lock().expect("events mutex").clone();
    assert!(events.iter().any(|event| {
        event.pointer("/event") == Some(&Value::String("error".into()))
            && event.pointer("/data/message")
                == Some(&Value::String("Setup cancelled by user.".into()))
            && event.pointer("/data/recoverable") == Some(&Value::Bool(false))
    }));
}

#[test]
fn action_executor_preserves_events_feedback_memory_and_terminal_outcomes() {
    let flow = read_source("src-tauri/src/setup/flow.rs");
    let flow_body = source_between(
        &flow,
        "async fn run_agent_loop(&mut self, system_prompt: String, initial_message: &str)",
        "fn execute_allowlisted(",
    );
    let executor_body = source_between(
        &flow,
        "async fn process_agent_turn_result(",
        "fn execute_allowlisted(",
    );

    for expected in [
        "SetupEvent::Progress",
        "message: format!(\"Agent turn {}/{}...\", turn_number, MAX_AGENT_TURNS)",
        "SetupEvent::Status",
        "message: \"Thinking...\".into()",
        "process_agent_turn_result(&result, turn_number, &next_message)",
        "if result.done",
        "next_message = feedback",
        "&next_message",
        "self.memory.end_session(&self.session_id, \"done\")",
    ] {
        assert_contains("setup loop preservation", flow_body, expected);
    }

    for expected in [
        "self.memory.record_turn(",
        "\"[]\"",
        "\"Command `{} {}` completed (exit {}).\\nstdout: {}\\nstderr: {}\"",
        "feedback_parts.push(format!(\"Command failed: {e}\"))",
        "SetupEvent::ShowResult",
        "ResultContent::CommandOutput",
        "ResultContent::ConfigWritten",
        "\"Config written: {path}\"",
        "\"Failed to write config: {e}\"",
        "ResultContent::TestResult",
        "\"Test for {model_name}: {} (exit {exit_code}). Output: {}\"",
        "\"User responded: {response_json}\"",
        "\"Skill '{skill_name}' synced to {target_cli}\"",
        "\"MCP '{mcp_name}' installed in {target_cli}\"",
        "self.memory.upsert_node(",
        "self.memory.add_edge(",
        "SetupEvent::Complete",
        "self.memory.end_session(&self.session_id, \"success\")",
    ] {
        assert_contains(
            "setup action executor preservation",
            executor_body,
            expected,
        );
    }

    assert_ordered(
        flow_body,
        &[
            "let result = match agent.send_turn(",
            "process_agent_turn_result(&result, turn_number, &next_message)",
            "if result.done",
            "next_message = feedback",
        ],
    );

    assert_ordered(
        executor_body,
        &[
            "self.memory.record_turn(",
            "let mut feedback_parts: Vec<String> = Vec::new();",
            "for action in &result.actions",
            "Some(if feedback_parts.is_empty()",
        ],
    );
}

#[test]
fn action_executor_preserves_cancel_resume_denials_and_max_turns() {
    let flow = read_source("src-tauri/src/setup/flow.rs");
    let flow_body = source_between(
        &flow,
        "async fn run_agent_loop(&mut self, system_prompt: String, initial_message: &str)",
        "fn execute_allowlisted(",
    );
    let executor_body = source_between(
        &flow,
        "async fn process_agent_turn_result(",
        "fn execute_allowlisted(",
    );
    let command_body = source_between(
        &flow,
        "fn execute_allowlisted(command: &str, args: &[String])",
        "fn validate_and_write(",
    );
    let write_body = source_between(
        &flow,
        "fn validate_and_write(",
        "fn get_install_instructions()",
    );

    for expected in [
        "if turn_number > MAX_AGENT_TURNS",
        "\"Setup agent exceeded maximum turns. Please retry or configure manually.\"",
        "end_session(&self.session_id, \"max_turns_exceeded\")",
    ] {
        assert_contains("setup loop terminal preservation", flow_body, expected);
    }

    for expected in [
        "Some(UserResponse::Cancel)",
        "\"Setup cancelled by user.\"",
        "self.memory.end_session(&self.session_id, \"cancelled\")",
        "Some(response) =>",
        "serde_json::to_string(&response)",
    ] {
        assert_contains(
            "setup pause and terminal preservation",
            executor_body,
            expected,
        );
    }

    assert_contains(
        "run command denial",
        command_body,
        "Command '{command}' is not in the allowlist",
    );
    assert_contains(
        "write config denial",
        write_body,
        "Write path '{resolved}' is not in allowed directories",
    );
    assert_contains(
        "write config success",
        write_body,
        "std::fs::write(&expanded, content)",
    );
}

fn read_source(relative: &str) -> String {
    std::fs::read_to_string(workspace_root().join(relative))
        .unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("missing start marker {start:?}"));
    let end_index = source[start_index..]
        .find(end)
        .map(|offset| start_index + offset)
        .unwrap_or_else(|| panic!("missing end marker {end:?} after {start:?}"));
    &source[start_index..end_index]
}

fn assert_contains(context: &str, source: &str, needle: &str) {
    assert!(source.contains(needle), "{context} must contain {needle:?}");
}

fn assert_ordered(source: &str, needles: &[&str]) {
    let mut cursor = 0;
    for needle in needles {
        let offset = source[cursor..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing ordered marker {needle:?}"));
        cursor += offset + needle.len();
    }
}

fn setup_session_outcome(path: &std::path::Path, session_id: &str) -> Option<String> {
    let connection = rusqlite::Connection::open(path).expect("state db");
    connection
        .query_row(
            "SELECT outcome FROM setup_sessions WHERE id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .expect("setup session outcome")
}

fn test_flow_parts(session_id: &str) -> TestFlowParts {
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let event_sink = events.clone();
    let channel = Channel::new(move |body| {
        match body {
            InvokeResponseBody::Json(json) => {
                event_sink
                    .lock()
                    .expect("events mutex")
                    .push(serde_json::from_str(&json).expect("setup event json"));
            }
            InvokeResponseBody::Raw(_) => panic!("setup events must be JSON"),
        }
        Ok(())
    });
    let (_tx, rx) = mpsc::channel(1);
    let temp = tempfile::tempdir().expect("tempdir");
    let memory = MemoryGraph::open(&temp.path().join("state.db")).expect("memory graph");
    memory.create_session(session_id).expect("session");
    (events, channel, rx, memory, session_id.to_string(), temp)
}

type TestFlowParts = (
    Arc<Mutex<Vec<Value>>>,
    Channel<SetupEvent>,
    mpsc::Receiver<oulipoly_setup::actions::UserResponse>,
    MemoryGraph,
    String,
    tempfile::TempDir,
);

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous_home: Option<std::ffi::OsString>,
    _temp: tempfile::TempDir,
}

impl EnvGuard {
    fn set_home(temp: tempfile::TempDir) -> Self {
        let lock = ENV_MUTEX
            .get_or_init(|| Mutex::new(()))
            .lock()
            .expect("env mutex");
        let previous_home = std::env::var_os("HOME");
        // Environment mutation is process-global, so these tests hold a mutex until restore.
        unsafe {
            std::env::set_var("HOME", temp.path());
        }
        Self {
            _lock: lock,
            previous_home,
            _temp: temp,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.previous_home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

static ENV_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf()
}
