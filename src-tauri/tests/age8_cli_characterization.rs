#![cfg(unix)]

// Characterization test for AGE-8 — pins current behavior of runner CLI seams touched by the agents binary refactor.

use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    agents_dir: PathBuf,
    prompt_dump: PathBuf,
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

        let provider_script = dir.path().join("fixture-provider.sh");
        let prompt_dump = dir.path().join("prompt-dump.txt");
        write_executable(
            &provider_script,
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
if [ "$#" -gt 0 ]; then
  printf '%s' "$1" > "{prompt_dump}"
else
  cat > "{prompt_dump}"
fi
printf 'fixture-ok\n'
"#,
                prompt_dump = prompt_dump.display()
            ),
        );

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
"#,
                provider_script.display()
            ),
        )
        .unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
            agents_dir,
            prompt_dump,
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
        cmd
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn parse_invocations(stderr: &str) -> Vec<CompositeInvocationId> {
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .filter_map(|raw| CompositeInvocationId::parse_env_value(raw).ok())
        .collect()
}

fn parse_single_invocation(stderr: &str) -> CompositeInvocationId {
    let invocations = parse_invocations(stderr);
    assert_eq!(
        invocations.len(),
        1,
        "stderr should contain exactly one invocation line: {stderr}"
    );
    invocations.into_iter().next().unwrap()
}

// Characterization test for AGE-8 updated by AGE-32/TI-13 — run_with_balancing must fail closed
// when the persistent state DB cannot open instead of falling back to in-memory state.
#[test]
fn one_shot_fails_closed_when_default_state_db_cannot_open() {
    let fixture = Fixture::new();
    let blocked_data_home = fixture._dir.path().join("blocked-data-home");
    fs::write(&blocked_data_home, "not a directory").unwrap();

    let mut cmd = fixture.command();
    cmd.env("XDG_DATA_HOME", &blocked_data_home);
    cmd.env_remove("OULIPOLY_DATA_DIR");
    cmd.arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("fixture")
        .arg("ping");

    let output = cmd.output().unwrap();

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to create state directory"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Not a directory"),
        "state DB open failure should preserve actionable OS cause: {stderr}"
    );
    assert_eq!(parse_invocations(&stderr).len(), 0, "{stderr}");
    assert!(
        !blocked_data_home.join("oulipoly-agent-runner").exists(),
        "failed run should not create durable state below the blocked data-home path"
    );
    assert!(
        !fixture.prompt_dump.exists(),
        "provider must not execute after state DB open failure"
    );
}

// Characterization test for AGE-8 — pins model-backed diagnostics from a failed run.
#[test]
fn failed_one_shot_loads_app_config_invokes_diagnostic_model_and_persists_category() {
    let fixture = Fixture::new();
    let failure_script = fixture._dir.path().join("failure-provider.sh");
    write_executable(
        &failure_script,
        r#"#!/usr/bin/env bash
set -euo pipefail
printf 'opaque child failure\n' >&2
exit 7
"#,
    );
    let diag_prompt_dump = fixture._dir.path().join("diagnostic-prompt.txt");
    let diag_script = fixture._dir.path().join("diagnostic-provider.sh");
    write_executable(
        &diag_script,
        &format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
cat > "{diag_prompt_dump}"
printf 'network_error\nDiagnostic model saw network trouble\n'
"#,
            diag_prompt_dump = diag_prompt_dump.display()
        ),
    );

    fs::write(
        fixture.models_dir.join("failing.toml"),
        r#"[[providers]]
name = "failure-provider"
"#,
    )
    .unwrap();
    fs::write(
        fixture.models_dir.join("diagnostic.toml"),
        r#"[[providers]]
name = "diagnostic-provider"
"#,
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        format!(
            r#"[failure-provider]
command = "{}"
args = []
prompt_mode = "arg"

[diagnostic-provider]
command = "{}"
args = []
prompt_mode = "stdin"
"#,
            failure_script.display(),
            diag_script.display()
        ),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("config.toml"),
        r#"diagnostics_model = "diagnostic"
"#,
    )
    .unwrap();

    let mut cmd = fixture.command();
    cmd.arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("failing")
        .arg("trigger failure");

    let output = cmd.output().unwrap();

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[diagnostics] network_error: Diagnostic model saw network trouble"),
        "{stderr}"
    );
    assert!(stderr.contains("[diagnostics: network_error]"), "{stderr}");

    let diag_prompt = fs::read_to_string(diag_prompt_dump).unwrap();
    assert!(diag_prompt.contains("Exit code: 7"), "{diag_prompt}");
    assert!(
        diag_prompt.contains("opaque child failure"),
        "{diag_prompt}"
    );

    let invocation = parse_single_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category.as_deref(), Some("network_error"));
}

