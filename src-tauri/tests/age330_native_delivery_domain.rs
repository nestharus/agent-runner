//! Native cross-platform evidence for external-provider result delivery.
//!
//! Declared roles: fixture, orchestration, validator.

mod provider_authority_fixture;

use oulipoly_state::StateDb;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MODEL: &str = "age330-native-delivery-model";
const PROVIDER: &str = "age330-native-delivery-provider";
const PROVIDER_INSTANCE_ID: &str = "age330-native-delivery-fixture-instance";
const SESSION_ID: &str = "ses_age330_native_delivery";
const CHAIN_ID: &str = "chain-age330-native-delivery";
const RESULT_PREFIX: &[u8] = b"OULIPOLY_RESULT=";

#[derive(Clone, Copy, Debug)]
enum Carrier {
    Fresh,
    Resume,
}

impl Carrier {
    fn stdout(self) -> &'static [u8] {
        match self {
            Self::Fresh => b"fresh-stdout:\x00\xff-tail",
            Self::Resume => b"resume-stdout:\x00\xff-tail",
        }
    }

    fn stderr(self) -> &'static [u8] {
        match self {
            Self::Fresh => b"fresh-provider-stderr",
            Self::Resume => b"resume-provider-stderr",
        }
    }
}

struct Fixture {
    root: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    models_dir: PathBuf,
    provider_script: PathBuf,
    python_executable: PathBuf,
    provider_started: PathBuf,
    provider_release: PathBuf,
}

impl Fixture {
    fn new(carrier: Carrier) -> Self {
        let root = tempfile::tempdir().unwrap();
        let config_home = root.path().join("config");
        let data_home = root.path().join("data");
        let home_dir = root.path().join("home");
        let app_config = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();

        let script = root.path().join("native-delivery-provider.py");
        fs::write(&script, provider_script()).unwrap();
        let launcher = compile_provider_launcher(root.path());
        fs::write(
            models_dir.join(format!("{MODEL}.toml")),
            format!("prompt_mode = \"arg\"\n\n[[providers]]\nname = \"{PROVIDER}\"\nargs = []\n"),
        )
        .unwrap();
        fs::write(
            app_config.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority_at(
                &format!(
                    "[{PROVIDER}]\ncommand = \"age330-native-fixture\"\nargs = []\nprompt_mode = \"arg\"\nsettings_id = \"{PROVIDER}\"\n"
                ),
                "age330-native-delivery",
                &launcher,
            ),
        )
        .unwrap();

        let fixture = Self {
            provider_started: root.path().join("provider-started"),
            provider_release: root.path().join("provider-release"),
            root,
            config_home,
            data_home,
            home_dir,
            models_dir,
            provider_script: script,
            python_executable: python_executable(),
        };
        if matches!(carrier, Carrier::Resume) {
            fixture.seed_resume();
        }
        fixture
    }

    fn command(&self, carrier: Carrier) -> Command {
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
            .env("AGE330_NATIVE_GATE_MODE", "none")
            .env("AGE330_NATIVE_PYTHON", &self.python_executable)
            .env("AGE330_NATIVE_PROVIDER_SCRIPT", &self.provider_script)
            .env("AGE330_NATIVE_PROVIDER_STARTED", &self.provider_started)
            .env("AGE330_NATIVE_PROVIDER_RELEASE", &self.provider_release)
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .current_dir(self.root.path());
        match carrier {
            Carrier::Fresh => {
                command
                    .arg("--models-dir")
                    .arg(&self.models_dir)
                    .arg("--model")
                    .arg(MODEL)
                    .arg("fresh native delivery");
            }
            Carrier::Resume => {
                command
                    .arg("-m")
                    .arg(MODEL)
                    .arg("--resume")
                    .arg(SESSION_ID)
                    .arg("--models-dir")
                    .arg(&self.models_dir)
                    .arg("resume native delivery");
            }
        }
        command
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn seed_resume(&self) {
        drop(StateDb::open(&self.state_path()).unwrap());
        let connection = Connection::open(self.state_path()).unwrap();
        connection
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-09-03T00:00:00Z', '2026-09-03T00:00:00Z', ?2)",
                params![CHAIN_ID, MODEL],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-09-03T00:00:00Z', 'initial')",
                params![CHAIN_ID, PROVIDER, SESSION_ID],
            )
            .unwrap();
        provider_authority_fixture::bind_session_authority_with_cwd_at(
            &connection,
            PROVIDER,
            SESSION_ID,
            PROVIDER_INSTANCE_ID,
            PROVIDER,
            self.root.path(),
        );
    }

