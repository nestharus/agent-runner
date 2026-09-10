#![cfg(unix)]
//! AGE-166 headless-resume zero-turn quota-detection CLI fixture tests.
//!
//! ## Declared roles
//!
//! `validator`, `orchestration`, `formatter`
//!
//! Drives `oulipoly-agent-runner --resume` flow through the Age153 fixture
//! harness (orchestration). Asserts that a lagging canonical checkpoint cannot
//! synthesize conclusive zero-turn evidence or exhaust/migrate a provider, and
//! that a later productive resume still succeeds (validator). Provider scripts
//! and capture-pool TOML are produced inline (formatter).

mod age153_support;
#[path = "pr_f_resume_integration.rs"]
mod pr_f_resume_integration;

use age153_support::{line_count, parse_valid_invocations, terminal_signal_lines, toml_string};
use pr_f_resume_integration::{Fixture, parse_invocation, session_turn_count};
use rusqlite::params;
use std::fs;
use std::path::Path;
use std::process::Output;

const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";
const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";

fn write_sessions_config(fixture: &Fixture, entries: &[(&str, &Path)]) {
    let app_config_dir = fixture.config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(&app_config_dir).unwrap();
    let mut body = String::new();
    for (provider, transcript) in entries {
        body.push_str(&format!(
            r#"[{provider}]
turn_script = 'cat "{}"'
state_dir = '{}'

"#,
            transcript.display(),
            fixture
                .dir
                .path()
                .join(format!("{provider}-session-state"))
                .display()
        ));
    }
    fs::write(app_config_dir.join("sessions.toml"), body).unwrap();
}

fn seed_baseline_transcript(transcript: &Path, session_id: &str) {
    fs::write(
        transcript,
        format!(
            r#"{{"session_id":"{session_id}","turn_id":"baseline","timestamp":"2026-04-17T08:00:00Z","role":"assistant"}}
"#
        ),
    )
    .unwrap();
}

fn resume_session_arg_parser() -> &'static str {
    r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--resume" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done"#
}

fn zero_turn_resume_body(marker: &Path, exit_code: i32) -> String {
    format!(
        r#"{}
printf '%s\n' ran >> {}
printf 'zero turn resume for %s\n' "$session_id"
exit {exit_code}"#,
        resume_session_arg_parser(),
        toml_string(&marker.display().to_string())
    )
}

fn productive_resume_body(marker: &Path, transcript: &Path, stdout: &str) -> String {
    format!(
        r#"{}
printf '%s\n' ran >> {}
printf '{{"session_id":"%s","turn_id":"productive-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
printf '%s\n' {}"#,
        resume_session_arg_parser(),
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string()),
        toml_string(stdout)
    )
}

fn counted_resume_body(
    marker: &Path,
    transcript: &Path,
    counter: &Path,
    productive_stdout: &str,
) -> String {
    format!(
        r#"{}
count=0
if [ -f {} ]; then
  count="$(cat {})"
fi
count=$((count + 1))
printf '%s\n' "$count" > {}
printf '%s\n' ran >> {}
if [ "$count" -ge 2 ]; then
  printf '{{"session_id":"%s","turn_id":"productive-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
  printf '%s\n' {}
  exit 0
fi
printf 'zero turn resume for %s\n' "$session_id"
exit 1"#,
        resume_session_arg_parser(),
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string()),
        toml_string(productive_stdout)
    )
}

fn output_text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_maybe_marker_with_session_but_without_zero_turn_evidence(stderr: &str) {
    let maybe_lines: Vec<_> = terminal_signal_lines(stderr)
        .into_iter()
        .filter(|line| line.contains("\"kind\":\"MaybeQuotaExhausted\""))
        .collect();
    assert!(
        !maybe_lines.is_empty(),
        "expected MaybeQuotaExhausted marker in stderr:\n{stderr}"
    );
    assert!(
        maybe_lines
            .iter()
            .any(|line| line.contains(&format!("\"session_id\":\"{SESSION_ID}\""))),
        "expected marker to include resolved resume session id:\n{stderr}"
    );
    assert!(
        maybe_lines
            .iter()
            .all(|line| !line.contains("new_assistant_turns=")),
        "lagging ingestion must not produce zero-turn evidence:\n{stderr}"
    );
}

fn assert_no_maybe_marker(stderr: &str) {
    assert!(
        !stderr.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "stderr must not contain MaybeQuotaExhausted marker:\n{stderr}"
    );
}

