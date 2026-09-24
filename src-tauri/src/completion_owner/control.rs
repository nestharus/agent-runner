//! The control reader does no durable work. The guardian remains the sole
//! context/custody/retirement writer and joins this thread before every fork.
use super::{declared_owner, identity, peer_pid};
use oulipoly_state::completion_continuation::SourceProcessIdentity;
use oulipoly_state::mailbox::CompletionDomainOwner;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::JoinHandle;
use std::time::Duration;

pub(super) const PENDING_LIMIT: usize = 128;
const SERVER_CONTROL_IO_TIMEOUT: Duration = Duration::from_millis(50);
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RefusalReason {
    QueueFull,
    Retiring,
    Persistence,
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
}

enum Command {
    Pause(Sender<()>),
    Resume,
    Close(Sender<()>),
    Stop,
}

pub(super) struct ControlService {
    commands: Sender<Command>,
    joins: Receiver<JoinRequest>,
    thread: Option<JoinHandle<()>>,
}

impl ControlService {
    pub fn start(listener: &UnixListener, owner: &CompletionDomainOwner) -> Result<Self, String> {
        let listener = listener.try_clone().map_err(|e| e.to_string())?;
        let owner = owner.clone();
        let (commands, receive) = mpsc::channel();
        // A full queue fails without an admission ACK; it never discards a
        // committed lease. Do not turn slow storage into unbounded sockets.
        let (send, joins) = mpsc::sync_channel(PENDING_LIMIT);
        let thread = std::thread::Builder::new()
            .name("completion-control".into())
            .spawn(move || serve(listener, owner, receive, send))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            commands,
            joins,
            thread: Some(thread),
        })
    }

    pub fn pending(&self) -> Vec<JoinRequest> {
        // Receiving frees slots that a concurrent producer can refill. Bound
        // this extraction itself, not merely the channel occupancy.
        collect_pending(self.joins.try_iter())
    }

    pub fn recover_finished(
        &mut self,
        listener: &UnixListener,
        owner: &CompletionDomainOwner,
    ) -> Result<Vec<JoinRequest>, String> {
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

    pub fn stop(mut self) -> Result<Vec<JoinRequest>, String> {
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
fn collect_pending(requests: impl Iterator<Item = JoinRequest>) -> Vec<JoinRequest> {
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
    joins: SyncSender<JoinRequest>,
) {
    let Ok(reply) = serde_json::to_vec(&declared_owner(&owner)) else {
        return;
    };
    let mut accepting = true;
    let mut answering = true;
    loop {
        if !apply_commands(&commands, &mut accepting, &mut answering) {
            return;
        }
        match listener.accept() {
            Ok((socket, _)) => serve_request(socket, &owner, &reply, accepting, answering, &joins),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL_INTERVAL)
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn apply_commands(
    commands: &Receiver<Command>,
    accepting: &mut bool,
    answering: &mut bool,
) -> bool {
    loop {
        match commands.try_recv() {
            Ok(Command::Pause(done)) => {
                *accepting = false;
                let _ = done.send(());
            }
            Ok(Command::Resume) => *accepting = true,
            Ok(Command::Close(done)) => {
                *accepting = false;
                *answering = false;
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
    accepting: bool,
    answering: bool,
    joins: &SyncSender<JoinRequest>,
) {
    let Ok(peer) = peer_pid(&socket) else { return };
    if socket
        .set_read_timeout(Some(SERVER_CONTROL_IO_TIMEOUT))
        .is_err()
        || socket
            .set_write_timeout(Some(SERVER_CONTROL_IO_TIMEOUT))
            .is_err()
    {
        return;
    }
    let mut request = [0; 6];
    if socket.read_exact(&mut request).is_err() || !answering {
        return;
    }
    if request == *b"hello\n" {
        let _ = socket.write_all(reply);
        return; // EOF remains part of the existing hello protocol.
    }
    if request != *b"join!\n" {
        return;
    }
    if !accepting {
        JoinRefusal::new(owner, RefusalReason::Retiring).send(&mut socket);
        return;
    }
    if let Ok(context) = identity(peer)
        && let Err(error) = joins.try_send(JoinRequest { socket, context })
    {
        let mut request = match error {
            mpsc::TrySendError::Full(request) | mpsc::TrySendError::Disconnected(request) => {
                request
            }
        };
        JoinRefusal::new(owner, RefusalReason::QueueFull).send(&mut request.socket);
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

    #[test]
    fn terminated_reader_recovers_with_pending_responsibility() {
        let root = tempfile::tempdir().unwrap();
        let endpoint = root.path().join("owner.sock");
        let listener = UnixListener::bind(&endpoint).unwrap();
        listener.set_nonblocking(true).unwrap();
        let owner = owner(&endpoint);
        let mut service = ControlService::start(&listener, &owner).unwrap();
        let mut client = UnixStream::connect(&endpoint).unwrap();
        client.write_all(b"join!\n").unwrap();
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
        let (send, joins) = mpsc::sync_channel(PENDING_LIMIT);
        let (socket, _client) = UnixStream::pair().unwrap();
        for _ in 0..PENDING_LIMIT {
            send.send(JoinRequest {
                socket: socket.try_clone().unwrap(),
                context: owner.guardian_identity.clone(),
            })
            .unwrap();
        }
        // Refill after EVERY receive before the next receive is permitted.
        // This is a real channel/producer with a deterministic interleaving,
        // not a scheduler-dependent chance that concurrent refill happened.
        let (refill, requested) = mpsc::channel();
        let (done, refilled) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            for _ in requested {
                send.send(JoinRequest {
                    socket: socket.try_clone().unwrap(),
                    context: owner.guardian_identity.clone(),
                })
                .unwrap();
                done.send(()).unwrap();
            }
        });
        let service = ControlService {
            commands,
            joins,
            thread: None,
        };
        let mut overlaps = 0;
        let requests = service.joins.try_iter().inspect(|_| {
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
}
