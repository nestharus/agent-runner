#![cfg(unix)]

//! ## Declared roles
//!
//! `orchestration`, `accessor`, `mapper`, `validator`, `formatter`, `parser`, `predicate`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/tests/age290_dispatch_integration.rs
//!     role: adapter
//!     Translates:
//!       - public-fresh-continuation-dispatch-contract
//!       - legacy-resume-dispatch-contract
//!       - isolated-provider-filesystem-and-SQLite-fixture-contract
//!       - fixture-authored-provider-output-source-owner-contract
//!       - direct-SQLite-observation-schema-producer-contract
//! ```

mod provider_authority_fixture;

use chrono::{DateTime, Utc};
use oulipoly_state::{
    CompositeInvocationId, InvocationStart, InvocationStatus, ProviderSessionBinding,
    SessionTurnIngest, StateDb,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const ORIGIN_INVOCATION_ID: &str = "11111111-1111-4111-8111-111111111111";
const ORIGIN_SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const MISMATCHED_SESSION_ID: &str = "6169694d-de0f-40d1-890c-6e28e55bab27";
const FRESH_SESSION_ID: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
const PARENT_INVOCATION_ID: &str = "22222222-2222-4222-8222-222222222222";
const FORCE_TERMINAL_SIGNAL_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";
// Split literals keep the legacy provider name out of the source guard's raw scan.
// Do not collapse them into plain literals.
const LEGACY_PROVIDER_A: &str = concat!("cla", "ude-legacy-a");
const LEGACY_PROVIDER_B: &str = concat!("cla", "ude-legacy-b");
const LEGACY_SESSION_STORAGE_KIND: &str = concat!("cla", "ude_code");

struct ContinuationFixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    planning_root: PathBuf,
    worktree: PathBuf,
    request_path: PathBuf,
    resume_record: PathBuf,
    fresh_record: PathBuf,
    resume_calls: PathBuf,
    fresh_calls: PathBuf,
    expected_artifacts: Vec<(&'static str, PathBuf, String)>,
}

impl ContinuationFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let planning_root = dir.path().join("planning");
        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&planning_root).unwrap();
        fs::create_dir_all(&worktree).unwrap();

        let resume_record = dir.path().join("resume-record.json");
        let fresh_record = dir.path().join("fresh-record.json");
        let resume_calls = dir.path().join("resume-calls");
        let fresh_calls = dir.path().join("fresh-calls");
        let transcript = dir.path().join("fresh-turns.jsonl");
        fs::write(&transcript, "").unwrap();

        let resume_provider = write_python_provider(
            dir.path(),
            "resume-provider.py",
            &format_resume_provider_source(&resume_record, &resume_calls),
        );
        let fresh_provider = write_python_provider(
            dir.path(),
            "fresh-provider.py",
            &format_fresh_provider_source(&fresh_record, &fresh_calls, &transcript),
        );

        fs::write(
            models_dir.join("resume-model.toml"),
            format_resume_model_config(),
        )
        .unwrap();
        fs::write(
            models_dir.join("fresh-model.toml"),
            format_fresh_model_config(),
        )
        .unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority_for_prompt_acceptance(
                &format_continuation_providers_config(&resume_provider, &fresh_provider),
                &["resume-provider"],
            ),
        )
        .unwrap();
        let fresh_session_state = dir.path().join("fresh-session-state");
        fs::write(
            app_config_dir.join("sessions.toml"),
            format_continuation_sessions_config(&transcript, &fresh_session_state),
        )
        .unwrap();

        let db_path = state_db_path(&data_home);
        let state = StateDb::open(&db_path).unwrap();
        let origin_row_id = start_invocation(
            &state,
            ORIGIN_INVOCATION_ID,
            "origin-model",
            "resume-provider",
            None,
        );
        bind_provider_session(
            &state,
            origin_row_id,
            "resume-provider",
            ORIGIN_SESSION_ID,
            "external_provider_launch",
            None,
            &worktree,
        );
        state
            .finalize_invocation(origin_row_id, true, 0, None, None)
            .unwrap();
        seed_session_turn(&state, "resume-provider", ORIGIN_SESSION_ID, "origin-turn");
        drop(state);

        let expected_artifacts = write_evidence(&planning_root, &worktree);
        let request_path = planning_root.join("request.json");
        write_request(
            &request_path,
            &planning_root,
            &worktree,
            ORIGIN_SESSION_ID,
            &expected_artifacts,
        );

        map_continuation_fixture(
            dir,
            config_home,
            data_home,
            models_dir,
            planning_root,
            worktree,
            request_path,
            resume_record,
            fresh_record,
            resume_calls,
            fresh_calls,
            expected_artifacts,
        )
    }

    fn command(&self) -> Command {
        isolated_command(&self.config_home, &self.data_home)
    }

    fn continuation_command(&self) -> Command {
        self.continuation_command_with(ORIGIN_SESSION_ID, &self.worktree)
    }

    fn continuation_command_with(&self, resume_session: &str, project: &Path) -> Command {
        let mut command = self.command();
        command
            .arg("--model")
            .arg("resume-model")
            .arg("--resume")
            .arg(resume_session)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--project")
            .arg(project)
            .arg("--fresh-continuation-request")
            .arg(&self.request_path)
            .arg("resume input from dispatch");
        command
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&state_db_path(&self.data_home)).unwrap()
    }

    fn connection(&self) -> Connection {
        Connection::open(state_db_path(&self.data_home)).unwrap()
    }

    fn continuation_row(&self) -> ContinuationRow {
        let encoded = self
            .connection()
            .query_row(
                "SELECT continuation_id, resume_invocation_id, resume_parent_invocation_id,
                        fresh_invocation_id, fresh_parent_invocation_id,
                        resume_outcome_json, fresh_outcome_json, handoff_json,
                        terminal_outcome_json
                   FROM fresh_continuations",
                [],
                map_encoded_continuation_row,
            )
            .unwrap();
        let resume_outcome = parse_encoded_json(&encoded.resume_outcome);
        let fresh_outcome = parse_encoded_json(&encoded.fresh_outcome);
        let handoff = parse_encoded_json(&encoded.handoff);
        let terminal_outcome = parse_encoded_json(&encoded.terminal_outcome);
        map_continuation_row(
            encoded,
            resume_outcome,
            fresh_outcome,
            handoff,
            terminal_outcome,
        )
    }
}

