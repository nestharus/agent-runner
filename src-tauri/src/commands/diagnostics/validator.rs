//! Declared roles: validator

use oulipoly_runtime::diagnostics::Diagnosis;
use oulipoly_runtime::services::DiagnosticsServiceOutput;

pub(super) fn diagnostics_output_diagnosis(
    output: DiagnosticsServiceOutput,
) -> Result<Diagnosis, String> {
    super::mapper::diagnosis_from_output(output)
        .ok_or_else(super::formatter::format_unexpected_exhaustion_classification)
}
