//! ## Declared roles
//!
//! Roles: mapper, formatter.
//!
//! - mapper: maps parent invocation identifiers to return-channel directory
//!   and file paths.
//! - formatter: renders the canonical return-channel filename.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_path.rs
//!     role: adapter
//!     Translates:
//!       - return-channel-filesystem-contract
//!       - composite-invocation-id-contract
//! ```

use oulipoly_state::CompositeInvocationId;
use std::path::{Path, PathBuf};

pub(super) fn return_channel_dir(invocation: &CompositeInvocationId) -> PathBuf {
    std::env::temp_dir()
        .join("oulipoly-return-channels")
        .join(format!("{}-{}", invocation.id, uuid::Uuid::new_v4()))
}

pub(super) fn return_channel_path(dir: &Path) -> PathBuf {
    dir.join(return_channel_filename())
}

fn return_channel_filename() -> String {
    format!("returns-{}.jsonl", uuid::Uuid::new_v4())
}
