#![cfg(unix)]

//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter

use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct CliFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl CliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();

        Self {
            dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn run_one_shot(&self, model_name: &str) -> Output {
        self.run_one_shot_with_env(model_name, &[])
    }

    fn run_one_shot_with_env(&self, model_name: &str, env: &[(&str, &str)]) -> Output {
        let mut cmd = self.command();
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(model_name)
            .arg("ping")
            .output()
            .unwrap()
    }

    fn run_repl(&self, model_name: &str) -> Output {
        self.command()
            .arg("repl")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(model_name)
            .output()
            .unwrap()
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        write_executable_script(&path, &script_content(body));
        path
    }

    fn write_model(&self, model_name: &str, providers: &[&str]) {
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            model_toml(providers),
        )
        .unwrap();
    }

    fn write_providers(&self, providers: &[&str], include_quota_scripts: bool) {
        let entries = self.provider_entries(providers, include_quota_scripts);
        fs::write(
            self.app_config_dir.join("providers.toml"),
            provider_entries_toml(&entries),
        )
        .unwrap();
    }

    fn provider_entries(
        &self,
        providers: &[&str],
        include_quota_scripts: bool,
    ) -> Vec<ProviderTomlEntry> {
        providers
            .iter()
            .enumerate()
            .map(|(index, provider)| self.provider_entry(provider, index, include_quota_scripts))
            .collect()
    }

    fn provider_entry(
        &self,
        provider: &str,
        index: usize,
        include_quota_scripts: bool,
    ) -> ProviderTomlEntry {
        let command = self.write_provider_command_script(provider);
        let quota_script = self.provider_quota_script(provider, index, include_quota_scripts);
        provider_toml_entry(provider, command, quota_script)
    }

    fn write_provider_command_script(&self, provider: &str) -> PathBuf {
        self.write_script(
            &provider_command_script_name(provider),
            &provider_command_body(provider),
        )
    }

    fn provider_quota_script(
        &self,
        provider: &str,
        index: usize,
        include_quota_scripts: bool,
    ) -> Option<PathBuf> {
        include_quota_scripts.then(|| self.write_provider_quota_script(provider, index))
    }

    fn write_provider_quota_script(&self, provider: &str, index: usize) -> PathBuf {
        self.write_script(
            &provider_quota_script_name(provider),
            &quota_script_body(index),
        )
    }

    fn write_providers_with_command_bodies(&self, providers: &[(&str, &str)]) {
        let entries = self.command_body_provider_entries(providers);
        fs::write(
            self.app_config_dir.join("providers.toml"),
            provider_entries_toml(&entries),
        )
        .unwrap();
    }

    fn command_body_provider_entries(&self, providers: &[(&str, &str)]) -> Vec<ProviderTomlEntry> {
        providers
            .iter()
            .map(|(provider, command_body)| {
                self.command_body_provider_entry(provider, command_body)
            })
            .collect()
    }

    fn command_body_provider_entry(&self, provider: &str, command_body: &str) -> ProviderTomlEntry {
        let command = self.write_script(&provider_command_script_name(provider), command_body);
        provider_toml_entry(provider, command, None)
    }

    fn write_sessions(&self, providers: &[&str]) {
        let entries = self.session_entries(providers);
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            session_entries_toml(&entries),
        )
        .unwrap();
    }

    fn session_entries(&self, providers: &[&str]) -> Vec<SessionTomlEntry> {
        providers
            .iter()
            .map(|provider| self.session_entry(provider))
            .collect()
    }

    fn session_entry(&self, provider: &str) -> SessionTomlEntry {
        let script = self.write_session_script(provider);
        session_toml_entry(provider, script, self.session_state_dir(provider))
    }

    fn write_session_script(&self, provider: &str) -> PathBuf {
        self.write_script(
            &session_script_name(provider),
            &session_script_body(provider),
        )
    }

    fn session_state_dir(&self, provider: &str) -> PathBuf {
        self.dir.path().join(format!("{provider}-sessions-state"))
    }
}

struct ProviderTomlEntry {
    provider: String,
    command: PathBuf,
    quota_script: Option<PathBuf>,
}

struct SessionTomlEntry {
    provider: String,
    script: PathBuf,
    state_dir: PathBuf,
}

fn provider_toml_entry(
    provider: &str,
    command: PathBuf,
    quota_script: Option<PathBuf>,
) -> ProviderTomlEntry {
    ProviderTomlEntry {
        provider: provider.to_string(),
        command,
        quota_script,
    }
}

