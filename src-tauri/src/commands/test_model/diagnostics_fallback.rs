//! ## Declared roles
//!
//! `orchestration`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/test_model/diagnostics_fallback.rs
//!     role: adapter
//!     Translates:
//!       - terminal-text diagnostics-fallback decision contract
//!       - local diagnostic-input duplicate contract
//!       - diagnostics classify-exhaustion request contract
//!       - diagnostics output validation contract
//!       - fallback disposition result contract
//! ```

//! This island preserves the terminal-text diagnostics fallback and its local
//! diagnostic-input duplicate for S10/S11. It intentionally does not call
//! `redaction::diagnostic_input`.

use oulipoly_runtime::executor::ExecutionResult;

use super::{dispatch, mapper::TestModelServices, validator};

pub(crate) fn diagnostics_fallback_should_mark_exhausted(
    services: &TestModelServices<'_>,
    result: &ExecutionResult,
) -> Result<bool, String> {
    let input = diagnostic_input(&result.stderr, &result.stdout);
    let output = dispatch::diagnostics_output_for_result(services.diagnostics_service, input)?;
    let is_exhausted = validator::validate_diagnostics_output_variant(output)?;
    Ok(validator::diagnostics_output_is_quota_exhausted(
        is_exhausted,
    ))
}

pub fn diagnostic_input(stderr: &str, stdout: &[u8]) -> String {
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
