//! ## Declared roles
//!
//! Roles: fixture, orchestration, validator.
//!
//! TEST: native host-memory admission through detached proactive wake delivery.

use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, EnqueueResult, MailboxDb, SessionMetadataUpsert,
};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const MODEL: &str = "age309-native-wake-model";
const PROVIDER: &str = "age309-native-wake-provider";
const SESSION: &str = "ses_age309_native_wake";

struct Fixture {
    root: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config: PathBuf,
    models_dir: PathBuf,
    marker: PathBuf,
    provider_gate: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config_home = root.path().join("config");
        let data_home = root.path().join("data");
        let app_config = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config.join("models");
        let home_dir = root.path().join("home");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        let marker = root.path().join("native-resume-prompts.jsonl");
        let provider_gate = root.path().join("allow-native-provider-exit");
        let fixture = Self {
            root,
            config_home,
            data_home,
            home_dir,
            app_config,
            models_dir,
            marker,
            provider_gate,
        };
        fixture.write_provider();
        fixture
    }

    fn write_provider(&self) {
        let script = self.root.path().join("native-provider.py");
        fs::write(&script, provider_script()).unwrap();
        let wrapper = write_provider_wrapper(self.root.path(), &script);
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                "provider = {{ path = {} }}\nprompt_mode = \"arg\"\n\n[[providers]]\nname = \"{PROVIDER}\"\nargs = []\n",
                toml_string(&path_string(&wrapper))
            ),
        )
        .unwrap();
        fs::write(
            self.app_config.join("providers.toml"),
            format!(
                "[{PROVIDER}]\ncommand = \"age309-native-fixture\"\nargs = []\nprompt_mode = \"arg\"\n"
            ),
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command
            .env("OULIPOLY_CONFIG_HOME", &self.config_home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.home_dir)
            .env(
                "OULIPOLY_DATA_DIR",
                self.data_home.join("oulipoly-agent-runner"),
            )
            .env("AGE309_NATIVE_WAKE_MARKER", &self.marker)
            .env("AGE309_NATIVE_WAKE_GATE", &self.provider_gate)
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .env_remove("OULIPOLY_AUTO_WAKE")
            .env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID")
            .env_remove("OULIPOLY_AUTO_WAKE_TOKEN")
            .env_remove("OULIPOLY_AUTO_WAKE_COUNT")
            .env_remove("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS")
            .current_dir(self.root.path());
        command
    }

    fn run_initial(&self) -> Output {
        let mut command = self.command();
        command
            .arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("establish native wake session");
        command.output().unwrap()
    }

    fn run_startup_sweep(&self) -> Output {
        let mut command = self.command();
        command
            .arg("mailbox")
            .arg("list")
            .arg("--session-id")
            .arg(SESSION)
            .arg("--json");
        command.output().unwrap()
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

    fn seed_pending_delivery(&self, owner_invocation_uuid: &str) {
        let state_dir = self.root.path().join("completed-child");
        fs::create_dir_all(&state_dir).unwrap();
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
        fs::write(&log, "native child completed\n").unwrap();
        fs::write(&rc, "0\n").unwrap();
        let payload_json = serde_json::json!({
            "schema_version": 1,
            "kind": "agent_bash_complete",
            "handle": "age309-native-child",
            "state_dir": path_string(&state_dir),
            "meta_path": path_string(&meta),
            "log_path": path_string(&log),
            "rc_path": path_string(&rc),
            "rc": 0,
        })
        .to_string();
        let mut mailbox = MailboxDb::open(&self.sidecar_path()).unwrap();
        let result = mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle: "age309-native-child",
                payload_json: &payload_json,
                owner_invocation_uuid: Some(owner_invocation_uuid),
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: &path_string(&state_dir),
                meta_path: &path_string(&meta),
                log_path: &path_string(&log),
                rc_path: &path_string(&rc),
                rc: 0,
            })
            .unwrap();
        assert!(matches!(result, EnqueueResult::Inserted(_)));
        let models_dir = path_string(&self.models_dir);
        mailbox
            .wake_sessions()
            .upsert_session_metadata(SessionMetadataUpsert {
                session_id: SESSION,
                mode: "headless",
                invocation_uuid: Some(owner_invocation_uuid),
                provider_name: Some(PROVIDER),
                model_name: Some(MODEL),
                models_dir: Some(&models_dir),
                effective_cwd: None,
            })
            .unwrap();
        Connection::open(self.sidecar_path())
            .unwrap()
            .execute(
                "UPDATE session_runtime SET auto_wake_count = 5 WHERE session_id = ?1",
                [SESSION],
            )
            .unwrap();
    }
}

