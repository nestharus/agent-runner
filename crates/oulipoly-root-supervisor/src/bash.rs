//! This root's Bash ingress: the positive path by which Bash work started
//! inside one of this owner's harness namespaces reaches **this** owner,
//! and nothing else. See the crate docs (Bash) for the wire.
//!
//! * The listener is `<store>/bash.sock`, inside the root's private store
//!   directory, and every harness is started with its path in
//!   [`BASH_ENV`], which its descendants inherit.
//! * Attribution is positive: the connecting process (`SO_PEERCRED`) must
//!   have this owner's uid and be, at that moment, a member of the exact PID
//!   namespace of one live harness work of this owner, as that work's own
//!   PID 1 recorded it. Everything else is refused (`peer-unattributed`),
//!   reported, and nothing is recorded or run: no fallback, no other owner.
//! * An accepted run is recorded durably (a `bash` work) before the
//!   requester is told `accepted`, then launched by root PID 1 as its own
//!   work: a per-work PID 1 in a new PID namespace and the command as its
//!   child, reading `/dev/null`, stderr joined to stdout. Its output is
//!   streamed back; its end comes only from its work PID 1's wait.
//! * Each line to the requester is one stage: `accepted` (durable record,
//!   not a start), `started`, `output`, `output-closed` (end of stream, not
//!   an end) or `output-failed`, then exactly one of `end` (with separate
//!   output state), `end-unknown`, `launch-failed` (proven no-start),
//!   `launch-unknown` (possible effects) or `left-to-successor`.
//!   Uncertain launches are never retried. A requester that goes away does not stop the run;
//!   the owner still waits for its end and records it.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use serde_json::{Value, json};

use crate::Event;
use crate::custody::{Adopted, PidNs, ReceiptWait, Root, RootSlot, SpawnError};
use crate::live::Custody;
use crate::store::Store;
use crate::sys;
use crate::transport;

/// Environment variable naming this root's Bash ingress socket.
pub const BASH_ENV: &str = "OULIPOLY_ROOT_BASH_V1";
/// The ingress socket's name in the store directory.
pub(crate) const SOCKET_FILE: &str = "bash.sock";
/// Protocol version of a run request.
pub const PROTOCOL: u64 = 1;
/// Longest request line accepted.
const MAX_REQUEST: u64 = 1 << 16;

/// An owner input sent (or being sent) to a harness's live process and not
/// yet covered by a turn end the harness attributed to it. `message_id` is
/// its acknowledged id, unknown until the acknowledgement is recorded.
#[derive(Debug, Clone)]
pub(crate) struct OpenInput {
    pub(crate) index: usize,
    pub(crate) message_id: Option<String>,
}

/// What the ingress knows of one harness, kept current by its worker.
#[derive(Default)]
pub(crate) struct View {
    pub(crate) id: String,
    pub(crate) session: Option<String>,
    /// Live works of this harness and each one's own PID namespace.
    pub(crate) works: Vec<(i64, PidNs)>,
    pub(crate) open: Vec<OpenInput>,
}

/// Every harness's view, by position.
pub(crate) type Views = Arc<Mutex<Vec<View>>>;

#[derive(Default)]
struct Gate {
    closed: bool,
    /// Runs admitted and not yet finished, including taken-over ones.
    open: usize,
    accepted: u64,
    refused: u64,
    not_run: u64,
    ended: u64,
    unknown: u64,
}

/// How one admitted run finished, for the owner's own accounting.
#[derive(Clone, Copy)]
enum Outcome {
    /// Its end was reported by its actual waiter.
    Ended,
    /// It may have run; how it ended is not known.
    Unknown,
    /// Nothing was started.
    NotRun,
}

/// One owner run's Bash ingress.
pub(crate) struct Ingress {
    store_dir: PathBuf,
    root_id: String,
    slot: Arc<RootSlot>,
    custody: Arc<Mutex<Custody>>,
    store: Arc<Mutex<Store>>,
    views: Views,
    tx: Sender<Event>,
    gate: Arc<(Mutex<Gate>, Condvar)>,
}

/// A Bash run an earlier owner accepted, as found at attach.
pub(crate) enum Prior {
    Live {
        harness: usize,
        adopted: Adopted,
    },
    Exited {
        harness: usize,
        work: i64,
        receipt: Value,
    },
    Unknown {
        harness: usize,
        work: i64,
    },
}