fn session_toml_entry(provider: &str, script: PathBuf, state_dir: PathBuf) -> SessionTomlEntry {
    SessionTomlEntry {
        provider: provider.to_string(),
        script,
        state_dir,
    }
}

fn provider_command_script_name(provider: &str) -> String {
    format!("{provider}-command.sh")
}

fn provider_command_body(provider: &str) -> String {
    format!("printf '%s\\n' '{provider} executed'")
}

fn provider_quota_script_name(provider: &str) -> String {
    format!("{provider}-quota.sh")
}

fn quota_script_body(index: usize) -> String {
    format!(
        "printf '%s\\n' '{{\"windows\":[{{\"used_percent\":{},\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'",
        10 + index
    )
}

fn session_script_name(provider: &str) -> String {
    format!("{provider}-sessions.sh")
}

fn session_script_body(provider: &str) -> String {
    format!(
        "printf '%s\\n' '{{\"session_id\":\"{provider}-session\",\"turn_id\":\"turn-1\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"role\":\"assistant\"}}'"
    )
}

fn write_executable_script(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    set_executable(path);
}

fn set_executable(path: &Path) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn script_content(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn model_toml(providers: &[&str]) -> String {
    providers
        .iter()
        .map(|provider| model_entry(provider))
        .collect()
}

fn model_entry(provider: &str) -> String {
    format!(
        r#"[[providers]]
name = "{provider}"
args = []
interactive_args = ["model-interactive"]

"#
    )
}

fn provider_entries_toml(entries: &[ProviderTomlEntry]) -> String {
    entries.iter().map(provider_entry_toml).collect()
}

fn provider_entry_toml(entry: &ProviderTomlEntry) -> String {
    format!(
        r#"[{}]
command = {}
args = []
interactive_args = ["provider-interactive"]
prompt_mode = "arg"
{}"#,
        entry.provider,
        toml_string(&entry.command.display().to_string()),
        quota_script_toml(&entry.quota_script)
    )
}

fn quota_script_toml(path: &Option<PathBuf>) -> String {
    path.as_ref()
        .map(|path| format!("quota_script = {}\n", toml_string(&shell_path(path))))
        .unwrap_or_default()
}

fn session_entries_toml(entries: &[SessionTomlEntry]) -> String {
    entries.iter().map(session_entry_toml).collect()
}

fn session_entry_toml(entry: &SessionTomlEntry) -> String {
    format!(
        r#"[{}]
turn_script = {}
state_dir = {}

"#,
        entry.provider,
        toml_string(&shell_path(&entry.script)),
        toml_string(&entry.state_dir.display().to_string())
    )
}

fn shell_path(path: &Path) -> String {
    format!("{:?}", path.display().to_string())
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn parse_invocation(stderr: &str) -> CompositeInvocationId {
    let lines = invocation_lines(stderr);
    assert_single_invocation_line(&lines, stderr);
    let raw = invocation_marker_payload(lines[0]);
    CompositeInvocationId::parse_env_value(raw).unwrap()
}

fn result_envelope(stdout: &str) -> serde_json::Value {
    let lines = result_envelope_lines(stdout);
    assert_single_result_envelope_line(&lines, stdout);
    parse_result_envelope_payload(result_envelope_payload(lines[0]))
}

fn invocation_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_INVOCATION="))
        .collect()
}

fn result_envelope_lines(stdout: &str) -> Vec<&str> {
    stdout
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_RESULT="))
        .collect()
}

fn assert_single_invocation_line(lines: &[&str], stderr: &str) {
    assert_eq!(
        lines.len(),
        1,
        "stderr should contain exactly one invocation line: {stderr}"
    );
}

fn assert_single_result_envelope_line(lines: &[&str], stdout: &str) {
    assert_eq!(
        lines.len(),
        1,
        "stdout should contain exactly one result envelope line: {stdout}"
    );
}

fn invocation_marker_payload(line: &str) -> &str {
    line.strip_prefix("OULIPOLY_INVOCATION=").unwrap()
}

fn result_envelope_payload(line: &str) -> &str {
    line.strip_prefix("OULIPOLY_RESULT=").unwrap()
}

fn parse_result_envelope_payload(payload: &str) -> serde_json::Value {
    serde_json::from_str(payload).unwrap()
}

fn assert_result_envelope_contract(
    output: &Output,
    expected_exit_code: i32,
    expected_status: &str,
    expected_success: bool,
) {
    assert_eq!(output.status.code(), Some(expected_exit_code), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let envelope = result_envelope(&stdout);
    let keys: BTreeSet<&str> = envelope
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let base_keys = BTreeSet::from([
        "error_category",
        "exit_code",
        "finished_at",
        "id",
        "status",
        "success",
        "terminal_reason",
    ]);
    if expected_success {
        assert_eq!(keys, base_keys);
    } else {
        let mut expected = base_keys;
        expected.extend([
            "agent_runner_invocation_id",
            "provider_name",
            "provider_session_id",
            "agent_runner_chain_id",
        ]);
        assert_eq!(keys, expected);
        assert_eq!(envelope["agent_runner_invocation_id"], envelope["id"]);
    }
    assert_eq!(envelope["status"], expected_status);
    assert_eq!(envelope["success"], expected_success);
    assert_eq!(envelope["exit_code"], expected_exit_code);
    assert!(envelope["finished_at"].as_str().is_some());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation = parse_invocation(&stderr);
    assert_eq!(envelope["id"], invocation.id);
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("OULIPOLY_PARENT_INVOCATION="))
            .count(),
        0,
        "no parent marker should be emitted when no parent env is supplied: {stderr}"
    );
}

