//! ## Declared roles
//!
//! Roles: parser, formatter.
//!
//! - parser: parses return-channel JSONL records, skips blank lines, and
//!   reports malformed records through the warning formatter.
//! - formatter: emits canonical malformed-line warnings for rejected records.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_jsonl.rs
//!     role: adapter
//!     Translates:
//!       - returned-artifact-jsonl-contract
//!       - std-io-cleanup-warning-contract
//! ```

use super::return_channel_predicates::return_channel_record_line;
use super::return_channel_warnings::parse_return_channel_line_warning;
use crate::executor::ReturnedArtifactRef;
use std::path::Path;

pub(super) fn parse_return_channel_body(body: &str, path: &Path) -> Vec<ReturnedArtifactRef> {
    body.lines()
        .enumerate()
        .filter_map(|(index, line)| return_channel_record_from_line(index, line, path))
        .collect()
}

fn return_channel_record_from_line(
    index: usize,
    line: &str,
    path: &Path,
) -> Option<ReturnedArtifactRef> {
    let trimmed = return_channel_record_line(line)?;
    parsed_return_channel_record(index, trimmed, path)
}

fn parsed_return_channel_record(
    index: usize,
    trimmed: &str,
    path: &Path,
) -> Option<ReturnedArtifactRef> {
    match parse_returned_artifact_ref(trimmed) {
        Ok(reference) => Some(reference),
        Err(err) => {
            emit_return_channel_line_parse_warning(index, path, &err);
            None
        }
    }
}

fn parse_returned_artifact_ref(line: &str) -> Result<ReturnedArtifactRef, serde_json::Error> {
    serde_json::from_str::<ReturnedArtifactRef>(line)
}

fn emit_return_channel_line_parse_warning(index: usize, path: &Path, err: &serde_json::Error) {
    eprintln!(
        "{}",
        parse_return_channel_line_warning(index + 1, path, err)
    );
}
