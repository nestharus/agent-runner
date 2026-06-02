use chrono::{TimeZone, Utc};
use oulipoly_runtime::quota::{QuotaScriptWindow, parse_output, run_refresh_command, run_script};

#[test]
fn parse_output_pins_malformed_json_error_string() {
    let stdout = "{";

    let err = parse_output(stdout).unwrap_err();

    assert_eq!(
        err,
        "Invalid JSON from quota script: EOF while parsing an object at line 1 column 1 (got: {)"
    );
}

#[test]
fn parse_output_pins_missing_shape_error_string() {
    let stdout = r#"{"other":true}"#;

    let err = parse_output(stdout).unwrap_err();

    assert_eq!(
        err,
        r#"quota script emitted neither `windows` nor `used_percent` (got: {"other":true})"#
    );
}

#[test]
fn parse_output_pins_legacy_used_percent_without_resets_at_error_string() {
    let stdout = r#"{"used_percent":12}"#;

    let err = parse_output(stdout).unwrap_err();

    assert_eq!(
        err,
        r#"legacy quota script emitted `used_percent` without `resets_at` (got: {"used_percent":12})"#
    );
}

#[test]
fn parse_output_pins_bad_rfc3339_resets_at_error_string() {
    let stdout = r#"{"used_percent":12,"resets_at":"tomorrow"}"#;

    let err = parse_output(stdout).unwrap_err();

    assert_eq!(err, "Bad resets_at tomorrow: premature end of input");
}

#[test]
fn parse_output_pins_out_of_range_percentage_error_string() {
    let stdout = r#"{"windows":[{"used_percent":150,"resets_at":"2026-04-23T19:00:00Z"}]}"#;

    let err = parse_output(stdout).unwrap_err();

    assert_eq!(
        err,
        r#"quota script emitted used_percent=150 outside 0..100 (got: {"windows":[{"used_percent":150,"resets_at":"2026-04-23T19:00:00Z"}]})"#
    );
}

#[test]
fn parse_output_overwrites_emitted_window_ids_by_parsed_order() {
    let stdout = r#"{"windows":[
        {"window_id":99,"used_percent":1,"resets_at":"2026-04-23T19:00:00Z"},
        {"window_id":42,"used_percent":86,"resets_at":"2026-04-17T15:00:00Z"}
    ]}"#;

    let windows = parse_output(stdout).unwrap();

    assert_eq!(windows[0].window_id, 0);
    assert_eq!(windows[1].window_id, 1);
}

#[test]
fn quota_script_window_to_quota_window_input_normalizes_percent_and_converts_timestamp_to_utc() {
    let window = QuotaScriptWindow {
        window_id: 7,
        used_percent: 42.5,
        resets_at: "2026-04-23T19:00:00+02:00".to_string(),
        label: Some("weekly".to_string()),
        limit: Some(1000),
        remaining: Some(575),
        unit: Some("requests".to_string()),
    };

    let input = window.to_quota_window_input();

    assert!((input.used_percent - 0.425).abs() < f64::EPSILON);
    assert_eq!(
        input.resets_at,
        Utc.with_ymd_and_hms(2026, 4, 23, 17, 0, 0).unwrap()
    );
}

#[test]
fn run_script_pins_non_zero_stderr_error_string() {
    let err = run_script("printf 'quota denied\\n' >&2; exit 7").unwrap_err();

    assert_eq!(err, "Quota script exited 7: quota denied");
}

#[test]
fn run_refresh_command_pins_non_zero_stderr_error_string() {
    let err = run_refresh_command("printf 'refresh denied\\n' >&2; exit 9").unwrap_err();

    assert_eq!(err, "auth_refresh_command exited 9: refresh denied");
}
