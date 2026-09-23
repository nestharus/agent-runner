//! The control reader does no durable work. The guardian remains the sole
//! context/custody/retirement writer and joins this thread before every fork.
use super::original_work::{
    CancelSubmission, FD_COUNT, InboundCancel, InboundWork, RootJoinRequest, WorkSubmission,
};
use super::{identity, peer_pid};
use oulipoly_state::completion_continuation::SourceProcessIdentity;
use oulipoly_state::mailbox::CompletionDomainOwner;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

pub(super) const PENDING_LIMIT: usize = 128;
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PEER_IDENTITY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RefusalReason {
    QueueFull,
    Retiring,
    Persistence,
    Identity,
}

// A negative is deliberately not an owner response. Old clients reject it.
// It carries no endpoint, session contents, capability or raw storage error.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct JoinRefusal {
    pub completion_join_refusal: RefusalReason,
    pub protocol: String,
    pub domain_id: String,
    pub owner_generation: String,
    pub guardian_identity: SourceProcessIdentity,
}

impl JoinRefusal {
    pub fn new(owner: &CompletionDomainOwner, reason: RefusalReason) -> Self {
        Self {
            completion_join_refusal: reason,
            protocol: owner.protocol.clone(),
            domain_id: owner.domain_id.clone(),
            owner_generation: owner.owner_generation.clone(),
            guardian_identity: owner.guardian_identity.clone(),
        }
    }

    pub fn send(&self, socket: &mut UnixStream) {
        let _ = serde_json::to_writer(&mut *socket, self);
        let _ = socket.write_all(b"\n");
    }
}

pub(super) struct JoinRequest {
    pub socket: UnixStream,
    pub context: SourceProcessIdentity,
    pub request: RootJoinRequest,
}

pub(super) enum ControlRequest {
    Join(JoinRequest),
    Work(InboundWork),
    Cancel(InboundCancel),
}

enum Command {
    Pause(Sender<()>),
    Resume,
    Close(Sender<()>),
    Stop,
}

pub(super) struct ControlService {
    commands: Sender<Command>,
    requests: Receiver<ControlRequest>,
    thread: Option<JoinHandle<()>>,
}

impl ControlService {
    pub fn start(listener: &UnixListener, owner: &CompletionDomainOwner) -> Result<Self, String> {
        let listener = listener.try_clone().map_err(|e| e.to_string())?;
        let owner = owner.clone();
        let (commands, receive) = mpsc::channel();
        // A full queue fails without an admission ACK; it never discards a
        // committed lease. Do not turn slow storage into unbounded sockets.
        let (send, requests) = mpsc::sync_channel(PENDING_LIMIT);
        let thread = std::thread::Builder::new()
            .name("completion-control".into())
            .spawn(move || serve(listener, owner, receive, send))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            requests,
            thread: Some(thread),
        })
    }

    pub fn pending(&self) -> Vec<ControlRequest> {
        // Receiving frees slots that a concurrent producer can refill. Bound
        // this extraction itself, not merely the channel occupancy.
        collect_pending(self.requests.try_iter())
    }

    pub fn recover_finished(
        &mut self,
        listener: &UnixListener,
        owner: &CompletionDomainOwner,
    ) -> Result<Vec<ControlRequest>, String> {
        if !self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
        {
            return Ok(Vec::new());
        }
        // Observe reader-only loss on every guardian pass, even with live
        // leases. Join before replacement; panic propagates guardian failure
        // and its existing succession signal, never a healthy endpoint claim.
        self.join()?;
        let pending = self.pending();
        *self = Self::start(listener, owner)?;
        Ok(pending)
    }

    pub fn pause(&self) -> Result<(), String> {
        let (send, receive) = mpsc::channel();
        self.commands
            .send(Command::Pause(send))
            .map_err(|e| e.to_string())?;
        receive.recv().map_err(|e| e.to_string())
    }

    pub fn resume(&self) -> Result<(), String> {
        self.commands
            .send(Command::Resume)
            .map_err(|e| e.to_string())
    }

    pub fn close(&self) -> Result<(), String> {
        let (send, receive) = mpsc::channel();
        self.commands
            .send(Command::Close(send))
            .map_err(|e| e.to_string())?;
        receive.recv().map_err(|e| e.to_string())
    }

    pub fn stop(mut self) -> Result<Vec<ControlRequest>, String> {
        self.join()?;
        Ok(self.pending())
    }

    fn join(&mut self) -> Result<(), String> {
        let _ = self.commands.send(Command::Stop);
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| "completion control thread panicked".into()),
            None => Ok(()),
        }
    }
}

