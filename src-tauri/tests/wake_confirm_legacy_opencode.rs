#![cfg(unix)]

use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{AgentBashCompleteEnqueue, MailboxDb};
use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const MODEL: &str = "wake-confirm-legacy-opencode";
const PROVIDER: &str = "opencode";
const SESSION: &str = "ses_wakeconfirmlegacy";
const HANDLE: &str = "h-wake-confirm-legacy";
const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    bin_dir: PathBuf,
    work_dir: PathBuf,
    export_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("xdg-config");
        let data_home = dir.path().join("xdg-data");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let bin_dir = dir.path().join("bin");
        let work_dir = dir.path().join("work");
        let export_dir = dir.path().join("fake-opencode-export");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        fs::create_dir_all(&export_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            home_dir,
            app_config_dir,
            models_dir,
            bin_dir,
            work_dir,
            export_dir,
        }
    }

    fn app_data_dir(&self) -> PathBuf {
        self.data_home.join("oulipoly-agent-runner")
    }

    fn sidecar_path(&self) -> PathBuf {
        self.app_data_dir().join("pid-identity.db")
    }

    fn state_path(&self) -> PathBuf {
        self.app_data_dir().join("state.db")
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.home_dir)
            .env("PATH", self.path_env())
            .env("AGENT_BASH_AGENT_RUNNER_BIN", runner_bin())
            .env("WAKE_CONFIRM_WORK_DIR", &self.work_dir)
            .env("WAKE_CONFIRM_EXPORT_DIR", &self.export_dir)
            .env_remove("OULIPOLY_DATA_DIR")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .current_dir(self.dir.path());
        cmd.output().unwrap()
    }

    fn path_env(&self) -> String {
        let current = std::env::var("PATH").unwrap_or_default();
        format!("{}:{current}", self.bin_dir.display())
    }

    fn run_agent(&self, prompt: &str) -> Output {
        let mut cmd = Command::new(runner_bin());
        cmd.env("WAKE_CONFIRM_NO_NOTIFY", "1")
            .arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(prompt);
        self.run(cmd)
    }

    fn run_resume_with_env(&self, envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(runner_bin());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.arg("resume")
            .arg("--session-id")
            .arg(SESSION)
            .arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir);
        self.run(cmd)
    }

    fn write_legacy_opencode(&self) {
        self.write_executable("opencode", fake_opencode_script());
        self.write_executable("opencode-turns", fake_opencode_turns_script());
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"prompt_mode = "arg"

[[providers]]
name = "{PROVIDER}"
args = []
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
command = "opencode"
args = ["run"]
prompt_mode = "arg"

[{PROVIDER}.session_capture]
kind = "stdout_json_event"
json_args = ["--json"]
event_type = "step_start"
event_id_path = "sessionID"

[{PROVIDER}.resume]
kind = "flag"
flag = "--session"
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                r#"[{PROVIDER}]
turn_script = "opencode-turns"
state_dir = {}
"#,
                toml_string(&path_string(&self.work_dir.join("turn-state")))
            ),
        )
        .unwrap();
    }

    fn write_executable(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin_dir.join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn state(&self) -> StateDb {
        StateDb::open(&self.state_path()).unwrap()
    }

    fn ensure_active_chain_for_legacy_session(&self) {
        if self
            .state()
            .chain_id_for_segment(PROVIDER, SESSION)
            .unwrap()
            .is_some()
        {
            return;
        }
        let conn = Connection::open(self.state_path()).unwrap();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-06-07T00:00:00Z', '2026-06-07T00:00:00Z', ?2)",
            params![CHAIN_ID, MODEL],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-06-07T00:00:00Z', 'initial')",
            params![CHAIN_ID, PROVIDER, SESSION],
        )
        .unwrap();
    }

    fn seed_mailbox_notification(&self) {
        let state_dir = self.work_dir.join(HANDLE);
        fs::create_dir_all(&state_dir).unwrap();
        let meta = state_dir.join("meta.json");
        let log = state_dir.join("log");
        let rc = state_dir.join("rc");
        fs::write(&meta, r#"{"caller_chain":[]}"#).unwrap();
        fs::write(&log, "legacy opencode workload completed\n").unwrap();
        fs::write(&rc, "0\n").unwrap();
        let payload_json = serde_json::json!({
            "schema_version": 1,
            "kind": "agent_bash_complete",
            "handle": HANDLE,
            "state_dir": path_string(&state_dir),
            "meta_path": path_string(&meta),
            "log_path": path_string(&log),
            "rc_path": path_string(&rc),
            "rc": 0,
        })
        .to_string();
        let mut mailbox = self.mailbox();
        mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: SESSION,
                handle: HANDLE,
                payload_json: &payload_json,
                owner_invocation_uuid: None,
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
    }

    fn notification_prompt_path(&self) -> PathBuf {
        self.work_dir.join("legacy-wake-resumed.txt")
    }

    fn export_path(&self) -> PathBuf {
        self.export_dir.join(format!("{SESSION}.jsonl"))
    }

    fn export_text(&self) -> String {
        fs::read_to_string(self.export_path()).unwrap_or_default()
    }

    fn unconfirmed_invocation_count(&self) -> i64 {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM invocations WHERE error_category = 'mailbox_delivery_unconfirmed'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn assert_xdg_isolated(&self) {
        assert!(
            !self.home_dir.join(".config/oulipoly-agent-runner").exists(),
            "config must stay in isolated XDG_CONFIG_HOME"
        );
        assert!(
            !self
                .home_dir
                .join(".local/share/oulipoly-agent-runner")
                .exists(),
            "state must stay in isolated XDG_DATA_HOME"
        );
    }
}

#[test]
fn legacy_opencode_resume_confirms_delivery_after_targeted_turn_ingest() {
    let _guard = integration_test_guard();
    let fixture = prepared_fixture();
    fixture.seed_mailbox_notification();

    let output = fixture.run_resume_with_env(&[]);
    let prompt = wait_for_file(&fixture.notification_prompt_path());
    assert!(prompt.starts_with("[OULIPOLY NOTIFICATIONS]"), "{prompt}");
    assert!(prompt.contains("kind: agent_bash_complete"), "{prompt}");
    assert!(prompt.contains("handle: h-wake-confirm-legacy"), "{prompt}");
    assert!(prompt.contains("[OULIPOLY-DELIVERY "), "{prompt}");
    wait_until("fake opencode export contains delivery nonce", || {
        let exported = fixture.export_text();
        exported.contains("[OULIPOLY NOTIFICATIONS]") && exported.contains("[OULIPOLY-DELIVERY ")
    });

    assert_success(&output);
    wait_until("legacy opencode delivery marked delivered", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_some()
            && rows[0].delivery_error.is_none()
            && rows[0].delivery_attempts == 1
            && rows[0].delivered_by_invocation_uuid.is_some()
    });
    assert!(
        fixture
            .state()
            .has_session_user_text_turn(PROVIDER, SESSION, &prompt)
            .unwrap(),
        "session_turns should contain the exact delivered user body"
    );
    assert_eq!(fixture.unconfirmed_invocation_count(), 0);
    assert!(fixture.mailbox().list_pending(SESSION).unwrap().is_empty());
    fixture.assert_xdg_isolated();
}

