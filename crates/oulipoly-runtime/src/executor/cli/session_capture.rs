//! ## Declared roles
//!
//! Roles: validator, parser, accessor, mapper, formatter, predicate, filter,
//! orchestration.
//!
//! - validator: [`validate_start_known_capture`],
//!   [`required_capture_field`].
//! - parser: [`parse_forced_flag_verified_session_id`],
//!   [`parse_stdout_json_event_session_id`], [`lookup_json_path`].
//! - accessor: [`start_known_session_id_for_capture`],
//!   [`last_message_capture_path`].
//! - mapper: [`build_capture_plan`] maps configured capture strategy onto
//!   runtime capture plans, argv fragments, and temp-file cleanup paths.
//! - formatter: [`forced_flag_capture_args`],
//!   [`stdout_json_event_capture_args`].
//! - predicate: [`looks_like_provider_telemetry`],
//!   [`is_money_field_name`].
//! - filter: [`strip_money_fields`],
//!   [`remove_unsanctioned_money_fields`].
//! - orchestration: [`start_known_provider_session_id`],
//!   [`build_capture_plan`].
//!
//! ## ACR-251 canonical-doc-as-schema declaration (PP-007 + PP-008)
//!
//! The runtime is a documented consumer of two implicit provider-output
//! schemas. Both schemas are pinned here.
//!
//! ### PP-007 — Forced-flag verified stdout JSONL
//!
//! - Input: provider stdout, decoded with `String::from_utf8_lossy`, then
//!   split by `lines()`.
//! - Each line is JSON-parsed. Non-JSON lines are skipped silently.
//! - Recognized event objects (success cases):
//!     - `{"type":"result","session_id":<string>}` → returns `session_id`.
//!     - `{"type":"system","subtype":"init","session_id":<string>}` →
//!       returns `session_id`.
//! - Recognized error cases (exact canonical strings):
//!     - When a `system.init` event is present but missing `session_id`,
//!       error is `"system.init event missing session_id"`.
//!     - When no recognized event is observed, error is
//!       `"stdout did not contain a result or system.init session_id event"`.
//!
//! ### PP-008 — Stdout JSON event session capture
//!
//! - Input: provider stdout, decoded with `String::from_utf8_lossy`, then
//!   split by `lines()`.
//! - Each line is JSON-parsed. Non-JSON lines are skipped silently.
//! - Match rule: object whose `type` field equals the configured
//!   `event_type`.
//! - Path lookup: dotted [`lookup_json_path`] traverses nested objects.
//! - Recognized error cases (exact canonical strings):
//!     - When the event is observed but the dotted id path returns nothing,
//!       error is `"event '<event_type>' missing id path '<event_id_path>'"`.
//!     - When no matching event is observed, error is
//!       `"stdout did not contain event '<event_type>'"`.
//!
//! `tests/age_164_c5_resume_capture.rs` (`acr251_pp007_*` and
//! `acr251_pp008_*`) pins these strings; the push-pull auditor accepts
//! this rustdoc as canonical-doc-as-schema proof for PP-007 and PP-008.
//!
//! ### PP-010 — Provider telemetry money-field redaction
//!
//! - Input: provider stdout, decoded only when it is valid UTF-8. Non-UTF-8
//!   stdout is returned unchanged.
//! - Each newline-delimited segment is JSON-parsed independently. Non-JSON
//!   lines are returned unchanged.
//! - Eligible telemetry objects are objects where either:
//!     - `type` equals `"result"`;
//!     - a top-level `modelUsage` field is present; or
//!     - any object key contains one of the lowercase tokens `cost`, `usd`,
//!       or `price`.
//! - Redaction recursively removes object fields whose lowercase key contains
//!   `cost`, `usd`, or `price`, preserving non-money fields, array structure,
//!   and the original line ending.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture.rs
//!     role: adapter
//!     Translates:
//!       - provider-session-capture-config-contract
//!       - forced-flag-verified-jsonl-contract
//!       - stdout-json-event-session-contract
//!       - provider-telemetry-money-redaction-contract
//!       - runtime-last-message-sidecar-contract
//! ```

use oulipoly_config::{ProviderConfig, SessionCapture, SessionCaptureKind};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub fn start_known_provider_session_id(
    provider: &ProviderConfig,
) -> Result<Option<String>, String> {
    let Some(capture) = provider.session_capture.as_ref() else {
        return Ok(None);
    };
    validate_start_known_capture(capture)?;
    Ok(start_known_session_id_for_capture(capture))
}

fn validate_start_known_capture(capture: &SessionCapture) -> Result<(), String> {
    match capture.kind {
        SessionCaptureKind::ForcedFlagVerified => {
            if capture.flag.is_none() {
                return Err("session_capture.flag is required".to_string());
            }
            Ok(())
        }
        SessionCaptureKind::None | SessionCaptureKind::StdoutJsonEvent => Ok(()),
    }
}

fn start_known_session_id_for_capture(capture: &SessionCapture) -> Option<String> {
    match capture.kind {
        SessionCaptureKind::ForcedFlagVerified => Some(uuid::Uuid::new_v4().to_string()),
        SessionCaptureKind::None | SessionCaptureKind::StdoutJsonEvent => None,
    }
}