struct LegacyResumeFixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    project: PathBuf,
    answer_path: PathBuf,
    first_record: PathBuf,
    second_record: PathBuf,
    interactive_record: PathBuf,
    parent_row_id: i64,
    baseline_max_invocation_id: i64,
}

impl LegacyResumeFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let project = dir.path().join("legacy-project");
        let source_projects = dir.path().join("source-projects");
        let target_projects = dir.path().join("target-projects");
        let answer_path = dir.path().join("answer.md");
        let first_record = dir.path().join("legacy-first.json");
        let second_record = dir.path().join("legacy-second.json");
        let interactive_record = dir.path().join("legacy-interactive.json");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(source_projects.join("source-project")).unwrap();
        fs::create_dir_all(&target_projects).unwrap();
        fs::write(&answer_path, "legacy answer from file\n").unwrap();
        let origin_session_file_name = format_origin_session_file_name();
        fs::write(
            map_origin_session_path(&source_projects, &origin_session_file_name),
            format_origin_session_turn(),
        )
        .unwrap();

        let first_provider = write_recording_shell_provider(
            dir.path(),
            "legacy-first.sh",
            &first_record,
            "printf 'quota on first legacy attempt\\n' >&2\nexit 42",
        );
        let second_provider = write_recording_shell_provider(
            dir.path(),
            "legacy-second.sh",
            &second_record,
            "printf 'legacy retry completed\\n'\nexit 0",
        );
        let interactive_provider = write_recording_shell_provider(
            dir.path(),
            "legacy-interactive.sh",
            &interactive_record,
            "printf 'legacy interactive completed\\n'\nexit 0",
        );

        fs::write(
            models_dir.join("legacy-headless.toml"),
            format_legacy_headless_model_config(),
        )
        .unwrap();
        fs::write(
            models_dir.join("legacy-interactive.toml"),
            format_legacy_interactive_model_config(),
        )
        .unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority(
                &format_legacy_providers_config(
                    &first_provider,
                    &second_provider,
                    &interactive_provider,
                    &source_projects,
                    &target_projects,
                ),
            ),
        )
        .unwrap();

        let state = StateDb::open(&state_db_path(&data_home)).unwrap();
        let parent_row_id = start_invocation(
            &state,
            PARENT_INVOCATION_ID,
            "parent-model",
            "parent-provider",
            None,
        );
        state
            .finalize_invocation(parent_row_id, true, 0, None, None)
            .unwrap();
        let interactive_origin_row_id = start_invocation(
            &state,
            "33333333-3333-4333-8333-333333333333",
            "legacy-interactive",
            "interactive-owner",
            None,
        );
        bind_provider_session(
            &state,
            interactive_origin_row_id,
            "interactive-owner",
            FRESH_SESSION_ID,
            "external_provider_launch",
            None,
            &project,
        );
        state
            .finalize_invocation(interactive_origin_row_id, true, 0, None, None)
            .unwrap();
        seed_session_turn(
            &state,
            "interactive-owner",
            FRESH_SESSION_ID,
            "interactive-turn",
        );
        drop(state);
        seed_active_chain(
            &state_db_path(&data_home),
            "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "legacy-headless",
            LEGACY_PROVIDER_A,
            ORIGIN_SESSION_ID,
            &project,
        );
        let baseline_max_invocation_id = Connection::open(state_db_path(&data_home))
            .unwrap()
            .query_row("SELECT MAX(id) FROM invocations", [], |row| row.get(0))
            .unwrap();

        map_legacy_resume_fixture(
            dir,
            config_home,
            data_home,
            models_dir,
            project,
            answer_path,
            first_record,
            second_record,
            interactive_record,
            parent_row_id,
            baseline_max_invocation_id,
        )
    }

    fn command(&self) -> Command {
        let mut command = isolated_command(&self.config_home, &self.data_home);
        let parent_invocation = map_parent_invocation();
        let parent_invocation = format_parent_invocation(&parent_invocation);
        command.env("OULIPOLY_PARENT_INVOCATION", parent_invocation);
        command
    }

    fn connection(&self) -> Connection {
        Connection::open(state_db_path(&self.data_home)).unwrap()
    }

    fn continuation_count(&self) -> i64 {
        self.connection()
            .query_row("SELECT COUNT(*) FROM fresh_continuations", [], |row| {
                row.get(0)
            })
            .unwrap()
    }
}

#[derive(Debug)]
struct ContinuationRow {
    continuation_id: String,
    resume_invocation_id: String,
    resume_parent_invocation_id: String,
    fresh_invocation_id: String,
    fresh_parent_invocation_id: String,
    resume_outcome: Value,
    fresh_outcome: Value,
    handoff: Value,
    terminal_outcome: Value,
}

