//! Optional live view of this root's Bash output: an explicitly granted
//! observer plane beside the required relay and retention, which it never
//! changes, delays or fails.
//!
//! # What is captured, and where
//!
//! The owner's own relay of each Bash run's combined output (stderr joined
//! to stdout by its work PID 1; nothing here separates them) hands every
//! chunk, after the required retention write and before the in-band send, to
//! this run's capture. That hand-off never waits: it tries the run's ring
//! lock once and, if a subscriber holds it, drops the chunk and later
//! records the exact range as a `capture-contention` loss. No I/O and no
//! subscriber state is reached from the relay. Retention, the requester's
//! in-band output, the `end`, the durable work record and completion
//! delivery are exactly what they are without this plane.
//!
//! # Grant and admission
//!
//! There is no plane unless the owner's request carries `live_output`
//! naming whom it grants the view (`{"grant":"uid:<n>"}`). Only the root's
//! attested requester (the control face's `uid:<n>`, the uid its work runs
//! as) can be granted; any other grant disables the plane, never the root.
//! Nothing a subscriber says grants anything. A connection to `live.sock`
//! (in the root's IPC directory, handed to that uid `0600`) is admitted only
//! when the kernel's `SO_PEERCRED` uid, translated into this owner's user
//! namespace, is the granted uid **and** the peer is outside this root's PID
//! namespace tree: a pid the kernel cannot translate into this owner's PID
//! namespace (`0`) is outside it; a visible pid is pinned with a pidfd and
//! its PID namespace's ancestry is walked (`NS_GET_PARENT`) and must not
//! reach the current root PID 1's namespace. Work inside the root (harness,
//! Bash, children, anything nested) is refused (`inside-observed-root`),
//! so the plane is no path between harnesses; their own runs stay theirs
//! through the Bash ingress. With no current root PID 1 (none started yet, or
//! it ended with its namespace) nothing of the root is alive to exclude.
//!
//! # Identity, cursor and loss
//!
//! A stream is one Bash run (`work`) of this root, observed by this owner
//! process. This owner's capture `incarnation` is 128 random bits minted at
//! start; its store `generation` is the owner fence it holds. An offset is a
//! byte position in what **this incarnation** received of that run's
//! combined output: from the run's launch (`basis` `from-launch`) or from
//! this owner's takeover of a run an earlier owner was relaying
//! (`from-takeover`, whose zero is wherever that owner stopped reading). A
//! cursor is `{incarnation, offset}`; one of another incarnation is
//! answered with a `discontinuity` whose earlier tail is unknown, then
//! delivery from this incarnation's zero. An offset ahead of what was
//! received is refused (`cursor-ahead`); a fabricated offset that is not
//! ahead cannot be told from a genuine one.
//!
//! Every offset below a stream's published end is either held or covered
//! by a `gap {from, to, reasons}`: `not-captured` (no subscriber had asked
//! for the plane yet, or it was reaped idle), `evicted` (ring bound),
//! `capture-overflow` (root bound with nothing of this run left to evict),
//! `capture-contention`, `reaped-idle`, `evicted-after-final`.
//!
//! # Bounds and lifetime
//!
//! Rings hold at most [`Limits::per_run`] per run and [`Limits::per_root`]
//! across this owner, charging each chunk's payload plus a fixed
//! [`CHUNK_OVERHEAD`]: accounting, not an RSS ceiling. Holding starts only
//! when a granted subscriber asks (on demand) and stops after
//! [`Limits::idle`] with no subscriber activity (held bytes are released,
//! later ones only counted). A finished stream's bytes are released
//! [`Limits::grace`] after its terminal record; its terminal metadata stays
//! as a bounded tombstone ([`MAX_TOMBSTONES`]). Nothing is written to disk:
//! no stream log or event database exists, and everything ends with this
//! owner process. Expiry is checked lazily at the next capture or
//! subscriber operation; there is no timer thread.
//!
//! # Terminal records
//!
//! `final` (`finalization` `custody-owner`) is sent only after this owner
//! committed the run's end to the root's store from its work PID 1's actual
//! report (`end`, or `end-unknown` when that PID 1 reported without the
//! command's wait) **and** committed its output seal (`complete`, `partial`
//! or `unsealed`), and after reporting its `bash-ended`. It names the
//! durable work reference `rv1w:<root>:<work>` and, when sealed, the
//! retained identity. Anything else is `ended` with `finalization` `none`
//! and why: root PID 1 lost, left to a successor, or the durable record not
//! committed. Pipe closure (`output-ended` state) is never a terminal.
//!
//! # Not here
//!
//! This is a per-owner endpoint, not the account-local broker: no
//! cross-root multiplexing, election, broker restart/update, A/B drain or
//! SDK `oulipoly.live_stream/v2` wire (its channels cannot name combined
//! output without relabelling it). Retained bytes past eviction are read
//! only through the Bash ingress by their requesting harness.

use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Event;
use crate::custody::{PidNs, RootSlot};
use crate::live::Custody;
use crate::sys;

