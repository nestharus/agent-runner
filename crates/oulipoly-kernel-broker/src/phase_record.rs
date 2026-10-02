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

/// CPU time this thread has run, in nanoseconds. Wall time minus this is
/// time the thread spent off CPU: blocked (locks, I/O, commit/fsync, sleeps)
/// or runnable but waiting for a CPU.
pub fn thread_cpu_ns() -> u64 {
    clock(libc::CLOCK_THREAD_CPUTIME_ID)
}

/// Voluntary and involuntary context switches of this thread so far. A
/// count, not a time: many involuntary switches mean the thread was
/// preempted while runnable; voluntary ones mean it blocked.
fn thread_switches() -> (u64, u64) {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_THREAD, &mut usage) } != 0 {
        return (0, 0);
    }
    (usage.ru_nvcsw as u64, usage.ru_nivcsw as u64)
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
    /// Waiting to acquire the shared per-State-root closed-history mutex.
    History,
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
    pub hist_ns: u64,
    pub hist_n: u32,
    /// Split of `parent` and `state`: this thread's CPU time inside them,
    /// and the measured shared-serialization waits inside them. The rest of
    /// each is off-CPU time not attributed to a named wait.
    pub parent_cpu_ns: u64,
    pub parent_busy_ns: u64,
    pub parent_hist_ns: u64,
    pub state_cpu_ns: u64,
    pub state_busy_ns: u64,
    pub state_hist_ns: u64,
    /// The whole request on this thread: CPU time and context switches.
    pub cpu_ns: u64,
    pub vcsw: u64,
    pub ivcsw: u64,
    /// Last stage label reached (see [`stage`]).
    pub stage: &'static str,
}

thread_local! {
    static SUBS: RefCell<SubPhases> = RefCell::new(SubPhases::default());
    /// CPU time and context switches when this thread's request began.
    static BEGUN: Cell<(u64, u64, u64)> = const { Cell::new((0, 0, 0)) };
    /// The last controlled stage label a request reached on this thread.
    static STAGE: Cell<&'static str> = const { Cell::new("") };
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
            Sub::History => {
                subs.hist_ns += ns;
                subs.hist_n += 1;
            }
        }
    });
}

fn hist_so_far() -> u64 {
    SUBS.with(|subs| subs.borrow().hist_ns)
}

/// Marks the stage a request has reached. Labels are fixed strings in the
/// code, never request content; a failed request's record keeps the last one,
/// which locates the refusal without its error text.
pub fn stage(label: &'static str) {
    STAGE.with(|stage| stage.set(label));
}

pub fn add_accounted_entries(count: usize) {
    SUBS.with(|subs| subs.borrow_mut().acct_entries += count as u64);
}

pub fn timed<T>(kind: Sub, work: impl FnOnce() -> T) -> T {
    // Parent and State also split their time into own CPU and the measured
    // shared waits inside them.
    // The CPU reads sit inside the wall interval. A measured wait can still
    // include a little CPU (a lock's spin), so the unattributed remainder is
    // approximate to that extent.
    let split = matches!(kind, Sub::Parent | Sub::State);
    let start = mono_ns();
    let before = split.then(|| {
        (
            thread_cpu_ns(),
            oulipoly_state::sqlite_wait::thread_wait_ns(),
            hist_so_far(),
        )
    });
    let result = work();
    let cpu_end = split.then(thread_cpu_ns);
    add(kind, mono_ns().saturating_sub(start));
    if let (Some((cpu, busy, hist)), Some(cpu_end)) = (before, cpu_end) {
        let cpu = cpu_end.saturating_sub(cpu);
        let busy = oulipoly_state::sqlite_wait::thread_wait_ns().saturating_sub(busy);
        let hist = hist_so_far().saturating_sub(hist);
        SUBS.with(|subs| {
            let mut subs = subs.borrow_mut();
            if matches!(kind, Sub::Parent) {
                subs.parent_cpu_ns += cpu;
                subs.parent_busy_ns += busy;
                subs.parent_hist_ns += hist;
            } else {
                subs.state_cpu_ns += cpu;
                subs.state_busy_ns += busy;
                subs.state_hist_ns += hist;
            }
        });
    }
    result
}

