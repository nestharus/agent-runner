//! ## Declared roles
//!
//! `formatter`

pub(crate) fn format_model_not_found_error(name: &str) -> String {
    format!("Model '{name}' not found")
}

pub(crate) fn format_unexpected_diagnostics_output_error() -> String {
    "Diagnostics service returned unexpected output".to_string()
}

pub(crate) fn diagnostic_input(stderr: &str, stdout: &[u8]) -> String {
    let stdout = String::from_utf8_lossy(stdout);
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    match (stderr.is_empty(), stdout.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stderr.to_string(),
        (true, false) => stdout.to_string(),
        (false, false) => format!("{stderr}\n{stdout}"),
    }
}