#[test]
fn legacy_opencode_resume_leaves_mailbox_pending_when_export_omits_notification_turn() {
    let _guard = integration_test_guard();
    let fixture = prepared_fixture();
    fixture.seed_mailbox_notification();

    let output = fixture.run_resume_with_env(&[("WAKE_CONFIRM_OMIT_RESUME_USER_TURN", "1")]);
    assert_exit_code(&output, 1);
    let prompt = wait_for_file(&fixture.notification_prompt_path());
    assert!(prompt.contains("[OULIPOLY-DELIVERY "), "{prompt}");
    wait_until("omitted export leaves mailbox pending unconfirmed", || {
        let db = fixture.mailbox();
        let rows = db.list_mailbox(SESSION, true).unwrap();
        rows.len() == 1
            && rows[0].delivered_at.is_none()
            && rows[0].delivery_attempts == 1
            && rows[0].delivery_error.as_deref() == Some("mailbox_delivery_unconfirmed")
            && rows[0].delivered_by_invocation_uuid.is_none()
    });
    assert!(
        !fixture
            .state()
            .has_session_user_text_turn(PROVIDER, SESSION, &prompt)
            .unwrap(),
        "omitted export must not create exact user-turn confirmation evidence"
    );
    assert_eq!(fixture.unconfirmed_invocation_count(), 1);
    assert_eq!(fixture.mailbox().list_pending(SESSION).unwrap().len(), 1);
    fixture.assert_xdg_isolated();
}

