//! The control reader does no durable work. The guardian remains the sole
//! context/custody/retirement writer and joins this thread before every fork.
use super::original_work::{
    CancelSubmission, FD_COUNT, InboundCancel, InboundWork, RootJoinRequest, WorkSubmission,
};
use super::{identity, peer_pid};
use oulipoly_kernel_broker::protocol::{self, SourceControlUse};
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
use std::time::Instant;

pub(super) const PENDING_LIMIT: usize = 128;
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const PEER_IDENTITY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_REQUEST_BYTES: usize = 64 * 1024;
type ControlFrame = (Vec<u8>, Vec<OwnedFd>);
type PinnedFrame = (Vec<u8>, Vec<OwnedFd>, Option<[u8; 16]>);
type FrameError = (Option<u8>, String);

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
    pinned_root_id: Option<String>,
}

impl ControlService {
    pub fn start(listener: &UnixListener, owner: &CompletionDomainOwner) -> Result<Self, String> {
        Self::start_with_root(listener, owner, None)
    }

    pub fn start_pinned(
        listener: &UnixListener,
        owner: &CompletionDomainOwner,
        root_id: &str,
    ) -> Result<Self, String> {
        Self::start_with_root(listener, owner, Some(root_id.to_owned()))
    }

    fn start_with_root(
        listener: &UnixListener,
        owner: &CompletionDomainOwner,
        pinned_root_id: Option<String>,
    ) -> Result<Self, String> {
        let listener = listener.try_clone().map_err(|e| e.to_string())?;
        let owner = owner.clone();
        let (commands, receive) = mpsc::channel();
        // A full queue fails without an admission ACK; it never discards a
        // committed lease. Do not turn slow storage into unbounded sockets.
        let (send, requests) = mpsc::sync_channel(PENDING_LIMIT);
        let root = pinned_root_id.clone();
        let thread = std::thread::Builder::new()
            .name("completion-control".into())
            .spawn(move || serve(listener, owner, root, receive, send))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            requests,
            thread: Some(thread),
            pinned_root_id,
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
        *self = Self::start_with_root(listener, owner, self.pinned_root_id.clone())?;
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
    pinned_root_id: Option<String>,
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
                let pinned_root_id = pinned_root_id.clone();
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
                            pinned_root_id.as_deref(),
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
    pinned_root_id: Option<&str>,
) {
    let peer = if pinned_root_id.is_some() {
        let Ok(credentials) = peer_credentials(&socket) else {
            return;
        };
        if credentials.uid != unsafe { libc::geteuid() } && credentials.uid != 0 {
            return;
        }
        i64::from(credentials.pid)
    } else {
        let Ok(peer) = peer_pid(&socket) else { return };
        peer
    };
    let Ok(context) = identity(peer) else {
        super::record_control_gap(owner, peer, "original_work_control_peer_disappeared");
        return;
    };
    if socket.set_nonblocking(true).is_err() {
        return;
    }
    let incoming = match pinned_root_id {
        Some(_) => receive_pinned_request(&socket, &context),
        None => receive_request(&socket, &context).map(|(frame, fds)| (frame, fds, None)),
    };
    let (request, descriptors, ticket) = match incoming {
        Ok(request) => request,
        Err((Some(b'w' | b'c'), _)) => {
            super::record_control_gap(owner, peer, "original_work_control_envelope_invalid");
            return;
        }
        Err(_) => {
            return;
        }
    };
    if !answering.load(Ordering::Acquire) {
        return;
    }
    if request == b"hello\n" && descriptors.is_empty() {
        if socket.set_nonblocking(false).is_err() {
            return;
        }
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
    if let Some(root_id) = pinned_root_id
        && (command == b"work!" || command == b"cancel")
    {
        let Some(ticket) = ticket else {
            super::record_control_gap(owner, peer, "original_work_control_source_ticket_missing");
            return;
        };
        if attest_source_frame(owner, root_id, &socket, ticket, command, json).is_err()
            || receive_pinned_eof(&socket, &context).is_err()
        {
            super::record_control_gap(owner, peer, "original_work_control_source_ticket_refused");
            return;
        }
    } else if ticket.is_some() {
        return;
    }
    if socket.set_nonblocking(false).is_err() {
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
) -> Result<ControlFrame, FrameError> {
    let (first, descriptors) = receive_first(socket, peer).map_err(|error| (None, error))?;
    receive_request_from_first(socket, peer, first, descriptors)
}

fn receive_request_from_first(
    socket: &UnixStream,
    peer: &SourceProcessIdentity,
    first: u8,
    descriptors: Vec<OwnedFd>,
) -> Result<ControlFrame, FrameError> {
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

fn receive_pinned_request(
    socket: &UnixStream,
    peer: &SourceProcessIdentity,
) -> Result<PinnedFrame, FrameError> {
    let enabled: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            (&enabled as *const libc::c_int).cast(),
            std::mem::size_of_val(&enabled) as _,
        )
    } != 0
    {
        return Err((None, std::io::Error::last_os_error().to_string()));
    }
    let (first, fds, marker_sender) =
        receive_credentialled_byte(socket, peer, Some(Instant::now() + Duration::from_secs(10)))
            .map_err(|error| (None, error))?;
    if first != b'@' {
        // The old join/hello exchange is still used to establish the owner.
        // A pinned work/cancel request without S-v2 is refused below.
        return receive_request_from_first(socket, peer, first, fds)
            .map(|(frame, descriptors)| (frame, descriptors, None));
    }
    if !fds.is_empty() {
        return Err((
            Some(first),
            "source ticket marker carried descriptors".into(),
        ));
    }
    let mut ticket = [0u8; 16];
    let marker_deadline = Instant::now() + Duration::from_secs(10);
    for byte in &mut ticket {
        let (next, fds, sender) = receive_credentialled_byte(socket, peer, Some(marker_deadline))
            .map_err(|error| (Some(first), error))?;
        if !fds.is_empty() || credential_tuple(sender) != credential_tuple(marker_sender) {
            return Err((Some(first), "source ticket marker changed sender".into()));
        }
        *byte = next;
    }
    let connector = peer_credentials(socket).map_err(|error| (Some(first), error))?;
    let mut frame = Vec::new();
    let mut descriptors = Vec::new();
    let mut lines = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if frame.len() >= MAX_REQUEST_BYTES || Instant::now() >= deadline {
            return Err((
                frame.first().copied(),
                "bounded source frame incomplete".into(),
            ));
        }
        let (byte, fds, sender) = receive_credentialled_byte(socket, peer, Some(deadline))
            .map_err(|error| (frame.first().copied(), error))?;
        if credential_tuple(sender) != credential_tuple(connector) {
            return Err((
                frame.first().copied(),
                "source frame sender differs from connector".into(),
            ));
        }
        if frame.is_empty() {
            descriptors = fds;
        } else if !fds.is_empty() {
            return Err((
                frame.first().copied(),
                "late source frame descriptors".into(),
            ));
        }
        frame.push(byte);
        lines += usize::from(byte == b'\n');
        if lines == 2 {
            break;
        }
    }
    if !frame.starts_with(b"work!\n") && !frame.starts_with(b"cancel\n") {
        return Err((
            frame.first().copied(),
            "source ticket used for unsupported command".into(),
        ));
    }
    Ok((frame, descriptors, Some(ticket)))
}

fn peer_credentials(socket: &UnixStream) -> Result<libc::ucred, String> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of_val(&credentials) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    } != 0
        || len as usize != std::mem::size_of_val(&credentials)
        || credentials.pid <= 0
    {
        return Err("invalid source connector credentials".into());
    }
    Ok(credentials)
}

