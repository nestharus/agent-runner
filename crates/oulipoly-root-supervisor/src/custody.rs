//! Owner side of root custody: starting this root's PID 1, attaching to a
//! surviving one after this owner restarts, and asking it to launch, kill
//! or release work. See the crate docs for what each observation means.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use serde_json::{Value, json};

use crate::Event;
use crate::store::{IncarnationRow, Store, StoreError};
use crate::sys::{self, Isolation};
use crate::transport::StopSignal;

/// Root PID 1's binary, installed next to the owner's.
pub(crate) const PID1_BINARY: &str = "oulipoly-root-pid1";
/// How long attach waits to see an unreachable root PID 1 exit before it
/// calls that PID 1 owned-unattached instead. An observation bound only:
/// nothing is killed when it expires.
const EXIT_OBSERVATION_MS: i32 = 2000;

pub(crate) fn socket_name(incarnation: i64) -> String {
    format!("pid1-{incarnation}.sock")
}

pub(crate) fn work_name(work: i64) -> String {
    format!("w{work}")
}

/// One harness's stdio, as handed over by root PID 1.
pub(crate) struct WorkStdio {
    pub(crate) stdin: File,
    pub(crate) stdout: File,
}

impl WorkStdio {
    fn from_fds(mut fds: Vec<OwnedFd>) -> Option<Self> {
        (fds.len() == 2).then(|| {
            let stdout = File::from(fds.pop().expect("two fds"));
            let stdin = File::from(fds.pop().expect("two fds"));
            Self { stdin, stdout }
        })
    }
}

pub(crate) struct Spawned {
    pub(crate) stdio: WorkStdio,
    pub(crate) harness_host_pid: Option<i32>,
    pub(crate) exec_error: Option<String>,
}

/// A surviving harness this owner attached to, with its stdio.
pub(crate) struct Adopted {
    pub(crate) work: i64,
    pub(crate) stdio: WorkStdio,
    pub(crate) harness_host_pid: Option<i32>,
}

pub(crate) enum ReceiptWait {
    Receipt(Value),
    /// The connection to root PID 1 ended: no waiter's report arrived.
    Lost,
    /// This owner detached from the root; the work is left to a successor.
    Detached,
}

pub(crate) enum Attach {
    /// The recorded incarnation is running and admitted this owner.
    Attached {
        root: Arc<Root>,
        live: Vec<Adopted>,
        receipts: Vec<Value>,
    },
    /// The recorded incarnation is not running. `observed` says how that is
    /// known; its own exit status is not. Receipts are what it recorded.
    Absent {
        observed: &'static str,
        receipts: Vec<Value>,
    },
    /// The recorded incarnation may be running, but this owner could not
    /// attach to it. Nothing about its work is known or changed.
    Unattached { reason: String },
}

/// How root PID 1's end was observed at release.
pub(crate) struct Release {
    pub(crate) outcome: &'static str,
    /// `code:N` / `signal:N`, only from this owner's own wait as its parent.
    pub(crate) status: Option<String>,
    pub(crate) live: Vec<String>,
}

#[derive(Default)]
struct Shared {
    next_req: u64,
    replies: HashMap<u64, (Value, Vec<OwnedFd>)>,
    receipts: HashMap<String, Value>,
    lost: bool,
    detached: bool,
}

/// This owner's connection to one root PID 1 incarnation.
pub(crate) struct Root {
    socket: OwnedFd,
    shared: Arc<(Mutex<Shared>, Condvar)>,
    pub(crate) incarnation: i64,
    pub(crate) host_pid: i32,
    pidfd: OwnedFd,
    /// This owner started this PID 1 and is its actual parent.
    pub(crate) parent: bool,
    stop: Arc<StopSignal>,
}

/// The exact identity recorded for a started root PID 1.
pub(crate) struct Identity {
    pub(crate) host_pid: i32,
    pub(crate) start_time: u64,
    pub(crate) boot_id: String,
}

