#![cfg(unix)]

mod age153_support;

use age153_support::{Age153Fixture, CHAIN_ID, SESSION_ID, toml_string};
use rusqlite::params;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

const MODEL: &str = "provider-turn-recovery";
const PROVIDER: &str = "fixture-recovery-provider";
const DIAGNOSTICS_MODEL: &str = "provider-turn-recovery-diagnostics";
const DIAGNOSTICS_PROVIDER: &str = "fixture-recovery-diagnostics-provider";
const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";
const EXTERNAL_MODEL: &str = "provider-turn-recovery-external";
const EXTERNAL_PROVIDER: &str = "fixture-recovery-external-provider";
const EXTERNAL_LAUNCH_CANARY: &str = "AGE270_EXTERNAL_LAUNCH_CANARY";
const EXTERNAL_MODE: &str = "AGE270_EXTERNAL_MODE";

#[derive(Debug, PartialEq, Eq)]
struct PersistedInvocationOutcome {
    invocation_uuid: String,
    status: String,
    success: i64,
    exit_code: i64,
    error_category: Option<String>,
    terminal_reason: Option<String>,
    resume_acceptance_status: Option<String>,
    resume_acceptance_evidence: Option<String>,
    provider_session_id: Option<String>,
}

struct RecoveryFixture {
    base: Age153Fixture,
    completion_marker: PathBuf,
    diagnostics_marker: PathBuf,
}

impl RecoveryFixture {
    fn new(turn_mode: &str) -> Self {
        let base = Age153Fixture::new();
        let completion_marker = base.dir.path().join("completion-mode");
        let diagnostics_marker = base.dir.path().join("diagnostics-ran");
        let provider = base.write_script(
            "recovery-provider.sh",
            &provider_script(&completion_marker, turn_mode),
        );
        let diagnostics = base.write_script(
            "recovery-diagnostics.sh",
            &format!(
                "printf ran > {}\nprintf 'unknown\\n'",
                shell_path(&diagnostics_marker)
            ),
        );
        let turn_script = base.write_script(
            "recovery-turns.sh",
            &turn_script(&completion_marker, turn_mode),
        );
        write_fixture_config(&base, &provider, &diagnostics, &turn_script);
        Self {
            base,
            completion_marker,
            diagnostics_marker,
        }
    }

    fn run(&self, forced_kind: Option<&str>) -> Output {
        let envs = forced_kind
            .map(|kind| vec![(FORCE_KIND, kind)])
            .unwrap_or_default();
        self.base.run_one_shot_with_env(MODEL, &envs)
    }

    fn run_resume(&self) -> Output {
        self.base.run_resume(MODEL)
    }

    fn latest_invocation(&self) -> (String, i64, Option<String>) {
        self.base
            .conn()
            .query_row(
                "SELECT status, exit_code, terminal_reason
                 FROM invocations
                 WHERE provider_name = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                params![PROVIDER],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
    }

    fn latest_persisted_invocation(&self) -> PersistedInvocationOutcome {
        latest_persisted_invocation(&self.base, PROVIDER)
    }
}

struct ExternalRecoveryFixture {
    base: Age153Fixture,
    launch_canary: PathBuf,
    poison_canary: PathBuf,
    mode: &'static str,
}

impl ExternalRecoveryFixture {
    fn new(mode: &'static str) -> Self {
        let base = Age153Fixture::new();
        let launch_canary = base.dir.path().join("external-launch-ran");
        let poison_canary = base.dir.path().join("legacy-provider-ran");
        let external_provider = write_fixture_executable(
            &base,
            "recovery-external-provider.py",
            external_provider_script(),
        );
        let poison_provider = base.write_script(
            "recovery-legacy-poison.sh",
            &format!("printf poison > {}\nexit 97", shell_path(&poison_canary)),
        );
        let turn_script = base.write_script("recovery-external-turns.sh", external_turn_script());
        write_external_fixture_config(&base, &external_provider, &poison_provider, &turn_script);
        Self {
            base,
            launch_canary,
            poison_canary,
            mode,
        }
    }

