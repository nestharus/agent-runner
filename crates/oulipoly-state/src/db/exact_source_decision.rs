//! Read-only Broker decision inspection under a State writer reservation.
//!
//! This is deliberately not registration authority: no decision is consumed,
//! no obligation is recorded, and the public registration path does not call it.

use super::{RusqliteOptionalExtension, StateDb, sqlite};
use crate::completion_continuation::{AdmittedSourceBinding, completion_obligation_admission_id};
use crate::mailbox::PreparedProcessStamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

const INSTALLED_BROKER_SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";

/// The witness is serialized unchanged into the challenged Broker `;` frame.
/// Broker authenticates the actual peer, connected guardian and registration FD.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDecisionVerification {
    pub request_id: String,
    pub decision_id: String,
    pub witness: serde_json::Value,
}

/// An observation only. Holding this value never permits effect promotion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDecisionInspection {
    pub request_id: String,
    pub decision_id: String,
    pub root_id: String,
    pub source_generation: String,
    pub owner_generation: String,
    pub owner_invocation_uuid: String,
    pub owner_session_id: String,
    pub issuer: PreparedProcessStamp,
    pub registration_id: String,
    pub registration_sha256: String,
    pub original_state_device: u64,
    pub original_state_inode: u64,
}

#[derive(Deserialize)]
struct BrokerReadback {
    request_id: String,
    decision_id: String,
    root_id: String,
    source_generation: String,
    owner_generation: String,
    owner_invocation_uuid: String,
    owner_session_id: String,
    issuer: PreparedProcessStamp,
    capability_digest: String,
    caller_admission_id: String,
    handle: String,
    registration_id: String,
    registration_path: PathBuf,
    registration_len: u64,
    registration_sha256: String,
    original_state_device: u64,
    original_state_inode: u64,
}

impl StateDb {
    /// Probe an already issued original-child decision while this opened State
    /// database holds BEGIN IMMEDIATE. This does not commit or grant admission.
    pub fn inspect_original_child_decision(
        &mut self,
        request: &SourceDecisionVerification,
        guardian_fd: RawFd,
        registration_fd: RawFd,
        binding: &AdmittedSourceBinding,
    ) -> Result<SourceDecisionInspection, String> {
        self.inspect_original_child_decision_on(
            Path::new(INSTALLED_BROKER_SOCKET),
            request,
            guardian_fd,
            registration_fd,
            binding,
        )
    }

    /// Fixture endpoint override; production uses only the fixed socket above.
    #[cfg(feature = "age319-private-broker-fixture")]
    pub fn inspect_original_child_decision_at(
        &mut self,
        socket: &Path,
        request: &SourceDecisionVerification,
        guardian_fd: RawFd,
        registration_fd: RawFd,
        binding: &AdmittedSourceBinding,
    ) -> Result<SourceDecisionInspection, String> {
        self.inspect_original_child_decision_on(
            socket,
            request,
            guardian_fd,
            registration_fd,
            binding,
        )
    }

