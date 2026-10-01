//! Per-request phase records for the Broker's request loops.
//!
//! One JSON object per line, appended with a single `write` so concurrent
//! threads never interleave and no lock sits on the measured path. Records
//! hold only times, counts, opcodes, process identities and sizes: never a
//! payload, command, environment value or provider text. The file is not
//! fsynced; a crash may lose its tail. A reader needs no Broker execution:
//! `<state>/phase-records-v1/broker-<pid>-<wall_ns>.jsonl`, first line `start`.
//!
//! Times are `CLOCK_MONOTONIC` nanoseconds (`*_ns` / short keys) so a host
//! sampler can join them; `start` and every `main` window also carry the
//! wall clock for the mono↔wall mapping.

use std::cell::{Cell, RefCell};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const DIRECTORY: &str = "phase-records-v1";
/// A full disk or a runaway writer stops recording, never serving.
const CAP_BYTES: u64 = 256 * 1024 * 1024;

struct Recorder {
    file: File,
    path: PathBuf,
    written: AtomicU64,
    seq: AtomicU64,
    stopped: AtomicBool,
}

static RECORDER: OnceLock<Recorder> = OnceLock::new();

fn clock(id: libc::clockid_t) -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(id, &mut now) };
    (now.tv_sec as u64) * 1_000_000_000 + now.tv_nsec as u64
}

pub fn mono_ns() -> u64 {
    clock(libc::CLOCK_MONOTONIC)
}

pub fn wall_ns() -> u64 {
    clock(libc::CLOCK_REALTIME)
}

/// Opens this incarnation's record file once. Failure leaves recording off
/// and is returned for the caller to report; it never blocks serving.
pub fn init(state_root: &Path) -> io::Result<PathBuf> {
    if let Some(recorder) = RECORDER.get() {
        return Ok(recorder.path.clone());
    }
    let directory = state_root.join(DIRECTORY);
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(&directory)?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other("phase record directory is untrusted"));
    }
    let (mono, wall) = (mono_ns(), wall_ns());
    let path = directory.join(format!("broker-{}-{wall}.jsonl", std::process::id()));
    let file = OpenOptions::new()
        .append(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    let recorder = Recorder {
        file,
        path: path.clone(),
        written: AtomicU64::new(0),
        seq: AtomicU64::new(0),
        stopped: AtomicBool::new(false),
    };
    let _ = RECORDER.set(recorder);
    emit(format!(
        "{{\"k\":\"start\",\"v\":1,\"pid\":{},\"mono_ns\":{mono},\"wall_ns\":{wall}}}",
        std::process::id()
    ));
    Ok(path)
}

pub fn next_seq() -> u64 {
    RECORDER
        .get()
        .map(|recorder| recorder.seq.fetch_add(1, Ordering::Relaxed))
        .unwrap_or(0)
}

fn emit(mut line: String) {
    let Some(recorder) = RECORDER.get() else {
        return;
    };
    if recorder.stopped.load(Ordering::Relaxed) {
        return;
    }
    line.push('\n');
    let total = recorder
        .written
        .fetch_add(line.len() as u64, Ordering::Relaxed)
        + line.len() as u64;
    if total > CAP_BYTES {
        if !recorder.stopped.swap(true, Ordering::Relaxed) {
            let _ = (&recorder.file)
                .write_all(format!("{{\"k\":\"cap\",\"mono_ns\":{}}}\n", mono_ns()).as_bytes());
        }
        return;
    }
    // O_APPEND plus one write per record: concurrent writers do not
    // interleave and no Broker lock is taken.
    if (&recorder.file).write_all(line.as_bytes()).is_err() {
        recorder.stopped.store(true, Ordering::Relaxed);
    }
}

/// Named handler sub-phases. Each is attributed to the request running on
/// the current thread; nested timers add to their own kind only.
#[derive(Clone, Copy)]
pub enum Sub {
    /// `fresh_bash_parent`, including accounting.
    Parent,
    /// `require_accounted_entries` inside parent derivation.
    Accounting,
    /// Waiting to acquire `admission_fences`.
    Fence,
    /// Waiting on a lane→Main bridge reply.
    Bridge,
    /// Durable State calls on the Bash path (commit and its waits).
    State,
}

