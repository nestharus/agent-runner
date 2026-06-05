#![cfg(unix)]

use chrono::{DateTime, Utc};
use oulipoly_state::{SessionTurnIngest, StateDb};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    agents_dir: PathBuf,
}

impl Fixture {
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
            dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
            agents_dir,
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
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
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

    fn write_model(&self, name: &str, body: &str) {
        fs::write(self.models_dir.join(format!("{name}.toml")), body).unwrap();
    }

    fn write_config(&self, body: &str) {
        fs::write(self.app_config_dir.join("config.toml"), body).unwrap();
    }

    fn write_providers(&self, body: &str) {
        fs::write(self.app_config_dir.join("providers.toml"), body).unwrap();
    }

    fn seed_session_turn(&self, provider_name: &str, session_id: &str) {
        let db = self.open_db();
        db.ingest_session_turns_batch(
            provider_name,
            &[SessionTurnIngest {
                session_id: session_id.to_string(),
                turn_id: "turn-1".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .unwrap();
    }
}

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn read_lines(path: &Path) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn assert_policy_before_user_prompt(prompt: &str, user: &str) {
    let policy_index = prompt
        .find("AGE-28 route policy")
        .unwrap_or_else(|| panic!("missing AGE-28 route policy in prompt:\n{prompt}"));
    let user_index = prompt
        .find(user)
        .unwrap_or_else(|| panic!("missing user prompt {user:?} in prompt:\n{prompt}"));
    assert!(policy_index < user_index, "{prompt}");
}

#[test]
fn oneshot_route_injects_policy_via_effective_provider() {
    let fixture = Fixture::new();
    let claude_argv = fixture.dir.path().join("claude-argv.txt");
    let claude_stdin = fixture.dir.path().join("claude-stdin.txt");
    let codex_argv = fixture.dir.path().join("codex-argv.txt");
    let codex_prompt = fixture.dir.path().join("codex-prompt.txt");
    let claude_script = fixture.write_script(
        "claude-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{claude_argv}"
cat > "{claude_stdin}"
printf 'claude ok\n'"#,
            claude_argv = claude_argv.display(),
            claude_stdin = claude_stdin.display()
        ),
    );
    let codex_script = fixture.write_script(
        "codex-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{codex_argv}"
last="${{@: -1}}"
printf '%s' "$last" > "{codex_prompt}"
printf 'codex ok\n'"#,
            codex_argv = codex_argv.display(),
            codex_prompt = codex_prompt.display()
        ),
    );
    fixture.write_model(
        "claude-opus",
        r#"[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
    );
    fixture.write_model(
        "gpt-high",
        r#"[[providers]]
name = "codex"
args = ["-m", "gpt-5.5"]
"#,
    );
    fixture.write_providers(&format!(
        r#"[claude]
command = {}
args = ["-p"]
prompt_mode = "stdin"
system_prompt_override = "AGE-28 route policy"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task"]

[codex]
command = {}
args = ["exec"]
prompt_mode = "arg"
system_prompt_override = "AGE-28 route policy"

[codex.tool_restrictions]
kind = "codex"
"#,
        toml_string(&claude_script.display().to_string()),
        toml_string(&codex_script.display().to_string())
    ));

    let claude_output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("claude-opus")
        .arg("claude route prompt")
        .output()
        .unwrap();
    assert_eq!(claude_output.status.code(), Some(0), "{claude_output:?}");
    assert_eq!(
        read_lines(&claude_argv),
        vec![
            "-p",
            "--model",
            "opus",
            "--append-system-prompt",
            "AGE-28 route policy",
            "--disallowed-tools",
            "Task",
        ]
    );
    assert_eq!(
        fs::read_to_string(&claude_stdin).unwrap(),
        "claude route prompt"
    );

    let codex_output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("gpt-high")
        .arg("codex route prompt")
        .output()
        .unwrap();
    assert_eq!(codex_output.status.code(), Some(0), "{codex_output:?}");
    assert_eq!(
        read_lines(&codex_argv)[..3],
        ["exec".to_string(), "-m".to_string(), "gpt-5.5".to_string()]
    );
    assert_policy_before_user_prompt(
        &fs::read_to_string(codex_prompt).unwrap(),
        "codex route prompt",
    );
}

#[test]
fn resume_route_injects_policy_via_effective_provider() {
    let fixture = Fixture::new();
    let argv_dump = fixture.dir.path().join("resume-argv.txt");
    let script = fixture.write_script(
        "resume-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{argv_dump}"
printf 'resume ok\n'"#,
            argv_dump = argv_dump.display()
        ),
    );
    fixture.write_model(
        "resumable",
        r#"[[providers]]
name = "claude"
args = ["--model", "opus"]
"#,
    );
    fixture.write_providers(&format!(
        r#"[claude]
command = {}
args = ["-p"]
interactive_args = ["--interactive-root"]
prompt_mode = "arg"
system_prompt_override = "AGE-28 route policy"

[claude.resume]
kind = "flag"
flag = "--resume"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#,
        toml_string(&script.display().to_string())
    ));
    let session_id = "5169694d-de0f-40d1-890c-6e28e55bab27";
    fixture.seed_session_turn("claude", session_id);

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("resumable")
        .arg("--resume")
        .arg(session_id)
        .arg("continue")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        read_lines(&argv_dump),
        vec![
            "-p",
            "--model",
            "opus",
            "--append-system-prompt",
            "AGE-28 route policy",
            "--disallowed-tools",
            "Task",
            "--resume",
            session_id,
            "continue",
        ]
    );
}