fn credential_tuple(credentials: libc::ucred) -> (i32, u32, u32) {
    (credentials.pid, credentials.uid, credentials.gid)
}

fn receive_credentialled_byte(
    socket: &UnixStream,
    peer: &SourceProcessIdentity,
    deadline: Option<Instant>,
) -> Result<(u8, Vec<OwnedFd>, libc::ucred), String> {
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut control = [0usize; 32];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    let count = loop {
        if deadline.is_some_and(|limit| Instant::now() >= limit) {
            return Err("source control byte deadline elapsed".into());
        }
        message.msg_controllen = std::mem::size_of_val(&control);
        message.msg_flags = 0;
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
    let mut sender = None;
    let mut descriptors = Vec::new();
    let mut header = unsafe { libc::CMSG_FIRSTHDR(&message) };
    while !header.is_null() {
        let item = unsafe { &*header };
        if item.cmsg_level == libc::SOL_SOCKET && item.cmsg_type == libc::SCM_CREDENTIALS {
            if sender.is_some()
                || item.cmsg_len as usize
                    != unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as _) } as usize
            {
                return Err("duplicate or malformed source credentials".into());
            }
            sender = Some(unsafe { *libc::CMSG_DATA(header).cast::<libc::ucred>() });
        } else if item.cmsg_level == libc::SOL_SOCKET && item.cmsg_type == libc::SCM_RIGHTS {
            let base = unsafe { libc::CMSG_LEN(0) } as usize;
            let len = (item.cmsg_len as usize).saturating_sub(base);
            if (item.cmsg_len as usize) < base || !len.is_multiple_of(std::mem::size_of::<RawFd>())
            {
                return Err("malformed source descriptors".into());
            }
            for index in 0..len / std::mem::size_of::<RawFd>() {
                descriptors.push(unsafe {
                    OwnedFd::from_raw_fd(*libc::CMSG_DATA(header).cast::<RawFd>().add(index))
                });
            }
        } else {
            return Err("unsupported source ancillary data".into());
        }
        header = unsafe { libc::CMSG_NXTHDR(&message, header) };
    }
    if count != 1 || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0 {
        return Err("incomplete source control byte or ancillary data".into());
    }
    let sender = sender.ok_or("missing source byte credentials")?;
    if sender.pid <= 0 {
        return Err("invalid source sender PID".into());
    }
    Ok((byte[0], descriptors, sender))
}

