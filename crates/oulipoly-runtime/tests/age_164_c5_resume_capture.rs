//! AGE-164 cluster 5 — resume acceptance + session capture (ACR-251 PP-007/008/009).
//!
//! Pins the CURRENT pre-refactor behavior of the resume/capture pull sites.
//! The refactor MUST preserve the implicit canonical schemas declared here:
//!
//! ### Forced-flag verified stdout JSONL (PP-007)
//! - Lossy line iteration; JSON-only line filtering.
//! - `{"type":"result","session_id":<string>}` → success.
//! - `{"type":"system","subtype":"init","session_id":<string>}` → success.
//! - `system.init` missing `session_id` → error
//!   "system.init event missing session_id".
//! - No matching event → error
//!   "stdout did not contain a result or system.init session_id event".
//!
//! ### Stdout JSON event session capture (PP-008)
//! - Lossy line iteration; JSON-only line filtering.
//! - `type == event_type` match.
//! - Dotted `event_id_path` lookup.
//! - Missing-path → error "event '<event_type>' missing id path '<event_id_path>'".
//! - No matching event → error "stdout did not contain event '<event_type>'".
//!
//! ### Resume missing-session phrase set (PP-009)
//! - Lowercase substring matching for `"no conversation found"` and
//!   `"no session found"`; the mapped result evidence is
//!   `"resume_session_mismatch: provider reported missing session"`.
//!
//! These constitute the ACR-251 canonical-doc-as-schema declaration for
//! the three pull sites. Step 6c MUST publish the same schema via rustdoc
//! on the extracted parser functions or as a module-level doc-comment.
//!
//! ## Declared roles
//!
//! Roles: formatter, mapper, parser, validator.
//!
//! - formatter: fixture scripts and canonical evidence strings.
//! - mapper: provider/resume/session-capture fixture construction.
//! - parser: fixture JSONL and last-message sidecar observations.
//! - validator: public resume and capture behavior assertions.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age_164_c5_resume_capture.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly-config-resume-session-capture-test-contract
//!       - executor-cli-resume-capture-public-api-contract
//!       - provider-stdout-jsonl-fixture-contract
//!       - runtime-last-message-sidecar-fixture-contract
//! ```

#![cfg(unix)]

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeAcceptanceRules, ResumeKind, ResumeStrategy,
    SessionCapture, SessionCaptureKind,
};
use oulipoly_runtime::executor::cli::{
    ResumePayload, compose_resume_args, execute, execute_resume, start_known_provider_session_id,
};
use oulipoly_runtime::executor::{ResumeAcceptanceStatus, SessionCaptureMethod};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("provider.sh");
    std::fs::write(&path, provider_script_body(body)).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    (dir, path)
}

fn provider_script_body(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn provider_for(path: &std::path::Path) -> ProviderConfig {
    ProviderConfig::new(path.to_string_lossy().into_owned(), Vec::new())
}

fn flag_strategy() -> ResumeStrategy {
    ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    }
}

fn subcommand_strategy() -> ResumeStrategy {
    ResumeStrategy {
        kind: ResumeKind::Subcommand,
        flag: None,
        subcommand: Some(vec!["resume".to_string(), "--id".to_string()]),
    }
}

// ===========================================================================
// compose_resume_args — public formatter contract.
// ===========================================================================

#[test]
fn compose_resume_args_flag_kind_returns_flag_then_session_id() {
    let strategy = flag_strategy();
    let args = compose_resume_args(&strategy, "abc-123").unwrap();
    assert_eq!(args, vec!["--resume", "abc-123"]);
}

#[test]
fn compose_resume_args_subcommand_kind_returns_subcommand_then_session_id() {
    let strategy = subcommand_strategy();
    let args = compose_resume_args(&strategy, "xyz").unwrap();
    assert_eq!(args, vec!["resume", "--id", "xyz"]);
}

#[test]
fn compose_resume_args_flag_kind_missing_flag_errors() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: None,
        subcommand: None,
    };
    let err = compose_resume_args(&strategy, "abc").unwrap_err();
    assert_eq!(err, "resume.flag is required");
}

