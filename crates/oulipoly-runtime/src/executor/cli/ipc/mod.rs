//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: exposes the return-channel lifecycle and captured-child
//!   marker parser surfaces through the stable IPC family module.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/mod.rs
//!     role: adapter
//!     Translates:
//!       - return-channel-filesystem-contract
//!       - composite-invocation-id-contract
//!       - returned-artifact-jsonl-contract
//!       - captured-child-marker-contract
//!       - std-io-cleanup-warning-contract
//! ```

mod captured_child_dedupe;
mod captured_child_marker;
mod return_channel;
mod return_channel_cleanup;
mod return_channel_jsonl;
mod return_channel_parent;
mod return_channel_path;
mod return_channel_predicates;
mod return_channel_warnings;

pub(in crate::executor::cli) use captured_child_marker::captured_child_invocations_from_stderr;
pub(in crate::executor::cli) use return_channel::{
    ReturnChannel, prepare_return_channel, read_and_cleanup_return_channel,
};
