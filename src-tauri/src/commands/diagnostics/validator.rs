//! Declared roles: validator

use oulipoly_runtime::diagnostics::Diagnosis;
use oulipoly_runtime::services::DiagnosticsServiceOutput;

pub(super) fn diagnostics_output_diagnosis(
    output: DiagnosticsServiceOutput,
) -> Result<Diagnosis, String> {
    match output {
        DiagnosticsServiceOutput::Diagnosis { diagnosis } => Ok(diagnosis),
        DiagnosticsServiceOutput::ExhaustionClassification { .. } => {
            Err("diagnostics service returned exhaustion classification".to_string())
        }
    }
}
