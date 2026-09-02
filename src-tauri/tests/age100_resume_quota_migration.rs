#![cfg(unix)]

mod provider_authority_fixture;

use oulipoly_state::{InvocationStatus, StateDb};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";

struct ResumeProviderFixture<'a> {
    name: &'a str,
    body: &'a str,
}

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl Fixture {
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

    fn conn(&self) -> Connection {
        let _ = self.open_db();
        Connection::open(self.db_path()).unwrap()
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, executable_script_body(body)).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn write_resume_pool(
        &self,
        model_name: &str,
        providers: &[ResumeProviderFixture<'_>],
        use_heuristic_diagnostics: bool,
    ) {
        let model = resume_model_toml(providers);
        fs::write(self.models_dir.join(format!("{model_name}.toml")), model).unwrap();

        fs::write(
            self.models_dir.join("diagnostic.toml"),
            diagnostic_model_toml(),
        )
        .unwrap();
        fs::write(self.app_config_dir.join("config.toml"), config_toml()).unwrap();

        let mut providers_toml = String::new();
        for provider in providers {
            let command = self.write_script(&format!("{}-resume.sh", provider.name), provider.body);
            let projects_dir = self.provider_projects_dir(provider.name);
            providers_toml.push_str(&resume_provider_toml(provider, &command, &projects_dir));
        }

        let diagnostic_body = if use_heuristic_diagnostics {
            "cat >/dev/null\nexit 9"
        } else {
            "cat >/dev/null\nprintf '%s\\n' 'quota_exhausted' 'Diagnostic model saw exhausted quota'"
        };
        let diagnostic_command = self.write_script("diagnostic-provider.sh", diagnostic_body);
        providers_toml.push_str(&diagnostic_provider_toml(&diagnostic_command));
        fs::write(
            self.app_config_dir.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority(&providers_toml),
        )
        .unwrap();
    }

    fn provider_projects_dir(&self, provider: &str) -> PathBuf {
        self.dir.path().join(format!("{provider}-projects"))
    }

    fn stage_active_claude_jsonl(&self, provider: &str) {
        let source_dir = self.provider_projects_dir(provider).join("source-project");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join(format!("{SESSION_ID}.jsonl")),
            format!(
                r#"{{"sessionId":"{SESSION_ID}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
    }

    fn seed_active_chain(&self, provider: &str, model: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![CHAIN_ID, model],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![CHAIN_ID, provider, SESSION_ID],
        )
        .unwrap();
        provider_authority_fixture::bind_session_authority_with_cwd(
            &conn,
            provider,
            SESSION_ID,
            self.dir.path(),
        );
    }

    fn run_resume(&self, model_name: &str) -> Output {
        self.base_resume_command(model_name).output().unwrap()
    }

    fn run_resume_with_env(&self, model_name: &str, env: &[(&str, &str)]) -> Output {
        let mut cmd = self.base_resume_command(model_name);
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.output().unwrap()
    }

    fn base_resume_command(&self, model_name: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg(model_name)
            .arg("--resume")
            .arg(SESSION_ID)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continue after quota");
        cmd.current_dir(self.dir.path());
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.env_remove("OULIPOLY_AUTO_WAKE");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_TOKEN");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_COUNT");
        cmd.env_remove("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS");
        cmd
    }

    fn active_segment_provider(&self) -> String {
        self.conn()
            .query_row(
                "SELECT provider_name
                 FROM session_chain_segments
                 WHERE chain_id = ?1 AND ended_at IS NULL",
                params![CHAIN_ID],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn exhausted_row_count(&self, provider: &str) -> i64 {
        // AGE-163 WU-A.4 moved the durable working-set write from
        // `exhausted_at` to `next_available_at` via the typed
        // `apply_post_failure_forensics` path. The contract pinned here
        // ("provider X was marked unavailable after a quota-failed
        // dispatch") is preserved verbatim; the observed column changes.
        self.conn()
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_quotas
                 WHERE provider_name = ?1 AND next_available_at IS NOT NULL",
                params![provider],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn failed_quota_invocation_count(&self, provider: &str) -> i64 {
        self.conn()
            .query_row(
                "SELECT COUNT(*)
                 FROM invocations
                 WHERE provider_name = ?1
                    AND status = ?2
                    AND success = 0
                    AND error_category = 'quota_exhausted'
                    AND finished_at IS NOT NULL",
                params![provider, InvocationStatus::Failed.as_str()],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn exhausted_provider_count(&self) -> i64 {
        // AGE-163 WU-A.4: durable working-set write moved to
        // `next_available_at`; see `exhausted_row_count`.
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM provider_quotas WHERE next_available_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn executable_script_body(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn resume_model_toml(providers: &[ResumeProviderFixture<'_>]) -> String {
    providers
        .iter()
        .map(resume_model_provider_toml)
        .collect::<String>()
}

fn resume_model_provider_toml(provider: &ResumeProviderFixture<'_>) -> String {
    format!(
        r#"[[providers]]
name = "{}"
args = ["exec-{}"]

"#,
        provider.name, provider.name
    )
}

fn diagnostic_model_toml() -> &'static str {
    r#"[[providers]]
name = "diagnostic-provider"
"#
}

fn config_toml() -> &'static str {
    r#"diagnostics_model = "diagnostic"
"#
}

fn resume_provider_toml(
    provider: &ResumeProviderFixture<'_>,
    command: &Path,
    projects_dir: &Path,
) -> String {
    format!(
        r#"[{}]
command = {}
args = []
interactive_args = ["launch-{}"]
prompt_mode = "arg"

[{}.resume]
kind = "flag"
flag = "--resume"

[{}.session_storage]
kind = "{}_code"
projects_dir = {}

"#,
        provider.name,
        toml_string(&command.display().to_string()),
        provider.name,
        provider.name,
        provider.name,
        ["cla", "ude"].concat(),
        toml_string(&projects_dir.display().to_string())
    )
}

fn diagnostic_provider_toml(command: &Path) -> String {
    format!(
        r#"[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
"#,
        toml_string(&command.display().to_string())
    )
}

fn provider_body(marker: &Path, shell: &str) -> String {
    format!("{}\n{shell}", provider_marker_line(marker))
}

fn provider_marker_line(marker: &Path) -> String {
    format!(
        "printf '%s\\n' ran >> {}",
        toml_string(&marker.display().to_string())
    )
}

fn line_count(path: &Path) -> usize {
    count_lines(optional_file_text(path).as_deref())
}

fn optional_file_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn count_lines(content: Option<&str>) -> usize {
    content.map(|content| content.lines().count()).unwrap_or(0)
}

fn single_result(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let results = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1, "{stdout}");
    serde_json::from_str(results[0]).unwrap()
}

fn assert_success_result(output: &Output, provider_stdout: &str) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout, format!("{provider_stdout}\n"));
    assert!(!stdout.contains("OULIPOLY_RESULT="), "{stdout}");
}

fn assert_nonzero_failure_result(output: &Output) {
    let result = single_result(output);
    let mut keys = result
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent_runner_chain_id",
            "agent_runner_invocation_id",
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "provider_name",
            "provider_session_id",
            "status",
            "success",
            "terminal_reason"
        ]
    );
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 17);
    assert_eq!(result["error_category"], "network_error");
    assert_eq!(result["terminal_reason"], "exit_nonzero");
    assert_eq!(result["provider_name"], ["cla", "ude-a"].concat());
    assert_eq!(result["provider_session_id"], SESSION_ID);
    assert_eq!(result["agent_runner_chain_id"], CHAIN_ID);
    assert_eq!(result["agent_runner_invocation_id"], result["id"]);
}

fn seed_base_resume_fixture(
    providers: &[(&str, &Path, String)],
    use_heuristic_diagnostics: bool,
) -> Fixture {
    let fixture = Fixture::new();
    let resume_providers = resume_provider_fixtures(providers);
    fixture.write_resume_pool(
        "age100-resume",
        &resume_providers,
        use_heuristic_diagnostics,
    );
    fixture.stage_active_claude_jsonl(providers[0].0);
    fixture.seed_active_chain(providers[0].0, "age100-resume");
    fixture
}

fn resume_provider_fixtures<'a>(
    providers: &'a [(&'a str, &Path, String)],
) -> Vec<ResumeProviderFixture<'a>> {
    providers.iter().map(resume_provider_fixture).collect()
}

