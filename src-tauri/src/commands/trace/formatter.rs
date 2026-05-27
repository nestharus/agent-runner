//! Declared roles: formatter

use oulipoly_runtime::trace::{TraceReport, render_ascii_trace};
use std::path::Path;

pub(super) fn format_trace_sessions_config_load_error(
    sessions_path: &Path,
    error: String,
) -> String {
    format!("Failed to load {}: {error}", sessions_path.display())
}

pub(super) fn format_trace_service_dispatch_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub(super) fn report_trace_invocation_not_found(message: &str) {
    eprintln!("{message}");
}

pub(super) fn render_trace_report(report: &TraceReport, json: bool) -> Result<i32, String> {
    if json {
        let json = serde_json::to_string_pretty(report)
            .map_err(|e| format!("Failed to serialize trace report: {e}"))?;
        println!("{json}");
    } else {
        print!("{}", render_ascii_trace(report));
    }
    Ok(0)
}