#[derive(Default, Clone, Copy)]
pub struct SubPhases {
    pub parent_ns: u64,
    pub parent_n: u32,
    pub acct_ns: u64,
    pub acct_entries: u64,
    pub fence_ns: u64,
    pub fence_n: u32,
    pub bridge_ns: u64,
    pub bridge_n: u32,
    pub state_ns: u64,
    pub sqlite_busy_ns: u64,
}

thread_local! {
    static SUBS: RefCell<SubPhases> = RefCell::new(SubPhases::default());
    static CHALLENGE: Cell<u64> = const { Cell::new(0) };
    static RECEIVED: Cell<(u64, u64, u32)> = const { Cell::new((0, 0, 0)) };
}

pub fn add(kind: Sub, ns: u64) {
    SUBS.with(|subs| {
        let mut subs = subs.borrow_mut();
        match kind {
            Sub::Parent => {
                subs.parent_ns += ns;
                subs.parent_n += 1;
            }
            Sub::Accounting => subs.acct_ns += ns,
            Sub::Fence => {
                subs.fence_ns += ns;
                subs.fence_n += 1;
            }
            Sub::Bridge => {
                subs.bridge_ns += ns;
                subs.bridge_n += 1;
            }
            Sub::State => subs.state_ns += ns,
        }
    });
}

pub fn add_accounted_entries(count: usize) {
    SUBS.with(|subs| subs.borrow_mut().acct_entries += count as u64);
}

pub fn timed<T>(kind: Sub, work: impl FnOnce() -> T) -> T {
    let start = mono_ns();
    let result = work();
    add(kind, mono_ns().saturating_sub(start));
    result
}

/// Clears this thread's sub-phases and SQLite busy wait for a new request.
pub fn begin_request() {
    SUBS.with(|subs| *subs.borrow_mut() = SubPhases::default());
    let _ = oulipoly_state::sqlite_wait::take_thread_wait_ns();
}

pub fn take_sub() -> SubPhases {
    let mut subs = SUBS.with(|subs| std::mem::take(&mut *subs.borrow_mut()));
    subs.sqlite_busy_ns = oulipoly_state::sqlite_wait::take_thread_wait_ns();
    subs
}

/// Records which thread held the root admission fences, and for how long,
/// when a hold could delay another request (20 ms or more).
pub fn fence_held(acquired: u64) {
    let released = mono_ns();
    if released.saturating_sub(acquired) < 20_000_000 || RECORDER.get().is_none() {
        return;
    }
    let thread = std::thread::current();
    emit(format!(
        "{{\"k\":\"fence\",\"thread\":\"{}\",\"from\":{acquired},\"to\":{released}}}",
        thread.name().unwrap_or("unnamed")
    ));
}

/// Set by the shared request reader at the challenge write and request read.
pub fn mark_challenge() {
    CHALLENGE.with(|cell| cell.set(mono_ns()));
}

pub fn mark_received(bytes: usize, descriptors: usize) {
    RECEIVED.with(|cell| cell.set((mono_ns(), bytes as u64, descriptors as u32)));
}

/// Takes the marks left by the last request read on this thread.
pub fn take_read_marks() -> (u64, u64, u64, u32) {
    let challenge = CHALLENGE.with(|cell| cell.replace(0));
    let (received, bytes, descriptors) = RECEIVED.with(|cell| cell.replace((0, 0, 0)));
    (challenge, received, bytes, descriptors)
}

/// Request-time image role of a peer: its `exe` device/inode compared with
/// the pinned images. A label only; authority checks are made elsewhere.
pub struct ImageRoles {
    runner: Option<(u64, u64)>,
    bash: Option<(u64, u64)>,
    broker: Option<(u64, u64)>,
}

fn identity_of(file: &File) -> Option<(u64, u64)> {
    file.metadata().ok().map(|meta| (meta.dev(), meta.ino()))
}

impl ImageRoles {
    pub fn new(runner: Option<&File>, bash: Option<&File>) -> Self {
        Self {
            runner: runner.and_then(identity_of),
            bash: bash.and_then(identity_of),
            broker: crate::identity::host_proc_file("self/exe")
                .ok()
                .and_then(|exe| identity_of(&exe)),
        }
    }

    pub fn role(&self, pid: i32) -> &'static str {
        let Some(actual) = crate::identity::host_proc_file(&format!("{pid}/exe"))
            .ok()
            .and_then(|exe| identity_of(&exe))
        else {
            return "unavailable";
        };
        if Some(actual) == self.runner {
            "runner"
        } else if Some(actual) == self.bash {
            "bash"
        } else if Some(actual) == self.broker {
            "broker"
        } else {
            "other"
        }
    }
}