fn attest_source_frame(
    owner: &CompletionDomainOwner,
    root_id: &str,
    socket: &UnixStream,
    ticket: [u8; 16],
    command: &[u8],
    json: &[u8],
) -> Result<(), String> {
    let request = if command == b"work!" {
        let work: WorkSubmission = serde_json::from_slice(json).map_err(|e| e.to_string())?;
        if work.root_authority.root_id != root_id
            || work.root_authority.domain_id != owner.domain_id
            || work.root_authority.supervisor_authority_id != owner.supervisor_authority_id
        {
            return Err("source work root binding changed".into());
        }
        match work.registration {
            super::original_work::WorkRegistration::Root => SourceControlUse::WorkRoot {
                root_id: root_id.to_owned(),
                work_id: work.work_id,
            },
            super::original_work::WorkRegistration::Nested { parent_work_id, .. } => {
                SourceControlUse::WorkNested {
                    root_id: root_id.to_owned(),
                    work_id: work.work_id,
                    parent_work_id,
                }
            }
        }
    } else if command == b"cancel" {
        let cancel: CancelSubmission = serde_json::from_slice(json).map_err(|e| e.to_string())?;
        if cancel.root_id != root_id
            || cancel.supervisor_authority_id != owner.supervisor_authority_id
        {
            return Err("source cancel root binding changed".into());
        }
        SourceControlUse::Cancel {
            root_id: root_id.to_owned(),
            work_id: cancel.work_id,
        }
    } else {
        return Err("source ticket cannot authorize command".into());
    };
    protocol::consume_source_ticket_at(
        &super::linux::owner_broker_socket(),
        ticket,
        request,
        socket.as_raw_fd(),
    )
    .map_err(|error| error.to_string())
}

