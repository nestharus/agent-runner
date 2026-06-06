#![cfg(unix)]

use oulipoly_state::mailbox::MailboxDb;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const MODEL: &str = "s11-external-wake-model";
const PROVIDER: &str = "opencode";
const SESSION: &str = "ses_s11externalwake";
const HANDLE: &str = "h-s11-external";

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
            .env("AGENT_BASH_AGENT_RUNNER_BIN", runner_bin())
            .env("S11_WORK_DIR", &self.work_dir)
            .env_remove("OULIPOLY_DATA_DIR")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .current_dir(self.dir.path());
        cmd.output().unwrap()
    }

    fn run_agent(&self, prompt: &str) -> Output {
        self.run_agent_with_env(prompt, &[])
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

    fn write_external_provider(&self) {
        let provider_path = self.write_external_provider_script();
        let turn_script_path = self.write_turn_script();
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

    fn write_external_provider_script(&self) -> PathBuf {
        let path = self.dir.path().join("external-provider.py");
        fs::write(&path, external_provider_script()).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn write_turn_script(&self) -> PathBuf {
        let path = self.dir.path().join("s11-turns.py");
        fs::write(&path, turn_script()).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn sidecar_conn(&self) -> Connection {
        Connection::open(self.sidecar_path()).unwrap()
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

    fn pid_identity_session_id_for_provider(&self, provider_name: &str) -> Option<String> {
        self.sidecar_conn()
            .query_row(
                "SELECT session_id
                 FROM pid_identity
                 WHERE provider_name = ?1
                 ORDER BY recorded_at DESC, os_pid DESC
                 LIMIT 1",
                params![provider_name],
                |row| row.get(0),
            )
            .ok()
    }

    fn prompt_file(&self, name: &str) -> PathBuf {
        self.work_dir.join(name)
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
fn external_provider_launch_notify_uses_captured_sidecar_owner_and_wakes() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent("dispatch external provider wake");
    assert_success(&output);

    wait_until("pid identity backfilled with captured session", || {
        fixture
            .pid_identity_session_id_for_provider(PROVIDER)
            .as_deref()
            == Some(SESSION)
    });
    let (notify_json, notify) =
        wait_for_json_file(&fixture.work_dir.join(HANDLE).join("notify.json"));
    let (_, notify_env) =
        wait_for_json_file(&fixture.work_dir.join(HANDLE).join("notify-env.json"));
    assert_eq!(
        notify_env.get("xdg_config_home").and_then(Value::as_str),
        None
    );
    assert_eq!(
        notify_env.get("xdg_data_home").and_then(Value::as_str),
        None
    );
    assert_eq!(
        notify_env.get("oulipoly_data_dir").and_then(Value::as_str),
        Some(path_string(&fixture.data_home.join("oulipoly-agent-runner")).as_str())
    );
    assert_eq!(
        notify.get("status").and_then(Value::as_str),
        Some("enqueued"),
        "notify response: {notify_json}"
    );
    assert_eq!(notify.get("enqueued").and_then(Value::as_bool), Some(true));
    assert_eq!(
        notify.get("owner_session_id").and_then(Value::as_str),
        Some(SESSION)
    );
    assert_eq!(
        notify.get("session_source").and_then(Value::as_str),
        Some("sidecar_session_id")
    );
    assert_eq!(
        notify
            .get("wake")
            .and_then(|wake| wake.get("status"))
            .and_then(Value::as_str),
        Some("spawned"),
        "notify response: {notify_json}"
    );
    let prompt = wait_for_file(&fixture.prompt_file("external-wake-resumed.txt"));
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    assert!(prompt.contains("kind: agent_bash_complete"), "{prompt}");
    assert!(prompt.contains("handle: h-s11-external"), "{prompt}");
    wait_until("external provider wake delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && rows[0].owner_invocation_uuid.is_some()
            && rows[0].matched_os_pid.is_some()
            && rows[0].matched_chain_index == Some(0)
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    let runtime = fixture.mailbox().session_runtime(SESSION).unwrap().unwrap();
    assert_eq!(runtime.run_state, "idle");
    let expected_models_dir = path_string(&fixture.models_dir);
    assert_eq!(
        runtime.models_dir.as_deref(),
        Some(expected_models_dir.as_str()),
        "detached wake must reload the same models directory as the original external launch"
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn external_provider_runtime_uses_ingested_session_when_launch_capture_missing() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider without launch session capture",
        &[("S11_NO_NOTIFY", "1"), ("S11_OMIT_EXIT_SESSION", "1")],
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
fn external_provider_wake_does_not_mark_delivered_when_resume_produces_no_turn() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider wake dropped payload",
        &[
            ("S11_DROP_RESUME_PAYLOAD", "1"),
            ("OULIPOLY_AUTO_WAKE_MAX", "1"),
        ],
    );
    assert_success(&output);

    let _dropped = wait_for_file(&fixture.prompt_file("external-wake-dropped.txt"));
    wait_until("failed wake released claim and recorded attempt", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivery_attempts == 1
            && rows[0].delivery_error.as_deref() == Some("mailbox_delivery_unconfirmed")
            && db.wake_claim(SESSION).unwrap().is_none()
    });

    let db = fixture.mailbox();
    let rows = db.list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert!(
        rows[0].delivered_at.is_none(),
        "dropped resume payload must not mark mailbox delivered: {rows:?}"
    );
    assert_eq!(db.list_pending(SESSION).unwrap().len(), 1);
    assert!(
        !fixture.prompt_file("external-wake-resumed.txt").exists(),
        "drop mode must not write the payload-bearing resume file"
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn external_provider_wake_confirms_delivery_from_submitted_turn_marker() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider wake confirmed by marker",
        &[
            ("S11_EMIT_SUBMITTED_TURN_MARKER", "1"),
            ("S11_SKIP_SCAN_WAKE_TURN", "1"),
        ],
    );
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("external-wake-resumed.txt"));
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    wait_until("submitted-turn marker delivered pending mailbox", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && rows[0].delivery_error.is_none()
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    fixture.assert_xdg_isolated();
}

#[test]
fn external_provider_wake_ignores_submitted_turn_marker_for_different_payload() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider wake mismatched marker",
        &[
            ("S11_EMIT_SUBMITTED_TURN_MARKER", "1"),
            ("S11_MARKER_PROMPT_SHA_MISMATCH", "1"),
            ("S11_SKIP_SCAN_WAKE_TURN", "1"),
            ("OULIPOLY_AUTO_WAKE_MAX", "1"),
        ],
    );
    assert_success(&output);

    let prompt = wait_for_file(&fixture.prompt_file("external-wake-resumed.txt"));
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    wait_until(
        "mismatched submitted-turn marker leaves mailbox pending",
        || {
            let db = fixture.mailbox();
            let rows = db.list_mailbox(SESSION, true).unwrap();
            rows.len() == 1
                && rows[0].delivered_at.is_none()
                && rows[0].delivery_attempts == 1
                && rows[0].delivery_error.as_deref() == Some("mailbox_delivery_unconfirmed")
                && db.wake_claim(SESSION).unwrap().is_none()
        },
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn external_provider_failed_wake_releases_claim_and_retries_pending_mailbox() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider wake first dropped payload",
        &[
            ("S11_DROP_FIRST_RESUME_PAYLOAD", "1"),
            ("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS", "1000"),
        ],
    );
    assert_success(&output);

    let _dropped = wait_for_file(&fixture.prompt_file("external-wake-dropped.txt"));
    wait_until("first failed wake released claim", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_none()
            && rows[0].delivery_attempts == 1
            && rows[0].delivery_error.as_deref() == Some("mailbox_delivery_unconfirmed")
            && db.wake_claim(SESSION).unwrap().is_none()
    });

    let prompt = wait_for_file(&fixture.prompt_file("external-wake-resumed.txt"));
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    wait_until("retried wake delivered pending mailbox", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && rows[0].delivery_attempts == 2
            && rows[0].delivery_error.is_none()
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    fixture.assert_xdg_isolated();
}

#[test]
fn external_provider_rate_limited_wake_records_error_and_retries_pending_mailbox() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider wake first rate limited",
        &[
            ("S11_RATE_LIMIT_FIRST_RESUME", "1"),
            ("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS", "1000"),
        ],
    );
    assert_success(&output);

    let _rate_limited = wait_for_file(&fixture.prompt_file("external-wake-rate-limited.txt"));
    wait_until("rate-limited wake released claim", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_none()
            && rows[0].delivery_attempts == 1
            && rows[0].delivery_error.as_deref() == Some("rate_limited")
            && db.wake_claim(SESSION).unwrap().is_none()
    });

    let prompt = wait_for_file(&fixture.prompt_file("external-wake-resumed.txt"));
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    wait_until("rate-limited wake retry delivered pending mailbox", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && rows[0].delivery_attempts == 2
            && rows[0].delivery_error.is_none()
            && db.wake_claim(SESSION).unwrap().is_none()
    });
    fixture.assert_xdg_isolated();
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
import subprocess
import sys
import time