impl Ingress {
    pub(crate) fn new(
        store_dir: PathBuf,
        root_id: String,
        slot: Arc<RootSlot>,
        custody: Arc<Mutex<Custody>>,
        store: Arc<Mutex<Store>>,
        views: Views,
        tx: Sender<Event>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store_dir,
            root_id,
            slot,
            custody,
            store,
            views,
            tx,
            gate: Arc::default(),
        })
    }

    pub(crate) fn socket_path(store: &Path) -> PathBuf {
        store.join(SOCKET_FILE)
    }

    fn report(&self, value: Value) {
        let _ = self.tx.send(Event::Report(value));
    }

    /// Binds the socket and serves it on a thread. Returns the socket path,
    /// or why there is no ingress (then every Bash request fails to
    /// connect, which a requester must read as no owner, never as success).
    pub(crate) fn listen(self: &Arc<Self>) -> Result<PathBuf, String> {
        let path = Self::socket_path(&self.store_dir);
        // A socket file left by an earlier owner of this store names no
        // listener: this owner holds the store lock.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("stale socket: {error}")),
        }
        let listener = UnixListener::bind(&path).map_err(|error| error.to_string())?;
        let ingress = Arc::clone(self);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if ingress.gate.0.lock().expect("bash gate").closed {
                    let _ = refuse(&stream, "ingress-closed");
                    if ingress.closed_and_idle() {
                        return;
                    }
                    continue;
                }
                let ingress = Arc::clone(&ingress);
                thread::spawn(move || ingress.serve(stream));
            }
        });
        Ok(path)
    }

    fn closed_and_idle(&self) -> bool {
        let gate = self.gate.0.lock().expect("bash gate");
        gate.closed && gate.open == 0
    }

    /// Whether no run is in progress. When none is, closes the ingress: no
    /// later request is accepted. Called by the owning loop once every
    /// harness's end is reported.
    pub(crate) fn close_if_idle(&self) -> bool {
        let mut gate = self.gate.0.lock().expect("bash gate");
        if gate.open > 0 {
            return false;
        }
        if !gate.closed {
            gate.closed = true;
            drop(gate);
            // Wakes the accept loop so it sees the close and returns.
            let path = Self::socket_path(&self.store_dir);
            let _ = UnixStream::connect(&path);
            let _ = std::fs::remove_file(&path);
        }
        true
    }

    /// Counts for the terminal report.
    pub(crate) fn summary(&self) -> Value {
        let gate = self.gate.0.lock().expect("bash gate");
        json!({
            "accepted": gate.accepted,
            "refused": gate.refused,
            "not_run": gate.not_run,
            "ended": gate.ended,
            "end_unknown": gate.unknown,
            "open": gate.open,
        })
    }

    pub(crate) fn ends_unproven(&self) -> bool {
        let gate = self.gate.0.lock().expect("bash gate");
        gate.unknown > 0 || gate.open > 0
    }

    /// Admits one run into the count the owning loop waits on, unless closed.
    fn enter(&self) -> bool {
        let mut gate = self.gate.0.lock().expect("bash gate");
        if gate.closed {
            return false;
        }
        gate.open += 1;
        true
    }

    fn leave(&self, outcome: Outcome) {
        {
            let mut gate = self.gate.0.lock().expect("bash gate");
            gate.open -= 1;
            match outcome {
                Outcome::Ended => gate.ended += 1,
                Outcome::Unknown => gate.unknown += 1,
                Outcome::NotRun => gate.not_run += 1,
            }
        }
        let _ = self.tx.send(Event::BashDone);
    }

    fn refused(&self, stream: &UnixStream, peer: Value, reason: &str) {
        self.gate.0.lock().expect("bash gate").refused += 1;
        let _ = refuse(stream, reason);
        self.report(json!({ "event": "bash-refused", "reason": reason, "peer": peer }));
    }

    fn serve(self: Arc<Self>, stream: UnixStream) {
        let cred = match peer_cred(&stream) {
            Ok(cred) => cred,
            Err(error) => {
                return self.refused(&stream, Value::Null, &format!("peer-unknown: {error}"));
            }
        };
        let peer = json!({ "pid": cred.pid, "uid": cred.uid });
        let attributed = match self.attribute(&cred) {
            Ok(attributed) => attributed,
            Err(reason) => return self.refused(&stream, peer, &reason),
        };
        let request = match read_request(&stream) {
            Ok(request) => request,
            Err(reason) => return self.refused(&stream, peer, &reason),
        };
        let mut sink = Sink(Some(stream));
        self.run(&attributed, cred.pid, &request, &mut sink);
    }

    /// The harness whose live work's own PID namespace the peer is in.
    fn attribute(&self, cred: &libc::ucred) -> Result<Attributed, String> {
        // SAFETY: geteuid has no preconditions.
        if cred.uid != unsafe { libc::geteuid() } {
            return Err("peer-unattributed: other-uid".to_owned());
        }
        let pidfd = sys::pidfd_open(cred.pid)
            .map_err(|error| format!("peer-unattributed: pidfd: {error}"))?;
        let ns = std::fs::metadata(format!("/proc/{}/ns/pid", cred.pid))
            .map_err(|error| format!("peer-unattributed: pidns: {error}"))?;
        // Still running after the read, so the pid named the peer, not a
        // reuse; a pid can only be reused after its process is reaped.
        if sys::pidfd_exited(&pidfd, 0).unwrap_or(true) {
            return Err("peer-unattributed: gone".to_owned());
        }
        let ns = PidNs {
            dev: ns.dev(),
            ino: ns.ino(),
        };
        let views = self.views.lock().expect("views");
        views
            .iter()
            .enumerate()
            .find_map(|(position, view)| {
                let &(work, _) = view.works.iter().find(|(_, pidns)| *pidns == ns)?;
                Some(Attributed {
                    position,
                    harness: view.id.clone(),
                    harness_work: work,
                    session: view.session.clone(),
                    open: view.open.clone(),
                })
            })
            .ok_or_else(|| "peer-unattributed: outside-every-harness-namespace".to_owned())
    }

    fn run(&self, who: &Attributed, requester: i32, request: &RunRequest, sink: &mut Sink) {
        let open: Vec<Value> = who
            .open
            .iter()
            .map(|input| json!({ "index": input.index, "message_id": input.message_id }))
            .collect();
        // Which owner input caused this run is known only when exactly one
        // was open on the requesting harness; the request carries no more.
        let input_attribution = match who.open.len() {
            0 => "no-open-input",
            1 => "single-open-input",
            _ => "ambiguous-open-inputs",
        };
        let shared = Arc::clone(&self.custody);
        let mut custody = shared.lock().expect("custody lock");
        if let Some(reason) = custody.reason() {
            drop(custody);
            self.refused(
                sink.stream(),
                json!({ "pid": requester }),
                &format!("owner-stopping: {reason}"),
            );
            return;
        }
        if !self.enter() {
            drop(custody);
            self.refused(sink.stream(), json!({ "pid": requester }), "ingress-closed");
            return;
        }
        let Some(root) = self.slot.current() else {
            drop(custody);
            self.refused(sink.stream(), json!({ "pid": requester }), "no-root-pid1");
            self.leave(Outcome::NotRun);
            return;
        };
        let inputs_open = Value::Array(open.clone()).to_string();
        let begun = self.store.lock().expect("store lock").begin_bash(
            who.position,
            root.incarnation,
            requester,
            &inputs_open,
            &request.argv,
            &request.cwd,
        );
        let work = match begun {
            Ok(work) => work,
            Err(error) => {
                let label = error.label();
                custody.stop(label);
                drop(custody);
                self.report(json!({ "event": label, "reason": format!("{error:?}") }));
                self.refused(sink.stream(), json!({ "pid": requester }), label);
                self.leave(Outcome::NotRun);
                return;
            }
        };
        self.gate.0.lock().expect("bash gate").accepted += 1;
        let attribution = json!({
            "root_id": self.root_id,
            "harness": who.harness,
            "harness_work": who.harness_work,
            "session": who.session,
            "requester_pid": requester,
            "inputs_open": open,
            "input_attribution": input_attribution,
        });
        let mut accepted = json!({ "event": "accepted", "work": work, "durable": true });
        merge(&mut accepted, &attribution);
        sink.send(&accepted);
        let mut owner = json!({ "event": "bash-accepted", "work": work, "argv": request.argv });
        merge(&mut owner, &attribution);
        self.report(owner);
        let spawned = root.spawn_observed(
            work,
            &request.argv,
            &serde_json::Map::new(),
            &request.cwd,
            true,
        );
        let spawned = match spawned {
            Ok(spawned) => spawned,
            Err(reason) => {
                drop(custody);
                let not_started = matches!(reason, SpawnError::NotStarted(_));
                let event = if not_started {
                    "launch-failed"
                } else {
                    "launch-unknown"
                };
                if not_started {
                    let _ = self.store.lock().expect("store lock").resolve_work(
                        work,
                        "launch-refused",
                        Some("root-pid1-no-start-reply"),
                    );
                }
                // Unknown stays unresolved for a successor; never retry here.
                sink.send(&json!({ "event": event, "reason": reason.reason(), "not_started": not_started }));
                self.report(json!({ "event": format!("bash-{event}"), "work": work, "reason": reason.reason(), "not_started": not_started }));
                self.leave(if not_started {
                    Outcome::NotRun
                } else {
                    Outcome::Unknown
                });
                return;
            }
        };
        let token = custody.register(Arc::clone(&root), work);
        drop(custody);
        let _ = self
            .store
            .lock()
            .expect("store lock")
            .record_work_spawned(work, spawned.harness_host_pid);
        sink.send(&json!({
            "event": "started",
            "pid": spawned.harness_host_pid,
            "exec_error": spawned.exec_error,
        }));
        self.report(json!({
            "event": "bash-started",
            "work": work,
            "pid": spawned.harness_host_pid,
            "exec_error": spawned.exec_error,
        }));
        self.finish(&root, work, token, spawned.stdio.stdout, Some(sink));
    }

    /// Streams the work's output to `sink` (or discards it), then waits for
    /// its end from its work PID 1's report, records it and says so.
    fn finish(&self, root: &Root, work: i64, token: u64, stdout: File, sink: Option<&mut Sink>) {
        let mut sink = sink;
        let (bytes, output) = relay_output(&self.slot.stop, stdout, &mut sink);
        let (mut event, outcome) = match root.wait_receipt(work) {
            ReceiptWait::Receipt(receipt) => match receipt["harness"].as_str() {
                Some(status) => {
                    let _ = self.store.lock().expect("store lock").resolve_work(
                        work,
                        status,
                        Some("work-pid1-wait"),
                    );
                    (
                        json!({
                            "event": "end",
                            "status": status,
                            "observer": "work-pid1-wait",
                            "work_pid1": receipt["work_pid1"],
                        }),
                        Outcome::Ended,
                    )
                }
                None => {
                    let _ = self.store.lock().expect("store lock").resolve_work(
                        work,
                        "ended-with-work-namespace-status-unknown",
                        None,
                    );
                    (
                        json!({
                            "event": "end-unknown",
                            "reason": "ended-with-work-namespace-status-unknown",
                            "work_pid1": receipt["work_pid1"],
                        }),
                        Outcome::Unknown,
                    )
                }
            },
            ReceiptWait::Lost => (
                json!({ "event": "end-unknown", "reason": "root-pid1-connection-lost" }),
                Outcome::Unknown,
            ),
            ReceiptWait::Detached => (json!({ "event": "left-to-successor" }), Outcome::Unknown),
        };
        event["output"] = output;
        self.custody.lock().expect("custody lock").release(token);
        let caller = match sink {
            Some(sink) => {
                sink.send(&event);
                if sink.0.is_some() {
                    "connected"
                } else {
                    "gone"
                }
            }
            None => "lost-with-prior-owner",
        };
        let mut owner = event;
        owner["bash"] = owner["event"].take();
        owner["event"] = json!("bash-ended");
        owner["work"] = json!(work);
        owner["output_bytes"] = json!(bytes);
        owner["requester"] = json!(caller);
        self.report(owner);
        self.leave(outcome);
    }

    /// Takes over a Bash run an earlier owner accepted. Its requester's
    /// connection died with that owner: output is drained, not delivered.
    pub(crate) fn recover(self: &Arc<Self>, prior: Prior) {
        let mut gate = self.gate.0.lock().expect("bash gate");
        gate.open += 1;
        drop(gate);
        let ingress = Arc::clone(self);
        thread::spawn(move || match prior {
            Prior::Live { harness, adopted } => {
                let Some(root) = ingress.slot.current() else {
                    ingress.leave(Outcome::Unknown);
                    return;
                };
                let mut custody = ingress.custody.lock().expect("custody lock");
                let token = custody.register(Arc::clone(&root), adopted.work);
                let stopped = custody.reason();
                drop(custody);
                if stopped.is_some_and(|reason| reason != "authority-lost") {
                    root.kill(adopted.work);
                }
                ingress.report(json!({
                    "event": "bash-reattached",
                    "work": adopted.work,
                    "harness_position": harness,
                    "pid": adopted.harness_host_pid,
                }));
                ingress.finish(&root, adopted.work, token, adopted.stdio.stdout, None);
            }
            Prior::Exited {
                harness,
                work,
                receipt,
            } => {
                let status = receipt["harness"].as_str();
                let outcome = status.unwrap_or("ended-with-work-namespace-status-unknown");
                let _ = ingress.store.lock().expect("store lock").resolve_work(
                    work,
                    outcome,
                    status.map(|_| "work-pid1-wait"),
                );
                ingress.report(json!({
                    "event": "bash-prior-end",
                    "work": work,
                    "harness_position": harness,
                    "status": status,
                    "meaning": if status.is_some() { "exit-reported-by-its-waiter" } else { "ended-with-work-namespace-status-unknown" },
                }));
                ingress.leave(if status.is_some() {
                    Outcome::Ended
                } else {
                    Outcome::Unknown
                });
            }
            Prior::Unknown { harness, work } => {
                let _ = ingress.store.lock().expect("store lock").resolve_work(
                    work,
                    "ended-with-root-namespace-status-unknown",
                    None,
                );
                ingress.report(json!({
                    "event": "bash-prior-end",
                    "work": work,
                    "harness_position": harness,
                    "status": Value::Null,
                    "meaning": "ended-with-root-namespace-status-unknown",
                }));
                ingress.leave(Outcome::Unknown);
            }
        });
    }
}