    fn run_resume(&self) -> Output {
        let launch_canary = self.launch_canary.to_string_lossy();
        self.base.run_resume_with_env(
            EXTERNAL_MODEL,
            &[
                (EXTERNAL_LAUNCH_CANARY, launch_canary.as_ref()),
                (EXTERNAL_MODE, self.mode),
            ],
        )
    }

    fn latest_persisted_invocation(&self) -> PersistedInvocationOutcome {
        latest_persisted_invocation(&self.base, EXTERNAL_PROVIDER)
    }

    fn assert_local_external_provider_only(&self) {
        assert!(
            self.launch_canary.exists(),
            "fixture-local external provider launch must run"
        );
        assert!(
            !self.poison_canary.exists(),
            "legacy account command must remain unselected"
        );
    }
}

#[test]
fn same_session_new_stop_recovers_logically_and_retains_physical_failure() {
    let fixture = RecoveryFixture::new("stop");

    let output = fixture.run(None);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let envelope = result_envelope(&output);
    assert_eq!(envelope["success"], true);
    assert_eq!(envelope["exit_code"], 1);
    assert_eq!(envelope["terminal_reason"], "exit_nonzero");
    assert_eq!(
        fixture.latest_invocation(),
        ("succeeded".to_string(), 1, Some("exit_nonzero".to_string()))
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"kind\":\"NonzeroExit\""), "{stderr}");
    assert!(
        !fixture.diagnostics_marker.exists(),
        "recovered completion must bypass failure diagnostics"
    );
    assert!(fixture.completion_marker.exists());
}