fn source_block_after<'a>(source: &'a str, start: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let open_idx = source[start_idx..]
        .find('{')
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing opening brace after {start}"));
    let mut depth = 1usize;
    let mut idx = open_idx + 1;
    let bytes = source.as_bytes();

    while idx < bytes.len() {
        match bytes[idx] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[open_idx + 1..idx];
                }
            }
            _ => {}
        }
        idx += 1;
    }

    panic!("missing closing brace after {start}");
}

#[test]
fn idx_main_02_one_shot_emits_single_result_envelope_and_invocation_marker() {
    let fixture = CliFixture::new();
    fixture.write_model("idx-main-02-ok", &["idx-main-02-ok-provider"]);
    fixture.write_model("idx-main-02-fail", &["idx-main-02-fail-provider"]);
    fixture.write_providers_with_command_bodies(&[
        (
            "idx-main-02-ok-provider",
            "printf '%s\\n' 'idx-main-02 provider stdout'",
        ),
        (
            "idx-main-02-fail-provider",
            "printf '%s\\n' 'idx-main-02 provider stderr' >&2\nexit 23",
        ),
    ]);

    let success = fixture.run_one_shot("idx-main-02-ok");
    let success_stdout = String::from_utf8_lossy(&success.stdout);
    assert!(
        success_stdout.starts_with("idx-main-02 provider stdout\n"),
        "{success_stdout}"
    );
    assert_result_envelope_contract(&success, 0, "succeeded", true);

    let failure = fixture.run_one_shot("idx-main-02-fail");
    assert_result_envelope_contract(&failure, 23, "failed", false);
}

#[test]
fn age_35_one_shot_routing_refreshes_quota_without_scanning_session_history() {
    let fixture = CliFixture::new();
    fixture.write_model("age35", &["age35-a", "age35-b"]);
    fixture.write_providers(&["age35-a", "age35-b"], true);
    fixture.write_sessions(&["age35-a", "age35-b"]);

    let output = fixture.run_one_shot("age35");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let db = fixture.open_db();
    for provider in ["age35-a", "age35-b"] {
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            1,
            "one-shot routing should refresh stale quota before selecting {provider}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            0,
            "one-shot routing must not scan session history for {provider}"
        );
    }
}