/// Clears this thread's sub-phases and SQLite busy wait for a new request.
pub fn begin_request() {
    SUBS.with(|subs| *subs.borrow_mut() = SubPhases::default());
    let _ = oulipoly_state::sqlite_wait::take_thread_wait_ns();
    STAGE.with(|stage| stage.set(""));
    let (voluntary, involuntary) = thread_switches();
    BEGUN.with(|begun| begun.set((thread_cpu_ns(), voluntary, involuntary)));
}

pub fn take_sub() -> SubPhases {
    let mut subs = SUBS.with(|subs| std::mem::take(&mut *subs.borrow_mut()));
    subs.sqlite_busy_ns = oulipoly_state::sqlite_wait::take_thread_wait_ns();
    subs.stage = STAGE.with(|stage| stage.replace(""));
    let (cpu, voluntary, involuntary) = BEGUN.with(|begun| begun.replace((0, 0, 0)));
    if cpu > 0 {
        let (now_voluntary, now_involuntary) = thread_switches();
        subs.cpu_ns = thread_cpu_ns().saturating_sub(cpu);
        subs.vcsw = now_voluntary.saturating_sub(voluntary);
        subs.ivcsw = now_involuntary.saturating_sub(involuntary);
    }
    subs
}

/// Takes the waits accumulated on this thread outside a request (Main's
/// bridge and advance work), leaving no request state behind.
pub fn take_untracked_waits() -> (u64, u32, u64) {
    let subs = SUBS.with(|subs| std::mem::take(&mut *subs.borrow_mut()));
    let _ = oulipoly_state::sqlite_wait::take_thread_wait_ns();
    (subs.fence_ns, subs.fence_n, subs.hist_ns)
}

/// Records which thread held the root admission fences, and for how long,
/// when a hold could delay another request (20 ms or more). Called after the
/// guard is released, with the release time.
pub fn fence_held(acquired: u64, released: u64) {
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
            "{{\"k\":\"req\",\"lane\":\"{}\",\"seq\":{},\"op\":{},\"acc\":{},\"chl\":{},\"rcv\":{},\"rb\":{},\"fds\":{},\"pid\":{},\"st\":{},\"uid\":{},\"img\":\"{}\",\"rt\":{},\"hs\":{},\"he\":{},\"rw\":{},\"wb\":{},\"ok\":{},\"parent\":{},\"parent_n\":{},\"acct\":{},\"acct_entries\":{},\"fence\":{},\"fence_n\":{},\"bridge\":{},\"bridge_n\":{},\"state\":{},\"sqlite_busy\":{},\"hist\":{},\"hist_n\":{},\"parent_cpu\":{},\"parent_busy\":{},\"parent_hist\":{},\"state_cpu\":{},\"state_busy\":{},\"state_hist\":{},\"cpu\":{},\"vcsw\":{},\"ivcsw\":{},\"stage\":\"{}\"}}",
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
            s.hist_ns,
            s.hist_n,
            s.parent_cpu_ns,
            s.parent_busy_ns,
            s.parent_hist_ns,
            s.state_cpu_ns,
            s.state_busy_ns,
            s.state_hist_ns,
            s.cpu_ns,
            s.vcsw,
            s.ivcsw,
            s.stage,
        ));
    }
}

/// Main-loop time shares over a short window. Every iteration's time lands
/// in exactly one bucket, so the buckets sum to the window's covered time.
/// The remaining fields are nested inside those buckets, not added to them.
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
    /// Main's own CPU time inside `advance` and `idle_duty`.
    pub advance_cpu_ns: u64,
    pub idle_duty_cpu_ns: u64,
    /// Main's wait for the exclusive side of the admission fences and for
    /// the closed-history mutex outside control requests (inside `bridge`,
    /// `advance` and `idle_duty`).
    pub fence_wait_ns: u64,
    pub fence_wait_n: u64,
    pub hist_wait_ns: u64,
    /// Pending logical duties seen by the owner-close advance: entries it
    /// still had to examine (summed over passes, and the largest pass), how
    /// many reached the drain readback, and passes that made an effect.
    pub adv_pending: u64,
    pub adv_pending_max: u64,
    pub adv_readback: u64,
    pub adv_acted: u64,
}

/// What one owner-close advance pass found. Counts only; it never steers
/// the pass.
#[derive(Default, Clone, Copy)]
pub struct AdvanceScan {
    pub pending: u64,
    pub readback: u64,
    pub acted: bool,
}