fn prepared_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.write_legacy_opencode();
    let output = fixture.run_agent("launch legacy opencode parent");
    assert_success(&output);
    fixture.ensure_active_chain_for_legacy_session();
    wait_until("legacy opencode parent runtime idle", || {
        fixture
            .mailbox()
            .session_runtime(SESSION)
            .unwrap()
            .is_some_and(|runtime| runtime.run_state == "idle")
    });
    fixture
}

fn fake_opencode_script() -> &'static str {
    r#"#!/usr/bin/env python3
import json
import os
import pathlib
import sys

SESSION = "ses_wakeconfirmlegacy"

def work_dir():
    return pathlib.Path(os.environ["WAKE_CONFIRM_WORK_DIR"])

def export_dir():
    return pathlib.Path(os.environ["WAKE_CONFIRM_EXPORT_DIR"])

def session_path(session_id):
    return export_dir() / f"{session_id}.jsonl"

def next_turn_id(session_id, role):
    path = session_path(session_id)
    if not path.exists():
        return f"{role}-1"
    count = sum(1 for line in path.read_text(encoding="utf-8").splitlines() if line.strip())
    return f"{role}-{count + 1}"

def append_turn(session_id, role, text):
    export_dir().mkdir(parents=True, exist_ok=True)
    turn = {
        "session_id": session_id,
        "turn_id": next_turn_id(session_id, role),
        "timestamp": "2026-06-07T00:00:00Z",
        "role": role,
        "body": [{"type": "text", "text": text}],
    }
    with session_path(session_id).open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(turn, separators=(",", ":")) + "\n")

def parse_run(argv):
    session_id = SESSION
    prompt = ""
    index = 0
    while index < len(argv):
        arg = argv[index]
        if arg == "--session" and index + 1 < len(argv):
            session_id = argv[index + 1]
            index += 2
            continue
        if arg == "--json":
            index += 1
            continue
        prompt = arg
        index += 1
    return session_id, prompt

def run(argv):
    session_id, prompt = parse_run(argv)
    if "--session" in argv:
        (work_dir() / "legacy-wake-resumed.txt").write_text(prompt, encoding="utf-8")
        if os.environ.get("WAKE_CONFIRM_OMIT_RESUME_USER_TURN") != "1":
            append_turn(session_id, "user", prompt)
        append_turn(session_id, "assistant", "legacy wake acknowledged")
        print("legacy wake acknowledged", flush=True)
        return 0
    print(json.dumps({"type": "step_start", "sessionID": SESSION}), flush=True)
    append_turn(SESSION, "user", prompt)
    append_turn(SESSION, "assistant", "legacy parent acknowledged")
    print("legacy parent acknowledged", flush=True)
    return 0

def export_session(argv):
    session_id = argv[0] if argv else SESSION
    path = session_path(session_id)
    if path.exists():
        sys.stdout.write(path.read_text(encoding="utf-8"))
    return 0

def main():
    subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
    if subcommand == "export":
        return export_session(sys.argv[2:])
    if subcommand == "run":
        return run(sys.argv[2:])
    print(f"unsupported fake opencode subcommand: {subcommand}", file=sys.stderr)
    return 64

if __name__ == "__main__":
    raise SystemExit(main())
"#
}

fn fake_opencode_turns_script() -> &'static str {
    r#"#!/usr/bin/env python3
import os
import subprocess
import sys

session_id = os.environ.get("SESSION_ID")
if not session_id:
    raise SystemExit(0)

result = subprocess.run(
    ["opencode", "export", session_id],
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    check=False,
)
if result.returncode != 0:
    sys.stderr.write(result.stderr)
    raise SystemExit(result.returncode)
sys.stdout.write(result.stdout)
"#
}

fn assert_success(output: &Output) {
    assert_exit_code(output, 0);
}

fn assert_exit_code(output: &Output, expected: i32) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for_file(path: &Path) -> String {
    wait_for_path_exists(path);
    fs::read_to_string(path).unwrap()
}

fn wait_for_path_exists(path: &Path) {
    wait_until(&format!("{} exists", path.display()), || path.exists());
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

fn integration_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn integration_test_guard() -> MutexGuard<'static, ()> {
    integration_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