fn exhausted_row_count(fixture: &Fixture, provider: &str) -> i64 {
    fixture
        .conn()
        .query_row(
            "SELECT COUNT(*)
             FROM provider_quotas
             WHERE provider_name = ?1 AND exhausted_at IS NOT NULL",
            params![provider],
            |row| row.get(0),
        )
        .unwrap()
}

fn failed_invocation_count(fixture: &Fixture, provider: &str, terminal_reason: &str) -> i64 {
    fixture
        .conn()
        .query_row(
            "SELECT COUNT(*)
             FROM invocations
             WHERE provider_name = ?1
               AND status = 'failed'
               AND success = 0
               AND terminal_reason = ?2
               AND finished_at IS NOT NULL",
            params![provider, terminal_reason],
            |row| row.get(0),
        )
        .unwrap()
}

fn invocation_count_for_provider_session(
    fixture: &Fixture,
    provider: &str,
    session_id: &str,
) -> i64 {
    fixture
        .conn()
        .query_row(
            "SELECT COUNT(*)
             FROM invocations
             WHERE provider_name = ?1 AND provider_session_id = ?2",
            params![provider, session_id],
            |row| row.get(0),
        )
        .unwrap()
}

fn provider_and_session_for_invocation(
    fixture: &Fixture,
    invocation_uuid: &str,
) -> (String, Option<String>) {
    fixture
        .conn()
        .query_row(
            "SELECT provider_name, provider_session_id
             FROM invocations
             WHERE invocation_uuid = ?1",
            params![invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn setup_migratable_fixture(
    fixture: &Fixture,
    active_body: &str,
    sibling_body: &str,
    active_transcript: &Path,
    sibling_transcript: &Path,
) {
    let source_projects = fixture.dir.path().join("source-projects");
    let target_projects = fixture.dir.path().join("target-projects");
    fixture.stage_claude_jsonl(&source_projects, SESSION_ID);
    let active = fixture.write_script("age166-resume-active.sh", active_body);
    let sibling = fixture.write_script("age166-resume-sibling.sh", sibling_body);
    fixture.write_migratable_two_provider_model(
        "age166-resume",
        &active,
        &sibling,
        &source_projects,
        &target_projects,
    );
    write_sessions_config(
        fixture,
        &[
            ("claude-a", active_transcript),
            ("claude-b", sibling_transcript),
        ],
    );
    fixture.seed_active_chain(CHAIN_ID, "claude-a", SESSION_ID, "age166-resume");
    fixture.seed_session_turns(
        "claude-a",
        SESSION_ID,
        &[("baseline", "2026-04-17T08:00:00Z")],
    );
}

#[test]
fn resume_lagging_zero_turn_does_not_confirm_quota_or_migrate() {
    let fixture = Fixture::new();
    let active_transcript = fixture.dir.path().join("resume-confirm-a.jsonl");
    let sibling_transcript = fixture.dir.path().join("resume-confirm-b.jsonl");
    seed_baseline_transcript(&active_transcript, SESSION_ID);
    fs::write(&sibling_transcript, "").unwrap();
    let active_marker = fixture.dir.path().join("resume-confirm-a.txt");
    let sibling_marker = fixture.dir.path().join("resume-confirm-b.txt");
    setup_migratable_fixture(
        &fixture,
        &zero_turn_resume_body(&active_marker, 1),
        &productive_resume_body(&sibling_marker, &sibling_transcript, "sibling resume ran"),
        &active_transcript,
        &sibling_transcript,
    );

    let first = fixture
        .base_resume_command("age166-resume", SESSION_ID)
        .arg("--prompt")
        .arg("verify quota")
        .env(FORCE_KIND, "MaybeQuotaExhausted")
        .output()
        .unwrap();
    let (_, first_stderr) = output_text(&first);
    assert_maybe_marker_with_session_but_without_zero_turn_evidence(&first_stderr);
    assert_eq!(first.status.code(), Some(1), "{first:?}");
    let first_invocation = parse_valid_invocations(&first_stderr)
        .into_iter()
        .find(|invocation| {
            provider_and_session_for_invocation(&fixture, &invocation.id).0 == "claude-a"
        })
        .expect("expected first maybe invocation on active provider");
    let (first_provider, first_provider_session_id) =
        provider_and_session_for_invocation(&fixture, &first_invocation.id);
    assert_eq!(first_provider, "claude-a");
    assert_eq!(first_provider_session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(exhausted_row_count(&fixture, &first_provider), 0);
    assert_eq!(
        failed_invocation_count(&fixture, "claude-a", "maybe_quota_exhausted"),
        1
    );
    assert_eq!(
        invocation_count_for_provider_session(&fixture, "claude-a", SESSION_ID),
        1
    );
    assert_eq!(line_count(&active_marker), 1);
    assert_eq!(line_count(&sibling_marker), 0);
    assert_eq!(
        fixture.active_segment(CHAIN_ID),
        (first_provider, SESSION_ID.to_string())
    );
}

#[test]
fn resume_lagging_failure_then_productive_turn_stays_on_active_provider() {
    let fixture = Fixture::new();
    let transcript = fixture.dir.path().join("resume-clears-a.jsonl");
    let sibling_transcript = fixture.dir.path().join("resume-clears-b.jsonl");
    seed_baseline_transcript(&transcript, SESSION_ID);
    fs::write(&sibling_transcript, "").unwrap();
    let marker = fixture.dir.path().join("resume-clears-a.txt");
    let sibling_marker = fixture.dir.path().join("resume-clears-b.txt");
    let counter = fixture.dir.path().join("resume-clears-count.txt");
    setup_migratable_fixture(
        &fixture,
        &counted_resume_body(&marker, &transcript, &counter, "productive resume wins"),
        &productive_resume_body(
            &sibling_marker,
            &sibling_transcript,
            "sibling should not run",
        ),
        &transcript,
        &sibling_transcript,
    );

    let first = fixture
        .base_resume_command("age166-resume", SESSION_ID)
        .arg("--prompt")
        .arg("first zero turn")
        .env(FORCE_KIND, "MaybeQuotaExhausted")
        .output()
        .unwrap();
    let (_, first_stderr) = output_text(&first);
    assert_maybe_marker_with_session_but_without_zero_turn_evidence(&first_stderr);
    assert_eq!(first.status.code(), Some(1), "{first:?}");
    assert_eq!(exhausted_row_count(&fixture, "claude-a"), 0);

    let second = fixture
        .base_resume_command("age166-resume", SESSION_ID)
        .arg("--prompt")
        .arg("productive verification")
        .output()
        .unwrap();

    assert_eq!(second.status.code(), Some(0), "{second:?}");
    let (second_stdout, second_stderr) = output_text(&second);
    assert_no_maybe_marker(&second_stderr);
    assert!(
        second_stdout.contains("productive resume wins"),
        "{second_stdout}"
    );
    assert_eq!(exhausted_row_count(&fixture, "claude-a"), 0);
    assert_eq!(line_count(&sibling_marker), 0);
    assert_eq!(
        fixture.active_segment(CHAIN_ID),
        ("claude-a".to_string(), SESSION_ID.to_string())
    );
    let active_provider = fixture.active_segment(CHAIN_ID).0;
    assert_eq!(
        session_turn_count(&fixture, &active_provider, SESSION_ID),
        1
    );
}

#[test]
fn interactive_maybe_signal_no_auto_relaunch() {
    let fixture = Fixture::new();
    let transcript = fixture.dir.path().join("interactive-maybe-a.jsonl");
    seed_baseline_transcript(&transcript, SESSION_ID);
    let marker = fixture.dir.path().join("interactive-maybe-a.txt");
    let provider = fixture.write_script(
        "interactive-maybe-a.sh",
        &zero_turn_resume_body(&marker, 23),
    );
    fixture.write_single_provider_model(
        "age166-interactive-resume",
        "claude2",
        &provider,
        r#"
[providers.resume]
kind = "flag"
flag = "--resume"
"#,
    );
    fixture.write_sessions_config("claude2", &transcript);
    fixture.seed_session_turns(
        "claude2",
        SESSION_ID,
        &[("baseline", "2026-04-17T08:00:00Z")],
    );

    let output = fixture
        .base_top_level_resume_command("age166-interactive-resume", SESSION_ID)
        .env(FORCE_KIND, "MaybeQuotaExhausted")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(23), "{output:?}");
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_with_session_but_without_zero_turn_evidence(&stderr);
    let invocation = parse_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.terminal_reason.as_deref(),
        Some("maybe_quota_exhausted")
    );
    assert_eq!(exhausted_row_count(&fixture, "claude2"), 0);
    assert_eq!(line_count(&marker), 1);
    assert_eq!(
        invocation_count_for_provider_session(&fixture, "claude2", SESSION_ID),
        1,
        "interactive/no-prompt maybe signal must not auto-relaunch"
    );
}
