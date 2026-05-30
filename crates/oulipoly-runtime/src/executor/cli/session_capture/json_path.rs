//! ## Declared roles
//!
//! Roles: accessor, parser.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/json_path.rs
//!     role: adapter
//!     Translates:
//!       - stdout-json-event-session-contract
//! ```

use serde_json::Value;

pub(super) fn lookup_json_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let segments = json_path_segments(path);
    lookup_json_path_segments(value, &segments)
}

fn json_path_segments(path: &str) -> Vec<&str> {
    path.split('.').collect()
}

fn lookup_json_path_segments<'a>(value: &'a Value, segments: &[&str]) -> Option<&'a Value> {
    segments
        .iter()
        .try_fold(value, |current, segment| current.get(*segment))
}
