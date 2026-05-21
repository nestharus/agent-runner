#![cfg(unix)]
//! AGE-166 one-shot zero-turn quota-detection CLI fixture tests.
//!
//! ## Declared roles
//!
//! `validator`, `orchestration`, `formatter`
//!
//! The tests drive `oulipoly-agent-runner` one-shot CLI invocations through
//! the `Age153Fixture::run_one_shot_with_env` harness (orchestration), assert
//! the typed `MaybeQuotaExhausted` marker shape + `provider_quotas` state
//! transitions (validator), and produce script bodies + capture-pool
//! provider/sessions TOML (formatter).

mod age153_support;

use age153_support::{
    Age153Fixture, assert_no_terminal_marker_on_stdout, assert_result_envelope_shape, line_count,
    parse_terminal_signal_line, success_body, terminal_signal_lines, toml_string,
};
use rusqlite::params;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Output;

const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";

fn write_capture_pool(
    fixture: &Age153Fixture,
    model_name: &str,
    providers: &[(&str, &Path)],
    transcript_paths: &[(&str, &Path)],
) {
    let mut model = String::new();
    let mut providers_toml = String::new();
    let mut sessions_toml = String::new();

    for (provider, command) in providers {
        model.push_str(&format!(
            r#"[[providers]]
name = "{provider}"
args = []

"#
        ));
        providers_toml.push_str(&format!(
            r#"[{provider}]
command = {}
args = []
interactive_args = ["interactive"]
prompt_mode = "arg"

[{provider}.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

"#,
            toml_string(&command.display().to_string())
        ));
    }

    for (provider, transcript) in transcript_paths {
        sessions_toml.push_str(&format!(
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

    fs::write(fixture.models_dir.join(format!("{model_name}.toml")), model).unwrap();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        providers_toml,
    )
    .unwrap();
    fs::write(fixture.app_config_dir.join("sessions.toml"), sessions_toml).unwrap();
}

fn zero_turn_capture_body(marker: &Path, exit_code: i32) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s\n' ran >> {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
exit {exit_code}"#,
        toml_string(&marker.display().to_string())
    )
}

fn assistant_turn_capture_body(marker: &Path, transcript: &Path, exit_code: i32) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s\n' ran >> {}
printf '{{"session_id":"%s","turn_id":"turn-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
exit {exit_code}"#,
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string())
    )
}

fn assistant_turn_stdout_capture_body(
    marker: &Path,
    transcript: &Path,
    stdout: &str,
    exit_code: i32,
) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s\n' ran >> {}
printf '{{"session_id":"%s","turn_id":"turn-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
printf '%s\n' {}
exit {exit_code}"#,
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string()),
        toml_string(stdout)
    )
}

fn zero_turn_then_assistant_turn_capture_body(
    marker: &Path,
    transcript: &Path,
    counter: &Path,
) -> String {
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
count=0
if [ -f {} ]; then
  count="$(cat {})"
fi
count=$((count + 1))
printf '%s\n' "$count" > {}
printf '%s\n' ran >> {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
if [ "$count" -ge 2 ]; then
  printf '{{"session_id":"%s","turn_id":"turn-%s","timestamp":"2026-04-17T08:00:01Z","role":"assistant"}}\n' "$session_id" "$$" >> {}
  exit 0
fi
exit 1"#,
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&counter.display().to_string()),
        toml_string(&marker.display().to_string()),
        toml_string(&transcript.display().to_string())
    )
}

fn output_text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn assert_maybe_marker_with_zero_turn_evidence(stderr: &str) -> Value {
    let maybe_lines: Vec<_> = terminal_signal_lines(stderr)
        .into_iter()
        .filter(|line| line.contains("\"kind\":\"MaybeQuotaExhausted\""))
        .collect();
    assert!(
        !maybe_lines.is_empty(),
        "expected MaybeQuotaExhausted marker in stderr:\n{stderr}"
    );
    let value = parse_terminal_signal_line(maybe_lines[0]);
    let evidence = value["evidence"]["excerpt"]
        .as_str()
        .expect("marker evidence excerpt");
    assert!(evidence.contains("provider_session_id="), "{evidence}");
    assert!(
        evidence.contains("baseline_assistant_turns=0"),
        "{evidence}"
    );
    assert!(evidence.contains("current_assistant_turns=0"), "{evidence}");
    assert!(evidence.contains("new_assistant_turns=0"), "{evidence}");
    value
}

