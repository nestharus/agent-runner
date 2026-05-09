#![cfg(unix)]

use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
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
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn run_one_shot(&self, model_name: &str) -> Output {
        self.command()
            .arg("--models-dir")
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
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn write_model(&self, model_name: &str, providers: &[&str]) {
        let mut body = String::new();
        for provider in providers {
            body.push_str(&format!(
                r#"[[providers]]
name = "{provider}"
args = []
interactive_args = ["model-interactive"]

"#
            ));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_providers(&self, providers: &[&str], include_quota_scripts: bool) {
        let mut body = String::new();
        for (index, provider) in providers.iter().enumerate() {
            let command = self.write_script(
                &format!("{provider}-command.sh"),
                &format!("printf '%s\\n' '{provider} executed'"),
            );
            let quota = if include_quota_scripts {
                let quota_script = self.write_script(
                    &format!("{provider}-quota.sh"),
                    &format!(
                        "printf '%s\\n' '{{\"windows\":[{{\"used_percent\":{},\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'",
                        10 + index
                    ),
                );
                format!(
                    "quota_script = {}\n",
                    toml_string(&shell_path(&quota_script))
                )
            } else {
                String::new()
            };
            body.push_str(&format!(
                r#"[{provider}]
command = {}
args = []
interactive_args = ["provider-interactive"]
prompt_mode = "arg"
{quota}
"#,
                toml_string(&command.display().to_string())
            ));
        }
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
    }

    fn write_sessions(&self, providers: &[&str]) {
        let mut body = String::new();
        for provider in providers {
            let script = self.write_script(
                &format!("{provider}-sessions.sh"),
                &format!(
                    "printf '%s\\n' '{{\"session_id\":\"{provider}-session\",\"turn_id\":\"turn-1\",\"timestamp\":\"2026-04-17T08:00:00Z\",\"role\":\"assistant\"}}'"
                ),
            );
            let state_dir = self.dir.path().join(format!("{provider}-sessions-state"));
            body.push_str(&format!(
                r#"[{provider}]
turn_script = {}
state_dir = {}

"#,
                toml_string(&shell_path(&script)),
                toml_string(&state_dir.display().to_string())
            ));
        }
        fs::write(self.app_config_dir.join("sessions.toml"), body).unwrap();
    }
}

fn shell_path(path: &Path) -> String {
    format!("{:?}", path.display().to_string())
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn parse_invocation(stderr: &str) -> CompositeInvocationId {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_INVOCATION="))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "stderr should contain exactly one invocation line: {stderr}"
    );
    let raw = lines[0].strip_prefix("OULIPOLY_INVOCATION=").unwrap();
    CompositeInvocationId::parse_env_value(raw).unwrap()
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
fn age_35_one_shot_run_with_balancing_uses_balance_context_to_refresh_and_scan_all_model_providers()
{
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
            1,
            "one-shot routing should scan sessions before selecting {provider}"
        );
    }
}

#[test]
fn age_35_non_resume_repl_uses_balance_context_to_refresh_and_scan_all_model_providers() {
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
            1,
            "non-resume REPL routing should scan sessions before selecting {provider}"
        );
    }
}

#[test]
fn age_35_gui_test_model_with_db_path_remains_outside_invocation_lifecycle() {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs");
    let source = fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", source_path.display()));
    let command_start = source
        .find("async fn test_model(")
        .expect("test_model command signature");
    let command_decl_end = source[command_start..]
        .find(")")
        .map(|idx| command_start + idx)
        .expect("test_model command signature close");
    let command_decl = &source[command_start..=command_decl_end];
    let body = source_block_after(&source, "fn test_model_with_db_path(");

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
        body.contains("RoutingServiceRequest"),
        "test_model_with_db_path should route through the routing service request"
    );
    assert!(
        body.contains(".select_route("),
        "test_model_with_db_path should select via RoutingServicePort"
    );
    assert!(
        body.contains("ctx: None"),
        "test_model_with_db_path should keep cached-only routing"
    );
    assert!(
        !body.contains("balancer::select_provider"),
        "test_model_with_db_path should not call balancer::select_provider directly after cutover"
    );
    let route = body.find(".select_route(").expect("routing service call");
    let effective_provider = body
        .find("effective_provider_for_model_provider")
        .expect("effective provider resolution");
    let exhausted_mark = body
        .find("mark_exhausted(&")
        .expect("caller-owned exhausted mark");
    assert!(
        route < effective_provider,
        "effective-provider resolution must remain downstream of provider index selection"
    );
    assert!(
        effective_provider < exhausted_mark,
        "quota-like stderr exhaustion marking must remain caller-owned after execution"
    );
    assert!(
        body.contains("parent_invocation_env: None"),
        "test_model_with_db_path should execute without parent invocation env"
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