// Characterization test for AGE-8 — pins current behavior of stdin prompt read path.
#[test]
fn model_execution_reads_prompt_from_piped_stdin_when_no_file_or_positional_prompt_exists() {
    let fixture = Fixture::new();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        format!(
            r#"[fixture-provider]
command = "{}"
args = []
prompt_mode = "stdin"
"#,
            fixture._dir.path().join("fixture-provider.sh").display()
        ),
    )
    .unwrap();

    let mut cmd = fixture.command();
    cmd.arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg("fixture")
        .stdin(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"prompt from stdin")
        .unwrap();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&fixture.prompt_dump).unwrap(),
        "prompt from stdin"
    );
}

// Characterization test for AGE-8 — pins current behavior of missing models-dir CLI surface.
#[test]
fn missing_models_dir_loads_as_empty_model_set_and_reports_unknown_model_without_invocation_line() {
    let fixture = Fixture::new();
    let missing_models_dir = fixture._dir.path().join("missing-models");
    let mut cmd = fixture.command();
    cmd.arg("--models-dir")
        .arg(&missing_models_dir)
        .arg("--model")
        .arg("fixture")
        .arg("prompt");

    let output = cmd.output().unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Error: Unknown model: fixture"), "{stderr}");
    assert!(
        parse_invocations(&stderr).is_empty(),
        "model-load failure should not start an invocation: {stderr}"
    );
}

// Characterization test for AGE-8 — pins current behavior of named agent execution through run.
#[test]
fn named_agent_execution_prepends_loaded_agent_instructions_to_prompt() {
    let fixture = Fixture::new();
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

    let mut cmd = fixture.command();
    cmd.arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--agents-dir")
        .arg(&fixture.agents_dir)
        .arg("writer")
        .arg("draft")
        .arg("summary");

    let output = cmd.output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&fixture.prompt_dump).unwrap(),
        "Use terse prose.\n\n\ndraft summary"
    );
}

#[test]
fn defined_feature_orchestrator_agent_file_uses_frontmatter_model_identity() {
    let fixture = Fixture::new();
    let agent_file = fixture._dir.path().join("feature-orchestrator.md");
    let prompt_file = fixture._dir.path().join("feature-orchestrator-prompt.md");
    fs::write(
        fixture.models_dir.join("gpt-xhigh.toml"),
        r#"[[providers]]
name = "fixture-provider"
"#,
    )
    .unwrap();
    fs::write(
        &agent_file,
        r#"---
description: Feature orchestrator
model: gpt-xhigh
output_format: text
---
Coordinate the defined feature workflow.
"#,
    )
    .unwrap();
    fs::write(&prompt_file, "Execute the supplied feature handoff.").unwrap();

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("-a")
        .arg(&agent_file)
        .arg("-p")
        .arg(fixture._dir.path())
        .arg("-f")
        .arg(&prompt_file)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&fixture.prompt_dump).unwrap(),
        "Coordinate the defined feature workflow.\n\n\nExecute the supplied feature handoff."
    );
    let invocation = parse_single_invocation(&String::from_utf8_lossy(&output.stderr));
    let record = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(record.model_name, "gpt-xhigh");
    assert_eq!(record.provider_name.as_deref(), Some("fixture-provider"));
}

// Characterization test for AGE-194 — pins current behavior of named-agent execution
// when the named agent's `model:` references a model not present in `--models-dir`.
#[test]
fn named_agent_with_unknown_model_emits_unknown_model_referenced_by_agent_stderr() {
    let fixture = Fixture::new();
    fs::write(
        fixture.agents_dir.join("writer.md"),
        r#"---
description: Test writer
model: missing-model
output_format: text
---
"#,
    )
    .unwrap();

    let mut cmd = fixture.command();
    cmd.arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--agents-dir")
        .arg(&fixture.agents_dir)
        .arg("writer")
        .arg("hello");

    let output = cmd.output().unwrap();

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("Unknown model 'missing-model' referenced by agent 'writer'"),
        "expected named-agent unknown-model stderr; got: {stderr}"
    );
}

// Characterization test for AGE-8 — pins current behavior of --agent-file execution through run.
#[test]
fn agent_file_execution_uses_prompt_args_after_the_first_positional_slot() {
    let fixture = Fixture::new();
    let agent_file = fixture._dir.path().join("file-agent.md");
    fs::write(
        &agent_file,
        r#"---
description: File agent
model: fixture
output_format: text
---
Follow file-agent rules.
"#,
    )
    .unwrap();

    let mut cmd = fixture.command();
    cmd.arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--agent-file")
        .arg(&agent_file)
        .arg("first-positional-is-not-prompt")
        .arg("actual prompt");

    let output = cmd.output().unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(&fixture.prompt_dump).unwrap(),
        "Follow file-agent rules.\n\n\nactual prompt"
    );
}