fn assert_no_maybe_marker(stderr: &str) {
    assert!(
        !stderr.contains("\"kind\":\"MaybeQuotaExhausted\""),
        "stderr must not contain MaybeQuotaExhausted marker:\n{stderr}"
    );
}

fn latest_invocation_terminal_reason(fixture: &Age153Fixture, provider: &str) -> Option<String> {
    fixture
        .conn()
        .query_row(
            "SELECT terminal_reason
             FROM invocations
             WHERE provider_name = ?1
               AND finished_at IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            params![provider],
            |row| row.get(0),
        )
        .unwrap()
}

fn latest_provider_session_id(fixture: &Age153Fixture, provider: &str) -> String {
    fixture
        .conn()
        .query_row(
            "SELECT provider_session_id
             FROM invocations
             WHERE provider_name = ?1
               AND provider_session_id IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            params![provider],
            |row| row.get(0),
        )
        .unwrap()
}

fn failed_invocation_error_category(
    fixture: &Age153Fixture,
    provider: &str,
    terminal_reason: &str,
) -> Option<String> {
    fixture
        .conn()
        .query_row(
            "SELECT error_category
             FROM invocations
             WHERE provider_name = ?1
               AND terminal_reason = ?2
               AND finished_at IS NOT NULL
             ORDER BY id DESC
             LIMIT 1",
            params![provider, terminal_reason],
            |row| row.get(0),
        )
        .unwrap()
}

fn failed_invocation_count_with_error_category(
    fixture: &Age153Fixture,
    provider: &str,
    terminal_reason: &str,
    error_category: &str,
) -> i64 {
    fixture
        .conn()
        .query_row(
            "SELECT COUNT(*)
             FROM invocations
             WHERE provider_name = ?1
               AND terminal_reason = ?2
               AND error_category = ?3
               AND finished_at IS NOT NULL",
            params![provider, terminal_reason, error_category],
            |row| row.get(0),
        )
        .unwrap()
}

