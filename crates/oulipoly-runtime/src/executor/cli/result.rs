//! ## Declared roles
//!
//! Roles: mapper, orchestration.
//!
//! - mapper: [`raw_result_from_supervised_output`] and
//!   [`execution_result_from_raw`] map supervised process output, capture
//!   results, terminal signals, child markers, and returned artifacts onto
//!   executor result DTOs.
//! - orchestration: [`cleanup_temp_files`] sequences prompt/session temp-file
//!   removal while preserving ignored cleanup failures.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/result.rs
//!     role: adapter
//!     Translates:
//!       - supervised-output-contract
//!       - executor-result-dto-contract
//!       - session-capture-result-contract
//!       - returned-artifact-contract
//!       - temp-file-cleanup-lifecycle-contract
//! ```

use super::super::assistant_response::legacy_stdout_produced_assistant_response;
use super::super::{
    CapturedChildInvocation, ExecutionResult, ResumeAcceptanceResult, ReturnedArtifactRef,
    SessionCaptureResult, TerminalSignal,
};
use super::capture_result::{finalize_capture, maybe_restore_plain_stdout};
use super::ipc;
use super::session_capture::{self, remove_unsanctioned_money_fields};
use super::supervision::SupervisedOutput;
use std::path::PathBuf;

pub(super) fn cleanup_temp_files(temp_files: Vec<PathBuf>) {
    for path in temp_files {
        let _ = std::fs::remove_file(path);
    }
}

pub(super) fn execution_result_from_raw(
    result: RawResult,
    provider_index: usize,
    resume_acceptance: Option<ResumeAcceptanceResult>,
    session_capture_override: Option<SessionCaptureResult>,
) -> ExecutionResult {
    let produced_assistant_response = legacy_stdout_produced_assistant_response(&result.stdout);
    ExecutionResult {
        stdout: result.stdout,
        stderr: result.stderr,
        output_spool: None,
        exit_code: result.exit_code,
        provider_index,
        session_capture: session_capture_override.unwrap_or(result.session_capture),
        resume_acceptance,
        terminal_reason: result.terminal_reason,
        terminal_signal: result.terminal_signal,
        produced_assistant_response,
        prompt_acceptance_attestation: None,
        captured_child_invocations: result.captured_child_invocations,
        returned_artifacts: result.returned_artifacts,
    }
}

pub(super) struct RawResult {
    pub(super) stdout: Vec<u8>,
    pub(super) telemetry_stdout: Vec<u8>,
    pub(super) stderr: String,
    pub(super) exit_code: i32,
    pub(super) session_capture: SessionCaptureResult,
    pub(super) terminal_reason: Option<String>,
    pub(super) terminal_signal: Option<TerminalSignal>,
    pub(super) captured_child_invocations: Vec<CapturedChildInvocation>,
    pub(super) returned_artifacts: Vec<ReturnedArtifactRef>,
}

pub(super) fn raw_result_from_supervised_output(
    capture_plan: &session_capture::CapturePlan,
    output: SupervisedOutput,
    returned_artifacts: Vec<ReturnedArtifactRef>,
) -> RawResult {
    let capture_outcome = finalize_capture(
        capture_plan,
        &output.stdout,
        output.streamed_session_id.as_deref(),
    );
    let stdout = maybe_restore_plain_stdout(capture_plan, &capture_outcome, &output.stdout);
    let stdout = remove_unsanctioned_money_fields(stdout);
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let captured_child_invocations = ipc::captured_child_invocations_from_stderr(&stderr);
    RawResult {
        stdout,
        telemetry_stdout: output.stdout.clone(),
        captured_child_invocations,
        stderr,
        exit_code: output.exit_code,
        terminal_reason: output.terminal_reason,
        terminal_signal: Some(output.terminal_signal),
        session_capture: capture_outcome,
        returned_artifacts,
    }
}
