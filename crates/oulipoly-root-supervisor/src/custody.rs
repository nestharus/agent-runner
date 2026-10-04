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
use crate::sys;
use crate::transport::StopSignal;
use crate::workload::Resolved;

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
    pub(crate) pidns: Option<PidNs>,
    pub(crate) exec_error: Option<String>,
}

/// Only a positive no-start reply proves absence of command effects.
#[derive(Debug)]
pub(crate) enum SpawnError {
    NotStarted(String),
    Unknown(String),
}

impl SpawnError {
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::NotStarted(reason) | Self::Unknown(reason) => reason,
        }
    }
}

/// A surviving harness this owner attached to, with its stdio.
pub(crate) struct Adopted {
    pub(crate) work: i64,
    pub(crate) stdio: WorkStdio,
    pub(crate) harness_host_pid: Option<i32>,
    pub(crate) pidns: Option<PidNs>,
}

/// One work's own PID namespace (`st_dev`, `st_ino` of its nsfs inode), as
/// that work's PID 1 read it before starting the harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PidNs {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

impl PidNs {
    fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            dev: value.get(0)?.as_u64()?,
            ino: value.get(1)?.as_u64()?,
        })
    }
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
    /// Work root PID 1 itself said is still live. Empty is not proof of no
    /// live work unless `ended` is true.
    pub(crate) live: Vec<String>,
    /// Root PID 1's end was actually observed (its parent's wait, or its
    /// pidfd). Only then may its incarnation be recorded as ended.
    pub(crate) ended: bool,
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
        workload: &Resolved,
        stop: Arc<StopSignal>,
    ) -> io::Result<(Arc<Self>, Identity)> {
        let (socket, child_end) = sys::seqpacket_pair()?;
        let host_pid = sys::spawn_pid1(&pid1_binary()?, &child_end, workload.isolation)?;
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
                // Every harness and Bash run of this incarnation is started
                // as this identity by its work PID 1 (`null`: as root PID 1).
                "workload": workload.identity.as_ref().map(|identity| json!({
                    "uid": identity.uid,
                    "gid": identity.gid,
                    "groups": identity.groups,
                })),
            }),
        )
        .and_then(|()| recv_json(&socket));
        match ready {
            Ok(Some((value, _)))
                if value["event"] == "ready"
                    && value["workload_uid"].as_u64()
                        == workload
                            .identity
                            .as_ref()
                            .map(|identity| u64::from(identity.uid)) => {}
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
                    let listed = reply["works"]
                        .as_array()
                        .and_then(|works| works.iter().find(|w| w["work"] == value["work"]));
                    let host = listed
                        .and_then(|w| w["harness_host_pid"].as_i64())
                        .and_then(|pid| i32::try_from(pid).ok());
                    let pidns = listed.and_then(|w| PidNs::parse(&w["pidns"]));
                    if let (Some(work), Some(stdio)) = (work, WorkStdio::from_fds(fds)) {
                        live.push(Adopted {
                            work,
                            stdio,
                            harness_host_pid: host,
                            pidns,
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

    /// Asks root PID 1 to start `argv` as the harness of `work`, in `cwd`,
    /// with `env` added to root PID 1's environment. A `command` (a Bash
    /// run, not a harness) reads `/dev/null` and its stderr joins its
    /// stdout; a harness keeps its stdin pipe and root PID 1's stderr.
    pub(crate) fn spawn(
        &self,
        work: i64,
        argv: &[String],
        env: &serde_json::Map<String, Value>,
        cwd: &str,
        command: bool,
    ) -> Result<Spawned, String> {
        self.spawn_observed(work, argv, env, cwd, command)
            .map_err(|error| error.reason().to_owned())
    }

    /// Preserves possible creation when a request or its stdio reply is lost.
    pub(crate) fn spawn_observed(
        &self,
        work: i64,
        argv: &[String],
        env: &serde_json::Map<String, Value>,
        cwd: &str,
        command: bool,
    ) -> Result<Spawned, SpawnError> {
        let Some((reply, fds)) = self.request(json!({
            "op": "spawn",
            "work": work_name(work),
            "argv": argv,
            "env": env,
            "cwd": cwd,
            "stdin": if command { "null" } else { "pipe" },
            "stderr": if command { "stdout" } else { "inherit" },
        })) else {
            return Err(SpawnError::Unknown("root-pid1-unreachable".to_owned()));
        };
        if reply["event"] != "spawned" {
            let reason = format!("spawn reply: {}", reply["reason"]);
            return Err(
                if reply["event"] == "refused" && reply["not_started"] == true {
                    SpawnError::NotStarted(reason)
                } else {
                    SpawnError::Unknown(reason)
                },
            );
        }
        let stdio = WorkStdio::from_fds(fds)
            .ok_or_else(|| SpawnError::Unknown("spawned without stdio".to_owned()))?;
        Ok(Spawned {
            stdio,
            harness_host_pid: reply["harness_host_pid"]
                .as_i64()
                .and_then(|pid| i32::try_from(pid).ok()),
            pidns: PidNs::parse(&reply["pidns"]),
            exec_error: reply["exec_error"].as_str().map(str::to_owned),
        })
    }

    /// Whether root PID 1 already reported the work's end, or this owner can
    /// no longer hear it. Does not consume the report.
    pub(crate) fn end_known(&self, work: i64) -> bool {
        let name = work_name(work);
        let (lock, _) = &*self.shared;
        let shared = lock.lock().expect("custody lock");
        shared.receipts.contains_key(&name) || shared.lost || shared.detached
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
    ///
    /// A missing reply says nothing about root PID 1's work: it is neither
    /// an empty live list nor an end. Without a `releasing` reply this owner
    /// only observes, for a bounded time, whether root PID 1 has exited, and
    /// never blocks in a wait on a root that may still hold live work.
    pub(crate) fn release(&self) -> Release {
        let reply = self.request(json!({ "op": "release" }));
        let event = reply.as_ref().map(|(value, _)| value["event"].clone());
        if event.as_ref().is_some_and(|event| event == "refused")
            && let Some((value, _)) = &reply
        {
            let live: Vec<String> = value["live"]
                .as_array()
                .map(|live| {
                    live.iter()
                        .filter_map(|work| work.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if !live.is_empty() {
                return Release {
                    outcome: "retained-live-work",
                    status: None,
                    live,
                    ended: false,
                };
            }
        }
        if event.as_ref().is_some_and(|event| event == "releasing") {
            // Root PID 1 said it has no live work and is exiting.
            if self.parent {
                return match sys::wait_child(self.host_pid) {
                    Ok(status) => Release {
                        outcome: "released-exit-waited",
                        status: Some(sys::describe_status(status)),
                        live: Vec::new(),
                        ended: true,
                    },
                    Err(_) => Release {
                        outcome: "wait-failed",
                        status: None,
                        live: Vec::new(),
                        ended: false,
                    },
                };
            }
            let exited = sys::pidfd_exited(&self.pidfd, EXIT_OBSERVATION_MS).unwrap_or(false);
            return Release {
                outcome: if exited {
                    "released-exit-observed-by-pidfd"
                } else {
                    "exit-not-observed"
                },
                status: None,
                live: Vec::new(),
                ended: exited,
            };
        }
        // No `releasing` reply: root PID 1 may still be running its work.
        if !sys::pidfd_exited(&self.pidfd, EXIT_OBSERVATION_MS).unwrap_or(false) {
            return Release {
                outcome: if self.stop.superseded() {
                    "left-to-successor"
                } else {
                    "release-unanswered"
                },
                status: None,
                live: Vec::new(),
                ended: false,
            };
        }
        if self.parent {
            // It has exited, so this wait of our own child does not block.
            return match sys::wait_child(self.host_pid) {
                Ok(status) => Release {
                    outcome: "lost-exit-waited",
                    status: Some(sys::describe_status(status)),
                    live: Vec::new(),
                    ended: true,
                },
                Err(_) => Release {
                    outcome: "wait-failed",
                    status: None,
                    live: Vec::new(),
                    ended: false,
                },
            };
        }
        Release {
            outcome: "lost-exit-observed-by-pidfd",
            status: None,
            live: Vec::new(),
            ended: true,
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
                    // A newer owner attached: this one may no longer act,
                    // and its whole run has lost authority over this root.
                    state.detached = true;
                    stop.supersede();
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
    /// The root's resolved work identity and IPC placement.
    pub(crate) workload: Arc<Resolved>,
    pub(crate) stop: Arc<StopSignal>,
    root: Mutex<Option<Arc<Root>>>,
}

impl RootSlot {
    pub(crate) fn new(
        store_dir: PathBuf,
        generation: i64,
        workload: Arc<Resolved>,
        stop: Arc<StopSignal>,
        attached: Option<Arc<Root>>,
    ) -> Self {
        Self {
            store_dir,
            generation,
            workload,
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
            .begin_incarnation(&token, self.workload.isolation.label())
            .map_err(Ok)?;
        let started = Root::start(
            &self.store_dir,
            id,
            &token,
            self.generation,
            &self.workload,
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
            "isolation": self.workload.isolation.label(),
            "workload": self.workload.identity.as_ref().map(crate::workload::Identity::to_json),
            "observed": crate::workload::observe(identity.host_pid),
            "parent": "this-owner",
        })));
        Ok(root)
    }
}

#[cfg(test)]
impl Root {
    /// CONFIGURED SEAM for tests: incarnation 1 over a connection whose far
    /// end the caller holds and plays root PID 1 on, naming `pid`.
    pub(crate) fn seam(parent: bool, pid: i32, stop: &Arc<StopSignal>) -> (Arc<Self>, OwnedFd) {
        let (near, far) = sys::seqpacket_pair().unwrap();
        let pidfd = sys::pidfd_open(pid).unwrap();
        let root = Self::connected(near, 1, pid, pidfd, parent, Arc::clone(stop)).unwrap();
        (root, far)
    }
}

#[cfg(test)]
mod tests {
    //! CONFIGURED SEAMS: each `Root` here is a connection whose far end this
    //! test holds and plays root PID 1 on; no root PID 1 or harness exists.
    //! They exercise this owner's side of custody only.

    use super::*;
    use crate::live::Custody;
    use std::sync::mpsc::channel;
    use std::time::Duration;

    const BOUND: Duration = Duration::from_secs(10);

    fn root(parent: bool, pid: i32, stop: &Arc<StopSignal>) -> (Arc<Root>, OwnedFd) {
        Root::seam(parent, pid, stop)
    }

    /// O1 late registration: a survivor registered after this owner's
    /// authority loss is already known is left to the successor, so
    /// waiting for its end returns `Detached` instead of blocking.
    #[test]
    fn survivor_registered_after_authority_loss_is_left_not_awaited() {
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, _far) = root(false, i32::try_from(std::process::id()).unwrap(), &stop);
        let mut custody = Custody::new(Arc::clone(&stop));
        assert_eq!(custody.stop("authority-lost"), 0);
        custody.register(Arc::clone(&root), 7);
        let (tx, rx) = channel();
        thread::spawn(move || {
            let _ = tx.send(matches!(root.wait_receipt(7), ReceiptWait::Detached));
        });
        assert_eq!(
            rx.recv_timeout(BOUND),
            Ok(true),
            "late-registered survivor must be left to the successor, not awaited"
        );
    }

    /// O1 run level: root PID 1's `superseded` is authority loss for the
    /// whole run, so a later cancel kills nothing (the request would reach
    /// the successor's work) and the run ends as authority loss.
    #[test]
    fn supersession_is_run_level_authority_loss() {
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, far) = root(false, i32::try_from(std::process::id()).unwrap(), &stop);
        let mut custody = Custody::new(Arc::clone(&stop));
        custody.register(Arc::clone(&root), 7);
        request_json(&far, &json!({ "event": "superseded", "by": 2 })).unwrap();
        let (tx, rx) = channel();
        let waiter = Arc::clone(&root);
        thread::spawn(move || {
            let _ = tx.send(matches!(waiter.wait_receipt(7), ReceiptWait::Detached));
        });
        assert_eq!(rx.recv_timeout(BOUND), Ok(true));
        assert_eq!(custody.reason(), Some("authority-lost"));
        assert_eq!(custody.cancel(), 0, "no kill after supersession");
        assert_eq!(custody.reason(), Some("authority-lost"));
    }

    /// O3: a release with no reply is neither an empty live list nor an
    /// end. As the actual parent of a still-running child, this owner does
    /// not block in its wait; it reports the release unanswered, no status,
    /// end not observed.
    #[test]
    fn unanswered_release_neither_blocks_on_a_running_child_nor_claims_its_end() {
        let mut child = std::process::Command::new("sleep")
            .arg("600")
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, far) = root(true, pid, &stop);
        drop(far);
        let (tx, rx) = channel();
        let releasing = Arc::clone(&root);
        thread::spawn(move || {
            let release = releasing.release();
            let _ = tx.send((release.outcome, release.status, release.live, release.ended));
        });
        let released = rx.recv_timeout(BOUND);
        // Exact cleanup of this test's own child, whatever happened above.
        let _ = child.kill();
        let _ = child.wait();
        let (outcome, status, live, ended) =
            released.expect("parent owner blocked in a wait on a running root after no reply");
        assert_eq!(outcome, "release-unanswered");
        assert_eq!(status, None);
        assert!(live.is_empty());
        assert!(!ended, "no end was observed");
    }

    /// O3, not the parent: the same missing reply with the process still
    /// running is not an observed end.
    #[test]
    fn unanswered_release_of_running_root_is_not_an_end() {
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, far) = root(false, i32::try_from(std::process::id()).unwrap(), &stop);
        drop(far);
        let release = root.release();
        assert_eq!(release.outcome, "release-unanswered");
        assert!(!release.ended);
    }

    /// Counter-control: an explicit `releasing` reply followed by the
    /// actual parent's wait is a positively observed end, with its status.
    #[test]
    fn answered_release_waited_by_parent_is_an_observed_end() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, far) = root(true, pid, &stop);
        let replier = thread::spawn(move || {
            let (request, _) = recv_json(&far).unwrap().unwrap();
            request_json(
                &far,
                &json!({ "req": request["req"], "event": "releasing" }),
            )
            .unwrap();
            far
        });
        let release = root.release();
        drop(replier.join().unwrap());
        let _ = child.try_wait();
        assert_eq!(release.outcome, "released-exit-waited");
        assert_eq!(release.status.as_deref(), Some("code:0"));
        assert!(release.ended);
    }
}