    fn latest_invocation(&self) -> InvocationRow {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT invocation_uuid, status, success, exit_code, resume_input_id
                 FROM invocations ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok(InvocationRow {
                        uuid: row.get(0)?,
                        status: row.get(1)?,
                        success: row.get(2)?,
                        exit_code: row.get(3)?,
                        resume_input_id: row.get(4)?,
                    })
                },
            )
            .unwrap()
    }

    fn latest_delivery(&self) -> DeliveryRow {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT d.invocation_uuid, d.provider_outcome_state, d.delivery_state,
                        d.delivery_failure_stage, d.delivery_failure_kind,
                        d.stdout_path, d.stderr_path
                 FROM invocation_output_deliveries d
                 ORDER BY d.invocation_id DESC LIMIT 1",
                [],
                |row| {
                    Ok(DeliveryRow {
                        invocation_uuid: row.get(0)?,
                        provider_outcome_state: row.get(1)?,
                        delivery_state: row.get(2)?,
                        failure_stage: row.get(3)?,
                        failure_kind: row.get(4)?,
                        stdout_path: row.get(5)?,
                        stderr_path: row.get(6)?,
                    })
                },
            )
            .unwrap()
    }

    fn wait_for_provider(&self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if self.provider_started.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("native provider did not reach its delivery gate");
    }

    fn release_provider(&self) {
        fs::write(&self.provider_release, b"continue\n").unwrap();
    }
}

#[derive(Debug)]
struct InvocationRow {
    uuid: String,
    status: String,
    success: i64,
    exit_code: i32,
    resume_input_id: Option<String>,
}

#[derive(Debug)]
struct DeliveryRow {
    invocation_uuid: String,
    provider_outcome_state: String,
    delivery_state: String,
    failure_stage: Option<String>,
    failure_kind: Option<String>,
    stdout_path: String,
    stderr_path: String,
}

#[test]
fn fresh_and_resume_native_capture_preserves_provider_bytes_and_final_result_authority() {
    for carrier in [Carrier::Fresh, Carrier::Resume] {
        let direct = Fixture::new(carrier);
        let output = direct.command(carrier).output().unwrap();
        assert_direct_success(&direct, carrier, &output);

        let merged = Fixture::new(carrier);
        let capture_path = merged.root.path().join("native-merged-capture.bin");
        let capture = shared_capture_file(&capture_path);
        let status = merged
            .command(carrier)
            .stdout(Stdio::from(capture.try_clone().unwrap()))
            .stderr(Stdio::from(capture.try_clone().unwrap()))
            .status()
            .unwrap();
        drop(capture);
        let bytes = fs::read(capture_path).unwrap();
        assert_merged_success(&merged, carrier, status, &bytes);
    }
}

#[test]
fn broken_native_payload_handle_fails_closed_and_persists_delivery_failure() {
    let fixture = Fixture::new(Carrier::Fresh);
    let stderr_path = fixture.root.path().join("payload-failure-stderr.bin");
    let stderr = shared_capture_file(&stderr_path);
    let mut child = fixture
        .command(Carrier::Fresh)
        .env("AGE330_NATIVE_GATE_MODE", "payload")
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr.try_clone().unwrap()))
        .spawn()
        .unwrap();
    let payload_read_handle = child.stdout.take().expect("runner stdout pipe");
    fixture.wait_for_provider();
    drop(payload_read_handle);
    fixture.release_provider();

    let status = wait_bounded(&mut child);
    drop(stderr);
    let surviving_stderr = fs::read(stderr_path).unwrap();
    assert!(!status.success(), "broken payload handle must fail runner");
    assert_failed_delivery(&fixture, Carrier::Fresh.stdout(), Carrier::Fresh.stderr());
    assert_no_authoritative_success(&fixture, &surviving_stderr);
}

