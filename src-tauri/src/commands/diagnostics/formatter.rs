//! Declared roles: formatter

use oulipoly_runtime::diagnostics::{Diagnosis, non_quota_failure_diagnosis};

pub(super) fn render_diagnostics_result(
    diagnosis: Result<Diagnosis, String>,
    provider_exit_code: i32,
    provider_output: &str,
) -> Option<String> {
    match diagnosis {
        Ok(diagnosis) => {
            emit_diagnostics_success(&diagnosis);
            Some(diagnostics_category_name(&diagnosis))
        }
        Err(e) => {
            emit_diagnostics_failure(&e);
            let failure = super::mapper::diagnostics_failure(&e, provider_exit_code);
            emit_diagnostics_failure_marker(&failure);
            let original = non_quota_failure_diagnosis(provider_output, provider_exit_code)?;
            emit_diagnostics_success(&original);
            Some(diagnostics_category_name(&original))
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

fn emit_diagnostics_failure_marker(failure: &super::mapper::DiagnosticsFailure) {
    match diagnostics_failure_marker_json(failure) {
        Ok(json) => eprintln!("OULIPOLY_DIAGNOSTIC_FAILURE={json}"),
        Err(error) => eprintln!("Warning: Failed to serialize diagnostic failure: {error}"),
    }
}

fn diagnostics_failure_marker_json(
    failure: &super::mapper::DiagnosticsFailure,
) -> Result<String, serde_json::Error> {
    serde_json::to_string(failure)
}

fn diagnostics_category_name(diagnosis: &Diagnosis) -> String {
    diagnosis.category.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_request_failure_preserves_original_network_category() {
        let category = render_diagnostics_result(
            Err("external provider protocol failed: registry_lookup".to_string()),
            17,
            "connection refused for active resume provider",
        );
        assert_eq!(category.as_deref(), Some("network_error"));
    }

    #[test]
    fn secondary_error_text_cannot_classify_original_or_infer_exhaustion() {
        for original in ["unclassified failure", "quota exceeded; HTTP 429 rate limited"] {
            assert_eq!(
                render_diagnostics_result(Err("connection refused".to_string()), 17, original),
                None
            );
        }
    }

    #[test]
    fn successful_diagnosis_keeps_its_category() {
        let category = render_diagnostics_result(
            Ok(Diagnosis {
                category: oulipoly_runtime::diagnostics::ErrorCategory::ProviderProtocol,
                summary: "primary protocol error".to_string(),
            }),
            17,
            "connection refused",
        );
        assert_eq!(category.as_deref(), Some("provider_protocol"));
    }

    #[test]
    fn diagnostic_failure_marker_records_secondary_operation_separately() {
        let failure = super::super::mapper::diagnostics_failure(
            "external provider protocol failed: registry_lookup",
            1,
        );
        let payload: serde_json::Value = serde_json::from_str(
            &diagnostics_failure_marker_json(&failure).expect("serialize diagnostic failure"),
        )
        .expect("parse diagnostic failure");

        assert_eq!(payload["stage"], "diagnostics");
        assert_eq!(payload["operation"], "registry_lookup");
        assert_eq!(payload["error_category"], "provider_protocol");
        assert_eq!(payload["provider_exit_code"], 1);
        assert_eq!(
            payload["message"],
            "external provider protocol failed: registry_lookup"
        );
    }
}
