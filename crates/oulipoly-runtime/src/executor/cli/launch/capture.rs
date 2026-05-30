//! ## Declared roles
//!
//! Roles: mapper.
//!
//! - mapper: maps session-capture configuration and optional known session id
//!   into launch arguments and temp-file handoff values.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/capture.rs
//!     role: adapter
//!     Translates:
//!       - session-capture-plan-contract
//!       - provider-config-launch-contract
//! ```

use crate::executor::cli::session_capture::{CapturePlan, build_capture_plan};
use oulipoly_config::SessionCapture;
use std::path::PathBuf;

pub(super) fn build_launch_capture_plan(
    session_capture: Option<&SessionCapture>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    build_capture_plan(session_capture, start_known_provider_session_id)
}