#[test]
fn compose_resume_args_subcommand_kind_missing_subcommand_errors() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Subcommand,
        flag: None,
        subcommand: None,
    };
    let err = compose_resume_args(&strategy, "abc").unwrap_err();
    assert_eq!(err, "resume.subcommand is required");
}

#[test]
fn compose_resume_args_subcommand_kind_empty_subcommand_errors() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Subcommand,
        flag: None,
        subcommand: Some(Vec::new()),
    };
    let err = compose_resume_args(&strategy, "abc").unwrap_err();
    assert_eq!(err, "resume.subcommand is required");
}

#[test]
fn resume_flag_kind_without_flag_is_rejected_with_canonical_error() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: None,
        subcommand: None,
    };
    let err = compose_resume_args(&strategy, "abc").unwrap_err();
    assert_eq!(err, "resume.flag is required");
}

#[test]
fn resume_subcommand_kind_without_subcommand_is_rejected_with_canonical_error() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Subcommand,
        flag: None,
        subcommand: None,
    };
    let err = compose_resume_args(&strategy, "abc").unwrap_err();
    assert_eq!(err, "resume.subcommand is required");
}

#[test]
fn resume_subcommand_kind_with_empty_subcommand_is_rejected_with_canonical_error() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Subcommand,
        flag: None,
        subcommand: Some(Vec::new()),
    };
    let err = compose_resume_args(&strategy, "abc").unwrap_err();
    assert_eq!(err, "resume.subcommand is required");
}

#[test]
fn resume_flag_kind_formats_as_flag_value_pair() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };
    let args = compose_resume_args(&strategy, "abc").unwrap();
    assert_eq!(args, vec!["--resume", "abc"]);
}

#[test]
fn resume_subcommand_kind_formats_as_subcommand_with_session_id_tail() {
    let strategy = ResumeStrategy {
        kind: ResumeKind::Subcommand,
        flag: None,
        subcommand: Some(vec!["resume".to_string(), "--id".to_string()]),
    };
    let args = compose_resume_args(&strategy, "abc").unwrap();
    assert_eq!(args, vec!["resume", "--id", "abc"]);
}

#[test]
fn execute_resume_propagates_resume_flag_required_error() {
    let provider = ProviderConfig::new("echo", Vec::new());
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: None,
        subcommand: None,
    };
    let err = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "abc",
            strategy: &strategy,
        },
    )
    .unwrap_err();
    assert_eq!(err, "resume.flag is required");
}

// ===========================================================================
// start_known_provider_session_id — public capture preflight.
// ===========================================================================

#[test]
fn start_known_provider_session_id_none_when_no_session_capture() {
    let provider = ProviderConfig::new("echo", Vec::new());
    assert_eq!(start_known_provider_session_id(&provider).unwrap(), None);
}

#[test]
fn start_known_provider_session_id_none_for_stdout_json_event_kind() {
    let mut provider = ProviderConfig::new("echo", Vec::new());
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::StdoutJsonEvent,
        flag: None,
        readback_args: None,
        event_type: Some("agent.session_started".to_string()),
        event_id_path: Some("data.id".to_string()),
        json_flag: Some("--json".to_string()),
        last_message_flag: Some("--last-message".to_string()),
    });
    // StdoutJsonEvent always returns None (no preflight session id).
    assert_eq!(start_known_provider_session_id(&provider).unwrap(), None);
}

#[test]
fn start_known_provider_session_id_some_uuid_for_forced_flag_verified_kind() {
    let mut provider = ProviderConfig::new("echo", Vec::new());
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let id = start_known_provider_session_id(&provider)
        .unwrap()
        .expect("forced_flag_verified returns Some");
    // It's a UUID v4 string — 36 chars with dashes at fixed positions.
    assert_eq!(id.len(), 36);
    assert_eq!(id.chars().nth(8), Some('-'));
}

#[test]
fn start_known_provider_session_id_forced_flag_verified_missing_flag_errors() {
    let mut provider = ProviderConfig::new("echo", Vec::new());
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: None,
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let err = start_known_provider_session_id(&provider).unwrap_err();
    assert_eq!(err, "session_capture.flag is required");
}

