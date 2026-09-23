//! Root process-tree launch authority. The driver proposes an already-reserved
//! attempt; only the guardian accepts, forks, grants execution, reaps, and
//! integrates its exact terminal result. Per-operation workers isolate physical
//! child-tree draining but hold no SQLite or replay authority.

use super::custody::{CustodianRequest, LaunchRecipe};
use oulipoly_state::diagnostic_recorder::{
    DiagnosticId, DiagnosticPhase, PhaseObservation, SpanStart, process_recorder,
};
use oulipoly_state::mailbox::{CompletionDomainOwner, ContinuationAttempt, MailboxDb};
use std::cell::RefCell;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const ROOT_WORKER_ARG: &str = "__completion-root-worker-v1";
const NATIVE_ACCEPTED_FILE: &str = "native-continuation-accepted-v1.json";
const MAX_NATIVE_REQUEST_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CONTROL_FRAME: usize = 64 * 1024;
const RETAINED_RESULT_BATCH: usize = 256;
const DATABASE_OBSERVATION_INTERVAL: Duration = Duration::from_millis(250);
const CONTROL_READ_BUFFER_BYTES: usize = 4 * 1024;

#[derive(serde::Serialize)]
#[serde(deny_unknown_fields)]
struct NativeAcceptedReceipt<'a> {
    protocol: &'static str,
    accepted: &'a oulipoly_state::mailbox::AcceptedNativeGrantSnapshot,
    accepted_sha256: String,
    custodian_request_sha256: String,
    request_name: &'static str,
    request_device: u64,
    request_inode: u64,
    request_byte_len: u64,
}

fn exact_request_bytes(path: &Path, file: &std::fs::File) -> Result<Vec<u8>, String> {
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    let named = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || !named.is_file()
        || metadata.dev() != named.dev()
        || metadata.ino() != named.ino()
        || metadata.len() > MAX_NATIVE_REQUEST_BYTES
    {
        return Err("native request descriptor/name conflict".into());
    }
    let mut bytes = vec![0; usize::try_from(metadata.len()).map_err(|e| e.to_string())?];
    file.read_exact_at(&mut bytes, 0)
        .map_err(|e| e.to_string())?;
    let after = file.metadata().map_err(|e| e.to_string())?;
    if after.len() != metadata.len()
        || after.dev() != metadata.dev()
        || after.ino() != metadata.ino()
    {
        return Err("native request changed during descriptor read".into());
    }
    Ok(bytes)
}

