#![cfg(unix)]

//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `formatter`, `validator`, `predicate`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/tests/age15_usage_cli_characterization.rs
//!     role: adapter
//!     Translates:
//!       - public-AgentsCli-syntax-contract
//!       - process-and-filesystem-CLI-fixture-contract
//!       - StateDb-CLI-observation-contract
//! ```

use agent_runner_lib::usage::cli::Cli;
use chrono::{TimeZone, Utc};
use clap::{Parser, error::ErrorKind};
use oulipoly_state::{QuotaWindowInput, StateDb};
use rusqlite::{Connection, params};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TRACE_UUID: &str = "00000000-0000-4000-8000-000000000001";
const SESSION_UUID: &str = "00000000-0000-4000-8000-000000000002";
const CHAIN_UUID: &str = "00000000-0000-4000-8000-000000000003";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    marker_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let agents_dir = app_config_dir.join("agents");
        let marker_dir = dir.path().join("markers");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&agents_dir).unwrap();
        fs::create_dir_all(&marker_dir).unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
            marker_dir,
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
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env("OULIPOLY_CONFIG_HOME", &self.config_home);
        cmd.env("OULIPOLY_DATA_HOME", &self.data_home);
        cmd
    }

    fn usage_command(&self) -> Command {
        let mut cmd = self.command();
        cmd.arg("--usage").arg("--models-dir").arg(&self.models_dir);
        cmd
    }

    fn write_model(&self, model_name: &str, providers: &[&str]) {
        let body = providers
            .iter()
            .map(|provider| format!("[[providers]]\nname = \"{provider}\"\n"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(self.models_dir.join(format!("{model_name}.toml")), body).unwrap();
    }

    fn write_providers(&self, entries: &[ProviderFixture]) {
        fs::write(
            self.app_config_dir.join("providers.toml"),
            providers_toml(entries),
        )
        .unwrap();
    }

    fn write_quota_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self._dir.path().join(name);
        write_executable(&path, body);
        path
    }

    fn write_marker_model_script(&self, name: &str) -> PathBuf {
        let marker = self.marker_dir.join(format!("{name}.ran"));
        self.write_quota_script(
            &format!("{name}.sh"),
            &format!(
                "#!/usr/bin/env bash\nset -euo pipefail\ntouch '{}'\nprintf 'model-ran\\n'\n",
                marker.display()
            ),
        )
    }

    fn assert_marker_absent(&self, name: &str) {
        assert!(
            !self.marker_dir.join(format!("{name}.ran")).exists(),
            "marker {name} should not have been written"
        );
    }
}

struct ProviderFixture {
    name: String,
    command: String,
    quota_script: Option<String>,
    auth_refresh_command: Option<String>,
}

impl ProviderFixture {
    fn no_usage(name: &str, command: &str) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            quota_script: None,
            auth_refresh_command: None,
        }
    }

    fn with_script(name: &str, command: &str, quota_script: &Path) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            quota_script: Some(quota_script.display().to_string()),
            auth_refresh_command: None,
        }
    }

    fn with_script_and_auth(
        name: &str,
        command: &str,
        quota_script: &Path,
        auth_refresh_command: &Path,
    ) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            quota_script: Some(quota_script.display().to_string()),
            auth_refresh_command: Some(auth_refresh_command.display().to_string()),
        }
    }
}

fn providers_toml(entries: &[ProviderFixture]) -> String {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&format!("[{}]\n", entry.name));
        out.push_str(&format!("command = \"{}\"\n", escape_toml(&entry.command)));
        out.push_str("args = []\nprompt_mode = \"arg\"\n");
        if let Some(script) = &entry.quota_script {
            out.push_str(&format!("quota_script = \"{}\"\n", escape_toml(script)));
        }
        if let Some(command) = &entry.auth_refresh_command {
            out.push_str(&format!(
                "auth_refresh_command = \"{}\"\n",
                escape_toml(command)
            ));
        }
        out.push('\n');
    }
    out
}

fn escape_toml(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn quota_script_json(script_log: &Path, json: &str) -> String {
    format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf ran >> '{}'\ncat <<'JSON'\n{}\nJSON\n",
        script_log.display(),
        json
    )
}

