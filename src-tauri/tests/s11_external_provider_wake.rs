#![cfg(unix)]

//! ## Declared roles
//!
//! Roles: orchestration, formatter, accessor, parser, validator.
//!
//! TEST: external-provider runtime fixtures for session ingestion, submitted
//! turn acceptance, and policy diagnostics.

use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, EnqueueResult, MailboxDb, MailboxRow};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const MODEL: &str = "s11-external-wake-model";
const PROVIDER: &str = "opencode";
const OPENCODE_PROVIDERS: [&str; 3] = ["opencode", "opencode2", "opencode5"];
const SESSION: &str = "ses_s11externalwake";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    work_dir: PathBuf,
    provider: &'static str,
}

impl Fixture {
    fn new() -> Self {
        Self::with_provider(PROVIDER)
    }

    fn with_provider(provider: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("xdg-config");
        let data_home = dir.path().join("xdg-data");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let work_dir = dir.path().join("work");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            home_dir,
            app_config_dir,
            models_dir,
            work_dir,
            provider,
        }
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.home_dir)
            .env("S11_WORK_DIR", &self.work_dir)
            .env_remove("OULIPOLY_DATA_DIR")
            .env_remove("OULIPOLY_AUTO_WAKE")
            .env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID")
            .env_remove("OULIPOLY_AUTO_WAKE_TOKEN")
            .env_remove("OULIPOLY_AUTO_WAKE_COUNT")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .current_dir(self.dir.path());
        cmd.output().unwrap()
    }

    fn run_agent_with_env(&self, prompt: &str, envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(runner_bin());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(prompt);
        self.run(cmd)
    }

    fn run_resume_with_env(&self, prompt: &str, envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(runner_bin());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.arg("resume")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg("--session-id")
            .arg(SESSION)
            .arg("--prompt")
            .arg(prompt);
        self.run(cmd)
    }

    fn run_trace(&self, invocation_uuid: &str) -> Output {
        let mut cmd = Command::new(runner_bin());
        cmd.arg("trace").arg(invocation_uuid).arg("--json");
        self.run(cmd)
    }

    fn write_external_provider(&self) {
        let provider = self.provider;
        let provider_path = self.write_script("external-provider.py", external_provider_script());
        let turn_script_path = self.write_script("s11-turns.py", turn_script());
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"provider = {{ path = {} }}
prompt_mode = "arg"

[[providers]]
name = "{provider}"
args = []
"#,
                toml_string(&path_string(&provider_path))
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{provider}]
command = "fixture-opencode"
args = []
prompt_mode = "arg"
"#
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                r#"[{provider}]
turn_script = {}
"#,
                toml_string(&path_string(&turn_script_path))
            ),
        )
        .unwrap();
    }

    fn remove_turn_script_fallback(&self) {
        fs::remove_file(self.app_config_dir.join("sessions.toml")).unwrap();
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn seed_detached_child_completion(&self, owner_invocation_uuid: &str) -> MailboxRow {
        let state_dir = self.dir.path().join("detached-child");
        fs::create_dir_all(&state_dir).unwrap();
        let artifact = state_dir.join("result.json");
        let artifact_bytes = b"{\"status\":\"PASS\"}\n";
        fs::write(&artifact, artifact_bytes).unwrap();
        let artifact_sha256 = format!("{:x}", Sha256::digest(artifact_bytes));
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        fs::write(
            &meta,
            serde_json::json!({
                "owner_session_id": SESSION,
                "owner_invocation_uuid": owner_invocation_uuid,
                "caller_chain": [],
            })
            .to_string(),
        )
        .unwrap();
        fs::write(&log, "detached child completed with PASS\n").unwrap();
        fs::write(&rc, "0\n").unwrap();
        let payload_json = serde_json::json!({
            "schema_version": 1,
            "kind": "agent_bash_complete",
            "handle": "age291-detached-child",
            "state_dir": path_string(&state_dir),
            "meta_path": path_string(&meta),
            "log_path": path_string(&log),
            "rc_path": path_string(&rc),
            "rc": 0,
            "terminal_artifact": {
                "path": path_string(&artifact),
                "sha256": artifact_sha256,
            },
        })
        .to_string();
        let mut mailbox = self.mailbox();
        match mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle: "age291-detached-child",
                payload_json: &payload_json,
                owner_invocation_uuid: Some(owner_invocation_uuid),
                matched_os_pid: Some(9000),
                matched_os_boot_id: Some("age291-fixture-boot"),
                matched_os_pid_starttime_ticks: Some(1),
                matched_chain_index: Some(0),
                state_dir: &path_string(&state_dir),
                meta_path: &path_string(&meta),
                log_path: &path_string(&log),
                rc_path: &path_string(&rc),
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("expected inserted detached-child notification, got {other:?}"),
        }
    }

    fn finalized_invocation_count(&self) -> i64 {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM invocations WHERE finished_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn latest_resume_acceptance(&self) -> (Option<String>, Option<String>) {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT resume_acceptance_status, resume_acceptance_evidence
                 FROM invocations
                 WHERE session_capture_method = 'resumed'
                 ORDER BY id DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
            .expect("expected a resumed invocation row")
    }

    fn latest_resumed_provider_identity(&self) -> (String, String) {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT provider_name, provider_session_id
                 FROM invocations
                 WHERE session_capture_method = 'resumed'
                 ORDER BY id DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    fn latest_invocation_uuid(&self) -> String {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT invocation_uuid FROM invocations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn mailbox_row(&self, seq: i64) -> MailboxRow {
        self.mailbox()
            .list_mailbox(SESSION, true)
            .unwrap()
            .into_iter()
            .find(|row| row.seq == seq)
            .unwrap()
    }

    fn assert_xdg_isolated(&self) {
        assert!(
            !self
                .home_dir
                .join(".local/share/oulipoly-agent-runner")
                .exists(),
            "state must stay in isolated XDG_DATA_HOME"
        );
        assert!(
            !self.home_dir.join(".config/oulipoly-agent-runner").exists(),
            "config must stay in isolated XDG_CONFIG_HOME"
        );
    }
}

fn assert_unconfirmed_resume(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{stdout}");
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let result: Value = serde_json::from_str(lines[0]).unwrap();
    let mut keys = result
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent_runner_chain_id",
            "agent_runner_invocation_id",
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "provider_name",
            "provider_session_id",
            "status",
            "success",
            "terminal_reason"
        ]
    );
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["error_category"], "resume_completion_unconfirmed");
    assert_eq!(result["terminal_reason"], "resume_completion_unconfirmed");
    assert_eq!(result["provider_name"], PROVIDER);
    assert_eq!(result["provider_session_id"], SESSION);
    assert_eq!(result["agent_runner_invocation_id"], result["id"]);
}

