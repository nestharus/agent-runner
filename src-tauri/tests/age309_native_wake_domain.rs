//! ## Declared roles
//!
//! Roles: fixture, orchestration, validator.
//!
//! TEST: native host-memory admission through detached proactive wake delivery.

mod provider_authority_fixture;

use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, EnqueueResult, MailboxDb, MailboxDeliveryObservationAnchor,
    SessionMetadataUpsert,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const MODEL: &str = "age309-native-wake-model";
const PROVIDER: &str = "age309-native-wake-provider";
const PROVIDER_INSTANCE_ID: &str = "age309-native-wake-fixture-instance";
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
            format!("prompt_mode = \"arg\"\n\n[[providers]]\nname = \"{PROVIDER}\"\nargs = []\n"),
        )
        .unwrap();
        fs::write(
            self.app_config.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority_at(
                &format!(
                "[{PROVIDER}]\ncommand = \"age309-native-fixture\"\nargs = []\nprompt_mode = \"arg\"\nsettings_id = \"{PROVIDER}\"\n"
            ),
                "age309-native-wake",
                &wrapper,
            ),
        )
        .unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("OULIPOLY_CONFIG_HOME", &self.config_home)
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

    fn start_startup_sweep(&self) -> Child {
        let mut command = self.command();
        command
            .arg("mailbox")
            .arg("list")
            .arg("--session-id")
            .arg(SESSION)
            .arg("--json")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.spawn().unwrap()
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

    fn seed_active_chain(&self) {
        drop(oulipoly_state::StateDb::open(&self.state_path()).unwrap());
        let connection = Connection::open(self.state_path()).unwrap();
        connection
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES ('age309-recovery-chain', '2026-08-30T12:00:00Z',
                         '2026-08-30T12:00:00Z', ?1)",
                [MODEL],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES ('age309-recovery-chain', ?1, ?2,
                         '2026-08-30T12:00:00Z', 'initial')",
                [PROVIDER, SESSION],
            )
            .unwrap();
        provider_authority_fixture::bind_session_authority_with_cwd_at(
            &connection,
            PROVIDER,
            SESSION,
            PROVIDER_INSTANCE_ID,
            PROVIDER,
            self.root.path(),
        );
    }

    fn seed_crashed_delivery_observation(&self, prompt: &str) {
        fs::write(
            &self.marker,
            format!("{}\n", serde_json::to_string(prompt).unwrap()),
        )
        .unwrap();
        let mut mailbox = MailboxDb::open(&self.sidecar_path()).unwrap();
        let row = mailbox.list_mailbox(SESSION, true).unwrap().remove(0);
        let attempt_id = "b".repeat(64);
        mailbox
            .register_headless_delivery_attempt(
                &attempt_id,
                SESSION,
                Some("age309-recovery-chain"),
                "age309-crashed-invocation",
                &[row.seq],
                0,
            )
            .unwrap();
        mailbox
            .record_delivery_observation_anchor(
                &attempt_id,
                SESSION,
                &MailboxDeliveryObservationAnchor {
                    provider_name: PROVIDER.to_string(),
                    provider_instance_id: PROVIDER_INSTANCE_ID.to_string(),
                    settings_id: PROVIDER.to_string(),
                    provider_session_id: SESSION.to_string(),
                    resume_token: "age309-anchor:0".to_string(),
                    expected_sha256: format!("{:x}", Sha256::digest(prompt.trim().as_bytes())),
                },
            )
            .unwrap();
    }
}

