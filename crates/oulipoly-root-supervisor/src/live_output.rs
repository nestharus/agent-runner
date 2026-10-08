//! Opt-in SDK live-stream v3 observation of root-owned combined Bash output.
//!
//! The host attests the requester and excludes this root's PID namespace tree.
//! SDK advertisements/descriptors grant no authority. Required code never takes
//! a blocking capture lock, performs viewer I/O, sweeps, or queues viewer reports.
//! Registration can be omitted; each lost frame has a sequence position. Terminal
//! knowledge has a separate single-writer slot, so ring contention cannot hide it.
//! A maintenance thread bounds idle/grace resources independently of output.
//! Owner close is atomic and never waits for subscribers. A closed owner may cut
//! the tail or terminal short; absence of delivered exit facts says only that.
//!
//! This embedded per-root endpoint is not an account broker or an outside
//! retained-read service. Byte charges are accounting, not measured RSS.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use agent_provider_contract::live_stream::{self as wire, attachment as attach};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::custody::{PidNs, RootSlot};
use crate::sys;

pub const SOCKET_FILE: &str = "live.sock";
pub use wire::PROTOCOL;
pub const ORIGIN: &str = "combined-stdout-stderr";
pub(crate) const CHUNK_OVERHEAD: u64 = 64;
const MAX_FRAME: usize = 16 * 1024;
const MAX_SUBSCRIBERS: usize = 8;
// Includes unfinished streams and tombstones; the SDK directory fits all of them.
const MAX_STREAMS: usize = attach::MAX_DIRECTORY_ENTRIES;
const POLL: Duration = Duration::from_millis(50);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
const NS_GET_PARENT: u64 = 0xb702;

/// Preserve arbitrary optional JSON through strict required request parsing.
/// Shape/capability refusal happens only at the view boundary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct LiveOutput(pub Value);