fn receive_pinned_eof(socket: &UnixStream, peer: &SourceProcessIdentity) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut byte = [0u8; 1];
    loop {
        if Instant::now() >= deadline {
            return Err("source frame write half-close absent".into());
        }
        match unsafe { libc::recv(socket.as_raw_fd(), byte.as_mut_ptr().cast(), 1, 0) } {
            0 => return Ok(()),
            count if count > 0 => return Err("duplicated or trailing source frame bytes".into()),
            _ => {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if error.kind() == std::io::ErrorKind::WouldBlock {
                    wait_for_peer_progress(socket, peer)?;
                    continue;
                }
                return Err(error.to_string());
            }
        }
    }
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

    fn pinned_pair() -> (UnixStream, UnixStream, SourceProcessIdentity) {
        let (server, client) = UnixStream::pair().unwrap();
        let enabled: libc::c_int = 1;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    server.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_PASSCRED,
                    (&enabled as *const libc::c_int).cast(),
                    std::mem::size_of_val(&enabled) as _,
                )
            },
            0
        );
        let peer = identity(i64::from(std::process::id())).unwrap();
        (server, client, peer)
    }

    fn send_frame(socket: &UnixStream, bytes: &[u8], fds: &[RawFd]) {
        let mut iov = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let mut control = [0u8; 128];
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        if !fds.is_empty() {
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen =
                unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds) as _) } as usize;
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&msg);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds) as _) as usize;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr(),
                    libc::CMSG_DATA(header).cast::<RawFd>(),
                    fds.len(),
                );
            }
        }
        assert_eq!(
            unsafe { libc::sendmsg(socket.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) },
            bytes.len() as isize
        );
    }

    #[test]
    fn pinned_complete_work_frame_has_one_sender_and_exact_four_fds() {
        let (server, mut client, peer) = pinned_pair();
        client.write_all(&[b'@']).unwrap();
        client.write_all(&[7u8; 16]).unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        send_frame(&client, b"work!\n{}\n", &[file.as_raw_fd(); 4]);
        client.shutdown(std::net::Shutdown::Write).unwrap();
        server.set_nonblocking(true).unwrap();
        let (frame, fds, ticket) = receive_pinned_request(&server, &peer).unwrap();
        assert_eq!(frame, b"work!\n{}\n");
        assert_eq!(fds.len(), 4);
        assert_eq!(ticket, Some([7u8; 16]));
        receive_pinned_eof(&server, &peer).unwrap();
    }

    #[test]
    fn pinned_complete_cancel_frame_has_no_fds() {
        let (server, mut client, peer) = pinned_pair();
        client.write_all(&[b'@']).unwrap();
        client.write_all(&[4u8; 16]).unwrap();
        send_frame(&client, b"cancel\n{}\n", &[]);
        client.shutdown(std::net::Shutdown::Write).unwrap();
        server.set_nonblocking(true).unwrap();
        let (frame, fds, ticket) = receive_pinned_request(&server, &peer).unwrap();
        assert_eq!(frame, b"cancel\n{}\n");
        assert!(fds.is_empty());
        assert_eq!(ticket, Some([4u8; 16]));
        receive_pinned_eof(&server, &peer).unwrap();
    }

    #[test]
    fn pinned_inherited_socket_child_cannot_send_cancel_as_connector() {
        let (server, mut client, peer) = pinned_pair();
        client.write_all(&[b'@']).unwrap();
        client.write_all(&[8u8; 16]).unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            drop(server);
            let _ = client.write_all(b"cancel\n{}\n");
            let _ = client.shutdown(std::net::Shutdown::Write);
            unsafe { libc::_exit(0) };
        }
        drop(client);
        server.set_nonblocking(true).unwrap();
        let refusal = receive_pinned_request(&server, &peer);
        assert!(refusal.is_err());
        assert!(
            refusal
                .err()
                .unwrap()
                .1
                .contains("sender differs from connector")
        );
        unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
    }

    #[test]
    fn pinned_first_frame_byte_does_not_cover_later_child_bytes() {
        let (server, mut client, peer) = pinned_pair();
        client.write_all(&[b'@']).unwrap();
        client.write_all(&[5u8; 16]).unwrap();
        client.write_all(b"cancel\n").unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            drop(server);
            let _ = client.write_all(b"{}\n");
            let _ = client.shutdown(std::net::Shutdown::Write);
            unsafe { libc::_exit(0) };
        }
        drop(client);
        server.set_nonblocking(true).unwrap();
        let refusal = receive_pinned_request(&server, &peer);
        assert!(refusal.is_err());
        assert!(
            refusal
                .err()
                .unwrap()
                .1
                .contains("sender differs from connector")
        );
        unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
    }

    #[test]
    fn pinned_unattested_work_never_reaches_owner_queue() {
        let (server, mut client, _) = pinned_pair();
        let endpoint = Path::new("/tmp/unused-pinned-control-test.sock");
        let owner = owner(endpoint);
        let reply = serde_json::to_vec(&owner).unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        client.write_all(b"work!\n{}\n").unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        serve_request(
            server,
            &owner,
            &reply,
            &AtomicBool::new(true),
            &AtomicBool::new(true),
            &send,
            Some("11111111-1111-4111-8111-111111111111"),
        );
        assert!(receive.try_recv().is_err());
    }

    #[test]
    fn pinned_transferred_work_fd_never_reaches_owner_queue() {
        let (server, mut client, _) = pinned_pair();
        let endpoint = Path::new("/tmp/unused-transferred-control-test.sock");
        let owner = owner(endpoint);
        let reply = serde_json::to_vec(&owner).unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        client.write_all(&[b'@']).unwrap();
        client.write_all(&[3u8; 16]).unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            drop(server);
            let file = std::fs::File::open("/dev/null").unwrap();
            send_frame(&client, b"work!\n{}\n", &[file.as_raw_fd(); 4]);
            let _ = client.shutdown(std::net::Shutdown::Write);
            unsafe { libc::_exit(0) };
        }
        drop(client);
        serve_request(
            server,
            &owner,
            &reply,
            &AtomicBool::new(true),
            &AtomicBool::new(true),
            &send,
            Some("11111111-1111-4111-8111-111111111111"),
        );
        assert!(receive.try_recv().is_err());
        unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
    }

    #[test]
    fn pinned_partial_duplicate_and_late_fds_refuse() {
        for (frame, late_fd) in [
            (b"cancel\n{".as_slice(), false),
            (b"cancel\n{}\ncancel\n{}\n".as_slice(), false),
            (b"cancel\n{}\n".as_slice(), true),
        ] {
            let (server, mut client, peer) = pinned_pair();
            client.write_all(&[b'@']).unwrap();
            client.write_all(&[9u8; 16]).unwrap();
            if late_fd {
                client.write_all(b"c").unwrap();
                let file = std::fs::File::open("/dev/null").unwrap();
                send_frame(&client, b"ancel\n{}\n", &[file.as_raw_fd()]);
            } else {
                client.write_all(frame).unwrap();
            }
            client.shutdown(std::net::Shutdown::Write).unwrap();
            server.set_nonblocking(true).unwrap();
            match receive_pinned_request(&server, &peer) {
                Ok(_) => assert!(receive_pinned_eof(&server, &peer).is_err()),
                Err(_) => {}
            }
        }
    }

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
            pinned_root_id: None,
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