/// The endpoint's name in the root's IPC directory.
pub const SOCKET_FILE: &str = "live.sock";
/// This plane's wire identity.
pub const PROTOCOL: &str = "oulipoly.root_bash_live/v1";
/// What a stream's bytes are.
pub const ORIGIN: &str = "combined-stdout-stderr";
/// Fixed charge per held chunk, besides its payload.
pub(crate) const CHUNK_OVERHEAD: u64 = 64;
/// Finished streams whose terminal metadata is kept after their bytes.
pub(crate) const MAX_TOMBSTONES: usize = 256;
/// Largest `data` payload sent at once.
const MAX_FRAME: usize = 16 * 1024;
const MAX_SUBSCRIBERS: usize = 8;
const MAX_REQUEST: u64 = 4096;
const MAX_LOSSES: usize = 32;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A subscriber that cannot take one line within this is disconnected.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// How often a waiting subscriber rechecks the owner's state.
const POLL: Duration = Duration::from_millis(250);
/// `_IO(0xb7, 0x2)`: the parent of a PID or user namespace.
const NS_GET_PARENT: u64 = 0xb702;

/// The owner request's `live_output`: whom this owner grants the view.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveOutput {
    /// The granted requester, as the root control face names it
    /// (`uid:<n>`). Only the root's attested requester can be granted.
    pub grant: String,
}

impl LiveOutput {
    /// Admits the grant only for exactly the root's attested requester.
    pub(crate) fn admit(&self, requester: &str) -> Result<(), String> {
        if self.grant == requester {
            Ok(())
        } else {
            Err("grant-refused: not this root's attested requester".to_owned())
        }
    }
}

/// Capture bounds.
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

struct Loss {
    from: u64,
    to: u64,
    reason: &'static str,
}

#[derive(Default)]
struct Ring {
    chunks: VecDeque<(u64, Vec<u8>)>,
    /// Every offset below is held or lost.
    end: u64,
    /// Accounted bytes held.
    held: u64,
    losses: VecDeque<Loss>,
    /// The relay's own output state once its stream ended.
    output: Option<Value>,
    terminal: Option<Value>,
    finished: Option<Instant>,
    evicted: bool,
}

impl Ring {
    fn lose(&mut self, from: u64, to: u64, reason: &'static str) {
        if from >= to {
            return;
        }
        if let Some(last) = self.losses.back_mut()
            && last.to == from
            && last.reason == reason
        {
            last.to = to;
            return;
        }
        self.losses.push_back(Loss { from, to, reason });
        if self.losses.len() > MAX_LOSSES {
            // Reasons only: gap ranges come from what is held.
            let first = self.losses.pop_front().expect("loss");
            let second = self.losses.front_mut().expect("loss");
            second.from = first.from;
            if second.reason != first.reason {
                second.reason = "mixed";
            }
        }
    }

    fn reasons(&self, from: u64, to: u64) -> Vec<&'static str> {
        let mut reasons: Vec<&'static str> = Vec::new();
        for loss in &self.losses {
            if loss.from < to && from < loss.to && !reasons.contains(&loss.reason) {
                reasons.push(loss.reason);
            }
        }
        reasons
    }

    fn evict_oldest(&mut self, capture: &Capture, reason: &'static str) -> bool {
        let Some((at, bytes)) = self.chunks.pop_front() else {
            return false;
        };
        let len = bytes.len() as u64;
        self.release(capture, len + CHUNK_OVERHEAD);
        capture.counters.evicted.fetch_add(len, Ordering::Relaxed);
        self.lose(at, at + len, reason);
        true
    }

    fn release_all(&mut self, capture: &Capture, reason: &'static str) {
        while self.evict_oldest(capture, reason) {}
    }

    fn release(&mut self, capture: &Capture, cost: u64) {
        self.held -= cost;
        capture.used.fetch_sub(cost, Ordering::Relaxed);
    }

    fn keep(&mut self, capture: &Capture, at: u64, bytes: &[u8]) {
        let len = bytes.len() as u64;
        let cost = len + CHUNK_OVERHEAD;
        if cost > capture.limits.per_run {
            capture.counters.overflow.fetch_add(len, Ordering::Relaxed);
            return self.lose(at, at + len, "capture-overflow");
        }
        while self.held + cost > capture.limits.per_run {
            self.evict_oldest(capture, "evicted");
        }
        while !capture.reserve(cost) {
            if !self.evict_oldest(capture, "evicted") {
                capture.counters.overflow.fetch_add(len, Ordering::Relaxed);
                return self.lose(at, at + len, "capture-overflow");
            }
        }
        self.held += cost;
        self.chunks.push_back((at, bytes.to_vec()));
        capture.counters.captured.fetch_add(len, Ordering::Relaxed);
    }

    /// Accounts for chunks the relay received but could not hand over.
    fn reconcile(&mut self, observed: u64) {
        if self.end < observed {
            self.lose(self.end, observed, "capture-contention");
            self.end = observed;
        }
    }

    fn state(&self) -> &'static str {
        match (&self.terminal, &self.output, self.evicted) {
            (Some(_), _, true) => "evicted",
            (Some(terminal), _, false) => {
                if terminal["event"] == "final" {
                    "final"
                } else {
                    "ended"
                }
            }
            (None, Some(_), _) => "output-ended",
            (None, None, _) => "open",
        }
    }

    /// What a subscriber at `cursor` gets next.
    fn next(&self, cursor: u64) -> Next {
        if cursor > self.end {
            return Next::Ahead;
        }
        for (at, bytes) in &self.chunks {
            let stop = at + bytes.len() as u64;
            if stop <= cursor {
                continue;
            }
            if *at > cursor {
                return Next::Gap(cursor, *at, self.reasons(cursor, *at));
            }
            let skip = usize::try_from(cursor - at).expect("chunk offset");
            let take = (bytes.len() - skip).min(MAX_FRAME);
            return Next::Data(bytes[skip..skip + take].to_vec());
        }
        if cursor < self.end {
            return Next::Gap(cursor, self.end, self.reasons(cursor, self.end));
        }
        match &self.terminal {
            Some(terminal) => Next::Terminal(terminal.clone()),
            None => Next::Wait,
        }
    }
}

