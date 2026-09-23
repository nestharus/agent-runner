//! Evidence validation for a distinct native-continuation grant. A successful
//! check is still preparation evidence, never a K release or a Q drain.
use crate::entry_registry::ProcessStamp;
use crate::identity::PeerIdentity;
use oulipoly_state::mailbox::{AcceptedNativeGrantSnapshot, ContinuationAttempt};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::PathBuf;

const RECEIPT_NAME: &str = "native-continuation-accepted-v1.json";
const REQUEST_NAME: &str = "custodian-request.json";
const MAX_RECEIPT: u64 = 1024 * 1024;
const MAX_REQUEST: u64 = 4 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeReceipt {
    protocol: String,
    accepted: AcceptedNativeGrantSnapshot,
    accepted_sha256: String,
    custodian_request_sha256: String,
    request_name: String,
    request_device: u64,
    request_inode: u64,
    request_byte_len: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestIdentity {
    path: PathBuf,
    attempt: ContinuationAttempt,
    recipe: serde_json::Value,
}

pub struct BoundNativeAuthority<'a> {
    pub root_id: &'a str,
    pub domain_id: &'a str,
    pub supervisor_authority_id: &'a str,
    pub owner_generation: &'a str,
    /// From the broker's already bound entry, never from this request.
    pub guardian: &'a ProcessStamp,
    /// From the guardian's in-memory positive decision, not an on-disk claim.
    pub receipt_sha256: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedNativeReceipt {
    pub attempt_id: String,
    pub accepted_snapshot_sha256: String,
    pub custodian_request_sha256: String,
    pub receipt_sha256: String,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn same_named_regular_file(dir: &File, name: &str, file: &File) -> io::Result<bool> {
    let name = std::ffi::CString::new(name).unwrap();
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let metadata = file.metadata()?;
    Ok(metadata.is_file()
        && stat.st_mode & libc::S_IFMT == libc::S_IFREG
        && metadata.dev() == stat.st_dev
        && metadata.ino() == stat.st_ino)
}

fn read_exact_bounded(file: &File, maximum: u64) -> io::Result<Vec<u8>> {
    let before = file.metadata()?;
    if !before.is_file() || before.len() > maximum {
        return Err(io::Error::other(
            "native evidence is not a bounded regular file",
        ));
    }
    let mut bytes = vec![0u8; before.len() as usize];
    file.read_exact_at(&mut bytes, 0)?;
    let after = file.metadata()?;
    if after.dev() != before.dev() || after.ino() != before.ino() || after.len() != before.len() {
        return Err(io::Error::other("native evidence changed during read"));
    }
    Ok(bytes)
}

/// Verify exactly the producer's receipt shape, request name/inode/bytes, and
/// accepted revision-2 snapshot under the broker's live guardian binding.
/// The broker must separately persist a tagged one-use grant before launch.
pub fn verify(
    peer: &PeerIdentity,
    bound: &BoundNativeAuthority<'_>,
    directory: &File,
    request: &File,
    receipt: &File,
) -> io::Result<VerifiedNativeReceipt> {
    peer.process.verify()?;
    if ProcessStamp::from(&peer.process) != *bound.guardian
        || !same_named_regular_file(directory, REQUEST_NAME, request)?
        || !same_named_regular_file(directory, RECEIPT_NAME, receipt)?
    {
        return Err(io::Error::other(
            "native guardian or descriptor binding changed",
        ));
    }
    let receipt_bytes = read_exact_bounded(receipt, MAX_RECEIPT)?;
    if digest(&receipt_bytes) != bound.receipt_sha256 {
        return Err(io::Error::other(
            "native receipt differs from guardian decision",
        ));
    }
    let evidence: NativeReceipt = serde_json::from_slice(&receipt_bytes)?;
    let request_bytes = read_exact_bounded(request, MAX_REQUEST)?;
    let request_meta = request.metadata()?;
    let accepted = &evidence.accepted;
    if evidence.protocol != "native-continuation-accepted-v1"
        || evidence.request_name != REQUEST_NAME
        || evidence.request_device != request_meta.dev()
        || evidence.request_inode != request_meta.ino()
        || evidence.request_byte_len != request_meta.len()
        || evidence.custodian_request_sha256 != digest(&request_bytes)
        || evidence.accepted_sha256 != digest(&serde_json::to_vec(accepted)?)
        || accepted.phase != "accepted"
        || accepted.revision != 2
        || accepted.integrated
        || accepted.custodian_identity.is_some()
        || accepted.adopter_identity.is_some()
        || accepted.kernel_root_id != bound.root_id
        || accepted.domain_id != bound.domain_id
        || accepted.supervisor_authority_id != bound.supervisor_authority_id
        || accepted.owner_generation != bound.owner_generation
        || accepted.attempt.owner_generation != bound.owner_generation
        || accepted.guardian_identity.pid != i64::from(peer.process.host_pid)
        || accepted.guardian_identity.boot_id != peer.process.boot_id
        || accepted.guardian_identity.starttime_ticks != peer.process.starttime_ticks as i64
    {
        return Err(io::Error::other(
            "native positive acceptance binding conflict",
        ));
    }
    let request_identity: RequestIdentity = serde_json::from_slice(&request_bytes)?;
    if request_identity.attempt != accepted.attempt
        || request_identity.path.as_os_str().is_empty()
        || request_identity.recipe.is_null()
        || !same_named_regular_file(directory, REQUEST_NAME, request)?
        || !same_named_regular_file(directory, RECEIPT_NAME, receipt)?
        || read_exact_bounded(request, MAX_REQUEST)? != request_bytes
    {
        return Err(io::Error::other(
            "native request changed or names another attempt",
        ));
    }
    Ok(VerifiedNativeReceipt {
        attempt_id: accepted.attempt.attempt_id.clone(),
        accepted_snapshot_sha256: evidence.accepted_sha256,
        custodian_request_sha256: evidence.custodian_request_sha256,
        receipt_sha256: bound.receipt_sha256.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::PinnedProcess;
    use oulipoly_state::completion_continuation::{PROTOCOL, SourceProcessIdentity};
    use oulipoly_state::mailbox::{CompletionDomainOwner, MailboxDb};
    use serde_json::json;
    use std::fs;
    use std::io::Write;

    #[test]
    fn real_state_acceptance_requires_exact_live_guardian_receipt_and_request() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open_completion_continuation_domain(&state_path).unwrap();
        let process = PinnedProcess::open(std::process::id() as i32).unwrap();
        let identity = SourceProcessIdentity {
            pid: i64::from(process.host_pid),
            boot_id: process.boot_id.clone(),
            starttime_ticks: process.starttime_ticks as i64,
        };
        let owner = CompletionDomainOwner {
            protocol: PROTOCOL.into(),
            domain_id: db.completion_continuation_domain().unwrap().unwrap(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: identity.clone(),
            driver_identity: identity,
            endpoint: dir.path().join("owner.sock").to_string_lossy().into(),
        };
        let root = uuid::Uuid::new_v4().to_string();
        db.publish_completion_owner_with_kernel_root(&owner, Some(&root))
            .unwrap();
        let attempt = ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: "transport".into(),
            request_sha256: "a".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: None,
            claim_token: None,
            result_path: dir.path().join("result.json").to_string_lossy().into(),
        };
        db.reserve_continuation_attempt(&attempt).unwrap();
        let accepted = db
            .accept_exact_native_attempt(&attempt, &owner, &root)
            .unwrap();
        let request_path = dir.path().join(REQUEST_NAME);
        let receipt_path = dir.path().join(RECEIPT_NAME);
        let request_bytes = serde_json::to_vec(&json!({
            "path": state_path,
            "attempt": attempt,
            "recipe": {"Native": {"args": [], "environment": [], "directory": null}}
        }))
        .unwrap();
        let mut request_writer = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&request_path)
            .unwrap();
        request_writer.write_all(&request_bytes).unwrap();
        request_writer.sync_all().unwrap();
        drop(request_writer);
        let request = File::open(&request_path).unwrap();
        let request_meta = request.metadata().unwrap();
        let accepted_sha = digest(&serde_json::to_vec(&accepted).unwrap());
        let receipt_bytes = serde_json::to_vec(&json!({
            "protocol": "native-continuation-accepted-v1",
            "accepted": accepted,
            "accepted_sha256": accepted_sha,
            "custodian_request_sha256": digest(&request_bytes),
            "request_name": REQUEST_NAME,
            "request_device": request_meta.dev(),
            "request_inode": request_meta.ino(),
            "request_byte_len": request_meta.len()
        }))
        .unwrap();
        let mut receipt_writer = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&receipt_path)
            .unwrap();
        receipt_writer.write_all(&receipt_bytes).unwrap();
        receipt_writer.sync_all().unwrap();
        drop(receipt_writer);
        File::open(dir.path()).unwrap().sync_all().unwrap();
        let receipt = File::open(&receipt_path).unwrap();
        let directory = File::open(dir.path()).unwrap();
        let peer = PeerIdentity {
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            process,
        };
        let stamp = ProcessStamp::from(&peer.process);
        let receipt_sha = digest(&receipt_bytes);
        let bound = BoundNativeAuthority {
            root_id: &root,
            domain_id: &owner.domain_id,
            supervisor_authority_id: &owner.supervisor_authority_id,
            owner_generation: &owner.owner_generation,
            guardian: &stamp,
            receipt_sha256: &receipt_sha,
        };
        let verified = verify(&peer, &bound, &directory, &request, &receipt).unwrap();
        assert_eq!(verified.attempt_id, attempt.attempt_id);
        assert_eq!(verified.custodian_request_sha256, digest(&request_bytes));
        let wrong_root = BoundNativeAuthority {
            root_id: "sibling",
            ..bound
        };
        assert!(verify(&peer, &wrong_root, &directory, &request, &receipt).is_err());
        let wrong_stamp = ProcessStamp {
            starttime_ticks: stamp.starttime_ticks + 1,
            ..stamp.clone()
        };
        let wrong_guardian = BoundNativeAuthority {
            guardian: &wrong_stamp,
            ..bound
        };
        assert!(verify(&peer, &wrong_guardian, &directory, &request, &receipt).is_err());
        let wrong_sha = BoundNativeAuthority {
            receipt_sha256: &"0".repeat(64),
            ..bound
        };
        assert!(verify(&peer, &wrong_sha, &directory, &request, &receipt).is_err());
        fs::write(&request_path, b"replaced bytes").unwrap();
        assert!(verify(&peer, &bound, &directory, &request, &receipt).is_err());
        fs::write(&request_path, &request_bytes).unwrap();
        fs::rename(&receipt_path, dir.path().join("moved-receipt")).unwrap();
        assert!(verify(&peer, &bound, &directory, &request, &receipt).is_err());
    }
}
