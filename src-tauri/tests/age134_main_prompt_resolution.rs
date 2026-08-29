#![cfg(unix)]

//! Declared roles: accessor, formatter, mapper, parser, filter,
//! orchestration, validator.

use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
use rusqlite::Connection;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const MODEL: &str = "fixture";
const PROVIDER: &str = "fixture-provider";

struct CliFixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    agents_dir: PathBuf,
    prompt_dump: PathBuf,
    answer_dump: PathBuf,
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
        let prompt_dump = dir.path().join("prompt.txt");
        let answer_dump = dir.path().join("answer.txt");

        let fixture = Self {
            dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
            agents_dir,
            prompt_dump,
            answer_dump,
        };
        fixture.write_prompt_dump_provider();
        fixture
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
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn write_prompt_dump_provider(&self) {
        let provider = self.dir.path().join("prompt-provider.sh");
        write_executable(
            &provider,
            &format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s' "${{@: -1}}" > "{}"
printf 'provider-ok\n'
"#,
                self.prompt_dump.display()
            ),
        );
        self.write_model_provider(&provider);
    }

    fn write_resume_provider(&self, body: &str) {
        let provider = self.dir.path().join("resume-provider.sh");
        write_executable(&provider, body);
        self.write_model_provider(&provider);
    }

    fn write_model_provider(&self, provider: &Path) {
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"[[providers]]
name = "{PROVIDER}"
"#
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
command = "{}"
args = []
interactive_args = []
prompt_mode = "arg"

[{PROVIDER}.resume]
kind = "flag"
flag = "--resume"
"#,
                provider.display()
            ),
        )
        .unwrap();
    }

    fn write_agent(&self) {
        fs::write(
            self.agents_dir.join("writer.md"),
            format!(
                "---\ndescription: writer\nmodel: {MODEL}\noutput_format: text\n---\nUse terse prose.\n"
            ),
        )
        .unwrap();
    }

    fn seed_active_chain(&self) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', ?2)",
            rusqlite::params![CHAIN_ID, MODEL],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            rusqlite::params![CHAIN_ID, PROVIDER, SESSION_ID],
        )
        .unwrap();
    }

    fn run_with_stdin(&self, args: &[&str], stdin: &[u8]) -> Output {
        let mut cmd = self.command();
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        child.stdin.as_mut().unwrap().write_all(stdin).unwrap();
        child.wait_with_output().unwrap()
    }

    fn run_under_pty(&self, args: &[&str]) -> PtyOutput {
        let mut command = shell_quote(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        for arg in args {
            command.push(' ');
            command.push_str(&shell_quote(arg));
        }
        let typescript = self.dir.path().join("pty-output.txt");
        let mut script = Command::new("script");
        script
            .arg("-q")
            .arg("-e")
            .arg("-c")
            .arg(command)
            .arg(&typescript)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env(
                "OULIPOLY_DATA_DIR",
                self.data_home.join("oulipoly-agent-runner"),
            )
            .env("HOME", &self.data_home)
            .env_remove("OULIPOLY_PARENT_INVOCATION");
        let output = script.output().unwrap();
        PtyOutput {
            status_code: output.status.code(),
            text: fs::read_to_string(typescript).unwrap_or_default(),
        }
    }
}