enum Next {
    Ahead,
    Gap(u64, u64, Vec<&'static str>),
    Data(Vec<u8>),
    Terminal(Value),
    Wait,
}

/// One Bash run's stream in this incarnation.
pub(crate) struct Stream {
    work: i64,
    basis: &'static str,
    /// What the relay received; written only by the relay's thread.
    observed: AtomicU64,
    /// End of the last chunk the relay dropped for contention: everything
    /// below it that is not held is lost, so a waiting subscriber can
    /// record it without the relay's next hand-off.
    dropped: AtomicU64,
    ring: Mutex<Ring>,
    changed: Condvar,
}

impl Stream {
    fn ring(&self) -> MutexGuard<'_, Ring> {
        self.ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn describe(&self, ring: &Ring) -> Value {
        json!({
            "work": self.work,
            "basis": self.basis,
            "origin": ORIGIN,
            "state": ring.state(),
            "first_held": ring.chunks.front().map_or(ring.end, |(at, _)| *at),
            "end": ring.end,
            "terminal": ring.terminal,
        })
    }
}

#[derive(Default)]
struct Counters {
    captured: AtomicU64,
    not_captured: AtomicU64,
    overflow: AtomicU64,
    contention: AtomicU64,
    evicted: AtomicU64,
    served: AtomicU64,
    refused: AtomicU64,
    slow: AtomicU64,
}

/// This owner's capture of its root's Bash output, and its endpoint.
pub(crate) struct Capture {
    root_id: String,
    generation: i64,
    incarnation: String,
    grant: String,
    uid: u32,
    limits: Limits,
    base: Instant,
    used: AtomicU64,
    active: AtomicBool,
    last_activity_ms: AtomicU64,
    streams: Mutex<BTreeMap<i64, Arc<Stream>>>,
    subscribers: AtomicUsize,
    counters: Counters,
    closed: AtomicBool,
    path: Mutex<Option<PathBuf>>,
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
            grant: format!("uid:{uid}"),
            uid,
            limits,
            base: Instant::now(),
            used: AtomicU64::new(0),
            active: AtomicBool::new(false),
            last_activity_ms: AtomicU64::new(0),
            streams: Mutex::new(BTreeMap::new()),
            subscribers: AtomicUsize::new(0),
            counters: Counters::default(),
            closed: AtomicBool::new(false),
            path: Mutex::new(None),
        }))
    }

    pub(crate) fn incarnation(&self) -> &str {
        &self.incarnation
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// A granted subscriber's activity: holding starts or continues.
    fn touch(&self) {
        self.last_activity_ms
            .store(self.now_ms(), Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }

    /// Whether bytes are held now; reaps the plane after its idle period.
    fn active(&self) -> bool {
        if !self.active.load(Ordering::Relaxed) {
            return false;
        }
        let idle = u64::try_from(self.limits.idle.as_millis()).unwrap_or(u64::MAX);
        let since = self
            .now_ms()
            .saturating_sub(self.last_activity_ms.load(Ordering::Relaxed));
        if since >= idle {
            self.active.store(false, Ordering::Relaxed);
            return false;
        }
        true
    }

    fn reserve(&self, cost: u64) -> bool {
        self.used
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used + cost <= self.limits.per_root).then_some(used + cost)
            })
            .is_ok()
    }

    /// One run's stream, started at its durable acceptance (or this
    /// owner's takeover) so it can be attached as soon as `accepted` is
    /// reported; the relay's later hand-off continues the same stream.
    pub(crate) fn register(self: &Arc<Self>, work: i64, basis: &'static str) -> Tap {
        let mut streams = self.streams.lock().expect("live streams");
        let stream = Arc::clone(streams.entry(work).or_insert_with(|| {
            Arc::new(Stream {
                work,
                basis,
                observed: AtomicU64::new(0),
                dropped: AtomicU64::new(0),
                ring: Mutex::new(Ring::default()),
                changed: Condvar::new(),
            })
        }));
        drop(streams);
        self.sweep();
        Tap {
            capture: Arc::clone(self),
            stream,
        }
    }

    /// Releases what has expired: finished streams past their grace, and
    /// every held byte once the plane is idle. Bounds tombstones.
    fn sweep(&self) {
        let active = self.active();
        let mut streams = self.streams.lock().expect("live streams");
        let mut tombstones = Vec::new();
        for (work, stream) in streams.iter() {
            let mut ring = stream.ring();
            let expired = ring
                .finished
                .is_some_and(|finished| finished.elapsed() >= self.limits.grace);
            if expired && !ring.evicted {
                ring.release_all(self, "evicted-after-final");
                ring.evicted = true;
            } else if !active {
                ring.release_all(self, "reaped-idle");
            }
            if ring.evicted {
                tombstones.push(*work);
            }
        }
        let excess = tombstones.len().saturating_sub(MAX_TOMBSTONES);
        for work in tombstones.into_iter().take(excess) {
            streams.remove(&work);
        }
    }

    pub(crate) fn summary(&self) -> Value {
        let count = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        json!({
            "protocol": PROTOCOL,
            "root_id": self.root_id,
            "generation": self.generation,
            "incarnation": self.incarnation,
            "grant": self.grant,
            "active": self.active(),
            "streams": self.streams.lock().expect("live streams").len(),
            "held": self.used.load(Ordering::Relaxed),
            "bounds": {
                "per_run": self.limits.per_run,
                "per_root": self.limits.per_root,
                "chunk_overhead": CHUNK_OVERHEAD,
                "grace_s": self.limits.grace.as_secs(),
                "idle_s": self.limits.idle.as_secs(),
                "meaning": "held payload plus a fixed per-chunk charge; accounting, not an RSS ceiling",
            },
            "bytes": {
                "captured": count(&self.counters.captured),
                "not_captured": count(&self.counters.not_captured),
                "overflow": count(&self.counters.overflow),
                "contention": count(&self.counters.contention),
                "evicted": count(&self.counters.evicted),
            },
            "subscribers": {
                "served": count(&self.counters.served),
                "refused": count(&self.counters.refused),
                "slow_disconnected": count(&self.counters.slow),
            },
        })
    }

    /// Binds `live.sock` in the root's IPC directory, hands it to the
    /// granted uid and serves it on a thread.
    pub(crate) fn listen(
        self: &Arc<Self>,
        slot: Arc<RootSlot>,
        custody: Arc<Mutex<Custody>>,
        tx: Sender<Event>,
    ) -> Result<PathBuf, String> {
        let path = slot.workload.ipc_dir.join(SOCKET_FILE);
        if path.as_os_str().len() > crate::SOCKET_PATH_MAX {
            return Err("ipc path too long for the live endpoint".to_owned());
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("stale socket: {error}")),
        }
        let listener = UnixListener::bind(&path).map_err(|error| error.to_string())?;
        crate::workload::hand_socket(&slot.workload, &path)?;
        *self.path.lock().expect("live path") = Some(path.clone());
        let capture = Arc::clone(self);
        thread::spawn(move || {
            for conn in listener.incoming() {
                if capture.closed.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(conn) = conn else { continue };
                let capture = Arc::clone(&capture);
                let slot = Arc::clone(&slot);
                let custody = Arc::clone(&custody);
                let tx = tx.clone();
                thread::spawn(move || capture.serve(&conn, &slot, &custody, &tx));
            }
        });
        Ok(path)
    }

    /// No later subscriber is served; waiting ones are told.
    pub(crate) fn close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(path) = self.path.lock().expect("live path").take() {
            let _ = UnixStream::connect(&path);
            let _ = std::fs::remove_file(&path);
        }
    }

    fn refuse(&self, conn: &UnixStream, tx: &Sender<Event>, peer: Value, reason: &str) {
        self.counters.refused.fetch_add(1, Ordering::Relaxed);
        let _ = send(conn, &json!({ "event": "unavailable", "reason": reason }));
        // Consume an unread request so closing does not reset the refusal.
        let _ = conn.shutdown(std::net::Shutdown::Write);
        let _ = conn.set_read_timeout(Some(Duration::from_millis(100)));
        let _ = std::io::copy(&mut conn.take(MAX_REQUEST), &mut std::io::sink());
        let _ = tx.send(Event::Report(
            json!({ "event": "live-output-refused", "reason": reason, "peer": peer }),
        ));
    }

    fn serve(
        &self,
        conn: &UnixStream,
        slot: &RootSlot,
        custody: &Mutex<Custody>,
        tx: &Sender<Event>,
    ) {
        let cred = match conn
            .as_fd()
            .try_clone_to_owned()
            .and_then(|fd| sys::peer_cred(&fd))
        {
            Ok(cred) => cred,
            Err(error) => {
                return self.refuse(conn, tx, Value::Null, &format!("peer-unknown: {error}"));
            }
        };
        let peer = json!({ "pid": cred.pid, "uid": cred.uid });
        if let Err(reason) = self.admit(&cred, slot) {
            return self.refuse(conn, tx, peer, &reason);
        }
        if let Some(reason) = custody.lock().expect("custody lock").reason() {
            return self.refuse(conn, tx, peer, &format!("owner-stopping: {reason}"));
        }
        if self
            .subscribers
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                (count < MAX_SUBSCRIBERS).then_some(count + 1)
            })
            .is_err()
        {
            return self.refuse(conn, tx, peer, "subscriber-bound");
        }
        match read_request(conn) {
            Ok(request) => {
                self.touch();
                self.sweep();
                self.counters.served.fetch_add(1, Ordering::Relaxed);
                match request {
                    Request::List => {
                        let _ = send(conn, &self.list());
                    }
                    Request::Attach { work, cursor } => {
                        if let Err(reason) = self.follow(conn, work, cursor) {
                            self.refuse(conn, tx, peer, reason);
                        }
                    }
                }
            }
            Err(reason) => self.refuse(conn, tx, peer, &reason),
        }
        self.subscribers.fetch_sub(1, Ordering::SeqCst);
    }

    /// The granted uid, from outside this root's PID namespace tree.
    fn admit(&self, cred: &libc::ucred, slot: &RootSlot) -> Result<(), String> {
        if cred.uid != self.uid {
            return Err("not-granted".to_owned());
        }
        if cred.pid == 0 {
            // The kernel could not name the peer in this owner's PID
            // namespace: it is outside it, and so outside the root's.
            return Ok(());
        }
        // This endpoint exists only for an owner that attached or started
        // its root (an owned-unattached one ends before it). With no current
        // root PID 1, none was started yet or it ended, and its namespace
        // with it: nothing of this root is alive to be inside.
        let Some(root) = slot.current() else {
            return Ok(());
        };
        // A live root PID 1's pid is its own; a gone one leaves no work
        // inside its namespace to exclude.
        let root_ns = std::fs::metadata(format!("/proc/{}/ns/pid", root.host_pid))
            .map(|meta| PidNs {
                dev: meta.dev(),
                ino: meta.ino(),
            })
            .map_err(|error| format!("root-namespace-unknown: {error}"))?;
        let pidfd = sys::pidfd_open(cred.pid).map_err(|error| format!("peer-unknown: {error}"))?;
        let inside = within(cred.pid, &root_ns)
            .map_err(|error| format!("peer-namespace-unknown: {error}"))?;
        if sys::pidfd_exited(&pidfd, 0).unwrap_or(true) {
            return Err("peer-gone".to_owned());
        }
        if inside {
            return Err("inside-observed-root".to_owned());
        }
        Ok(())
    }

    fn list(&self) -> Value {
        let streams: Vec<Value> = self
            .streams
            .lock()
            .expect("live streams")
            .values()
            .map(|stream| stream.describe(&stream.ring()))
            .collect();
        json!({
            "event": "streams",
            "protocol": PROTOCOL,
            "root_id": self.root_id,
            "generation": self.generation,
            "incarnation": self.incarnation,
            "streams": streams,
        })
    }

    /// Delivers one stream from a cursor until its terminal record, the
    /// subscriber falls behind its write bound, or this owner ends.
    fn follow(
        &self,
        conn: &UnixStream,
        work: i64,
        cursor: Option<Cursor>,
    ) -> Result<(), &'static str> {
        let stream = self
            .streams
            .lock()
            .expect("live streams")
            .get(&work)
            .cloned()
            .ok_or("unknown-work")?;
        let mut at = 0;
        let mut discontinuity = None;
        if let Some(cursor) = cursor {
            if cursor.incarnation == self.incarnation {
                at = cursor.offset;
            } else {
                discontinuity = Some(json!({
                    "event": "discontinuity",
                    "previous_incarnation": cursor.incarnation,
                    "previous_offset": cursor.offset,
                    "lost": "unknown",
                    "resume_offset": 0,
                    "meaning": "the cursor names another capture incarnation; delivery restarts at this incarnation's zero",
                }));
            }
        }
        let attached = {
            let ring = stream.ring();
            if at > ring.end {
                return Err("cursor-ahead");
            }
            let mut attached = stream.describe(&ring);
            attached["event"] = json!("attached");
            attached["protocol"] = json!(PROTOCOL);
            attached["root_id"] = json!(self.root_id);
            attached["generation"] = json!(self.generation);
            attached["incarnation"] = json!(self.incarnation);
            attached["offset"] = json!(at);
            attached
        };
        let _ = conn.set_write_timeout(Some(WRITE_TIMEOUT));
        let deliver = |value: &Value| -> bool {
            if send(conn, value).is_ok() {
                return true;
            }
            self.counters.slow.fetch_add(1, Ordering::Relaxed);
            false
        };
        if !deliver(&attached) || discontinuity.as_ref().is_some_and(|line| !deliver(line)) {
            return Ok(());
        }
        loop {
            if self.closed.load(Ordering::SeqCst) {
                let _ = send(
                    conn,
                    &json!({ "event": "unavailable", "reason": "owner-ended" }),
                );
                return Ok(());
            }
            self.touch();
            let next = {
                let mut ring = stream.ring();
                ring.reconcile(stream.dropped.load(Ordering::Relaxed));
                match ring.next(at) {
                    Next::Wait => {
                        let _ = stream.changed.wait_timeout(ring, POLL);
                        continue;
                    }
                    next => next,
                }
            };
            let line = match next {
                Next::Ahead => return Err("cursor-ahead"),
                Next::Wait => unreachable!("waited above"),
                Next::Gap(from, to, reasons) => {
                    at = to;
                    json!({ "event": "gap", "from": from, "to": to, "reasons": reasons })
                }
                Next::Data(bytes) => {
                    let line = json!({ "event": "data", "offset": at, "b64": crate::bash::base64(&bytes) });
                    at += bytes.len() as u64;
                    line
                }
                Next::Terminal(terminal) => {
                    deliver(&terminal);
                    return Ok(());
                }
            };
            if !deliver(&line) {
                return Ok(());
            }
        }
    }
}