#[test]
fn resumed_clean_exit_after_new_tool_calls_boundary_is_non_success() {
    let fixture = RecoveryFixture::new("clean-tool-calls");
    fixture.base.seed_active_chain(PROVIDER, MODEL);

    let output = fixture.run_resume();
    let persisted = fixture.latest_persisted_invocation();
    let envelope = optional_result_envelope(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let observed = (
        output.status.success(),
        stderr.contains("incomplete_tool_boundary"),
        envelope
            .as_ref()
            .and_then(|result| result["status"].as_str())
            .map(str::to_string),
        envelope
            .as_ref()
            .and_then(|result| result["success"].as_bool()),
        envelope
            .as_ref()
            .and_then(|result| result["exit_code"].as_i64()),
        envelope
            .as_ref()
            .and_then(|result| result["terminal_reason"].as_str())
            .map(str::to_string),
    );
    let expected = (
        false,
        true,
        Some("failed".to_string()),
        Some(false),
        Some(0),
        Some("incomplete_tool_boundary".to_string()),
    );

    assert_eq!(
        observed,
        expected,
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        persisted,
        PersistedInvocationOutcome {
            invocation_uuid: persisted.invocation_uuid.clone(),
            status: "failed".to_string(),
            success: 0,
            exit_code: 0,
            error_category: Some("incomplete_tool_boundary".to_string()),
            terminal_reason: Some("incomplete_tool_boundary".to_string()),
            resume_acceptance_status: Some("rejected".to_string()),
            resume_acceptance_evidence: Some("incomplete_tool_boundary".to_string()),
            provider_session_id: Some(SESSION_ID.to_string()),
        }
    );

    assert!(
        stderr.contains("earlier provider error evidence retained"),
        "{stderr}"
    );
    let envelope = single_result_envelope(&output);
    assert_failure_envelope_matches(&envelope, &persisted, PROVIDER, SESSION_ID, CHAIN_ID);
}

#[test]
fn resumed_clean_exit_after_loaded_tool_boundary_is_typed_incomplete() {
    let fixture = ExternalRecoveryFixture::new("loaded-incomplete");
    fixture
        .base
        .seed_active_chain(EXTERNAL_PROVIDER, EXTERNAL_MODEL);

    let output = fixture.run_resume();
    let persisted = fixture.latest_persisted_invocation();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(persisted.status, "failed");
    assert_eq!(persisted.success, 0);
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(
        persisted.error_category.as_deref(),
        Some("incomplete_tool_boundary")
    );
    assert_eq!(
        persisted.terminal_reason.as_deref(),
        Some("incomplete_tool_boundary")
    );
    assert_eq!(
        persisted.resume_acceptance_status.as_deref(),
        Some("rejected")
    );
    assert_eq!(
        persisted.resume_acceptance_evidence.as_deref(),
        Some("incomplete_tool_boundary")
    );
    assert!(stderr.contains("incomplete_tool_boundary"), "{stderr}");
    assert!(
        !stderr.contains("resume_completion_unconfirmed"),
        "{stderr}"
    );
    let envelope = single_result_envelope(&output);
    assert_failure_envelope_matches(
        &envelope,
        &persisted,
        EXTERNAL_PROVIDER,
        SESSION_ID,
        CHAIN_ID,
    );
    fixture.assert_local_external_provider_only();
}

#[test]
fn resumed_clean_exit_without_terminal_completion_is_unconfirmed_failure() {
    let fixture = ExternalRecoveryFixture::new("unconfirmed");
    fixture
        .base
        .seed_active_chain(EXTERNAL_PROVIDER, EXTERNAL_MODEL);

    let output = fixture.run_resume();
    let persisted = fixture.latest_persisted_invocation();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert_eq!(persisted.status, "failed");
    assert_eq!(persisted.success, 0);
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(
        persisted.error_category.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    assert_eq!(
        persisted.terminal_reason.as_deref(),
        Some("resume_completion_unconfirmed")
    );
    assert_eq!(
        persisted.resume_acceptance_status.as_deref(),
        Some("accepted")
    );
    assert!(
        persisted
            .resume_acceptance_evidence
            .as_deref()
            .is_some_and(|evidence| evidence.contains("exact session and prompt SHA-256")),
        "{:?}",
        persisted.resume_acceptance_evidence
    );
    assert!(
        stderr.contains("fixture provider stderr retained"),
        "{stderr}"
    );
    assert!(stderr.contains("resume_completion_unconfirmed"), "{stderr}");
    assert!(!stderr.contains("incomplete_tool_boundary"), "{stderr}");
    assert_eq!(
        fixture
            .base
            .invocation_count_with_terminal_reason("incomplete_tool_boundary"),
        0
    );
    let envelope = single_result_envelope(&output);
    assert_failure_envelope_matches(
        &envelope,
        &persisted,
        EXTERNAL_PROVIDER,
        SESSION_ID,
        CHAIN_ID,
    );
    fixture.assert_local_external_provider_only();
}

#[test]
fn resumed_clean_exit_with_terminal_assistant_response_succeeds() {
    let fixture = ExternalRecoveryFixture::new("confirmed");
    fixture
        .base
        .seed_active_chain(EXTERNAL_PROVIDER, EXTERNAL_MODEL);

    let output = fixture.run_resume();
    let persisted = fixture.latest_persisted_invocation();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(
        stdout.contains("fixture terminal assistant response"),
        "{stdout}"
    );
    assert_eq!(persisted.status, "succeeded");
    assert_eq!(persisted.success, 1);
    assert_eq!(persisted.exit_code, 0);
    assert_eq!(persisted.error_category, None);
    assert_eq!(persisted.terminal_reason, None);
    let envelope = single_result_envelope(&output);
    assert_success_envelope_matches(&envelope, &persisted);
    assert!(
        !stderr.contains("resume_completion_unconfirmed"),
        "{stderr}"
    );
    assert!(!stderr.contains("incomplete_tool_boundary"), "{stderr}");
    fixture.assert_local_external_provider_only();
}

#[test]
fn stale_missing_wrong_session_and_non_stop_completion_remain_failed() {
    for mode in [
        "stale",
        "missing",
        "partial",
        "error",
        "wrong-session",
        "baseline-missing",
        "degraded",
        "new-stop-then-error",
    ] {
        let fixture = RecoveryFixture::new(mode);
        let output = fixture.run(None);
        assert_ne!(output.status.code(), Some(0), "mode={mode} {output:?}");
        let envelope = result_envelope(&output);
        assert_eq!(envelope["success"], false, "mode={mode}");
        assert_eq!(envelope["exit_code"], 1, "mode={mode}");
        assert_eq!(fixture.latest_invocation().0, "failed", "mode={mode}");
    }
}

#[test]
fn typed_terminal_failures_cannot_be_recovered_by_new_stop() {
    for kind in [
        "QuotaExhaustedInband",
        "RateLimited",
        "ProlongedSilence",
        "SignalExit",
        "Unknown",
    ] {
        let fixture = RecoveryFixture::new("stop");
        let output = fixture.run(Some(kind));
        assert_ne!(output.status.code(), Some(0), "kind={kind} {output:?}");
        assert_eq!(fixture.latest_invocation().0, "failed", "kind={kind}");
    }
}

fn latest_persisted_invocation(
    fixture: &Age153Fixture,
    provider: &str,
) -> PersistedInvocationOutcome {
    fixture
        .conn()
        .query_row(
            "SELECT invocation_uuid, status, success, exit_code, error_category,
                    terminal_reason, resume_acceptance_status,
                    resume_acceptance_evidence, provider_session_id
             FROM invocations
             WHERE provider_name = ?1
             ORDER BY id DESC
             LIMIT 1",
            params![provider],
            |row| {
                Ok(PersistedInvocationOutcome {
                    invocation_uuid: row.get(0)?,
                    status: row.get(1)?,
                    success: row.get(2)?,
                    exit_code: row.get(3)?,
                    error_category: row.get(4)?,
                    terminal_reason: row.get(5)?,
                    resume_acceptance_status: row.get(6)?,
                    resume_acceptance_evidence: row.get(7)?,
                    provider_session_id: row.get(8)?,
                })
            },
        )
        .unwrap()
}

fn write_external_fixture_config(
    fixture: &Age153Fixture,
    external_provider: &Path,
    poison_provider: &Path,
    turn_script: &Path,
) {
    fs::write(
        fixture.models_dir.join(format!("{EXTERNAL_MODEL}.toml")),
        format!(
            r#"provider = {{ path = {} }}
prompt_mode = "arg"

[[providers]]
name = {EXTERNAL_PROVIDER:?}
args = []
"#,
            toml_string(&external_provider.display().to_string()),
        ),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        format!(
            r#"[{EXTERNAL_PROVIDER}]
command = {}
args = []
prompt_mode = "arg"
"#,
            toml_string(&poison_provider.display().to_string()),
        ),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("sessions.toml"),
        format!(
            "[{EXTERNAL_PROVIDER}]\nturn_script = {}\n",
            toml_string(&turn_script.display().to_string())
        ),
    )
    .unwrap();
}

fn write_fixture_executable(fixture: &Age153Fixture, name: &str, body: &str) -> PathBuf {
    let path = fixture.dir.path().join(name);
    fs::write(&path, body).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn external_turn_script() -> &'static str {
    r#"printf '{"session_id":"%s","turn_id":"old-stop","timestamp":"2026-07-21T00:00:00Z","role":"assistant","completion_outcome":"stop"}\n' "$SESSION_ID"
if [ ! -f "${AGE270_EXTERNAL_LAUNCH_CANARY:-}" ]; then
  exit 0
fi
if [ "${AGE270_EXTERNAL_MODE:-}" = "loaded-incomplete" ]; then
  printf '{"session_id":"%s","turn_id":"new-tool-calls","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"tool-calls"}\n' "$SESSION_ID"
  printf '{"session_id":"%s","turn_id":"new-outcome-less","timestamp":"2026-07-21T00:00:02Z","role":"assistant"}\n' "$SESSION_ID"
fi"#
}

fn external_provider_script() -> &'static str {
    r#"#!/usr/bin/env python3
import base64
import hashlib
import json
import os
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
PROMPT_ACCEPTANCE = "oulipoly.prompt_acceptance/v1"
PROMPT_ACCEPTED_MARKER = "oulipoly.prompt_accepted/v1"
SESSION_ID = "5169694d-de0f-40d1-890c-6e28e55bab27"

subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{}")

def request_id():
    return request.get("request_id", "age270-request")

def envelope(result):
    return {
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(),
        "ok": True,
        "result": result,
    }

def error(code):
    return {
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(),
        "ok": False,
        "error": {
            "category": "failed",
            "code": code,
            "message": code,
            "retryable": False,
        },
    }

def describe():
    return envelope({
        "provider_id": "age270-local-external-provider",
        "display_name": "AGE-270 Local External Provider",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
            "launch": True,
            "prompt_acceptance_v1": True,
            "policy": True,
            "quota": False,
            "session": True,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        },
    })

def policy_evaluate():
    return envelope({
        "accepted": True,
        "env": {},
        "stdin": None,
        "prompt": None,
        "diagnostics": [],
        "markers": [],
    })

def emit(event):
    print(json.dumps(event, separators=(",", ":")), flush=True)

def stream_event(seq, kind, text):
    return {
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": kind,
        "data_base64": base64.b64encode(text.encode("utf-8")).decode("ascii"),
    }

def marker_event(seq, name, value):
    return {
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": name,
        "value": value,
    }

def exit_event(seq, session_id):
    return {
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {"kind": "exited", "code": 0},
        "terminal_signal": {
            "kind": "clean_exit",
            "evidence": "fixture clean exit",
            "observed_at_unix_ms": 1000 + seq,
        },
        "session": {
            "provider_session_id": session_id,
            "state": {"cursor": "after-age270-launch"},
        },
    }

def launch():
    pathlib.Path(os.environ["AGE270_EXTERNAL_LAUNCH_CANARY"]).write_text("ran")
    params = request.get("params", {})
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    session_id = params.get("session", {}).get("known_provider_session_id") or SESSION_ID
    mode = os.environ.get("AGE270_EXTERNAL_MODE", "unconfirmed")
    seq = 1
    emit(stream_event(seq, "stderr", "fixture provider stderr retained\n"))
    seq += 1
    if mode == "confirmed":
        emit(stream_event(seq, "stdout", "fixture terminal assistant response\n"))
        seq += 1
    acceptance = params.get("prompt_acceptance", {})
    value = {
        "protocol": PROMPT_ACCEPTANCE,
        "provider_session_id": session_id,
        "prompt_sha256": hashlib.sha256(prompt.encode("utf-8")).hexdigest(),
        "source": "age270.fixture",
        "message_id": "age270-prompt-accepted",
    }
    if acceptance.get("delivery_nonce"):
        value["delivery_nonce"] = acceptance["delivery_nonce"]
    emit(marker_event(seq, PROMPT_ACCEPTED_MARKER, value))
    seq += 1
    if mode == "confirmed":
        emit(marker_event(seq, "oulipoly.produced_assistant_response", True))
        seq += 1
    emit(exit_event(seq, session_id))

def session_id_from_request():
    params = request.get("params", {})
    extra = params.get("extra", {})
    return params.get("session_id") or extra.get("start_bound_provider_session_id") or SESSION_ID

def read_turns():
    session_id = session_id_from_request()
    return envelope({
        "turns": [{
            "session_id": session_id,
            "turn_id": "age270-session-read",
            "role": "assistant",
            "timestamp": "2026-07-21T00:00:02Z",
            "body": [{"type": "text", "text": "fixture session turn"}],
        }],
        "turn_count": 1,
        "complete": True,
    })

def capture():
    return envelope({
        "provider_session_id": session_id_from_request(),
        "state": {"captured": True},
        "artifacts": [],
    })

if subcommand == "describe":
    print(json.dumps(describe()))
elif subcommand == "policy.evaluate":
    print(json.dumps(policy_evaluate()))
elif subcommand == "launch":
    launch()
elif subcommand == "session.read_turns":
    print(json.dumps(read_turns()))
elif subcommand == "session.capture":
    print(json.dumps(capture()))
else:
    print(json.dumps(error("unsupported_subcommand")))
"#
}