impl MainWindow {
    /// Folds one advance pass into the window.
    pub fn advance_scanned(&mut self, scan: AdvanceScan) {
        self.adv_pending += scan.pending;
        self.adv_pending_max = self.adv_pending_max.max(scan.pending);
        self.adv_readback += scan.readback;
        self.adv_acted += u64::from(scan.acted);
    }

    /// Folds Main's untracked fence and history waits since the last call.
    pub fn untracked_waits(&mut self) {
        let (fence, fence_n, hist) = take_untracked_waits();
        self.fence_wait_ns += fence;
        self.fence_wait_n += u64::from(fence_n);
        self.hist_wait_ns += hist;
    }
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
                "{{\"k\":\"main\",\"from\":{},\"to\":{now},\"wall_ns\":{},\"it\":{},\"reap\":{},\"bridge\":{},\"bridge_n\":{},\"advance\":{},\"advance_n\":{},\"control\":{},\"control_n\":{},\"idle_duty\":{},\"idle_sleep\":{},\"idle_n\":{},\"advance_cpu\":{},\"idle_duty_cpu\":{},\"fence_w\":{},\"fence_wn\":{},\"hist_w\":{},\"adv_pending\":{},\"adv_pending_max\":{},\"adv_readback\":{},\"adv_acted\":{}}}",
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
                self.advance_cpu_ns,
                self.idle_duty_cpu_ns,
                self.fence_wait_ns,
                self.fence_wait_n,
                self.hist_wait_ns,
                self.adv_pending,
                self.adv_pending_max,
                self.adv_readback,
                self.adv_acted,
            ));
        }
        *self = Self {
            start: now,
            ..Self::default()
        };
    }
}