#[test]
fn external_provider_runtime_uses_ingested_session_when_launch_capture_missing() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider without launch session capture",
        &[("S11_OMIT_EXIT_SESSION", "1")],
    );
    assert_success(&output);

    wait_until("initial invocation finalized", || {
        fixture.finalized_invocation_count() >= 1
    });
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    let expected_models_dir = path_string(&fixture.models_dir);
    assert_eq!(runtime.provider_name.as_deref(), Some(PROVIDER));
    assert_eq!(runtime.model_name.as_deref(), Some(MODEL));
    assert_eq!(
        runtime.models_dir.as_deref(),
        Some(expected_models_dir.as_str())
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn submitted_turn_prompt_hash_accepts_exact_and_rejects_mismatch_without_delivery_nonce() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    assert_success(&fixture.run_agent_with_env("seed manual resume", &[]));

    let output = fixture.run_resume_with_env(
        "manual exact payload",
        &[("S11_EMIT_SUBMITTED_TURN_MARKER", "1")],
    );
    assert_unconfirmed_resume(&output);
    let (status, evidence) = fixture.latest_resume_acceptance();
    assert_eq!(status.as_deref(), Some("accepted"));
    assert!(
        evidence
            .as_deref()
            .is_some_and(|value| value.contains("exact session and prompt SHA-256")),
        "{evidence:?}"
    );

    let fixture = Fixture::new();
    fixture.write_external_provider();
    assert_success(&fixture.run_agent_with_env("seed hash mismatch", &[]));
    let output = fixture.run_resume_with_env(
        "manual hash mismatch",
        &[
            ("S11_EMIT_SUBMITTED_TURN_MARKER", "1"),
            ("S11_MARKER_PROMPT_SHA_MISMATCH", "1"),
        ],
    );
    assert_unconfirmed_resume(&output);
    assert_eq!(
        fixture.latest_resume_acceptance(),
        (None, None),
        "prompt hash mismatch without a nonce"
    );
}