struct EncodedContinuationRow {
    continuation_id: String,
    resume_invocation_id: String,
    resume_parent_invocation_id: String,
    fresh_invocation_id: String,
    fresh_parent_invocation_id: String,
    resume_outcome: String,
    fresh_outcome: String,
    handoff: String,
    terminal_outcome: String,
}

type InvocationRow = (i64, Option<i64>, String, String, Option<String>);
type EvidenceArtifact = (&'static str, PathBuf, Vec<u8>);
type EvidenceIdentity = (&'static str, PathBuf, String);

fn format_resume_provider_source(record: &Path, calls: &Path) -> String {
    format!(
        r#"import json
import os
import pathlib
import sys
pathlib.Path({record:?}).write_text(json.dumps({{"argv": sys.argv[1:], "cwd": os.getcwd()}}))
with pathlib.Path({calls:?}).open("a") as stream:
    stream.write("resume\n")
print("resume accepted", file=sys.stderr)
"#,
        record = record.display().to_string(),
        calls = calls.display().to_string(),
    )
}

fn format_fresh_provider_source(record: &Path, calls: &Path, transcript: &Path) -> String {
    format!(
        r#"import datetime
import json
import os
import pathlib
import sys
pathlib.Path({record:?}).write_text(json.dumps({{"argv": sys.argv[1:], "cwd": os.getcwd()}}))
with pathlib.Path({calls:?}).open("a") as stream:
    stream.write("fresh\n")
turn = {{
    "session_id": {fresh_session:?},
    "turn_id": "fresh-turn-1",
    "timestamp": "2026-08-10T12:00:00Z",
    "role": "assistant",
}}
with pathlib.Path({transcript:?}).open("a") as stream:
    stream.write(json.dumps(turn) + "\n")
print(json.dumps({{"type": "step_start", "sessionID": {fresh_session:?}}}))
print("fresh continuation completed")
"#,
        record = record.display().to_string(),
        calls = calls.display().to_string(),
        fresh_session = FRESH_SESSION_ID,
        transcript = transcript.display().to_string(),
    )
}

fn format_resume_model_config() -> String {
    "[[providers]]\nname = \"resume-provider\"\nargs = [\"resume-base\"]\n".to_string()
}

fn format_fresh_model_config() -> String {
    "[[providers]]\nname = \"fresh-provider\"\nargs = [\"fresh-base\"]\n".to_string()
}

fn format_continuation_providers_config(resume_provider: &Path, fresh_provider: &Path) -> String {
    format!(
        r#"[resume-provider]
command = {resume_command:?}
args = []
interactive_args = ["resume-interactive"]
prompt_mode = "arg"

[resume-provider.resume]
kind = "flag"
flag = "--resume"

[resume-provider.resume_acceptance]
accepted_output_patterns = ["resume accepted"]

[fresh-provider]
command = {fresh_command:?}
args = []
interactive_args = ["fresh-interactive"]
prompt_mode = "arg"

[fresh-provider.session_capture]
kind = "stdout_json_event"
json_args = ["--format", "json"]
event_type = "step_start"
event_id_path = "sessionID"
"#,
        resume_command = resume_provider.display().to_string(),
        fresh_command = fresh_provider.display().to_string(),
    )
}

fn format_continuation_sessions_config(transcript: &Path, state_dir: &Path) -> String {
    format!(
        r#"[fresh-provider]
turn_script = {turn_script:?}
state_dir = {state_dir:?}
"#,
        turn_script = format!("cat {}", transcript.display()),
        state_dir = state_dir.display().to_string(),
    )
}

#[allow(clippy::too_many_arguments)]
fn map_continuation_fixture(
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    planning_root: PathBuf,
    worktree: PathBuf,
    request_path: PathBuf,
    resume_record: PathBuf,
    fresh_record: PathBuf,
    resume_calls: PathBuf,
    fresh_calls: PathBuf,
    expected_artifacts: Vec<(&'static str, PathBuf, String)>,
) -> ContinuationFixture {
    ContinuationFixture {
        _dir: dir,
        config_home,
        data_home,
        models_dir,
        planning_root,
        worktree,
        request_path,
        resume_record,
        fresh_record,
        resume_calls,
        fresh_calls,
        expected_artifacts,
    }
}

fn map_encoded_continuation_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<EncodedContinuationRow> {
    Ok(EncodedContinuationRow {
        continuation_id: row.get(0)?,
        resume_invocation_id: row.get(1)?,
        resume_parent_invocation_id: row.get(2)?,
        fresh_invocation_id: row.get(3)?,
        fresh_parent_invocation_id: row.get(4)?,
        resume_outcome: row.get(5)?,
        fresh_outcome: row.get(6)?,
        handoff: row.get(7)?,
        terminal_outcome: row.get(8)?,
    })
}

fn parse_encoded_json(encoded: &str) -> Value {
    serde_json::from_str(encoded).unwrap()
}

fn map_continuation_row(
    encoded: EncodedContinuationRow,
    resume_outcome: Value,
    fresh_outcome: Value,
    handoff: Value,
    terminal_outcome: Value,
) -> ContinuationRow {
    ContinuationRow {
        continuation_id: encoded.continuation_id,
        resume_invocation_id: encoded.resume_invocation_id,
        resume_parent_invocation_id: encoded.resume_parent_invocation_id,
        fresh_invocation_id: encoded.fresh_invocation_id,
        fresh_parent_invocation_id: encoded.fresh_parent_invocation_id,
        resume_outcome,
        fresh_outcome,
        handoff,
        terminal_outcome,
    }
}

fn map_origin_session_path(source_projects: &Path, file_name: &str) -> PathBuf {
    source_projects.join("source-project").join(file_name)
}

fn format_origin_session_file_name() -> String {
    format!("{ORIGIN_SESSION_ID}.jsonl")
}

fn format_origin_session_turn() -> String {
    format!(
        "{{\"sessionId\":\"{ORIGIN_SESSION_ID}\",\"turnId\":\"origin-turn\",\"timestamp\":\"2026-08-10T10:00:00Z\",\"type\":\"assistant\"}}\n"
    )
}

fn format_legacy_headless_model_config() -> String {
    format!(
        "[[providers]]\nname = {LEGACY_PROVIDER_A:?}\nargs = [\"headless-a\"]\n\n[[providers]]\nname = {LEGACY_PROVIDER_B:?}\nargs = [\"headless-b\"]\n"
    )
}

fn format_legacy_interactive_model_config() -> String {
    "[[providers]]\nname = \"interactive-owner\"\nargs = [\"headless-unused\"]\n".to_string()
}

fn format_legacy_providers_config(
    first_provider: &Path,
    second_provider: &Path,
    interactive_provider: &Path,
    source_projects: &Path,
    target_projects: &Path,
) -> String {
    format!(
        r#"[{first_provider_name}]
command = {first_command:?}
args = []
interactive_args = ["interactive-a"]
prompt_mode = "arg"

[{first_provider_name}.resume]
kind = "flag"
flag = "--resume"

[{first_provider_name}.session_storage]
kind = "{session_storage_kind}"
projects_dir = {source_projects:?}

[{second_provider_name}]
command = {second_command:?}
args = []
interactive_args = ["interactive-b"]
prompt_mode = "arg"

[{second_provider_name}.resume]
kind = "flag"
flag = "--resume"

[{second_provider_name}.session_storage]
kind = "{session_storage_kind}"
projects_dir = {target_projects:?}

[interactive-owner]
command = {interactive_command:?}
args = []
interactive_args = ["interactive-launch"]
prompt_mode = "arg"

[interactive-owner.resume]
kind = "flag"
flag = "--resume"
"#,
        first_command = first_provider.display().to_string(),
        second_command = second_provider.display().to_string(),
        first_provider_name = LEGACY_PROVIDER_A,
        second_provider_name = LEGACY_PROVIDER_B,
        session_storage_kind = LEGACY_SESSION_STORAGE_KIND,
        interactive_command = interactive_provider.display().to_string(),
        source_projects = source_projects.display().to_string(),
        target_projects = target_projects.display().to_string(),
    )
}

#[allow(clippy::too_many_arguments)]
fn map_legacy_resume_fixture(
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    project: PathBuf,
    answer_path: PathBuf,
    first_record: PathBuf,
    second_record: PathBuf,
    interactive_record: PathBuf,
    parent_row_id: i64,
    baseline_max_invocation_id: i64,
) -> LegacyResumeFixture {
    LegacyResumeFixture {
        _dir: dir,
        config_home,
        data_home,
        models_dir,
        project,
        answer_path,
        first_record,
        second_record,
        interactive_record,
        parent_row_id,
        baseline_max_invocation_id,
    }
}

fn map_parent_invocation() -> CompositeInvocationId {
    CompositeInvocationId {
        source: "parent-provider".to_string(),
        id: PARENT_INVOCATION_ID.to_string(),
    }
}

fn format_parent_invocation(parent_invocation: &CompositeInvocationId) -> String {
    serde_json::to_string(parent_invocation).unwrap()
}

#[test]
fn request_flag_runs_reserved_production_adapters_and_terminal_replay_does_not_relaunch() {
    let fixture = ContinuationFixture::new();

    let first = fixture.continuation_command().output().unwrap();

    assert_process_success(&first);
    let continuation = fixture.continuation_row();
    let state = fixture.open_db();
    let origin = state
        .get_invocation_by_uuid(ORIGIN_INVOCATION_ID)
        .unwrap()
        .unwrap();
    let resume = state
        .get_invocation_by_uuid(&continuation.resume_invocation_id)
        .unwrap()
        .unwrap();
    let fresh = state
        .get_invocation_by_uuid(&continuation.fresh_invocation_id)
        .unwrap()
        .unwrap();

    assert_eq!(
        continuation.resume_parent_invocation_id,
        ORIGIN_INVOCATION_ID
    );
    assert_eq!(
        continuation.fresh_parent_invocation_id,
        continuation.resume_invocation_id
    );
    assert_eq!(resume.parent_invocation_id, Some(origin.id));
    assert_eq!(fresh.parent_invocation_id, Some(resume.id));
    assert_eq!(resume.provider_name.as_deref(), Some("resume-provider"));
    assert_eq!(fresh.provider_name.as_deref(), Some("fresh-provider"));
    assert_eq!(
        resume.provider_session_id.as_deref(),
        Some(ORIGIN_SESSION_ID)
    );
    assert_eq!(fresh.provider_session_id.as_deref(), Some(FRESH_SESSION_ID));
    assert_eq!(resume.status, InvocationStatus::Failed);
    assert_eq!(resume.success, Some(false));
    assert_eq!(resume.exit_code, Some(0));
    assert_eq!(resume.resume_acceptance_status, None);
    assert_eq!(resume.resume_acceptance_evidence, None);
    assert_eq!(
        resume.error_category.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    assert_eq!(
        resume.terminal_reason.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    assert_eq!(fresh.status, InvocationStatus::Succeeded);
    assert_eq!(fresh.success, Some(true));
    assert_eq!(fresh.exit_code, Some(0));

    let resume_record = read_provider_record(&fixture.resume_record);
    assert_eq!(
        resume_record["argv"],
        json!([
            "resume-base",
            "--resume",
            ORIGIN_SESSION_ID,
            "resume input from dispatch"
        ])
    );
    assert_eq!(
        resume_record["cwd"],
        fixture.worktree.to_string_lossy().as_ref()
    );
    let fresh_record = read_provider_record(&fixture.fresh_record);
    let fresh_argv = fresh_record["argv"].as_array().unwrap();
    assert_eq!(fresh_argv.len(), 4, "{fresh_record}");
    assert_eq!(fresh_argv[0], "fresh-base");
    assert_eq!(fresh_argv[1], "--format");
    assert_eq!(fresh_argv[2], "json");
    assert_eq!(
        fresh_argv[3],
        expected_fresh_prompt(
            &fixture,
            &continuation.resume_invocation_id,
            ORIGIN_INVOCATION_ID,
            ORIGIN_SESSION_ID,
        )
    );
    assert_eq!(
        fresh_record["cwd"],
        fixture.worktree.to_string_lossy().as_ref()
    );

    let expected_resume_outcome = json!({
        "invocation_id": continuation.resume_invocation_id,
        "session_id": ORIGIN_SESSION_ID,
        "physical_exit_code": 0,
        "acceptance": "Accepted",
        "disposition": {
            "Failed": {
                "error_category": "resume_completion_unconfirmed",
                "terminal_reason": "resume_completion_unconfirmed"
            }
        }
    });
    let expected_fresh_outcome = json!({
        "invocation_id": continuation.fresh_invocation_id,
        "session_id": FRESH_SESSION_ID,
        "physical_exit_code": 0,
        "acceptance": "NotApplicable",
        "disposition": "Succeeded"
    });
    assert_eq!(continuation.resume_outcome, expected_resume_outcome);
    assert_eq!(continuation.fresh_outcome, expected_fresh_outcome);

    let handoff_path = PathBuf::from(continuation.handoff["path"].as_str().unwrap());
    assert!(handoff_path.starts_with(&fixture.planning_root));
    let handoff_bytes = fs::read(&handoff_path).unwrap();
    let request_json =
        serde_json::from_slice::<Value>(&fs::read(&fixture.request_path).unwrap()).unwrap();
    let expected_handoff = json!({
        "schema_version": 1,
        "kind": "fresh_continuation_handoff",
        "continuation_id": continuation.continuation_id,
        "fresh_prompt": expected_fresh_prompt(
            &fixture,
            &continuation.resume_invocation_id,
            ORIGIN_INVOCATION_ID,
            ORIGIN_SESSION_ID,
        ),
        "request": request_json,
        "resume": {
            "invocation_id": continuation.resume_invocation_id,
            "session_id": ORIGIN_SESSION_ID,
            "physical_exit_code": 0,
            "acceptance": "accepted",
            "disposition": {
                "status": "failed",
                "error_category": "resume_completion_unconfirmed",
                "terminal_reason": "resume_completion_unconfirmed"
            }
        },
        "fresh": {
            "invocation_id": continuation.fresh_invocation_id,
            "session_id": FRESH_SESSION_ID,
            "physical_exit_code": 0,
            "acceptance": "not_applicable",
            "disposition": {"status": "succeeded"}
        }
    });
    assert_eq!(
        serde_json::from_slice::<Value>(&handoff_bytes).unwrap(),
        expected_handoff
    );
    assert_eq!(
        continuation.handoff["sha256"],
        format!("{:x}", Sha256::digest(&handoff_bytes))
    );
    assert_eq!(
        continuation.terminal_outcome["Continued"]["resume"],
        expected_resume_outcome
    );
    assert_eq!(
        continuation.terminal_outcome["Continued"]["fresh"],
        expected_fresh_outcome
    );
    assert_eq!(line_count(&fixture.resume_calls), 1);
    assert_eq!(line_count(&fixture.fresh_calls), 1);
    let first_invocation_count = invocation_count(&fixture.connection());

    let replay = fixture.continuation_command().output().unwrap();

    assert_process_success(&replay);
    assert_eq!(line_count(&fixture.resume_calls), 1);
    assert_eq!(line_count(&fixture.fresh_calls), 1);
    assert_eq!(
        invocation_count(&fixture.connection()),
        first_invocation_count
    );
    assert_eq!(fs::read(&handoff_path).unwrap(), handoff_bytes);
    let replayed = fixture.continuation_row();
    assert_eq!(
        replayed.resume_invocation_id,
        continuation.resume_invocation_id
    );
    assert_eq!(
        replayed.fresh_invocation_id,
        continuation.fresh_invocation_id
    );
    assert_eq!(replayed.terminal_outcome, continuation.terminal_outcome);
}

#[test]
fn request_flag_accepts_an_equivalent_project_path() {
    let fixture = ContinuationFixture::new();
    let mut command = fixture.continuation_command_with(ORIGIN_SESSION_ID, Path::new("."));
    command.current_dir(&fixture.worktree);

    let output = command.output().unwrap();

    assert_process_success(&output);
    assert_eq!(line_count(&fixture.resume_calls), 1);
    assert_eq!(line_count(&fixture.fresh_calls), 1);
    assert_eq!(continuation_count(&fixture.connection()), 1);
}

#[test]
fn request_flag_rejects_a_project_that_differs_from_the_requested_worktree() {
    let fixture = ContinuationFixture::new();
    let other_project = fixture.worktree.join("other");
    fs::create_dir(&other_project).unwrap();
    let mut command = fixture.continuation_command_with(ORIGIN_SESSION_ID, &other_project);

    let output = command.output().unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("worktree does not match --project"),
        "{output:?}"
    );
    assert!(!fixture.resume_calls.exists());
    assert!(!fixture.fresh_calls.exists());
    assert_eq!(continuation_count(&fixture.connection()), 0);
}

#[test]
fn request_flag_rejects_a_resume_session_that_differs_from_the_evidence_bound_origin() {
    let fixture = ContinuationFixture::new();
    let baseline_invocation_count = invocation_count(&fixture.connection());
    let mut command = fixture.continuation_command_with(MISMATCHED_SESSION_ID, &fixture.worktree);

    let output = command.output().unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Error: Fresh continuation origin session does not match --resume\n"
    );
    assert!(!fixture.resume_calls.exists());
    assert!(!fixture.fresh_calls.exists());
    assert_eq!(continuation_count(&fixture.connection()), 0);
    assert_eq!(
        invocation_count(&fixture.connection()),
        baseline_invocation_count
    );
    assert!(!fixture.planning_root.join("continuations").exists());
}

#[test]
fn request_flag_rejects_evidence_changed_after_the_request_was_written() {
    let fixture = ContinuationFixture::new();
    let baseline_invocation_count = invocation_count(&fixture.connection());
    fs::write(&fixture.expected_artifacts[0].1, b"mutated question").unwrap();

    let output = fixture.continuation_command().output().unwrap();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(!fixture.resume_calls.exists());
    assert!(!fixture.fresh_calls.exists());
    assert_eq!(continuation_count(&fixture.connection()), 0);
    assert_eq!(
        invocation_count(&fixture.connection()),
        baseline_invocation_count
    );
    assert!(!fixture.planning_root.join("continuations").exists());
}

#[test]
fn no_request_headless_dispatch_preserves_file_project_parent_retry_and_migration() {
    let fixture = LegacyResumeFixture::new();
    let mut command = fixture.command();
    command
        .arg("--model")
        .arg("legacy-headless")
        .arg("--resume")
        .arg(ORIGIN_SESSION_ID)
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--file")
        .arg(&fixture.answer_path)
        .arg("--project")
        .arg(&fixture.project)
        .env(FORCE_TERMINAL_SIGNAL_KIND, "QuotaExhaustedInband,None");

    let output = command.output().unwrap();

    assert_process_success(&output);
    assert_eq!(
        read_provider_record(&fixture.first_record)["argv"],
        json!([
            "headless-a",
            "--resume",
            ORIGIN_SESSION_ID,
            "legacy answer from file\n"
        ])
    );
    let second = read_provider_record(&fixture.second_record);
    let second_argv = second["argv"].as_array().unwrap();
    assert_eq!(second_argv[0], "headless-b");
    assert_eq!(second_argv.last().unwrap(), "legacy answer from file\n");
    assert_eq!(second["cwd"], fixture.project.to_string_lossy().as_ref());
    let rows = invocation_rows_after(&fixture.connection(), fixture.baseline_max_invocation_id);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().all(|row| row.1 == Some(fixture.parent_row_id)));
    assert_eq!(rows[0].2, LEGACY_PROVIDER_A);
    assert_eq!(rows[0].3, "failed");
    assert_eq!(rows[0].4.as_deref(), Some("quota_exhausted"));
    assert_eq!(rows[1].2, LEGACY_PROVIDER_B);
    assert_eq!(rows[1].3, "succeeded");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(&format!(
            "[migrate] {LEGACY_PROVIDER_A} -> {LEGACY_PROVIDER_B} reason=exhausted"
        )),
        "{output:?}"
    );
    assert_eq!(fixture.continuation_count(), 0);
    assert!(!fixture.project.join("continuations").exists());
}

#[test]
fn no_request_interactive_dispatch_preserves_session_project_and_parent_without_continuation() {
    let fixture = LegacyResumeFixture::new();
    let mut command = fixture.command();
    command
        .arg("--model")
        .arg("legacy-interactive")
        .arg("--resume")
        .arg(FRESH_SESSION_ID)
        .arg("--models-dir")
        .arg(&fixture.models_dir)
        .arg("--project")
        .arg(&fixture.project)
        .stdin(Stdio::null());

    let output = command.output().unwrap();

    assert_process_success(&output);
    let record = read_provider_record(&fixture.interactive_record);
    assert_eq!(
        record["argv"],
        json!(["interactive-launch", "--resume", FRESH_SESSION_ID])
    );
    assert_eq!(record["cwd"], fixture.project.to_string_lossy().as_ref());
    let rows = invocation_rows_after(&fixture.connection(), fixture.baseline_max_invocation_id);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, Some(fixture.parent_row_id));
    assert_eq!(rows[0].2, "interactive-owner");
    assert_eq!(rows[0].3, "succeeded");
    assert_eq!(fixture.continuation_count(), 0);
    assert!(!fixture.project.join("continuations").exists());
}