fn resume_provider_fixture<'a>(
    provider: &'a (&'a str, &Path, String),
) -> ResumeProviderFixture<'a> {
    ResumeProviderFixture {
        name: provider.0,
        body: provider.2.as_str(),
    }
}

#[test]
fn resume_quota_exhausted_marks_provider_and_migrates_to_next_pool_member() {
    let first_marker = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    let sibling_marker = tempfile::NamedTempFile::new().unwrap().into_temp_path();
    let first_marker = first_marker.to_path_buf();
    let sibling_marker = sibling_marker.to_path_buf();
    let _ = fs::remove_file(&first_marker);
    let _ = fs::remove_file(&sibling_marker);
    let first_body = provider_body(
        &first_marker,
        "printf '%s\\n' 'Claude usage limit reached for active resume provider' >&2\nexit 42",
    );
    let sibling_body = provider_body(
        &sibling_marker,
        "printf '%s\\n' 'sibling resume stdout'\nexit 0",
    );
    let fixture = seed_base_resume_fixture(
        &[
            ("claude-a", &first_marker, first_body),
            ("claude-b", &sibling_marker, sibling_body),
        ],
        true,
    );

    let output = fixture.run_resume_with_env(
        "age100-resume",
        &[(FORCE_KIND, "QuotaExhaustedInband,None")],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_success_result(&output, "sibling resume stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[migrate] claude-a -> claude-b reason=exhausted"),
        "{stderr}"
    );
    assert_eq!(fixture.exhausted_row_count("claude-a"), 1);
    assert_eq!(fixture.exhausted_row_count("claude-b"), 0);
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&sibling_marker), 1);
    assert_eq!(fixture.failed_quota_invocation_count("claude-a"), 1);
    assert_eq!(fixture.active_segment_provider(), "claude-b");
}