CONTRACT = "oulipoly.provider/v1"
SESSION = "ses_s11externalwake"
HANDLE = "h-s11-external"

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
    terminal_capability = os.environ.get("S11_RATE_LIMIT_FIRST_RESUME") == "1"
    return envelope(request, {
        "provider_id": "s11-external-provider-wake-fixture",
        "display_name": "S11 External Provider Wake Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
            "launch": True,
            "policy": True,
            "quota": False,
            "session": True,
            "terminal": terminal_capability,
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

def provider_identity():
    pid = os.getpid()
    boot_id = pathlib.Path("/proc/sys/kernel/random/boot_id").read_text(encoding="utf-8").strip()
    stat = pathlib.Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    after = stat.rsplit(") ", 1)[1]
    start_ticks = int(after.split()[19])
    return pid, boot_id, start_ticks

def spawn_notify_workload():
    pid, boot_id, start_ticks = provider_identity()
    subprocess.Popen(
        [sys.executable, __file__, "notify-helper", str(pid), boot_id, str(start_ticks)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
        env=os.environ.copy(),
    )

def launch(request):
    params = request.get("params", {})
    known = params.get("session", {}).get("known_provider_session_id")
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    if known:
        if os.environ.get("S11_RATE_LIMIT_FIRST_RESUME") == "1":
            count_path = pathlib.Path(os.environ["S11_WORK_DIR"]) / "resume-count.txt"
            count = int(count_path.read_text(encoding="utf-8")) if count_path.exists() else 0
            count_path.write_text(str(count + 1), encoding="utf-8")
            if count == 0:
                target = pathlib.Path(os.environ["S11_WORK_DIR"]) / "external-wake-rate-limited.txt"
                target.write_text("rate limited\n", encoding="utf-8")
                emit(stdout_event(request, 1, "HTTP 429 rate_limit_error\n"))
                emit(exit_event(request, 2, known))
                return
        if os.environ.get("S11_DROP_FIRST_RESUME_PAYLOAD") == "1":
            count_path = pathlib.Path(os.environ["S11_WORK_DIR"]) / "resume-count.txt"
            count = int(count_path.read_text(encoding="utf-8")) if count_path.exists() else 0
            count_path.write_text(str(count + 1), encoding="utf-8")
            if count == 0:
                target = pathlib.Path(os.environ["S11_WORK_DIR"]) / "external-wake-dropped.txt"
                target.write_text("dropped\n", encoding="utf-8")
                emit(stdout_event(request, 1, "dropped\n"))
                emit(exit_event(request, 2, known))
                return
        if os.environ.get("S11_DROP_RESUME_PAYLOAD") == "1":
            target = pathlib.Path(os.environ["S11_WORK_DIR"]) / "external-wake-dropped.txt"
            target.write_text("dropped\n", encoding="utf-8")
            emit(stdout_event(request, 1, "dropped\n"))
            emit(exit_event(request, 2, known))
            return
        target = pathlib.Path(os.environ["S11_WORK_DIR"]) / "external-wake-resumed.txt"
        target.write_text(prompt, encoding="utf-8")
        emit(stdout_event(request, 1, "resumed\n"))
        if os.environ.get("S11_SKIP_SCAN_WAKE_TURN") == "1":
            (pathlib.Path(os.environ["S11_WORK_DIR"]) / "skip-scan-wake-turn.txt").write_text("skip\n", encoding="utf-8")
        if os.environ.get("S11_EMIT_SUBMITTED_TURN_MARKER") == "1":
            emit(submitted_turn_marker_event(request, 2, known, prompt))
            emit(exit_event(request, 3, known))
        else:
            emit(exit_event(request, 2, known))
        return
    if os.environ.get("S11_NO_NOTIFY") != "1":
        spawn_notify_workload()
    emit(stdout_event(request, 1, "initial\n"))
    session_id = None if os.environ.get("S11_OMIT_EXIT_SESSION") == "1" else SESSION
    emit(exit_event(request, 2, session_id))

def session_id_from_request(request):
    params = request.get("params", {})
    extra = params.get("extra", {})
    return params.get("session_id") or extra.get("start_bound_provider_session_id") or SESSION

def read_turns(request):
    session_id = session_id_from_request(request)
    return envelope(request, {
        "turns": [{
            "session_id": session_id,
            "turn_id": "turn-s11-external-wake",
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

def terminal_classify(request):
    count_path = pathlib.Path(os.environ["S11_WORK_DIR"]) / "resume-count.txt"
    first_rate_limited_resume = (
        os.environ.get("S11_RATE_LIMIT_FIRST_RESUME") == "1"
        and count_path.exists()
        and count_path.read_text(encoding="utf-8").strip() == "1"
    )
    if first_rate_limited_resume:
        kind = "rate_limited"
        evidence = "HTTP 429 rate_limit_error"
    else:
        kind = "clean_exit"
        evidence = "fixture clean exit"
    return envelope(request, {
        "terminal_signal": {
            "kind": kind,
            "evidence": evidence,
            "observed_at_unix_ms": 2005,
        },
    })

def notify_helper():
    provider_pid = int(sys.argv[2])
    boot_id = sys.argv[3]
    start_ticks = int(sys.argv[4])
    time.sleep(1.5)
    work = pathlib.Path(os.environ["S11_WORK_DIR"])
    state = work / HANDLE
    state.mkdir(parents=True, exist_ok=True)
    (state / "meta.json").write_text(json.dumps({
        "caller_chain": [{
            "pid": provider_pid,
            "boot_id": boot_id,
            "starttime_ticks": start_ticks,
        }]
    }), encoding="utf-8")
    (state / "log").write_text("external provider workload completed\n", encoding="utf-8")
    (state / "rc").write_text("0\n", encoding="utf-8")
    runner = os.environ["AGENT_BASH_AGENT_RUNNER_BIN"]
    child_env = os.environ.copy()
    child_env["OULIPOLY_DATA_DIR"] = str(pathlib.Path(os.environ["XDG_DATA_HOME"]) / "oulipoly-agent-runner")
    child_env.pop("XDG_CONFIG_HOME", None)
    child_env.pop("XDG_DATA_HOME", None)
    (state / "notify-env.json").write_text(json.dumps({
        "xdg_config_home": child_env.get("XDG_CONFIG_HOME"),
        "xdg_data_home": child_env.get("XDG_DATA_HOME"),
        "oulipoly_data_dir": child_env.get("OULIPOLY_DATA_DIR"),
    }, separators=(",", ":")), encoding="utf-8")
    with (state / "notify.json").open("w", encoding="utf-8") as out, \
         (state / "notify.err").open("w", encoding="utf-8") as err:
        subprocess.run([
            runner, "notify", "agent-bash-complete",
            "--caller-ppid", str(provider_pid),
            "--handle", HANDLE,
            "--state-dir", str(state),
            "--meta", str(state / "meta.json"),
            "--log", str(state / "log"),
            "--rc", str(state / "rc"),
            "--json",
        ], stdout=out, stderr=err, check=False, env=child_env)

def main():
    subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
    if subcommand == "notify-helper":
        notify_helper()
        return 0
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
    if subcommand == "terminal.classify":
        print(json.dumps(terminal_classify(request)))
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
import os
import pathlib

SESSION = "ses_s11externalwake"

def emit(turn_id, text):
    print(json.dumps({
        "session_id": SESSION,
        "turn_id": turn_id,
        "timestamp": "2026-06-06T00:00:00Z",
        "role": "assistant",
        "body": [{"type": "text", "text": text}],
    }, separators=(",", ":")))

work = pathlib.Path(os.environ["S11_WORK_DIR"])
emit("turn-s11-initial", "initial fixture turn")
if (work / "external-wake-resumed.txt").exists() and not (work / "skip-scan-wake-turn.txt").exists():
    emit("turn-s11-woke", "WOKE reply")
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
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_TERMINAL_SIGNAL="))
        .unwrap_or_else(|| panic!("missing terminal-signal marker in stderr:\n{stderr}"));
    serde_json::from_str(line).unwrap()
}

fn wait_for_file(path: &Path) -> String {
    wait_until(&format!("{} exists", path.display()), || path.exists());
    fs::read_to_string(path).unwrap()
}

fn wait_for_json_file(path: &Path) -> (String, Value) {
    let mut parsed = None;
    wait_until(&format!("{} contains JSON", path.display()), || {
        let Ok(text) = fs::read_to_string(path) else {
            return false;
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(value) => {
                parsed = Some((text, value));
                true
            }
            Err(_) => false,
        }
    });
    parsed.unwrap()
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