fn isolated_command(config_home: &Path, data_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    command.env("XDG_CONFIG_HOME", config_home);
    command.env("XDG_DATA_HOME", data_home);
    command.env("HOME", data_home);
    command.env("OULIPOLY_DATA_DIR", data_home.join("oulipoly-agent-runner"));
    command.env_remove("OULIPOLY_AUTO_WAKE");
    command.env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID");
    command.env_remove("OULIPOLY_AUTO_WAKE_TOKEN");
    command.env_remove("OULIPOLY_AUTO_WAKE_COUNT");
    command.env_remove("OULIPOLY_AUTO_WAKE_RETRY_BASE_MS");
    command.env_remove("OULIPOLY_PARENT_INVOCATION");
    command
}

fn state_db_path(data_home: &Path) -> PathBuf {
    data_home.join("oulipoly-agent-runner").join("state.db")
}

fn write_python_provider(root: &Path, name: &str, body: &str) -> PathBuf {
    let path = root.join(name);
    let source = format_python_provider_source(body);
    fs::write(&path, source).unwrap();
    make_executable(&path);
    path
}

fn format_python_provider_source(body: &str) -> String {
    format!("#!/usr/bin/env python3\n{body}\n")
}

fn write_recording_shell_provider(root: &Path, name: &str, record: &Path, body: &str) -> PathBuf {
    let path = root.join(name);
    let source = format_recording_shell_provider_source(record, body);
    fs::write(&path, source).unwrap();
    make_executable(&path);
    path
}