#[test]
fn broken_native_control_handle_fails_closed_and_persists_delivery_failure() {
    const CONTROL_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

    let fixture = Fixture::new(Carrier::Resume);
    let mut child = fixture
        .command(Carrier::Resume)
        .env("AGE330_NATIVE_GATE_MODE", "control")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut payload_read_handle = child.stdout.take().expect("runner stdout pipe");
    let control_read_handle = child.stderr.take().expect("runner stderr pipe");
    let (payload_started_tx, payload_started_rx) = mpsc::sync_channel(0);
    let (drain_tx, drain_rx) = mpsc::sync_channel(0);
    // Stop after one byte so the 2 MiB replay remains blocked while the parent
    // closes the control pipe, then drain stdout to let delivery advance.
    let reader = std::thread::spawn(move || {
        let mut payload = vec![0];
        payload_read_handle.read_exact(&mut payload).unwrap();
        payload_started_tx.send(()).unwrap();
        drain_rx.recv().unwrap();
        payload_read_handle.read_to_end(&mut payload).unwrap();
        payload
    });
    fixture.wait_for_provider();
    fixture.release_provider();
    payload_started_rx
        .recv_timeout(Duration::from_secs(20))
        .expect("provider stdout delivery did not begin");
    drop(control_read_handle);
    drain_tx.send(()).unwrap();

    let status = wait_bounded(&mut child);
    let surviving_stdout = reader.join().unwrap();
    let expected_stdout = vec![b'C'; CONTROL_PAYLOAD_BYTES];
    assert!(!status.success(), "broken control handle must fail runner");
    assert_eq!(surviving_stdout, expected_stdout);
    assert_failed_delivery(&fixture, &expected_stdout, b"");
    assert_no_authoritative_success(&fixture, &surviving_stdout);
}

#[test]
fn delivery_state_write_failure_leaves_retained_output_failed_without_provider_redispatch() {
    for carrier in [Carrier::Fresh, Carrier::Resume] {
        let fixture = Fixture::new(carrier);
        let child = fixture
            .command(carrier)
            .env("AGE330_NATIVE_GATE_MODE", "delivery-state")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        fixture.wait_for_provider();
        Connection::open(fixture.state_path())
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_output_delivered
                 BEFORE UPDATE OF delivery_state ON invocation_output_deliveries
                 WHEN NEW.delivery_state = 'delivered'
                 BEGIN
                     SELECT RAISE(FAIL, 'injected delivered-state write failure');
                 END;",
            )
            .unwrap();
        fixture.release_provider();

        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success(), "{carrier:?}: {output:?}");
        assert_eq!(output.stdout, carrier.stdout(), "{carrier:?}");
        let marker = final_result_marker_index(&output.stderr);
        let mut provider_boundary = carrier.stderr().to_vec();
        provider_boundary.push(b'\n');
        assert!(
            output.stderr[..marker].ends_with(&provider_boundary),
            "{carrier:?}: {:?}",
            output.stderr
        );

        let invocation = fixture.latest_invocation();
        let matching = matching_results(&output.stderr, &invocation.uuid);
        assert_eq!(matching.len(), 1, "{carrier:?}: {:?}", output.stderr);
        assert_success_result(&matching[0], &invocation.uuid);

        let delivery = fixture.latest_delivery();
        assert_eq!(delivery.invocation_uuid, invocation.uuid, "{delivery:?}");
        assert_eq!(delivery.provider_outcome_state, "settled", "{delivery:?}");
        assert_eq!(delivery.delivery_state, "failed", "{delivery:?}");
        assert_eq!(
            delivery.failure_stage.as_deref(),
            Some("delivery_confirmation"),
            "{delivery:?}"
        );
        assert_eq!(
            delivery.failure_kind.as_deref(),
            Some("unconfirmed"),
            "{delivery:?}"
        );
        assert_eq!(fs::read(delivery.stdout_path).unwrap(), carrier.stdout());
        assert_eq!(fs::read(delivery.stderr_path).unwrap(), carrier.stderr());
        let provider_launches = fs::read_to_string(&fixture.provider_started).unwrap();
        assert_eq!(
            provider_launches.lines().collect::<Vec<_>>(),
            ["delivery-state"],
            "{carrier:?}: provider launch must not be redispatched"
        );
    }
}