// ===========================================================================
// ACR-251 PP-007: forced-flag verified stdout JSONL schema.
// ===========================================================================

#[test]
fn acr251_pp007_forced_flag_verified_recognizes_system_init_event() {
    // The fixture echoes a system.init line whose session_id mirrors the
    // injected flag value. The recognizer must accept the match.
    let (_dir, path) = script(
        r#"
requested=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) requested="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$requested"
"#,
    );
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let model = ModelConfig {
        name: "pp007-init-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::ForcedFlagVerified
    ));
    assert!(result.session_capture.session_id.is_some());
}

#[test]
fn acr251_pp007_forced_flag_verified_recognizes_result_event() {
    // Mirror the requested session_id from a `{"type":"result", ...}` event.
    let (_dir, path) = script(
        r#"
requested=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) requested="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf '{"type":"result","session_id":"%s"}\n' "$requested"
"#,
    );
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let model = ModelConfig {
        name: "pp007-result-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::ForcedFlagVerified
    ));
    assert!(result.session_capture.session_id.is_some());
}

#[test]
fn acr251_pp007_forced_flag_verified_no_event_fails_with_canonical_message() {
    let (_dir, path) = script("printf 'no events here\\n'");
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let model = ModelConfig {
        name: "pp007-no-event-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    match result.session_capture.method {
        SessionCaptureMethod::Failed(reason) => {
            assert!(
                reason.contains("stdout did not contain a result or system.init session_id event"),
                "{reason}"
            );
        }
        other => panic!("expected Failed; got {other:?}"),
    }
}

#[test]
fn acr251_pp007_forced_flag_verified_system_init_missing_session_id_errors() {
    let (_dir, path) = script(r#"printf '{"type":"system","subtype":"init"}\n'"#);
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let model = ModelConfig {
        name: "pp007-missing-session-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    match result.session_capture.method {
        SessionCaptureMethod::Failed(reason) => {
            assert!(
                reason.contains("system.init event missing session_id"),
                "{reason}"
            );
        }
        other => panic!("expected Failed; got {other:?}"),
    }
}

#[test]
fn acr251_pp007_forced_flag_verified_skips_malformed_lines_before_valid_event() {
    let (_dir, path) = script(
        r#"
requested=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) requested="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf 'plain text line\n'
printf '{not-json\n'
printf '{"type":"system","subtype":"init","session_id":"%s"}\n' "$requested"
"#,
    );
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let model = ModelConfig {
        name: "pp007-skip-malformed-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::ForcedFlagVerified
    ));
}

// ===========================================================================
// ACR-251 PP-008: stdout JSON event session capture schema.
// ===========================================================================

#[test]
fn acr251_pp008_stdout_json_event_uses_dotted_id_path_lookup() {
    // The provider emits a typed event whose id lives at a NESTED dotted
    // path. The capture parser must walk the path through `lookup_json_path`.
    let (_dir, path) = script(
        r#"
last_message_path=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --last-message) last_message_path="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf 'restored body' > "$last_message_path"
printf '{"type":"agent.session_started","data":{"id":"nested-uuid"}}\n'
"#,
    );
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::StdoutJsonEvent,
        flag: None,
        readback_args: None,
        event_type: Some("agent.session_started".to_string()),
        event_id_path: Some("data.id".to_string()),
        json_flag: Some("--json".to_string()),
        last_message_flag: Some("--last-message".to_string()),
    });
    let model = ModelConfig {
        name: "pp008-nested-path-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::StdoutJsonEvent
    ));
    assert_eq!(
        result.session_capture.session_id.as_deref(),
        Some("nested-uuid")
    );
    // PP-008 also restores the last-message file as plain stdout.
    assert_eq!(result.stdout, b"restored body");
}