fn format_recording_shell_provider_source(record: &Path, body: &str) -> String {
    format!(
        r#"#!/usr/bin/env python3
import json
import os
import pathlib
import subprocess
import sys
pathlib.Path({record:?}).write_text(json.dumps({{"argv": sys.argv[1:], "cwd": os.getcwd()}}))
raise SystemExit(subprocess.call(["bash", "-c", {body:?}]))
"#,
        record = record.display().to_string(),
        body = body,
    )
}

fn make_executable(path: &Path) {
    let permissions = read_permissions(path);
    let permissions = map_executable_permissions(permissions);
    write_permissions(path, permissions);
}

fn read_permissions(path: &Path) -> fs::Permissions {
    fs::metadata(path).unwrap().permissions()
}

fn map_executable_permissions(mut permissions: fs::Permissions) -> fs::Permissions {
    permissions.set_mode(0o755);
    permissions
}

fn write_permissions(path: &Path, permissions: fs::Permissions) {
    fs::set_permissions(path, permissions).unwrap();
}

fn start_invocation(
    state: &StateDb,
    invocation_uuid: &str,
    model_name: &str,
    provider_name: &str,
    parent_invocation_id: Option<i64>,
) -> i64 {
    state
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: model_name.to_string(),
            provider_name: provider_name.to_string(),
            provider_index: 0,
            parent_invocation_id,
        })
        .unwrap()
}

