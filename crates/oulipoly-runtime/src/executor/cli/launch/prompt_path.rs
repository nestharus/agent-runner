//! ## Declared roles
//!
//! Roles: mapper, accessor.
//!
//! - mapper: maps prompt file directory inputs and generated filenames to
//!   filesystem paths.
//! - accessor: reads the effective prompt directory fallback.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/prompt_path.rs
//!     role: adapter
//!     Translates:
//!       - temp-prompt-file-contract
//! ```

use std::path::{Path, PathBuf};

pub(super) fn temp_prompt_path(working_dir: Option<&Path>, filename: &str) -> PathBuf {
    temp_prompt_dir(working_dir).join(filename)
}

fn temp_prompt_dir(working_dir: Option<&Path>) -> &Path {
    working_dir.unwrap_or(Path::new("."))
}