fn quota_script_first_empty_then_error(marker: &Path) -> String {
    format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nif [[ -f '{}' ]]; then\n  echo retry quota failed >&2\n  exit 19\nfi\ntouch '{}'\ncat <<'JSON'\n{{\"windows\":[]}}\nJSON\n",
        marker.display(),
        marker.display()
    )
}

fn run_usage(fixture: &Fixture) -> (i32, String, String) {
    let output = fixture.usage_command().output().unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_success_with_stdout(code: i32, stdout: &str, stderr: &str) {
    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        !stdout.trim().is_empty(),
        "usage must render a table on stdout"
    );
}

fn assert_contains_all(haystack: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            haystack.contains(needle),
            "missing `{needle}` in:\n{haystack}"
        );
    }
}

fn assert_not_contains_any(haystack: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            !haystack.contains(needle),
            "unexpected `{needle}` in:\n{haystack}"
        );
    }
}

fn invocation_count(db_path: &Path) -> i64 {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

fn session_turn_count(db_path: &Path) -> i64 {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM session_turns", [], |row| row.get(0))
        .unwrap()
}

fn quota_window_count(db_path: &Path, provider: &str) -> i64 {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM provider_quota_windows WHERE provider_name = ?1",
        params![provider],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

fn quota_window_used_percent(db_path: &Path, provider: &str, window_id: i64) -> f64 {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row(
        "SELECT used_percent FROM provider_quota_windows WHERE provider_name = ?1 AND window_id = ?2",
        params![provider, window_id],
        |row| row.get(0),
    )
    .unwrap()
}

fn clap_rejects(argv: &[&str]) {
    let err = Cli::try_parse_from(argv).expect_err("argv should be rejected");
    assert_eq!(err.kind(), ErrorKind::ArgumentConflict, "{err}");
}

fn assert_ordered(haystack: &str, needles: &[&str]) {
    let mut last = 0;
    for needle in needles {
        let offset = haystack[last..]
            .find(needle)
            .unwrap_or_else(|| panic!("missing ordered `{needle}` in:\n{haystack}"));
        last += offset + needle.len();
    }
}

#[test]
fn usage_accepted_with_no_other_args() {
    Cli::try_parse_from(["oulipoly-agent-runner", "--usage"]).unwrap();
}

#[test]
fn usage_rejected_with_positional_prompt() {
    clap_rejects(&["oulipoly-agent-runner", "--usage", "hello"]);
}

#[test]
fn usage_rejected_with_file_flag() {
    clap_rejects(&[
        "oulipoly-agent-runner",
        "--usage",
        "--file",
        "/tmp/prompt.md",
    ]);
}

#[test]
fn usage_rejected_with_model_flag() {
    clap_rejects(&["oulipoly-agent-runner", "--usage", "--model", "fixture"]);
}

#[test]
fn usage_rejected_with_named_agent_positional() {
    clap_rejects(&["oulipoly-agent-runner", "--usage", "writer", "draft"]);
}

#[test]
fn usage_rejected_with_agent_file_flag() {
    clap_rejects(&[
        "oulipoly-agent-runner",
        "--usage",
        "--agent-file",
        "/tmp/agent.md",
    ]);
}

#[test]
fn usage_rejected_with_new_flag() {
    clap_rejects(&["oulipoly-agent-runner", "--usage", "--new"]);
}

#[test]
fn usage_rejected_with_top_level_resume() {
    clap_rejects(&["oulipoly-agent-runner", "--usage", "--resume", SESSION_UUID]);
}

#[test]
fn usage_rejected_with_fresh_continuation_request() {
    clap_rejects(&[
        "oulipoly-agent-runner",
        "--usage",
        "--resume",
        SESSION_UUID,
        "--fresh-continuation-request",
        "/tmp/request.json",
    ]);
}

#[test]
fn fresh_continuation_request_is_opt_in_and_requires_top_level_resume() {
    Cli::try_parse_from([
        "oulipoly-agent-runner",
        "--resume",
        SESSION_UUID,
        "--fresh-continuation-request",
        "/tmp/request.json",
    ])
    .unwrap();

    let error = Cli::try_parse_from([
        "oulipoly-agent-runner",
        "--fresh-continuation-request",
        "/tmp/request.json",
    ])
    .expect_err("a fresh continuation request must name the resume session");
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument, "{error}");
}

#[test]
fn fresh_continuation_request_rejects_provider_rotation() {
    let error = Cli::try_parse_from([
        "oulipoly-agent-runner",
        "--resume",
        SESSION_UUID,
        "--fresh-continuation-request",
        "/tmp/request.json",
        "--rotate-provider",
        "rotation-target",
    ])
    .expect_err("a fresh continuation request must reject provider rotation");

    assert_eq!(error.kind(), ErrorKind::ArgumentConflict, "{error}");
}

#[test]
fn usage_rejected_with_top_level_rotate_provider_flag() {
    clap_rejects(&[
        "oulipoly-agent-runner",
        "--usage",
        "--rotate-provider",
        "claude2",
    ]);
}

#[test]
fn usage_rejected_with_input_flag() {
    clap_rejects(&["oulipoly-agent-runner", "--usage", "-i", "size=large"]);
}

#[test]
fn usage_rejected_with_each_subcommand_family() {
    let cases: &[&[&str]] = &[
        &["oulipoly-agent-runner", "--usage", "trace", TRACE_UUID],
        &["oulipoly-agent-runner", "--usage", "repl", "fixture"],
        &["oulipoly-agent-runner", "--usage", "resume", CHAIN_UUID],
        &[
            "oulipoly-agent-runner",
            "--usage",
            "resume-list",
            TRACE_UUID,
        ],
        &[
            "oulipoly-agent-runner",
            "--usage",
            "session",
            "locate",
            SESSION_UUID,
        ],
        &[
            "oulipoly-agent-runner",
            "--usage",
            "session",
            "schema-probe",
        ],
        &[
            "oulipoly-agent-runner",
            "--usage",
            "session",
            "export",
            SESSION_UUID,
        ],
        &[
            "oulipoly-agent-runner",
            "--usage",
            "session",
            "pause-handshake",
            SESSION_UUID,
        ],
        &[
            "oulipoly-agent-runner",
            "--usage",
            "session",
            "resume-handshake",
            SESSION_UUID,
            "--token",
            "lease-token",
        ],
        &[
            "oulipoly-agent-runner",
            "--usage",
            "session",
            "import-replace",
            SESSION_UUID,
            "--from-file",
            "/tmp/session.jsonl",
        ],
        &["oulipoly-agent-runner", "--usage", "migrate-db"],
        &["oulipoly-agent-runner", "--usage", "migrate", "--rebuild"],
        &["oulipoly-agent-runner", "--usage", "migrate-config"],
    ];

    for argv in cases {
        clap_rejects(argv);
    }
}

#[test]
fn usage_does_not_select_route_create_invocation_read_prompt_or_run_model() {
    let fixture = Fixture::new();
    let quota_log = fixture.marker_dir.join("quota.log");
    let quota_script = fixture.write_quota_script(
        "usage-ok.sh",
        &quota_script_json(
            &quota_log,
            r#"{"windows":[{"label":"weekly","used_percent":37,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );
    let model_script = fixture.write_marker_model_script("model-provider");

    fixture.write_model("fixture", &["fixture-provider"]);
    fixture.write_providers(&[ProviderFixture::with_script(
        "fixture-provider",
        &model_script.display().to_string(),
        &quota_script,
    )]);

    let mut cmd = fixture.usage_command();
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"prompt that usage must not read")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_contains_all(&stdout, &["fixture-provider", "weekly", "37%"]);
    assert_eq!(invocation_count(&fixture.db_path()), 0);
    assert_eq!(fs::read_to_string(&quota_log).unwrap(), "ran");
    fixture.assert_marker_absent("model-provider");
    assert!(!stderr.contains("OULIPOLY_INVOCATION="), "{stderr}");
}

#[test]
fn usage_runs_pending_replace_recovery_before_dispatch_with_no_invocation_row() {
    let fixture = Fixture::new();
    let journal_root = fixture
        .data_home
        .join("oulipoly-agent-runner/replace_journal");
    fs::create_dir_all(&journal_root).unwrap();
    fs::write(journal_root.join("session-bad.pending"), b"not json").unwrap();

    let quota_script = fixture.write_quota_script(
        "quota.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"weekly","used_percent":12,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );
    fixture.write_model("fixture", &["claude"]);
    fixture.write_providers(&[ProviderFixture::with_script(
        "claude",
        "claude",
        &quota_script,
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["claude", "weekly"]);
    assert_eq!(invocation_count(&fixture.db_path()), 0);
    assert!(
        journal_root.join("quarantine/session-bad.pending").exists(),
        "recover_pending_replaces should quarantine invalid pending journals before usage dispatch"
    );
}

#[test]
fn usage_runs_after_global_preconditions_and_before_execution_lifecycle_branches() {
    let fixture = Fixture::new();
    let quota_script = fixture.write_quota_script(
        "quota.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"5h-burst","used_percent":9,"resets_at":"2099-01-01T05:00:00Z"}]}"#,
        ),
    );
    let diagnostic_script = fixture.write_marker_model_script("diagnostic");
    fixture.write_model("fixture", &["usage-account"]);
    fixture.write_model("diagnostic", &["diagnostic-provider"]);
    fixture.write_providers(&[
        ProviderFixture::with_script("usage-account", "claude", &quota_script),
        ProviderFixture::no_usage(
            "diagnostic-provider",
            &diagnostic_script.display().to_string(),
        ),
    ]);
    fs::write(
        fixture.app_config_dir.join("config.toml"),
        r#"diagnostics_model = "diagnostic"
"#,
    )
    .unwrap();

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["usage-account", "5h-burst"]);
    assert_eq!(invocation_count(&fixture.db_path()), 0);
    fixture.assert_marker_absent("diagnostic");
}

#[test]
fn usage_does_not_ingest_sessions() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude"]);
    fixture.write_providers(&[ProviderFixture::no_usage("claude", "claude")]);
    let db = fixture.open_db();
    db.connection()
        .execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "claude",
                "session-a",
                "turn-1",
                "2026-05-01T00:00:00Z",
                "assistant",
                "/tmp/session-a.jsonl",
                "2026-05-01T00:00:01Z",
            ],
        )
        .unwrap();
    db.connection()
        .execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "claude",
                "session-b",
                "turn-1",
                "2026-05-01T00:01:00Z",
                "assistant",
                "/tmp/session-b.jsonl",
                "2026-05-01T00:01:01Z",
            ],
        )
        .unwrap();
    drop(db);
    let before = session_turn_count(&fixture.db_path());
    assert_eq!(before, 2);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["claude", "(no usage api)"]);
    assert_eq!(session_turn_count(&fixture.db_path()), before);
}

