#![cfg(unix)]

mod fixtures;

use fixtures::initiative_06_import_replace::{
    canonical_jsonl, prepared_claude_replace_fixture, prepared_codex_replace_fixture,
};
use oulipoly_state::{InvocationStart, StateDb};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const MODEL: &str = "claude-opus";
const TRACE_UUID: &str = "11111111-1111-1111-1111-111111111111";

struct CliFixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    agents_dir: PathBuf,
}

impl CliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let agents_dir = app_config_dir.join("agents");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&agents_dir).unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
            agents_dir,
        }
    }

    fn root(&self) -> &Path {
        self._dir.path()
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

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root().join(name);
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

    fn write_model(&self, model_name: &str, provider_name: &str, args: &[&str]) {
        let args = args
            .iter()
            .map(|arg| format!("{arg:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!(
                r#"[[providers]]
name = "{provider_name}"
args = [{args}]
"#
            ),
        )
        .unwrap();
    }

    fn write_provider(
        &self,
        provider_name: &str,
        script: &Path,
        args: &[&str],
        interactive_args: Option<&[&str]>,
    ) {
        let args = toml_array(args);
        let interactive = interactive_args
            .map(|items| format!("interactive_args = {}\n", toml_array(items)))
            .unwrap_or_default();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{provider_name}]
command = {:?}
args = {args}
{interactive}prompt_mode = "arg"
"#,
                script.to_string_lossy()
            ),
        )
        .unwrap();
    }

    fn write_claude_storage_provider(&self, provider_name: &str, projects_dir: &Path) {
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{provider_name}]
command = "provider-command-that-must-not-run"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"

[{provider_name}.resume]
kind = "flag"
flag = "--resume"

[{provider_name}.session_storage]
kind = "claude_code"
projects_dir = {:?}
"#,
                projects_dir.to_string_lossy()
            ),
        )
        .unwrap();
    }

    fn write_sessions_locator(&self, provider_name: &str, transcript_path: &Path) {
        let locator = self.write_script(
            &format!("{provider_name}-locator.sh"),
            &format!("printf '%s\\n' {}", sh_path(transcript_path)),
        );
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                "[{provider_name}]\nturn_script = \"true\"\ntranscript_locator = {:?}\nstate_dir = {:?}\n",
                locator.to_string_lossy(),
                self.root()
                    .join(format!("{provider_name}-locator-state"))
                    .to_string_lossy()
            ),
        )
        .unwrap();
    }

    fn seed_active_chain(&self, provider_name: &str, session_id: &str, model_name: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            params![CHAIN_A, model_name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![CHAIN_A, provider_name, session_id],
        )
        .unwrap();
    }

    fn seed_trace_row(&self) {
        let db = self.open_db();
        let row_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: TRACE_UUID.to_string(),
                model_name: "fixture".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.finalize_invocation(row_id, true, 0, None, None).unwrap();
    }

    fn stage_claude_transcript(&self, projects_dir: &Path, session_id: &str) -> PathBuf {
        let workspace = self.root().join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let encoded = workspace.to_string_lossy().replace('/', "-");
        let transcript_dir = projects_dir.join(encoded);
        fs::create_dir_all(&transcript_dir).unwrap();
        let path = transcript_dir.join(format!("{session_id}.jsonl"));
        fs::write(
            &path,
            format!(
                "{{\"sessionId\":\"{session_id}\",\"type\":\"assistant\",\"message\":\"fixture\"}}\n"
            ),
        )
        .unwrap();
        path
    }

    fn run_direct_model(&self, model_name: &str) -> Output {
        let mut cmd = self.command();
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(model_name)
            .arg("ping");
        cmd.output().unwrap()
    }

    fn run_named_agent(&self, agent: &str) -> Output {
        let mut cmd = self.command();
        cmd.arg(agent).arg("draft summary");
        cmd.output().unwrap()
    }
}

