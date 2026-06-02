use super::QuotaScriptWindow;
use chrono::DateTime;
use serde::Deserialize;

/// Script output shape — prefer the new `windows` array, fall back to the
/// old flat `{used_percent, resets_at}` shape so existing scripts keep working.
#[derive(Debug, Deserialize)]
struct QuotaScriptOutput {
    /// New multi-window shape. One entry per rolling window the CLI exposes.
    #[serde(default)]
    windows: Option<Vec<QuotaScriptWindow>>,
    /// Legacy single-window shape.
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<String>,
}

pub fn parse_output(stdout: &str) -> Result<Vec<QuotaScriptWindow>, String> {
    let trimmed = stdout.trim();
    let parsed = parse_json_output(trimmed, stdout)?;
    let raw_windows = output_windows(parsed, stdout)?;
    validated_windows(raw_windows, stdout)
}

fn parse_json_output(trimmed: &str, stdout: &str) -> Result<QuotaScriptOutput, String> {
    serde_json::from_str(trimmed).map_err(|e| format_invalid_json(e, stdout))
}

fn output_windows(
    parsed: QuotaScriptOutput,
    stdout: &str,
) -> Result<Vec<QuotaScriptWindow>, String> {
    match parsed.windows {
        Some(ws) => Ok(ws),
        None => legacy_output_windows(parsed.used_percent, parsed.resets_at, stdout),
    }
}

fn legacy_output_windows(
    used_percent: Option<f64>,
    resets_at: Option<String>,
    stdout: &str,
) -> Result<Vec<QuotaScriptWindow>, String> {
    let (pct, resets_at) = validate_legacy_fields(used_percent, resets_at, stdout)?;
    Ok(vec![legacy_window(pct, resets_at)])
}

fn validate_legacy_fields(
    used_percent: Option<f64>,
    resets_at: Option<String>,
    stdout: &str,
) -> Result<(f64, String), String> {
    let Some(pct) = used_percent else {
        return Err(format_missing_windows_and_percent(stdout));
    };
    let Some(resets_at) = resets_at else {
        return Err(format_legacy_missing_resets_at(stdout));
    };
    Ok((pct, resets_at))
}

fn legacy_window(used_percent: f64, resets_at: String) -> QuotaScriptWindow {
    QuotaScriptWindow {
        window_id: 0,
        used_percent,
        resets_at,
        label: None,
        limit: None,
        remaining: None,
        unit: None,
    }
}

fn validated_windows(
    raw_windows: Vec<QuotaScriptWindow>,
    stdout: &str,
) -> Result<Vec<QuotaScriptWindow>, String> {
    let mut out = Vec::with_capacity(raw_windows.len());
    for (index, mut w) in raw_windows.into_iter().enumerate() {
        validate_window(&w, stdout)?;
        assign_window_id(&mut w, index);
        out.push(w);
    }
    Ok(out)
}

fn validate_window(window: &QuotaScriptWindow, stdout: &str) -> Result<(), String> {
    validate_resets_at(&window.resets_at)?;
    validate_used_percent(window.used_percent, stdout)
}

fn validate_resets_at(resets_at: &str) -> Result<(), String> {
    DateTime::parse_from_rfc3339(resets_at)
        .map(|_| ())
        .map_err(|e| format_bad_resets_at(resets_at, e))
}

fn validate_used_percent(used_percent: f64, stdout: &str) -> Result<(), String> {
    if (0.0..=100.0).contains(&used_percent) && !used_percent.is_nan() {
        return Ok(());
    }
    Err(format_used_percent_out_of_range(used_percent, stdout))
}

fn assign_window_id(window: &mut QuotaScriptWindow, index: usize) {
    window.window_id = index as u32;
}

fn format_invalid_json(error: serde_json::Error, stdout: &str) -> String {
    format!("Invalid JSON from quota script: {error} (got: {stdout})")
}

fn format_missing_windows_and_percent(stdout: &str) -> String {
    format!("quota script emitted neither `windows` nor `used_percent` (got: {stdout})")
}

fn format_legacy_missing_resets_at(stdout: &str) -> String {
    format!("legacy quota script emitted `used_percent` without `resets_at` (got: {stdout})")
}

fn format_bad_resets_at(resets_at: &str, error: chrono::ParseError) -> String {
    format!("Bad resets_at {resets_at}: {error}")
}

fn format_used_percent_out_of_range(used_percent: f64, stdout: &str) -> String {
    format!("quota script emitted used_percent={used_percent} outside 0..100 (got: {stdout})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_multi_window_output() {
        let json = r#"{"windows":[
            {"used_percent":1, "resets_at":"2026-04-23T19:00:00Z"},
            {"used_percent":86, "resets_at":"2026-04-17T15:00:00Z"}
        ]}"#;
        let windows = parse_output(json).unwrap();
        assert_eq!(windows.len(), 2);
        assert!((windows[0].used_percent - 1.0).abs() < 1e-6);
        assert!((windows[1].used_percent - 86.0).abs() < 1e-6);
    }

    #[test]
    fn parse_legacy_single_window_output() {
        let json = r#"{"used_percent":12, "resets_at":"2026-04-23T19:00:00Z"}"#;
        let windows = parse_output(json).unwrap();
        assert_eq!(windows.len(), 1);
        assert!((windows[0].used_percent - 12.0).abs() < 1e-6);
    }

    #[test]
    fn parse_rejects_legacy_without_resets_at() {
        let json = r#"{"used_percent":12}"#;
        assert!(parse_output(json).is_err());
    }

    #[test]
    fn parse_rejects_used_percent_above_100() {
        let json = r#"{"windows":[{"used_percent":150, "resets_at":"2026-04-23T19:00:00Z"}]}"#;
        let err = parse_output(json).unwrap_err();
        assert!(err.contains("outside 0..100"), "unexpected error: {err}");
    }

    #[test]
    fn parse_rejects_negative_used_percent() {
        let json = r#"{"windows":[{"used_percent":-1, "resets_at":"2026-04-23T19:00:00Z"}]}"#;
        let err = parse_output(json).unwrap_err();
        assert!(err.contains("outside 0..100"), "unexpected error: {err}");
    }
}