#[test]
fn repl_default_provider_route_injects_policy() {
    let fixture = Fixture::new();
    let argv_dump = fixture.dir.path().join("new-argv.txt");
    let script = fixture.write_script(
        "new-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{argv_dump}"
printf 'new ok\n'"#,
            argv_dump = argv_dump.display()
        ),
    );
    fixture.write_config(r#"default_provider = "claude""#);
    fixture.write_providers(&format!(
        r#"[claude]
command = {}
interactive_args = ["--interactive-root"]
system_prompt_override = "AGE-28 route policy"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#,
        toml_string(&script.display().to_string())
    ));

    let output = fixture.command().arg("--new").output().unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        read_lines(&argv_dump),
        vec![
            "--interactive-root",
            "--append-system-prompt",
            "AGE-28 route policy",
            "--disallowed-tools",
            "Task",
        ]
    );
}

#[test]
fn diagnostics_provider_inherits_policy_or_documents_exemption() {
    let fixture = Fixture::new();
    let diag_argv = fixture.dir.path().join("diagnostic-argv.txt");
    let diag_prompt = fixture.dir.path().join("diagnostic-prompt.txt");
    let failure_script = fixture.write_script(
        "failing-provider.sh",
        r#"printf 'opaque failure\n' >&2
exit 7"#,
    );
    let diagnostic_script = fixture.write_script(
        "diagnostic-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{diag_argv}"
cat > "{diag_prompt}"
printf 'network_error\nrouted diagnostic\n'"#,
            diag_argv = diag_argv.display(),
            diag_prompt = diag_prompt.display()
        ),
    );
    fixture.write_model(
        "failing",
        r#"[[providers]]
name = "failing-provider"
"#,
    );
    fixture.write_model(
        "diagnostic",
        r#"[[providers]]
name = "diagnostic-provider"
"#,
    );
    fixture.write_config(r#"diagnostics_model = "diagnostic""#);
    fixture.write_providers(&format!(
        r#"[failing-provider]
command = {}
args = []
prompt_mode = "arg"

[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
system_prompt_override = "AGE-28 route policy"

[diagnostic-provider.tool_restrictions]
kind = "claude"

[diagnostic-provider.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#,
        toml_string(&failure_script.display().to_string()),
        toml_string(&diagnostic_script.display().to_string())
    ));

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("failing")
        .arg("trigger diagnostic")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("[diagnostics: network_error]"),
        "{output:?}"
    );
    assert_eq!(
        read_lines(&diag_argv),
        vec![
            "--append-system-prompt",
            "AGE-28 route policy",
            "--disallowed-tools",
            "Task",
        ]
    );
    assert!(
        fs::read_to_string(diag_prompt)
            .unwrap()
            .contains("opaque failure")
    );
}

#[test]
fn agent_instructions_prompt_prepend_unchanged() {
    let fixture = Fixture::new();
    let argv_dump = fixture.dir.path().join("agent-argv.txt");
    let stdin_dump = fixture.dir.path().join("agent-stdin.txt");
    let script = fixture.write_script(
        "agent-provider.sh",
        &format!(
            r#"printf '%s\n' "$@" > "{argv_dump}"
cat > "{stdin_dump}"
printf 'agent ok\n'"#,
            argv_dump = argv_dump.display(),
            stdin_dump = stdin_dump.display()
        ),
    );
    fixture.write_model(
        "fixture",
        r#"[[providers]]
name = "claude"
"#,
    );
    fs::write(
        fixture.agents_dir.join("writer.md"),
        r#"---
description: Test writer
model: fixture
output_format: text
---
Use terse prose.
"#,
    )
    .unwrap();
    fixture.write_providers(&format!(
        r#"[claude]
command = {}
args = ["-p"]
prompt_mode = "stdin"
system_prompt_override = "AGE-28 route policy"

[claude.tool_restrictions]
kind = "claude"

[claude.tool_restrictions.claude]
disallowed_tools = ["Task"]
"#,
        toml_string(&script.display().to_string())
    ));

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--agents-dir")
        .arg(&fixture.agents_dir)
        .arg("writer")
        .arg("draft")
        .arg("summary")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fs::read_to_string(stdin_dump).unwrap(),
        "Use terse prose.\n\n\ndraft summary"
    );
    assert_eq!(
        read_lines(&argv_dump),
        vec![
            "-p",
            "--append-system-prompt",
            "AGE-28 route policy",
            "--disallowed-tools",
            "Task",
        ]
    );
}

#[test]
fn raw_executor_callsite_allowlist_unchanged_or_intentionally_updated() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let allowlist =
        fs::read_to_string(workspace.join("tests/raw_executor_callsite_allowlist.txt")).unwrap();

    assert_eq!(
        allowlist,
        "# Raw executor facade and implementation internals.\ncrates/oulipoly-runtime/src/executor/mod.rs\ncrates/oulipoly-runtime/src/executor/cli.rs\n\n# Integration and unit tests are allowed to exercise raw executor APIs directly.\nsrc-tauri/tests/age27_raw_executor_callsite_scan.rs\nsrc-tauri/tests/age27_diagnostics_effective_provider.rs\n"
    );
}

#[test]
fn route_fixture_helpers_do_not_spawn_bare_agents() {
    let source = include_str!("age28_provider_policy_routing.rs");
    let forbidden_command = ["Command::new(", "\"agents\")"].concat();
    let forbidden_arg = [".arg(", "\"agents\")"].concat();
    assert!(!source.contains(&forbidden_command));
    assert!(!source.contains(&forbidden_arg));
}