#[test]
fn acr251_pp008_stdout_json_event_no_event_fails_with_canonical_message() {
    let (_dir, path) = script(
        r#"
last_message_path=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --last-message) last_message_path="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf 'no last message' > "$last_message_path"
printf 'no events here\n'
"#,
    );
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::StdoutJsonEvent,
        flag: None,
        readback_args: None,
        event_type: Some("agent.session_started".to_string()),
        event_id_path: Some("data.id".to_string()),
        json_flag: Some("--json".to_string()),
        last_message_flag: Some("--last-message".to_string()),
    });
    let model = ModelConfig {
        name: "pp008-no-event-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    match result.session_capture.method {
        SessionCaptureMethod::Failed(reason) => {
            assert!(
                reason.contains("stdout did not contain event 'agent.session_started'"),
                "{reason}"
            );
        }
        other => panic!("expected Failed; got {other:?}"),
    }
}

#[test]
fn acr251_pp008_stdout_json_event_missing_id_path_fails_with_canonical_message() {
    let (_dir, path) = script(
        r#"
last_message_path=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --last-message) last_message_path="$2"; shift 2 ;;
    *) shift ;;
  esac
done
printf 'last-message' > "$last_message_path"
printf '{"type":"agent.session_started","data":{}}\n'
"#,
    );
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::StdoutJsonEvent,
        flag: None,
        readback_args: None,
        event_type: Some("agent.session_started".to_string()),
        event_id_path: Some("data.id".to_string()),
        json_flag: Some("--json".to_string()),
        last_message_flag: Some("--last-message".to_string()),
    });
    let model = ModelConfig {
        name: "pp008-missing-id-path-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");
    match result.session_capture.method {
        SessionCaptureMethod::Failed(reason) => {
            assert!(
                reason.contains("event 'agent.session_started' missing id path 'data.id'"),
                "{reason}"
            );
        }
        other => panic!("expected Failed; got {other:?}"),
    }
}

// ===========================================================================
// ACR-251 PP-009: resume missing-session phrase set.
// ===========================================================================

#[test]
fn acr251_pp009_no_conversation_found_lowercase_matches() {
    let (_dir, path) =
        script(r#"printf 'No conversation found for session whatever\n' >&2; exit 1"#);
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Rejected);
    assert_eq!(
        acceptance.evidence.as_deref(),
        Some("resume_session_mismatch: provider reported missing session")
    );
}

#[test]
fn acr251_pp009_no_session_found_lowercase_matches() {
    let (_dir, path) = script(r#"printf 'NO SESSION FOUND\n' >&2; exit 1"#);
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Rejected);
    assert_eq!(
        acceptance.evidence.as_deref(),
        Some("resume_session_mismatch: provider reported missing session")
    );
}

#[test]
fn no_conversation_found_phrase_maps_to_resume_session_mismatch() {
    let session_id = "11111111-1111-1111-1111-111111111111";
    let (_dir, path) =
        script(r#"printf 'No conversation found for session whatever\n' >&2; exit 1"#);
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id,
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Rejected);
    assert!(
        acceptance
            .evidence
            .as_deref()
            .unwrap_or("")
            .contains("resume_session_mismatch"),
        "{acceptance:?}"
    );
}

#[test]
fn no_session_found_phrase_maps_to_resume_session_mismatch() {
    let session_id = "11111111-1111-1111-1111-111111111111";
    let (_dir, path) = script(r#"printf 'Error: NO SESSION FOUND. bye.\n' >&2; exit 1"#);
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id,
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Rejected);
    assert!(
        acceptance
            .evidence
            .as_deref()
            .unwrap_or("")
            .contains("resume_session_mismatch"),
        "{acceptance:?}"
    );
}

// ===========================================================================
// Resume acceptance rules: configured accept/reject patterns and
// `{session_id}` placeholder expansion.
// ===========================================================================

#[test]
fn resume_rejected_pattern_wins_over_exit_code() {
    let (_dir, path) = script(r#"printf 'fatal: stale conversation\n'; exit 0"#);
    let mut provider = provider_for(&path);
    provider.resume_acceptance = Some(ResumeAcceptanceRules {
        accepted_output_patterns: None,
        rejected_output_patterns: Some(vec!["stale conversation".to_string()]),
    });
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Rejected);
    assert_eq!(
        acceptance.evidence.as_deref(),
        Some("matched reject pattern: stale conversation")
    );
}