/// Coarse timing of one accepted request. Absent edges stay zero.
#[derive(Default)]
pub struct Request {
    pub lane: &'static str,
    pub seq: u64,
    pub accept: u64,
    pub challenge: u64,
    pub received: u64,
    pub opcode: Option<u8>,
    pub request_bytes: u64,
    pub descriptors: u32,
    pub peer_pid: i32,
    pub peer_starttime: u64,
    pub peer_uid: u32,
    pub peer_image: &'static str,
    pub routed: u64,
    pub handler_start: u64,
    pub handler_end: u64,
    pub reply_written: u64,
    pub reply_bytes: u64,
    pub ok: bool,
    pub sub: SubPhases,
}

impl Request {
    pub fn accepted(lane: &'static str) -> Self {
        Self {
            lane,
            seq: next_seq(),
            accept: mono_ns(),
            peer_image: "unknown",
            ..Self::default()
        }
    }

    pub fn read_done(&mut self) {
        let (challenge, received, bytes, descriptors) = take_read_marks();
        self.challenge = challenge;
        self.received = received;
        self.request_bytes = bytes;
        self.descriptors = descriptors;
    }

    pub fn peer(&mut self, pid: i32, starttime: u64, uid: u32, image: &'static str) {
        self.peer_pid = pid;
        self.peer_starttime = starttime;
        self.peer_uid = uid;
        self.peer_image = image;
    }

    pub fn emit(&self) {
        if RECORDER.get().is_none() {
            return;
        }
        let s = &self.sub;
        emit(format!(
            "{{\"k\":\"req\",\"lane\":\"{}\",\"seq\":{},\"op\":{},\"acc\":{},\"chl\":{},\"rcv\":{},\"rb\":{},\"fds\":{},\"pid\":{},\"st\":{},\"uid\":{},\"img\":\"{}\",\"rt\":{},\"hs\":{},\"he\":{},\"rw\":{},\"wb\":{},\"ok\":{},\"parent\":{},\"parent_n\":{},\"acct\":{},\"acct_entries\":{},\"fence\":{},\"fence_n\":{},\"bridge\":{},\"bridge_n\":{},\"state\":{},\"sqlite_busy\":{}}}",
            self.lane,
            self.seq,
            self.opcode.map_or(-1, i32::from),
            self.accept,
            self.challenge,
            self.received,
            self.request_bytes,
            self.descriptors,
            self.peer_pid,
            self.peer_starttime,
            self.peer_uid,
            self.peer_image,
            self.routed,
            self.handler_start,
            self.handler_end,
            self.reply_written,
            self.reply_bytes,
            self.ok,
            s.parent_ns,
            s.parent_n,
            s.acct_ns,
            s.acct_entries,
            s.fence_ns,
            s.fence_n,
            s.bridge_ns,
            s.bridge_n,
            s.state_ns,
            s.sqlite_busy_ns,
        ));
    }
}

/// Main-loop time shares over a short window. Every iteration's time lands
/// in exactly one bucket, so the buckets sum to the window's covered time.
#[derive(Default)]
pub struct MainWindow {
    start: u64,
    iterations: u64,
    pub reap_ns: u64,
    pub bridge_ns: u64,
    pub bridge_n: u64,
    pub advance_ns: u64,
    pub advance_n: u64,
    pub control_ns: u64,
    pub control_n: u64,
    pub idle_duty_ns: u64,
    pub idle_sleep_ns: u64,
    pub idle_n: u64,
}

const MAIN_WINDOW_NS: u64 = 500_000_000;

impl MainWindow {
    pub fn new() -> Self {
        Self {
            start: mono_ns(),
            ..Self::default()
        }
    }

    /// Counts one finished iteration and emits the window once it is full.
    pub fn iteration_done(&mut self) {
        self.iterations += 1;
        let now = mono_ns();
        if now.saturating_sub(self.start) < MAIN_WINDOW_NS {
            return;
        }
        if RECORDER.get().is_some() {
            emit(format!(
                "{{\"k\":\"main\",\"from\":{},\"to\":{now},\"wall_ns\":{},\"it\":{},\"reap\":{},\"bridge\":{},\"bridge_n\":{},\"advance\":{},\"advance_n\":{},\"control\":{},\"control_n\":{},\"idle_duty\":{},\"idle_sleep\":{},\"idle_n\":{}}}",
                self.start,
                wall_ns(),
                self.iterations,
                self.reap_ns,
                self.bridge_ns,
                self.bridge_n,
                self.advance_ns,
                self.advance_n,
                self.control_ns,
                self.control_n,
                self.idle_duty_ns,
                self.idle_sleep_ns,
                self.idle_n,
            ));
        }
        *self = Self {
            start: now,
            ..Self::default()
        };
    }
}