fn provider_invocation_sessions(fixture: &Age153Fixture, provider: &str) -> Vec<Option<String>> {
    let conn = fixture.conn();
    let mut stmt = conn
        .prepare(
            "SELECT provider_session_id
             FROM invocations
             WHERE provider_name = ?1
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map(params![provider], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn one_shot_first_zero_turn_resumes_same_provider_without_exhausted_write() {
    let fixture = Age153Fixture::new();
    let transcript = fixture.dir.path().join("one-shot-first-zero-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture.dir.path().join("one-shot-first-zero-turn.txt");
    let counter = fixture
        .dir
        .path()
        .join("one-shot-first-zero-turn-count.txt");
    let provider = fixture.write_script(
        "one-shot-first-zero-turn.sh",
        &zero_turn_then_assistant_turn_capture_body(&marker, &transcript, &counter),
    );
    write_capture_pool(
        &fixture,
        "age166-first-zero-turn",
        &[("claude-age166-a", &provider)],
        &[("claude-age166-a", &transcript)],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-first-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_with_zero_turn_evidence(&stderr);
    assert_eq!(
        latest_invocation_terminal_reason(&fixture, "claude-age166-a").as_deref(),
        None
    );
    assert_eq!(fixture.exhausted_row_count("claude-age166-a"), 0);
    let sessions = provider_invocation_sessions(&fixture, "claude-age166-a");
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(sessions[1], sessions[0], "{sessions:?}");
    assert_eq!(line_count(&marker), 2);
}

#[test]
fn one_shot_second_zero_turn_confirms_quota_and_migrates() {
    let fixture = Age153Fixture::new();
    let first_transcript = fixture.dir.path().join("one-shot-second-a.jsonl");
    let sibling_transcript = fixture.dir.path().join("one-shot-second-b.jsonl");
    fs::write(&first_transcript, "").unwrap();
    fs::write(&sibling_transcript, "").unwrap();
    let first_marker = fixture.dir.path().join("one-shot-second-a.txt");
    let sibling_marker = fixture.dir.path().join("one-shot-second-b.txt");
    let first = fixture.write_script(
        "one-shot-second-a.sh",
        &zero_turn_capture_body(&first_marker, 1),
    );
    let sibling = fixture.write_script(
        "one-shot-second-b.sh",
        &assistant_turn_stdout_capture_body(
            &sibling_marker,
            &sibling_transcript,
            "next provider ran",
            17,
        ),
    );
    write_capture_pool(
        &fixture,
        "age166-second-zero-turn",
        &[("claude-age166-a", &first), ("claude-age166-b", &sibling)],
        &[
            ("claude-age166-a", &first_transcript),
            ("claude-age166-b", &sibling_transcript),
        ],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-second-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_with_zero_turn_evidence(&stderr);
    assert_eq!(fixture.exhausted_row_count("claude-age166-a"), 1);
    assert_eq!(
        failed_invocation_count_with_error_category(
            &fixture,
            "claude-age166-a",
            "maybe_quota_exhausted",
            "quota_exhausted"
        ),
        1
    );
    assert_eq!(
        failed_invocation_error_category(&fixture, "claude-age166-a", "maybe_quota_exhausted")
            .as_deref(),
        Some("quota_exhausted")
    );
    let sessions = provider_invocation_sessions(&fixture, "claude-age166-a");
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(sessions[1], sessions[0], "{sessions:?}");
    assert_eq!(line_count(&first_marker), 2);
    assert_eq!(line_count(&sibling_marker), 1);
    assert_eq!(output.status.code(), Some(17), "{output:?}");
}

#[test]
fn one_shot_clean_exit_with_turn_preserves_status_zero_no_terminal_reason() {
    let fixture = Age153Fixture::new();
    let transcript = fixture.dir.path().join("one-shot-clean-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture.dir.path().join("one-shot-clean-turn.txt");
    let provider = fixture.write_script(
        "one-shot-clean-turn.sh",
        &assistant_turn_capture_body(&marker, &transcript, 0),
    );
    write_capture_pool(
        &fixture,
        "age166-clean-turn",
        &[("claude-age166-clean", &provider)],
        &[("claude-age166-clean", &transcript)],
    );

    let output = fixture.run_one_shot_with_env("age166-clean-turn", &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let (stdout, stderr) = output_text(&output);
    assert_no_maybe_marker(&stderr);
    let result = assert_result_envelope_shape(&stdout);
    assert_eq!(result["success"], true);
    assert!(result["terminal_reason"].is_null(), "{result}");
    assert_eq!(
        latest_invocation_terminal_reason(&fixture, "claude-age166-clean"),
        None
    );
    assert_eq!(fixture.exhausted_row_count("claude-age166-clean"), 0);
    assert_eq!(
        fixture.successful_invocation_count_without_terminal_reason("claude-age166-clean"),
        1
    );
    assert_eq!(line_count(&marker), 1);
}

#[test]
fn one_shot_failed_invocation_completion_scan_before_classification() {
    let fixture = Age153Fixture::new();
    let transcript = fixture.dir.path().join("one-shot-nonzero-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture.dir.path().join("one-shot-nonzero-turn.txt");
    let provider = fixture.write_script(
        "one-shot-nonzero-turn.sh",
        &assistant_turn_capture_body(&marker, &transcript, 7),
    );
    write_capture_pool(
        &fixture,
        "age166-nonzero-turn",
        &[("claude-age166-nonzero", &provider)],
        &[("claude-age166-nonzero", &transcript)],
    );

    let output =
        fixture.run_one_shot_with_env("age166-nonzero-turn", &[(FORCE_KIND, "NonzeroExit")]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_no_maybe_marker(&stderr);
    assert_eq!(
        fixture.failed_invocation_count("claude-age166-nonzero", "exit_nonzero"),
        1
    );
    assert_eq!(
        failed_invocation_error_category(&fixture, "claude-age166-nonzero", "exit_nonzero")
            .as_deref(),
        None
    );
    let provider_session_id = latest_provider_session_id(&fixture, "claude-age166-nonzero");
    let counts = fixture
        .open_db()
        .count_session_turns("claude-age166-nonzero", &provider_session_id)
        .unwrap();
    assert_eq!(counts.assistant, 1);
    assert_eq!(fixture.exhausted_row_count("claude-age166-nonzero"), 0);
    assert_eq!(line_count(&marker), 1);
}

#[test]
fn one_shot_provider_without_session_id_does_not_emit_maybe_quota() {
    let fixture = Age153Fixture::new();
    let marker = fixture.dir.path().join("one-shot-no-session-id.txt");
    fixture.write_model("age166-no-session-id", &["openai-compatible-age166"]);
    fixture.write_providers_with_bodies(&[(
        "openai-compatible-age166",
        &success_body(&marker, "ordinary sessionless success"),
    )]);

    let output = fixture.run_one_shot_with_env("age166-no-session-id", &[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_no_terminal_marker_on_stdout(&output);
    let (stdout, stderr) = output_text(&output);
    assert_no_maybe_marker(&stderr);
    let result = assert_result_envelope_shape(&stdout);
    assert_eq!(result["success"], true);
    assert!(result["terminal_reason"].is_null(), "{result}");
    assert_eq!(fixture.exhausted_row_count("openai-compatible-age166"), 0);
    assert_eq!(
        fixture.successful_invocation_count_without_terminal_reason("openai-compatible-age166"),
        1
    );
}

#[test]
fn quota_exhausted_inband_semantics_regression_e2e() {
    let fixture = Age153Fixture::new();
    let first_marker = fixture.dir.path().join("one-shot-inband-a.txt");
    let sibling_marker = fixture.dir.path().join("one-shot-inband-b.txt");
    fixture.write_model(
        "age166-inband-regression",
        &["claude-age166-inband-a", "claude-age166-inband-b"],
    );
    fixture.write_providers_with_bodies(&[
        (
            "claude-age166-inband-a",
            &zero_turn_capture_body(&first_marker, 42),
        ),
        (
            "claude-age166-inband-b",
            &success_body(&sibling_marker, "inband sibling ran"),
        ),
    ]);

    let output = fixture.run_one_shot_with_env(
        "age166-inband-regression",
        &[(FORCE_KIND, "QuotaExhaustedInband")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert!(
        terminal_signal_lines(&stderr)
            .iter()
            .any(|line| line.contains("\"kind\":\"QuotaExhaustedInband\"")),
        "{stderr}"
    );
    assert_eq!(fixture.exhausted_row_count("claude-age166-inband-a"), 1);
    assert_eq!(
        fixture.failed_invocation_count("claude-age166-inband-a", "quota_exhausted_inband"),
        1
    );
    assert_eq!(
        failed_invocation_error_category(
            &fixture,
            "claude-age166-inband-a",
            "quota_exhausted_inband"
        )
        .as_deref(),
        Some("quota_exhausted")
    );
    assert_eq!(line_count(&sibling_marker), 1);
}

// AGE-166 F3: when the openai_compat provider IS configured with
// `session_capture.forced_flag_verified` (so a session id is known
// at start), zero-turn detection wires through to MaybeQuotaExhausted +
// VerifySameProvider exactly like the claude path.
//
// The provider name does not start with "claude" or "codex" so
// `ProviderRecognizer::for_provider` routes to OpenAiCompat.
#[test]
fn one_shot_openai_compat_first_zero_turn_resumes_same_provider_without_exhausted_write() {
    let fixture = Age153Fixture::new();
    let transcript = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-first-zero-turn.jsonl");
    fs::write(&transcript, "").unwrap();
    let marker = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-first-zero-turn.txt");
    let counter = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-first-zero-turn-count.txt");
    let provider = fixture.write_script(
        "one-shot-oai-compat-first-zero-turn.sh",
        &zero_turn_then_assistant_turn_capture_body(&marker, &transcript, &counter),
    );
    write_capture_pool(
        &fixture,
        "age166-oai-compat-first-zero-turn",
        &[("openai-compat-age166-zt-a", &provider)],
        &[("openai-compat-age166-zt-a", &transcript)],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-oai-compat-first-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_with_zero_turn_evidence(&stderr);
    assert_eq!(
        latest_invocation_terminal_reason(&fixture, "openai-compat-age166-zt-a").as_deref(),
        None
    );
    assert_eq!(fixture.exhausted_row_count("openai-compat-age166-zt-a"), 0);
    let sessions = provider_invocation_sessions(&fixture, "openai-compat-age166-zt-a");
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(sessions[1], sessions[0], "{sessions:?}");
    assert_eq!(line_count(&marker), 2);
}

// AGE-166 F3: second consecutive zero-turn on an openai_compat provider
// (with session_capture) confirms quota exhaustion and migrates exactly
// like the claude path. Proves the confirmation key carries the openai_compat
// provider identity and `mark_provider_exhausted` flips `exhausted_at`.
#[test]
fn one_shot_openai_compat_second_zero_turn_confirms_quota_and_migrates() {
    let fixture = Age153Fixture::new();
    let first_transcript = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-second-a.jsonl");
    let sibling_transcript = fixture
        .dir
        .path()
        .join("one-shot-oai-compat-second-b.jsonl");
    fs::write(&first_transcript, "").unwrap();
    fs::write(&sibling_transcript, "").unwrap();
    let first_marker = fixture.dir.path().join("one-shot-oai-compat-second-a.txt");
    let sibling_marker = fixture.dir.path().join("one-shot-oai-compat-second-b.txt");
    let first = fixture.write_script(
        "one-shot-oai-compat-second-a.sh",
        &zero_turn_capture_body(&first_marker, 1),
    );
    let sibling = fixture.write_script(
        "one-shot-oai-compat-second-b.sh",
        &assistant_turn_stdout_capture_body(
            &sibling_marker,
            &sibling_transcript,
            "sibling openai-compat provider ran",
            23,
        ),
    );
    write_capture_pool(
        &fixture,
        "age166-oai-compat-second-zero-turn",
        &[
            ("openai-compat-age166-zt-a", &first),
            ("openai-compat-age166-zt-b", &sibling),
        ],
        &[
            ("openai-compat-age166-zt-a", &first_transcript),
            ("openai-compat-age166-zt-b", &sibling_transcript),
        ],
    );

    let output = fixture.run_one_shot_with_env(
        "age166-oai-compat-second-zero-turn",
        &[(FORCE_KIND, "MaybeQuotaExhausted")],
    );

    assert_no_terminal_marker_on_stdout(&output);
    let (_, stderr) = output_text(&output);
    assert_maybe_marker_with_zero_turn_evidence(&stderr);
    assert_eq!(fixture.exhausted_row_count("openai-compat-age166-zt-a"), 1);
    assert_eq!(
        failed_invocation_error_category(
            &fixture,
            "openai-compat-age166-zt-a",
            "maybe_quota_exhausted"
        )
        .as_deref(),
        Some("quota_exhausted")
    );
    let sessions = provider_invocation_sessions(&fixture, "openai-compat-age166-zt-a");
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    assert!(sessions[0].is_some(), "{sessions:?}");
    assert_eq!(sessions[1], sessions[0], "{sessions:?}");
    assert_eq!(line_count(&first_marker), 2);
    assert_eq!(line_count(&sibling_marker), 1);
    assert_eq!(output.status.code(), Some(23), "{output:?}");
}