/// EOF, read failure and detach are different observations; none is a wait.
fn relay_output(
    stop: &transport::StopSignal,
    mut stdout: File,
    sink: &mut Option<&mut Sink>,
) -> (u64, Value) {
    let mut bytes = 0u64;
    let mut buf = [0u8; 16 * 1024];
    let output = loop {
        match transport::readable(stop, &stdout) {
            Ok(true) => {}
            Ok(false) => break json!({ "state": "unknown", "reason": "detached" }),
            Err(error) => break json!({ "state": "failed", "reason": error.to_string() }),
        }
        match stdout.read(&mut buf) {
            Ok(0) => break json!({ "state": "closed", "bytes": bytes }),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => break json!({ "state": "failed", "reason": error.to_string() }),
            Ok(read) => {
                bytes += read as u64;
                if let Some(sink) = sink.as_deref_mut() {
                    sink.send(&json!({ "event": "output", "b64": base64(&buf[..read]) }));
                }
            }
        }
    };
    if let Some(sink) = sink.as_deref_mut() {
        match output["state"].as_str() {
            Some("closed") => sink.send(&json!({ "event": "output-closed", "bytes": bytes })),
            Some("failed") => {
                sink.send(&json!({ "event": "output-failed", "reason": output["reason"] }))
            }
            _ => {}
        }
    }
    (bytes, output)
}