/// Reads every record file under a State root without the Broker.
pub fn read_all(state_root: &Path) -> io::Result<Vec<serde_json::Value>> {
    let mut paths: Vec<_> = fs::read_dir(state_root.join(DIRECTORY))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    let mut records = Vec::new();
    for path in paths {
        for line in fs::read_to_string(path)?.lines() {
            if let Ok(value) = serde_json::from_str(line) {
                records.push(value);
            }
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints the per-request recording cost on this host: one image-role
    /// lookup plus one record write. Run explicitly with `--ignored`.
    #[test]
    #[ignore = "measurement; run explicitly"]
    fn recording_cost_per_request() {
        let root = tempfile::tempdir().unwrap();
        // A separate process-global recorder may exist; write to a file of
        // the same shape directly to time the same single append.
        let file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(root.path().join("cost.jsonl"))
            .unwrap();
        let roles = ImageRoles::new(None, None);
        let pid = std::process::id() as i32;
        let rounds = 20_000u64;
        let started = mono_ns();
        for request in 0..rounds {
            let role = roles.role(pid);
            let line = format!(
                "{{\"k\":\"req\",\"seq\":{request},\"img\":\"{role}\",\"acc\":{}}}\n",
                mono_ns()
            );
            (&file).write_all(line.as_bytes()).unwrap();
        }
        let per = (mono_ns() - started) / rounds;
        println!("AGE353_RECORDING_COST_NS_PER_REQUEST {per}");
    }

    // The recorder is process-global, so one test owns its initialization.
    #[test]
    fn concurrent_threads_append_whole_records_with_their_own_sub_phases() {
        let root = tempfile::tempdir().unwrap();
        let path = init(root.path()).unwrap();
        assert_eq!(init(root.path()).unwrap(), path);
        let threads: Vec<_> = (0..8)
            .map(|thread| {
                std::thread::spawn(move || {
                    for request in 0..200u64 {
                        begin_request();
                        let mut record = Request::accepted("test");
                        record.opcode = Some(b'X');
                        record.peer(thread as i32, request, 1000, "other");
                        add(Sub::Fence, thread as u64 + 1);
                        timed(Sub::Bridge, || ());
                        record.handler_start = mono_ns();
                        record.handler_end = mono_ns();
                        record.sub = take_sub();
                        record.emit();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let mut window = MainWindow::new();
        window.control_n = 1;
        window.start = 0;
        window.iteration_done();
        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 1 + 8 * 200 + 1);
        let records = read_all(root.path()).unwrap();
        assert_eq!(records.len(), raw.lines().count());
        assert_eq!(records[0]["k"], "start");
        assert!(records[0]["wall_ns"].as_u64().unwrap() > 0);
        let requests: Vec<_> = records.iter().filter(|r| r["k"] == "req").collect();
        assert_eq!(requests.len(), 1600);
        let mut seqs: Vec<_> = requests
            .iter()
            .map(|r| r["seq"].as_u64().unwrap())
            .collect();
        seqs.sort();
        seqs.dedup();
        assert_eq!(seqs.len(), 1600);
        for request in &requests {
            // Each thread's accumulator saw only its own request's waits.
            assert_eq!(
                request["fence"].as_u64().unwrap(),
                request["pid"].as_u64().unwrap() + 1
            );
            assert_eq!(request["fence_n"], 1);
            assert_eq!(request["bridge_n"], 1);
            let keys: Vec<_> = request.as_object().unwrap().keys().cloned().collect();
            for forbidden in ["payload", "command", "argv", "env", "text"] {
                assert!(!keys.iter().any(|key| key.contains(forbidden)), "{keys:?}");
            }
        }
        let main = records.iter().find(|r| r["k"] == "main").unwrap();
        assert_eq!(main["control_n"], 1);
        assert!(main["wall_ns"].as_u64().unwrap() > 0);
    }
}