thread_local! {
    /// What the current owner-close visit on this thread reached: the last
    /// step label, and whether the visit read back or acted.
    static OWNER_CLOSE_VISIT: Cell<(&'static str, bool, bool)> = const { Cell::new(("", false, false)) };
}

/// Marks the last step an owner-close visit reached. A fixed label in the
/// code, never root content; it locates where a pending root waited.
pub fn owner_close_step(label: &'static str) {
    OWNER_CLOSE_VISIT.with(|visit| {
        let (_, readback, acted) = visit.get();
        visit.set((label, readback, acted));
    });
}

/// Notes what the current owner-close visit did beyond its step label.
pub fn owner_close_effects(readback: bool, acted: bool) {
    OWNER_CLOSE_VISIT.with(|visit| {
        let (label, _, _) = visit.get();
        visit.set((label, readback, acted));
    });
}

fn take_owner_close_visit() -> (&'static str, bool, bool) {
    OWNER_CLOSE_VISIT.with(|visit| visit.replace(("", false, false)))
}

/// One root's identifier as a JSON string. Root IDs are canonical UUIDs;
/// anything else is still escaped, never written raw.
fn root_json(root_id: &str) -> String {
    serde_json::Value::from(root_id).to_string()
}

/// Per-root owner-close selection records. Each pass, the roots Main visits
/// (in revisit order), and each root's first admission to the queue are
/// recorded, so a reader can rebuild every root's selection timeline: when it
/// became pending, each visit's time, outcome and step, the interval between
/// its visits, and the passes in which it waited unvisited. They describe the
/// pass; they never steer it.
pub struct OwnerCloseTrace {
    pass: u64,
    pass_from: u64,
    position: u64,
    visit_from: u64,
}

impl OwnerCloseTrace {
    /// Starts the next pass. Passes are numbered from 1 in this process.
    pub fn pass() -> Self {
        static PASSES: AtomicU64 = AtomicU64::new(0);
        Self {
            pass: PASSES.fetch_add(1, Ordering::Relaxed) + 1,
            pass_from: mono_ns(),
            position: 0,
            visit_from: 0,
        }
    }

    /// A root joined the queue for the first time in this incarnation.
    pub fn admitted(&self, root_id: &str) {
        if RECORDER.get().is_none() {
            return;
        }
        emit(format!(
            "{{\"k\":\"oca\",\"pass\":{},\"root\":{},\"at\":{}}}",
            self.pass,
            root_json(root_id),
            mono_ns()
        ));
    }

    /// Called just before Main examines a root.
    pub fn visit_begins(&mut self) {
        take_owner_close_visit();
        self.visit_from = mono_ns();
    }

    /// Called once the examination returned: `Some(true)` completed,
    /// `Some(false)` still pending, `None` an error that stopped the pass.
    pub fn visit_ended(&mut self, root_id: &str, completed: Option<bool>) {
        let to = mono_ns();
        let (step, readback, acted) = take_owner_close_visit();
        let position = self.position;
        self.position += 1;
        if RECORDER.get().is_none() {
            return;
        }
        let outcome = match completed {
            Some(true) => "complete",
            Some(false) => "pending",
            None => "error",
        };
        emit(format!(
            "{{\"k\":\"ocv\",\"pass\":{},\"pos\":{position},\"root\":{},\"from\":{},\"to\":{to},\"out\":\"{outcome}\",\"step\":\"{step}\",\"rb\":{readback},\"acted\":{acted}}}",
            self.pass,
            root_json(root_id),
            self.visit_from,
        ));
    }

    /// Ends the pass: how many roots were pending at its start, whether its
    /// budget ran out before every queued root was reached, and whether an
    /// error stopped it.
    pub fn pass_ended(&self, pending: u64, budget_spent: bool, error: bool) {
        if RECORDER.get().is_none() {
            return;
        }
        emit(format!(
            "{{\"k\":\"ocp\",\"pass\":{},\"from\":{},\"to\":{},\"pending\":{pending},\"visited\":{},\"budget_spent\":{budget_spent},\"err\":{error}}}",
            self.pass,
            self.pass_from,
            mono_ns(),
            self.position,
        ));
    }
}

/// Exit status of an installed successor the Broker started, observed
/// without reaping it, so the Broker's own later wait is unchanged.
#[derive(Debug, PartialEq, Eq)]
pub enum SuccessorExit {
    Exited(i32),
    Signaled(i32),
    /// The status could not be observed (the child was already reaped):
    /// unknown, never success.
    Unobserved(&'static str),
    /// The wait itself failed with this errno: unknown, never success.
    WaitFailed(&'static str, i32),
}

fn successor_exit_fields(exit: &SuccessorExit) -> (&'static str, i32, i32, String) {
    match exit {
        SuccessorExit::Exited(code) => ("exited", *code, 0, String::new()),
        SuccessorExit::Signaled(signal) => ("signaled", 0, *signal, String::new()),
        SuccessorExit::Unobserved(reason) => ("unobserved", 0, 0, (*reason).into()),
        SuccessorExit::WaitFailed(reason, errno) => {
            ("unobserved", 0, 0, format!("{reason}-errno-{errno}"))
        }
    }
}

/// Records that the Broker spawned a successor for one offer.
pub fn successor_spawned(offer: &str, pid: i32, starttime: u64) {
    if RECORDER.get().is_none() {
        return;
    }
    emit(format!(
        "{{\"k\":\"succ_spawn\",\"offer\":{},\"pid\":{pid},\"st\":{starttime},\"at\":{}}}",
        root_json(offer),
        mono_ns()
    ));
}

/// Records how a spawned successor ended. A successor that fails at entry
/// writes its reason to a null stderr; this record keeps the fact and its
/// status. The reason text is not available here.
/// `by` names the observer: `observer` waits without reaping from spawn;
/// `reaper` is the Broker's own wait after ACK. Both may record one exit.
pub fn successor_exited(
    offer: &str,
    pid: i32,
    starttime: u64,
    by: &'static str,
    exit: &SuccessorExit,
) {
    if RECORDER.get().is_none() {
        return;
    }
    let (how, code, signal, reason) = successor_exit_fields(exit);
    emit(format!(
        "{{\"k\":\"succ_exit\",\"offer\":{},\"pid\":{pid},\"st\":{starttime},\"by\":\"{by}\",\"at\":{},\"how\":\"{how}\",\"code\":{code},\"sig\":{signal},\"reason\":\"{reason}\"}}",
        root_json(offer),
        mono_ns()
    ));
}

/// Records a successor the Broker reaped because it exited before any offer
/// was accepted from it, with what it reported on its gate (`failure`: its
/// stage and bounded reason, or that it left none). One row per such
/// failure; offered and successful successors never write one.
pub fn successor_failed_before_offer(
    offer: &str,
    pid: i32,
    starttime: u64,
    exit: &SuccessorExit,
    failure: &str,
) {
    if RECORDER.get().is_none() {
        return;
    }
    let (how, code, signal, reason) = successor_exit_fields(exit);
    emit(format!(
        "{{\"k\":\"succ_exit\",\"offer\":{},\"pid\":{pid},\"st\":{starttime},\"by\":\"pre-offer-reaper\",\"at\":{},\"how\":\"{how}\",\"code\":{code},\"sig\":{signal},\"reason\":\"{reason}\",\"failure\":{}}}",
        root_json(offer),
        mono_ns(),
        serde_json::Value::from(failure),
    ));
}

/// Reads every record file under a State root without the Broker.
pub fn read_all(state_root: &Path) -> io::Result<Vec<serde_json::Value>> {
    read_all_counted(state_root).map(|(records, _)| records)
}

/// As [`read_all`], also counting lines that did not parse (a torn tail or
/// a damaged record), which `read_all` skips.
pub fn read_all_counted(state_root: &Path) -> io::Result<(Vec<serde_json::Value>, u64)> {
    let mut paths: Vec<_> = fs::read_dir(state_root.join(DIRECTORY))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    paths.sort();
    let mut records = Vec::new();
    let mut skipped = 0;
    for path in paths {
        for line in fs::read_to_string(path)?.lines() {
            match serde_json::from_str(line) {
                Ok(value) => records.push(value),
                Err(_) => skipped += 1,
            }
        }
    }
    Ok((records, skipped))
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
                        stage("test:stage");
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
            assert_eq!(request["stage"], "test:stage");
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

    /// Parent time splits into this thread's CPU, the contended closed-history
    /// wait inside it, and an unattributed remainder; none exceeds the whole.
    #[test]
    fn parent_time_splits_own_cpu_from_the_shared_history_wait() {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().to_path_buf();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let holder = {
            let state = state.clone();
            std::thread::spawn(move || {
                let _history = crate::admission_accounting::closed_history(&state).unwrap();
                held_tx.send(()).unwrap();
                std::thread::sleep(std::time::Duration::from_millis(60));
            })
        };
        held_rx.recv().unwrap();
        begin_request();
        timed(Sub::Parent, || {
            // Own compute, then the shared mutex another thread holds.
            let spin = thread_cpu_ns();
            while thread_cpu_ns() - spin < 20_000_000 {
                std::hint::black_box(spin);
            }
            drop(crate::admission_accounting::closed_history(&state).unwrap());
        });
        let subs = take_sub();
        holder.join().unwrap();
        assert_eq!(subs.parent_n, 1);
        assert_eq!(subs.hist_n, 1);
        assert!(subs.parent_cpu_ns >= 20_000_000, "{}", subs.parent_cpu_ns);
        assert!(subs.parent_hist_ns >= 20_000_000, "{}", subs.parent_hist_ns);
        assert_eq!(subs.parent_hist_ns, subs.hist_ns);
        // Within the spin a lock acquisition may also count as CPU.
        assert!(
            subs.parent_cpu_ns + subs.parent_hist_ns <= subs.parent_ns + 2_000_000,
            "{} + {} > {}",
            subs.parent_cpu_ns,
            subs.parent_hist_ns,
            subs.parent_ns
        );
        assert!(subs.cpu_ns >= subs.parent_cpu_ns);
        assert_eq!(
            subs.state_cpu_ns + subs.state_busy_ns + subs.state_hist_ns,
            0
        );
        // An uncontended acquisition records no wait.
        begin_request();
        drop(crate::admission_accounting::closed_history(&state).unwrap());
        assert_eq!(take_sub().hist_n, 0);
    }

    #[test]
    fn advance_scans_and_untracked_waits_fold_into_the_main_window() {
        let mut window = MainWindow::new();
        window.advance_scanned(AdvanceScan {
            pending: 3,
            readback: 2,
            acted: false,
        });
        window.advance_scanned(AdvanceScan {
            pending: 1,
            readback: 1,
            acted: true,
        });
        add(Sub::Fence, 7);
        add(Sub::History, 5);
        window.untracked_waits();
        assert_eq!(
            (
                window.adv_pending,
                window.adv_pending_max,
                window.adv_readback
            ),
            (4, 3, 3)
        );
        assert_eq!(window.adv_acted, 1);
        assert_eq!((window.fence_wait_ns, window.fence_wait_n), (7, 1));
        assert_eq!(window.hist_wait_ns, 5);
        window.untracked_waits();
        assert_eq!(window.fence_wait_ns, 7, "waits are taken once");
    }
}
