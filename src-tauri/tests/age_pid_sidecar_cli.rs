#![cfg(target_os = "linux")]

use oulipoly_state::pid_identity::{PidIdentityDb, PidIdentityRecord, ProcessIdentity};
use oulipoly_state::{InvocationStart, StateDb};
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

const ROOT_UUID: &str = "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb";
const CHILD_UUID: &str = "cccccccc-cccc-4ccc-cccc-cccccccccccc";
const ROOT_SESSION: &str = "session-root";
const CHILD_SESSION: &str = "session-child";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            config_home: dir.path().join("config"),
            data_home: dir.path().join("data"),
            _dir: dir,
        }
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn seed_root_invocation(&self) -> i64 {
        let db = StateDb::open(&self.state_path()).unwrap();
        let id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: ROOT_UUID.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.update_session_capture(id, Some(ROOT_SESSION), "fixture")
            .unwrap();
        id
    }

    fn seed_root_and_child_invocations(&self) {
        let db = StateDb::open(&self.state_path()).unwrap();
        let root_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: ROOT_UUID.to_string(),
                model_name: "fixture-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.update_session_capture(root_id, Some(ROOT_SESSION), "fixture")
            .unwrap();
        let child_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: CHILD_UUID.to_string(),
                model_name: "fixture-child".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: Some(root_id),
            })
            .unwrap();
        db.update_session_capture(child_id, Some(CHILD_SESSION), "fixture")
            .unwrap();
    }

    fn record_sidecar_identity(
        &self,
        identity: &ProcessIdentity,
        invocation_uuid: &str,
        session_id: Option<&str>,
    ) {
        let sidecar = PidIdentityDb::open(&self.sidecar_path()).unwrap();
        sidecar
            .record_identity(PidIdentityRecord {
                identity,
                os_pgid: None,
                invocation_uuid,
                session_id,
                provider_name: Some("fixture-provider"),
                model_name: Some("fixture-model"),
                recorded_at: "2026-06-04T12:00:00Z",
            })
            .unwrap();
    }

    fn run_session(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("session");
        cmd.args(args);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.output().unwrap()
    }
}

#[test]
fn of_pid_resolves_session_from_state_when_sidecar_session_is_null() {
    let fixture = Fixture::new();
    fixture.seed_root_invocation();
    let pid = std::process::id();
    let identity = current_process_identity();
    fixture.record_sidecar_identity(&identity, ROOT_UUID, None);

    let output = fixture.run_session(&["of-pid", &pid.to_string(), "--json"]);

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["found"], true);
    assert_eq!(json["pid"], pid);
    assert_eq!(json["invocation_uuid"], ROOT_UUID);
    assert_eq!(json["session_id"], ROOT_SESSION);
    assert_eq!(json["provider_name"], "fixture-provider");
    assert_eq!(json["model_name"], "fixture-model");
}

#[test]
fn of_pid_rejects_recycled_pid_starttime() {
    let fixture = Fixture::new();
    fixture.seed_root_invocation();
    let pid = std::process::id();
    let mut identity = current_process_identity();
    identity.os_pid_starttime_ticks += 1;
    fixture.record_sidecar_identity(&identity, ROOT_UUID, Some(ROOT_SESSION));

    let output = fixture.run_session(&["of-pid", &pid.to_string(), "--json"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["found"], false);
    assert_eq!(json["pid"], pid);
}

#[test]
fn of_pid_rejects_cross_boot_identity() {
    let fixture = Fixture::new();
    fixture.seed_root_invocation();
    let pid = std::process::id();
    let mut identity = current_process_identity();
    identity.os_boot_id = format!("wrong-{}", identity.os_boot_id);
    fixture.record_sidecar_identity(&identity, ROOT_UUID, Some(ROOT_SESSION));

    let output = fixture.run_session(&["of-pid", &pid.to_string(), "--json"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["found"], false);
}

#[test]
fn of_pid_missing_identity_returns_not_found() {
    let fixture = Fixture::new();
    fixture.seed_root_invocation();
    let pid = std::process::id();

    let output = fixture.run_session(&["of-pid", &pid.to_string(), "--json"]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["found"], false);
}

#[test]
fn alive_reports_true_and_false_matrix() {
    let true_fixture = Fixture::new();
    true_fixture.seed_root_invocation();
    let pid = std::process::id();
    let identity = current_process_identity();
    true_fixture.record_sidecar_identity(&identity, ROOT_UUID, Some(ROOT_SESSION));

    let true_output = true_fixture.run_session(&["alive", &pid.to_string(), "--json"]);

    assert!(true_output.status.success(), "{true_output:?}");
    let true_json = stdout_json(&true_output);
    assert_eq!(true_json["alive"], true);
    assert_eq!(true_json["invocation_uuid"], ROOT_UUID);
    assert_eq!(true_json["session_id"], ROOT_SESSION);

    let recycled_fixture = Fixture::new();
    recycled_fixture.seed_root_invocation();
    let mut recycled_identity = current_process_identity();
    recycled_identity.os_pid_starttime_ticks += 1;
    recycled_fixture.record_sidecar_identity(&recycled_identity, ROOT_UUID, Some(ROOT_SESSION));

    let false_output = recycled_fixture.run_session(&["alive", &pid.to_string(), "--json"]);

    assert_eq!(false_output.status.code(), Some(1), "{false_output:?}");
    let false_json = stdout_json(&false_output);
    assert_eq!(false_json["alive"], false);

    let missing_fixture = Fixture::new();
    missing_fixture.seed_root_invocation();
    let missing_output = missing_fixture.run_session(&["alive", &pid.to_string(), "--json"]);

    assert_eq!(missing_output.status.code(), Some(1), "{missing_output:?}");
    let missing_json = stdout_json(&missing_output);
    assert_eq!(missing_json["alive"], false);
}

#[test]
fn subtree_walks_parent_invocation_tree_and_annotates_pid_liveness() {
    let fixture = Fixture::new();
    fixture.seed_root_and_child_invocations();
    let pid = std::process::id();
    let root_identity = current_process_identity();
    fixture.record_sidecar_identity(&root_identity, ROOT_UUID, None);
    fixture.record_sidecar_identity(
        &ProcessIdentity {
            os_pid: 999_999_999,
            os_boot_id: "dead-boot".to_string(),
            os_pid_starttime_ticks: 1,
        },
        CHILD_UUID,
        None,
    );

    let output = fixture.run_session(&["subtree", &pid.to_string(), "--json", "--max-depth", "64"]);

    assert!(output.status.success(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["found"], true);
    assert_eq!(json["root_invocation_uuid"], ROOT_UUID);
    assert_eq!(json["root"]["invocation_uuid"], ROOT_UUID);
    assert_eq!(json["root"]["session_id"], ROOT_SESSION);
    assert_eq!(json["root"]["pid"], pid);
    assert_eq!(json["root"]["alive"], true);
    assert_eq!(json["root"]["children"].as_array().unwrap().len(), 1);
    let child = &json["root"]["children"][0];
    assert_eq!(child["invocation_uuid"], CHILD_UUID);
    assert_eq!(child["parent_invocation_uuid"], ROOT_UUID);
    assert_eq!(child["session_id"], CHILD_SESSION);
    assert_eq!(child["pid"], 999_999_999);
    assert_eq!(child["alive"], false);
}

fn current_process_identity() -> ProcessIdentity {
    oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
        .unwrap()
        .unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "failed to parse stdout as JSON: {err}\nstdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