struct Attributed {
    position: usize,
    harness: String,
    harness_work: i64,
    session: Option<String>,
    open: Vec<OpenInput>,
}

struct RunRequest {
    argv: Vec<String>,
    cwd: String,
}

/// The requester's connection; writes stop being attempted once it fails.
struct Sink(Option<UnixStream>);

impl Sink {
    fn send(&mut self, value: &Value) {
        if let Some(stream) = self.0.as_mut()
            && writeln!(stream, "{value}")
                .and_then(|()| stream.flush())
                .is_err()
        {
            self.0 = None;
        }
    }

    fn stream(&self) -> &UnixStream {
        self.0
            .as_ref()
            .expect("requester stream before any write failed")
    }
}

fn refuse(mut stream: &UnixStream, reason: &str) -> std::io::Result<()> {
    writeln!(
        stream,
        "{}",
        json!({ "event": "refused", "reason": reason })
    )?;
    stream.flush()
}

fn merge(into: &mut Value, from: &Value) {
    if let (Some(into), Some(from)) = (into.as_object_mut(), from.as_object()) {
        for (key, value) in from {
            into.insert(key.clone(), value.clone());
        }
    }
}

fn peer_cred(stream: &UnixStream) -> std::io::Result<libc::ucred> {
    let fd: OwnedFd = stream.as_fd().try_clone_to_owned()?;
    sys::peer_cred(&fd)
}