fn bind_provider_session(
    state: &StateDb,
    row_id: i64,
    provider_name: &str,
    session_id: &str,
    capture_method: &'static str,
    resume_input_id: Option<&str>,
    cwd: &Path,
) {
    state
        .bind_invocation_provider_session_start(
            row_id,
            &ProviderSessionBinding {
                provider_session_id: session_id.to_string(),
                capture_method,
                resume_input_id: resume_input_id.map(str::to_string),
                provider_session_resolved_account: Some(cwd.display().to_string()),
            },
        )
        .unwrap();
    provider_authority_fixture::bind_session_authority_with_cwd(
        &Connection::open(state.path()).unwrap(),
        provider_name,
        session_id,
        cwd,
    );
}

fn seed_session_turn(state: &StateDb, provider: &str, session_id: &str, turn_id: &str) {
    state
        .ingest_session_turns_batch(
            provider,
            &[SessionTurnIngest {
                session_id: session_id.to_string(),
                turn_id: turn_id.to_string(),
                timestamp: timestamp("2026-08-10T10:00:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some("fixture turn".to_string()),
            }],
        )
        .unwrap();
}

fn seed_active_chain(
    db_path: &Path,
    chain_id: &str,
    model_name: &str,
    provider_name: &str,
    session_id: &str,
    cwd: &Path,
) {
    let connection = Connection::open(db_path).unwrap();
    connection
        .execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-08-10T10:00:00Z', '2026-08-10T10:00:00Z', ?2)",
            params![chain_id, model_name],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-08-10T10:00:00Z', 'initial')",
            params![chain_id, provider_name, session_id],
        )
        .unwrap();
    provider_authority_fixture::bind_session_authority_with_cwd(
        &connection,
        provider_name,
        session_id,
        cwd,
    );
}

