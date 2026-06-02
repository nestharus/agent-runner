//! ## Declared roles
//!
//! `formatter`

pub(crate) fn format_model_not_found_error(name: &str) -> String {
    format!("Model '{name}' not found")
}

pub(crate) fn format_unexpected_diagnostics_output_error() -> String {
    "Diagnostics service returned unexpected output".to_string()
}