fn toml_array(items: &[&str]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|item| format!("{item:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn sh_path(path: &Path) -> String {
    format!("{:?}", path.to_string_lossy())
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn source_slice<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_idx = source
        .find(start)
        .unwrap_or_else(|| panic!("missing {start}"));
    let end_idx = source[start_idx..]
        .find(end)
        .map(|idx| start_idx + idx)
        .unwrap_or_else(|| panic!("missing {end} after {start}"));
    &source[start_idx..end_idx]
}

#[test]
fn age_33_one_shot_loads_models_with_provider_aware_codex_overlap_validation() {
    let fixture = CliFixture::new();
    fixture.write_model(
        "codex-overlap",
        "codex",
        &["-c", "sandbox_mode=danger-full-access"],
    );
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        r#"[codex]
command = "provider-command-that-must-not-run"
args = ["-c", "sandbox_mode=danger-full-access"]
prompt_mode = "arg"
"#,
    )
    .unwrap();

    let output = fixture.run_direct_model("codex-overlap");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let err = stderr(&output);
    assert!(err.contains("duplicates root [codex].args"), "{err}");
    assert!(err.contains("sandbox_mode=danger-full-access"), "{err}");
}

#[test]
fn age_33_one_shot_malformed_providers_defaults_before_model_provider_resolution() {
    let fixture = CliFixture::new();
    fixture.write_model("fixture", "fixture-provider", &[]);
    fs::write(fixture.app_config_dir.join("providers.toml"), "not = [").unwrap();

    let output = fixture.run_direct_model("fixture");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let err = stderr(&output);
    assert!(
        err.contains("provider fixture-provider is missing from providers.toml"),
        "{err}"
    );
    assert!(
        !err.contains("TOML parse error"),
        "one-shot provider load currently defaults malformed providers.toml: {err}"
    );
}

#[test]
fn age_33_default_provider_repl_strictly_surfaces_malformed_providers() {
    let fixture = CliFixture::new();
    fs::write(
        fixture.app_config_dir.join("config.toml"),
        r#"default_provider = "fixture""#,
    )
    .unwrap();
    fs::write(fixture.app_config_dir.join("providers.toml"), "not = [").unwrap();

    let mut cmd = fixture.command();
    cmd.arg("--new");
    let output = cmd.output().unwrap();

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let err = stderr(&output);
    assert!(err.contains("TOML parse error"), "{err}");
    assert!(err.contains("providers.toml"), "{err}");
}

#[test]
fn age_33_trace_strictly_rejects_malformed_sessions_config() {
    let fixture = CliFixture::new();
    fixture.seed_trace_row();
    fs::write(fixture.app_config_dir.join("sessions.toml"), "not = [").unwrap();

    let mut cmd = fixture.command();
    cmd.arg("trace").arg(TRACE_UUID).arg("--json");
    let output = cmd.output().unwrap();

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let err = stderr(&output);
    assert!(err.contains("Failed to load"), "{err}");
    assert!(err.contains("sessions.toml"), "{err}");
    assert!(err.contains("TOML parse error"), "{err}");
}

#[test]
fn age_33_locate_defaults_malformed_sessions_config_and_uses_storage_scan() {
    let fixture = CliFixture::new();
    let projects_dir = fixture.root().join("claude-projects");
    let transcript = fixture.stage_claude_transcript(&projects_dir, SESSION_A);
    fixture.write_model(MODEL, "claude", &[]);
    fixture.write_claude_storage_provider("claude", &projects_dir);
    fixture.write_sessions_locator("claude", &transcript);
    fs::write(fixture.app_config_dir.join("sessions.toml"), "not = [").unwrap();
    fixture.seed_active_chain("claude", SESSION_A, MODEL);

    let mut cmd = fixture.command();
    cmd.arg("session").arg("locate").arg(SESSION_A);
    let output = cmd.output().unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let json = stdout_json(&output);
    assert_eq!(json["session_id"], SESSION_A);
    assert_eq!(json["transcript_state"], "available");
    assert!(
        !stderr(&output).contains("TOML parse error"),
        "locate currently defaults malformed sessions.toml"
    );
}