// Bound work at the extraction boundary, independently of iterator/channel
// occupancy. The reader may refill every slot as soon as it is received.
fn collect_pending(requests: impl Iterator<Item = ControlRequest>) -> Vec<ControlRequest> {
    requests.take(PENDING_LIMIT).collect()
}

impl Drop for ControlService {
    fn drop(&mut self) {
        let _ = self.join();
    }
}

fn serve(
    listener: UnixListener,
    owner: CompletionDomainOwner,
    commands: Receiver<Command>,
    requests: SyncSender<ControlRequest>,
) {
    let Ok(reply) = serde_json::to_vec(&owner) else {
        return;
    };
    let mut accepting = true;
    let mut answering = true;
    let accepting_state = Arc::new(AtomicBool::new(true));
    let answering_state = Arc::new(AtomicBool::new(true));
    let readers = Arc::new(AtomicUsize::new(0));
    let mut reader_threads = Vec::new();
    loop {
        reap_reader_threads(&mut reader_threads);
        if !apply_commands(
            &commands,
            &mut accepting,
            &mut answering,
            &accepting_state,
            &answering_state,
        ) {
            stop_reader_threads(reader_threads);
            return;
        }
        match listener.accept() {
            Ok((socket, _)) => {
                if readers.fetch_add(1, Ordering::AcqRel) >= PENDING_LIMIT {
                    readers.fetch_sub(1, Ordering::AcqRel);
                    continue;
                }
                let owner = owner.clone();
                let reply = reply.clone();
                let requests = requests.clone();
                let thread_readers = Arc::clone(&readers);
                let thread_accepting = Arc::clone(&accepting_state);
                let thread_answering = Arc::clone(&answering_state);
                let Ok(shutdown) = socket.try_clone() else {
                    readers.fetch_sub(1, Ordering::AcqRel);
                    continue;
                };
                match std::thread::Builder::new()
                    .name("completion-control-request".into())
                    .spawn(move || {
                        serve_request(
                            socket,
                            &owner,
                            &reply,
                            &thread_accepting,
                            &thread_answering,
                            &requests,
                        );
                        thread_readers.fetch_sub(1, Ordering::AcqRel);
                    }) {
                    Ok(thread) => reader_threads.push((shutdown, thread)),
                    Err(_) => {
                        readers.fetch_sub(1, Ordering::AcqRel);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL_INTERVAL)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => {
                stop_reader_threads(reader_threads);
                return;
            }
        }
    }
}

fn reap_reader_threads(readers: &mut Vec<(UnixStream, JoinHandle<()>)>) {
    let mut index = 0;
    while index < readers.len() {
        if readers[index].1.is_finished() {
            let (_, thread) = readers.swap_remove(index);
            let _ = thread.join();
        } else {
            index += 1;
        }
    }
}

fn stop_reader_threads(mut readers: Vec<(UnixStream, JoinHandle<()>)>) {
    for (socket, _) in &readers {
        let _ = socket.shutdown(std::net::Shutdown::Both);
    }
    for (_, thread) in readers.drain(..) {
        let _ = thread.join();
    }
}

fn apply_commands(
    commands: &Receiver<Command>,
    accepting: &mut bool,
    answering: &mut bool,
    accepting_state: &AtomicBool,
    answering_state: &AtomicBool,
) -> bool {
    loop {
        match commands.try_recv() {
            Ok(Command::Pause(done)) => {
                *accepting = false;
                accepting_state.store(false, Ordering::Release);
                let _ = done.send(());
            }
            Ok(Command::Resume) => {
                *accepting = true;
                accepting_state.store(true, Ordering::Release);
            }
            Ok(Command::Close(done)) => {
                *accepting = false;
                *answering = false;
                accepting_state.store(false, Ordering::Release);
                answering_state.store(false, Ordering::Release);
                let _ = done.send(());
            }
            Ok(Command::Stop) | Err(mpsc::TryRecvError::Disconnected) => return false,
            Err(mpsc::TryRecvError::Empty) => return true,
        }
    }
}

fn serve_request(
    mut socket: UnixStream,
    owner: &CompletionDomainOwner,
    reply: &[u8],
    accepting: &AtomicBool,
    answering: &AtomicBool,
    requests: &SyncSender<ControlRequest>,
) {
    let Ok(peer) = peer_pid(&socket) else { return };
    let Ok(context) = identity(peer) else {
        super::record_control_gap(owner, peer, "original_work_control_peer_disappeared");
        return;
    };
    if socket.set_nonblocking(true).is_err() {
        return;
    }
    let (request, descriptors) = match receive_request(&socket, &context) {
        Ok(request) => request,
        Err((Some(b'w' | b'c'), _)) => {
            super::record_control_gap(owner, peer, "original_work_control_envelope_invalid");
            return;
        }
        Err(_) => {
            return;
        }
    };
    if socket.set_nonblocking(false).is_err() {
        return;
    }
    if !answering.load(Ordering::Acquire) {
        return;
    }
    if request == b"hello\n" && descriptors.is_empty() {
        let _ = socket.write_all(reply);
        return; // EOF remains part of the existing hello protocol.
    }
    let Some(separator) = request.iter().position(|byte| *byte == b'\n') else {
        if request.starts_with(b"work") || request.starts_with(b"cancel") {
            super::record_control_gap(owner, peer, "original_work_control_frame_invalid");
        }
        return;
    };
    let (command, framed_json) = request.split_at(separator);
    let json = &framed_json[1..];
    if identity(peer).as_ref() != Ok(&context) {
        super::record_control_gap(owner, peer, "original_work_control_peer_disappeared");
        return;
    }
    let queued = match command {
        b"join!" if descriptors.is_empty() => {
            if !accepting.load(Ordering::Acquire) {
                JoinRefusal::new(owner, RefusalReason::Retiring).send(&mut socket);
                return;
            }
            serde_json::from_slice::<RootJoinRequest>(json).map(|request| {
                ControlRequest::Join(JoinRequest {
                    socket,
                    context,
                    request,
                })
            })
        }
        b"work!" if descriptors.len() == FD_COUNT => {
            let Ok(descriptors) = descriptors.try_into() else {
                return;
            };
            serde_json::from_slice::<WorkSubmission>(json).map(|submission| {
                ControlRequest::Work(InboundWork {
                    socket,
                    peer: context,
                    submission,
                    descriptors,
                })
            })
        }
        b"cancel" if descriptors.is_empty() => serde_json::from_slice::<CancelSubmission>(json)
            .map(|submission| {
                ControlRequest::Cancel(InboundCancel {
                    socket,
                    peer: context,
                    submission,
                })
            }),
        _ => {
            if command.starts_with(b"work") || command.starts_with(b"cancel") {
                super::record_control_gap(owner, peer, "original_work_control_command_invalid");
            }
            return;
        }
    };
    let Ok(queued) = queued else {
        if command == b"work!" || command == b"cancel" {
            super::record_control_gap(owner, peer, "original_work_control_payload_invalid");
        }
        return;
    };
    if !accepting.load(Ordering::Acquire) {
        refuse_retiring(owner, queued);
        return;
    }
    if let Err(error) = requests.try_send(queued) {
        match error {
            mpsc::TrySendError::Full(ControlRequest::Join(mut request))
            | mpsc::TrySendError::Disconnected(ControlRequest::Join(mut request)) => {
                JoinRefusal::new(owner, RefusalReason::QueueFull).send(&mut request.socket);
            }
            mpsc::TrySendError::Full(ControlRequest::Work(request))
            | mpsc::TrySendError::Disconnected(ControlRequest::Work(request)) => {
                super::reject_work(
                    owner,
                    request,
                    "root control queue is full; request was not accepted".into(),
                );
            }
            mpsc::TrySendError::Full(ControlRequest::Cancel(request))
            | mpsc::TrySendError::Disconnected(ControlRequest::Cancel(request)) => {
                super::reject_cancel(
                    owner,
                    request,
                    "root control queue is full before cancellation".into(),
                );
            }
        }
    }
}

fn refuse_retiring(owner: &CompletionDomainOwner, request: ControlRequest) {
    match request {
        ControlRequest::Join(mut request) => {
            JoinRefusal::new(owner, RefusalReason::Retiring).send(&mut request.socket)
        }
        ControlRequest::Work(request) => {
            super::reject_work(
                owner,
                request,
                "root authority is retiring before acceptance".into(),
            );
        }
        ControlRequest::Cancel(request) => {
            super::reject_cancel(
                owner,
                request,
                "root authority is retiring before cancellation".into(),
            );
        }
    }
}

fn receive_request(
    socket: &UnixStream,
    peer: &SourceProcessIdentity,
) -> Result<(Vec<u8>, Vec<OwnedFd>), (Option<u8>, String)> {
    let (first, descriptors) = receive_first(socket, peer).map_err(|error| (None, error))?;
    let mut request = vec![first];
    let required_lines = if first == b'h' { 1 } else { 2 };
    let mut lines = usize::from(first == b'\n');
    let mut chunk = [0_u8; 4096];
    let mut reader = socket;
    while lines < required_lines {
        if request.len() >= MAX_REQUEST_BYTES {
            return Err((
                Some(first),
                "root control request exceeds its bounded frame".into(),
            ));
        }
        let remaining = (MAX_REQUEST_BYTES - request.len()).min(chunk.len());
        match reader.read(&mut chunk[..remaining]) {
            Ok(0) => {
                return Err((
                    Some(first),
                    "root control peer closed an incomplete frame".into(),
                ));
            }
            Ok(count) => {
                let mut frame_end = count;
                for (index, byte) in chunk[..count].iter().enumerate() {
                    if *byte == b'\n' {
                        lines += 1;
                        if lines == required_lines {
                            frame_end = index + 1;
                            break;
                        }
                    }
                }
                request.extend_from_slice(&chunk[..frame_end]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                wait_for_peer_progress(socket, peer).map_err(|error| (Some(first), error))?;
            }
            Err(error) => return Err((Some(first), error.to_string())),
        }
    }
    while request.last() == Some(&b'\n') {
        request.pop();
        if required_lines == 1 {
            request.push(b'\n');
            break;
        }
    }
    Ok((request, descriptors))
}

fn receive_first(
    socket: &UnixStream,
    peer: &SourceProcessIdentity,
) -> Result<(u8, Vec<OwnedFd>), String> {
    let mut byte = [0u8];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0usize; 16];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&control);
    let count = loop {
        let count =
            unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
        if count >= 0 {
            break count;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if error.kind() == std::io::ErrorKind::WouldBlock {
            wait_for_peer_progress(socket, peer)?;
            continue;
        }
        return Err(error.to_string());
    };
    if count != 1 || message.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err("invalid root control envelope".into());
    }
    let mut descriptors = Vec::new();
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let count = ((*header).cmsg_len - libc::CMSG_LEN(0) as usize)
                    / std::mem::size_of::<RawFd>();
                for index in 0..count {
                    descriptors.push(OwnedFd::from_raw_fd(
                        *libc::CMSG_DATA(header).cast::<RawFd>().add(index),
                    ));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    Ok((byte[0], descriptors))
}

fn wait_for_peer_progress(socket: &UnixStream, peer: &SourceProcessIdentity) -> Result<(), String> {
    let mut descriptor = libc::pollfd {
        fd: socket.as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
        revents: 0,
    };
    loop {
        let result = unsafe {
            libc::poll(
                &mut descriptor,
                1,
                i32::try_from(PEER_IDENTITY_POLL_INTERVAL.as_millis()).unwrap_or(50),
            )
        };
        if identity(peer.pid).as_ref() != Ok(peer) {
            return Err("root control frame owner process terminated or changed identity".into());
        }
        if result > 0 || result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Instant;

    fn owner(endpoint: &Path) -> CompletionDomainOwner {
        let id = identity(i64::from(std::process::id())).unwrap();
        CompletionDomainOwner {
            protocol: oulipoly_state::completion_continuation::PROTOCOL.into(),
            domain_id: "reader-test".into(),
            supervisor_authority_id: "11111111-1111-4111-8111-111111111111".into(),
            owner_generation: "reader-generation".into(),
            guardian_identity: id.clone(),
            driver_identity: id,
            endpoint: endpoint.to_string_lossy().into_owned(),
        }
    }

    fn join_request(socket: UnixStream, context: SourceProcessIdentity) -> ControlRequest {
        ControlRequest::Join(JoinRequest {
            socket,
            context,
            request: RootJoinRequest {
                protocol: super::super::original_work::ROOT_PROTOCOL.into(),
                mode: super::super::original_work::RootJoinMode::Fresh,
            },
        })
    }

    #[test]
    fn large_legal_control_frame_is_received_and_parsed_with_bounded_chunks() {
        let (socket, mut writer) = UnixStream::pair().unwrap();
        socket.set_nonblocking(true).unwrap();
        let peer = identity(i64::from(std::process::id())).unwrap();
        let mut frame =
            b"join!\n{\"protocol\":\"root-authority-v1\",\"mode\":{\"kind\":\"fresh\"}}".to_vec();
        frame.resize(60 * 1024 - 1, b' ');
        frame.push(b'\n');
        let writer_thread = std::thread::spawn(move || writer.write_all(&frame).unwrap());

        let (received, descriptors) = receive_request(&socket, &peer).unwrap();
        writer_thread.join().unwrap();
        assert!(descriptors.is_empty());
        assert_eq!(received.len(), 60 * 1024 - 1);
        let (command, json) = received.split_at(6);
        assert_eq!(command, b"join!\n");
        let request: RootJoinRequest = serde_json::from_slice(json).unwrap();
        assert!(matches!(
            request.mode,
            super::super::original_work::RootJoinMode::Fresh
        ));
    }

    #[test]
    fn terminated_reader_recovers_with_pending_responsibility() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("owner.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        listener.set_nonblocking(true).unwrap();
        let owner = owner(&endpoint);
        let mut service = ControlService::start(&listener, &owner).unwrap();
        let mut client = UnixStream::connect(&endpoint).unwrap();
        client
            .write_all(
                b"join!\n{\"protocol\":\"root-authority-v1\",\"mode\":{\"kind\":\"fresh\"}}\n",
            )
            .unwrap();
        super::super::hello(&endpoint).unwrap();
        service.commands.send(Command::Stop).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !service.thread.as_ref().unwrap().is_finished() {
            assert!(Instant::now() < deadline, "reader failed to terminate");
            std::thread::yield_now();
        }
        let pending = service.recover_finished(&listener, &owner).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(
            super::super::hello(&endpoint).unwrap().owner_generation,
            owner.owner_generation
        );
        client.set_nonblocking(true).unwrap();
        assert_eq!(
            client.read(&mut [0]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        service.stop().unwrap();
    }

    #[test]
    fn collection_is_bounded_under_concurrent_refill() {
        let root = tempfile::tempdir().unwrap();
        let owner = owner(&root.path().join("unused"));
        let (commands, _receiver) = mpsc::channel();
        let (send, requests) = mpsc::sync_channel(PENDING_LIMIT);
        let (socket, _client) = UnixStream::pair().unwrap();
        for _ in 0..PENDING_LIMIT {
            send.send(join_request(
                socket.try_clone().unwrap(),
                owner.guardian_identity.clone(),
            ))
            .unwrap();
        }
        // Refill after EVERY receive before the next receive is permitted.
        // This is a real channel/producer with a deterministic interleaving,
        // not a scheduler-dependent chance that concurrent refill happened.
        let (refill, requested) = mpsc::channel();
        let (done, refilled) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            for _ in requested {
                send.send(join_request(
                    socket.try_clone().unwrap(),
                    owner.guardian_identity.clone(),
                ))
                .unwrap();
                done.send(()).unwrap();
            }
        });
        let service = ControlService {
            commands,
            requests,
            thread: None,
        };
        let mut overlaps = 0;
        let requests = service.requests.try_iter().inspect(|_| {
            // A finite adversarial stream: an unbounded collector completes
            // with 2*LIMIT rather than hanging the regression control.
            if overlaps < PENDING_LIMIT {
                refill.send(()).unwrap();
                refilled.recv().unwrap();
                overlaps += 1;
            }
        });
        let first = collect_pending(requests);
        drop(refill);
        producer.join().unwrap();
        assert_eq!(overlaps, PENDING_LIMIT, "refill overlap was not reached");
        assert_eq!(first.len(), PENDING_LIMIT);
        assert_eq!(service.pending().len(), PENDING_LIMIT);
        assert!(service.pending().is_empty());
    }

    #[test]
    fn incomplete_request_is_ended_by_authority_shutdown_not_a_read_deadline() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("owner.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        listener.set_nonblocking(true).unwrap();
        let owner = owner(&endpoint);
        let service = ControlService::start(&listener, &owner).unwrap();
        let mut client = UnixStream::connect(&endpoint).unwrap();
        client.write_all(b"work").unwrap();
        assert_eq!(
            super::super::hello(&endpoint).unwrap().owner_generation,
            owner.owner_generation
        );

        assert!(service.stop().unwrap().is_empty());
        assert_eq!(client.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn request_started_before_pause_cannot_use_a_stale_acceptance_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("owner.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        listener.set_nonblocking(true).unwrap();
        let owner = owner(&endpoint);
        let service = ControlService::start(&listener, &owner).unwrap();
        let mut client = UnixStream::connect(&endpoint).unwrap();
        client.write_all(b"join!\n").unwrap();
        service.pause().unwrap();
        client
            .write_all(b"{\"protocol\":\"root-authority-v1\",\"mode\":{\"kind\":\"fresh\"}}\n")
            .unwrap();
        let refusal: JoinRefusal = serde_json::from_reader(&mut client).unwrap();

        assert!(matches!(
            refusal.completion_join_refusal,
            RefusalReason::Retiring
        ));
        assert!(service.pending().is_empty());
        service.stop().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn partial_frame_reader_releases_when_original_peer_dies_even_if_socket_is_inherited() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("partial.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        let (mut release_parent, mut release_child) = UnixStream::pair().unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            if unsafe { libc::setsid() } < 0 {
                unsafe { libc::_exit(71) }
            }
            drop(release_parent);
            let mut socket = UnixStream::connect(&endpoint).unwrap();
            socket.write_all(b"work").unwrap();
            let mut release = [0];
            release_child.read_exact(&mut release).unwrap();
            let holder = unsafe { libc::fork() };
            if holder == 0 {
                loop {
                    unsafe { libc::pause() };
                }
            }
            unsafe { libc::_exit(if holder > 0 { 0 } else { 72 }) }
        }
        drop(release_child);
        let (socket, _) = listener.accept().unwrap();
        socket.set_nonblocking(true).unwrap();
        let peer = identity(i64::from(child)).unwrap();
        release_parent.write_all(b"x").unwrap();
        drop(release_parent);
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status));

        let error = receive_request(&socket, &peer).unwrap_err().1;

        assert!(
            error.contains("owner process terminated or changed identity"),
            "{error}"
        );
        // The inherited descriptor is still open in the session; process
        // identity, not EOF or a clock deadline, released admission ownership.
        assert_eq!(unsafe { libc::kill(-child, libc::SIGKILL) }, 0);
    }
}