#[test]
fn usage_reports_accounts_from_loaded_model_pools_with_provider_metadata() {
    let fixture = Fixture::new();
    let quota_script = fixture.write_quota_script(
        "claude-quota.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"weekly","used_percent":44,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );
    fixture.write_model("primary", &["claude2", "local-helper"]);
    fixture.write_providers(&[
        ProviderFixture::with_script("claude2", "Anthropic Claude", &quota_script),
        ProviderFixture::no_usage("local-helper", "node /opt/local-helper.js"),
        ProviderFixture::no_usage("stale-not-in-model", "codex"),
    ]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(
        &stdout,
        &[
            "claude2",
            "anthropic",
            "weekly",
            "local-helper",
            "node",
            "(no usage api)",
        ],
    );
    assert_not_contains_any(&stdout, &["stale-not-in-model"]);
}

#[test]
fn usage_models_dir_override_pins_enumeration_source() {
    let fixture = Fixture::new();
    let override_dir = fixture._dir.path().join("override-models");
    fs::create_dir_all(&override_dir).unwrap();
    fs::write(
        fixture.models_dir.join("default.toml"),
        "[[providers]]\nname = \"claude-default-only\"\n\n[[providers]]\nname = \"codex-default-only\"\n",
    )
    .unwrap();
    fs::write(
        override_dir.join("override.toml"),
        "[[providers]]\nname = \"claude-override-only\"\n\n[[providers]]\nname = \"chatgpt-override-only\"\n",
    )
    .unwrap();
    fixture.write_providers(&[
        ProviderFixture::no_usage("claude-default-only", "claude"),
        ProviderFixture::no_usage("codex-default-only", "codex"),
        ProviderFixture::no_usage("claude-override-only", "claude"),
        ProviderFixture::no_usage("chatgpt-override-only", "codex"),
    ]);

    let mut cmd = fixture.command();
    cmd.arg("--usage").arg("--models-dir").arg(&override_dir);
    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_contains_all(&stdout, &["claude-override-only", "chatgpt-override-only"]);
    assert_not_contains_any(&stdout, &["claude-default-only", "codex-default-only"]);
}

#[test]
fn usage_renders_accounts_in_stable_provider_then_model_order() {
    let fixture = Fixture::new();
    fixture.write_model("b-model", &["zulu", "alpha", "alpha"]);
    fixture.write_model("a-model", &["middle"]);
    fixture.write_providers(&[
        ProviderFixture::no_usage("zulu", "claude"),
        ProviderFixture::no_usage("alpha", "codex"),
        ProviderFixture::no_usage("middle", "gemini"),
    ]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_ordered(&stdout, &["alpha", "middle", "zulu"]);
    assert_eq!(stdout.matches("alpha").count(), 1, "{stdout}");
}

#[test]
fn usage_invokes_quota_script_even_when_cache_is_fresh() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude"]);
    let script_log = fixture.marker_dir.join("quota.log");
    let quota_script = fixture.write_quota_script(
        "fresh-cache-bypass.sh",
        &quota_script_json(
            &script_log,
            r#"{"windows":[{"label":"live","used_percent":77,"resets_at":"2099-02-01T00:00:00Z"}]}"#,
        ),
    );
    fixture.write_providers(&[ProviderFixture::with_script(
        "claude",
        "claude",
        &quota_script,
    )]);
    fixture
        .open_db()
        .upsert_quota_refresh(
            "claude",
            &[QuotaWindowInput {
                used_percent: 0.11,
                resets_at: Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
            }],
        )
        .unwrap();

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["claude", "live", "77%"]);
    assert!(!stdout.contains("11%"), "{stdout}");
    assert_eq!(fs::read_to_string(script_log).unwrap(), "ran");
    assert!((quota_window_used_percent(&fixture.db_path(), "claude", 0) - 0.77).abs() < 1e-6);
}

#[test]
fn usage_renders_no_usage_api_row_for_provider_without_quota_script() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["local-helper"]);
    fixture.write_providers(&[ProviderFixture::no_usage(
        "local-helper",
        "node /opt/helper.js",
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["local-helper", "node", "(no usage api)"]);
    assert_eq!(quota_window_count(&fixture.db_path(), "local-helper"), 0);
}

#[test]
fn usage_renders_in_flight_row_state_when_refresh_outcome_is_in_flight_with_exit_zero_and_no_cache_write()
 {
    let fixture = Fixture::new();
    let started = fixture.marker_dir.join("started");
    let release = fixture.marker_dir.join("release");
    let quota_script = fixture.write_quota_script(
        "blocking-quota.sh",
        &format!(
            "#!/usr/bin/env bash\nset -euo pipefail\ntouch '{}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\nprintf '{{\"windows\":[{{\"label\":\"weekly\",\"used_percent\":31,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}\\n'\n",
            started.display(),
            release.display()
        ),
    );
    fixture.write_model("fixture", &["claude"]);
    fixture.write_providers(&[ProviderFixture::with_script(
        "claude",
        "claude",
        &quota_script,
    )]);

    let mut first_cmd = fixture.usage_command();
    first_cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let first = first_cmd.spawn().unwrap();
    wait_for_path(&started);

    let mut second = fixture.usage_command();
    second.stdout(Stdio::piped()).stderr(Stdio::piped());
    let second_output = wait_with_timeout(second.spawn().unwrap(), Duration::from_secs(2));

    let second_output =
        second_output.expect("second --usage should return promptly with an in-flight row");
    let second_stdout = String::from_utf8_lossy(&second_output.stdout);
    let second_stderr = String::from_utf8_lossy(&second_output.stderr);
    assert!(
        second_output.status.success(),
        "stdout:\n{second_stdout}\nstderr:\n{second_stderr}"
    );
    assert_contains_all(&second_stdout, &["claude", "(in flight)"]);
    assert_eq!(
        quota_window_count(&fixture.db_path(), "claude"),
        0,
        "second usage run must not write stale or partial cache data while first refresh is in flight"
    );

    fs::write(&release, b"go").unwrap();
    let first_output = first.wait_with_output().unwrap();

    let first_stdout = String::from_utf8_lossy(&first_output.stdout);
    let first_stderr = String::from_utf8_lossy(&first_output.stderr);
    assert!(
        first_output.status.success(),
        "stdout:\n{first_stdout}\nstderr:\n{first_stderr}"
    );
}

#[test]
fn usage_renders_error_row_for_any_failed_outcome_variant_with_exit_zero_and_no_fresh_sample_rendered()
 {
    let script_fail = Fixture::new();
    script_fail.write_model("fixture", &["script-fail"]);
    let failing_script = script_fail.write_quota_script(
        "script-fails.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\necho provider exploded >&2\nexit 42\n",
    );
    script_fail.write_providers(&[ProviderFixture::with_script(
        "script-fail",
        "claude",
        &failing_script,
    )]);
    let (code, stdout, stderr) = run_usage(&script_fail);
    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["script-fail", "(error:", "provider exploded"]);
    assert_not_contains_any(&stdout, &["weekly", "42% / 100%"]);
    assert_eq!(quota_window_count(&script_fail.db_path(), "script-fail"), 0);

    let cache_fail = Fixture::new();
    cache_fail.write_model("fixture", &["cache-fail"]);
    let cache_script = cache_fail.write_quota_script(
        "cache-fail.sh",
        &quota_script_json(
            &cache_fail.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"weekly","used_percent":55,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );
    cache_fail.write_providers(&[ProviderFixture::with_script(
        "cache-fail",
        "claude",
        &cache_script,
    )]);
    make_quota_window_cache_unwritable(&cache_fail.db_path());
    let (code, stdout, stderr) = run_usage(&cache_fail);
    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["cache-fail", "(error:", "cache write failed"]);
    assert_not_contains_any(&stdout, &["weekly", "55%"]);

    let auth_fail = Fixture::new();
    auth_fail.write_model("fixture", &["auth-fail"]);
    let first_script = auth_fail.write_quota_script(
        "quota-needs-auth.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\necho expired token >&2\nexit 4\n",
    );
    let auth_script = auth_fail.write_quota_script(
        "auth-refresh-fails.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\necho login required >&2\nexit 17\n",
    );
    auth_fail.write_providers(&[ProviderFixture::with_script_and_auth(
        "auth-fail",
        "claude",
        &first_script,
        &auth_script,
    )]);
    let (code, stdout, stderr) = run_usage(&auth_fail);
    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["auth-fail", "(error:", "login required"]);
}