#[test]
fn age_33_resolve_agent_uses_default_agents_dir_and_maps_missing_agent_to_unknown_agent() {
    let fixture = CliFixture::new();
    let script = fixture.write_script(
        "provider.sh",
        "printf '%s' \"$1\" > \"$PROMPT_DUMP\"\nprintf 'ok\\n'",
    );
    let prompt_dump = fixture.root().join("prompt.txt");
    fixture.write_model(MODEL, "fixture-provider", &[]);
    fixture.write_provider("fixture-provider", &script, &[], None);
    fs::write(
        fixture.agents_dir.join("helper.md"),
        format!(
            "---\ndescription: helper\nmodel: {MODEL}\noutput_format: ''\n---\nUse terse prose.\n"
        ),
    )
    .unwrap();

    let mut cmd = fixture.command();
    cmd.env("PROMPT_DUMP", &prompt_dump)
        .arg("helper")
        .arg("draft summary");
    let success = cmd.output().unwrap();
    assert_eq!(success.status.code(), Some(0), "{success:?}");
    let prompt = fs::read_to_string(&prompt_dump).unwrap();
    assert!(prompt.contains("Use terse prose."), "{prompt}");
    assert!(prompt.contains("draft summary"), "{prompt}");

    let missing = fixture.run_named_agent("missing-helper");
    assert_ne!(missing.status.code(), Some(0), "{missing:?}");
    assert!(stderr(&missing).contains("Unknown agent: missing-helper"));
}

#[test]
fn age_33_resolve_agent_cutover_uses_repository_for_both_loader_paths() {
    let source = include_str!("../src/main.rs");
    let resolve_agent = source_slice(source, "fn resolve_agent(", "struct FinalizerGuard");

    assert!(
        resolve_agent.contains("agent_config: &dyn AgentConfigRepository"),
        "resolve_agent must accept the post-cutover AgentConfigRepository parameter"
    );
    assert!(
        resolve_agent.contains("agent_config.load_agent_file(path)"),
        "explicit --agent-file must delegate through AgentConfigRepository::load_agent_file"
    );
    assert!(
        resolve_agent.contains("agent_config.load_agents(&agents_dir)?"),
        "named-agent lookup must delegate through AgentConfigRepository::load_agents"
    );

    let file_load = resolve_agent
        .find("agent_config.load_agent_file(path)")
        .expect("explicit file load");
    let dir_load = resolve_agent
        .find("agent_config.load_agents(&agents_dir)?")
        .expect("directory load");
    assert!(
        file_load < dir_load,
        "explicit --agent-file should remain the bypass path before named-agent directory lookup"
    );
}

#[test]
fn age_33_deferred_one_shot_agent_file_site_remains_direct_loader_call() {
    let source = include_str!("../src/main.rs");
    let run = source_slice(source, "fn run(cli: Cli)", "fn resolve_agent(");

    assert!(
        run.contains("if let Some(ref model_name) = cli.model"),
        "direct one-shot model branch must remain in run"
    );
    assert!(
        run.contains("if let Some(ref agent_path) = cli.agent_file"),
        "direct one-shot agent-file branch must remain nested under model execution"
    );
    assert!(
        run.contains("load_agent_file(agent_path)?"),
        "deferred site #1 must keep the direct load_agent_file call"
    );
    assert!(
        !run.contains("agent_config.load_agent_file(agent_path)"),
        "deferred site #1 must not be cut over by AGE-33"
    );
}

