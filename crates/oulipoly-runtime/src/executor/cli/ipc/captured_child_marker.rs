//! ## Declared roles
//!
//! Roles: orchestration, parser.
//!
//! - orchestration: sequences stderr-line marker parsing with duplicate
//!   suppression.
//! - parser: parses captured-child stderr marker lines and preserves the raw
//!   marker line on accepted records.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/captured_child_marker.rs
//!     role: adapter
//!     Translates:
//!       - captured-child-marker-contract
//!       - composite-invocation-id-contract
//! ```

use super::captured_child_dedupe::mark_captured_child_seen;
use crate::executor::CapturedChildInvocation;
use oulipoly_state::CompositeInvocationId;

pub(crate) fn captured_child_invocations_from_stderr(stderr: &str) -> Vec<CapturedChildInvocation> {
    let mut captured = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in stderr.lines() {
        if let Some(invocation) = captured_child_invocation_from_line(line)
            && mark_captured_child_seen(&mut seen, &invocation.composite_id)
        {
            captured.push(invocation);
        }
    }

    captured
}

fn captured_child_invocation_from_line(line: &str) -> Option<CapturedChildInvocation> {
    let composite_id = captured_child_composite_id(line)?;
    Some(CapturedChildInvocation {
        composite_id,
        raw_marker_line: line.to_string(),
    })
}

fn captured_child_composite_id(line: &str) -> Option<CompositeInvocationId> {
    let raw = line.strip_prefix("OULIPOLY_INVOCATION=")?;
    CompositeInvocationId::parse_env_value(raw).ok()
}