#[test]
fn native_count_five_startup_sweep_reaches_one_detached_provider_turn() {
    let fixture = Fixture::new();
    let initial = fixture.run_initial();
    assert_success(&initial);
    let owner_invocation_uuid = result_id(&initial);
    fixture.seed_pending_delivery(&owner_invocation_uuid);

    let sweep = fixture.run_startup_sweep();
    assert_success(&sweep);
    assert!(
        wait_until(|| fixture.marker.exists()),
        "native detached provider turn did not start\nsweep stdout={}\nsweep stderr={}\nmailbox={:?}\nruntime={:?}\nclaim={:?}",
        String::from_utf8_lossy(&sweep.stdout),
        String::from_utf8_lossy(&sweep.stderr),
        MailboxDb::open(&fixture.sidecar_path())
            .unwrap()
            .list_mailbox(SESSION, true)
            .unwrap(),
        MailboxDb::open(&fixture.sidecar_path())
            .unwrap()
            .wake_session_reader()
            .session_metadata(SESSION)
            .unwrap(),
        MailboxDb::open(&fixture.sidecar_path())
            .unwrap()
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
    );
    let live_claim = MailboxDb::open(&fixture.sidecar_path())
        .unwrap()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .expect("one live native wake claim while provider turn is blocked");
    assert!(!live_claim.claim_token.is_empty());
    assert_eq!(live_claim.auto_wake_count, 6);
    assert_eq!(live_claim.min_pending_seq_at_claim, Some(1));
    assert_eq!(live_claim.max_pending_seq_at_claim, Some(1));
    assert!(live_claim.wake_pid.is_some());
    fs::write(&fixture.provider_gate, "continue\n").unwrap();
    assert!(wait_until(|| {
        let mailbox = MailboxDb::open(&fixture.sidecar_path()).unwrap();
        let rows = mailbox.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && mailbox
                .wake_session_reader()
                .wake_claim(SESSION)
                .unwrap()
                .is_none()
    }));

    let mailbox = MailboxDb::open(&fixture.sidecar_path()).unwrap();
    let row = mailbox.list_mailbox(SESSION, true).unwrap().remove(0);
    assert_eq!(row.delivery_attempts, 1);
    assert!(row.delivery_error.is_none());
    assert!(
        mailbox
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .is_none()
    );
    let connection = Connection::open(fixture.state_path()).unwrap();
    let resumed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM invocations WHERE session_capture_method = 'resumed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(resumed, 1);
    let marker_lines = fs::read_to_string(&fixture.marker).unwrap();
    assert_eq!(marker_lines.lines().count(), 1);
    let runtime_count: i64 = Connection::open(fixture.sidecar_path())
        .unwrap()
        .query_row(
            "SELECT auto_wake_count FROM session_runtime WHERE session_id = ?1",
            [SESSION],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(runtime_count, 6);
}

fn result_id(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = stdout
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .expect("runner result envelope");
    serde_json::from_str::<serde_json::Value>(result).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_until(mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[cfg(unix)]
fn write_provider_wrapper(root: &Path, script: &Path) -> PathBuf {
    let wrapper = root.join("native-provider.sh");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexec {} {} \"$@\"\n",
            shell_quote(&python_executable()),
            shell_quote(script)
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&wrapper).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&wrapper, permissions).unwrap();
    wrapper
}

#[cfg(windows)]
fn write_provider_wrapper(root: &Path, script: &Path) -> PathBuf {
    let wrapper = root.join("native-provider.cmd");
    fs::write(
        &wrapper,
        format!(
            "@echo off\r\n\"{}\" \"{}\" %*\r\n",
            python_executable().display(),
            script.display()
        ),
    )
    .unwrap();
    wrapper
}

fn python_executable() -> PathBuf {
    for candidate in ["python3", "python"] {
        let output = Command::new(candidate)
            .args(["-c", "import sys; print(sys.executable)"])
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            let path = String::from_utf8(output.stdout).unwrap();
            return PathBuf::from(path.trim());
        }
    }
    panic!("native wake fixture requires Python");
}