#[test]
fn usage_renders_error_row_when_refresh_outcome_failed_due_to_auth_refresh_command_nonzero_exit() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude"]);
    let quota_script = fixture.write_quota_script(
        "quota-empty-then-fails.sh",
        &quota_script_first_empty_then_error(&fixture.marker_dir.join("quota-called")),
    );
    let auth_script = fixture.write_quota_script(
        "auth-fail.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\necho auth refresh nonzero >&2\nexit 23\n",
    );
    fixture
        .open_db()
        .upsert_quota_refresh(
            "claude",
            &[QuotaWindowInput {
                used_percent: 0.30,
                resets_at: Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
            }],
        )
        .unwrap();
    fixture.write_providers(&[ProviderFixture::with_script_and_auth(
        "claude",
        "claude",
        &quota_script,
        &auth_script,
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(
        &stdout,
        &[
            "claude",
            "(error:",
            "Quota script exited 19",
            "retry quota failed",
            "auth refresh nonzero",
        ],
    );
    assert_not_contains_any(&stdout, &["30%", "(no windows)"]);
}

#[test]
fn usage_refresh_writes_quota_cache_and_records_delta_learning_samples_per_window() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude"]);
    let db = fixture.open_db();
    db.upsert_quota_refresh(
        "claude",
        &[QuotaWindowInput {
            used_percent: 0.10,
            resets_at: Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
        }],
    )
    .unwrap();
    let prior_refresh = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    db.connection()
        .execute(
            "UPDATE provider_quotas SET refreshed_at = ?1 WHERE provider_name = ?2",
            params![prior_refresh.to_rfc3339(), "claude"],
        )
        .unwrap();
    for i in 0..20 {
        let timestamp = prior_refresh + chrono::Duration::seconds((i + 1) as i64);
        db.connection()
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_sidechain)
                 VALUES (?1, 's1', ?2, ?3, 'assistant', 'test.json', ?4, 0)",
                params![
                    "claude",
                    format!("t{i}"),
                    timestamp.to_rfc3339(),
                    timestamp.to_rfc3339()
                ],
            )
            .unwrap();
    }
    drop(db);

    let quota_script = fixture.write_quota_script(
        "delta.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"weekly","used_percent":35,"resets_at":"2099-02-01T00:00:00Z"}]}"#,
        ),
    );
    fixture.write_providers(&[ProviderFixture::with_script(
        "claude",
        "claude",
        &quota_script,
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    let db = fixture.open_db();
    let windows = db.get_windows("claude").unwrap();
    assert_eq!(windows.len(), 1);
    assert!((windows[0].used_percent - 0.35).abs() < 1e-6);
    assert!(windows[0].last_delta_percent.unwrap() > 0.20, "{windows:?}");
    assert_eq!(windows[0].last_delta_calls, Some(20));
}

#[test]
fn usage_renders_no_windows_row_for_refresh_outcome_success_with_zero_windows_and_exit_zero() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude"]);
    let quota_script = fixture.write_quota_script(
        "empty-windows.sh",
        &quota_script_json(&fixture.marker_dir.join("quota.log"), r#"{"windows":[]}"#),
    );
    fixture.write_providers(&[ProviderFixture::with_script(
        "claude",
        "claude",
        &quota_script,
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["claude", "(no windows)"]);
    assert_eq!(quota_window_count(&fixture.db_path(), "claude"), 0);
}

#[test]
fn usage_table_renders_account_vendor_window_used_limit_and_remaining_columns() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["rich"]);
    let quota_script = fixture.write_quota_script(
        "rich.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"daily","used_percent":25,"resets_at":"2099-01-01T00:00:00Z","limit":1000,"remaining":750,"unit":"tokens"}]}"#,
        ),
    );
    fixture.write_providers(&[ProviderFixture::with_script(
        "rich",
        "Custom Provider",
        &quota_script,
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(
        &stdout.to_lowercase(),
        &["account", "vendor", "window", "used", "limit", "remaining"],
    );
    assert_contains_all(
        &stdout,
        &["rich", "custom", "daily", "250 / 1000 tokens", "750 tokens"],
    );
}

#[test]
fn usage_table_renders_vendor_column_from_documented_fallback_when_metadata_absent() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["agent-runner-helper", "claude2"]);
    fixture.write_providers(&[
        ProviderFixture::no_usage("claude2", "claude"),
        ProviderFixture::no_usage("agent-runner-helper", "node /opt/x.js"),
    ]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(
        &stdout,
        &["claude2", "claude", "agent-runner-helper", "node"],
    );
}

#[test]
fn usage_renders_window_label_column_with_index_fallback_when_label_absent() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["legacy"]);
    let quota_script = fixture.write_quota_script(
        "legacy.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"used_percent":10,"resets_at":"2099-01-01T00:00:00Z"},{"used_percent":20,"resets_at":"2099-01-01T05:00:00Z"}]}"#,
        ),
    );
    fixture.write_providers(&[ProviderFixture::with_script(
        "legacy",
        "claude",
        &quota_script,
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_ordered(&stdout, &["window-0", "10%", "window-1", "20%"]);
}

#[test]
fn usage_rich_fields_survive_script_to_mapper_path() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["rich"]);
    let quota_script = fixture.write_quota_script(
        "rich-fields.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"daily","used_percent":40,"resets_at":"2099-01-01T00:00:00Z","limit":250,"remaining":150,"unit":"requests"}]}"#,
        ),
    );
    fixture.write_providers(&[ProviderFixture::with_script(
        "rich",
        "claude",
        &quota_script,
    )]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["daily", "100 / 250 requests", "150 requests"]);
    assert_not_contains_any(&stdout, &["40% / 100%", "60%"]);
    assert!((quota_window_used_percent(&fixture.db_path(), "rich", 0) - 0.40).abs() < 1e-6);
}