/// One run's hand-off to its stream, held by the relay.
pub(crate) struct Tap {
    capture: Arc<Capture>,
    stream: Arc<Stream>,
}

impl Tap {
    /// Hands one relayed chunk over without waiting (see the module docs).
    pub(crate) fn push(&self, bytes: &[u8]) {
        let len = bytes.len() as u64;
        if len == 0 {
            return;
        }
        let at = self.stream.observed.fetch_add(len, Ordering::Relaxed);
        let active = self.capture.active();
        let Ok(mut ring) = self.stream.ring.try_lock() else {
            self.capture
                .counters
                .contention
                .fetch_add(len, Ordering::Relaxed);
            self.stream.dropped.fetch_max(at + len, Ordering::Relaxed);
            self.stream.changed.notify_all();
            return;
        };
        ring.reconcile(at);
        if active {
            ring.keep(&self.capture, at, bytes);
        } else {
            ring.release_all(&self.capture, "reaped-idle");
            self.capture
                .counters
                .not_captured
                .fetch_add(len, Ordering::Relaxed);
            ring.lose(at, at + len, "not-captured");
        }
        ring.end = at + len;
        drop(ring);
        self.stream.changed.notify_all();
    }

    /// The relay's stream ended (closed, failed or detached): not a terminal.
    pub(crate) fn output_ended(&self, output: &Value) {
        let mut ring = self.stream.ring();
        ring.reconcile(self.stream.observed.load(Ordering::Relaxed));
        ring.output = Some(output.clone());
        drop(ring);
        self.stream.changed.notify_all();
    }

