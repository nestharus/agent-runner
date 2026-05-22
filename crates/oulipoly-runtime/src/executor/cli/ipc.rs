//! ## Declared roles
//!
//! Roles: orchestration, parser, formatter, filter, predicate, accessor,
//! validator, mapper.
//!
//! - orchestration: [`prepare_return_channel`] +
//!   [`read_and_cleanup_return_channel`].
//! - parser: [`parse_return_channel_parent_invocation`],
//!   [`parse_return_channel_body`], [`parse_returned_artifact_ref`],
//!   [`captured_child_invocations_from_stderr`],
//!   [`captured_child_composite_id`].
//! - formatter: [`return_channel_filename`],
//!   [`delete_return_channel_warning`],
//!   [`delete_return_channel_dir_warning`],
//!   [`read_return_channel_warning`],
//!   [`parse_return_channel_line_warning`].
//! - filter: [`return_channel_record_line`], [`mark_captured_child_seen`].
//! - predicate: [`return_channel_dir_cleanup_should_warn`],
//!   [`return_channel_line_is_empty`].
//! - accessor: [`read_return_channel_body`].
//! - mapper: [`return_channel_path`], [`captured_child_composite_id`].
//! - validator: embedded parser contract test
//!   `captured_child_marker_parser_keeps_one_valid_marker_and_drops_noise`.
//!
//! Preserves implicit external IPC contracts bit-for-bit:
//!
//! - Environment variable names: `OULIPOLY_PARENT_INVOCATION`,
//!   `OULIPOLY_RETURN_CHANNEL`.
//! - Stderr marker prefix: `OULIPOLY_INVOCATION=<json>`.
//! - Return-channel JSONL: one object per line; blank lines tolerated;
//!   malformed lines warned and dropped.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc.rs
//!     role: adapter
//!     Translates:
//!       - return-channel-filesystem-contract
//!       - composite-invocation-id-contract
//!       - returned-artifact-jsonl-contract
//!       - captured-child-marker-contract
//!       - std-io-cleanup-warning-contract
//! ```

use super::super::{CapturedChildInvocation, ReturnedArtifactRef};
use oulipoly_state::CompositeInvocationId;
use std::fmt::Display;
use std::path::{Path, PathBuf};

pub(super) struct ReturnChannel {
    pub(super) path: PathBuf,
    pub(super) dir: PathBuf,
}

impl ReturnChannel {
    pub(super) fn cleanup(&self) {
        cleanup_return_channel_file(&self.path);
        cleanup_return_channel_dir(&self.dir);
    }
}

fn cleanup_return_channel_file(path: &Path) {
    if let Err(err) = std::fs::remove_file(path) {
        eprintln!("{}", delete_return_channel_warning(path, &err));
    }
}

fn cleanup_return_channel_dir(dir: &Path) {
    if let Err(err) = std::fs::remove_dir(dir)
        && return_channel_dir_cleanup_should_warn(&err)
    {
        eprintln!("{}", delete_return_channel_dir_warning(dir, &err));
    }
}

fn return_channel_dir_cleanup_should_warn(err: &std::io::Error) -> bool {
    err.kind() != std::io::ErrorKind::NotFound
        && err.kind() != std::io::ErrorKind::DirectoryNotEmpty
}

fn delete_return_channel_warning(path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to delete return channel {}: {err}",
        path.display()
    )
}

fn delete_return_channel_dir_warning(dir: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to delete return channel directory {}: {err}",
        dir.display()
    )
}

pub(super) fn prepare_return_channel(
    parent_invocation_env: Option<&str>,
) -> Result<Option<ReturnChannel>, String> {
    let Some(parent_invocation_env) = parent_invocation_env else {
        return Ok(None);
    };
    let invocation = parse_return_channel_parent_invocation(parent_invocation_env)?;
    let dir = return_channel_dir(&invocation);
    create_return_channel_dir(&dir)?;
    let path = return_channel_path(&dir);
    create_return_channel_file(&path)?;
    Ok(Some(ReturnChannel { path, dir }))
}

