//! Declared roles: formatter

use oulipoly_runtime::diagnostics::Diagnosis;

pub(super) fn render_diagnostics_result(diagnosis: Result<Diagnosis, String>) -> Option<String> {
    match diagnosis {
        Ok(diagnosis) => {
            emit_diagnostics_success(&diagnosis);
            Some(diagnostics_category_name(&diagnosis))
        }
        Err(e) => {
            emit_diagnostics_failure(&e);
            None
        }
    }
}

pub(super) fn diagnostics_service_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub(super) fn format_unexpected_exhaustion_classification() -> String {
    "diagnostics service returned exhaustion classification".to_string()
}

fn emit_diagnostics_success(diagnosis: &Diagnosis) {
    eprintln!(
        "[diagnostics] {}: {}",
        diagnosis.category.as_str(),
        diagnosis.summary
    );
}

fn emit_diagnostics_failure(error: &str) {
    eprintln!("[diagnostics] Failed to diagnose: {error}");
}

fn diagnostics_category_name(diagnosis: &Diagnosis) -> String {
    diagnosis.category.as_str().to_string()
}