    /// Records the run's terminal record and starts its grace.
    pub(crate) fn finish(self, mut terminal: Value) {
        let mut ring = self.stream.ring();
        ring.reconcile(self.stream.observed.load(Ordering::Relaxed));
        terminal["offset"] = json!(ring.end);
        terminal["incarnation"] = json!(self.capture.incarnation);
        ring.terminal = Some(terminal);
        ring.finished = Some(Instant::now());
        drop(ring);
        self.stream.changed.notify_all();
        self.capture.sweep();
    }
}

/// The terminal record of one run, from what this owner committed.
/// `recorded`: the run's end was committed to the store; `sealed`: its
/// output seal (any state) was.
pub(crate) fn terminal(
    root_id: &str,
    work: i64,
    end: &Value,
    recorded: Result<(), String>,
    sealed: bool,
) -> Value {
    let waits = json!({
        "command": { "event": end["event"], "status": end["status"], "observer": end["observer"], "reason": end["reason"] },
        "work_pid1": end["work_pid1"],
    });
    let ended = matches!(end["event"].as_str(), Some("end" | "end-unknown"));
    match recorded {
        Ok(()) if ended && sealed => json!({
            "event": "final",
            "finalization": "custody-owner",
            "work": work,
            "durable_reference": format!("rv1w:{root_id}:{work}"),
            "retained": end["retained"],
            "output": end["output"],
            "waits": waits,
            "basis": "this owner committed the run's end from its work PID 1's report and its output seal to the root's store, then reported bash-ended",
        }),
        recorded => json!({
            "event": "ended",
            "finalization": "none",
            "work": work,
            "reason": match (&recorded, ended, sealed) {
                (_, false, _) => end["reason"].as_str().or(end["event"].as_str()).unwrap_or("unknown").to_owned(),
                (Err(error), _, _) => format!("end-not-recorded: {error}"),
                (Ok(()), _, false) => "seal-not-recorded".to_owned(),
                _ => "unknown".to_owned(),
            },
            "retained": end["retained"],
            "output": end["output"],
            "waits": waits,
            "meaning": "this incarnation's stream ended without a durable terminal from this owner; a later owner or the store may still settle the run",
        }),
    }
}