#[derive(Debug)]
struct PtyOutput {
    status_code: Option<i32>,
    text: String,
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', r#"'\''"#))
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn parse_invocations(stderr: &str) -> Vec<CompositeInvocationId> {
    stderr
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .filter_map(|raw| CompositeInvocationId::parse_env_value(raw).ok())
        .collect()
}

fn invocation_count_if_db_exists(fixture: &CliFixture) -> i64 {
    if !fixture.db_path().exists() {
        return 0;
    }
    let conn = Connection::open(fixture.db_path()).unwrap();
    conn.query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

#[test]
fn age134_direct_model_reads_prompt_file_and_missing_file_exits_before_invocation() {
    let fixture = CliFixture::new();
    let prompt_file = fixture.dir.path().join("prompt.md");
    fs::write(&prompt_file, "prompt from file\nsecond line").unwrap();

    let success = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg(MODEL)
        .arg("--file")
        .arg(&prompt_file)
        .output()
        .unwrap();

    assert_eq!(success.status.code(), Some(0), "{success:?}");
    assert_eq!(
        fs::read_to_string(&fixture.prompt_dump).unwrap(),
        "prompt from file\nsecond line"
    );
    let invocation = parse_invocations(&stderr(&success));
    assert_eq!(invocation.len(), 1, "{success:?}");

    let missing_file = fixture.dir.path().join("missing.md");
    let missing = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg(MODEL)
        .arg("--file")
        .arg(&missing_file)
        .output()
        .unwrap();

    assert_eq!(missing.status.code(), Some(1), "{missing:?}");
    assert!(
        stderr(&missing).contains("Failed to read prompt file"),
        "{missing:?}"
    );
    assert_eq!(parse_invocations(&stderr(&missing)).len(), 0, "{missing:?}");
    assert_eq!(invocation_count_if_db_exists(&fixture), 1);
}

#[test]
fn age134_prompt_resolution_errors_for_terminal_and_empty_stdin_before_invocation() {
    let fixture = CliFixture::new();
    fixture.write_agent();

    let direct_terminal = fixture.run_under_pty(&[
        "--models-dir",
        &fixture.models_dir.to_string_lossy(),
        "--model",
        MODEL,
    ]);
    assert_eq!(direct_terminal.status_code, Some(1), "{direct_terminal:?}");
    assert!(
        direct_terminal
            .text
            .contains("No prompt provided. Pass as argument, --file, or pipe to stdin."),
        "{direct_terminal:?}"
    );

    let direct_empty = fixture.run_with_stdin(
        &[
            "--models-dir",
            &fixture.models_dir.to_string_lossy(),
            "--model",
            MODEL,
        ],
        b" \n\t",
    );
    assert_eq!(direct_empty.status.code(), Some(1), "{direct_empty:?}");
    assert!(
        stderr(&direct_empty).contains("Empty prompt from stdin."),
        "{direct_empty:?}"
    );

    let agent_terminal = fixture.run_under_pty(&[
        "--models-dir",
        &fixture.models_dir.to_string_lossy(),
        "--agents-dir",
        &fixture.agents_dir.to_string_lossy(),
        "writer",
    ]);
    assert_eq!(agent_terminal.status_code, Some(1), "{agent_terminal:?}");
    assert!(
        agent_terminal
            .text
            .contains("No prompt provided. Pass as argument, --file, or pipe to stdin."),
        "{agent_terminal:?}"
    );

    let agent_empty = fixture.run_with_stdin(
        &[
            "--models-dir",
            &fixture.models_dir.to_string_lossy(),
            "--agents-dir",
            &fixture.agents_dir.to_string_lossy(),
            "writer",
        ],
        b"\n ",
    );
    assert_eq!(agent_empty.status.code(), Some(1), "{agent_empty:?}");
    assert!(
        stderr(&agent_empty).contains("Empty prompt from stdin."),
        "{agent_empty:?}"
    );
    assert_eq!(invocation_count_if_db_exists(&fixture), 0);
    assert!(!fixture.prompt_dump.exists());
}

#[test]
fn age134_bare_no_agent_path_errors_without_opening_invocation_lifecycle() {
    let fixture = CliFixture::new();

    let output = fixture
        .command()
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        stderr(&output).contains("No agent specified. Use a positional argument or --agent-file."),
        "{output:?}"
    );
    assert_eq!(invocation_count_if_db_exists(&fixture), 0);
    assert!(!fixture.prompt_dump.exists());
}

#[test]
fn age134_resume_answer_reads_piped_stdin_and_allows_terminal_or_empty_stdin_resume() {
    let fixture = CliFixture::new();
    fixture.seed_active_chain();
    fixture.write_resume_provider(&format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s' "${{@: -1}}" > "{}"
printf 'resume-ok\n'
"#,
        fixture.answer_dump.display()
    ));

    let piped = fixture.run_with_stdin(
        &[
            "resume",
            "--session-id",
            SESSION_ID,
            "--models-dir",
            &fixture.models_dir.to_string_lossy(),
            "--model",
            MODEL,
        ],
        b"answer from stdin",
    );
    assert_eq!(piped.status.code(), Some(0), "{piped:?}");
    assert_eq!(
        fs::read_to_string(&fixture.answer_dump).unwrap(),
        "answer from stdin"
    );

    let terminal = fixture.run_under_pty(&[
        "resume",
        "--session-id",
        SESSION_ID,
        "--models-dir",
        &fixture.models_dir.to_string_lossy(),
        "--model",
        MODEL,
    ]);
    assert_eq!(terminal.status_code, Some(0), "{terminal:?}");
    assert!(terminal.text.contains("resume-ok"), "{terminal:?}");
    assert_eq!(
        fs::read_to_string(&fixture.answer_dump).unwrap(),
        SESSION_ID
    );

    let empty = fixture.run_with_stdin(
        &[
            "resume",
            "--session-id",
            SESSION_ID,
            "--models-dir",
            &fixture.models_dir.to_string_lossy(),
            "--model",
            MODEL,
        ],
        b"\n \t",
    );
    assert_eq!(empty.status.code(), Some(0), "{empty:?}");
    assert_eq!(
        fs::read_to_string(&fixture.answer_dump).unwrap(),
        SESSION_ID
    );
}

#[test]
fn age134_resume_mismatch_persists_rejected_acceptance_and_diagnostic_category() {
    let fixture = CliFixture::new();
    fixture.seed_active_chain();
    fixture.write_resume_provider(
        r#"#!/usr/bin/env bash
set -euo pipefail
sid=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--resume" ]; then
    shift
    sid="${1:-}"
    break
  fi
  shift
done
printf 'No conversation found with session ID: %s\n' "$sid" >&2
exit 9
"#,
    );

    let output = fixture
        .command()
        .arg("resume")
        .arg("--session-id")
        .arg(SESSION_ID)
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--model")
        .arg(MODEL)
        .arg("--prompt")
        .arg("resume answer")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(9), "{output:?}");
    let err = stderr(&output);
    assert!(
        err.contains("No conversation found with session ID"),
        "{err}"
    );
    assert!(
        err.contains("[diagnostics: resume_session_mismatch]"),
        "{err}"
    );
    let invocation = parse_invocations(&err);
    assert_eq!(invocation.len(), 1, "{err}");

    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation[0].id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.exit_code, Some(9));
    assert_eq!(
        row.error_category.as_deref(),
        Some("resume_session_mismatch")
    );
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("rejected"));
    assert!(
        row.resume_acceptance_evidence
            .as_deref()
            .unwrap_or_default()
            .contains("resume_session_mismatch"),
        "{row:?}"
    );
}