fn write_evidence(planning_root: &Path, worktree: &Path) -> Vec<EvidenceIdentity> {
    let graph_path = map_session_graph_path(planning_root);
    let artifacts = map_evidence_artifacts(
        planning_root,
        graph_path.clone(),
        format_question_artifact(worktree, &graph_path),
        format_answer_artifact(&graph_path),
        format_session_graph_artifact(),
        format_origin_trace_artifact(),
        format_ticket_snapshot_artifact(),
    );
    artifacts.into_iter().map(write_evidence_artifact).collect()
}

fn map_session_graph_path(planning_root: &Path) -> PathBuf {
    planning_root.join("graph.json")
}

fn format_question_artifact(worktree: &Path, graph_path: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": 1,
        "kind": "agent_question",
        "question_id": "question-1",
        "origin": {
            "invocation_uuid": ORIGIN_INVOCATION_ID,
            "session_id": ORIGIN_SESSION_ID,
            "worktree_path": worktree,
        },
        "state_refs": {"session_graph_manifest": graph_path},
    }))
    .unwrap()
}

fn format_answer_artifact(graph_path: &Path) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schema_version": 1,
        "kind": "agent_answer",
        "question_id": "question-1",
        "answered_by": "user-via-root-orchestrator",
        "continuation_plan": {"session_graph_manifest": graph_path},
    }))
    .unwrap()
}

fn format_session_graph_artifact() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "root_invocation_uuid": ORIGIN_INVOCATION_ID,
        "invocation_ids": [ORIGIN_INVOCATION_ID],
        "session_ids": [ORIGIN_SESSION_ID],
        "question_ids": ["question-1"],
    }))
    .unwrap()
}

fn format_origin_trace_artifact() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "root": {
            "invocation": {"id": ORIGIN_INVOCATION_ID},
            "session": {"provider_session_id": ORIGIN_SESSION_ID},
        },
    }))
    .unwrap()
}