struct Cursor {
    incarnation: String,
    offset: u64,
}

enum Request {
    List,
    Attach { work: i64, cursor: Option<Cursor> },
}

fn read_request(conn: &UnixStream) -> Result<Request, String> {
    let _ = conn.set_read_timeout(Some(REQUEST_TIMEOUT));
    let mut line = String::new();
    BufReader::new(conn.take(MAX_REQUEST))
        .read_line(&mut line)
        .map_err(|error| format!("malformed: {error}"))?;
    let value: Value = serde_json::from_str(line.trim()).map_err(|_| "malformed".to_owned())?;
    if value["v"].as_u64() != Some(1) {
        return Err("unsupported-version".to_owned());
    }
    match value["op"].as_str() {
        Some("list") => Ok(Request::List),
        Some("attach") => {
            let work = value["work"]
                .as_i64()
                .filter(|work| *work > 0)
                .ok_or("bad-work")?;
            let cursor = match &value["cursor"] {
                Value::Null => None,
                cursor => Some(Cursor {
                    incarnation: cursor["incarnation"]
                        .as_str()
                        .filter(|text| {
                            text.len() == 32 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                        .ok_or("bad-cursor")?
                        .to_owned(),
                    offset: cursor["offset"].as_u64().ok_or("bad-cursor")?,
                }),
            };
            Ok(Request::Attach { work, cursor })
        }
        _ => Err("unknown-op".to_owned()),
    }
}

fn send(mut conn: &UnixStream, value: &Value) -> std::io::Result<()> {
    let mut line = value.to_string();
    line.push('\n');
    conn.write_all(line.as_bytes())?;
    conn.flush()
}

/// Whether `pid`'s PID namespace is `root` or nested inside it.
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
        Capture::new("r".into(), 1, 1000, limits).unwrap()
    }

    const SMALL: Limits = Limits {
        per_run: 3 * (4 + CHUNK_OVERHEAD),
        per_root: 4 * (4 + CHUNK_OVERHEAD),
        grace: Duration::from_secs(3600),
        idle: Duration::from_secs(3600),
    };

    type Drained = (
        Vec<(u64, Vec<u8>)>,
        Vec<(u64, u64, Vec<&'static str>)>,
        Option<Value>,
    );

    /// Everything a subscriber at `cursor` would be given, as a byte map:
    /// held bytes by offset and gaps, until it would wait or end.
    fn drain(stream: &Stream, mut cursor: u64) -> Drained {
        let (mut data, mut gaps) = (Vec::new(), Vec::new());
        loop {
            match stream.ring().next(cursor) {
                Next::Data(bytes) => {
                    data.push((cursor, bytes.clone()));
                    cursor += bytes.len() as u64;
                }
                Next::Gap(from, to, reasons) => {
                    gaps.push((from, to, reasons));
                    cursor = to;
                }
                Next::Terminal(terminal) => return (data, gaps, Some(terminal)),
                Next::Wait => return (data, gaps, None),
                Next::Ahead => panic!("ahead"),
            }
        }
    }

    /// Before any subscriber asks, nothing is held but every offset is
    /// counted, so a later subscriber gets an exact `not-captured` gap.
    #[test]
    fn on_demand_holding_reports_the_uncaptured_prefix_exactly() {
        let capture = capture(LIMITS);
        let tap = capture.register(7, "from-launch");
        tap.push(b"before");
        capture.touch();
        tap.push(b"after");
        let (data, gaps, terminal) = drain(&tap.stream, 0);
        assert_eq!(gaps, vec![(0, 6, vec!["not-captured"])]);
        assert_eq!(data, vec![(6, b"after".to_vec())]);
        assert!(terminal.is_none());
        assert_eq!(capture.summary()["bytes"]["not_captured"], 6);
    }

    /// Run and root bounds evict oldest first; a chunk that cannot fit even
    /// after evicting all of its own run is dropped as overflow. Gaps and
    /// data together cover every offset exactly once.
    #[test]
    fn bounds_evict_and_overflow_with_exact_gaps() {
        let capture = capture(SMALL);
        capture.touch();
        let first = capture.register(1, "from-launch");
        for chunk in [b"aaaa", b"bbbb", b"cccc", b"dddd"] {
            first.push(chunk);
        }
        // Per-run bound: three chunks held, the first evicted.
        let (data, gaps, _) = drain(&first.stream, 0);
        assert_eq!(gaps, vec![(0, 4, vec!["evicted"])]);
        assert_eq!(
            data.iter().map(|(at, _)| *at).collect::<Vec<_>>(),
            vec![4, 8, 12]
        );
        // Root bound: a run evicts only its own oldest chunk, never
        // another run's; with nothing of its own to evict, its chunk is
        // dropped as overflow.
        let second = capture.register(2, "from-launch");
        second.push(b"eeee");
        second.push(b"ffff");
        let (data, gaps, _) = drain(&second.stream, 0);
        assert_eq!(gaps, vec![(0, 4, vec!["evicted"])]);
        assert_eq!(data, vec![(4, b"ffff".to_vec())]);
        let third = capture.register(3, "from-launch");
        third.push(b"gggg");
        let (data, gaps, _) = drain(&third.stream, 0);
        assert!(data.is_empty());
        assert_eq!(gaps, vec![(0, 4, vec!["capture-overflow"])]);
        assert_eq!(drain(&first.stream, 0).0.len(), 3, "first run untouched");
        assert_eq!(capture.used.load(Ordering::Relaxed), SMALL.per_root);
    }

    /// The relay never waits on a subscriber: with the ring held, the chunk
    /// is not kept, and the next hand-off records its exact range.
    #[test]
    fn a_held_ring_drops_the_chunk_and_records_contention_exactly() {
        let capture = capture(LIMITS);
        capture.touch();
        let tap = capture.register(3, "from-launch");
        tap.push(b"one");
        {
            let _held = tap.stream.ring();
            tap.push(b"two");
        }
        // A waiting subscriber learns of the dropped tail without any
        // further output, exactly as the relay declared it.
        {
            let mut ring = tap.stream.ring();
            ring.reconcile(tap.stream.dropped.load(Ordering::Relaxed));
            assert!(matches!(ring.next(3), Next::Gap(3, 6, _)));
        }
        tap.push(b"three");
        let (data, gaps, _) = drain(&tap.stream, 0);
        assert_eq!(data, vec![(0, b"one".to_vec()), (6, b"three".to_vec())]);
        assert_eq!(gaps, vec![(3, 6, vec!["capture-contention"])]);
        assert_eq!(capture.summary()["bytes"]["contention"], 3);
    }

    /// Output closure is not a terminal; the terminal record is reached
    /// only after it is set, and after the grace the bytes are released
    /// while the terminal remains, behind an exact gap.
    #[test]
    fn terminal_follows_output_end_and_survives_eviction() {
        let capture = capture(Limits {
            grace: Duration::ZERO,
            ..LIMITS
        });
        capture.touch();
        let tap = capture.register(4, "from-launch");
        tap.push(b"hello");
        tap.output_ended(&json!({ "state": "closed", "bytes": 5 }));
        let stream = Arc::clone(&tap.stream);
        assert_eq!(stream.ring().state(), "output-ended");
        assert!(drain(&stream, 0).2.is_none(), "pipe closure is not an end");
        tap.finish(json!({ "event": "final" }));
        assert_eq!(stream.ring().state(), "evicted");
        let (data, gaps, terminal) = drain(&stream, 0);
        assert!(data.is_empty());
        assert_eq!(gaps, vec![(0, 5, vec!["evicted-after-final"])]);
        assert_eq!(terminal.unwrap()["offset"], 5);
        assert_eq!(capture.used.load(Ordering::Relaxed), 0);
    }

    /// After the idle period with no subscriber, held bytes are released
    /// and later ones only counted.
    #[test]
    fn idle_plane_is_reaped() {
        let capture = capture(Limits {
            idle: Duration::ZERO,
            ..LIMITS
        });
        capture.touch();
        let tap = capture.register(5, "from-launch");
        tap.push(b"kept?");
        let (data, gaps, _) = drain(&tap.stream, 0);
        assert!(data.is_empty());
        assert_eq!(gaps, vec![(0, 5, vec!["not-captured"])]);
        assert_eq!(capture.used.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn terminal_is_final_only_with_a_recorded_end_and_seal() {
        let end = json!({ "event": "end", "status": "code:0", "observer": "work-pid1-wait", "work_pid1": "code:0", "retained": {"state": "complete"}, "output": {"state": "closed"} });
        let final_ = terminal("r", 9, &end, Ok(()), true);
        assert_eq!(final_["event"], "final");
        assert_eq!(final_["durable_reference"], "rv1w:r:9");
        assert_eq!(final_["waits"]["work_pid1"], "code:0");
        assert_eq!(
            terminal("r", 9, &end, Err("store-failed".into()), true)["event"],
            "ended"
        );
        assert_eq!(
            terminal("r", 9, &end, Ok(()), false)["reason"],
            "seal-not-recorded"
        );
        let lost = json!({ "event": "end-unknown", "reason": "root-pid1-connection-lost" });
        let ended = terminal("r", 9, &lost, Err("no-durable-end".into()), true);
        assert_eq!(ended["finalization"], "none");
        assert_eq!(ended["reason"], "end-not-recorded: no-durable-end");
        let left = json!({ "event": "left-to-successor" });
        assert_eq!(
            terminal("r", 9, &left, Err("x".into()), true)["reason"],
            "left-to-successor"
        );
    }

    #[test]
    fn only_the_attested_requester_can_be_granted() {
        let live = LiveOutput {
            grant: "uid:1000".into(),
        };
        assert!(live.admit("uid:1000").is_ok());
        assert!(live.admit("uid:1001").is_err());
        assert!(
            LiveOutput {
                grant: "1000".into()
            }
            .admit("uid:1000")
            .is_err()
        );
    }

    /// This test process is not inside a namespace it creates nothing in:
    /// its own PID namespace is "within" itself, and not within a sibling.
    #[test]
    fn namespace_walk_finds_self_and_stops_at_the_top() {
        let pid = i32::try_from(std::process::id()).unwrap();
        let own = std::fs::metadata(format!("/proc/{pid}/ns/pid")).unwrap();
        let own = PidNs {
            dev: own.dev(),
            ino: own.ino(),
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
}
