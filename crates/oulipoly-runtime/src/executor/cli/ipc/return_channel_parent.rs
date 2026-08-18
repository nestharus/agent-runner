//! ## Declared roles
//!
//! Roles: parser.
//!
//! - parser: parses parent invocation environment values for return-channel
//!   setup.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_parent.rs
//!     role: adapter
//!     Translates:
//!       - composite-invocation-id-contract
//!       - cli-ipc-return-channel-contract
//! ```

use super::return_channel_warnings::return_channel_parent_invocation_parse_error;
use crate::executor::cli::spawn_identity::split_invocation_launch_environment;
use oulipoly_state::CompositeInvocationId;

pub(super) fn parse_return_channel_parent_invocation(
    parent_invocation_env: &str,
) -> Result<CompositeInvocationId, String> {
    let (identity, _) = split_invocation_launch_environment(parent_invocation_env)
        .map_err(|error| return_channel_parent_invocation_parse_error(&error))?;
    CompositeInvocationId::parse_env_value(&identity)
        .map_err(|err| return_channel_parent_invocation_parse_error(&err))
}