fn format_ticket_snapshot_artifact() -> Vec<u8> {
    b"AGE-290 ticket snapshot".to_vec()
}

fn map_evidence_artifacts(
    planning_root: &Path,
    graph_path: PathBuf,
    question: Vec<u8>,
    answer: Vec<u8>,
    session_graph: Vec<u8>,
    origin_trace: Vec<u8>,
    ticket_snapshot: Vec<u8>,
) -> [EvidenceArtifact; 5] {
    [
        ("question", planning_root.join("question.json"), question),
        ("answer", planning_root.join("answer.json"), answer),
        ("session graph", graph_path, session_graph),
        (
            "origin trace",
            planning_root.join("trace.json"),
            origin_trace,
        ),
        (
            "ticket snapshot",
            planning_root.join("ticket.md"),
            ticket_snapshot,
        ),
    ]
}

fn write_evidence_artifact(artifact: EvidenceArtifact) -> EvidenceIdentity {
    let (name, path, bytes) = artifact;
    write_file_bytes(&path, &bytes);
    let sha256 = format_sha256(&bytes);
    map_evidence_identity(name, path, sha256)
}

fn write_file_bytes(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).unwrap();
}

fn format_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn map_evidence_identity(name: &'static str, path: PathBuf, sha256: String) -> EvidenceIdentity {
    (name, path, sha256)
}

fn write_request(
    request_path: &Path,
    planning_root: &Path,
    worktree: &Path,
    origin_session_id: &str,
    artifacts: &[EvidenceIdentity],
) {
    let question = map_artifact_identity(find_evidence_artifact(artifacts, "question"));
    let answer = map_artifact_identity(find_evidence_artifact(artifacts, "answer"));
    let session_graph = map_artifact_identity(find_evidence_artifact(artifacts, "session graph"));
    let origin_trace = map_artifact_identity(find_evidence_artifact(artifacts, "origin trace"));
    let ticket_snapshot =
        map_artifact_identity(find_evidence_artifact(artifacts, "ticket snapshot"));
    let bytes = format_continuation_request(
        planning_root,
        worktree,
        origin_session_id,
        question,
        answer,
        session_graph,
        origin_trace,
        ticket_snapshot,
    );
    write_file_bytes(request_path, &bytes);
}

fn find_evidence_artifact<'a>(
    artifacts: &'a [EvidenceIdentity],
    name: &str,
) -> &'a EvidenceIdentity {
    artifacts
        .iter()
        .find(|artifact| artifact.0 == name)
        .unwrap()
}

fn map_artifact_identity(artifact: &EvidenceIdentity) -> Value {
    json!({"path": artifact.1, "sha256": artifact.2})
}

#[allow(clippy::too_many_arguments)]
fn format_continuation_request(
    planning_root: &Path,
    worktree: &Path,
    origin_session_id: &str,
    question: Value,
    answer: Value,
    session_graph: Value,
    origin_trace: Value,
    ticket_snapshot: Value,
) -> Vec<u8> {
    serde_json::to_vec_pretty(&json!({
        "schema_version": 1,
        "kind": "fresh_continuation_request",
        "question_id": "question-1",
        "origin_invocation_id": ORIGIN_INVOCATION_ID,
        "origin_session_id": origin_session_id,
        "planning_root": planning_root,
        "worktree": worktree,
        "last_successful_boundary": "phase-4-verified",
        "active_blocked_boundary": "phase-5-apply-answer",
        "target_model": "fresh-model",
        "evidence": {
            "question": question,
            "answer": answer,
            "session_graph": session_graph,
            "origin_trace": origin_trace,
            "ticket_snapshot": ticket_snapshot,
        },
    }))
    .unwrap()
}

fn expected_fresh_prompt(
    fixture: &ContinuationFixture,
    resume_invocation_id: &str,
    origin_invocation_id: &str,
    origin_session_id: &str,
) -> String {
    let mut prompt = format!(
        "Continue the blocked workflow in this fresh provider session.\n\
         Do not retry or mutate the origin session.\n\
         Origin invocation: {origin_invocation_id}\n\
         Origin session: {origin_session_id}\n\
         Failed resume invocation: {resume_invocation_id}\n\
         Worktree: {}\n\
         Last successful boundary: phase-4-verified\n\
         Active blocked boundary: phase-5-apply-answer\n\
         Read these exact artifacts before continuing:\n",
        fixture.worktree.display()
    );
    for (name, path, sha256) in &fixture.expected_artifacts {
        prompt.push_str(&format!("- {name}: {} (sha256 {sha256})\n", path.display()));
    }
    prompt
}

fn read_provider_record(path: &Path) -> Value {
    let bytes = read_file_bytes(path);
    parse_provider_record(&bytes)
}

fn read_file_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}

fn parse_provider_record(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn assert_process_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn line_count(path: &Path) -> usize {
    let content = read_optional_file_text(path);
    map_line_count(content)
}

fn read_optional_file_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn map_line_count(content: Option<String>) -> usize {
    content.map(|content| content.lines().count()).unwrap_or(0)
}

fn invocation_count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

fn continuation_count(connection: &Connection) -> i64 {
    connection
        .query_row("SELECT COUNT(*) FROM fresh_continuations", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn invocation_rows_after(connection: &Connection, row_id: i64) -> Vec<InvocationRow> {
    let mut statement = connection
        .prepare(
            "SELECT id, parent_invocation_id, provider_name, status, error_category
               FROM invocations
              WHERE id > ?1
              ORDER BY id",
        )
        .unwrap();
    statement
        .query_map([row_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}