fn request_json(socket: &OwnedFd, value: &Value) -> io::Result<()> {
    sys::send(socket, value.to_string().as_bytes(), &[])
}

fn recv_json(socket: &OwnedFd) -> io::Result<Option<(Value, Vec<OwnedFd>)>> {
    Ok(sys::recv(socket)?
        .map(|(bytes, fds)| (serde_json::from_slice(&bytes).unwrap_or(Value::Null), fds)))
}

fn pid1_binary() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe
        .parent()
        .ok_or_else(|| io::Error::other("owner binary has no directory"))?
        .join(PID1_BINARY))
}

fn store_dir(store: &Path) -> io::Result<OwnedFd> {
    Ok(OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_PATH)
        .open(store)?
        .into())
}

/// Reads what an ended incarnation recorded about its work.
fn recorded_receipts(store: &Path, incarnation: i64) -> Vec<Value> {
    std::fs::read(store.join(format!("pid1-{incarnation}.receipts")))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|record| record["receipts"].as_array().cloned())
        .unwrap_or_default()
}

impl Root {
    /// Starts this root's PID 1 as a direct child of this owner, in a new
    /// PID namespace, and initializes it.
    pub(crate) fn start(
        store: &Path,
        incarnation: i64,
        token: &str,
        generation: i64,
        isolation: Isolation,
        stop: Arc<StopSignal>,
    ) -> io::Result<(Arc<Self>, Identity)> {
        let (socket, child_end) = sys::seqpacket_pair()?;
        let host_pid = sys::spawn_pid1(&pid1_binary()?, &child_end, isolation)?;
        drop(child_end);
        // Still our unreaped child, so this pidfd names exactly it.
        let pidfd = sys::pidfd_open(host_pid)?;
        let identity = Identity {
            host_pid,
            start_time: sys::start_time(host_pid)?,
            boot_id: sys::boot_id()?,
        };
        let ready = request_json(
            &socket,
            &json!({
                "op": "init",
                "store": store,
                "incarnation": incarnation,
                "token": token,
                "generation": generation,
            }),
        )
        .and_then(|()| recv_json(&socket));
        match ready {
            Ok(Some((value, _))) if value["event"] == "ready" => {}
            other => {
                // It never became custodian of anything: stop and reap it.
                let _ = sys::pidfd_kill(&pidfd);
                let _ = sys::wait_child(host_pid);
                return Err(io::Error::other(format!(
                    "root pid1 did not initialize: {}",
                    match other {
                        Ok(Some((value, _))) => value.to_string(),
                        Ok(None) => "closed".to_owned(),
                        Err(error) => error.to_string(),
                    }
                )));
            }
        }
        let root = Self::connected(socket, incarnation, host_pid, pidfd, true, stop)?;
        Ok((root, identity))
    }

    fn connected(
        socket: OwnedFd,
        incarnation: i64,
        host_pid: i32,
        pidfd: OwnedFd,
        parent: bool,
        stop: Arc<StopSignal>,
    ) -> io::Result<Arc<Self>> {
        let reader = socket.try_clone()?;
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let root = Arc::new(Self {
            socket,
            shared: Arc::clone(&shared),
            incarnation,
            host_pid,
            pidfd,
            parent,
            stop: Arc::clone(&stop),
        });
        thread::spawn(move || read_loop(&reader, &shared, &stop));
        Ok(root)
    }