#[test]
fn accepted_owner_session_consumes_detached_child_completion_despite_ingest_evidence_loss() {
    for provider in OPENCODE_PROVIDERS {
        assert_owner_session_consumes_detached_child_completion(provider);
    }
}

#[test]
fn trusted_submission_settles_mailbox_delivery_after_provider_nonzero() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    fixture.remove_turn_script_fallback();
    assert_success(&fixture.run_agent_with_env("owner waits for detached child", &[]));
    let owner_invocation_uuid = fixture.latest_invocation_uuid();
    let notification = fixture.seed_detached_child_completion(&owner_invocation_uuid);

    let resumed = fixture.run_resume_with_env(
        "continue owning workflow",
        &[
            ("S11_EMIT_SUBMITTED_TURN_MARKER", "1"),
            ("S11_NO_ASSISTANT_RESULT", "1"),
            ("S11_EXIT_NONZERO", "1"),
        ],
    );

    assert_eq!(resumed.status.code(), Some(29), "{resumed:?}");
    let result = result_envelope(&resumed);
    let invocation_uuid = result["id"].as_str().unwrap();
    assert_eq!(result["status"], "failed");
    assert_eq!(result["exit_code"], 29);
    assert_eq!(
        result["terminal_reason"],
        "resume_prompt_accepted_provider_failed"
    );
    assert_eq!(
        fixture.latest_resume_acceptance().0.as_deref(),
        Some("accepted")
    );
    let delivered = fixture.mailbox_row(notification.seq);
    assert!(delivered.delivered_at.is_some(), "{delivered:?}");
    assert_eq!(delivered.delivery_attempts, 1);
    assert_eq!(
        delivered.delivered_by_invocation_uuid.as_deref(),
        Some(invocation_uuid)
    );
    fixture.assert_xdg_isolated();
}

