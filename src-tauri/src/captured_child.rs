//! Captured-child invocation supervision.
//!
//! Relocated from `src-tauri/src/main.rs` by AGE-205 (slice B10 of the AGE-183 main.rs
//! decomposition program; map row H14). Output-preserving: bodies byte-identical to the
//! pre-AGE-205 main.rs definitions; only visibility + import targets change. Given a parent
//! invocation's captured child-invocation markers, this module emits the raw marker lines and
//! finalizes any still-running child rows that belong to the parent with a supervisor reason.
//!
//! ## Declared roles
//!
//! `orchestration`, `accessor`, `predicate`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/captured_child.rs
//!     role: adapter
//!     Translates:
//!       - AGE-205 H14: executor CapturedChildInvocation markers -> stderr raw marker lines
//!       - AGE-205 H14: captured child marker + parent id -> StateDb finalize of matching running child rows
//! ```

use oulipoly_runtime::executor;
use oulipoly_state::{InvocationRecord, InvocationStatus, StateDb};

pub(crate) fn supervise_captured_child_invocations(
    state: &StateDb,
    parent_invocation_id: i64,
    captured: &[executor::CapturedChildInvocation],
    parent_terminal_reason: Option<&str>,
) {
    let supervisor_reason = format_supervisor_reason(parent_terminal_reason);

    for child in captured {
        supervise_captured_child_invocation(state, parent_invocation_id, child, &supervisor_reason);
    }
}

pub(crate) fn emit_captured_child_marker_lines(captured: &[executor::CapturedChildInvocation]) {
    for child in captured {
        eprintln!("{}", child.raw_marker_line);
    }
}

fn inspected_captured_child_row(
    state: &StateDb,
    child: &executor::CapturedChildInvocation,
) -> Result<Option<InvocationRecord>, String> {
    lookup_captured_child_row(state, child)
}

fn captured_child_row_or_warning(
    state: &StateDb,
    child: &executor::CapturedChildInvocation,
) -> Option<InvocationRecord> {
    inspected_captured_child_row(state, child).unwrap_or_else(|err| {
        emit_captured_child_inspection_warning(child, &err);
        None
    })
}

fn supervise_captured_child_invocation(
    state: &StateDb,
    parent_invocation_id: i64,
    child: &executor::CapturedChildInvocation,
    supervisor_reason: &str,
) {
    let Some(row) = captured_child_row_or_warning(state, child) else {
        return;
    };
    if captured_child_matches_parent(&row, parent_invocation_id, child) {
        finalize_captured_child_or_warning(state, &row, child, supervisor_reason);
    }
}

fn lookup_captured_child_row(
    state: &StateDb,
    child: &executor::CapturedChildInvocation,
) -> Result<Option<InvocationRecord>, String> {
    state.get_invocation_by_uuid(&child.composite_id.id)
}

fn emit_captured_child_inspection_warning(child: &executor::CapturedChildInvocation, err: &str) {
    eprintln!(
        "Warning: Failed to inspect captured child invocation {}: {err}",
        child.composite_id.id
    );
}

fn finalize_captured_child_invocation(
    state: &StateDb,
    row: &InvocationRecord,
    supervisor_reason: &str,
) -> Result<(), String> {
    finalize_captured_child_row(state, row, supervisor_reason)
}

fn finalize_captured_child_or_warning(
    state: &StateDb,
    row: &InvocationRecord,
    child: &executor::CapturedChildInvocation,
    supervisor_reason: &str,
) {
    finalize_captured_child_invocation(state, row, supervisor_reason)
        .unwrap_or_else(|err| emit_captured_child_finalize_warning(child, &err));
}

fn finalize_captured_child_row(
    state: &StateDb,
    row: &InvocationRecord,
    supervisor_reason: &str,
) -> Result<(), String> {
    state.finalize_invocation(row.id, false, -1, None, Some(supervisor_reason))
}

fn emit_captured_child_finalize_warning(child: &executor::CapturedChildInvocation, err: &str) {
    eprintln!(
        "Warning: Failed to finalize captured child invocation {}: {err}",
        child.composite_id.id
    );
}

fn format_supervisor_reason(parent_terminal_reason: Option<&str>) -> String {
    let observed_reason = parent_terminal_reason.unwrap_or("unknown_exit");
    format!("supervisor_observed_{observed_reason}")
}

fn captured_child_matches_parent(
    row: &InvocationRecord,
    parent_invocation_id: i64,
    child: &executor::CapturedChildInvocation,
) -> bool {
    row.status == InvocationStatus::Running
        && row.parent_invocation_id == Some(parent_invocation_id)
        && row.provider_name.as_deref() == Some(child.composite_id.source.as_str())
}