    fn inspect_original_child_decision_on(
        &mut self,
        socket: &Path,
        request: &SourceDecisionVerification,
        guardian_fd: RawFd,
        registration_fd: RawFd,
        binding: &AdmittedSourceBinding,
    ) -> Result<SourceDecisionInspection, String> {
        let authority = self
            .completion_authority_state
            .as_ref()
            .ok_or("exact source inspection requires an opened local State file")?;
        let opened_file = authority.file;
        let source_path = authority.source_path.clone();
        let canonical_path = authority.path.clone();
        if self.completion_authority_state_path().is_none() {
            return Err("exact source inspection State file identity changed".into());
        }
        let source = binding.registration()?;
        if binding.is_late_listener() {
            return Err("exact source inspection requires original listener".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(sqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("exact source inspection writer reservation: {error}"))?;
        let readback = verify_broker_decision(socket, request, guardian_fd, registration_fd)?;
        let witness = &request.witness;
        if readback.request_id != request.request_id
            || readback.decision_id != request.decision_id
            || readback.original_state_device != opened_file.volume
            || readback.original_state_inode != opened_file.file
            || readback.root_id != witness_string(witness, &["owner", "root_id"])?
            || readback.source_generation != witness_string(witness, &["source_generation"])?
            || readback.owner_generation != witness_string(witness, &["owner_generation"])?
            || readback.owner_invocation_uuid != source.owner_invocation_uuid
            || readback.owner_session_id != source.owner_session_id
            || i64::from(readback.issuer.host_pid) != source.registering_caller.pid
            || readback.issuer.boot_id != source.registering_caller.boot_id
            || i64::try_from(readback.issuer.starttime_ticks).ok()
                != Some(source.registering_caller.starttime_ticks)
            || readback.handle != source.handle
            || readback.registration_id != source.registration_id
            || readback.registration_path
                != Path::new(&source.handle_dir).join(&source.registration_relative)
            || readback.registration_len != binding.registration_bytes().len() as u64
            || readback.registration_sha256 != binding.registration_digest()
            || readback.caller_admission_id != binding.caller_admission_id()
            || readback.caller_admission_id
                != completion_obligation_admission_id(&source.handle, &source.owner_invocation_uuid)
        {
            return Err("exact source inspection decision/State/registration mismatch".into());
        }
        let capability = witness_string(witness, &["capability"])?;
        let mut hasher = Sha256::new();
        hasher.update(b"oulipoly-completion-registration-authority-v1");
        hasher.update(capability.as_bytes());
        let digest = format!("{:x}", hasher.finalize());
        let row: Option<(Option<String>, Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT completion_registration_capability_digest,provider_session_id,session_id
                 FROM invocations WHERE invocation_uuid=?1",
                [&source.owner_invocation_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let (Some(expected), provider_session, fallback_session) =
            row.ok_or("exact source inspection original invocation absent")?
        else {
            return Err("exact source inspection invocation capability absent".into());
        };
        if expected != digest
            || readback.capability_digest != digest
            || provider_session.or(fallback_session).as_deref() != Some(&source.owner_session_id)
        {
            return Err("exact source inspection invocation/session/capability mismatch".into());
        }
        let canonical_now = std::fs::canonicalize(&source_path).map_err(|e| e.to_string())?;
        let metadata = std::fs::metadata(&canonical_now).map_err(|e| e.to_string())?;
        let identity = crate::filesystem_identity::path_file_identity(&canonical_now, &metadata)
            .map_err(|e| e.to_string())?;
        if canonical_now != canonical_path
            || !metadata.is_file()
            || identity.links != 1
            || identity.storage != opened_file.volume
            || identity.file != opened_file.file
        {
            return Err("exact source inspection State file changed during verification".into());
        }
        // The transaction is rolled back on drop. A later registration cannot
        // treat this observation as a consumed decision.
        Ok(SourceDecisionInspection {
            request_id: readback.request_id,
            decision_id: readback.decision_id,
            root_id: readback.root_id,
            source_generation: readback.source_generation,
            owner_generation: readback.owner_generation,
            owner_invocation_uuid: readback.owner_invocation_uuid,
            owner_session_id: readback.owner_session_id,
            issuer: readback.issuer,
            registration_id: readback.registration_id,
            registration_sha256: readback.registration_sha256,
            original_state_device: readback.original_state_device,
            original_state_inode: readback.original_state_inode,
        })
    }
}

fn witness_string<'a>(value: &'a serde_json::Value, path: &[&str]) -> Result<&'a str, String> {
    path.iter()
        .try_fold(value, |current, key| current.get(*key))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("exact source inspection witness {} absent", path.join(".")))
}

fn verify_broker_decision(
    path: &Path,
    request: &SourceDecisionVerification,
    guardian_fd: RawFd,
    registration_fd: RawFd,
) -> Result<BrokerReadback, String> {
    let body = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    if body.len() > 2048 {
        return Err("exact source inspection request too large".into());
    }
    let mut stream = UnixStream::connect(path).map_err(|error| error.to_string())?;
    let timeout = Some(std::time::Duration::from_secs(10));
    stream
        .set_read_timeout(timeout)
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(timeout)
        .map_err(|error| error.to_string())?;
    let mut peer = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut peer_len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            peer.as_mut_ptr().cast(),
            &mut peer_len,
        )
    } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if peer_len as usize != std::mem::size_of::<libc::ucred>()
        || unsafe { peer.assume_init().uid } != 0
    {
        return Err("exact source inspection broker peer is not host root".into());
    }
    let mut challenge = [0_u8; 16];
    stream
        .read_exact(&mut challenge)
        .map_err(|error| error.to_string())?;
    let mut frame = Vec::with_capacity(17 + body.len());
    frame.push(b';');
    frame.extend_from_slice(&challenge);
    frame.extend_from_slice(&body);
    let descriptors = [guardian_fd, registration_fd];
    let mut iov = libc::iovec {
        iov_base: frame.as_mut_ptr().cast(),
        iov_len: frame.len(),
    };
    let mut control = [0_u8; 64];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&descriptors) as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&descriptors) as _) as usize;
        std::ptr::copy_nonoverlapping(descriptors.as_ptr(), libc::CMSG_DATA(header).cast(), 2);
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &message, libc::MSG_NOSIGNAL) }
        != frame.len() as isize
    {
        return Err("exact source inspection short Broker frame; outcome uncertain".into());
    }
    let mut reply = Vec::new();
    stream
        .take(4097)
        .read_to_end(&mut reply)
        .map_err(|error| error.to_string())?;
    if reply.len() > 4096 || !reply.ends_with(b"\n") {
        return Err("exact source inspection malformed Broker reply".into());
    }
    if reply.starts_with(b"error ") {
        return Err(String::from_utf8_lossy(&reply).trim().to_owned());
    }
    serde_json::from_slice(&reply).map_err(|error| error.to_string())
}