#[test]
fn age_33_run_with_balancing_opens_state_via_opener_before_config_loads() {
    let source = include_str!("../src/main.rs");
    let run_with_balancing = source_slice(source, "fn run_with_balancing(", "fn run_diagnostics(");

    assert!(
        run_with_balancing.contains("state_db_opener: &dyn StateDbOpener"),
        "run_with_balancing must accept the post-cutover StateDbOpener parameter"
    );
    let open = run_with_balancing
        .find("state_db_opener.open_default()?")
        .expect("state opener call");
    let providers = run_with_balancing
        .find("ProvidersConfig::load(&providers_path).unwrap_or_default()")
        .or_else(|| {
            run_with_balancing
                .find("oulipoly_config::ProvidersConfig::load(&providers_path).unwrap_or_default()")
        })
        .expect("provider config defaulting load");
    let sessions = run_with_balancing
        .find("SessionsConfig::load(&sessions_path).unwrap_or_default()")
        .or_else(|| {
            run_with_balancing
                .find("oulipoly_config::SessionsConfig::load(&sessions_path).unwrap_or_default()")
        })
        .expect("sessions config defaulting load");

    assert!(
        open < providers && open < sessions,
        "state DB open must remain before provider/session config loading"
    );
}

#[test]
fn age_33_diagnostics_and_balancing_config_fallbacks_remain_direct() {
    let source = include_str!("../src/main.rs");
    let run_with_balancing = source_slice(source, "fn run_with_balancing(", "fn run_diagnostics(");
    let run_diagnostics = source_slice(source, "fn run_diagnostics(", "fn run_migrate_db(");

    assert!(
        run_with_balancing.contains("ProvidersConfig::load(&providers_path).unwrap_or_default()")
            || run_with_balancing.contains(
                "oulipoly_config::ProvidersConfig::load(&providers_path).unwrap_or_default()"
            ),
        "run_with_balancing provider loading must keep unwrap_or_default semantics"
    );
    assert!(
        run_with_balancing.contains("SessionsConfig::load(&sessions_path).unwrap_or_default()")
            || run_with_balancing.contains(
                "oulipoly_config::SessionsConfig::load(&sessions_path).unwrap_or_default()"
            ),
        "run_with_balancing sessions loading must keep unwrap_or_default semantics"
    );
    assert!(
        run_diagnostics.contains("load_app_config()"),
        "run_diagnostics must keep its direct app-config load"
    );
    assert!(
        run_diagnostics.contains("ProvidersConfig::load(&providers_path).unwrap_or_default()"),
        "run_diagnostics provider loading must keep unwrap_or_default semantics"
    );
    assert!(
        !run_diagnostics.contains("state_db_opener"),
        "run_diagnostics is out of scope for the AGE-33 state-opener cutover"
    );
}

#[test]
fn age_33_migrate_db_compaction_uses_provider_unaware_model_load_when_models_dir_exists() {
    let fixture = CliFixture::new();
    let _ = fixture.open_db();
    fixture.write_model(
        "codex-overlap",
        "codex",
        &["-c", "sandbox_mode=danger-full-access"],
    );
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        r#"[codex]
command = "codex"
args = ["-c", "sandbox_mode=danger-full-access"]
prompt_mode = "arg"
"#,
    )
    .unwrap();

    let mut cmd = fixture.command();
    cmd.arg("migrate-db");
    let output = cmd.output().unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let out = stdout(&output);
    assert!(out.contains("session chain backfill:"), "{out}");
    assert!(out.contains("compaction backfill:"), "{out}");
    assert!(!stderr(&output).contains("duplicates root [codex].args"));
}

#[test]
fn age_33_migrate_rebuild_discovers_default_state_db_path_from_xdg_data_home() {
    let fixture = CliFixture::new();

    let mut cmd = fixture.command();
    cmd.arg("migrate").arg("--rebuild");
    let output = cmd.output().unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let out = stdout(&output);
    assert!(out.contains("no state.db to rebuild at"), "{out}");
    assert!(
        out.contains(&fixture.db_path().to_string_lossy().to_string()),
        "rebuild should report the StateDb::default_path-derived location: {out}"
    );
    assert!(!fixture.db_path().exists());
}