fn assert_direct_success(fixture: &Fixture, carrier: Carrier, output: &Output) {
    assert!(output.status.success(), "{carrier:?}: {output:?}");
    assert_eq!(
        output.stdout,
        carrier.stdout(),
        "direct provider stdout must remain byte-exact and unterminated"
    );
    let invocation = assert_successful_invocation(fixture, carrier);
    let result = final_authoritative_result(&output.stderr, &invocation.uuid);
    assert_success_result(&result, &invocation.uuid);

    let marker = final_result_marker_index(&output.stderr);
    let mut provider_boundary = carrier.stderr().to_vec();
    provider_boundary.push(b'\n');
    assert!(
        output.stderr[..marker].ends_with(&provider_boundary),
        "direct provider stderr bytes changed or did not immediately precede the anchored result: {:?}",
        output.stderr
    );
    assert_delivered(fixture, &invocation.uuid);
}

fn assert_merged_success(fixture: &Fixture, carrier: Carrier, status: ExitStatus, capture: &[u8]) {
    assert!(status.success(), "{carrier:?}: {status:?}");
    let invocation = assert_successful_invocation(fixture, carrier);
    let result = final_authoritative_result(capture, &invocation.uuid);
    assert_success_result(&result, &invocation.uuid);

    let marker = final_result_marker_index(capture);
    let mut provider_boundary = carrier.stderr().to_vec();
    provider_boundary.extend_from_slice(carrier.stdout());
    provider_boundary.push(b'\n');
    assert!(
        capture[..marker].ends_with(&provider_boundary),
        "native merged capture must contain unchanged provider stderr then byte-exact provider stdout before the final anchored result: {capture:?}"
    );
    assert_delivered(fixture, &invocation.uuid);
}

fn assert_successful_invocation(fixture: &Fixture, carrier: Carrier) -> InvocationRow {
    let invocation = fixture.latest_invocation();
    assert_eq!(invocation.status, "succeeded", "{invocation:?}");
    assert_eq!(invocation.success, 1, "{invocation:?}");
    assert_eq!(invocation.exit_code, 0, "{invocation:?}");
    match carrier {
        Carrier::Fresh => assert_eq!(invocation.resume_input_id, None, "{invocation:?}"),
        Carrier::Resume => assert_eq!(
            invocation.resume_input_id.as_deref(),
            Some(SESSION_ID),
            "{invocation:?}"
        ),
    }
    invocation
}

fn assert_delivered(fixture: &Fixture, invocation_uuid: &str) {
    let delivery = fixture.latest_delivery();
    assert_eq!(delivery.invocation_uuid, invocation_uuid, "{delivery:?}");
    assert_eq!(delivery.provider_outcome_state, "settled", "{delivery:?}");
    assert_eq!(delivery.delivery_state, "delivered", "{delivery:?}");
    assert_eq!(delivery.failure_stage, None, "{delivery:?}");
    assert_eq!(delivery.failure_kind, None, "{delivery:?}");
}

fn assert_failed_delivery(fixture: &Fixture, expected_stdout: &[u8], expected_stderr: &[u8]) {
    let invocation = fixture.latest_invocation();
    let delivery = fixture.latest_delivery();
    assert_eq!(delivery.invocation_uuid, invocation.uuid, "{delivery:?}");
    assert_eq!(delivery.provider_outcome_state, "settled", "{delivery:?}");
    assert_eq!(delivery.delivery_state, "failed", "{delivery:?}");
    assert_eq!(
        delivery.failure_stage.as_deref(),
        Some("payload_or_control"),
        "{delivery:?}"
    );
    assert!(
        delivery
            .failure_kind
            .as_ref()
            .is_some_and(|kind| !kind.is_empty()),
        "{delivery:?}"
    );
    assert_eq!(fs::read(delivery.stdout_path).unwrap(), expected_stdout);
    assert_eq!(fs::read(delivery.stderr_path).unwrap(), expected_stderr);
}