fn assert_owner_session_consumes_detached_child_completion(provider: &'static str) {
    let positive = Fixture::with_provider(provider);
    positive.write_external_provider();
    positive.remove_turn_script_fallback();
    let owner = positive.run_agent_with_env(
        "owner waits for detached child",
        &[("S11_READ_TURNS_STDOUT_LIMIT", "1")],
    );
    assert_success(&owner);
    let owner_stderr = String::from_utf8_lossy(&owner.stderr);
    assert!(
        owner_stderr.contains("session.read_turns: stdout_limit_exceeded"),
        "incident ingest condition missing from owning log:\n{owner_stderr}"
    );
    let owner_invocation_uuid = positive.latest_invocation_uuid();
    let notification = positive.seed_detached_child_completion(&owner_invocation_uuid);

    let resumed = positive.run_resume_with_env(
        "continue owning workflow",
        &[
            ("S11_EMIT_SUBMITTED_TURN_MARKER", "1"),
            ("S11_EMIT_AFFIRMATIVE_ASSISTANT_RESULT", "1"),
            ("S11_READ_TURNS_STDOUT_LIMIT", "1"),
        ],
    );
    let resumed_result = result_envelope(&resumed);
    let resumed_invocation_uuid = resumed_result["id"].as_str().unwrap();
    let resumed_stderr = String::from_utf8_lossy(&resumed.stderr);
    let (acceptance, evidence) = positive.latest_resume_acceptance();
    assert_eq!(acceptance.as_deref(), Some("accepted"));
    assert_eq!(
        evidence.as_deref(),
        Some("validated submitted user turn: exact session and delivery nonce")
    );
    assert_eq!(resumed_result["exit_code"], 0);
    let (provider_name, provider_session_id) = positive.latest_resumed_provider_identity();
    assert_eq!(provider_session_id, SESSION);
    assert_eq!(provider_name, provider);
    assert_eq!(
        fs::read_to_string(positive.work_dir.join("affirmative-result")).unwrap(),
        "owner consumed detached child result and continued\n"
    );
    let delivered = positive.mailbox_row(notification.seq);
    assert!(delivered.delivered_at.is_some(), "{delivered:?}");
    assert_eq!(delivered.delivery_attempts, 1);
    assert_eq!(
        delivered.delivered_by_invocation_uuid.as_deref(),
        Some(resumed_invocation_uuid)
    );
    let trace = positive.run_trace(resumed_invocation_uuid);
    assert_success(&trace);
    let trace: Value = serde_json::from_slice(&trace.stdout).unwrap();
    assert_eq!(trace["root"]["session"]["transcript_state"], "no_locator");
    assert_eq!(trace["root"]["session"]["turn_count"], 0);
    assert_eq!(trace["root"]["session"]["assistant_turn_count"], 0);
    positive.assert_xdg_isolated();

    let no_assistant = Fixture::with_provider(provider);
    no_assistant.write_external_provider();
    no_assistant.remove_turn_script_fallback();
    let owner = no_assistant.run_agent_with_env("owner waits for detached child", &[]);
    assert_success(&owner);
    let owner_invocation_uuid = no_assistant.latest_invocation_uuid();
    let notification = no_assistant.seed_detached_child_completion(&owner_invocation_uuid);
    let unconfirmed = no_assistant.run_resume_with_env(
        "continue owning workflow",
        &[
            ("S11_EMIT_SUBMITTED_TURN_MARKER", "1"),
            ("S11_NO_ASSISTANT_RESULT", "1"),
            ("S11_READ_TURNS_STDOUT_LIMIT", "1"),
        ],
    );
    let unconfirmed_result = result_envelope(&unconfirmed);
    let unconfirmed_invocation_uuid = unconfirmed_result["id"].as_str().unwrap().to_owned();
    assert_eq!(unconfirmed.status.code(), Some(1), "{unconfirmed:?}");
    assert_eq!(unconfirmed_result["status"], "failed");
    assert_eq!(
        unconfirmed_result["error_category"],
        "resume_completion_unconfirmed"
    );
    let delivered = no_assistant.mailbox_row(notification.seq);
    assert!(delivered.delivered_at.is_some(), "{delivered:?}");
    assert_eq!(delivered.delivery_attempts, 1);
    assert_eq!(
        delivered.delivered_by_invocation_uuid.as_deref(),
        Some(unconfirmed_invocation_uuid.as_str())
    );

    let later_resume = no_assistant.run_resume_with_env(
        "continue after child completion",
        &[
            ("S11_EMIT_SUBMITTED_TURN_MARKER", "1"),
            ("S11_EMIT_AFFIRMATIVE_ASSISTANT_RESULT", "1"),
        ],
    );
    assert_success(&later_resume);
    let still_delivered = no_assistant.mailbox_row(notification.seq);
    assert_eq!(still_delivered.delivery_attempts, 1);
    assert_eq!(
        still_delivered.delivered_by_invocation_uuid.as_deref(),
        Some(unconfirmed_invocation_uuid.as_str())
    );
    no_assistant.assert_xdg_isolated();

    assert_eq!(
        resumed.status.code(),
        Some(0),
        "AGE-291: the exact owner session accepted the detached-child delivery nonce and the provider emitted an affirmative assistant result, but the owning continuation did not consume it\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&resumed.stdout),
        resumed_stderr,
    );
    assert_eq!(resumed_result["status"], "succeeded");
    assert_eq!(resumed_result["success"], true);
    assert!(resumed_result["error_category"].is_null());
}

