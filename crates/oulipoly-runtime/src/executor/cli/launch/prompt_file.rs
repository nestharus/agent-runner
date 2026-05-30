//! ## Declared roles
//!
//! Roles: orchestration.
//!
//! - orchestration: sequences temporary prompt filename creation, path
//!   mapping, file writing, and instruction formatting.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/launch/prompt_file.rs
//!     role: adapter
//!     Translates:
//!       - temp-prompt-file-contract
//!       - prompt-transport-contract
//! ```

use super::prompt_format::{
    temp_prompt_filename, temp_prompt_instruction, temp_prompt_write_error,
};
use super::prompt_path::temp_prompt_path;
use std::path::{Path, PathBuf};

pub(super) fn write_large_prompt_file(
    rendered_prompt: &str,
    working_dir: Option<&Path>,
) -> Result<(PathBuf, String), String> {
    let filename = temp_prompt_filename(uuid::Uuid::new_v4());
    let path = temp_prompt_path(working_dir, &filename);
    std::fs::write(&path, rendered_prompt).map_err(|err| temp_prompt_write_error(&err))?;
    Ok((path, temp_prompt_instruction(&filename)))
}