#[test]
fn resume_accepted_pattern_with_session_id_expansion_marks_accepted() {
    let session_id = "11111111-1111-1111-1111-111111111111";
    let (_dir, path) = script(&format!("printf 'resumed session {session_id}\\n'; exit 0"));
    let mut provider = provider_for(&path);
    provider.resume_acceptance = Some(ResumeAcceptanceRules {
        accepted_output_patterns: Some(vec!["resumed session {session_id}".to_string()]),
        rejected_output_patterns: None,
    });
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id,
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Accepted);
}

#[test]
fn pattern_matches_output_expands_session_id_placeholder() {
    let session_id = "11111111-1111-1111-1111-111111111111";
    let (_dir, path) = script(&format!("printf 'resumed session {session_id}\\n'; exit 0"));
    let mut provider = provider_for(&path);
    provider.resume_acceptance = Some(ResumeAcceptanceRules {
        accepted_output_patterns: Some(vec!["resumed session {session_id}".to_string()]),
        rejected_output_patterns: None,
    });
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id,
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Accepted);
    let evidence = acceptance.evidence.unwrap_or_default();
    assert!(
        evidence.contains("matched accept pattern: resumed session {session_id}"),
        "evidence={evidence}"
    );
}

#[test]
fn resume_rules_no_match_with_zero_exit_returns_unconfirmed_with_no_accepted_evidence() {
    let (_dir, path) = script("printf 'silent ok\\n'; exit 0");
    let mut provider = provider_for(&path);
    provider.resume_acceptance = Some(ResumeAcceptanceRules {
        accepted_output_patterns: Some(vec!["explicit acceptance".to_string()]),
        rejected_output_patterns: None,
    });
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Unconfirmed);
    assert_eq!(
        acceptance.evidence.as_deref(),
        Some("child exited 0 but no accepted resume pattern matched")
    );
}

#[test]
fn resume_no_rules_with_zero_exit_returns_unconfirmed_with_no_rules_evidence() {
    let (_dir, path) = script("printf 'silent ok\\n'; exit 0");
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Unconfirmed);
    assert_eq!(
        acceptance.evidence.as_deref(),
        Some("child exited 0 but provider has no resume_acceptance rules configured")
    );
}

#[test]
fn child_exit_zero_yields_unconfirmed_when_no_rules() {
    let (_dir, path) = script("exit 0");
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Unconfirmed);
}

#[test]
fn resume_nonzero_exit_with_no_match_returns_rejected_with_canonical_evidence() {
    let (_dir, path) = script("printf 'unexpected failure\\n'; exit 9");
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Rejected);
    assert_eq!(
        acceptance.evidence.as_deref(),
        Some("child exited 9; no rejection patterns matched")
    );
}

#[test]
fn child_exit_nonzero_yields_rejected_when_no_rules() {
    let (_dir, path) = script("exit 2");
    let provider = provider_for(&path);
    let strategy = flag_strategy();
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");
    let acceptance = result
        .resume_acceptance
        .expect("resume acceptance populated");
    assert_eq!(acceptance.status, ResumeAcceptanceStatus::Rejected);
    let evidence = acceptance.evidence.unwrap_or_default();
    assert!(evidence.contains("child exited 2"), "evidence={evidence}");
}

// ===========================================================================
// execute_resume forces session_capture to None even if the provider
// declares one.
// ===========================================================================

#[test]
fn execute_resume_forces_session_capture_to_none_in_returned_result() {
    let (_dir, path) = script("exit 0");
    let mut provider = provider_for(&path);
    provider.session_capture = Some(SessionCapture {
        kind: SessionCaptureKind::ForcedFlagVerified,
        flag: Some("--session-id".to_string()),
        readback_args: None,
        event_type: None,
        event_id_path: None,
        json_flag: None,
        last_message_flag: None,
    });
    let strategy = flag_strategy();

    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");

    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::None
    ));
    assert_eq!(result.session_capture.session_id, None);
}
