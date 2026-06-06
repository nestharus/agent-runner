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
        let mut cmd = Command::new(runner_bin());
        cmd.arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(prompt);
        self.run(cmd)
    }

    fn write_external_provider(&self) {
        let provider_path = self.write_external_provider_script();
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
    }

    fn write_external_provider_script(&self) -> PathBuf {
        let path = self.dir.path().join("external-provider.py");
        fs::write(&path, external_provider_script()).unwrap();
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
    assert_eq!(
        fixture
            .mailbox()
            .session_runtime(SESSION)
            .unwrap()
            .unwrap()
            .run_state,
        "idle"
    );
    fixture.assert_xdg_isolated();
}

fn external_provider_script() -> &'static str {
    r#"#!/usr/bin/env python3
import base64
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

def exit_event(request, seq, session_id):
    return {
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
        "session": {
            "provider_session_id": session_id,
            "state": {"cursor": "after-launch"},
        },
    }

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
        target = pathlib.Path(os.environ["S11_WORK_DIR"]) / "external-wake-resumed.txt"
        target.write_text(prompt, encoding="utf-8")
        emit(stdout_event(request, 1, "resumed\n"))
        emit(exit_event(request, 2, known))
        return
    spawn_notify_workload()
    emit(stdout_event(request, 1, "initial\n"))
    emit(exit_event(request, 2, SESSION))

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
        ], stdout=out, stderr=err, check=False, env=os.environ.copy())

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

fn assert_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