fn write_fixture_config(
    fixture: &Age153Fixture,
    provider: &Path,
    diagnostics: &Path,
    turn_script: &Path,
) {
    fs::write(
        fixture.models_dir.join(format!("{MODEL}.toml")),
        format!("[[providers]]\nname = {PROVIDER:?}\nargs = []\n"),
    )
    .unwrap();
    fs::write(
        fixture.models_dir.join(format!("{DIAGNOSTICS_MODEL}.toml")),
        format!("[[providers]]\nname = {DIAGNOSTICS_PROVIDER:?}\nargs = []\n"),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("config.toml"),
        format!("diagnostics_model = {DIAGNOSTICS_MODEL:?}\n"),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("providers.toml"),
        format!(
            r#"[{PROVIDER}]
command = {}
args = []
prompt_mode = "arg"

[{PROVIDER}.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[{PROVIDER}.resume]
kind = "flag"
flag = "--resume"

[{DIAGNOSTICS_PROVIDER}]
command = {}
args = []
prompt_mode = "arg"
"#,
            toml_string(&provider.display().to_string()),
            toml_string(&diagnostics.display().to_string()),
        ),
    )
    .unwrap();
    fs::write(
        fixture.app_config_dir.join("sessions.toml"),
        format!(
            "[{PROVIDER}]\nturn_script = {}\n",
            toml_string(&turn_script.display().to_string())
        ),
    )
    .unwrap();
}

fn provider_script(marker: &Path, turn_mode: &str) -> String {
    let exit_code = if turn_mode == "clean-tool-calls" {
        0
    } else {
        1
    };
    format!(
        r#"session_id=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--session-id" ] || [ "$1" = "--resume" ]; then
    session_id="$2"
    shift 2
  else
    shift
  fi
done
printf '%s' {turn_mode:?} > {}
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$session_id"
printf 'earlier provider error evidence retained\n' >&2
exit {exit_code}"#,
        shell_path(marker)
    )
}

fn turn_script(marker: &Path, turn_mode: &str) -> String {
    let baseline = if turn_mode == "baseline-missing" {
        String::new()
    } else {
        r#"printf '{"session_id":"%s","turn_id":"old-stop","timestamp":"2026-07-21T00:00:00Z","role":"assistant","completion_outcome":"stop"}\n' "$SESSION_ID""#.to_string()
    };
    format!(
        r#"{baseline}
if [ ! -f {} ]; then
  exit 0
fi
mode="$(cat {})"
case "$mode" in
  stale) ;;
  missing)
    printf '{{"session_id":"%s","turn_id":"new-missing","timestamp":"2026-07-21T00:00:01Z","role":"assistant"}}\n' "$SESSION_ID"
    ;;
  partial|clean-tool-calls)
    printf '{{"session_id":"%s","turn_id":"new-partial","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"tool-calls"}}\n' "$SESSION_ID"
    ;;
  error)
    printf '{{"session_id":"%s","turn_id":"new-error","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"error"}}\n' "$SESSION_ID"
    ;;
  wrong-session)
    printf '{{"session_id":"%s-other","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    ;;
  degraded)
    printf '{{"session_id":"%s","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    printf '{{"degraded":true,"count":2}}\n'
    ;;
  new-stop-then-error)
    printf '{{"session_id":"%s","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    printf '{{"session_id":"%s","turn_id":"new-error","timestamp":"2026-07-21T00:00:02Z","role":"assistant","completion_outcome":"error"}}\n' "$SESSION_ID"
    ;;
  *)
    printf '{{"session_id":"%s","turn_id":"new-stop","timestamp":"2026-07-21T00:00:01Z","role":"assistant","completion_outcome":"stop"}}\n' "$SESSION_ID"
    ;;