#[test]
fn native_count_five_startup_sweep_reaches_one_detached_provider_turn() {
    let fixture = Fixture::new();
    let initial = fixture.run_initial();
    assert_success(&initial);
    assert_eq!(initial.stdout, b"native initial\n");
    let owner_invocation_uuid = latest_invocation_uuid(&fixture.state_path());
    fixture.seed_pending_delivery(&owner_invocation_uuid);

    let sweep = fixture.start_startup_sweep();
    assert!(
        wait_until(|| fixture.marker.exists()),
        "native detached provider turn did not start\nmailbox={:?}\nruntime={:?}\nclaim={:?}\ninvocations={:?}",
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
            .unwrap(),
        invocation_diagnostics(&fixture.state_path())
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
    let sweep = sweep.wait_with_output().unwrap();
    assert_success(&sweep);
    assert!(
        wait_until(|| {
            let mailbox = MailboxDb::open(&fixture.sidecar_path()).unwrap();
            let rows = mailbox.list_mailbox(SESSION, true).unwrap();
            rows.len() == 1
                && rows[0].delivered_at.is_some()
                && mailbox
                    .wake_session_reader()
                    .wake_claim(SESSION)
                    .unwrap()
                    .is_none()
        }),
        "native detached provider turn did not settle\nmailbox={:?}\nruntime={:?}\nclaim={:?}\ninvocations={:?}\nmarker={}",
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
            .unwrap(),
        invocation_diagnostics(&fixture.state_path()),
        fs::read_to_string(&fixture.marker).unwrap_or_default()
    );

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
            "SELECT COUNT(*) FROM invocations
             WHERE session_capture_method = 'provider_session_capture'
               AND resume_input_id = ?1",
            [SESSION],
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

#[test]
fn startup_recovery_settles_a_persisted_post_anchor_turn_without_relaunching() {
    let fixture = Fixture::new();
    fixture.seed_active_chain();
    fixture.seed_pending_delivery("age309-recovery-owner");
    fixture.seed_crashed_delivery_observation("persisted mailbox delivery");

    let sweep = fixture.start_startup_sweep().wait_with_output().unwrap();
    assert_success(&sweep);
    assert!(
        wait_until(|| {
            let mailbox = MailboxDb::open(&fixture.sidecar_path()).unwrap();
            mailbox
                .list_mailbox(SESSION, true)
                .unwrap()
                .first()
                .is_some_and(|row| row.delivered_at.is_some())
                && mailbox
                    .wake_session_reader()
                    .wake_claim(SESSION)
                    .unwrap()
                    .is_none()
        }),
        "persisted post-anchor delivery was not recovered"
    );

    let mailbox = MailboxDb::open(&fixture.sidecar_path()).unwrap();
    assert_eq!(
        mailbox
            .delivery_observation_confirmation(&"b".repeat(64))
            .unwrap()
            .as_deref(),
        Some("age309-observed-user-1")
    );
    assert!(
        mailbox
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fs::read_to_string(&fixture.marker).unwrap().lines().count(),
        1
    );
    assert!(
        !fixture.provider_gate.exists(),
        "recovery must settle before a provider launch waits on the fixture gate"
    );
}

fn latest_invocation_uuid(state_path: &Path) -> String {
    Connection::open(state_path)
        .unwrap()
        .query_row(
            "SELECT invocation_uuid FROM invocations ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn invocation_diagnostics(state_path: &Path) -> Vec<(String, Option<String>, Option<String>)> {
    let connection = Connection::open(state_path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT status, error_category, terminal_reason
             FROM invocations
             ORDER BY created_at, id",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
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

def output_completion(request, seq, stdout):
    event(request, seq, "marker", name="oulipoly.launch_output_complete/v1", value={
        "protocol": "oulipoly.launch_output/v1",
        "stdout": {"bytes": len(stdout), "sha256": hashlib.sha256(stdout).hexdigest()},
        "stderr": {"bytes": 0, "sha256": hashlib.sha256(b"").hexdigest()},
        "data_event_count": 1,
    })

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
        stdout = b"native resumed\n"
        event(request, seq, "stdout", data_base64=base64.b64encode(stdout).decode("ascii"))
        seq += 1
        acceptance = params.get("prompt_acceptance", {})
        acceptance_marker = {
            "protocol": "oulipoly.prompt_acceptance/v1",
            "provider_session_id": known,
            "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
            "source": "age309.native.fixture",
            "message_id": "native-accepted",
        }
        if acceptance.get("delivery_nonce"):
            acceptance_marker["delivery_nonce"] = acceptance["delivery_nonce"]
        event(request, seq, "marker", name="oulipoly.prompt_accepted/v1", value=acceptance_marker)
        seq += 1
        event(request, seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1
    else:
        known = SESSION
        event(request, seq, "marker", name="oulipoly.provider_session", value={"provider_session_id": known})
        seq += 1
        stdout = b"native initial\n"
        event(request, seq, "stdout", data_base64=base64.b64encode(stdout).decode("ascii"))
        seq += 1
    output_completion(request, seq, stdout)
    seq += 1
    event(request, seq, "exit", status={"kind": "exited", "code": 0}, terminal_signal={"kind": "clean_exit", "evidence": "native fixture clean exit", "observed_at_unix_ms": 1000 + seq}, session={"provider_session_id": known, "state": {"cursor": "native"}})

def session_turn_page(request):
    params = request.get("params", {})
    marker = pathlib.Path(os.environ["AGE309_NATIVE_WAKE_MARKER"])
    prompts = []
    if marker.exists():
        prompts = [json.loads(line) for line in marker.read_text().splitlines() if line]
    projection = params.get("turn_projection")
    if params.get("start_mode") == "tail":
        selected = []
    elif projection == "user_observation":
        token = params.get("after_token") or "age309-anchor:0"
        selected = prompts[int(token.rsplit(":", 1)[1]):]
    else:
        selected = []
    turns = []
    for offset, prompt in enumerate(selected[:params.get("max_turns", 1)]):
        normalized = prompt.replace("\r\n", "\n").replace("\r", "\n").strip()
        turns.append({
            "session_id": SESSION,
            "turn_id": "age309-observed-user-" + str(offset + 1),
            "snapshot_sequence": offset,
            "timestamp": "2026-08-30T12:00:00Z",
            "role": "user",
            "parent_turn_id": None,
            "is_sidechain": False,
            "is_compaction_boundary": False,
            "body_state": "omitted_oversize",
            "body": None,
            "body_bytes": len(normalized.encode("utf-8")),
            "body_sha256": None,
            "canonical_text_sha256": hashlib.sha256(normalized.encode("utf-8")).hexdigest(),
        })
    count = len(prompts)
    return envelope(request, {
        "read_protocol": "oulipoly.session_turn_pages/v1",
        "provider_instance_id": request.get("provider_instance_id"),
        "settings_id": params.get("settings_id"),
        "session_id": SESSION,
        "turn_projection": projection,
        "snapshot_id": "age309-observation:" + str(count),
        "page_index": 0,
        "page_start_sequence": 0,
        "turns": turns,
        "page_turn_count": len(turns),
        "source_bytes_examined": sum(len(json.dumps(turn)) for turn in turns),
        "scan_progress": False,
        "snapshot_complete": True,
        "next_page_token": None,
        "resume_token": "age309-anchor:" + str(count),
        "source_final": False,
        "warnings": [],
    })

request = json.loads(sys.stdin.read() or "{}")
method = sys.argv[1] if len(sys.argv) > 1 else ""
if method == "describe":
    print(json.dumps(envelope(request, {
        "provider_id": "age309-native-wake-fixture",
        "display_name": "AGE-309 Native Wake Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {"launch": True, "launch_output_v1": True, "policy": True, "quota": False, "session": True, "session_turn_pages_v1": True, "terminal": False, "rotation": False, "discovery": False, "settings": False, "setup_brain": False, "setup": False, "migration": False, "prompt_acceptance_v1": True},
    })))
elif method == "policy.evaluate":
    print(json.dumps(envelope(request, {"accepted": True, "env": {}, "stdin": None, "prompt": None, "diagnostics": [], "markers": []})))
elif method == "launch":
    launch(request)
elif method == "session.capture":
    print(json.dumps(envelope(request, {"provider_session_id": SESSION, "state": {"captured": True}, "artifacts": []})))
elif method == "session.read_turns":
    print(json.dumps(session_turn_page(request)))
else:
    print(json.dumps({"contract": CONTRACT, "request_id": request.get("request_id", "missing"), "ok": False, "error": {"category": "failed", "code": "unsupported_subcommand", "message": method, "retryable": False}}))
"#
}
