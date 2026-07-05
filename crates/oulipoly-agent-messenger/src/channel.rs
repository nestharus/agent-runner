//! ## Declared roles
//!
//! - orchestration
//!
//! Return-channel append boundary.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-agent-messenger/src/channel.rs
//!     role: adapter
//!     Translates:
//!       - returned-artifact JSONL line contract
//!       - std filesystem append contract
//! ```

use crate::MessengerError;
use crate::formatter::return_channel_line;
use crate::model::ReturnedArtifact;
use std::fs;
use std::io::Write;
use std::path::Path;

pub fn append_return_channel(
    path: &Path,
    receipt: &ReturnedArtifact,
) -> Result<(), MessengerError> {
    let line = return_channel_line(receipt)?;
    write_return_channel_line(path, &line)
}

fn write_return_channel_line(path: &Path, line: &[u8]) -> Result<(), MessengerError> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line)?;
    file.flush()?;
    Ok(())
}