pub(super) enum CapturePlan {
    None,
    ForcedFlagVerified {
        requested_session_id: String,
    },
    StdoutJsonEvent {
        event_type: String,
        event_id_path: String,
        last_message_path: PathBuf,
    },
}

pub(super) fn build_capture_plan(
    capture: Option<&SessionCapture>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let Some(capture) = capture else {
        return Ok((CapturePlan::None, Vec::new(), Vec::new()));
    };

    match capture.kind {
        SessionCaptureKind::None => Ok(empty_capture_plan()),
        SessionCaptureKind::ForcedFlagVerified => {
            build_forced_flag_capture_plan(capture, start_known_provider_session_id)
        }
        SessionCaptureKind::StdoutJsonEvent => build_stdout_json_event_capture_plan(capture),
    }
}

fn empty_capture_plan() -> (CapturePlan, Vec<String>, Vec<PathBuf>) {
    (CapturePlan::None, Vec::new(), Vec::new())
}

fn build_forced_flag_capture_plan(
    capture: &SessionCapture,
    start_known_provider_session_id: Option<&str>,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let flag = required_capture_field(&capture.flag, "session_capture.flag")?;
    let requested_session_id = capture_requested_session_id(start_known_provider_session_id);
    let args = forced_flag_capture_args(flag, &requested_session_id, capture);
    Ok((
        CapturePlan::ForcedFlagVerified {
            requested_session_id,
        },
        args,
        Vec::new(),
    ))
}

fn build_stdout_json_event_capture_plan(
    capture: &SessionCapture,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let json_flag = required_capture_field(&capture.json_flag, "session_capture.json_flag")?;
    let last_message_flag = required_capture_field(
        &capture.last_message_flag,
        "session_capture.last_message_flag",
    )?;
    let event_type = required_capture_field(&capture.event_type, "session_capture.event_type")?;
    let event_id_path =
        required_capture_field(&capture.event_id_path, "session_capture.event_id_path")?;
    let last_message_path = last_message_capture_path();
    Ok((
        CapturePlan::StdoutJsonEvent {
            event_type,
            event_id_path,
            last_message_path: last_message_path.clone(),
        },
        stdout_json_event_capture_args(json_flag, last_message_flag, &last_message_path),
        vec![last_message_path],
    ))
}

fn required_capture_field(field: &Option<String>, name: &str) -> Result<String, String> {
    field
        .clone()
        .ok_or_else(|| required_capture_field_message(name))
}

fn required_capture_field_message(name: &str) -> String {
    format!("{name} is required")
}

fn capture_requested_session_id(start_known_provider_session_id: Option<&str>) -> String {
    start_known_provider_session_id
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}

fn forced_flag_capture_args(
    flag: String,
    requested_session_id: &str,
    capture: &SessionCapture,
) -> Vec<String> {
    let mut args = vec![flag, requested_session_id.to_string()];
    if let Some(readback_args) = &capture.readback_args {
        args.extend(readback_args.clone());
    }
    args
}

fn last_message_capture_path() -> PathBuf {
    temp_path_for_filename(last_message_capture_filename())
}

fn last_message_capture_filename() -> String {
    format!("oulipoly-last-message-{}", uuid::Uuid::new_v4())
}

fn temp_path_for_filename(filename: String) -> PathBuf {
    std::env::temp_dir().join(filename)
}

fn stdout_json_event_capture_args(
    json_flag: String,
    last_message_flag: String,
    last_message_path: &Path,
) -> Vec<String> {
    vec![
        json_flag,
        last_message_flag,
        last_message_path.to_string_lossy().into_owned(),
    ]
}

pub(super) fn remove_unsanctioned_money_fields(stdout: Vec<u8>) -> Vec<u8> {
    let scrubbed = {
        let Some(text) = stdout_as_utf8(&stdout) else {
            return stdout;
        };
        scrub_provider_json_lines(text)
    };
    scrubbed.map(String::into_bytes).unwrap_or(stdout)
}

fn stdout_as_utf8(stdout: &[u8]) -> Option<&str> {
    std::str::from_utf8(stdout).ok()
}

fn scrub_provider_json_lines(text: &str) -> Option<String> {
    let mut scrubbed = String::with_capacity(text.len());
    let mut changed = false;

    for line in text.split_inclusive('\n') {
        append_scrubbed_provider_json_line(&mut scrubbed, &mut changed, line);
    }

    changed.then_some(scrubbed)
}

fn append_scrubbed_provider_json_line(scrubbed: &mut String, changed: &mut bool, line: &str) {
    if let Some(line) = scrub_provider_json_line(line) {
        *changed = true;
        scrubbed.push_str(&line);
    } else {
        scrubbed.push_str(line);
    }
}

fn scrub_provider_json_line(line: &str) -> Option<String> {
    let (content, line_ending) = split_line_ending(line);
    let mut value = parse_provider_json_line(content)?;
    scrub_provider_telemetry_value(&mut value)?;
    render_provider_json_line(&value, line_ending)
}