#[cfg(unix)]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}

fn provider_script() -> &'static str {
    r#"import base64
import hashlib
import json
import os
import pathlib
import sys
import time

CONTRACT = "oulipoly.provider/v1"
SESSION = "ses_age309_native_wake"

def envelope(request, result):
    return {"contract": CONTRACT, "request_id": request["request_id"], "ok": True, "result": result}

def event(request, seq, kind, **fields):
    value = {"contract": CONTRACT, "request_id": request["request_id"], "seq": seq, "time_unix_ms": 1000 + seq, "kind": kind}
    value.update(fields)
    print(json.dumps(value, separators=(",", ":")), flush=True)

def launch(request):
    params = request.get("params", {})
    known = params.get("session", {}).get("known_provider_session_id")
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    seq = 1
    if known:
        marker = pathlib.Path(os.environ["AGE309_NATIVE_WAKE_MARKER"])
        with marker.open("a") as stream:
            stream.write(json.dumps(prompt, separators=(",", ":")) + "\n")
        gate = pathlib.Path(os.environ["AGE309_NATIVE_WAKE_GATE"])
        deadline = time.monotonic() + 20
        while not gate.exists() and time.monotonic() < deadline:
            time.sleep(0.02)
        event(request, seq, "stdout", data_base64=base64.b64encode(b"native resumed\n").decode("ascii"))
        seq += 1
        acceptance = params["prompt_acceptance"]
        event(request, seq, "marker", name="oulipoly.prompt_accepted/v1", value={
            "protocol": "oulipoly.prompt_acceptance/v1",
            "provider_session_id": known,
            "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
            "delivery_nonce": acceptance["delivery_nonce"],
            "source": "age309.native.fixture",
            "message_id": "native-accepted",
        })
        seq += 1
        event(request, seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1
    else:
        known = SESSION
        event(request, seq, "marker", name="oulipoly.provider_session", value={"provider_session_id": known})
        seq += 1
        event(request, seq, "stdout", data_base64=base64.b64encode(b"native initial\n").decode("ascii"))
        seq += 1
    event(request, seq, "exit", status={"kind": "exited", "code": 0}, terminal_signal={"kind": "clean_exit", "evidence": "native fixture clean exit", "observed_at_unix_ms": 1000 + seq}, session={"provider_session_id": known, "state": {"cursor": "native"}})

request = json.loads(sys.stdin.read() or "{}")
method = sys.argv[1] if len(sys.argv) > 1 else ""
if method == "describe":
    print(json.dumps(envelope(request, {
        "provider_id": "age309-native-wake-fixture",
        "display_name": "AGE-309 Native Wake Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {"launch": True, "policy": True, "quota": False, "session": True, "terminal": False, "rotation": False, "discovery": False, "settings": False, "setup_brain": False, "setup": False, "migration": False, "prompt_acceptance_v1": True},
    })))
elif method == "policy.evaluate":
    print(json.dumps(envelope(request, {"accepted": True, "env": {}, "stdin": None, "prompt": None, "diagnostics": [], "markers": []})))
elif method == "launch":
    launch(request)
elif method == "session.read_turns":
    print(json.dumps(envelope(request, {"turns": [{"session_id": SESSION, "turn_id": "native-turn", "role": "assistant", "timestamp": "2026-08-29T00:00:00Z", "body": [{"type": "text", "text": "native fixture turn"}]}], "turn_count": 1, "complete": True})))
elif method == "session.capture":
    print(json.dumps(envelope(request, {"provider_session_id": SESSION, "state": {"captured": True}, "artifacts": []})))
else:
    print(json.dumps({"contract": CONTRACT, "request_id": request.get("request_id", "missing"), "ok": False, "error": {"category": "failed", "code": "unsupported_subcommand", "message": method, "retryable": False}}))
"#
}