esac"#,
        shell_path(marker),
        shell_path(marker),
    )
}

fn result_envelope(output: &Output) -> Value {
    single_result_envelope(output)
}

fn optional_result_envelope(output: &Output) -> Option<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .map(|line| serde_json::from_str(line).unwrap())
}

fn single_result_envelope(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "expected exactly one OULIPOLY_RESULT line in stdout:\n{stdout}"
    );
    serde_json::from_str(lines[0]).unwrap()
}

fn assert_failure_envelope_matches(
    envelope: &Value,
    persisted: &PersistedInvocationOutcome,
    provider: &str,
    provider_session_id: &str,
    chain_id: &str,
) {
    assert_eq!(envelope_keys(envelope), failure_envelope_keys());
    assert_eq!(envelope["status"].as_str(), Some(persisted.status.as_str()));
    assert_eq!(envelope["success"].as_bool(), Some(false));
    assert_eq!(envelope["success"].as_bool(), Some(persisted.success != 0));
    assert_eq!(envelope["exit_code"].as_i64(), Some(persisted.exit_code));
    assert_eq!(
        envelope["error_category"].as_str(),
        persisted.error_category.as_deref()
    );
    assert_eq!(
        envelope["terminal_reason"].as_str(),
        persisted.terminal_reason.as_deref()
    );
    assert_eq!(
        envelope["id"].as_str(),
        Some(persisted.invocation_uuid.as_str())
    );
    assert_eq!(envelope["agent_runner_invocation_id"], envelope["id"]);
    assert_eq!(envelope["provider_name"].as_str(), Some(provider));
    assert_eq!(
        envelope["provider_session_id"].as_str(),
        Some(provider_session_id)
    );
    assert_eq!(
        persisted.provider_session_id.as_deref(),
        Some(provider_session_id)
    );
    assert_eq!(envelope["agent_runner_chain_id"].as_str(), Some(chain_id));
}

