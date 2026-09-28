//! Exact, root-only request and PID1's own terminal ECHILD witness. This is
//! separate from the stable parent's wait and State/sidecar root close.

use crate::identity::observed_incarnation_absent;
use crate::json_artifact;
use crate::registry::RootRecord;
use crate::work_registry::root_only_bytes;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RootPid1Request {
    pub version: u32,
    pub root: RootRecord,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RootPid1Terminal {
    pub version: u32,
    pub root: RootRecord,
    pub request_sha256: String,
    pub original_exit_reported: bool,
    pub owned_children: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RootPid1ParentWait {
    pub version: u32,
    pub root: RootRecord,
    pub wait_status: i32,
    pub reaped: bool,
}

fn request_name(root: &RootRecord) -> String {
    format!("{}.request.json", root.root_id)
}

fn terminal_name(root: &RootRecord) -> String {
    format!("{}.terminal.json", root.root_id)
}

fn parent_wait_name(root: &RootRecord) -> String {
    format!("{}.parent-wait.json", root.root_id)
}

pub fn read_request(
    directory: &Path,
    root: &RootRecord,
) -> io::Result<Option<(RootPid1Request, String)>> {
    let bytes = match root_only_bytes(directory, &request_name(root), 4096) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let request: RootPid1Request = serde_json::from_slice(&bytes)?;
    if request.version != 1 || request.root != *root {
        return Err(io::Error::other("root PID1 request incarnation changed"));
    }
    Ok(Some((request, format!("{:x}", Sha256::digest(&bytes)))))
}

pub fn publish_request(directory: &Path, root: &RootRecord) -> io::Result<()> {
    if read_request(directory, root)?.is_some() {
        return sync_existing(directory, &request_name(root));
    }
    let result = json_artifact::create_new(
        directory,
        &request_name(root),
        &RootPid1Request {
            version: 1,
            root: root.clone(),
        },
    );
    if result.is_err() && read_request(directory, root)?.is_some() {
        return sync_existing(directory, &request_name(root));
    }
    result
}

/// Called by the exact PID1 after waitpid(-1) reported ECHILD. The final
/// name is create-new, file-synced and directory-synced before PID1 exits.
pub fn publish_terminal(directory: &Path, root: &RootRecord) -> io::Result<()> {
    let (_, request_sha256) = read_request(directory, root)?
        .ok_or_else(|| io::Error::other("root PID1 drain request absent"))?;
    if read_terminal(directory, root)?.is_some() {
        return sync_existing(directory, &terminal_name(root));
    }
    let result = json_artifact::create_new(
        directory,
        &terminal_name(root),
        &RootPid1Terminal {
            version: 1,
            root: root.clone(),
            request_sha256,
            original_exit_reported: true,
            owned_children: "ECHILD".into(),
        },
    );
    if result.is_err() && read_terminal(directory, root)?.is_some() {
        // The final name can be visible before a directory fsync reports an
        // error. Recheck and sync that same exact receipt before exiting.
        return sync_existing(directory, &terminal_name(root));
    }
    result
}

fn sync_existing(directory: &Path, name: &str) -> io::Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join(name))?;
    file.sync_all()?;
    File::open(directory)?.sync_all()
}

pub fn read_terminal(directory: &Path, root: &RootRecord) -> io::Result<Option<RootPid1Terminal>> {
    let bytes = match root_only_bytes(directory, &terminal_name(root), 4096) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let terminal: RootPid1Terminal = serde_json::from_slice(&bytes)?;
    let (_, request_sha256) = read_request(directory, root)?
        .ok_or_else(|| io::Error::other("root PID1 terminal request absent"))?;
    if terminal.version != 1
        || terminal.root != *root
        || terminal.request_sha256 != request_sha256
        || !terminal.original_exit_reported
        || terminal.owned_children != "ECHILD"
    {
        return Err(io::Error::other("root PID1 terminal proof changed"));
    }
    Ok(Some(terminal))
}

pub fn publish_parent_wait(
    directory: &Path,
    root: &RootRecord,
    wait_status: i32,
) -> io::Result<()> {
    if !libc::WIFEXITED(wait_status) || libc::WEXITSTATUS(wait_status) != 0 {
        return Err(io::Error::other("root PID1 parent wait was not clean"));
    }
    read_terminal(directory, root)?
        .ok_or_else(|| io::Error::other("root PID1 terminal receipt absent"))?;
    if parent_wait_proof(directory, root)? {
        return sync_existing(directory, &parent_wait_name(root));
    }
    let result = json_artifact::create_new(
        directory,
        &parent_wait_name(root),
        &RootPid1ParentWait {
            version: 1,
            root: root.clone(),
            wait_status,
            reaped: true,
        },
    );
    if result.is_err() && parent_wait_proof(directory, root)? {
        return sync_existing(directory, &parent_wait_name(root));
    }
    result
}

pub fn parent_wait_proof(directory: &Path, root: &RootRecord) -> io::Result<bool> {
    let bytes = match root_only_bytes(directory, &parent_wait_name(root), 4096) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let wait: RootPid1ParentWait = serde_json::from_slice(&bytes)?;
    if wait.version != 1
        || wait.root != *root
        || !wait.reaped
        || !libc::WIFEXITED(wait.wait_status)
        || libc::WEXITSTATUS(wait.wait_status) != 0
        || read_terminal(directory, root)?.is_none()
    {
        return Err(io::Error::other("root PID1 parent wait proof changed"));
    }
    Ok(true)
}

/// A receipt while the exact PID1 still exists is pending, never physical Q.
/// A dead PID without this exact receipt is likewise never physical Q.
pub fn terminal_proof(directory: &Path, root: &RootRecord) -> io::Result<Option<RootPid1Terminal>> {
    let Some(terminal) = read_terminal(directory, root)? else {
        return Ok(None);
    };
    if !observed_incarnation_absent(
        root.init_host_pid,
        &root.boot_id,
        root.init_starttime_ticks,
        (root.pidns_dev, root.pidns_ino),
    )? {
        return Ok(None);
    }
    Ok(Some(terminal))
}