impl LiveOutput {
    pub(crate) fn admit(&self, requester: &str) -> Result<(), String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Agreement {
            grant: String,
            advertisement: Value,
        }
        let a: Agreement = serde_json::from_value(self.0.clone())
            .map_err(|_| "invalid optional live agreement".to_owned())?;
        if a.grant != requester {
            return Err("not this root's attested requester".into());
        }
        let selected = wire::select(&offer(), &a.advertisement).map_err(|e| e.to_string())?;
        descriptor(
            "00000000000000000000000000000000",
            "00000000000000000000000000000000",
            "root",
            1,
            1,
        )
        .agree(&selected)
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn offer() -> wire::Offer {
    wire::Offer {
        channels: vec![wire::Channel::Combined, wire::Channel::Control],
        audiences: vec![wire::Audience::Scoped],
        max_data_bytes: MAX_FRAME as u32,
    }
}

fn descriptor(
    id: &str,
    incarnation: &str,
    root: &str,
    generation: i64,
    work: i64,
) -> wire::Descriptor {
    wire::Descriptor {
        protocol: PROTOCOL.into(),
        stream_id: id.into(),
        incarnation: incarnation.into(),
        channels: offer().channels,
        max_data_bytes: MAX_FRAME as u32,
        visibility: wire::VisibilityClaim {
            audience: wire::Audience::Scoped,
            scope: Some(root.into()),
            channels: offer().channels,
        },
        correlation: Some(wire::Correlation {
            root: Some(root.into()),
            work: Some(work.to_string()),
            generation: Some(generation.to_string()),
            ..Default::default()
        }),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) per_run: u64,
    pub(crate) per_root: u64,
    pub(crate) grace: Duration,
    pub(crate) idle: Duration,
}
pub(crate) const LIMITS: Limits = Limits {
    per_run: 1024 * 1024,
    per_root: 8 * 1024 * 1024,
    grace: Duration::from_secs(30),
    idle: Duration::from_secs(3600),
};

struct Held {
    seq: u64,
    bytes: Vec<u8>,
}
#[derive(Default)]
struct Ring {
    frames: VecDeque<Held>,
    end: u64,
    held: u64,
    evicted_through: u64,
}
impl Ring {
    fn evict(&mut self, capture: &Capture) -> bool {
        let Some(frame) = self.frames.pop_front() else {
            return false;
        };
        let cost = frame.bytes.len() as u64 + CHUNK_OVERHEAD;
        self.held -= cost;
        capture.used.fetch_sub(cost, Ordering::Relaxed);
        capture
            .evicted
            .fetch_add(frame.bytes.len() as u64, Ordering::Relaxed);
        self.evicted_through = frame.seq;
        true
    }
    fn clear(&mut self, capture: &Capture) {
        while self.evict(capture) {}
    }
    fn keep(&mut self, capture: &Capture, seq: u64, bytes: &[u8]) {
        let cost = bytes.len() as u64 + CHUNK_OVERHEAD;
        if cost > capture.limits.per_run {
            return;
        }
        while self.held + cost > capture.limits.per_run {
            self.evict(capture);
        }
        // At most one reservation attempt per own eviction; finite ring length.
        loop {
            let used = capture.used.load(Ordering::Relaxed);
            if used + cost <= capture.limits.per_root
                && capture
                    .used
                    .compare_exchange(used, used + cost, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                break;
            }
            if !self.evict(capture) {
                return;
            }
        }
        self.held += cost;
        self.frames.push_back(Held {
            seq,
            bytes: bytes.to_vec(),
        });
    }
}

struct Finished {
    terminal: wire::Terminal,
    controls: Vec<wire::ControlFrame>,
    at: Instant,
}
struct Stream {
    descriptor: wire::Descriptor,
    next: AtomicU64,
    dropped: AtomicU64,
    ring: Mutex<Ring>,
    finished: OnceLock<Finished>,
}
impl Stream {
    fn terminal_seq(&self) -> Option<u64> {
        self.finished.get().map(|f| match &f.terminal {
            wire::Terminal::Finalized(t) => t.seq,
            wire::Terminal::Ended(t) => t.seq,
        })
    }
}

pub(crate) struct Capture {
    root_id: String,
    generation: i64,
    incarnation: String,
    uid: u32,
    limits: Limits,
    base: Instant,
    last_activity_ms: AtomicU64,
    active: AtomicBool,
    used: AtomicU64,
    streams: Mutex<BTreeMap<i64, Arc<Stream>>>,
    subscribers: AtomicUsize,
    refused: AtomicU64,
    slow: AtomicU64,
    captured: AtomicU64,
    dropped: AtomicU64,
    evicted: AtomicU64,
    closed: AtomicBool,
    path: OnceLock<PathBuf>,
}
impl Capture {
    pub(crate) fn new(
        root_id: String,
        generation: i64,
        uid: u32,
        limits: Limits,
    ) -> std::io::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            root_id,
            generation,
            incarnation: sys::random_hex()?,
            uid,
            limits,
            base: Instant::now(),
            last_activity_ms: AtomicU64::new(0),
            active: AtomicBool::new(false),
            used: AtomicU64::new(0),
            streams: Mutex::new(BTreeMap::new()),
            subscribers: AtomicUsize::new(0),
            refused: AtomicU64::new(0),
            slow: AtomicU64::new(0),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            evicted: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            path: OnceLock::new(),
        }))
    }
    pub(crate) fn incarnation(&self) -> &str {
        &self.incarnation
    }
    fn now_ms(&self) -> u64 {
        self.base
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn touch(&self) {
        self.last_activity_ms
            .store(self.now_ms(), Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }
    fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
            && self
                .now_ms()
                .saturating_sub(self.last_activity_ms.load(Ordering::Relaxed))
                < self.limits.idle.as_millis().try_into().unwrap_or(u64::MAX)
    }
    /// A failed registration disables only this run's view. No sweep, I/O or wait.
    pub(crate) fn register(self: &Arc<Self>, work: i64, _basis: &'static str) -> Option<Tap> {
        let Ok(mut streams) = self.streams.try_lock() else {
            return None;
        };
        if let Some(stream) = streams.get(&work) {
            return Some(Tap {
                capture: Arc::clone(self),
                stream: Arc::clone(stream),
            });
        }
        if streams.len() >= MAX_STREAMS || self.closed.load(Ordering::Relaxed) {
            return None;
        }
        // Root identity is random; this domain-separated opaque work identity
        // survives owner takeover without becoming a Bash handle or authority.
        let id = format!(
            "{:x}",
            Sha256::digest(format!("live-stream:{}:{work}", self.root_id).as_bytes())
        );
        let stream = Arc::new(Stream {
            descriptor: descriptor(
                &id[..32],
                &self.incarnation,
                &self.root_id,
                self.generation,
                work,
            ),
            next: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            ring: Mutex::new(Ring::default()),
            finished: OnceLock::new(),
        });
        streams.insert(work, Arc::clone(&stream));
        Some(Tap {
            capture: Arc::clone(self),
            stream,
        })
    }
    /// Maintenance is viewer-only; required threads never call it.
    fn sweep(&self) {
        let active = self.active();
        if !active {
            self.active.store(false, Ordering::Relaxed);
        }
        let mut streams = self.streams.lock().unwrap();
        streams.retain(|_, stream| {
            let expired = stream
                .finished
                .get()
                .is_some_and(|f| f.at.elapsed() >= self.limits.grace);
            let mut ring = stream.ring.lock().unwrap();
            if expired || !active || self.closed.load(Ordering::Relaxed) {
                ring.clear(self);
            }
            // Do not detach held resources from accounting while a follower owns
            // them. All registered unfinished and finished metadata fits 32 slots.
            !(expired && Arc::strong_count(stream) == 1)
        });
    }
    pub(crate) fn summary(&self) -> Value {
        json!({ "protocol": PROTOCOL, "root_id": self.root_id, "generation": self.generation,
            "incarnation": self.incarnation, "grant": format!("uid:{}", self.uid), "active": self.active(),
            "held": self.used.load(Ordering::Relaxed), "bounds": {"per_run": self.limits.per_run, "per_root": self.limits.per_root,
                "chunk_overhead": CHUNK_OVERHEAD, "streams": MAX_STREAMS, "connections": MAX_SUBSCRIBERS,
                "grace_s": self.limits.grace.as_secs(), "idle_s": self.limits.idle.as_secs(), "meaning": "accounting, not RSS"},
            "bytes": {"captured": self.captured.load(Ordering::Relaxed), "dropped": self.dropped.load(Ordering::Relaxed), "evicted": self.evicted.load(Ordering::Relaxed)},
            "subscribers": {"current": self.subscribers.load(Ordering::Relaxed), "refused": self.refused.load(Ordering::Relaxed), "slow_disconnected": self.slow.load(Ordering::Relaxed)} })
    }
    /// Setup/accept/expiry run separately. The announced path is configured,
    /// not a promise that asynchronous socket setup has already succeeded.
    pub(crate) fn listen(self: &Arc<Self>, slot: Arc<RootSlot>) -> Result<PathBuf, String> {
        let path = slot.workload.ipc_dir.join(SOCKET_FILE);
        if path.as_os_str().len() > crate::SOCKET_PATH_MAX {
            return Err("ipc path too long".into());
        }
        let _ = self.path.set(path.clone());
        let capture = Arc::clone(self);
        thread::Builder::new()
            .name("optional-live".into())
            .spawn(move || {
                let serve = || -> std::io::Result<()> {
                    let path = capture.path.get().unwrap();
                    match std::fs::remove_file(path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                    let listener = UnixListener::bind(path)?;
                    // Bound the kernel pending queue separately from eight workers.
                    // SAFETY: listen updates the backlog of this owned socket.
                    if unsafe { libc::listen(listener.as_raw_fd(), MAX_SUBSCRIBERS as i32) } != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    crate::workload::hand_socket(&slot.workload, path)
                        .map_err(std::io::Error::other)?;
                    listener.set_nonblocking(true)?;
                    while !capture.closed.load(Ordering::Acquire) {
                        capture.sweep();
                        // Bound work per maintenance pass, including rejected accepts.
                        for _ in 0..MAX_SUBSCRIBERS {
                            let conn = match listener.accept() {
                                Ok((c, _)) => c,
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                                Err(e) => return Err(e),
                            };
                            // Count before admission, decoding, or spawning. Full peers
                            // get EOF, without writes or owner report queue traffic.
                            if capture
                                .subscribers
                                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                                    (n < MAX_SUBSCRIBERS).then_some(n + 1)
                                })
                                .is_err()
                            {
                                drop(conn);
                                continue;
                            }
                            let c = Arc::clone(&capture);
                            let s = Arc::clone(&slot);
                            let spawned = thread::Builder::new()
                                .name("optional-viewer".into())
                                .spawn(move || {
                                    c.serve(&conn, &s);
                                    c.subscribers.fetch_sub(1, Ordering::SeqCst);
                                });
                            if spawned.is_err() {
                                capture.subscribers.fetch_sub(1, Ordering::SeqCst);
                            }
                        }
                        thread::sleep(POLL);
                    }
                    Ok(())
                };
                let _ = serve();
                capture.closed.store(true, Ordering::Release);
                if let Some(path) = capture.path.get() {
                    let _ = std::fs::remove_file(path);
                }
                capture.sweep();
            })
            .map_err(|e| e.to_string())?;
        Ok(path)
    }
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    fn refuse(&self, conn: &UnixStream, reason: wire::UnavailableReason, detail: &str) {
        self.refused.fetch_add(1, Ordering::Relaxed);
        let _ = send(
            conn,
            &attach::Message::Unavailable {
                diagnostic: wire::LiveUnavailable::new(reason, detail),
            },
        );
    }
    fn admit(&self, cred: &libc::ucred, slot: &RootSlot) -> Result<(), String> {
        if cred.uid != self.uid {
            return Err("not-granted".into());
        }
        if cred.pid == 0 {
            return Ok(());
        }
        let root = slot.observed_root()?;
        let Some(root) = root else {
            return Ok(());
        };
        let meta = std::fs::metadata(format!("/proc/{}/ns/pid", root.host_pid))
            .map_err(|e| format!("root-namespace-unknown: {e}"))?;
        let root_ns = PidNs {
            dev: meta.dev(),
            ino: meta.ino(),
        };
        let pidfd = sys::pidfd_open(cred.pid).map_err(|e| e.to_string())?;
        let inside = within(cred.pid, &root_ns).map_err(|e| e.to_string())?;
        if sys::pidfd_exited(&pidfd, 0).unwrap_or(true) {
            return Err("peer-gone".into());
        }
        if inside {
            return Err("inside-observed-root".into());
        }
        Ok(())
    }
    fn serve(&self, conn: &UnixStream, slot: &RootSlot) {
        use wire::UnavailableReason as Why;
        if conn.set_nonblocking(true).is_err() {
            return;
        }
        let cred = conn
            .as_fd()
            .try_clone_to_owned()
            .and_then(|fd| sys::peer_cred(&fd));
        match cred {
            Ok(c) => {
                if let Err(e) = self.admit(&c, slot) {
                    self.refuse(conn, Why::NotAuthorized, &e);
                    return;
                }
            }
            Err(_) => {
                self.refuse(conn, Why::NotAuthorized, "peer-unknown");
                return;
            }
        }
        let hello = match read_message(conn) {
            Ok(attach::Message::Hello(h)) => h,
            _ => {
                self.refuse(conn, Why::InvalidRecord, "hello required");
                return;
            }
        };
        if hello.role != attach::Role::Subscriber {
            self.refuse(conn, Why::ProtocolViolation, "subscriber required");
            return;
        }
        let selected = match hello.select(attach::Role::Broker, &offer()) {
            Ok(s) => s,
            Err(diagnostic) => {
                let _ = send(conn, &attach::Message::Unavailable { diagnostic });
                return;
            }
        };
        if send(
            conn,
            &attach::Message::Hello(attach::Hello::new(attach::Role::Broker, &offer())),
        )
        .is_err()
        {
            return;
        }
        let request = match read_message(conn) {
            Ok(r) => r,
            Err(_) => {
                self.refuse(conn, Why::InvalidRecord, "request required");
                return;
            }
        };
        self.touch();
        match request {
            attach::Message::List {} => {
                let streams = self
                    .streams
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|s| s.descriptor.agree(&selected).is_ok())
                    .map(|s| s.descriptor.clone())
                    .collect();
                let _ = send(
                    conn,
                    &attach::Message::Directory(attach::Directory { streams }),
                );
            }
            attach::Message::Attach(request) => {
                let stream = self
                    .streams
                    .lock()
                    .unwrap()
                    .values()
                    .find(|s| s.descriptor.stream_id == request.cursor.stream_id)
                    .cloned();
                let Some(stream) = stream else {
                    self.refuse(conn, Why::UnknownStream, "not registered or expired");
                    return;
                };
                if let Err(diagnostic) = self.follow(conn, &selected, &stream, &request) {
                    let _ = send(conn, &attach::Message::Unavailable { diagnostic });
                }
            }
            _ => self.refuse(conn, Why::ProtocolViolation, "list or attach required"),
        }
    }
    fn follow(
        &self,
        conn: &UnixStream,
        selected: &wire::Selected,
        stream: &Stream,
        request: &attach::Attach,
    ) -> Result<(), wire::LiveUnavailable> {
        let window = {
            let ring = stream.ring.lock().unwrap();
            let terminal = stream.finished.get().map(|f| f.terminal.clone());
            let last = stream
                .terminal_seq()
                .unwrap_or(ring.end.max(stream.dropped.load(Ordering::Acquire)));
            wire::RetainedWindow {
                stream_id: stream.descriptor.stream_id.clone(),
                incarnation: self.incarnation.clone(),
                first_retained: ring
                    .frames
                    .front()
                    .map(|h| h.seq)
                    .into_iter()
                    .chain(
                        stream
                            .finished
                            .get()
                            .and_then(|f| f.controls.first().map(|c| c.seq)),
                    )
                    .chain(stream.terminal_seq())
                    .min()
                    .unwrap_or(last + 1),
                last_published: last,
                terminal,
                previous: None,
            }
        };
        // Kernel admission above is the host decision, never this descriptor.
        let decision = attach::HostDecision::Granted {
            stream_id: stream.descriptor.stream_id.clone(),
            scope: self.root_id.clone(),
        };
        let attached = attach::attach(selected, &stream.descriptor, &window, request, &decision)?;
        let mut at = attached.plan.deliver_from;
        send(conn, &attach::Message::Attached(attached.clone()))
            .map_err(|_| wire::LiveUnavailable::broker_absent())?;
        let already_terminal =
            attached.plan.prefix.iter().any(|record| {
                matches!(record, wire::Record::Finalized(_) | wire::Record::Ended(_))
            });
        // The plan prefix is already delivered in Attached and consumed by
        // SDK follow_attached. Only records from deliver_from follow it.
        if already_terminal {
            return Ok(());
        }
        let mut waiting_since = Instant::now();
        loop {
            if self.closed.load(Ordering::Acquire) || peer_closed(conn) {
                return Ok(());
            }
            let next = {
                let ring = stream.ring.lock().unwrap();
                let end = stream
                    .terminal_seq()
                    .unwrap_or(ring.end.max(stream.dropped.load(Ordering::Acquire)));
                let data = ring.frames.iter().find(|h| h.seq >= at);
                let control = stream
                    .finished
                    .get()
                    .and_then(|f| f.controls.iter().find(|c| c.seq >= at));
                let candidate = data
                    .map(|d| d.seq)
                    .into_iter()
                    .chain(control.map(|c| c.seq))
                    .chain(stream.terminal_seq())
                    .filter(|s| *s >= at)
                    .min();
                if let Some(seq) = candidate {
                    if at < seq {
                        Some(gap(
                            stream,
                            at,
                            seq - 1,
                            if at <= ring.evicted_through {
                                wire::GapReason::Evicted
                            } else {
                                wire::GapReason::CaptureOverflow
                            },
                        ))
                    } else if let Some(c) = control.filter(|c| c.seq == at) {
                        Some(wire::Record::Control(c.clone()))
                    } else if let Some(h) = data.filter(|d| d.seq == at) {
                        Some(wire::Record::Data(wire::DataFrame {
                            stream_id: stream.descriptor.stream_id.clone(),
                            incarnation: self.incarnation.clone(),
                            seq: at,
                            observed_at_unix_ms: now_unix_ms(),
                            channel: wire::DataChannel::Combined,
                            data_base64: crate::bash::base64(&h.bytes),
                        }))
                    } else {
                        Some(stream.finished.get().unwrap().terminal.record())
                    }
                } else if at <= end {
                    Some(gap(
                        stream,
                        at,
                        end,
                        if at <= ring.evicted_through {
                            wire::GapReason::Evicted
                        } else {
                            wire::GapReason::CaptureOverflow
                        },
                    ))
                } else {
                    None
                }
            };
            if let Some(record) = next {
                let terminal =
                    matches!(record, wire::Record::Finalized(_) | wire::Record::Ended(_));
                at = match &record {
                    wire::Record::Gap(g) => g.last + 1,
                    _ => at + 1,
                };
                if send(conn, &attach::Message::Record { record }).is_err() {
                    self.slow.fetch_add(1, Ordering::Relaxed);
                    return Ok(());
                }
                self.touch();
                waiting_since = Instant::now();
                if terminal {
                    return Ok(());
                }
            } else {
                // Polling is not subscriber activity. A quiet connection has
                // a finite idle lifetime and closed peers release slots promptly.
                if waiting_since.elapsed() >= self.limits.idle {
                    return Ok(());
                }
                thread::sleep(POLL);
            }
        }
    }
}
fn gap(stream: &Stream, first: u64, last: u64, reason: wire::GapReason) -> wire::Record {
    wire::Record::Gap(wire::Gap {
        stream_id: stream.descriptor.stream_id.clone(),
        incarnation: stream.descriptor.incarnation.clone(),
        first,
        last,
        reason,
    })
}