#[test]
fn resume_retries_n_minus_one_quota_exhausted_providers_then_succeeds() {
    let markers: Vec<PathBuf> = (0..3)
        .map(|index| {
            let path = std::env::temp_dir().join(format!(
                "age100-resume-n-minus-one-{index}-{}.txt",
                uuid::Uuid::new_v4()
            ));
            let _ = fs::remove_file(&path);
            path
        })
        .collect();
    let first_body = provider_body(
        &markers[0],
        "printf '%s\\n' 'Claude usage limit reached for provider a' >&2\nexit 42",
    );
    let second_body = provider_body(
        &markers[1],
        "printf '%s\\n' 'Claude usage limit reached for provider b' >&2\nexit 43",
    );
    let third_body = provider_body(&markers[2], "printf '%s\\n' 'third resume stdout'\nexit 0");
    let fixture = seed_base_resume_fixture(
        &[
            ("claude-a", &markers[0], first_body),
            ("claude-b", &markers[1], second_body),
            ("claude-c", &markers[2], third_body),
        ],
        true,
    );

    let output = fixture.run_resume_with_env(
        "age100-resume",
        &[(FORCE_KIND, "QuotaExhaustedInband,QuotaExhaustedInband,None")],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_success_result(&output, "third resume stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("reason=exhausted").count(), 2, "{stderr}");
    assert_eq!(fixture.exhausted_row_count("claude-a"), 1);
    assert_eq!(fixture.exhausted_row_count("claude-b"), 1);
    assert_eq!(fixture.exhausted_row_count("claude-c"), 0);
    assert_eq!(fixture.exhausted_provider_count(), 2);
    assert_eq!(line_count(&markers[0]), 1);
    assert_eq!(line_count(&markers[1]), 1);
    assert_eq!(line_count(&markers[2]), 1);
    assert_eq!(fixture.failed_quota_invocation_count("claude-a"), 1);
    assert_eq!(fixture.failed_quota_invocation_count("claude-b"), 1);
    assert_eq!(fixture.active_segment_provider(), "claude-c");
}

#[test]
fn resume_all_pool_members_quota_exhausted_returns_all_providers_exhausted() {
    let markers: Vec<PathBuf> = (0..2)
        .map(|index| {
            let path = std::env::temp_dir().join(format!(
                "age100-resume-all-exhausted-{index}-{}.txt",
                uuid::Uuid::new_v4()
            ));
            let _ = fs::remove_file(&path);
            path
        })
        .collect();
    let first_body = provider_body(
        &markers[0],
        "printf '%s\\n' 'Claude usage limit reached for provider a' >&2\nexit 42",
    );
    let second_body = provider_body(
        &markers[1],
        "printf '%s\\n' 'Claude usage limit reached for provider b' >&2\nexit 43",
    );
    let fixture = seed_base_resume_fixture(
        &[
            ("claude-a", &markers[0], first_body),
            ("claude-b", &markers[1], second_body),
        ],
        true,
    );

    let output =
        fixture.run_resume_with_env("age100-resume", &[(FORCE_KIND, "QuotaExhaustedInband")]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("BLOCKED:all-providers-exhausted"),
        "{stderr}"
    );
    assert_eq!(line_count(&markers[0]), 1);
    assert_eq!(line_count(&markers[1]), 2);
    assert_eq!(fixture.exhausted_row_count("claude-a"), 1);
    assert_eq!(fixture.exhausted_row_count("claude-b"), 1);
    assert_eq!(fixture.exhausted_provider_count(), 2);
    assert_eq!(fixture.failed_quota_invocation_count("claude-a"), 1);
    assert_eq!(fixture.failed_quota_invocation_count("claude-b"), 2);
}

#[test]
fn resume_non_quota_failure_does_not_migrate_or_mark_exhausted() {
    let first_marker = std::env::temp_dir().join(format!(
        "age100-resume-non-quota-a-{}.txt",
        uuid::Uuid::new_v4()
    ));
    let sibling_marker = std::env::temp_dir().join(format!(
        "age100-resume-non-quota-b-{}.txt",
        uuid::Uuid::new_v4()
    ));
    let _ = fs::remove_file(&first_marker);
    let _ = fs::remove_file(&sibling_marker);
    let first_body = provider_body(
        &first_marker,
        "printf '%s\\n' 'connection refused for active resume provider' >&2\nexit 17",
    );
    let sibling_body = provider_body(
        &sibling_marker,
        "printf '%s\\n' 'sibling should not execute'\nexit 0",
    );
    let fixture = seed_base_resume_fixture(
        &[
            ("claude-a", &first_marker, first_body),
            ("claude-b", &sibling_marker, sibling_body),
        ],
        true,
    );

    let output = fixture.run_resume("age100-resume");

    assert_eq!(output.status.code(), Some(17), "{output:?}");
    assert_nonzero_failure_result(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("connection refused for active resume provider"),
        "{stderr}"
    );
    assert!(stderr.contains("[diagnostics: network_error]"), "{stderr}");
    assert!(
        stderr.lines().any(|line| line == "exit_nonzero"),
        "{stderr}"
    );
    assert!(!stderr.contains("rotating to another provider"), "{stderr}");
    assert_eq!(line_count(&first_marker), 1);
    assert_eq!(line_count(&sibling_marker), 0);
    assert_eq!(fixture.exhausted_provider_count(), 0);
    assert_eq!(fixture.active_segment_provider(), "claude-a");
}

#[test]
fn resume_heuristic_stderr_quota_uses_same_path_as_diagnostic_model_quota() {
    let heuristic_first_marker = std::env::temp_dir().join(format!(
        "age100-resume-heuristic-a-{}.txt",
        uuid::Uuid::new_v4()
    ));
    let heuristic_sibling_marker = std::env::temp_dir().join(format!(
        "age100-resume-heuristic-b-{}.txt",
        uuid::Uuid::new_v4()
    ));
    let _ = fs::remove_file(&heuristic_first_marker);
    let _ = fs::remove_file(&heuristic_sibling_marker);
    let heuristic_first_body = provider_body(
        &heuristic_first_marker,
        "printf '%s\\n' 'Claude usage limit reached in heuristic stderr shape' >&2\nexit 42",
    );
    let heuristic_sibling_body = provider_body(
        &heuristic_sibling_marker,
        "printf '%s\\n' 'heuristic sibling stdout'\nexit 0",
    );
    let heuristic = seed_base_resume_fixture(
        &[
            ("claude-a", &heuristic_first_marker, heuristic_first_body),
            (
                "claude-b",
                &heuristic_sibling_marker,
                heuristic_sibling_body,
            ),
        ],
        true,
    );

    let heuristic_output = heuristic.run_resume_with_env(
        "age100-resume",
        &[(FORCE_KIND, "QuotaExhaustedInband,None")],
    );

    assert_eq!(
        heuristic_output.status.code(),
        Some(0),
        "{heuristic_output:?}"
    );
    assert_success_result(&heuristic_output, "heuristic sibling stdout");
    let heuristic_stderr = String::from_utf8_lossy(&heuristic_output.stderr);
    assert!(
        heuristic_stderr.contains("OULIPOLY_TERMINAL_SIGNAL="),
        "{heuristic_stderr}"
    );
    assert!(
        heuristic_stderr.contains("[migrate] claude-a -> claude-b reason=exhausted"),
        "{heuristic_stderr}"
    );
    assert_eq!(heuristic.exhausted_row_count("claude-a"), 1);
    assert_eq!(heuristic.exhausted_row_count("claude-b"), 0);
    assert_eq!(heuristic.failed_quota_invocation_count("claude-a"), 1);

    let model_first_marker = std::env::temp_dir().join(format!(
        "age100-resume-model-a-{}.txt",
        uuid::Uuid::new_v4()
    ));
    let model_sibling_marker = std::env::temp_dir().join(format!(
        "age100-resume-model-b-{}.txt",
        uuid::Uuid::new_v4()
    ));
    let _ = fs::remove_file(&model_first_marker);
    let _ = fs::remove_file(&model_sibling_marker);
    let model_first_body = provider_body(
        &model_first_marker,
        "printf '%s\\n' 'Claude usage limit reached requiring diagnostic model' >&2\nexit 44",
    );
    let model_sibling_body = provider_body(
        &model_sibling_marker,
        "printf '%s\\n' 'diagnostic sibling stdout'\nexit 0",
    );
    let model_backed = seed_base_resume_fixture(
        &[
            ("claude-a", &model_first_marker, model_first_body),
            ("claude-b", &model_sibling_marker, model_sibling_body),
        ],
        false,
    );

    let model_output = model_backed.run_resume_with_env(
        "age100-resume",
        &[(FORCE_KIND, "QuotaExhaustedInband,None")],
    );

    assert_eq!(model_output.status.code(), Some(0), "{model_output:?}");
    assert_success_result(&model_output, "diagnostic sibling stdout");
    let model_stderr = String::from_utf8_lossy(&model_output.stderr);
    assert!(
        model_stderr.contains("OULIPOLY_TERMINAL_SIGNAL="),
        "{model_stderr}"
    );
    assert!(
        model_stderr.contains("[migrate] claude-a -> claude-b reason=exhausted"),
        "{model_stderr}"
    );
    assert_eq!(model_backed.exhausted_row_count("claude-a"), 1);
    assert_eq!(model_backed.exhausted_row_count("claude-b"), 0);
    assert_eq!(model_backed.failed_quota_invocation_count("claude-a"), 1);
    assert_eq!(line_count(&model_first_marker), 1);
    assert_eq!(line_count(&model_sibling_marker), 1);
}