#[test]
fn usage_partial_provider_failure_exits_zero() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["bad", "good"]);
    let bad_script = fixture.write_quota_script(
        "bad.sh",
        "#!/usr/bin/env bash\nset -euo pipefail\necho upstream timeout >&2\nexit 7\n",
    );
    let good_script = fixture.write_quota_script(
        "good.sh",
        &quota_script_json(
            &fixture.marker_dir.join("good.log"),
            r#"{"windows":[{"label":"weekly","used_percent":22,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );
    fixture.write_providers(&[
        ProviderFixture::with_script("bad", "claude", &bad_script),
        ProviderFixture::with_script("good", "codex", &good_script),
    ]);

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(
        &stdout,
        &["bad", "(error:", "upstream timeout", "good", "weekly"],
    );
}

#[test]
fn usage_local_state_open_failure_exits_nonzero() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude"]);
    fixture.write_providers(&[ProviderFixture::no_usage("claude", "claude")]);
    fs::create_dir_all(fixture.db_path().parent().unwrap()).unwrap();
    fs::write(fixture.db_path(), b"not a sqlite db").unwrap();

    let output = fixture.usage_command().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "");
    assert_contains_all(&stderr, &["state", "db"]);
}

#[test]
fn usage_malformed_providers_toml_exits_nonzero() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude"]);
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        "not = [valid",
    )
    .unwrap();

    let output = fixture.usage_command().output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr_lower = stderr.to_lowercase();

    assert!(
        !output.status.success(),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(stdout, "");
    assert_contains_all(&stderr_lower, &["providers.toml"]);
    assert!(
        stderr_lower.contains("parse") || stderr_lower.contains("toml"),
        "stderr should identify a provider config parse failure:\n{stderr}"
    );
}