fn assert_success_envelope_matches(envelope: &Value, persisted: &PersistedInvocationOutcome) {
    assert_eq!(envelope_keys(envelope), common_envelope_keys());
    assert_eq!(envelope["status"].as_str(), Some(persisted.status.as_str()));
    assert_eq!(envelope["success"].as_bool(), Some(true));
    assert_eq!(envelope["success"].as_bool(), Some(persisted.success != 0));
    assert_eq!(envelope["exit_code"].as_i64(), Some(persisted.exit_code));
    assert_eq!(
        envelope["error_category"].as_str(),
        persisted.error_category.as_deref()
    );
    assert_eq!(
        envelope["terminal_reason"].as_str(),
        persisted.terminal_reason.as_deref()
    );
    assert_eq!(
        envelope["id"].as_str(),
        Some(persisted.invocation_uuid.as_str())
    );
}

fn envelope_keys(envelope: &Value) -> BTreeSet<&str> {
    envelope
        .as_object()
        .expect("result envelope object")
        .keys()
        .map(String::as_str)
        .collect()
}

fn common_envelope_keys() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "error_category",
        "exit_code",
        "finished_at",
        "id",
        "status",
        "success",
        "terminal_reason",
    ])
}

fn failure_envelope_keys() -> BTreeSet<&'static str> {
    let mut keys = common_envelope_keys();
    keys.extend([
        "agent_runner_chain_id",
        "agent_runner_invocation_id",
        "provider_name",
        "provider_session_id",
    ]);
    keys
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.display())
}