#[test]
fn external_provider_policy_rejection_terminal_signal_excerpt_includes_diagnostics() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider reject",
        &[("S11_POLICY_REJECT", "1")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let marker = terminal_signal_marker(&stderr);
    let excerpt = marker
        .get("evidence")
        .and_then(|evidence| evidence.get("excerpt"))
        .and_then(Value::as_str)
        .expect("terminal-signal marker should include evidence excerpt");
    assert!(excerpt.contains("policy rejected"), "{excerpt}");
    assert!(excerpt.contains("s11_policy_reject"), "{excerpt}");
    assert!(excerpt.contains("params.launch.argv"), "{excerpt}");
    assert!(excerpt.contains("s11 fixture rejected policy"), "{excerpt}");
    fixture.assert_xdg_isolated();
}

fn external_provider_script() -> &'static str {
    r#"#!/usr/bin/env python3
import base64
import hashlib
import json
import os
import pathlib
import re
import sys

CONTRACT = "oulipoly.provider/v1"
SESSION = "ses_s11externalwake"

def request_id(request):
    return request.get("request_id", "s11-request")

def envelope(request, result):
    return {
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(request),
        "ok": True,
        "result": result,
    }

def describe(request):
    return envelope(request, {
        "provider_id": "s11-external-provider-runtime-fixture",
        "display_name": "S11 External Provider Runtime Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
            "launch": True,
            "policy": True,
            "quota": False,
            "session": True,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        },
    })

def policy_evaluate(request):
    if os.environ.get("S11_POLICY_REJECT") == "1":
        return envelope(request, {
            "accepted": False,
            "env": {},
            "stdin": None,
            "prompt": None,
            "diagnostics": [{
                "severity": "error",
                "code": "s11_policy_reject",
                "path": "params.launch.argv",
                "message": "s11 fixture rejected policy",
            }],
            "markers": [],
        })
    return envelope(request, {
        "accepted": True,
        "env": {},
        "stdin": None,
        "prompt": None,
        "diagnostics": [],
        "markers": [],
    })

def emit(event):
    print(json.dumps(event, separators=(",", ":")), flush=True)

def stdout_event(request, seq, payload):
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "stdout",
        "data_base64": base64.b64encode(payload.encode("utf-8")).decode("ascii"),
    }

def provider_session_marker_event(request, seq, session_id):
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.provider_session",
        "value": {"provider_session_id": session_id},
    }

def submitted_turn_marker_event(request, seq, session_id, prompt):
    prompt_sha = hashlib.sha256(prompt.encode("utf-8")).hexdigest()
    if os.environ.get("S11_MARKER_PROMPT_SHA_MISMATCH") == "1":
        prompt_sha = hashlib.sha256(b"different payload").hexdigest()
    value = {
        "provider_session_id": session_id,
        "prompt_sha256": prompt_sha,
        "source": "s11.fixture",
        "message_id": "msg-s11-submitted",
    }
    match = re.search(r"^\[OULIPOLY-DELIVERY ([^\]]+)\]$", prompt, re.MULTILINE)
    if match:
        value["delivery_nonce"] = match.group(1)
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.submitted_user_turn",
        "value": value,
    }

def produced_assistant_response_marker_event(request, seq):
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.produced_assistant_response",
        "value": True,
    }

def exit_event(request, seq, session_id):
    code = 29 if os.environ.get("S11_EXIT_NONZERO") == "1" else 0
    event = {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {"kind": "exited", "code": code},
        "terminal_signal": {
            "kind": "nonzero_exit" if code else "clean_exit",
            "evidence": "fixture nonzero exit" if code else "fixture clean exit",
            "observed_at_unix_ms": 1000 + seq,
        },
    }
    if session_id:
        event["session"] = {
            "provider_session_id": session_id,
            "state": {"cursor": "after-launch"},
        }
    return event