#[test]
fn usage_warn_and_skips_when_model_references_provider_missing_from_providers_toml() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["claude-present", "claude-missing"]);
    fixture.write_providers(&[ProviderFixture::no_usage("claude-present", "claude")]);

    let output = fixture.usage_command().output().unwrap();
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr_lower = stderr.to_lowercase();

    assert_eq!(code, 0, "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stdout.contains("claude-present"),
        "present provider must render in stdout:\nstdout:\n{stdout}"
    );
    assert!(
        stderr.contains("claude-missing"),
        "dangling provider reference must be surfaced on stderr:\nstderr:\n{stderr}"
    );
    assert!(
        stderr.contains("fixture"),
        "stderr warning must name the referencing model:\nstderr:\n{stderr}"
    );
    assert!(
        stderr_lower.contains("warn")
            || stderr_lower.contains("skip")
            || stderr_lower.contains("missing"),
        "missing-provider notice must be surfaced as a non-fatal warning \
         (matching `warn` / `skip` / `missing`):\nstderr:\n{stderr}"
    );
}

#[test]
fn usage_exits_zero_when_a_refresh_outcome_failed_due_to_cache_write_with_error_row_rendered() {
    let fixture = Fixture::new();
    fixture.write_model("fixture", &["cache-fail"]);
    let quota_script = fixture.write_quota_script(
        "cache-fail.sh",
        &quota_script_json(
            &fixture.marker_dir.join("quota.log"),
            r#"{"windows":[{"label":"weekly","used_percent":66,"resets_at":"2099-01-01T00:00:00Z"}]}"#,
        ),
    );
    fixture.write_providers(&[ProviderFixture::with_script(
        "cache-fail",
        "claude",
        &quota_script,
    )]);
    make_quota_window_cache_unwritable(&fixture.db_path());

    let (code, stdout, stderr) = run_usage(&fixture);

    assert_success_with_stdout(code, &stdout, &stderr);
    assert_contains_all(&stdout, &["cache-fail", "(error:", "cache write failed"]);
    assert_not_contains_any(&stdout, &["weekly", "66%"]);
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {}", path.display());
}

fn wait_with_timeout(mut child: Child, timeout: Duration) -> Option<std::process::Output> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(_status) = child.try_wait().unwrap() {
            return Some(child.wait_with_output().unwrap());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn make_quota_window_cache_unwritable(db_path: &Path) {
    let db = StateDb::open(db_path).unwrap();
    drop(db);
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        "DROP TABLE provider_quota_windows;
         CREATE VIEW provider_quota_windows AS
         SELECT
            CAST(NULL AS TEXT) AS provider_name,
            CAST(NULL AS INTEGER) AS window_id,
            CAST(NULL AS REAL) AS used_percent,
            CAST(NULL AS TEXT) AS resets_at,
            CAST(NULL AS REAL) AS last_delta_percent,
            CAST(NULL AS INTEGER) AS last_delta_calls
         WHERE 0;",
    )
    .unwrap();
}