fn assert_no_authoritative_success(fixture: &Fixture, surviving_capture: &[u8]) {
    let invocation = fixture.latest_invocation();
    assert!(
        matching_results(surviving_capture, &invocation.uuid)
            .iter()
            .all(|result| result["success"] != true),
        "delivery failure exposed an authoritative success result: {surviving_capture:?}"
    );
}

fn final_authoritative_result(capture: &[u8], invocation_uuid: &str) -> Value {
    let matching = matching_results(capture, invocation_uuid);
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one matching shape-valid result for {invocation_uuid}: {capture:?}"
    );
    let final_line = capture
        .strip_suffix(b"\n")
        .expect("result capture must end with a newline")
        .rsplit(|byte| *byte == b'\n')
        .next()
        .expect("final result line");
    assert!(
        final_line.starts_with(RESULT_PREFIX),
        "matching runner result must be the final line-anchored record: {capture:?}"
    );
    matching.into_iter().next().unwrap()
}

fn matching_results(capture: &[u8], invocation_uuid: &str) -> Vec<Value> {
    capture
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(RESULT_PREFIX))
        .filter_map(|payload| serde_json::from_slice::<Value>(payload).ok())
        .filter(|result| result["id"] == invocation_uuid && result_shape_is_valid(result))
        .collect()
}

fn result_shape_is_valid(result: &Value) -> bool {
    let Some(object) = result.as_object() else {
        return false;
    };
    object.keys().map(String::as_str).collect::<BTreeSet<_>>()
        == BTreeSet::from([
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "status",
            "success",
            "terminal_reason",
        ])
}

fn assert_success_result(result: &Value, invocation_uuid: &str) {
    assert_eq!(result["id"], invocation_uuid);
    assert_eq!(result["status"], "succeeded");
    assert_eq!(result["success"], true);
    assert_eq!(result["exit_code"], 0);
    assert!(result["error_category"].is_null());
    assert!(result["terminal_reason"].is_null());
}

fn final_result_marker_index(capture: &[u8]) -> usize {
    let marker = capture
        .windows(RESULT_PREFIX.len())
        .rposition(|window| window == RESULT_PREFIX)
        .expect("final result marker");
    assert!(
        marker == 0 || capture[marker - 1] == b'\n',
        "result marker must be line-anchored"
    );
    marker
}

fn shared_capture_file(path: &Path) -> File {
    OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}

fn wait_bounded(child: &mut Child) -> ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("runner did not exit after native delivery handle was broken");
}

