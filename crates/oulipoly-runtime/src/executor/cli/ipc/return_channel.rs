//! ## Declared roles
//!
//! Roles: orchestration, accessor.
//!
//! - orchestration: creates, reads, parses, and cleans return-channel files
//!   while preserving the existing operation sequence.
//! - accessor: reads return-channel file contents before JSONL parsing.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/ipc/return_channel.rs
//!     role: adapter
//!     Translates:
//!       - return-channel-filesystem-contract
//!       - composite-invocation-id-contract
//!       - returned-artifact-jsonl-contract
//!       - std-io-cleanup-warning-contract
//! ```

use super::return_channel_cleanup::cleanup_return_channel;
use super::return_channel_jsonl::parse_return_channel_body;
use super::return_channel_parent::parse_return_channel_parent_invocation;
use super::return_channel_path::{return_channel_dir, return_channel_path};
use super::return_channel_warnings::{
    create_return_channel_dir_error, create_return_channel_file_error, read_return_channel_warning,
};
use crate::executor::ReturnedArtifactRef;
use std::path::{Path, PathBuf};

pub(crate) struct ReturnChannel {
    path: PathBuf,
    dir: PathBuf,
}

impl ReturnChannel {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(&self) {
        cleanup_return_channel(&self.path, &self.dir);
    }
}

impl Drop for ReturnChannel {
    fn drop(&mut self) {
        self.cleanup();
    }
}

pub(crate) fn prepare_return_channel(
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

fn create_return_channel_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| create_return_channel_dir_error(dir, &err))
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

pub(crate) fn read_and_cleanup_return_channel(
    return_channel: Option<ReturnChannel>,
) -> Vec<ReturnedArtifactRef> {
    return_channel
        .as_ref()
        .map(read_return_channel)
        .unwrap_or_default()
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
