#![cfg(unix)]

//! ## Declared roles
//!
//! Roles: orchestration, formatter, accessor, parser, validator.
//!
//! TEST: external-provider runtime fixtures for session ingestion, submitted
//! turn acceptance, and policy diagnostics.

use oulipoly_state::mailbox::MailboxDb;
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const MODEL: &str = "s11-external-wake-model";
const PROVIDER: &str = "opencode";
const SESSION: &str = "ses_s11externalwake";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    work_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
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

    fn write_external_provider(&self) {
        let provider_path = self.write_script("external-provider.py", external_provider_script());
        let turn_script_path = self.write_script("s11-turns.py", turn_script());
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"provider = {{ path = {} }}
prompt_mode = "arg"

[[providers]]
name = "{PROVIDER}"
args = []
"#,
                toml_string(&path_string(&provider_path))
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
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
                r#"[{PROVIDER}]
turn_script = {}
"#,
                toml_string(&path_string(&turn_script_path))
            ),
        )
        .unwrap();
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
    let runtime = fixture.mailbox().session_runtime(SESSION).unwrap().unwrap();
    let expected_models_dir = path_string(&fixture.models_dir);
    assert_eq!(runtime.run_state, "idle");
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
    assert_success(&output);
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
    assert_success(&output);
    assert_eq!(
        fixture.latest_resume_acceptance(),
        (None, None),
        "prompt hash mismatch without a nonce"
    );
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
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.submitted_user_turn",
        "value": {
            "provider_session_id": session_id,
            "prompt_sha256": prompt_sha,
            "source": "s11.fixture",
            "message_id": "msg-s11-submitted",
        },
    }

def exit_event(request, seq, session_id):
    event = {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {"kind": "exited", "code": 0},
        "terminal_signal": {
            "kind": "clean_exit",
            "evidence": "fixture clean exit",
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
        emit(stdout_event(request, 1, "resumed\n"))
        seq = 2
        if os.environ.get("S11_EMIT_SUBMITTED_TURN_MARKER") == "1":
            emit(submitted_turn_marker_event(request, seq, known, prompt))
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
        print(json.dumps(read_turns(request)))
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