fn compile_provider_launcher(root: &Path) -> PathBuf {
    let source = root.join("native-delivery-provider-launcher.rs");
    let binary = root.join(format!(
        "native-delivery-provider-launcher{}",
        std::env::consts::EXE_SUFFIX
    ));
    fs::write(
        &source,
        r#"use std::process::{Command, Stdio};

fn main() {
    let python = std::env::var_os("AGE330_NATIVE_PYTHON")
        .expect("AGE330_NATIVE_PYTHON must identify the fixture interpreter");
    let script = std::env::var_os("AGE330_NATIVE_PROVIDER_SCRIPT")
        .expect("AGE330_NATIVE_PROVIDER_SCRIPT must identify the provider fixture");
    let status = Command::new(python)
        .arg(script)
        .args(std::env::args_os().skip(1))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("launch AGE-330 Python provider fixture");
    std::process::exit(status.code().unwrap_or(1));
}
"#,
    )
    .unwrap();
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("rustc should run for AGE-330 native launcher fixture");
    assert!(
        output.status.success(),
        "AGE-330 native launcher fixture should compile: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

fn python_executable() -> PathBuf {
    for candidate in ["python3", "python"] {
        if let Ok(output) = Command::new(candidate)
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            && output.status.success()
        {
            return PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        }
    }
    panic!("native delivery fixture requires Python");
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
SESSION = "ses_age330_native_delivery"

def envelope(request, result):
    return {"contract": CONTRACT, "request_id": request["request_id"], "ok": True, "result": result}

def event(request, seq, kind, **fields):
    value = {"contract": CONTRACT, "request_id": request["request_id"], "seq": seq, "time_unix_ms": 1000 + seq, "kind": kind}
    value.update(fields)
    print(json.dumps(value, separators=(",", ":")), flush=True)

def gated_mode():
    mode = os.environ.get("AGE330_NATIVE_GATE_MODE", "none")
    if mode == "none":
        return mode
    with pathlib.Path(os.environ["AGE330_NATIVE_PROVIDER_STARTED"]).open("a") as started:
        started.write(mode + "\n")
    release = pathlib.Path(os.environ["AGE330_NATIVE_PROVIDER_RELEASE"])
    deadline = time.monotonic() + 20
    while not release.exists() and time.monotonic() < deadline:
        time.sleep(0.02)
    return mode

def launch(request):
    known = request.get("params", {}).get("session", {}).get("known_provider_session_id")
    is_resume = bool(known)
    seq = 1
    if not known:
        known = SESSION
        event(request, seq, "marker", name="oulipoly.provider_session", value={"provider_session_id": known})
        seq += 1
    mode = gated_mode()
    if mode == "control":
        stdout = b"C" * (2 * 1024 * 1024)
        stderr = b""
    elif request.get("params", {}).get("session", {}).get("known_provider_session_id"):
        stdout = bytes.fromhex("726573756d652d7374646f75743a00ff2d7461696c")
        stderr = b"resume-provider-stderr"
    else:
        stdout = bytes.fromhex("66726573682d7374646f75743a00ff2d7461696c")
        stderr = b"fresh-provider-stderr"
    stdout_events = 0
    for offset in range(0, len(stdout), 32 * 1024):
        event(request, seq, "stdout", data_base64=base64.b64encode(stdout[offset:offset + 32 * 1024]).decode("ascii"))
        seq += 1
        stdout_events += 1
    if stderr:
        event(request, seq, "stderr", data_base64=base64.b64encode(stderr).decode("ascii"))
        seq += 1
    if is_resume:
        event(request, seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1
    event(request, seq, "marker", name="oulipoly.launch_output_complete/v1", value={
        "protocol": "oulipoly.launch_output/v1",
        "stdout": {"bytes": len(stdout), "sha256": hashlib.sha256(stdout).hexdigest()},
        "stderr": {"bytes": len(stderr), "sha256": hashlib.sha256(stderr).hexdigest()},
        "data_event_count": stdout_events + int(bool(stderr)),
    })
    seq += 1
    event(request, seq, "exit", status={"kind": "exited", "code": 0}, terminal_signal={"kind": "clean_exit", "evidence": "native delivery fixture clean exit", "observed_at_unix_ms": 1000 + seq}, session={"provider_session_id": known, "state": {"cursor": "native"}})

request = json.loads(sys.stdin.read() or "{}")
method = sys.argv[1] if len(sys.argv) > 1 else ""
if method == "describe":
    print(json.dumps(envelope(request, {
        "provider_id": "age330-native-delivery-fixture",
        "display_name": "AGE-330 Native Delivery Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {"launch": True, "launch_output_v1": True, "policy": True, "quota": False, "session": True, "session_turn_pages_v1": False, "terminal": False, "rotation": False, "discovery": False, "settings": False, "setup_brain": False, "setup": False, "migration": False, "prompt_acceptance_v1": False},
    })))
elif method == "policy.evaluate":
    print(json.dumps(envelope(request, {"accepted": True, "env": {}, "stdin": None, "prompt": None, "diagnostics": [], "markers": []})))
elif method == "launch":
    launch(request)
elif method == "session.capture":
    print(json.dumps(envelope(request, {"provider_session_id": SESSION, "state": {"captured": True}, "artifacts": []})))
else:
    print(json.dumps({"contract": CONTRACT, "request_id": request.get("request_id", "missing"), "ok": False, "error": {"category": "failed", "code": "unsupported_subcommand", "message": method, "retryable": False}}))
"#
}