fn read_request(stream: &UnixStream) -> Result<RunRequest, String> {
    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST))
        .read_line(&mut line)
        .map_err(|error| format!("malformed: {error}"))?;
    let value: Value = serde_json::from_str(line.trim()).map_err(|_| "malformed".to_owned())?;
    if value["v"].as_u64() != Some(PROTOCOL) {
        return Err("unsupported-version".to_owned());
    }
    if value["op"] != "run" {
        return Err("unknown-op".to_owned());
    }
    let argv: Option<Vec<String>> = value["argv"].as_array().and_then(|argv| {
        argv.iter()
            .map(|arg| {
                arg.as_str()
                    .filter(|arg| !arg.contains('\0'))
                    .map(str::to_owned)
            })
            .collect()
    });
    let argv = argv.filter(|argv| !argv.is_empty()).ok_or("bad-argv")?;
    let cwd = value["cwd"]
        .as_str()
        .filter(|cwd| cwd.starts_with('/') && !cwd.contains('\0'))
        .ok_or("bad-cwd")?
        .to_owned();
    Ok(RunRequest { argv, cwd })
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0u32, |acc, (i, &byte)| {
            acc | u32::from(byte) << (16 - 8 * i)
        });
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(triple >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Inverse of [`base64`]; `None` for anything it did not produce.
pub fn unbase64(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let chunks = text.as_bytes().chunks(4);
    let count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        let pad = chunk.iter().rev().take_while(|&&byte| byte == b'=').count();
        if pad > 2 || (pad > 0 && index + 1 != count) {
            return None;
        }
        let mut triple = 0u32;
        for (i, &byte) in chunk[..4 - pad].iter().enumerate() {
            triple |= u32::from(value(byte)?) << (18 - 6 * i);
        }
        if (pad == 2 && triple & 0xffff != 0) || (pad == 1 && triple & 0xff != 0) {
            return None;
        }
        for i in 0..3 - pad {
            out.push((triple >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::StopSignal;
    use crate::{Endpoint, HarnessSpec, Intent};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    // Configured custody peer, not a native root: faults occur after an
    // actual deterministic command effect in a fresh private fixture.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("bash-honesty-{}", sys::random_hex().unwrap()));
            std::fs::create_dir(&dir).unwrap();
            eprintln!("owned-fixture: {}", dir.display());
            Self(dir)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn fixture(dir: &Path) -> (Arc<Ingress>, Arc<Root>, OwnedFd) {
        let intent = Intent {
            outage_closure_cap: 1,
            delivery_attempt_cap: 1,
            cwd: dir.display().to_string(),
            harnesses: vec![HarnessSpec {
                id: "h".into(),
                argv: vec!["peer".into()],
                endpoint: Endpoint::Stdio,
                session: None,
                messages: vec!["one".into()],
            }],
        };
        let mut claimed = Store::claim(&dir.join("store"), Some(&intent)).unwrap();
        claimed
            .store
            .begin_incarnation("fixture", "fixture")
            .unwrap();
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, far) = Root::seam(false, i32::try_from(std::process::id()).unwrap(), &stop);
        let slot = Arc::new(RootSlot::new(
            dir.join("store"),
            1,
            sys::Isolation::UnprivilegedUserns,
            Arc::clone(&stop),
            Some(Arc::clone(&root)),
        ));
        let (tx, _rx) = channel();
        let ingress = Ingress::new(
            dir.join("store"),
            claimed.root_id,
            slot,
            Arc::new(Mutex::new(Custody::new(stop))),
            Arc::new(Mutex::new(claimed.store)),
            Arc::default(),
            tx,
        );
        (ingress, root, far)
    }

    #[test]
    fn spawn_reply_faults_keep_possible_effects_unknown_and_intent_open() {
        for mode in ["lost-reply", "missing-stdio", "unproven-refusal", "refused"] {
            let dir = Fixture::new();
            let (ingress, _root, far) = fixture(&dir.0);
            let marker = dir.0.join("effects");
            let replier = thread::spawn(move || {
                let (bytes, _) = sys::recv(&far).unwrap().unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                if mode != "refused" {
                    let status = std::process::Command::new("/bin/sh")
                        .args(["-c", "printf x >> effects"])
                        .current_dir(marker.parent().unwrap())
                        .status()
                        .unwrap();
                    assert!(status.success());
                }
                if mode != "lost-reply" {
                    let reply = if mode == "refused" {
                        json!({ "req": request["req"], "event": "refused", "reason": "fixture-no-start", "not_started": true })
                    } else if mode == "unproven-refusal" {
                        json!({ "req": request["req"], "event": "refused", "reason": "fixture-after-creation", "not_started": false })
                    } else {
                        json!({ "req": request["req"], "event": "spawned" })
                    };
                    sys::send(&far, reply.to_string().as_bytes(), &[]).unwrap();
                }
            });
            let (near, far) = UnixStream::pair().unwrap();
            far.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let who = Attributed {
                position: 0,
                harness: "h".into(),
                harness_work: 0,
                session: None,
                open: vec![],
            };
            let request = RunRequest {
                argv: vec!["fixture".into()],
                cwd: dir.0.display().to_string(),
            };
            ingress.run(&who, 1, &request, &mut Sink(Some(near)));
            replier.join().unwrap();
            let events: Vec<Value> = BufReader::new(far)
                .lines()
                .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
                .collect();
            assert_eq!(events[0]["event"], "accepted");
            let conn = rusqlite::Connection::open(dir.0.join("store").join(crate::store::DB_FILE))
                .unwrap();
            let outcome: Option<String> = conn
                .query_row("SELECT outcome FROM work WHERE kind = 'bash'", [], |row| {
                    row.get(0)
                })
                .unwrap();
            if mode == "refused" {
                assert_eq!(events[1]["event"], "launch-failed");
                assert_eq!(outcome.as_deref(), Some("launch-refused"));
                assert!(!dir.0.join("effects").exists());
                assert_eq!(ingress.summary()["not_run"], 1);
            } else {
                assert_eq!(
                    std::fs::read(dir.0.join("effects")).unwrap(),
                    b"x",
                    "one effect, no retry"
                );
                assert_eq!(events[1]["event"], "launch-unknown");
                assert_eq!(outcome, None, "retain unresolved durable intent");
                assert_eq!(ingress.summary()["not_run"], 0);
                assert_eq!(ingress.summary()["end_unknown"], 1);
                assert!(ingress.ends_unproven());
            }
        }
    }

    #[test]
    fn output_read_failure_preserves_wait_but_never_claims_eof() {
        let dir = Fixture::new();
        let (ingress, root, far) = fixture(&dir.0);
        let work = ingress
            .store
            .lock()
            .unwrap()
            .begin_bash(0, 1, 1, "[]", &["fixture".into()], "/")
            .unwrap();
        assert!(ingress.enter());
        let token = ingress
            .custody
            .lock()
            .unwrap()
            .register(Arc::clone(&root), work);
        let receipt = json!({ "event": "receipt", "work": crate::custody::work_name(work), "harness": "code:0", "work_pid1": "code:0" });
        sys::send(&far, receipt.to_string().as_bytes(), &[]).unwrap();
        let (near, far) = UnixStream::pair().unwrap();
        far.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut sink = Sink(Some(near));
        // Linux directory read yields EISDIR; it is not EOF.
        ingress.finish(
            &root,
            work,
            token,
            File::open(&dir.0).unwrap(),
            Some(&mut sink),
        );
        drop(sink);
        let events: Vec<Value> = BufReader::new(far)
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect();
        assert_eq!(events[0]["event"], "output-failed");
        assert_eq!(events[1]["event"], "end");
        assert_eq!(events[1]["status"], "code:0");
        assert_eq!(events[1]["output"]["state"], "failed");
        assert!(events.iter().all(|event| event["event"] != "output-closed"));
        assert_eq!(
            ingress.summary()["ended"],
            1,
            "actual wait remains distinct"
        );
    }

    #[test]
    fn base64_round_trips_every_tail_length() {
        for len in 0..9 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 200) as u8).collect();
            assert_eq!(unbase64(&base64(&bytes)).unwrap(), bytes);
        }
        assert_eq!(base64(b"hi\n"), "aGkK");
        assert!(unbase64("a").is_none());
        for corrupt in ["aG==aGkK", "aGl=", "aH==", "!!!!"] {
            assert!(unbase64(corrupt).is_none(), "{corrupt}");
        }
    }
}