fn parse_provider_json_line(content: &str) -> Option<Value> {
    serde_json::from_str(content).ok()
}

fn scrub_provider_telemetry_value(value: &mut Value) -> Option<()> {
    (looks_like_provider_telemetry(value) && strip_money_fields(value)).then_some(())
}

fn render_provider_json_line(value: &Value, line_ending: &str) -> Option<String> {
    let mut rendered = serde_json::to_string(value).ok()?;
    rendered.push_str(line_ending);
    Some(rendered)
}

fn split_line_ending(line: &str) -> (&str, &str) {
    let Some(without_lf) = line.strip_suffix('\n') else {
        return (line, "");
    };
    if let Some(without_crlf) = without_lf.strip_suffix('\r') {
        (without_crlf, "\r\n")
    } else {
        (without_lf, "\n")
    }
}

fn looks_like_provider_telemetry(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    object.get("type").and_then(Value::as_str) == Some("result")
        || object.contains_key("modelUsage")
        || object.keys().any(|key| is_money_field_name(key))
}

fn strip_money_fields(value: &mut Value) -> bool {
    money_fields_were_removed(strip_money_fields_in_place(value))
}

fn strip_money_fields_in_place(value: &mut Value) -> usize {
    match value {
        Value::Object(object) => {
            let keys_to_remove: Vec<String> = object
                .keys()
                .filter(|key| is_money_field_name(key))
                .cloned()
                .collect();
            let mut removed = keys_to_remove.len();
            for key in keys_to_remove {
                object.remove(&key);
            }
            for child in object.values_mut() {
                removed += strip_money_fields_in_place(child);
            }
            removed
        }
        Value::Array(items) => {
            let mut removed = 0;
            for item in items {
                removed += strip_money_fields_in_place(item);
            }
            removed
        }
        _ => 0,
    }
}

fn money_fields_were_removed(removed_count: usize) -> bool {
    removed_count > 0
}

fn is_money_field_name(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase();
    normalized.contains("cost") || normalized.contains("usd") || normalized.contains("price")
}

/// Parses a forced-flag-verified session id from provider stdout JSONL.
///
/// See module-level PP-007 ACR-251 declaration for the canonical schema.
pub(super) fn parse_forced_flag_verified_session_id(stdout: &[u8]) -> Result<String, String> {
    for line in String::from_utf8_lossy(stdout).lines() {
        let Some(value) = parse_stdout_json_line(line) else {
            continue;
        };
        if value_is_result_event(&value) {
            if let Some(session_id) = json_session_id(&value) {
                return Ok(session_id.to_string());
            }
        }
        if !value_is_system_init_event(&value) {
            continue;
        }
        return required_system_init_session_id(&value);
    }
    Err("stdout did not contain a result or system.init session_id event".to_string())
}

fn parse_stdout_json_line(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line).ok()
}

fn value_is_result_event(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("result")
}

fn json_session_id(value: &Value) -> Option<&str> {
    value.get("session_id").and_then(Value::as_str)
}

fn value_is_system_init_event(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("system")
        && value.get("subtype").and_then(Value::as_str) == Some("init")
}

fn required_system_init_session_id(value: &Value) -> Result<String, String> {
    value
        .get("session_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "system.init event missing session_id".to_string())
}

/// Parses a configured-event session id from provider stdout JSONL.
///
/// See module-level PP-008 ACR-251 declaration for the canonical schema.
pub(super) fn parse_stdout_json_event_session_id(
    stdout: &[u8],
    event_type: &str,
    event_id_path: &str,
) -> Result<String, String> {
    for line in String::from_utf8_lossy(stdout).lines() {
        let Some(value) = parse_stdout_json_line(line) else {
            continue;
        };
        if !value_matches_event_type(&value, event_type) {
            continue;
        }
        return required_configured_event_session_id(&value, event_type, event_id_path);
    }
    Err(stdout_event_missing_message(event_type))
}

fn stdout_event_missing_message(event_type: &str) -> String {
    format!("stdout did not contain event '{event_type}'")
}

fn value_matches_event_type(value: &Value, event_type: &str) -> bool {
    value.get("type").and_then(Value::as_str) == Some(event_type)
}

fn required_configured_event_session_id(
    value: &Value,
    event_type: &str,
    event_id_path: &str,
) -> Result<String, String> {
    lookup_json_path(value, event_id_path)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| configured_event_missing_id_path_message(event_type, event_id_path))
}

fn configured_event_missing_id_path_message(event_type: &str, event_id_path: &str) -> String {
    format!("event '{event_type}' missing id path '{event_id_path}'")
}

fn lookup_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let segments = json_path_segments(path);
    lookup_json_path_segments(value, &segments)
}

fn json_path_segments(path: &str) -> Vec<&str> {
    path.split('.').collect()
}

fn lookup_json_path_segments<'a>(value: &'a Value, segments: &[&str]) -> Option<&'a Value> {
    segments
        .iter()
        .try_fold(value, |current, segment| current.get(*segment))
}
