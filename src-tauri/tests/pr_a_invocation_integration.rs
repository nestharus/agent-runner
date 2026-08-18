#![cfg(unix)]

use oulipoly_state::{
    CompositeInvocationId, InvocationStart, InvocationStatus, ProviderSessionBinding, StateDb,
};
use rusqlite::params;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

// These tests start with an empty invocation table. When a child row is seeded
// before the CLI run, the supervised parent row created by that run receives id 2.
const FIRST_PARENT_ROW_ID_IN_FRESH_FIXTURE: i64 = 2;
const AGENT_BASH_BIN_ENV: &str = "AGENT_BASH_BIN";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    env_dump_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_script_body(
            r#"SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
printf '%s' "${OULIPOLY_PARENT_INVOCATION-}" > "$SCRIPT_DIR/env_dump.txt"
printf '{}'
"#,
        )
    }

    fn with_script_body(script_body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();

        let script_path = dir.path().join("fixture-provider.sh");
        let env_dump_path = dir.path().join("env_dump.txt");
        fs::write(
            &script_path,
            format!("#!/usr/bin/env bash\n{script_body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        fs::write(
            models_dir.join("fixture.toml"),
            r#"[[providers]]
name = "fixture-provider"
"#,
        )
        .unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            format!(
                r#"[fixture-provider]
command = "{}"
args = []
prompt_mode = "arg"

[fixture-provider.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"
"#,
                script_path.display()
            ),
        )
        .unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
            models_dir,
            env_dump_path,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn state_home(&self) -> PathBuf {
        self._dir.path().join("state")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    fn run(&self, parent_env: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg("fixture")
            .arg("ping");
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        if let Some(value) = parent_env {
            cmd.env("OULIPOLY_PARENT_INVOCATION", value);
        }
        cmd.output().unwrap()
    }
}

fn run_agent_bash_nested_child(
    fixture: &Fixture,
    parent_env: &str,
    owner_session_id: &str,
    owner_invocation_uuid: &str,
    registration_authority: &str,
    agent_bash_bin: &Path,
) -> String {
    let command = nested_child_command(fixture);
    let mut run = Command::new(agent_bash_bin);
    run.arg("run").arg("--").arg("bash").arg("-lc").arg(command);
    configure_agent_bash_env(
        &mut run,
        fixture,
        parent_env,
        owner_session_id,
        owner_invocation_uuid,
        registration_authority,
    );
    let output = run.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let dispatch: Value = serde_json::from_slice(&output.stdout).unwrap();
    let handle = dispatch["handle"].as_str().expect("agent-bash handle");
    wait_for_agent_bash_done(fixture, agent_bash_bin, handle)
}

fn nested_child_command(fixture: &Fixture) -> String {
    format!(
        "{} --models-dir {} --model fixture ping",
        shell_quote(env!("CARGO_BIN_EXE_oulipoly-agent-runner")),
        shell_quote(&fixture.models_dir.to_string_lossy())
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn configure_agent_bash_env(
    command: &mut Command,
    fixture: &Fixture,
    parent_env: &str,
    owner_session_id: &str,
    owner_invocation_uuid: &str,
    registration_authority: &str,
) {
    command.env("XDG_CONFIG_HOME", &fixture.config_home);
    command.env("XDG_DATA_HOME", &fixture.data_home);
    command.env("XDG_STATE_HOME", fixture.state_home());
    command.env("OULIPOLY_PARENT_INVOCATION", parent_env);
    command.env("AGENT_BASH_OWNER_SESSION_ID", owner_session_id);
    command.env("AGENT_BASH_OWNER_INVOCATION_UUID", owner_invocation_uuid);
    assert_eq!(registration_authority.len(), 64, "registration authority");
    command.env(
        oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
        registration_authority,
    );
    command.env(
        "AGENT_BASH_AGENT_RUNNER_BIN",
        env!("CARGO_BIN_EXE_oulipoly-agent-runner"),
    );
    command.env_remove("OULIPOLY_DATA_DIR");
}

fn wait_for_agent_bash_done(fixture: &Fixture, agent_bash_bin: &Path, handle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = String::new();
    while Instant::now() < deadline {
        last = agent_bash_status(fixture, agent_bash_bin, handle);
        if last.starts_with("DONE") {
            return last;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("agent-bash job did not finish: {last}");
}

fn agent_bash_status(fixture: &Fixture, agent_bash_bin: &Path, handle: &str) -> String {
    let mut status = Command::new(agent_bash_bin);
    status.arg("status").arg("--full").arg(handle);
    status.env("XDG_STATE_HOME", fixture.state_home());
    let output = status.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn agent_bash_bin_from_env() -> PathBuf {
    let value = std::env::var_os(AGENT_BASH_BIN_ENV)
        .map(PathBuf::from)
        .or_else(find_agent_bash_in_path)
        .unwrap_or_else(|| {
            panic!("{AGENT_BASH_BIN_ENV} must point to an agent-bash binary or agent-bash must be on PATH")
        });
    assert_agent_bash_bin(&value);
    value
}

fn find_agent_bash_in_path() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("agent-bash"))
        .find(|path| path.is_file())
}

fn assert_agent_bash_bin(path: &Path) {
    assert!(
        path.is_file(),
        "{AGENT_BASH_BIN_ENV} must point to an agent-bash binary"
    );
}

fn parse_invocation(stderr: &str) -> CompositeInvocationId {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_INVOCATION="))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "stderr should contain exactly one ID line: {stderr}"
    );
    let raw = lines[0].strip_prefix("OULIPOLY_INVOCATION=").unwrap();
    CompositeInvocationId::parse_env_value(raw).unwrap()
}

fn parse_valid_invocations(stderr: &str) -> Vec<CompositeInvocationId> {
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .filter_map(|raw| CompositeInvocationId::parse_env_value(raw).ok())
        .collect()
}

fn run_trace_json(fixture: &Fixture, invocation_uuid: &str) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    cmd.arg("trace").arg(invocation_uuid).arg("--json");
    cmd.env("XDG_CONFIG_HOME", &fixture.config_home);
    cmd.env("XDG_DATA_HOME", &fixture.data_home);
    cmd.env_remove("OULIPOLY_DATA_DIR");
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn seed_running_child_for_first_parent(fixture: &Fixture, child_uuid: &str) -> i64 {
    drop(fixture.open_db());
    let conn = rusqlite::Connection::open(fixture.db_path()).unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute(
        "INSERT INTO invocations (
            invocation_uuid, model_name, provider_name, provider_index,
            parent_invocation_id, status, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            child_uuid,
            "fixture-child-model",
            "fixture-child",
            0,
            FIRST_PARENT_ROW_ID_IN_FRESH_FIXTURE,
            "running",
            "2026-04-17T08:00:01Z"
        ],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn invocation_count(fixture: &Fixture) -> i64 {
    fixture
        .open_db()
        .connection()
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn emits_single_invocation_line_and_finalizes_succeeded_row() {
    let fixture = Fixture::new();
    let output = fixture.run(None);
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation = parse_invocation(&stderr);
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation.id).unwrap().unwrap();

    assert_eq!(row.provider_name.as_deref(), Some("fixture-provider"));
    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert!(row.finished_at.is_some());
    assert_eq!(
        fs::read_to_string(&fixture.env_dump_path).unwrap(),
        serde_json::to_string(&invocation).unwrap()
    );
}

#[test]
fn direct_run_preserves_existing_history_with_invocation_count_sentinel() {
    let fixture = Fixture::new();
    seed_running_child_for_first_parent(&fixture, "aaaaaaaa-0000-4000-8000-000000000001");
    let before = invocation_count(&fixture);

    let output = fixture.run(None);

    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation = parse_invocation(&stderr);
    assert_eq!(invocation_count(&fixture), before + 1);
    assert!(
        fixture
            .open_db()
            .get_invocation_by_uuid("aaaaaaaa-0000-4000-8000-000000000001")
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .open_db()
            .get_invocation_by_uuid(&invocation.id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn resolves_parent_env_and_overwrites_child_subprocess_env() {
    let fixture = Fixture::new();

    let parent_output = fixture.run(None);
    assert!(parent_output.status.success());
    let parent = parse_invocation(&String::from_utf8_lossy(&parent_output.stderr));
    let parent_row = fixture
        .open_db()
        .get_invocation_by_uuid(&parent.id)
        .unwrap()
        .unwrap();

    let parent_env = serde_json::to_string(&parent).unwrap();
    let child_output = fixture.run(Some(&parent_env));
    assert!(child_output.status.success());

    let child = parse_invocation(&String::from_utf8_lossy(&child_output.stderr));
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(&child.id)
        .unwrap()
        .unwrap();

    assert_eq!(child_row.parent_invocation_id, Some(parent_row.id));
    assert_eq!(
        fs::read_to_string(&fixture.env_dump_path).unwrap(),
        serde_json::to_string(&child).unwrap()
    );
}

#[test]
fn nested_agent_bash_chain_records_parent_id_from_inherited_env() {
    let agent_bash_bin = agent_bash_bin_from_env();
    let fixture = Fixture::new();
    let parent = CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: "aaaaaaaa-0000-4000-8000-000000000010".to_string(),
    };
    let owner_session_id = "fixture-parent-session";
    let state = fixture.open_db();
    let started = state
        .start_invocation_with_completion_registration_authority(&InvocationStart {
            invocation_uuid: parent.id.clone(),
            model_name: "fixture".to_string(),
            provider_name: parent.source.clone(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .bind_invocation_provider_session_start(
            started.invocation_row_id,
            &ProviderSessionBinding {
                provider_session_id: owner_session_id.to_string(),
                capture_method: "fixture_running_parent",
                resume_input_id: None,
                provider_session_resolved_account: Some(parent.source.clone()),
            },
        )
        .unwrap();
    let registration_authority = started
        .completion_registration_authority
        .process_environment_value()
        .to_string();
    let parent_row_id = started.invocation_row_id;
    drop(state);
    let parent_env = serde_json::to_string(&parent).unwrap();

    let status = run_agent_bash_nested_child(
        &fixture,
        &parent_env,
        owner_session_id,
        &parent.id,
        &registration_authority,
        &agent_bash_bin,
    );

    assert!(status.starts_with("DONE rc=0"), "{status}");
    let child = parse_valid_invocations(&status)
        .into_iter()
        .find(|invocation| invocation.id != parent.id)
        .expect("nested child invocation marker should be captured");
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(&child.id)
        .unwrap()
        .unwrap();
    assert_eq!(child_row.parent_invocation_id, Some(parent_row_id));
}

#[test]
fn ignores_malformed_and_unresolved_parent_env_values() {
    let fixture = Fixture::new();

    for raw in [
        // Malformed JSON entirely.
        "not-json".to_string(),
        // Well-formed JSON with a valid UUID that doesn't exist in the DB.
        r#"{"source":"fixture-provider","id":"00000000-0000-0000-0000-000000000000"}"#.to_string(),
        // Well-formed JSON shape but the id field is not a valid UUID.
        // The contract says invalid UUIDs are silently treated as root —
        // the binary must NOT panic and must record a root invocation.
        r#"{"source":"fixture-provider","id":"not-a-uuid"}"#.to_string(),
    ] {
        let output = fixture.run(Some(&raw));
        assert!(output.status.success());

        let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
        let row = fixture
            .open_db()
            .get_invocation_by_uuid(&invocation.id)
            .unwrap()
            .unwrap();
        assert_eq!(row.parent_invocation_id, None, "{raw}");
    }
}

// RISK: common nonzero one-shot exits could lack terminal_reason while preserving exit_code behavior (proposal §test-intent "direct nonzero terminal-reason test", assumption A5)
// LEVEL: particular-integration
// SOURCE: contracts/nes-250-contract.md § Test catalog § Finalize cascade (T-FINAL-ONESHOT-EXIT7)
#[test]
fn direct_provider_nonzero_exit_finalizes_failed_row_with_child_exit_code() {
    // CHARACTERIZATION: T-FINAL-ONESHOT-EXIT7 persists exit_code=7 and terminal_reason=exit_nonzero.
    let fixture = Fixture::with_script_body("exit 7");

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category, None);
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_nonzero"));
    assert!(row.finished_at.is_some());
}

// risk: exhaustive surfaces 2, 30-34, 37, 57-59; level: particular-integration; source: contract § 5.3, contract § 5.7, A1, A3, A7, A10
#[test]
fn direct_provider_raw_signal_finalizes_failed_row_with_unified_signal_exit_code() {
    let fixture = Fixture::with_script_body(
        r#"kill -TERM "$$"
sleep 1"#,
    );

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(143), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(143));
    assert_eq!(row.error_category, None);
    assert_eq!(row.terminal_reason.as_deref(), Some("signal:SIGTERM"));
    assert!(row.finished_at.is_some());

    let trace = run_trace_json(&fixture, &invocation.id);
    assert_eq!(trace["root"]["invocation"]["exit_code"], 143);
    assert_eq!(
        trace["root"]["invocation"]["terminal_reason"],
        "signal:SIGTERM"
    );
    assert_eq!(trace["root"]["invocation"]["success"], false);
}

#[test]
fn direct_provider_spawn_error_finalizes_failed_row_with_spawn_error_reason() {
    let fixture = Fixture::new();
    let missing_command = fixture
        .config_home
        .join("definitely-missing-fixture-provider");
    fs::write(
        fixture
            .config_home
            .join("oulipoly-agent-runner")
            .join("providers.toml"),
        format!(
            r#"[fixture-provider]
command = "{}"
args = []
prompt_mode = "arg"
"#,
            missing_command.display()
        ),
    )
    .unwrap();

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(-1));
    assert_eq!(row.error_category.as_deref(), Some("spawn_error"));
    assert_eq!(row.terminal_reason.as_deref(), Some("spawn_error"));
    assert!(row.finished_at.is_some());
}

// RISK: supervised direct child can emit a valid marker then exit before finalizing its row, leaving durable running state (proposal §test-intent "supervised-child fence test", assumptions A1/A2/A4)
// LEVEL: particular-integration
// SOURCE: contracts/nes-250-contract.md § Test catalog § Process-supervision fence (T-FENCE-SUPERVISOR-CLEAN-CHILD)
#[test]
fn t_fence_supervisor_clean_child_finalizes_captured_running_child_row() {
    let child_uuid = "33333333-3333-3333-3333-333333333333";
    let child_marker = CompositeInvocationId {
        source: "fixture-child".to_string(),
        id: child_uuid.to_string(),
    }
    .stderr_line();
    let fixture = Fixture::with_script_body(&format!(
        r#"printf '%s\n' "{child_marker}" >&2
exit 7"#
    ));
    seed_running_child_for_first_parent(&fixture, child_uuid);

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        parse_valid_invocations(&stderr)
            .iter()
            .any(|invocation| invocation.id == child_uuid),
        "child marker should be present in captured stderr: {stderr}"
    );
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(child_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(child_row.status, InvocationStatus::Failed);
    assert_eq!(child_row.success, Some(false));
    assert_eq!(child_row.error_category, None);
    assert_eq!(
        child_row.terminal_reason.as_deref(),
        Some("supervisor_observed_exit_nonzero")
    );
    assert!(child_row.finished_at.is_some());
}

// RISK: supervisor fallback could overwrite a child row that won the finalization race (proposal §test-intent "supervised-child fence test", assumption A4)
// LEVEL: particular-integration
// SOURCE: contracts/nes-250-contract.md § Test catalog § Process-supervision fence (T-FENCE-SUPERVISOR-NOOP-IF-FINALIZED)
#[test]
fn t_fence_supervisor_noop_if_captured_child_row_already_finalized() {
    let child_uuid = "44444444-4444-4444-4444-444444444444";
    let child_marker = CompositeInvocationId {
        source: "fixture-child".to_string(),
        id: child_uuid.to_string(),
    }
    .stderr_line();
    let fixture = Fixture::with_script_body(&format!(
        r#"printf '%s\n' "{child_marker}" >&2
exit 7"#
    ));
    let child_id = seed_running_child_for_first_parent(&fixture, child_uuid);
    let db = fixture.open_db();
    db.finalize_invocation(child_id, false, 7, None, Some("exit_nonzero"))
        .unwrap();

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(child_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(child_row.status, InvocationStatus::Failed);
    assert_eq!(child_row.success, Some(false));
    assert_eq!(child_row.exit_code, Some(7));
    assert_eq!(child_row.terminal_reason.as_deref(), Some("exit_nonzero"));
}

// RISK: supervisor fallback could finalize an unrelated running row if a provider spoofs another invocation marker (proposal §test-intent "supervised-child fence test", assumptions A1/A4)
// LEVEL: particular-integration
// SOURCE: CodeRabbit pass 2 R2-F03 lineage hardening for process-supervision fence
#[test]
fn t_fence_supervisor_ignores_captured_row_without_current_parent_lineage() {
    let child_uuid = "55555555-5555-5555-5555-555555555555";
    let child_marker = CompositeInvocationId {
        source: "fixture-child".to_string(),
        id: child_uuid.to_string(),
    }
    .stderr_line();
    let fixture = Fixture::with_script_body(&format!(
        r#"printf '%s\n' "{child_marker}" >&2
exit 7"#
    ));
    fixture
        .open_db()
        .start_invocation(&oulipoly_state::InvocationStart {
            invocation_uuid: child_uuid.to_string(),
            model_name: "fixture-child-model".to_string(),
            provider_name: "fixture-child".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(child_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(child_row.status, InvocationStatus::Running);
    assert_eq!(child_row.success, None);
    assert_eq!(child_row.terminal_reason, None);
    assert_eq!(child_row.finished_at, None);
}

// RISK: post-wait supervision/diagnostics handling could prevent the one-shot owner row from terminalizing (proposal §test-intent "own-row fence ordering test", assumption A4)
// LEVEL: particular-integration
// SOURCE: contracts/nes-250-contract.md § Test catalog § Process-supervision fence (T-FENCE-OWN-ROW-AFTER-WAIT)
#[test]
fn t_fence_own_row_after_wait_finalizes_even_with_malformed_child_marker_noise() {
    let fixture = Fixture::with_script_body(
        r#"printf '%s\n' 'OULIPOLY_INVOCATION={"source":"fixture-child","id":"not-a-uuid"}' >&2
exit 7"#,
    );

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parent_invocation = parse_valid_invocations(&stderr)
        .into_iter()
        .find(|invocation| invocation.source == "fixture-provider")
        .expect("parent invocation marker should remain parseable");
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&parent_invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_nonzero"));
    assert!(row.finished_at.is_some());
}

// risk: exhaustive surfaces 33 and 35; level: particular-integration; source: contract § 5.6, A6, A10
#[test]
fn t_fence_supervisor_signal_parent_finalizes_captured_child_with_signal_reason() {
    let child_uuid = "66666666-6666-6666-6666-666666666666";
    let child_marker = CompositeInvocationId {
        source: "fixture-child".to_string(),
        id: child_uuid.to_string(),
    }
    .stderr_line();
    let fixture = Fixture::with_script_body(&format!(
        r#"printf '%s\n' "{child_marker}" >&2
kill -TERM "$$"
sleep 1"#
    ));
    seed_running_child_for_first_parent(&fixture, child_uuid);

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(143), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        parse_valid_invocations(&stderr)
            .iter()
            .any(|invocation| invocation.id == child_uuid),
        "child marker should be present in captured stderr: {stderr}"
    );
    let parent_invocation = parse_valid_invocations(&stderr)
        .into_iter()
        .find(|invocation| invocation.source == "fixture-provider")
        .expect("parent invocation marker should remain parseable");
    let db = fixture.open_db();
    let parent_row = db
        .get_invocation_by_uuid(&parent_invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(parent_row.status, InvocationStatus::Failed);
    assert_eq!(parent_row.success, Some(false));
    assert_eq!(parent_row.exit_code, Some(143));
    assert_eq!(
        parent_row.terminal_reason.as_deref(),
        Some("signal:SIGTERM")
    );

    let child_row = db.get_invocation_by_uuid(child_uuid).unwrap().unwrap();
    assert_eq!(child_row.status, InvocationStatus::Failed);
    assert_eq!(child_row.success, Some(false));
    assert_eq!(child_row.exit_code, Some(-1));
    assert_eq!(child_row.error_category, None);
    assert_eq!(
        child_row.terminal_reason.as_deref(),
        Some("supervisor_observed_signal:SIGTERM")
    );
    assert!(child_row.finished_at.is_some());
}