fn publish_native_receipt(
    request_path: &Path,
    request_file: &std::fs::File,
    request_bytes: &[u8],
    accepted: &oulipoly_state::mailbox::AcceptedNativeGrantSnapshot,
) -> Result<(), String> {
    // Recheck the name and all bytes after State commit. Broker will repeat
    // these checks on its descriptor; a renamed/replaced request cannot ride
    // on this guardian's receipt.
    if exact_request_bytes(request_path, request_file)? != request_bytes {
        return Err("native request changed after acceptance".into());
    }
    let metadata = request_file.metadata().map_err(|e| e.to_string())?;
    let accepted_bytes = serde_json::to_vec(accepted).map_err(|e| e.to_string())?;
    let receipt = NativeAcceptedReceipt {
        protocol: "native-continuation-accepted-v1",
        accepted,
        accepted_sha256: oulipoly_state::completion_continuation::sha256(&accepted_bytes),
        custodian_request_sha256: oulipoly_state::completion_continuation::sha256(request_bytes),
        request_name: "custodian-request.json",
        request_device: metadata.dev(),
        request_inode: metadata.ino(),
        request_byte_len: metadata.len(),
    };
    let path = request_path.with_file_name(NATIVE_ACCEPTED_FILE);
    let temp = request_path.with_file_name(format!(
        ".native-continuation-accepted-{}.tmp",
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec(&receipt).map_err(|e| e.to_string())?)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        if exact_request_bytes(request_path, request_file)? != request_bytes {
            return Err("native request changed before receipt publication".into());
        }
        std::fs::hard_link(&temp, &path).map_err(|e| e.to_string())?;
        std::fs::File::open(path.parent().ok_or("receipt directory absent")?)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_file(&temp);
    result
}

#[cfg(test)]
mod native_acceptance_tests {
    use super::*;
    use oulipoly_state::completion_continuation::SourceProcessIdentity;

    fn snapshot(result_path: &Path) -> oulipoly_state::mailbox::AcceptedNativeGrantSnapshot {
        let guardian_identity = SourceProcessIdentity {
            pid: 123,
            boot_id: "boot".into(),
            starttime_ticks: 456,
        };
        let owner_generation = uuid::Uuid::new_v4().to_string();
        oulipoly_state::mailbox::AcceptedNativeGrantSnapshot {
            attempt: ContinuationAttempt {
                attempt_id: uuid::Uuid::new_v4().to_string(),
                owner_generation: owner_generation.clone(),
                operation: "transport".into(),
                request_sha256: "a".repeat(64),
                source_registration_id: None,
                source_listener_revision: None,
                session_id: None,
                claim_token: None,
                result_path: result_path.to_str().unwrap().into(),
            },
            domain_id: uuid::Uuid::new_v4().to_string(),
            kernel_root_id: uuid::Uuid::new_v4().to_string(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation,
            guardian_identity,
            phase: "accepted".into(),
            revision: 2,
            integrated: false,
            custodian_identity: None,
            adopter_identity: None,
        }
    }

    #[test]
    fn native_receipt_binds_write_once_request_descriptor_and_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let request = dir.path().join("custodian-request.json");
        let receipt = dir.path().join(NATIVE_ACCEPTED_FILE);
        let accepted = snapshot(&dir.path().join("result.json"));
        assert!(std::fs::File::open(&request).is_err());
        assert!(!receipt.exists());
        super::super::custody::write_request_once(&request, b"request-one").unwrap();
        assert!(super::super::custody::write_request_once(&request, b"request-two").is_err());
        let file = std::fs::File::open(&request).unwrap();
        assert_eq!(
            exact_request_bytes(&request, &file).unwrap(),
            b"request-one"
        );
        assert!(publish_native_receipt(&request, &file, b"request-two", &accepted).is_err());
        assert!(!receipt.exists());
        std::fs::write(&request, b"request-two").unwrap();
        assert!(publish_native_receipt(&request, &file, b"request-one", &accepted).is_err());
        assert!(!receipt.exists());
        std::fs::write(&request, b"request-one").unwrap();
        let moved = dir.path().join("moved-request");
        std::fs::rename(&request, &moved).unwrap();
        assert!(publish_native_receipt(&request, &file, b"request-one", &accepted).is_err());
        assert!(!receipt.exists());
        std::fs::rename(&moved, &request).unwrap();
        publish_native_receipt(&request, &file, b"request-one", &accepted).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&receipt).unwrap()).unwrap();
        assert_eq!(parsed["protocol"], "native-continuation-accepted-v1");
        assert_eq!(
            parsed["accepted"]["attempt"]["attempt_id"],
            accepted.attempt.attempt_id
        );
        assert_eq!(
            parsed["custodian_request_sha256"],
            oulipoly_state::completion_continuation::sha256(b"request-one")
        );
        assert_eq!(parsed["request_inode"], file.metadata().unwrap().ino());
        assert!(publish_native_receipt(&request, &file, b"request-one", &accepted).is_err());
        std::fs::rename(&request, dir.path().join("old-request")).unwrap();
        std::fs::write(&request, b"request-one").unwrap();
        assert!(exact_request_bytes(&request, &file).is_err());
    }

    #[test]
    fn committed_acceptance_crash_window_has_no_positive_artifact_or_replay() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("pid-identity.db");
        let mut db =
            oulipoly_state::mailbox::MailboxDb::open_completion_continuation_domain(&state_path)
                .unwrap();
        let identity = super::super::linux::current_identity().unwrap();
        let owner = CompletionDomainOwner {
            protocol: oulipoly_state::completion_continuation::PROTOCOL.into(),
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
        let result_path = dir.path().join("result.json");
        let attempt = ContinuationAttempt {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: owner.owner_generation.clone(),
            operation: "transport".into(),
            request_sha256: "a".repeat(64),
            source_registration_id: None,
            source_listener_revision: None,
            session_id: None,
            claim_token: None,
            result_path: result_path.to_string_lossy().into(),
        };
        db.reserve_continuation_attempt(&attempt).unwrap();
        let receipt = dir.path().join(NATIVE_ACCEPTED_FILE);
        let mut wrong = owner.clone();
        wrong.guardian_identity.starttime_ticks += 1;
        assert!(
            db.accept_exact_native_attempt(&attempt, &wrong, &root)
                .is_err()
        );
        assert!(!receipt.exists());
        let accepted = db
            .accept_exact_native_attempt(&attempt, &owner, &root)
            .unwrap();
        assert_eq!(accepted.phase, "accepted");
        drop(db); // models death after State commit, before receipt publication
        assert!(!receipt.exists());
        let mut recovered = oulipoly_state::mailbox::MailboxDb::open(&state_path).unwrap();
        assert!(
            recovered
                .accept_exact_native_attempt(&attempt, &owner, &root)
                .is_err()
        );
        assert!(!receipt.exists());
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchRequest {
    attempt: ContinuationAttempt,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchResponse {
    attempt_id: String,
    worker_pid: Option<i64>,
    error: Option<String>,
}

struct DriverClient {
    socket: UnixStream,
    response: Vec<u8>,
}

thread_local! {
    static DRIVER_CLIENT: RefCell<Option<DriverClient>> = const { RefCell::new(None) };
}

pub(super) fn install_driver_channel(socket: UnixStream) {
    DRIVER_CLIENT.with_borrow_mut(|client| {
        *client = Some(DriverClient {
            socket,
            response: Vec::new(),
        });
    });
}

#[cfg(test)]
pub(super) fn clear_driver_channel() {
    DRIVER_CLIENT.with_borrow_mut(|client| *client = None);
}

#[cfg(test)]
pub(super) fn driver_channel_installed() -> bool {
    DRIVER_CLIENT.with_borrow(|client| client.is_some())
}

pub(super) fn request_launch(
    path: &Path,
    attempt: &ContinuationAttempt,
    recipe: LaunchRecipe,
) -> Result<i64, String> {
    let request = CustodianRequest {
        path: path.to_path_buf(),
        attempt: attempt.clone(),
        recipe,
    };
    let request_path = super::custody::request_path(attempt)?;
    super::custody::write_request_once(
        &request_path,
        &serde_json::to_vec(&request).map_err(|error| error.to_string())?,
    )?;
    DRIVER_CLIENT.with_borrow_mut(|client| {
        let client = client
            .as_mut()
            .ok_or("root supervisor launch channel is unavailable")?;
        write_frame(
            &mut client.socket,
            &LaunchRequest {
                attempt: attempt.clone(),
            },
        )?;
        let response: LaunchResponse = read_frame(&mut client.socket, &mut client.response)?;
        if response.attempt_id != attempt.attempt_id {
            return Err("root supervisor launch response attempt conflict".into());
        }
        match (response.worker_pid, response.error) {
            (Some(pid), None) => Ok(pid),
            (None, Some(error)) => Err(error),
            _ => Err("invalid root supervisor launch response".into()),
        }
    })
}

fn write_frame<T: serde::Serialize>(socket: &mut UnixStream, value: &T) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_CONTROL_FRAME {
        return Err("root supervisor control frame is too large".into());
    }
    bytes.push(b'\n');
    socket.write_all(&bytes).map_err(|error| error.to_string())
}

fn read_frame<T: serde::de::DeserializeOwned>(
    socket: &mut UnixStream,
    buffer: &mut Vec<u8>,
) -> Result<T, String> {
    loop {
        if let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let frame = buffer.drain(..=end).collect::<Vec<_>>();
            return serde_json::from_slice(&frame[..frame.len() - 1])
                .map_err(|error| error.to_string());
        }
        if buffer.len() > MAX_CONTROL_FRAME {
            return Err("root supervisor control frame is too large".into());
        }
        let mut chunk = [0; CONTROL_READ_BUFFER_BYTES];
        let count = socket.read(&mut chunk).map_err(|error| error.to_string())?;
        if count == 0 {
            return Err("root supervisor launch channel closed".into());
        }
        buffer.extend_from_slice(&chunk[..count]);
    }
}