#[test]
fn age_81_one_shot_retries_first_quota_exhausted_provider_then_succeeds() {
    let fixture = CliFixture::new();
    fixture.write_model("age81", &["age81-a", "age81-b"]);
    fixture.write_providers_with_command_bodies(&[
        (
            "age81-a",
            "printf '%s\\n' 'quota exhausted for fixture provider' >&2\nexit 42",
        ),
        ("age81-b", "printf '%s\\n' 'age81-b executed'"),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age81",
        &[(
            "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND",
            "QuotaExhaustedInband,None",
        )],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("age81-b executed\n"), "{stdout}");
    assert!(stdout.contains("OULIPOLY_RESULT="), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("age81-a") && stderr.contains("rotating to another provider"),
        "{stderr}"
    );
    let db = fixture.open_db();
    // AGE-163 WU-A.4: durable working-set unavailability moved from
    // `exhausted_at` to `next_available_at` via the typed forensics path.
    assert!(
        db.get_quota("age81-a")
            .unwrap()
            .and_then(|quota| quota.next_available_at)
            .is_some(),
        "first provider should be marked unavailable (next_available_at) before retry"
    );
    assert!(
        db.get_quota("age81-b")
            .unwrap()
            .and_then(|quota| quota.next_available_at)
            .is_none(),
        "successful retry provider should not be marked unavailable"
    );
}

#[test]
fn age_81_one_shot_retries_n_minus_one_quota_exhausted_providers_then_succeeds() {
    let fixture = CliFixture::new();
    fixture.write_model("age81", &["age81-a", "age81-b", "age81-c"]);
    fixture.write_providers_with_command_bodies(&[
        (
            "age81-a",
            "printf '%s\\n' 'quota exhausted for fixture provider a' >&2\nexit 42",
        ),
        (
            "age81-b",
            "printf '%s\\n' 'quota exhausted for fixture provider b' >&2\nexit 42",
        ),
        ("age81-c", "printf '%s\\n' 'age81-c executed'"),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age81",
        &[(
            "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND",
            "QuotaExhaustedInband,QuotaExhaustedInband,None",
        )],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("age81-c executed\n"), "{stdout}");
    assert!(stdout.contains("OULIPOLY_RESULT="), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("rotating to another provider").count(),
        2,
        "{stderr}"
    );
    let db = fixture.open_db();
    // AGE-163 WU-A.4: see `next_available_at` comment above.
    for provider in ["age81-a", "age81-b"] {
        assert!(
            db.get_quota(provider)
                .unwrap()
                .and_then(|quota| quota.next_available_at)
                .is_some(),
            "{provider} should be marked unavailable"
        );
    }
}

#[test]
fn age_81_one_shot_all_quota_exhausted_returns_pool_error() {
    let fixture = CliFixture::new();
    fixture.write_model("age81", &["age81-a", "age81-b"]);
    fixture.write_providers_with_command_bodies(&[
        (
            "age81-a",
            "printf '%s\\n' 'quota exhausted for fixture provider a' >&2\nexit 42",
        ),
        (
            "age81-b",
            "printf '%s\\n' 'quota exhausted for fixture provider b' >&2\nexit 42",
        ),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age81",
        &[(
            "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND",
            "QuotaExhaustedInband",
        )],
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("all providers in pool age81 are quota-exhausted"),
        "{stderr}"
    );
    let db = fixture.open_db();
    // AGE-163 WU-A.4: see `next_available_at` comment above.
    for provider in ["age81-a", "age81-b"] {
        assert!(
            db.get_quota(provider)
                .unwrap()
                .and_then(|quota| quota.next_available_at)
                .is_some(),
            "{provider} should be marked unavailable"
        );
    }
}

#[test]
fn age_81_one_shot_non_quota_failure_does_not_retry() {
    let fixture = CliFixture::new();
    fixture.write_model("age81", &["age81-a", "age81-b"]);
    fixture.write_providers_with_command_bodies(&[
        (
            "age81-a",
            "printf '%s\\n' 'network failure for fixture provider' >&2\nexit 42",
        ),
        ("age81-b", "printf '%s\\n' 'age81-b executed'"),
    ]);

    let output = fixture.run_one_shot("age81");

    assert_eq!(output.status.code(), Some(42), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("OULIPOLY_RESULT="), "{stdout}");
    assert!(!stdout.contains("age81-b executed"), "{stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("rotating to another provider"), "{stderr}");
    assert!(!stderr.contains("age81-b executed"), "{stderr}");
    let db = fixture.open_db();
    // AGE-163 WU-A.4: non-quota failures may still mutate routing state
    // (UpstreamApiDown / TransientStderrNoise) via the typed forensics
    // path. The original `exhausted_at`-only assertion is preserved
    // verbatim — `exhausted_at` is no longer written by the typed path.
    assert!(
        db.get_quota("age81-a")
            .unwrap()
            .and_then(|quota| quota.exhausted_at)
            .is_none(),
        "non-quota failure should not flip the legacy `exhausted_at` flag"
    );
}

#[test]
fn age_35_non_resume_repl_refreshes_quota_without_scanning_session_history() {
    let fixture = CliFixture::new();
    fixture.write_model("age35", &["age35-a", "age35-b"]);
    fixture.write_providers(&["age35-a", "age35-b"], true);
    fixture.write_sessions(&["age35-a", "age35-b"]);

    let output = fixture.run_repl("age35");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let db = fixture.open_db();
    for provider in ["age35-a", "age35-b"] {
        assert_eq!(
            db.get_windows(provider).unwrap().len(),
            1,
            "non-resume REPL routing should refresh stale quota before selecting {provider}"
        );
        assert_eq!(
            db.count_assistant_turns_since(provider, None).unwrap(),
            0,
            "non-resume REPL routing must not scan session history for {provider}"
        );
    }
}

#[test]
fn age_35_gui_test_model_with_db_path_remains_outside_invocation_lifecycle() {
    let source_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/test_model/orchestration.rs");
    let dispatch_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/test_model/dispatch.rs");
    let mapper_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/test_model/mapper.rs");
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));
    let dispatch = fs::read_to_string(&dispatch_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", dispatch_path.display()));
    let mapper = fs::read_to_string(&mapper_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", mapper_path.display()));
    let command_start = source
        .find("async fn test_model(")
        .expect("test_model command signature");
    let command_decl_end = source[command_start..]
        .find(")")
        .map(|idx| command_start + idx)
        .expect("test_model command signature close");
    let command_decl = &source[command_start..=command_decl_end];
    let body = source_block_after(&source, "fn test_model_with_db_path(");
    let route_body = source_block_after(&dispatch, "fn select_test_model_route(");
    let request_body = source_block_after(&mapper, "fn build_effective_executor_request(");
    let mark_body = source_block_after(&dispatch, "fn mark_effective_provider_exhausted(");

    assert!(
        command_decl.contains("name: String"),
        "test_model Tauri command must keep the existing name argument"
    );
    assert!(
        command_decl.contains("tauri::State"),
        "test_model Tauri command must keep state injection internal to Tauri"
    );
    assert!(
        !command_decl.contains("RoutingServicePort") && !command_decl.contains("routing"),
        "test_model Tauri command must not expose routing over IPC"
    );
    assert!(
        route_body.contains("RoutingServiceRequest"),
        "test_model_with_db_path should route through the routing service request"
    );
    assert!(
        route_body.contains(".select_route("),
        "test_model_with_db_path should select via RoutingServicePort"
    );
    assert!(
        route_body.contains("ctx: None"),
        "test_model_with_db_path should keep cached-only routing"
    );
    assert!(
        !body.contains("balancer::select_provider"),
        "test_model_with_db_path should not call balancer::select_provider directly after cutover"
    );
    let route = body
        .find("select_test_model_route")
        .expect("routing service call");
    let effective_provider = body
        .find("effective_provider_for_model_provider")
        .expect("effective provider resolution");
    let executor_dispatch = body
        .find("execute_effective_request")
        .expect("executor service dispatch");
    let exhausted_mark = body
        .find("apply_exhaustion_disposition")
        .expect("caller-owned exhausted disposition");
    assert!(
        route < effective_provider,
        "effective-provider resolution must remain downstream of provider index selection"
    );
    assert!(
        effective_provider < executor_dispatch && executor_dispatch < exhausted_mark,
        "quota-like stderr exhaustion marking must remain caller-owned after execution"
    );
    assert!(
        request_body.contains("parent_invocation_env: None"),
        "test_model_with_db_path should execute without parent invocation env"
    );
    assert!(
        mark_body.contains("ProviderQuotaRepository"),
        "test_model_with_db_path should keep quota marking behind the repository"
    );
    for lifecycle_call in [
        "start_invocation(",
        "finalize_invocation(",
        "InvocationLifecycle",
        "CompositeInvocationId",
        "record_returned_artifacts(",
        "increment_calls_since_refresh(",
    ] {
        assert!(
            !body.contains(lifecycle_call),
            "test_model_with_db_path must not enter invocation lifecycle via {lifecycle_call}"
        );
    }
}

#[test]
fn age_35_one_shot_post_run_increments_calls_since_refresh_for_selected_provider() {
    let fixture = CliFixture::new();
    fixture.write_model("age35", &["age35-solo"]);
    fixture.write_providers(&["age35-solo"], false);

    let output = fixture.run_one_shot("age35");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation = parse_invocation(&stderr);
    let db = fixture.open_db();
    let row = db
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap_or_else(|| panic!("missing invocation row {}", invocation.id));
    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.success, Some(true));

    let quota = db
        .get_quota(&invocation.source)
        .unwrap()
        .unwrap_or_else(|| panic!("missing quota row for {}", invocation.source));
    assert_eq!(
        quota.calls_since_refresh, 1,
        "one-shot lifecycle should tick the selected provider after a completed run"
    );
}