    /// Attaches to the recorded incarnation only if it is still the exact
    /// recorded process and admits this owner's attestation.
    pub(crate) fn attach(
        store: &Path,
        recorded: &IncarnationRow,
        generation: i64,
        stop: &Arc<StopSignal>,
    ) -> Attach {
        let absent = |observed| Attach::Absent {
            observed,
            receipts: recorded_receipts(store, recorded.id),
        };
        let (Some(host_pid), Some(start_time), Some(boot_id)) = (
            recorded.host_pid,
            recorded.start_time,
            recorded.boot_id.as_deref(),
        ) else {
            // Started, or about to be, without its identity recorded. Such a
            // PID 1 never had work and exits once its starter is gone.
            return absent("identity-unrecorded");
        };
        let pidfd = match sys::pidfd_open(host_pid) {
            Ok(fd) => fd,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                return absent("absent-exit-not-observed");
            }
            Err(error) => {
                return Attach::Unattached {
                    reason: format!("pidfd: {error}"),
                };
            }
        };
        let same_boot = sys::boot_id().is_ok_and(|current| current == boot_id);
        if !same_boot || sys::start_time(host_pid).ok() != Some(start_time) {
            // That pid now names another process; never adopt or signal it.
            return absent("recorded-process-not-running");
        }
        if sys::pidfd_exited(&pidfd, 0).unwrap_or(false) {
            return absent("exit-observed-by-pidfd");
        }
        let gone_or = |reason: String| {
            if sys::pidfd_exited(&pidfd, EXIT_OBSERVATION_MS).unwrap_or(false) {
                absent("exit-observed-by-pidfd")
            } else {
                Attach::Unattached { reason }
            }
        };
        let socket = match store_dir(store)
            .and_then(|dir| sys::connect(&sys::path_in(&dir, &socket_name(recorded.id))))
        {
            Ok(socket) => socket,
            Err(error) => return gone_or(format!("custody-socket: {error}")),
        };
        if sys::peer_cred(&socket).map(|cred| cred.pid).ok() != Some(host_pid) {
            return Attach::Unattached {
                reason: "custody-socket-unattributed".to_owned(),
            };
        }
        let hello = json!({ "op": "hello", "token": recorded.token, "generation": generation });
        let reply = match request_json(&socket, &hello).and_then(|()| recv_json(&socket)) {
            Ok(Some((value, _))) => value,
            Ok(None) => return gone_or("closed-before-attach".to_owned()),
            Err(error) => return gone_or(format!("attach: {error}")),
        };
        if reply["event"] != "attached" {
            return Attach::Unattached {
                reason: format!("refused: {}", reply["reason"]),
            };
        }
        let mut live = Vec::new();
        loop {
            match recv_json(&socket) {
                Ok(Some((value, fds))) if value["event"] == "work-fds" => {
                    let work = value["work"]
                        .as_str()
                        .and_then(|name| name.strip_prefix('w'))
                        .and_then(|id| id.parse().ok());
                    let host = reply["works"]
                        .as_array()
                        .and_then(|works| works.iter().find(|w| w["work"] == value["work"]))
                        .and_then(|w| w["harness_host_pid"].as_i64())
                        .and_then(|pid| i32::try_from(pid).ok());
                    if let (Some(work), Some(stdio)) = (work, WorkStdio::from_fds(fds)) {
                        live.push(Adopted {
                            work,
                            stdio,
                            harness_host_pid: host,
                        });
                    }
                }
                Ok(Some((value, _))) if value["event"] == "attach-complete" => break,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return gone_or("closed-during-attach".to_owned()),
            }
        }
        let receipts = reply["receipts"].as_array().cloned().unwrap_or_default();
        match Self::connected(
            socket,
            recorded.id,
            host_pid,
            pidfd,
            false,
            Arc::clone(stop),
        ) {
            Ok(root) => {
                {
                    let (lock, _) = &*root.shared;
                    let mut shared = lock.lock().expect("custody lock");
                    for receipt in &receipts {
                        if let Some(work) = receipt["work"].as_str() {
                            shared.receipts.insert(work.to_owned(), receipt.clone());
                        }
                    }
                }
                Attach::Attached {
                    root,
                    live,
                    receipts,
                }
            }
            Err(error) => Attach::Unattached {
                reason: format!("reader: {error}"),
            },
        }
    }

    fn wait<T>(&self, mut ready: impl FnMut(&mut Shared) -> Option<T>) -> T {
        let (lock, condvar) = &*self.shared;
        let mut shared = lock.lock().expect("custody lock");
        loop {
            if let Some(value) = ready(&mut shared) {
                return value;
            }
            shared = condvar.wait(shared).expect("custody lock");
        }
    }

    fn request(&self, mut value: Value) -> Option<(Value, Vec<OwnedFd>)> {
        let req = {
            let (lock, _) = &*self.shared;
            let mut shared = lock.lock().expect("custody lock");
            if shared.lost || shared.detached {
                return None;
            }
            shared.next_req += 1;
            shared.next_req
        };
        value["req"] = json!(req);
        request_json(&self.socket, &value).ok()?;
        self.wait(|shared| {
            if let Some(reply) = shared.replies.remove(&req) {
                Some(Some(reply))
            } else if shared.lost || shared.detached {
                Some(None)
            } else {
                None
            }
        })
    }

    pub(crate) fn spawn(&self, work: i64, argv: &[String]) -> Result<Spawned, String> {
        let Some((reply, fds)) =
            self.request(json!({ "op": "spawn", "work": work_name(work), "argv": argv }))
        else {
            return Err("root-pid1-unreachable".to_owned());
        };
        if reply["event"] != "spawned" {
            return Err(format!("refused: {}", reply["reason"]));
        }
        let stdio = WorkStdio::from_fds(fds).ok_or("spawned without stdio")?;
        Ok(Spawned {
            stdio,
            harness_host_pid: reply["harness_host_pid"]
                .as_i64()
                .and_then(|pid| i32::try_from(pid).ok()),
            exec_error: reply["exec_error"].as_str().map(str::to_owned),
        })
    }

    /// Asks root PID 1 to have the work's PID 1 kill its harness.
    pub(crate) fn kill(&self, work: i64) -> bool {
        self.request(json!({ "op": "kill", "work": work_name(work) }))
            .is_some_and(|(reply, _)| reply["sent"] == true)
    }

    pub(crate) fn wait_receipt(&self, work: i64) -> ReceiptWait {
        let name = work_name(work);
        self.wait(|shared| {
            if let Some(receipt) = shared.receipts.remove(&name) {
                Some(ReceiptWait::Receipt(receipt))
            } else if shared.detached {
                Some(ReceiptWait::Detached)
            } else if shared.lost {
                Some(ReceiptWait::Lost)
            } else {
                None
            }
        })
    }

    /// Stops acting on this root: no further requests, and every waiter
    /// and blocked harness read returns. Kills nothing.
    pub(crate) fn detach(&self) {
        let (lock, condvar) = &*self.shared;
        lock.lock().expect("custody lock").detached = true;
        condvar.notify_all();
        self.stop.trigger();
    }

    /// Asks root PID 1 to exit (it refuses while it has live work), then
    /// observes its end: its exit status only by this owner's own wait as
    /// its parent; otherwise only that it exited, through the pidfd.
    pub(crate) fn release(&self) -> Release {
        let reply = self.request(json!({ "op": "release" }));
        let live = match &reply {
            Some((value, _)) if value["event"] == "refused" => value["live"]
                .as_array()
                .map(|live| {
                    live.iter()
                        .filter_map(|work| work.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        if !live.is_empty() {
            return Release {
                outcome: "retained-live-work",
                status: None,
                live,
            };
        }
        let released = matches!(&reply, Some((value, _)) if value["event"] == "releasing");
        if self.parent {
            return match sys::wait_child(self.host_pid) {
                Ok(status) => Release {
                    outcome: if released {
                        "released-exit-waited"
                    } else {
                        "lost-exit-waited"
                    },
                    status: Some(sys::describe_status(status)),
                    live,
                },
                Err(_) => Release {
                    outcome: "wait-failed",
                    status: None,
                    live,
                },
            };
        }
        let exited = sys::pidfd_exited(&self.pidfd, EXIT_OBSERVATION_MS).unwrap_or(false);
        Release {
            outcome: match (released, exited) {
                (true, true) => "released-exit-observed-by-pidfd",
                (false, true) => "lost-exit-observed-by-pidfd",
                (_, false) => "exit-not-observed",
            },
            status: None,
            live,
        }
    }
}

fn read_loop(socket: &OwnedFd, shared: &Arc<(Mutex<Shared>, Condvar)>, stop: &StopSignal) {
    let (lock, condvar) = &**shared;
    loop {
        let message = recv_json(socket);
        let mut state = lock.lock().expect("custody lock");
        match message {
            Ok(Some((value, fds))) => {
                if let Some(req) = value["req"].as_u64() {
                    state.replies.insert(req, (value, fds));
                } else if value["event"] == "receipt" {
                    if let Some(work) = value["work"].as_str() {
                        state.receipts.insert(work.to_owned(), value);
                    }
                } else if value["event"] == "superseded" {
                    // A newer owner attached: this one may no longer act.
                    state.detached = true;
                    stop.trigger();
                }
            }
            Ok(None) | Err(_) => {
                state.lost = true;
                condvar.notify_all();
                return;
            }
        }
        condvar.notify_all();
    }
}

/// This owner's root PID 1, started on first launch unless an attached
/// survivor already supplied it. At most one per owner instance.
pub(crate) struct RootSlot {
    pub(crate) store_dir: PathBuf,
    pub(crate) generation: i64,
    pub(crate) isolation: Isolation,
    pub(crate) stop: Arc<StopSignal>,
    root: Mutex<Option<Arc<Root>>>,
}

impl RootSlot {
    pub(crate) fn new(
        store_dir: PathBuf,
        generation: i64,
        isolation: Isolation,
        stop: Arc<StopSignal>,
        attached: Option<Arc<Root>>,
    ) -> Self {
        Self {
            store_dir,
            generation,
            isolation,
            stop,
            root: Mutex::new(attached),
        }
    }

    pub(crate) fn current(&self) -> Option<Arc<Root>> {
        self.root.lock().expect("root slot").clone()
    }

    /// The root PID 1, starting a new recorded incarnation if there is
    /// none. `Err(Ok(_))` is a store loss; `Err(Err(_))` a start failure.
    pub(crate) fn ensure(
        &self,
        store: &Mutex<Store>,
        tx: &Sender<Event>,
    ) -> Result<Arc<Root>, Result<StoreError, String>> {
        let mut slot = self.root.lock().expect("root slot");
        if let Some(root) = slot.as_ref() {
            return Ok(Arc::clone(root));
        }
        let token = sys::random_hex().map_err(|error| Err(error.to_string()))?;
        let id = store
            .lock()
            .expect("store lock")
            .begin_incarnation(&token, self.isolation.label())
            .map_err(Ok)?;
        let started = Root::start(
            &self.store_dir,
            id,
            &token,
            self.generation,
            self.isolation,
            Arc::clone(&self.stop),
        );
        let (root, identity) = match started {
            Ok(started) => started,
            Err(error) => {
                let _ = store
                    .lock()
                    .expect("store lock")
                    .end_incarnation(id, "start-failed-child-waited");
                return Err(Err(error.to_string()));
            }
        };
        // Held from here, so the run's end releases it whatever follows.
        *slot = Some(Arc::clone(&root));
        store
            .lock()
            .expect("store lock")
            .record_incarnation_identity(
                id,
                identity.host_pid,
                identity.start_time,
                &identity.boot_id,
            )
            .map_err(Ok)?;
        let _ = tx.send(Event::Report(json!({
            "event": "root-pid1-started",
            "pid": identity.host_pid,
            "incarnation": id,
            "isolation": self.isolation.label(),
            "parent": "this-owner",
        })));
        Ok(root)
    }
}