def launch(request):
    params = request.get("params", {})
    known = params.get("session", {}).get("known_provider_session_id")
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    if known:
        seq = 1
        produced_assistant_response = False
        if os.environ.get("S11_NO_ASSISTANT_RESULT") != "1":
            text = "resumed\n"
            if os.environ.get("S11_EMIT_AFFIRMATIVE_ASSISTANT_RESULT") == "1":
                text = "owner consumed detached child result and continued\n"
                pathlib.Path(os.environ["S11_WORK_DIR"]).joinpath("affirmative-result").write_text(text)
                produced_assistant_response = True
            emit(stdout_event(request, seq, text))
            seq += 1
        if os.environ.get("S11_EMIT_SUBMITTED_TURN_MARKER") == "1":
            emit(submitted_turn_marker_event(request, seq, known, prompt))
            seq += 1
        if produced_assistant_response:
            emit(produced_assistant_response_marker_event(request, seq))
            seq += 1
        emit(exit_event(request, seq, known))
        return
    session_id = None if os.environ.get("S11_OMIT_EXIT_SESSION") == "1" else SESSION
    seq = 1
    if session_id:
        emit(provider_session_marker_event(request, seq, session_id))
        seq += 1
    emit(stdout_event(request, seq, "initial\n"))
    emit(exit_event(request, seq + 1, session_id))

def session_id_from_request(request):
    params = request.get("params", {})
    extra = params.get("extra", {})
    return params.get("session_id") or extra.get("start_bound_provider_session_id") or SESSION

def read_turns(request):
    if os.environ.get("S11_READ_TURNS_STDOUT_LIMIT") == "1":
        sys.stdout.write("x" * (2 * 1024 * 1024))
        return None
    session_id = session_id_from_request(request)
    return envelope(request, {
        "turns": [{
            "session_id": session_id,
            "turn_id": "turn-s11-external-runtime",
            "role": "assistant",
            "timestamp": "2026-06-06T00:00:00Z",
            "body": [{"type": "text", "text": "fixture turn"}],
        }],
        "turn_count": 1,
        "complete": True,
    })

def capture(request):
    return envelope(request, {
        "provider_session_id": session_id_from_request(request),
        "state": {"captured": True},
        "artifacts": [],
    })

def main():
    subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
    request = json.loads(sys.stdin.read() or "{}")
    if subcommand == "describe":
        print(json.dumps(describe(request)))
        return 0
    if subcommand == "policy.evaluate":
        print(json.dumps(policy_evaluate(request)))
        return 0
    if subcommand == "launch":
        launch(request)
        return 0
    if subcommand == "session.read_turns":
        result = read_turns(request)
        if result is not None:
            print(json.dumps(result))
        return 0
    if subcommand == "session.capture":
        print(json.dumps(capture(request)))
        return 0
    print(json.dumps({
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(request),
        "ok": False,
        "error": {
            "category": "failed",
            "code": "unsupported_subcommand",
            "message": subcommand,
            "retryable": False,
        },
    }))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
"#
}

fn turn_script() -> &'static str {
    r#"#!/usr/bin/env python3
import json

print(json.dumps({
    "session_id": "ses_s11externalwake",
    "turn_id": "turn-s11-initial",
    "timestamp": "2026-06-06T00:00:00Z",
    "role": "assistant",
    "completion_outcome": "stop",
    "body": [{"type": "text", "text": "initial fixture turn"}],
}, separators=(",", ":")))
"#
}

fn assert_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn result_envelope(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "expected one result envelope: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(lines[0]).unwrap()
}

fn terminal_signal_marker(stderr: &str) -> Value {
    let marker = stderr
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_TERMINAL_SIGNAL="))
        .unwrap_or_else(|| panic!("missing terminal-signal marker in stderr:\n{stderr}"));
    serde_json::from_str(marker).unwrap()
}

fn wait_until(label: &str, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {label}");
}

fn runner_bin() -> &'static str {
    env!("CARGO_BIN_EXE_oulipoly-agent-runner")
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}
