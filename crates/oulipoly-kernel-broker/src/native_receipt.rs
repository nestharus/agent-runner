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
    pub owner_uid: u32,
    /// From the broker's already bound entry, never from this request.
    pub guardian: &'a ProcessStamp,
    pub host_namespace: &'a File,
    pub runner_image: &'a File,
    /// From the guardian's in-memory positive decision, not an on-disk claim.
    pub receipt_sha256: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedNativeReceipt {
    pub attempt_id: String,
    /// Path in the guardian's immutable, digest-bound request, never a K claim.
    pub state_path: PathBuf,
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
    if peer.uid != bound.owner_uid
        || ProcessStamp::from(&peer.process) != *bound.guardian
        || !peer.process.in_namespace(bound.host_namespace)?
        || !peer.process.same_executable_as(bound.runner_image)?
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
        || !request_identity.path.is_absolute()
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
        state_path: request_identity.path,
        accepted_snapshot_sha256: evidence.accepted_sha256,
        custodian_request_sha256: evidence.custodian_request_sha256,
        receipt_sha256: bound.receipt_sha256.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accepted_grant::GrantRegistry;
    use crate::entry_registry::{EntryRecord, EntryRegistry};
    use crate::identity::PinnedProcess;
    use crate::protocol::NativeKSpec;
    use crate::registry::{RootRecord, RootRegistry};
    use crate::work_registry::WorkRegistry;
    use oulipoly_state::completion_continuation::{PROTOCOL, SourceProcessIdentity};
    use oulipoly_state::mailbox::{CompletionDomainOwner, MailboxDb};
    use serde_json::json;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixListener;
    use std::process::Command;

    struct PrivateInit(std::process::Child);

    impl Drop for PrivateInit {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

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
        let host_namespace = File::open("/proc/self/ns/pid").unwrap();
        let runner_image = File::open("/proc/self/exe").unwrap();
        let receipt_sha = digest(&receipt_bytes);
        let sidecar = File::open(&state_path).unwrap();
        let bound = BoundNativeAuthority {
            root_id: &root,
            domain_id: &owner.domain_id,
            supervisor_authority_id: &owner.supervisor_authority_id,
            owner_generation: &owner.owner_generation,
            owner_uid: peer.uid,
            guardian: &stamp,
            host_namespace: &host_namespace,
            runner_image: &runner_image,
            receipt_sha256: &receipt_sha,
        };
        let verified = verify(&peer, &bound, &directory, &request, &receipt).unwrap();
        assert_eq!(verified.attempt_id, attempt.attempt_id);
        assert_eq!(verified.custodian_request_sha256, digest(&request_bytes));
        // Exercise the registry with evidence from the actual State CAS and
        // live guardian PID/image, including fsynced reply loss and restart.
        let registry_dir = dir.path().join("registry");
        fs::create_dir(&registry_dir).unwrap();
        for name in ["entries", "works", "grants"] {
            fs::create_dir(registry_dir.join(name)).unwrap();
        }
        // A real private PID1 supplies live root evidence; host PID1 is not
        // inspectable by an unprivileged test process on this machine.
        let init_socket = dir.path().join("init.sock");
        let listener = UnixListener::bind(&init_socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut init_child = PrivateInit(Command::new("unshare")
            .args(["--user", "--map-root-user", "--pid", "--fork", "--kill-child", "--mount", "--mount-proc", "python3", "-c",
                "import os,socket,sys,time\nassert os.getpid()==1\ns=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);s.connect(sys.argv[1]);s.send(b'1');time.sleep(30)",
                init_socket.to_str().unwrap()])
            .spawn().unwrap());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let (mut init_stream, _) = loop {
            match listener.accept() {
                Ok(accepted) => break accepted,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    assert!(
                        init_child.0.try_wait().unwrap().is_none(),
                        "private PID1 exited before handshake"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(error) => panic!("private PID1 handshake: {error}"),
            }
        };
        let mut signal = [0u8; 1];
        init_stream.read_exact(&mut signal).unwrap();
        assert_eq!(signal, [b'1']);
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        assert_eq!(
            unsafe {
                libc::getsockopt(
                    init_stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PEERCRED,
                    credentials.as_mut_ptr().cast(),
                    &mut length,
                )
            },
            0
        );
        let init = PinnedProcess::open(unsafe { credentials.assume_init().pid }).unwrap();
        let root_record = RootRecord {
            version: 1,
            boot_id: init.boot_id.clone(),
            root_id: root.clone(),
            owner_uid: peer.uid,
            init_host_pid: init.host_pid,
            init_starttime_ticks: init.starttime_ticks,
            pidns_dev: init.pidns_dev,
            pidns_ino: init.pidns_ino,
        };
        fs::write(
            registry_dir.join(format!("{root}.json")),
            serde_json::to_vec(&root_record).unwrap(),
        )
        .unwrap();
        let entry = EntryRecord {
            version: 1,
            root_id: root.clone(),
            owner_uid: peer.uid,
            entry: stamp.clone(),
            prepared_guardian: Some(stamp.clone()),
            domain_id: Some(owner.domain_id.clone()),
            supervisor_authority_id: Some(owner.supervisor_authority_id.clone()),
            guardian: Some(stamp.clone()),
            join_consumed: true,
            joined_child: Some(stamp.clone()),
        };
        fs::write(
            registry_dir.join("entries").join(format!("{root}.json")),
            serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();
        let roots = RootRegistry::open(&registry_dir).unwrap();
        let entries = EntryRegistry::open(registry_dir.join("entries")).unwrap();
        let works = WorkRegistry::open(registry_dir.join("works"), &roots).unwrap();
        let grants_dir = registry_dir.join("grants");
        let mut grants = GrantRegistry::open(&grants_dir).unwrap();
        let first = grants
            .prepare_native(
                &roots,
                &entries,
                &works,
                &peer,
                &host_namespace,
                &runner_image,
                &directory,
                &request,
                &receipt,
                &root,
                &attempt.attempt_id,
                &owner.owner_generation,
                &receipt_sha,
            )
            .unwrap();
        assert_eq!(first.version, 4);
        assert_eq!(first.kind, "native-continuation-v1");
        assert_eq!(first.state, "prepared");
        assert!(
            grants
                .prepare_native(
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &root,
                    &uuid::Uuid::new_v4().to_string(),
                    &owner.owner_generation,
                    &receipt_sha,
                )
                .is_err()
        );
        let mut reopened = GrantRegistry::open(&grants_dir).unwrap();
        let replay = reopened
            .prepare_native(
                &roots,
                &entries,
                &works,
                &peer,
                &host_namespace,
                &runner_image,
                &directory,
                &request,
                &receipt,
                &root,
                &attempt.attempt_id,
                &owner.owner_generation,
                &receipt_sha,
            )
            .unwrap();
        assert_eq!(replay, first);
        let k_spec = NativeKSpec {
            protocol: "native-continuation-v1".into(),
            grant_id: first.grant_id.clone(),
            root_id: root.clone(),
            attempt_id: attempt.attempt_id.clone(),
            owner_generation: owner.owner_generation.clone(),
            receipt_sha256: receipt_sha.clone(),
        };
        let wrong_k_spec = NativeKSpec {
            grant_id: uuid::Uuid::new_v4().to_string(),
            ..k_spec.clone()
        };
        let grant_path = grants_dir.join(format!("{}.json", first.grant_id));
        let mut old_v4 = serde_json::to_value(&first).unwrap();
        old_v4.as_object_mut().unwrap().remove("state_path");
        old_v4.as_object_mut().unwrap().remove("state_file");
        fs::write(&grant_path, serde_json::to_vec(&old_v4).unwrap()).unwrap();
        let mut old_registry = GrantRegistry::open(&grants_dir).unwrap();
        assert_eq!(
            old_registry
                .prepare_native(
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &root,
                    &attempt.attempt_id,
                    &owner.owner_generation,
                    &receipt_sha,
                )
                .unwrap()
                .grant_id,
            first.grant_id
        );
        assert!(
            old_registry
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar,
                )
                .is_err()
        );
        fs::write(&grant_path, serde_json::to_vec(&first).unwrap()).unwrap();
        reopened = GrantRegistry::open(&grants_dir).unwrap();
        // No K transition can be inferred from the prepared broker record.
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar
                )
                .is_err()
        );
        assert_eq!(
            reopened.native_record(&attempt.attempt_id).unwrap().state,
            "prepared"
        );
        let mut wrong_snapshot = accepted.clone();
        wrong_snapshot.kernel_root_id = uuid::Uuid::new_v4().to_string();
        assert!(
            db.bind_exact_native_grant(
                &wrong_snapshot,
                &first.grant_id,
                &verified.custodian_request_sha256,
            )
            .is_err()
        );
        assert_eq!(db.native_grant_binding(&attempt.attempt_id).unwrap(), None);
        let bound_row = db
            .bind_exact_native_grant(
                &accepted,
                &first.grant_id,
                &verified.custodian_request_sha256,
            )
            .unwrap();
        assert_eq!(
            db.native_grant_binding(&attempt.attempt_id).unwrap(),
            Some(bound_row.clone())
        );
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &wrong_k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar
                )
                .is_err()
        );
        let wrong_peer = PeerIdentity {
            uid: peer.uid.wrapping_add(1),
            gid: peer.gid,
            process: PinnedProcess::open(std::process::id() as i32).unwrap(),
        };
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &wrong_peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar
                )
                .is_err()
        );
        assert_eq!(
            reopened.native_record(&attempt.attempt_id).unwrap().state,
            "prepared"
        );
        fs::create_dir(dir.path().join("unrelated")).unwrap();
        let unrelated_path = dir.path().join("unrelated/pid-identity.db");
        let _unrelated_state =
            MailboxDb::open_completion_continuation_domain(&unrelated_path).unwrap();
        let unrelated_sidecar = File::open(&unrelated_path).unwrap();
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &unrelated_sidecar
                )
                .is_err()
        );
        // A copied sidecar has the exact same v28 row but a different inode.
        let copied_path = dir.path().join("unrelated/copied.db");
        rusqlite::Connection::open(&state_path)
            .unwrap()
            .execute("VACUUM INTO ?1", [copied_path.to_str().unwrap()])
            .unwrap();
        let copied_db = MailboxDb::open_read_only(&copied_path).unwrap();
        assert_eq!(
            copied_db.native_grant_binding(&attempt.attempt_id).unwrap(),
            Some(bound_row.clone())
        );
        let copied_sidecar = File::open(&copied_path).unwrap();
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &copied_sidecar
                )
                .is_err()
        );
        fs::write(&request_path, b"changed before K").unwrap();
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar
                )
                .is_err()
        );
        fs::write(&request_path, &request_bytes).unwrap();
        let verified_k = reopened
            .inspect_legacy_native_k_preflight(
                &k_spec,
                &roots,
                &entries,
                &works,
                &peer,
                &host_namespace,
                &runner_image,
                &directory,
                &request,
                &receipt,
                &sidecar,
            )
            .unwrap();
        assert_eq!(verified_k.state, "prepared");
        let refused = reopened
            .verify_native_k(
                &k_spec,
                &roots,
                &entries,
                &works,
                &peer,
                &host_namespace,
                &runner_image,
                &directory,
                &request,
                &receipt,
                &sidecar,
            )
            .unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("requires broker-owned accepted State authority")
        );
        assert_eq!(
            reopened.native_record(&attempt.attempt_id).unwrap().state,
            "prepared"
        );
        let foreign_guardian = PeerIdentity {
            uid: peer.uid,
            gid: peer.gid,
            process: PinnedProcess::open(roots.live_roots().next().unwrap().init.host_pid).unwrap(),
        };
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &foreign_guardian,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar,
                )
                .is_err()
        );
        for conflict in [
            NativeKSpec {
                root_id: uuid::Uuid::new_v4().to_string(),
                ..k_spec.clone()
            },
            NativeKSpec {
                attempt_id: uuid::Uuid::new_v4().to_string(),
                ..k_spec.clone()
            },
            NativeKSpec {
                owner_generation: uuid::Uuid::new_v4().to_string(),
                ..k_spec.clone()
            },
            NativeKSpec {
                receipt_sha256: "b".repeat(64),
                ..k_spec.clone()
            },
        ] {
            assert!(
                reopened
                    .inspect_legacy_native_k_preflight(
                        &conflict,
                        &roots,
                        &entries,
                        &works,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &directory,
                        &request,
                        &receipt,
                        &sidecar,
                    )
                    .is_err()
            );
        }
        assert!(
            reopened
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar
                )
                .is_ok()
        );
        assert!(
            reopened
                .prepare_native(
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &root,
                    &attempt.attempt_id,
                    &owner.owner_generation,
                    &receipt_sha,
                )
                .is_ok()
        );
        assert!(
            db.native_worker_attach(&attempt.attempt_id)
                .unwrap()
                .is_none()
        );
        assert!(
            db.bind_exact_native_grant(
                &accepted,
                &uuid::Uuid::new_v4().to_string(),
                &verified.custodian_request_sha256,
            )
            .is_err()
        );
        drop(db);
        let restarted_state = MailboxDb::open_completion_continuation_domain(&state_path).unwrap();
        assert_eq!(
            restarted_state
                .native_grant_binding(&attempt.attempt_id)
                .unwrap(),
            Some(bound_row)
        );
        let restarted_grants = GrantRegistry::open(&grants_dir).unwrap();
        assert_eq!(
            restarted_grants
                .native_record(&attempt.attempt_id)
                .unwrap()
                .state,
            "prepared"
        );
        assert!(
            restarted_grants
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar,
                )
                .is_ok()
        );
        assert!(
            restarted_grants
                .verify_native_k(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar,
                )
                .unwrap_err()
                .to_string()
                .contains("requires broker-owned accepted State authority")
        );
        assert!(
            restarted_state
                .native_worker_attach(&attempt.attempt_id)
                .unwrap()
                .is_none()
        );
        // A legacy spent v4 record remains spent on restart; this test does
        // not create one through the now-closed native K transport.
        let mut spent = first.clone();
        spent.state = "consumed".into();
        fs::write(
            grants_dir.join(format!("{}.json", first.grant_id)),
            serde_json::to_vec(&spent).unwrap(),
        )
        .unwrap();
        let mut spent_registry = GrantRegistry::open(&grants_dir).unwrap();
        assert!(
            spent_registry
                .inspect_legacy_native_k_preflight(
                    &k_spec,
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar,
                )
                .is_err()
        );
        assert!(
            spent_registry
                .prepare_native(
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &root,
                    &attempt.attempt_id,
                    &owner.owner_generation,
                    &receipt_sha,
                )
                .is_err()
        );
        let duplicate_id = uuid::Uuid::new_v4().to_string();
        let duplicate_path = grants_dir.join(format!("{duplicate_id}.json"));
        let mut duplicate = serde_json::to_value(&first).unwrap();
        duplicate["grant_id"] = duplicate_id.into();
        fs::write(&duplicate_path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
        assert!(GrantRegistry::open(&grants_dir).is_err());
        fs::remove_file(duplicate_path).unwrap();
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
        let wrong_uid = BoundNativeAuthority {
            owner_uid: peer.uid.wrapping_add(1),
            ..bound
        };
        assert!(verify(&peer, &wrong_uid, &directory, &request, &receipt).is_err());
        let wrong_sha = BoundNativeAuthority {
            receipt_sha256: &"0".repeat(64),
            ..bound
        };
        assert!(verify(&peer, &wrong_sha, &directory, &request, &receipt).is_err());
        fs::write(&request_path, b"replaced bytes").unwrap();
        assert!(verify(&peer, &bound, &directory, &request, &receipt).is_err());
        assert!(
            reopened
                .prepare_native(
                    &roots,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &root,
                    &attempt.attempt_id,
                    &owner.owner_generation,
                    &receipt_sha,
                )
                .is_err()
        );
        fs::write(&request_path, &request_bytes).unwrap();
        // The copied database contains the exact bound row, but replacing the
        // pathname after N cannot turn it into N's pinned file. Do not open
        // this copied file with SQLite under the old WAL name after the swap.
        let held_state = dir.path().join("held-pid-identity.db");
        fs::rename(&state_path, &held_state).unwrap();
        fs::rename(&copied_path, &state_path).unwrap();
        let replaced = reopened
            .inspect_legacy_native_k_preflight(
                &k_spec,
                &roots,
                &entries,
                &works,
                &peer,
                &host_namespace,
                &runner_image,
                &directory,
                &request,
                &receipt,
                &sidecar,
            )
            .unwrap_err();
        assert!(replaced.to_string().contains("sidecar path/inode conflict"));
        assert_eq!(
            reopened.native_record(&attempt.attempt_id).unwrap().state,
            "prepared"
        );
        fs::rename(&receipt_path, dir.path().join("moved-receipt")).unwrap();
        assert!(verify(&peer, &bound, &directory, &request, &receipt).is_err());
    }
}