fn parse_return_channel_parent_invocation(
    parent_invocation_env: &str,
) -> Result<CompositeInvocationId, String> {
    CompositeInvocationId::parse_env_value(parent_invocation_env)
        .map_err(|err| return_channel_parent_invocation_parse_error(&err))
}

fn return_channel_parent_invocation_parse_error(err: &dyn Display) -> String {
    format!("Failed to parse parent invocation for return channel: {err}")
}

fn return_channel_dir(invocation: &CompositeInvocationId) -> PathBuf {
    std::env::temp_dir()
        .join("oulipoly-return-channels")
        .join(&invocation.id)
}

fn create_return_channel_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| create_return_channel_dir_error(dir, &err))
}

fn create_return_channel_dir_error(dir: &Path, err: &std::io::Error) -> String {
    format!(
        "Failed to create return channel directory {}: {err}",
        dir.display()
    )
}

fn return_channel_path(dir: &Path) -> PathBuf {
    dir.join(return_channel_filename())
}

fn return_channel_filename() -> String {
    format!("returns-{}.jsonl", uuid::Uuid::new_v4())
}

fn create_return_channel_file(path: &Path) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map(|_| ())
        .map_err(|err| create_return_channel_file_error(path, &err))
}

fn create_return_channel_file_error(path: &Path, err: &std::io::Error) -> String {
    format!("Failed to create return channel {}: {err}", path.display())
}

pub(super) fn read_and_cleanup_return_channel(
    return_channel: &Option<ReturnChannel>,
) -> Vec<ReturnedArtifactRef> {
    let returned_artifacts = return_channel
        .as_ref()
        .map(read_return_channel)
        .unwrap_or_default();
    if let Some(channel) = return_channel {
        channel.cleanup();
    }
    returned_artifacts
}

fn read_return_channel(channel: &ReturnChannel) -> Vec<ReturnedArtifactRef> {
    let body = match read_return_channel_body(&channel.path) {
        Some(body) => body,
        None => return Vec::new(),
    };
    parse_return_channel_body(&body, &channel.path)
}

fn read_return_channel_body(path: &Path) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(body) => Some(body),
        Err(err) => {
            eprintln!("{}", read_return_channel_warning(path, &err));
            None
        }
    }
}

fn read_return_channel_warning(path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to read return channel {}: {err}",
        path.display()
    )
}

fn parse_return_channel_body(body: &str, path: &Path) -> Vec<ReturnedArtifactRef> {
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

fn return_channel_record_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if return_channel_line_is_empty(trimmed) {
        None
    } else {
        Some(trimmed)
    }
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

fn return_channel_line_is_empty(line: &str) -> bool {
    line.is_empty()
}

fn parse_return_channel_line_warning(
    line_number: usize,
    path: &Path,
    err: &serde_json::Error,
) -> String {
    format!(
        "Warning: failed to parse return channel line {} in {}: {err}",
        line_number,
        path.display()
    )
}

pub(super) fn captured_child_invocations_from_stderr(stderr: &str) -> Vec<CapturedChildInvocation> {
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

fn mark_captured_child_seen(
    seen: &mut std::collections::HashSet<(String, String)>,
    composite_id: &CompositeInvocationId,
) -> bool {
    seen.insert((composite_id.source.clone(), composite_id.id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_child_marker_parser_keeps_one_valid_marker_and_drops_noise() {
        let marker = CompositeInvocationId {
            source: "fixture-child".to_string(),
            id: "11111111-1111-1111-1111-111111111111".to_string(),
        };
        let marker_line = marker.stderr_line();
        let stderr = format!(
            "noise\n{}\nOULIPOLY_INVOCATION={{\"source\":\"fixture-child\",\"id\":\"not-a-uuid\"}}\n{}\n",
            marker_line, marker_line
        );

        let captured = captured_child_invocations_from_stderr(&stderr);

        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].composite_id, marker);
        assert_eq!(captured[0].raw_marker_line, marker_line);
    }
}
