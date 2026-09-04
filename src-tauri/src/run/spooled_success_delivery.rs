//! Shared CLI delivery boundary for sealed external-provider success output.
//!
//! Declared roles: `orchestration`, `formatter`.

use std::io::Write as _;

use oulipoly_runtime::executor::ExecutionOutputSpool;
use oulipoly_state::StateDb;

use crate::invocation::result_envelope::emit_result_envelope_to;

pub(super) fn deliver(
    spool: &ExecutionOutputSpool,
    invocation_id: &str,
    exit_code: i32,
    error_category: Option<&str>,
    terminal_reason: Option<&str>,
) -> std::io::Result<()> {
    // Provider streams are opaque and may contain matching result markers. The
    // final control write gains provenance only from occurring after both
    // complete spools have been replayed and provider stdout has been flushed.
    let mut stderr = std::io::stderr().lock();
    spool.write_stderr_to(&mut stderr)?;
    stderr.flush()?;

    let summary = spool.summary()?;
    let control_separator_required = (summary.stdout_bytes > 0
        && !spool.stdout_ends_with_newline())
        || (summary.stderr_bytes > 0 && !spool.stderr_ends_with_newline());
    drop(stderr);

    let mut stdout = std::io::stdout().lock();
    spool.write_stdout_to(&mut stdout)?;
    stdout.flush()?;
    drop(stdout);

    let mut stderr = std::io::stderr().lock();
    if control_separator_required {
        stderr.write_all(b"\n")?;
    }
    emit_result_envelope_to(
        &mut stderr,
        invocation_id,
        true,
        exit_code,
        error_category,
        terminal_reason,
        None,
    )
}

pub(super) fn settle(
    state: &StateDb,
    invocation_row_id: i64,
    spooled: bool,
    delivery: std::io::Result<()>,
) -> bool {
    if let Err(error) = delivery {
        if let Err(state_error) = state.mark_invocation_output_delivery_failed(
            invocation_row_id,
            "payload_or_control",
            &format!("{:?}", error.kind()),
            None,
        ) {
            emit_diagnostic(&format!(
                "failed to record provider output delivery failure: {state_error}"
            ));
        }
        emit_diagnostic(&format!("failed to deliver provider output: {error}"));
        return false;
    }

    if spooled && let Err(error) = state.mark_invocation_output_delivered(invocation_row_id) {
        emit_diagnostic(&format!(
            "failed to record provider output delivery: {error}"
        ));
        return false;
    }

    true
}

fn emit_diagnostic(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}