#[test]
fn age_33_session_export_default_open_operational_error_is_json_without_stdout() {
    let fixture = CliFixture::new();
    let blocked_data_home = fixture.root().join("blocked-data-home");
    fs::write(&blocked_data_home, "not a directory").unwrap();

    let mut cmd = fixture.command();
    cmd.env("XDG_DATA_HOME", &blocked_data_home)
        .arg("session")
        .arg("export")
        .arg(SESSION_A);
    let output = cmd.output().unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let json = stderr_json(&output);
    assert_eq!(json["error"]["code"], "operational-error");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Failed to create state directory"),
        "{json}"
    );
}

#[test]
fn age_33_session_replace_strictly_rejects_malformed_providers_config() {
    let prepared = prepared_claude_replace_fixture();
    fs::write(prepared.fixture.providers_path(), "not = [").unwrap();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "malformed-providers",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = stderr_json(&output);
    assert_eq!(json["error"]["code"], "operational-error");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("TOML parse error"),
        "{json}"
    );
}

#[test]
fn age_33_session_replace_strictly_rejects_malformed_sessions_config() {
    let prepared = prepared_claude_replace_fixture();
    fs::write(prepared.fixture.sessions_path(), "not = [").unwrap();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "malformed-sessions",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = stderr_json(&output);
    assert_eq!(json["error"]["code"], "operational-error");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("TOML parse error"),
        "{json}"
    );
}

#[test]
fn age_33_session_replace_loads_models_with_provider_aware_codex_overlap_validation() {
    let prepared = prepared_codex_replace_fixture();
    fs::write(
        prepared.fixture.models_dir().join(format!("{MODEL}.toml")),
        r#"[[providers]]
name = "codex"
args = ["-c", "sandbox_mode=danger-full-access"]
"#,
    )
    .unwrap();
    fs::write(
        prepared.fixture.providers_path(),
        format!(
            r#"[codex]
command = "provider-command-that-must-not-run"
args = ["-c", "sandbox_mode=danger-full-access"]
interactive_args = ["launch"]
prompt_mode = "arg"

[codex.resume]
kind = "flag"
flag = "--resume"

[codex.session_storage]
kind = "codex"
sessions_dir = {:?}
"#,
            prepared
                .fixture
                .root()
                .join("codex-sessions")
                .to_string_lossy()
        ),
    )
    .unwrap();
    let input = canonical_jsonl(
        &prepared.session_id,
        &prepared.provider_name,
        &prepared.jsonl_path,
        "codex-overlap",
    );

    let output = prepared
        .fixture
        .run_import_replace(&prepared.session_id, &input, &[]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let json = stderr_json(&output);
    assert_eq!(json["error"]["code"], "operational-error");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("duplicates root [codex].args"),
        "{json}"
    );
}

#[test]
fn age_33_tauri_private_helpers_keep_models_dir_parent_config_and_state_policy() {
    let lib_source = include_str!("../src/lib.rs");

    assert!(
        lib_source.contains("fn load_providers_for_models_dir(models_dir: &Path)")
            && lib_source.contains(".parent()")
            && lib_source.contains("join(\"providers.toml\")")
            && lib_source.contains("unwrap_or_default()"),
        "GUI provider loading must keep deriving providers.toml from models_dir.parent()"
    );
    assert!(
        lib_source.contains("fn db_path(&self) -> PathBuf")
            && lib_source.contains("join(\"state.db\")"),
        "GUI state helpers must keep opening state.db beside models_dir.parent()"
    );
    assert!(
        lib_source.contains("MemoryGraph::open(&db_path)")
            && lib_source.contains("state.models_dir")
            && lib_source.contains("join(\"state.db\")"),
        "setup memory command helpers must keep deriving the memory DB beside models_dir"
    );
    let discover_models_cmd = source_slice(
        lib_source,
        "async fn discover_models_cmd(",
        "fn persist_discovery_result(",
    );
    let discovery_opens_gui_db_path = discover_models_cmd
        .contains("let db_path = state.db_path();")
        && discover_models_cmd.contains("state_db_opener.open_at(&db_path)?");
    assert!(
        discovery_opens_gui_db_path,
        "real backend discovery command must keep opening the GUI-derived DB path"
    );
}