struct ActiveOperation {
    attempt: ContinuationAttempt,
    diagnostic_id: DiagnosticId,
    worker: Option<Child>,
    worker_identity: Option<oulipoly_state::completion_continuation::SourceProcessIdentity>,
    unreleased_reason: Option<String>,
    never_forked: bool,
    broker_pending: bool,
    cancellation: UnixStream,
    cancellation_sent: bool,
    cancellation_output: Vec<u8>,
    cancellation_observation_error: Option<String>,
    cancellation_delivery_error: Option<String>,
    worker_wait_error: Option<String>,
    terminal_integration_error: Option<String>,
    next_cancellation_poll: Instant,
    next_integration_attempt: Instant,
    granted: bool,
    terminal: bool,
}

pub(super) struct RootSupervisor {
    path: PathBuf,
    driver: UnixStream,
    input: Vec<u8>,
    output: Vec<u8>,
    active: Vec<ActiveOperation>,
    next_reconcile: Instant,
    reconcile_cursor: Option<(String, String)>,
    reconcile_error: Option<String>,
    driver_error: Option<String>,
    original: super::original_work::OriginalWorkSupervisor,
    kernel_pinned: bool,
}

impl RootSupervisor {
    pub(super) fn new(path: &Path, driver: UnixStream) -> Result<Self, String> {
        driver
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            path: path.to_path_buf(),
            driver,
            input: Vec::new(),
            output: Vec::new(),
            active: Vec::new(),
            next_reconcile: Instant::now(),
            reconcile_cursor: None,
            reconcile_error: None,
            driver_error: None,
            original: super::original_work::OriginalWorkSupervisor::default(),
            kernel_pinned: false,
        })
    }

    pub(super) fn set_kernel_pinned(&mut self, pinned: bool) {
        self.kernel_pinned = pinned;
        self.original.set_kernel_pinned(pinned);
    }

    pub(super) fn replace_driver(&mut self, driver: UnixStream) -> Result<(), String> {
        driver
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        self.driver = driver;
        self.input.clear();
        // A reply lost with the old driver cannot authorize replay. The
        // retained attempt and active operation remain the durable answer for
        // the replacement driver.
        self.output.clear();
        self.driver_error = None;
        Ok(())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.active.is_empty() && self.original.is_empty()
    }

    pub(super) fn original_accepts_nested(
        &self,
        root_id: &str,
        parent_work_id: &str,
        parent_capability: &str,
        peer: &oulipoly_state::completion_continuation::SourceProcessIdentity,
    ) -> bool {
        self.original
            .accepts_nested(root_id, parent_work_id, parent_capability, peer)
    }

    pub(super) fn original_peer_in_root(
        &self,
        root_id: &str,
        peer: &oulipoly_state::completion_continuation::SourceProcessIdentity,
    ) -> bool {
        self.original.peer_in_root(root_id, peer)
    }

    pub(super) fn original_parent_for_peer(
        &self,
        root_id: &str,
        peer: &oulipoly_state::completion_continuation::SourceProcessIdentity,
    ) -> Option<&str> {
        self.original.parent_for_peer(root_id, peer)
    }

    pub(super) fn original_root_for_peer(
        &self,
        peer: &oulipoly_state::completion_continuation::SourceProcessIdentity,
    ) -> Option<&str> {
        self.original.root_for_peer(peer)
    }

    pub(super) fn original_worker_pids(&self) -> Vec<i64> {
        self.original.worker_pids()
    }

    pub(super) fn known_direct_child_pids(&self) -> Vec<i64> {
        let mut children = self.original.live_worker_pids();
        children.extend(
            self.active
                .iter()
                .filter(|operation| !operation.terminal)
                .filter_map(|operation| operation.worker_identity.as_ref())
                .filter(|worker| super::linux::identity(worker.pid).as_ref() == Ok(worker))
                .map(|worker| worker.pid),
        );
        children
    }

    pub(super) fn original_active_root_ids(&self) -> std::collections::BTreeSet<String> {
        self.original.active_root_ids()
    }

    pub(super) fn submit_original(
        &mut self,
        owner: &CompletionDomainOwner,
        request: super::original_work::InboundWork,
    ) {
        self.original.submit(owner, request);
    }

    pub(super) fn cancel_original(
        &mut self,
        owner: &CompletionDomainOwner,
        request: super::original_work::InboundCancel,
    ) {
        self.original.cancel(owner, request);
    }

    fn retain_never_forked(
        &mut self,
        attempt: &ContinuationAttempt,
        diagnostic_id: DiagnosticId,
        cancellation: UnixStream,
        reason: String,
    ) {
        self.active.push(ActiveOperation {
            attempt: attempt.clone(),
            diagnostic_id,
            worker: None,
            worker_identity: None,
            unreleased_reason: Some(reason),
            never_forked: true,
            broker_pending: false,
            cancellation,
            cancellation_sent: false,
            cancellation_output: Vec::new(),
            cancellation_observation_error: None,
            cancellation_delivery_error: None,
            worker_wait_error: None,
            terminal_integration_error: None,
            next_cancellation_poll: Instant::now(),
            next_integration_attempt: Instant::now(),
            granted: false,
            terminal: true,
        });
    }

    fn retain_native_broker_pending(
        &mut self,
        attempt: &ContinuationAttempt,
        diagnostic_id: DiagnosticId,
        cancellation: UnixStream,
        reason: String,
    ) {
        self.active.push(ActiveOperation {
            attempt: attempt.clone(),
            diagnostic_id,
            worker: None,
            worker_identity: None,
            unreleased_reason: Some(reason),
            never_forked: false,
            broker_pending: true,
            cancellation,
            cancellation_sent: false,
            cancellation_output: Vec::new(),
            cancellation_observation_error: None,
            cancellation_delivery_error: None,
            worker_wait_error: None,
            terminal_integration_error: None,
            next_cancellation_poll: Instant::now(),
            next_integration_attempt: Instant::now(),
            granted: false,
            terminal: true,
        });
    }

    pub(super) fn tick(
        &mut self,
        owner: &CompletionDomainOwner,
        adopted_child_live: bool,
    ) -> Result<(), String> {
        if self.driver_error.is_none() {
            let driver_result = self
                .flush_driver_output()
                .and_then(|()| self.collect_driver_input(owner))
                .and_then(|()| self.flush_driver_output());
            if let Err(error) = driver_result {
                self.fail_driver(owner, &error);
            }
        }
        self.observe_active(owner);
        self.original.set_adopted_child_gate(adopted_child_live);
        self.original.tick(owner);
        let now = Instant::now();
        if now >= self.next_reconcile {
            // Retained-result reconciliation is recovery, not guardian
            // liveness. SQLite contention leaves the bounded obligation for a
            // later pass and must not destroy the process-tree authority.
            match self.reconcile_retained_results(owner) {
                Ok(()) => self.reconcile_error = None,
                Err(error) => {
                    if self.reconcile_error.as_deref() != Some(error.as_str()) {
                        let start =
                            SpanStart::new("root_supervisor_reconcile", "pid_mailbox_sqlite")
                                .with_lifecycle_phase("completion_recovery_authority")
                                .with_identifier(
                                    "supervisor_authority_id",
                                    &owner.supervisor_authority_id,
                                );
                        process_recorder().with_requested_span(start, |span| {
                            let _ = span.record(
                                DiagnosticPhase::Failed,
                                PhaseObservation::started_unknown()
                                    .with_cause("root_reconciliation_deferred")
                                    .with_cause(&error),
                            );
                        });
                    }
                    self.reconcile_error = Some(error);
                }
            }
            self.next_reconcile = now + DATABASE_OBSERVATION_INTERVAL;
        }
        Ok(())
    }

    fn flush_driver_output(&mut self) -> Result<(), String> {
        while !self.output.is_empty() {
            match self.driver.write(&self.output) {
                Ok(0) => {
                    return Err("root supervisor driver response channel wrote zero bytes".into());
                }
                Ok(written) => {
                    self.output.drain(..written);
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    // Driver loss cannot roll back a launch. A replacement
                    // gets state from the immutable attempt/result boundary.
                    self.output.clear();
                    return Err(format!(
                        "root supervisor driver response channel failed: {error}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn fail_driver(&mut self, owner: &CompletionDomainOwner, error: &str) {
        let first_observation = self.driver_error.as_deref() != Some(error);
        if first_observation {
            let start = SpanStart::new("root_supervisor_driver_channel", "process_tree")
                .with_lifecycle_phase("completion_driver_authority")
                .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
                .with_hashed_correlation("owner_generation", &owner.owner_generation);
            process_recorder().with_requested_span(start, |span| {
                let _ = span.record(
                    DiagnosticPhase::Failed,
                    PhaseObservation::effects_possible()
                        .with_cause("driver_channel_retired")
                        .with_cause(error),
                );
            });
        }
        self.driver_error = Some(error.to_owned());
        self.input.clear();
        self.output.clear();
        // A malformed or failed control channel must not take down the root or
        // leave a live driver waiting forever for a reply. Retire only the exact
        // recorded incarnation; the guardian's ordinary child wait installs a
        // fresh driver beneath the same durable supervisor authority.
        if first_observation
            && super::linux::identity(owner.driver_identity.pid).as_ref()
                == Ok(&owner.driver_identity)
        {
            unsafe {
                libc::kill(owner.driver_identity.pid as i32, libc::SIGTERM);
            }
        }
    }

    fn reconcile_retained_results(&mut self, owner: &CompletionDomainOwner) -> Result<(), String> {
        let attempts = MailboxDb::open(&self.path)?
            .pending_continuation_attempts_for_supervisor_after(
                &owner.supervisor_authority_id,
                self.reconcile_cursor
                    .as_ref()
                    .map(|(generation, attempt)| (generation.as_str(), attempt.as_str())),
                RETAINED_RESULT_BATCH,
            )?;
        self.reconcile_cursor = (attempts.len() == RETAINED_RESULT_BATCH)
            .then(|| {
                attempts
                    .last()
                    .map(|attempt| (attempt.owner_generation.clone(), attempt.attempt_id.clone()))
            })
            .flatten();
        for attempt in attempts {
            if self
                .active
                .iter()
                .any(|active| active.attempt.attempt_id == attempt.attempt_id)
            {
                continue;
            }
            // Only this root process performs replay/integration. Missing
            // evidence or unknown custody remains unresolved and is never
            // converted into permission to execute a duplicate provider.
            let _ = super::custody::replay_result(&self.path, &attempt);
        }
        Ok(())
    }

    fn collect_driver_input(&mut self, owner: &CompletionDomainOwner) -> Result<(), String> {
        loop {
            let mut chunk = [0; CONTROL_READ_BUFFER_BYTES];
            match self.driver.read(&mut chunk) {
                Ok(0) => return Err("root supervisor driver channel closed".into()),
                Ok(count) => {
                    self.input.extend_from_slice(&chunk[..count]);
                    if self.input.len() > MAX_CONTROL_FRAME + 1 {
                        return Err("root supervisor driver frame is too large".into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    return Err(format!(
                        "root supervisor driver request channel failed: {error}"
                    ));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        while let Some(end) = self.input.iter().position(|byte| *byte == b'\n') {
            let frame = self.input.drain(..=end).collect::<Vec<_>>();
            let response = match serde_json::from_slice::<LaunchRequest>(&frame[..frame.len() - 1])
            {
                Ok(request) => self.launch(owner, request),
                Err(error) => LaunchResponse {
                    attempt_id: "invalid".into(),
                    worker_pid: None,
                    error: Some(format!("invalid root supervisor launch request: {error}")),
                },
            };
            // Never block the guardian behind a driver that stopped reading.
            // A lost reply never rolls back a granted launch; the attempt row
            // and active root custody prevent a duplicate provider execution.
            let mut encoded = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
            if encoded.len() > MAX_CONTROL_FRAME {
                return Err("root supervisor response frame is too large".into());
            }
            encoded.push(b'\n');
            self.output.extend(encoded);
            self.flush_driver_output()?;
            if !self.output.is_empty() {
                break;
            }
        }
        Ok(())
    }

    fn launch(&mut self, owner: &CompletionDomainOwner, request: LaunchRequest) -> LaunchResponse {
        let attempt = request.attempt;
        if let Some(active) = self
            .active
            .iter()
            .find(|active| active.attempt.attempt_id == attempt.attempt_id)
        {
            return LaunchResponse {
                attempt_id: attempt.attempt_id,
                worker_pid: active
                    .granted
                    .then(|| active.worker_identity.as_ref().map(|identity| identity.pid))
                    .flatten(),
                error: (!active.granted)
                    .then(|| "root supervisor retained an unreleased launch".into()),
            };
        }
        let start = SpanStart::new("root_supervisor_launch", "process_tree")
            .with_lifecycle_phase("completion_launch_authority")
            .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
            .with_hashed_correlation("attempt_id", &attempt.attempt_id);
        let diagnostic_id = start.diagnostic_id().clone();
        let result = process_recorder().with_requested_span(start, |span| {
            let result = self.launch_new(owner, &attempt, diagnostic_id);
            match &result {
                Ok(_) => {
                    let _ = span.record(
                        DiagnosticPhase::Committed,
                        PhaseObservation::committed().with_cause("root_execution_grant_sent"),
                    );
                }
                Err(_) => {
                    let _ = span.record(
                        DiagnosticPhase::Failed,
                        PhaseObservation::effects_possible()
                            .with_cause("root_launch_not_confirmed")
                            .with_cause(result.as_ref().unwrap_err()),
                    );
                }
            }
            result
        });
        match result {
            Ok(pid) => LaunchResponse {
                attempt_id: attempt.attempt_id,
                worker_pid: Some(pid),
                error: None,
            },
            Err(error) => LaunchResponse {
                attempt_id: attempt.attempt_id,
                worker_pid: None,
                error: Some(error),
            },
        }
    }

    fn launch_new(
        &mut self,
        owner: &CompletionDomainOwner,
        attempt: &ContinuationAttempt,
        diagnostic_id: DiagnosticId,
    ) -> Result<i64, String> {
        if attempt.owner_generation != owner.owner_generation {
            return Err("root supervisor rejected a foreign owner generation".into());
        }
        let request_path = super::custody::request_path(attempt)?;
        let request_file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&request_path)
            .map_err(|error| error.to_string())?;
        let request_bytes = exact_request_bytes(&request_path, &request_file)?;
        let request: CustodianRequest =
            serde_json::from_slice(&request_bytes).map_err(|error| error.to_string())?;
        if request.path != self.path || request.attempt != *attempt {
            return Err("root supervisor immutable launch request conflict".into());
        }
        let (mut grant, worker_gate) = UnixStream::pair().map_err(|error| error.to_string())?;
        let (cancellation, worker_cancellation) =
            UnixStream::pair().map_err(|error| error.to_string())?;
        cancellation
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;

        let mut mailbox = MailboxDb::open(&self.path)?;
        if self.kernel_pinned {
            let guardian = super::linux::current_identity()?;
            if guardian != owner.guardian_identity {
                return Err("native acceptance caller is not the original guardian".into());
            }
            let root = mailbox
                .completion_owner_kernel_root_id(&owner.owner_generation)?
                .ok_or("native acceptance has no pinned kernel root")?;
            let snapshot = mailbox.accept_exact_native_attempt(attempt, owner, &root)?;
            if let Err(error) =
                publish_native_receipt(&request_path, &request_file, &request_bytes, &snapshot)
            {
                self.retain_native_broker_pending(
                    attempt,
                    diagnostic_id,
                    cancellation,
                    format!(
                        "native acceptance committed but receipt publication is uncertain: {error}"
                    ),
                );
                return Err(error);
            }
            // A tagged broker grant, one-use K and Q settlement are separate
            // authority. Until those are wired, retain the accepted obligation
            // without executing a host-local worker.
            let reason =
                "native broker grant is not yet wired; accepted attempt retained without launch"
                    .to_owned();
            self.retain_native_broker_pending(attempt, diagnostic_id, cancellation, reason.clone());
            return Err(reason);
        }
        mailbox.accept_continuation_attempt(attempt)?;

        let gate_fd = worker_gate.as_raw_fd();
        let request_fd = request_file.as_raw_fd();
        let cancellation_fd = worker_cancellation.as_raw_fd();
        let mut command = Command::new("/proc/self/exe");
        command
            .arg(ROOT_WORKER_ARG)
            .arg(gate_fd.to_string())
            .arg(request_fd.to_string())
            .arg(cancellation_fd.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                for fd in [gate_fd, request_fd, cancellation_fd] {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let worker = match command.spawn() {
            Ok(worker) => worker,
            Err(error) => {
                let reason = format!("root supervisor worker spawn failed: {error}");
                // The live root is the only exact witness that spawn failed.
                // Retain that testimony in memory until its database outcome is
                // read back; returning an error to the driver must not abandon
                // an accepted attempt when SQLite is concurrently unavailable.
                self.retain_never_forked(attempt, diagnostic_id, cancellation, reason.clone());
                return Err(reason);
            }
        };
        drop(worker_gate);
        drop(worker_cancellation);
        drop(request_file);
        let worker_identity = match super::linux::direct_child_identity(worker.id()) {
            Ok(identity) => identity,
            Err(error) => {
                // The execution grant is still locally held, so closing it is
                // conclusive no-provider evidence. Keep the exact fork child
                // under the root until wait/ECHILD, then settle through the
                // recorded guardian rather than dropping an unknown Child.
                let reason = format!("root supervisor worker identity unavailable: {error}");
                drop(grant);
                self.active.push(ActiveOperation {
                    attempt: attempt.clone(),
                    diagnostic_id,
                    worker: Some(worker),
                    worker_identity: None,
                    unreleased_reason: Some(reason.clone()),
                    never_forked: false,
                    broker_pending: false,
                    cancellation,
                    cancellation_sent: false,
                    cancellation_output: Vec::new(),
                    cancellation_observation_error: None,
                    cancellation_delivery_error: None,
                    worker_wait_error: None,
                    terminal_integration_error: None,
                    next_cancellation_poll: Instant::now(),
                    next_integration_attempt: Instant::now(),
                    granted: false,
                    terminal: false,
                });
                return Err(reason);
            }
        };
        let mut operation = ActiveOperation {
            attempt: attempt.clone(),
            diagnostic_id,
            worker: Some(worker),
            worker_identity: Some(worker_identity.clone()),
            unreleased_reason: None,
            never_forked: false,
            broker_pending: false,
            cancellation,
            cancellation_sent: false,
            cancellation_output: Vec::new(),
            cancellation_observation_error: None,
            cancellation_delivery_error: None,
            worker_wait_error: None,
            terminal_integration_error: None,
            next_cancellation_poll: Instant::now(),
            next_integration_attempt: Instant::now(),
            granted: false,
            terminal: false,
        };
        let grant_result = mailbox
            .attach_continuation_custodian_with_adopter(
                attempt,
                &worker_identity,
                Some(&owner.guardian_identity),
            )
            .and_then(|()| {
                mailbox
                    .advance_continuation_attempt(
                        attempt,
                        3,
                        "accepted",
                        "starting",
                        &worker_identity,
                    )
                    .map(|_| ())
            })
            .and_then(|()| grant.write_all(&[1]).map_err(|error| error.to_string()));
        drop(grant);
        if grant_result.is_ok() {
            operation.granted = true;
        }
        let pid = worker_identity.pid;
        self.active.push(operation);
        grant_result.map(|()| pid)
    }

    fn observe_active(&mut self, owner: &CompletionDomainOwner) {
        let now = Instant::now();
        for operation in &mut self.active {
            if !operation.terminal
                && operation.granted
                && !operation.cancellation_sent
                && operation.cancellation_output.is_empty()
                && now >= operation.next_cancellation_poll
            {
                match super::custody::root_cancellation_identity(&self.path, &operation.attempt) {
                    Ok(Some(identity)) => {
                        operation.cancellation_observation_error = None;
                        if !identity.contains('\n') && identity.len() <= 4096 {
                            operation.cancellation_output = format!("{identity}\n").into_bytes();
                        } else {
                            let message =
                                "root cancellation identity exceeds its bounded frame".to_owned();
                            record_operation_failure_once(
                                operation,
                                "root_supervisor_cancellation_delivery",
                                "completion_cancellation_authority",
                                "cancellation_identity_invalid",
                                &message,
                            );
                            operation.cancellation_delivery_error = Some(message);
                        }
                    }
                    Ok(None) => operation.cancellation_observation_error = None,
                    Err(error) => {
                        if operation.cancellation_observation_error.as_deref()
                            != Some(error.as_str())
                        {
                            let start = SpanStart::new(
                                "root_supervisor_cancellation_observation",
                                "state_and_pid_mailbox",
                            )
                            .with_lifecycle_phase("completion_cancellation_authority")
                            .with_diagnostic_id(operation.diagnostic_id.clone())
                            .with_hashed_correlation("attempt_id", &operation.attempt.attempt_id);
                            process_recorder().with_requested_span(start, |span| {
                                let _ = span.record(
                                    DiagnosticPhase::Failed,
                                    PhaseObservation::started_unknown()
                                        .with_cause("cancellation_observation_deferred")
                                        .with_cause(&error),
                                );
                            });
                        }
                        operation.cancellation_observation_error = Some(error);
                    }
                }
                operation.next_cancellation_poll = now + DATABASE_OBSERVATION_INTERVAL;
            }
            while !operation.cancellation_output.is_empty() {
                match operation.cancellation.write(&operation.cancellation_output) {
                    Ok(0) => {
                        let message = "root cancellation channel wrote zero bytes".to_owned();
                        record_operation_failure_once(
                            operation,
                            "root_supervisor_cancellation_delivery",
                            "completion_cancellation_authority",
                            "cancellation_delivery_failed",
                            &message,
                        );
                        operation.cancellation_delivery_error = Some(message);
                        operation.cancellation_output.clear();
                        break;
                    }
                    Ok(written) => {
                        operation.cancellation_output.drain(..written);
                        operation.cancellation_sent = operation.cancellation_output.is_empty();
                        if operation.cancellation_sent {
                            operation.cancellation_delivery_error = None;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        let message = format!("root cancellation channel failed: {error}");
                        record_operation_failure_once(
                            operation,
                            "root_supervisor_cancellation_delivery",
                            "completion_cancellation_authority",
                            "cancellation_delivery_failed",
                            &message,
                        );
                        operation.cancellation_delivery_error = Some(message);
                        operation.cancellation_output.clear();
                        break;
                    }
                }
            }
            if !operation.terminal {
                let worker = operation
                    .worker
                    .as_mut()
                    .expect("a forked root operation retains its exact child");
                match worker.try_wait() {
                    Ok(Some(_)) => {
                        operation.worker_wait_error = None;
                        operation.terminal = true;
                    }
                    Ok(None) => operation.worker_wait_error = None,
                    Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
                        operation.worker_wait_error = None;
                        operation.terminal = true;
                    }
                    Err(error) => {
                        let message = format!("root worker wait failed: {error}");
                        record_operation_failure_once(
                            operation,
                            "root_supervisor_worker_wait",
                            "completion_terminal_authority",
                            "worker_wait_deferred",
                            &message,
                        );
                        operation.worker_wait_error = Some(message);
                    }
                }
            }
        }
        let path = self.path.clone();
        self.active.retain_mut(|operation| {
            if operation.broker_pending {
                return true;
            }
            if !operation.terminal || now < operation.next_integration_attempt {
                return true;
            }
            let start = SpanStart::new("root_supervisor_terminal", "process_tree")
                .with_lifecycle_phase("completion_terminal_authority")
                .with_diagnostic_id(operation.diagnostic_id.clone())
                .with_hashed_correlation("attempt_id", &operation.attempt.attempt_id);
            let result = if operation.never_forked {
                MailboxDb::open(&path).and_then(|mut mailbox| {
                    mailbox.record_supervisor_never_forked(
                        &operation.attempt,
                        &owner.guardian_identity,
                        operation
                            .unreleased_reason
                            .as_deref()
                            .unwrap_or("root worker spawn failed"),
                    )
                })
            } else if operation.granted {
                #[cfg(feature = "age360-fault-fixtures")]
                oulipoly_state::completion_continuation::age360_fault_barrier(
                    "root-result-retained",
                );
                super::custody::replay_root_result(
                    &path,
                    &operation.attempt,
                    operation
                        .worker_identity
                        .as_ref()
                        .expect("granted operation has an exact worker identity"),
                )
            } else if let Some(worker_identity) = &operation.worker_identity {
                let receipt = serde_json::json!({
                    "attempt_id": operation.attempt.attempt_id,
                    "custodian": worker_identity,
                    "gate": "root_execution_grant_not_sent",
                    "owned_children": "ECHILD",
                })
                .to_string();
                MailboxDb::open(&path).and_then(|mut mailbox| {
                    mailbox.cancel_unreleased_continuation_gate(
                        &operation.attempt,
                        worker_identity,
                        &receipt,
                    )
                })
            } else {
                MailboxDb::open(&path).and_then(|mut mailbox| {
                    mailbox.record_supervisor_unreleased_worker(
                        &operation.attempt,
                        &owner.guardian_identity,
                        i64::from(
                            operation
                                .worker
                                .as_ref()
                                .expect("forked unreleased worker remains retained")
                                .id(),
                        ),
                        operation
                            .unreleased_reason
                            .as_deref()
                            .unwrap_or("root worker identity unavailable"),
                    )
                })
            };
            match &result {
                Ok(()) => {
                    operation.terminal_integration_error = None;
                    process_recorder().with_requested_span(start, |span| {
                        let _ = span.record(
                            DiagnosticPhase::Committed,
                            PhaseObservation::terminal()
                                .with_cause("root_terminal_result_integrated"),
                        );
                    });
                }
                Err(error) => {
                    if operation.terminal_integration_error.as_deref() != Some(error.as_str()) {
                        process_recorder().with_requested_span(start, |span| {
                            let _ = span.record(
                                DiagnosticPhase::Failed,
                                PhaseObservation::started_unknown()
                                    .with_cause("root_terminal_integration_pending")
                                    .with_cause(error),
                            );
                        });
                    }
                    operation.terminal_integration_error = Some(error.clone());
                }
            }
            if result.is_err() {
                operation.next_integration_attempt = now + DATABASE_OBSERVATION_INTERVAL;
            }
            result.is_err()
        });
    }
}

fn record_operation_failure_once(
    operation: &ActiveOperation,
    recorder_operation: &str,
    lifecycle_phase: &str,
    cause: &str,
    error: &str,
) {
    let already_recorded = match recorder_operation {
        "root_supervisor_cancellation_delivery" => {
            operation.cancellation_delivery_error.as_deref() == Some(error)
        }
        "root_supervisor_worker_wait" => operation.worker_wait_error.as_deref() == Some(error),
        _ => false,
    };
    if already_recorded {
        return;
    }
    let start = SpanStart::new(recorder_operation, "process_tree")
        .with_lifecycle_phase(lifecycle_phase)
        .with_diagnostic_id(operation.diagnostic_id.clone())
        .with_hashed_correlation("attempt_id", &operation.attempt.attempt_id);
    process_recorder().with_requested_span(start, |span| {
        let _ = span.record(
            DiagnosticPhase::Failed,
            PhaseObservation::started_unknown()
                .with_cause(cause)
                .with_cause(error),
        );
    });
}