pub(crate) struct Tap {
    capture: Arc<Capture>,
    stream: Arc<Stream>,
}
impl Tap {
    pub(crate) fn push(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let seq = self.stream.next.fetch_add(1, Ordering::Relaxed) + 1;
        let Ok(mut ring) = self.stream.ring.try_lock() else {
            self.stream.dropped.fetch_max(seq, Ordering::Release);
            self.capture
                .dropped
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
            return;
        };
        if self.capture.active() && bytes.len() <= MAX_FRAME {
            ring.keep(&self.capture, seq, bytes);
        }
        if ring.frames.back().is_some_and(|h| h.seq == seq) {
            self.capture
                .captured
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        } else {
            self.capture
                .dropped
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        ring.end = seq;
    }
    /// Terminal publication has no ring/registry lock and one producer.
    pub(crate) fn finish(self, result: Value) {
        let id = self.stream.descriptor.stream_id.clone();
        let incarnation = self.capture.incarnation.clone();
        let mut controls = Vec::new();
        let mut fact = |fact| {
            controls.push(wire::ControlFrame {
                stream_id: id.clone(),
                incarnation: incarnation.clone(),
                seq: self.stream.next.fetch_add(1, Ordering::Relaxed) + 1,
                observed_at_unix_ms: now_unix_ms(),
                fact,
            })
        };
        if result["output"]["state"] == "closed" {
            fact(wire::ControlFact::ChannelClosed {
                channel: wire::DataChannel::Combined,
            });
        }
        if let Some(status) = result["status"].as_str() {
            if let Some(code) = status.strip_prefix("code:").and_then(|n| n.parse().ok()) {
                fact(wire::ControlFact::ExitObserved {
                    code: Some(code),
                    signal: None,
                });
            } else if let Some(signal) = status.strip_prefix("signal:").and_then(|n| n.parse().ok())
            {
                fact(wire::ControlFact::ExitObserved {
                    code: None,
                    signal: Some(signal),
                });
            }
        }
        let seq = self.stream.next.fetch_add(1, Ordering::Relaxed) + 1;
        let terminal = if result["event"] == "finalized" {
            wire::Terminal::Finalized(wire::FinalizedFrame {
                stream_id: id,
                incarnation,
                seq,
                observed_at_unix_ms: now_unix_ms(),
                durable_reference: result["durable_reference"].as_str().unwrap().into(),
            })
        } else {
            wire::Terminal::Ended(wire::EndedFrame {
                stream_id: id,
                incarnation,
                seq,
                observed_at_unix_ms: now_unix_ms(),
            })
        };
        let _ = self.stream.finished.set(Finished {
            terminal,
            controls,
            at: Instant::now(),
        });
    }
}

/// The reference names the matching durable work + seal classification, never
/// report delivery, wait success, complete/readable bytes or outside read access.
pub(crate) fn terminal(
    root_id: &str,
    work: i64,
    end: &Value,
    recorded: Result<(), String>,
    sealed: bool,
) -> Value {
    if recorded.is_ok() && sealed && matches!(end["event"].as_str(), Some("end" | "end-unknown")) {
        json!({"event":"finalized", "durable_reference":format!("rv1w:{root_id}:{work}"), "status":end["status"], "output":end["output"]})
    } else {
        json!({"event":"ended", "status":end["status"], "output":end["output"]})
    }
}
fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

// Nonblocking reads/writes use an absolute deadline per whole line. Trickle
// progress cannot reset it. No socket buffering escapes the line-size bound.
fn read_message(mut conn: &UnixStream) -> Result<attach::Message, String> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut line = Vec::new();
    loop {
        if Instant::now() >= deadline {
            return Err("request deadline".into());
        }
        let mut byte = [0];
        match conn.read(&mut byte) {
            Ok(0) => return Err("peer ended".into()),
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
                if line.len() > attach::MAX_MESSAGE_BYTES {
                    return Err("message bound".into());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(_) => return Err("read failed".into()),
        }
    }
    attach::Message::decode_line(std::str::from_utf8(&line).map_err(|_| "utf8")?)
        .map_err(|e| e.to_string())
}
fn send(mut conn: &UnixStream, message: &attach::Message) -> std::io::Result<()> {
    let mut line = message.encode_line();
    line.push('\n');
    if line.len() > attach::MAX_MESSAGE_BYTES + 1 {
        return Err(std::io::Error::other("message bound"));
    }
    let deadline = Instant::now() + WRITE_TIMEOUT;
    let mut bytes = line.as_bytes();
    while !bytes.is_empty() {
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "line deadline",
            ));
        }
        match conn.write(bytes) {
            Ok(0) => return Err(std::io::Error::other("peer ended")),
            Ok(n) => bytes = &bytes[n..],
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
fn peer_closed(conn: &UnixStream) -> bool {
    let mut byte = [0u8];
    // SAFETY: valid fd and one-byte buffer. Peek never consumes a request.
    let n = unsafe {
        libc::recv(
            conn.as_raw_fd(),
            byte.as_mut_ptr().cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    n == 0
        || n > 0
        || (n < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock)
}
fn within(pid: i32, root: &PidNs) -> std::io::Result<bool> {
    let mut ns: OwnedFd = File::open(format!("/proc/{pid}/ns/pid"))?.into();
    // PID namespaces nest at most 32 deep.
    for _ in 0..40 {
        let meta = File::from(ns.try_clone()?).metadata()?;
        if meta.dev() == root.dev && meta.ino() == root.ino {
            return Ok(true);
        }
        // SAFETY: NS_GET_PARENT on a namespace fd returns a new fd or -1.
        let parent = unsafe { libc::ioctl(ns.as_raw_fd(), NS_GET_PARENT as _) };
        if parent < 0 {
            let error = std::io::Error::last_os_error();
            // No parent this owner can name: the walk left every namespace
            // the root's can be nested in.
            return if error.raw_os_error() == Some(libc::EPERM) {
                Ok(false)
            } else {
                Err(error)
            };
        }
        // SAFETY: the ioctl returned a new descriptor we now own.
        ns = unsafe { OwnedFd::from_raw_fd(parent) };
    }
    Err(std::io::Error::other("namespace nesting too deep"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn capture(limits: Limits) -> Arc<Capture> {
        Capture::new(
            "12345678901234567890123456789012".into(),
            1,
            unsafe { libc::geteuid() },
            limits,
        )
        .unwrap()
    }
    fn selected() -> wire::Selected {
        wire::select(&offer(), &wire::advertisement(&offer())).unwrap()
    }

    #[test]
    fn registration_drain_terminal_and_summary_do_not_wait_for_viewer_locks() {
        let c = capture(LIMITS);
        c.touch();
        let tap = c.register(1, "from-launch").unwrap();
        let stream = Arc::clone(&tap.stream);
        let ring = stream.ring.lock().unwrap();
        let registry = c.streams.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let owner = Arc::clone(&c);
        let thread = thread::spawn(move || {
            assert!(owner.register(2, "from-launch").is_none());
            tap.push(b"lost");
            tap.finish(json!({"event":"ended"}));
            let summary = owner.summary();
            owner.close();
            tx.send(summary).unwrap();
        });
        let summary = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("optional consumer locks cannot hold required operations");
        assert_eq!(summary["bytes"]["dropped"], 4);
        assert_eq!(stream.dropped.load(Ordering::Acquire), 1);
        assert_eq!(stream.terminal_seq(), Some(2));
        drop(registry);
        drop(ring);
        thread.join().unwrap();
    }
    #[test]
    fn gaps_bounds_and_terminal_survive_ring_contention() {
        let c = capture(Limits {
            per_run: 132,
            per_root: 264,
            ..LIMITS
        });
        c.touch();
        let a = c.register(1, "from-launch").unwrap();
        let b = c.register(2, "from-launch").unwrap();
        b.push(b"bbbb");
        a.push(b"aaaa");
        a.push(b"cccc");
        a.push(b"dddd");
        assert!(c.used.load(Ordering::Relaxed) <= 264);
        assert_eq!(b.stream.ring.lock().unwrap().frames[0].bytes, b"bbbb");
        let stream = Arc::clone(&a.stream);
        let ring = stream.ring.lock().unwrap();
        a.push(b"eeee");
        a.finish(json!({"event":"ended"}));
        drop(ring);
        assert_eq!(stream.dropped.load(Ordering::Acquire), 4);
        assert_eq!(stream.terminal_seq(), Some(5));
        let (server, client) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let window = wire::RetainedWindow {
            stream_id: stream.descriptor.stream_id.clone(),
            incarnation: c.incarnation.clone(),
            first_retained: 2,
            last_published: 5,
            terminal: stream.finished.get().map(|f| f.terminal.clone()),
            previous: None,
        };
        assert!(
            wire::plan_replay(&window, &stream.descriptor.start())
                .unwrap()
                .prefix
                .iter()
                .any(|r| matches!(r,wire::Record::Gap(g) if g.first==1&&g.last==1))
        );
        // Real SDK follower follows an overflow gap and ends; no final reference.
        let mut f = wire::Follower::from_descriptor(
            &selected(),
            &stream.descriptor,
            stream.descriptor.start(),
        )
        .unwrap();
        f.accept(&gap(&stream, 1, 4, wire::GapReason::CaptureOverflow))
            .unwrap();
        f.accept(&stream.finished.get().unwrap().terminal.record())
            .unwrap();
        assert!(matches!(
            f.cursor().terminal,
            Some(wire::Terminal::Ended(_))
        ));
    }
    #[test]
    fn malformed_optional_agreements_are_view_only_and_capabilities_are_selected() {
        let grant = "uid:7";
        for raw in [
            json!(false),
            json!([]),
            json!({"grant":grant}),
            json!({"grant":grant,"advertisement":{"oulipoly.live_stream/v2":{}}}),
            json!({"grant":grant,"advertisement":{"oulipoly.live_stream/v3":17}}),
        ] {
            let live: LiveOutput = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(live.0, raw);
            assert!(live.admit(grant).is_err());
        }
        let valid =
            LiveOutput(json!({"grant":grant,"advertisement":wire::advertisement(&offer())}));
        assert!(valid.admit(grant).is_ok());
        assert!(valid.admit("uid:8").is_err());
        let narrow = wire::Offer {
            channels: vec![wire::Channel::Stdout],
            ..offer()
        };
        assert!(
            LiveOutput(json!({"grant":grant,"advertisement":wire::advertisement(&narrow)}))
                .admit(grant)
                .is_err()
        );
    }
    #[test]
    fn quiet_disconnect_and_idle_do_not_renew_capture_and_grace_releases_bytes() {
        let c = capture(Limits {
            idle: Duration::from_millis(100),
            grace: Duration::from_millis(10),
            ..LIMITS
        });
        c.touch();
        let tap = c.register(1, "from-launch").unwrap();
        tap.push(b"kept");
        let stream = Arc::clone(&tap.stream);
        let (server, mut client) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let owner = Arc::clone(&c);
        let s = Arc::clone(&stream);
        let request = attach::Attach {
            cursor: stream.descriptor.start(),
        };
        let thread =
            thread::spawn(move || owner.follow(&server, &selected(), &s, &request).unwrap());
        // Read attached and data, then close a now-quiet follower.
        let mut reader = std::io::BufReader::new(&mut client);
        let mut line = String::new();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        line.clear();
        std::io::BufRead::read_line(&mut reader, &mut line).unwrap();
        drop(reader);
        drop(client);
        thread.join().unwrap();
        let last = c.last_activity_ms.load(Ordering::Relaxed);
        thread::sleep(Duration::from_millis(120));
        assert!(!c.active());
        assert_eq!(c.last_activity_ms.load(Ordering::Relaxed), last);
        c.sweep();
        assert_eq!(c.used.load(Ordering::Relaxed), 0);
        c.touch();
        tap.push(b"tail");
        tap.finish(json!({"event":"ended"}));
        thread::sleep(Duration::from_millis(20));
        c.sweep();
        assert_eq!(c.used.load(Ordering::Relaxed), 0);
        assert!(matches!(
            stream.finished.get().unwrap().terminal,
            wire::Terminal::Ended(_)
        ));
    }
    #[test]
    fn unknown_wait_and_partial_record_do_not_invent_an_exit_fact() {
        let c = capture(LIMITS);
        let tap = c.register(1, "from-launch").unwrap();
        let stream = Arc::clone(&tap.stream);
        let end = json!({"event":"end-unknown","retained":{"state":"unsealed"},"reason":"ended-with-work-namespace-status-unknown"});
        tap.finish(terminal(&c.root_id, 1, &end, Ok(()), true));
        let finished = stream.finished.get().unwrap();
        assert!(matches!(finished.terminal, wire::Terminal::Finalized(_)));
        assert!(finished.controls.is_empty());
        let other = c.register(2, "from-launch").unwrap();
        let s = Arc::clone(&other.stream);
        other.finish(terminal(
            &c.root_id,
            2,
            &end,
            Err("store failure".into()),
            true,
        ));
        assert!(matches!(
            s.finished.get().unwrap().terminal,
            wire::Terminal::Ended(_)
        ));
    }
    #[test]
    fn namespace_walk_finds_self_and_stops_at_the_top() {
        let pid = std::process::id() as i32;
        let meta = std::fs::metadata(format!("/proc/{pid}/ns/pid")).unwrap();
        let own = PidNs {
            dev: meta.dev(),
            ino: meta.ino(),
        };
        assert!(within(pid, &own).unwrap());
        assert!(
            !within(
                pid,
                &PidNs {
                    dev: own.dev,
                    ino: own.ino.wrapping_add(1)
                }
            )
            .unwrap()
        );
    }
    #[test]
    fn final_cursor_replay_returns_without_holding_a_quiet_connection() {
        let c = capture(LIMITS);
        let tap = c.register(1, "from-launch").unwrap();
        let stream = Arc::clone(&tap.stream);
        tap.finish(json!({"event":"ended"}));
        let terminal = stream.finished.get().unwrap().terminal.clone();
        let mut cursor = stream.descriptor.start();
        cursor.after_seq = 1;
        cursor.terminal = Some(terminal);
        let (server, client) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        let owner = Arc::clone(&c);
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = thread::spawn(move || {
            owner
                .follow(&server, &selected(), &stream, &attach::Attach { cursor })
                .unwrap();
            tx.send(()).unwrap();
        });
        rx.recv_timeout(Duration::from_secs(1))
            .expect("at-final replay must finish without viewer EOF or idle timeout");
        thread.join().unwrap();
        drop(client);
    }
    #[test]
    fn a_nonreader_write_has_a_whole_line_deadline() {
        let (server, _client) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        let size: libc::c_int = 1024;
        // SAFETY: set a socket option on this owned test descriptor.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    server.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        let record = wire::Record::Data(wire::DataFrame {
            stream_id: "1".repeat(32),
            incarnation: "2".repeat(32),
            seq: 1,
            observed_at_unix_ms: 0,
            channel: wire::DataChannel::Combined,
            data_base64: crate::bash::base64(&vec![0; MAX_FRAME]),
        });
        let now = Instant::now();
        let error = send(&server, &attach::Message::Record { record }).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(now.elapsed() < WRITE_TIMEOUT + Duration::from_millis(500));
    }
}
