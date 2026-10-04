//! Process-level controls: the real supervisor binary owning real
//! deterministic-peer subprocesses. Deterministic peers prove this crate's
//! contract, not real-harness behaviour.
//!
//! The only timer here is a test watchdog: if an expected line never
//! arrives, the test kills its own supervisor and fails, rather than
//! hanging. The supervisor itself has no timer. A supervisor this test
//! launched is also killed when its `Run` is dropped (including on a failed
//! assertion) and, through a parent-death signal, when the test process
//! itself dies.
//!
//! Peers now run under root PID 1 and their work PID 1s, which outlive a
//! killed owner by design. Cleanup is exact: each root PID 1 a supervisor
//! reports starting is watched by a pidfd opened while it is verified to
//! be the recorded process (pid and start time in the test's own fresh
//! store), and the test's `Scratch` kills each still-running one through
//! that pidfd on drop (its namespace, with every peer, ends with it). This
//! test process is a child subreaper so that root PID 1s orphaned by a
//! killed owner are reparented to it and reaped here by exact pid. That is
//! test cleanup only: no owner relies on it (in production an orphaned
//! root PID 1 goes to the host's init).

use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_oulipoly-root-supervisor");
const PEER: &str = env!("CARGO_BIN_EXE_oulipoly-acp-deterministic-peer");
const WATCHDOG: Duration = Duration::from_secs(60);
const QUIET_WINDOW: Duration = Duration::from_secs(2);

/// A root PID 1 reported started by one of this test's supervisors.
struct RootWatch {
    pid: i32,
    fd: OwnedFd,
}

type Roots = Arc<Mutex<Vec<RootWatch>>>;

struct Scratch(PathBuf, Roots);

fn pidfd(pid: i32) -> Option<OwnedFd> {
    // SAFETY: open an observation fd; verified against the store below.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    // SAFETY: the new fd has no other owner.
    (raw >= 0).then(|| unsafe { OwnedFd::from_raw_fd(i32::try_from(raw).unwrap()) })
}

fn start_time(pid: i32) -> Option<i64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn exited(fd: &OwnedFd, timeout_ms: i32) -> bool {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll receives one valid pollfd.
    let ready = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
    assert!(ready >= 0, "pidfd poll failed");
    ready == 1
}

/// Appends a watched root PID 1's exact identity to the file named by
/// `ROOT_CUSTODY_STAMPS`, when set, so a controller can afterwards check
/// those exact processes (and only those) are gone.
fn stamp(pid: i32, start_time: i64) {
    if let Some(path) = std::env::var_os("ROOT_CUSTODY_STAMPS") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        // One append buffer: concurrent fixture stamps must not interleave
        // the pid and start time from different owned roots.
        file.write_all(format!("{pid} {start_time}\n").as_bytes())
            .unwrap();
    }
}

/// The isolation an owner run by this test's uid must report.
fn expected_isolation() -> &'static str {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } == 0 {
        "host-root-pidns"
    } else {
        "unprivileged-userns-pidns"
    }
}

/// Reaps `pid` if (and only if) it is this test process's child.
fn reap_if_ours(pid: i32) -> Option<i32> {
    let mut status = 0;
    // SAFETY: waitpid on one exact pid; ECHILD if it is not our child.
    let reaped = unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) };
    (reaped == pid).then_some(status)
}

impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        // SAFETY: prctl with integer arguments.
        assert_eq!(
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) },
            0
        );
        let dir = std::env::temp_dir().join(format!(
            "root-supervisor-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        eprintln!("owned-fixture: {}", dir.display());
        Self(dir, Roots::default())
    }

    /// The watched root PID 1 with this host pid.
    fn root_fd(&self, pid: i32) -> OwnedFd {
        let roots = self.1.lock().unwrap();
        let watch = roots
            .iter()
            .find(|watch| watch.pid == pid)
            .expect("watched root pid1");
        watch.fd.try_clone().unwrap()
    }

    fn state(&self, harness: &str) -> PathBuf {
        self.0.join(format!("{harness}.json"))
    }

    /// This root's private store directory (created by the supervisor).
    fn store(&self) -> PathBuf {
        self.0.join("root")
    }
}

impl Drop for Scratch {
    /// Kills each still-running root PID 1 of this test through its verified
    /// pidfd, waits for its exit, and reaps it if it is this process's child.
    fn drop(&mut self) {
        for watch in self.1.lock().unwrap().drain(..) {
            if !exited(&watch.fd, 0) {
                // SAFETY: pidfd_send_signal on a pidfd naming our verified root.
                unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        watch.fd.as_raw_fd(),
                        libc::SIGKILL,
                        std::ptr::null::<libc::siginfo_t>(),
                        0,
                    );
                }
                let _ = exited(&watch.fd, 10_000);
            }
            let _ = reap_if_ours(watch.pid);
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn peer(state: &Path, extra: &[&str]) -> Vec<String> {
    let mut argv = vec![
        PEER.to_owned(),
        "--state".to_owned(),
        state.display().to_string(),
    ];
    argv.extend(extra.iter().map(|arg| (*arg).to_owned()));
    argv
}

fn read_state(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Waits for a peer's state file to satisfy `pred` (test watchdog only).
fn wait_state(path: &Path, pred: impl Fn(&Value) -> bool) {
    let deadline = std::time::Instant::now() + WATCHDOG;
    loop {
        let state = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if state.as_ref().is_some_and(&pred) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "watchdog: peer state");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Run {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    seen: Vec<Value>,
    store: PathBuf,
    roots: Roots,
}

impl Run {
    fn start(dir: &Scratch, spec: &Value) -> Self {
        let mut command = Command::new(SUPERVISOR);
        command.stdin(Stdio::piped()).stdout(Stdio::piped());
        // SAFETY: prctl with integer arguments, async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "{spec}").unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                let value = serde_json::from_str(&line).expect("json line");
                if tx.send(value).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            seen: Vec::new(),
            store: PathBuf::from(spec["store"].as_str().unwrap()),
            roots: Arc::clone(&dir.1),
        }
    }

    /// Watches a reported root PID 1, only if it is still the process the
    /// store recorded (same pid and start time).
    fn observe(&mut self, value: &Value) {
        if value["event"] != "root-pid1-started" {
            return;
        }
        let pid = i32::try_from(value["pid"].as_i64().unwrap()).unwrap();
        let Some(fd) = pidfd(pid) else { return };
        let conn = rusqlite::Connection::open_with_flags(
            self.store.join("intent.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let recorded: Option<i64> = conn
            .query_row(
                "SELECT start_time FROM incarnation WHERE host_pid = ?1 ORDER BY id DESC",
                [pid],
                |row| row.get(0),
            )
            .ok();
        if let Some(recorded) = recorded.filter(|recorded| Some(*recorded) == start_time(pid)) {
            stamp(pid, recorded);
            self.roots.lock().unwrap().push(RootWatch { pid, fd });
        }
    }

    /// Returns the first line, already seen or new, that matches, failing
    /// (and killing our own
    /// supervisor) if the watchdog expires first.
    fn until(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(value) = self.seen.iter().find(|value| pred(value)) {
            return value.clone();
        }
        loop {
            match self.lines.recv_timeout(WATCHDOG) {
                Ok(value) => {
                    self.observe(&value);
                    self.seen.push(value.clone());
                    if pred(&value) {
                        return value;
                    }
                    assert_ne!(
                        value["event"], "terminal",
                        "terminal before {what}: {value}\nseen: {:#?}",
                        self.seen
                    );
                }
                Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("watchdog: no {what}\nseen: {:#?}", self.seen);
                }
            }
        }
    }

    fn event(&mut self, harness: &str, event: &str) -> Value {
        let what = format!("{event} from {harness}");
        self.until(&what, |value| {
            value["harness"] == harness && value["event"] == event
        })
    }

    fn cancel(&mut self) {
        writeln!(self.stdin, "{}", json!({ "cmd": "cancel" })).unwrap();
    }

    fn terminal(mut self) -> (Value, ExitStatus, Vec<Value>) {
        let terminal = self.until("terminal", |value| value["event"] == "terminal");
        let status = self.child.wait().unwrap();
        (terminal, status, std::mem::take(&mut self.seen))
    }

    fn supervisor_pid(&self) -> u32 {
        self.child.id()
    }

    /// Lines already emitted, without waiting for more.
    fn drain_now(&mut self) -> &[Value] {
        while let Ok(value) = self.lines.try_recv() {
            self.observe(&value);
            self.seen.push(value);
        }
        &self.seen
    }

    /// SIGKILLs this test's own supervisor child and returns what it said.
    fn kill(mut self) -> Vec<Value> {
        self.child.kill().unwrap();
        let status = self.child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL));
        while let Ok(value) = self.lines.recv_timeout(WATCHDOG) {
            self.observe(&value);
            self.seen.push(value);
        }
        std::mem::take(&mut self.seen)
    }
}

impl Drop for Run {
    /// Kills and reaps this test's own supervisor if it is still running
    /// (a no-op after `terminal` or `kill` have reaped it).
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn harness<'a>(terminal: &'a Value, id: &str) -> &'a Value {
    terminal["harnesses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == id)
        .unwrap()
}

fn alive(pid: u64) -> bool {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // Only called for a child launched by this test's supervisor. pidfd
    // readiness distinguishes an exited child (including a zombie).
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: open an observation fd for this test's owned child.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        return false;
    }
    // SAFETY: the new fd has no other owner.
    let fd = unsafe { OwnedFd::from_raw_fd(i32::try_from(raw).unwrap()) };
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll receives one valid pollfd and never waits.
    let result = unsafe { libc::poll(&mut poll, 1, 0) };
    assert!(result >= 0, "pidfd poll failed");
    result == 0
}

fn events<'a>(seen: &'a [Value], harness: &str, event: &str) -> Vec<&'a Value> {
    seen.iter()
        .filter(|value| value["harness"] == harness && value["event"] == event)
        .collect()
}

/// A create request for a new root store.
fn spec(dir: &Scratch, cap: u32, harnesses: Value) -> Value {
    json!({
        "store": dir.store(),
        "intent": {
            "outage_closure_cap": cap,
            "delivery_attempt_cap": 10,
            "cwd": "/",
            "harnesses": harnesses,
        },
    })
}

/// A recover request for an existing root store.
fn recover(dir: &Scratch) -> Value {
    json!({ "store": dir.store() })
}

/// (a) A separate supervisor process concurrently owns two peer processes,
/// reaps both exactly, and ends normally only when nothing is owed.
#[test]
fn separate_process_owns_two_harnesses_and_ends_after_exact_reaping() {
    let dir = Scratch::new("own");
    let run = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([
                { "id": "a", "argv": peer(&dir.state("a"), &["--exit-after-acks", "2"]), "messages": ["one", "two"] },
                { "id": "b", "argv": peer(&dir.state("b"), &["--exit-after-acks", "1"]), "messages": ["three"] },
            ]),
        ),
    );
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    assert_eq!(terminal["owed"], 0);
    assert_eq!(terminal["all_harnesses_reaped"], true);
    assert_eq!(terminal["records_complete"], true);
    for id in ["a", "b"] {
        let record = harness(&terminal, id);
        assert_eq!(record["launches"], 1);
        assert_eq!(record["exits"], json!(["code:0"]));
        let launches = read_state(&dir.state(id))["launches"].clone();
        assert_eq!(launches.as_array().unwrap().len(), 1);
        // The peer is pid 2 of its own work namespace, whose PID 1 (its
        // actual parent and waiter) is the per-work PID 1.
        assert_eq!(launches[0]["ppid"], 1);
        assert_eq!(launches[0]["pid"], 2);
        assert_eq!(events(&seen, id, "exited")[0]["reaped"], "work-pid1-wait");
        assert_eq!(events(&seen, id, "exited")[0]["work_pid1"], "code:0");
    }
    let started = seen
        .iter()
        .find(|value| value["event"] == "root-pid1-started")
        .unwrap();
    assert_eq!(started["isolation"], expected_isolation());
    // This owner started root PID 1, released it, and waited it as parent.
    assert_eq!(terminal["root_pid1"]["outcome"], "released-exit-waited");
    assert_eq!(terminal["root_pid1"]["status"], "code:0");
    assert_eq!(terminal["root_pid1"]["parent"], true);
    let messages = harness(&terminal, "a")["messages"].as_array().unwrap();
    assert!(messages.iter().all(|m| m["state"] == "acknowledged"));
    assert!(messages.iter().all(|m| m["basis"] == "single-attempt"));
}

/// (b) Exit before acknowledgement, observed by the exact child's exit,
/// relaunches and resubmits the same key. The dedup peer inserts once; the
/// non-dedup peer yields duplicate-unknown, never at-most-once.
#[test]
fn exit_before_ack_relaunches_with_same_key() {
    let dir = Scratch::new("retry");
    let run = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([
                { "id": "dedup", "argv": peer(&dir.state("dedup"), &["--mode", "exit-before-ack-once", "--exit-after-acks", "1"]), "messages": ["hello"] },
                { "id": "plain", "argv": peer(&dir.state("plain"), &["--mode", "exit-before-ack-once", "--no-dedup", "--exit-after-acks", "1"]), "messages": ["hello"] },
            ]),
        ),
    );
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    for id in ["dedup", "plain"] {
        // Exit observed before the closure is counted and before relaunch.
        let order: Vec<&str> = seen
            .iter()
            .filter(|value| value["harness"] == id)
            .filter_map(|value| value["event"].as_str())
            .filter(|event| ["exited", "closure-observed", "relaunch", "launched"].contains(event))
            .collect();
        assert_eq!(
            order,
            [
                "launched",
                "exited",
                "closure-observed",
                "relaunch",
                "launched",
                "exited"
            ],
            "{id}"
        );
        assert_eq!(events(&seen, id, "relaunch")[0]["same_key"], true);
        assert_eq!(events(&seen, id, "session-resumed").len(), 1);
        let state = read_state(&dir.state(id));
        let prompts = state["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 2, "{id}");
        assert_eq!(prompts[0]["key"], prompts[1]["key"], "{id}: same key");
        assert_eq!(prompts[0]["session"], prompts[1]["session"], "{id}");
        assert_eq!(harness(&terminal, id)["launches"], 2);
        assert_eq!(harness(&terminal, id)["messages"][0]["closures"], 1);
    }
    let dedup = &harness(&terminal, "dedup")["messages"][0];
    assert_eq!(
        read_state(&dir.state("dedup"))["insertions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(dedup["label"], "accepted");
    assert_eq!(dedup["basis"], "session-contract");
    assert_eq!(dedup["recovered"], true);

    let plain = &harness(&terminal, "plain")["messages"][0];
    assert_eq!(
        read_state(&dir.state("plain"))["insertions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(plain["label"], "duplicate-unknown");
    assert_eq!(plain["at_most_once"], false);
}

/// (c) A silent harness does not block another harness's delivery, is not
/// killed, and stays alive and owed until explicit cancel. (d) Cancel
/// reports undelivered owner/history loss and observes its termination exit.
#[test]
fn silent_harness_stays_owed_while_other_delivers_until_cancel() {
    let dir = Scratch::new("silent");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([
                { "id": "quiet", "argv": peer(&dir.state("quiet"), &["--mode", "silent"]), "messages": ["are you there"] },
                { "id": "busy", "argv": peer(&dir.state("busy"), &["--exit-after-acks", "1"]), "messages": ["hi"] },
            ]),
        ),
    );
    let quiet_pid = run.event("quiet", "launched")["pid"].as_u64().unwrap();
    run.event("busy", "ack");
    run.event("busy", "exited");
    // The quiet prompt reached the harness and nothing answered it.
    wait_state(&dir.state("quiet"), |state| {
        !state["prompts"].as_array().unwrap().is_empty()
    });
    // Stay quiet for a while before looking: a supervisor that killed or
    // gave up on silence within this window would be caught. Longer silence
    // timeouts are outside what any finite window can show.
    std::thread::sleep(QUIET_WINDOW);
    assert!(alive(quiet_pid), "silent harness must not be killed");
    assert!(events(&run.seen, "quiet", "ack").is_empty());
    assert!(events(&run.seen, "quiet", "outage").is_empty());
    assert!(events(&run.seen, "quiet", "closure-observed").is_empty());

    run.cancel();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "cancelled");
    assert_eq!(status.code(), Some(2));
    let quiet = harness(&terminal, "quiet");
    assert_eq!(quiet["exits"], json!(["signal:9"]));
    assert_eq!(quiet["messages"][0]["state"], "owed");
    assert_eq!(quiet["messages"][0]["label"], "cancelled");
    assert_eq!(quiet["messages"][0]["closures"], 0);
    assert!(events(&seen, "quiet", "relaunch").is_empty());
    assert!(!alive(quiet_pid));
    let busy = harness(&terminal, "busy");
    assert_eq!(busy["messages"][0]["state"], "acknowledged");
    assert_eq!(busy["exits"], json!(["code:0"]));
}

/// (e) Reaching the cap of observed no-ack closures yields the outage label
/// with its count, and nothing is relaunched beyond it.
#[test]
fn closure_cap_declares_outage_on_observed_closures() {
    let dir = Scratch::new("outage");
    let run = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([
                { "id": "flaky", "argv": peer(&dir.state("flaky"), &["--mode", "exit-before-ack-always"]), "messages": ["x", "y"] },
                { "id": "fine", "argv": peer(&dir.state("fine"), &["--exit-after-acks", "1"]), "messages": ["z"] },
            ]),
        ),
    );
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    assert_eq!(terminal["all_harnesses_reaped"], true);
    assert_eq!(terminal["records_complete"], true);
    assert_eq!(terminal["owed_history"], "retained-in-store");
    let flaky = harness(&terminal, "flaky");
    assert_eq!(flaky["launches"], 3);
    assert_eq!(flaky["exits"], json!(["code:1", "code:1", "code:1"]));
    assert_eq!(
        read_state(&dir.state("flaky"))["launches"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(flaky["messages"][0]["label"], "outage");
    assert_eq!(flaky["messages"][0]["closures"], 3);
    assert_eq!(flaky["messages"][0]["state"], "owed");
    assert_eq!(flaky["messages"][1]["label"], "not-attempted");
    let outage = events(&seen, "flaky", "outage");
    assert_eq!(outage.len(), 1);
    assert_eq!(outage[0]["closures"], 3);
    assert_eq!(events(&seen, "flaky", "closure-observed").len(), 3);
    assert_eq!(
        harness(&terminal, "fine")["messages"][0]["state"],
        "acknowledged"
    );
}

/// (f) A send fault with no observed closure is not PeerGone: no closure is
/// counted, nothing is relaunched, the harness stays alive and owed.
#[test]
fn send_fault_without_closure_is_not_peer_gone() {
    let dir = Scratch::new("sendfault");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([
                { "id": "deaf", "argv": peer(&dir.state("deaf"), &["--mode", "close-stdin"]), "messages": ["hello"] },
                { "id": "ok", "argv": peer(&dir.state("ok"), &["--exit-after-acks", "1"]), "messages": ["hi"] },
            ]),
        ),
    );
    let deaf_pid = run.event("deaf", "launched")["pid"].as_u64().unwrap();
    run.event("deaf", "send-fault");
    run.event("ok", "exited");
    assert!(alive(deaf_pid));
    assert!(events(&run.seen, "deaf", "peer-gone").is_empty());
    assert!(events(&run.seen, "deaf", "closure-observed").is_empty());
    assert!(events(&run.seen, "deaf", "outage").is_empty());

    run.cancel();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "cancelled");
    assert_eq!(status.code(), Some(2));
    let deaf = harness(&terminal, "deaf");
    assert_eq!(deaf["launches"], 1);
    assert_eq!(deaf["exits"], json!(["signal:9"]));
    assert_eq!(deaf["messages"][0]["label"], "cancelled");
    assert_eq!(deaf["messages"][0]["closures"], 0);
    assert!(events(&seen, "deaf", "relaunch").is_empty());
}

/// (g) Acknowledgement alone never ends the process: with every message
/// acknowledged but one harness still alive, only cancel ends it.
#[test]
fn acknowledgement_alone_does_not_end_before_reaping() {
    let dir = Scratch::new("ackend");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([
                { "id": "stays", "argv": peer(&dir.state("stays"), &[]), "messages": ["a"] },
                { "id": "goes", "argv": peer(&dir.state("goes"), &["--exit-after-acks", "1"]), "messages": ["b"] },
            ]),
        ),
    );
    let stays_pid = run.event("stays", "launched")["pid"].as_u64().unwrap();
    run.event("stays", "ack");
    run.event("stays", "idle");
    run.event("goes", "exited");
    assert!(alive(stays_pid));
    run.cancel();
    let (terminal, status, _) = run.terminal();
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert_eq!(status.code(), Some(2));
    assert_eq!(terminal["owed"], 0);
    assert_eq!(terminal["all_harnesses_reaped"], true);
    assert_eq!(terminal["records_complete"], true);
    let stays = harness(&terminal, "stays");
    assert_eq!(stays["messages"][0]["state"], "acknowledged");
    assert_eq!(stays["exits"], json!(["signal:9"]));
    assert!(!alive(stays_pid));
}

#[test]
fn invalid_spec_launches_nothing() {
    let dir = Scratch::new("invalid");
    let run = Run::start(&dir, &spec(&dir, 0, json!([])));
    let (terminal, status, _) = run.terminal();
    assert_eq!(terminal["status"], "spec-refused");
    assert_eq!(status.code(), Some(64));
    assert!(!dir.store().exists(), "a refused request writes nothing");
}

/// An exit watch on a peer opened while it is known to be alive, so that
/// a later reuse of its PID cannot be mistaken for it.
struct PeerWatch(std::os::fd::OwnedFd);

/// Opens a watch on a just-launched peer that has not yet been released.
fn watch(pid: u64) -> PeerWatch {
    use std::os::fd::{FromRawFd, OwnedFd};
    let pid = libc::pid_t::try_from(pid).unwrap();
    // SAFETY: open an observation fd for a peer of this test's supervisor.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    assert!(raw >= 0, "peer {pid} gone before its watch was opened");
    // SAFETY: the new fd has no other owner.
    PeerWatch(unsafe { OwnedFd::from_raw_fd(i32::try_from(raw).unwrap()) })
}

/// Waits until a watched process has exited. Test watchdog only.
fn wait_gone(peer: &PeerWatch) {
    use std::os::fd::AsRawFd;
    let mut poll = libc::pollfd {
        fd: peer.0.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = i32::try_from(WATCHDOG.as_millis()).unwrap();
    // SAFETY: poll receives one valid pollfd.
    let ready = unsafe { libc::poll(&mut poll, 1, timeout) };
    assert_eq!(ready, 1, "watchdog: peer did not exit");
}

fn db(dir: &Scratch) -> rusqlite::Connection {
    rusqlite::Connection::open_with_flags(
        dir.store().join("intent.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

/// Direct children of `pid`, a process of this test's own tree.
fn children(pid: u64) -> Vec<u64> {
    let mut found = Vec::new();
    for task in std::fs::read_dir(format!("/proc/{pid}/task")).unwrap() {
        if let Ok(text) = std::fs::read_to_string(task.unwrap().path().join("children")) {
            found.extend(
                text.split_whitespace()
                    .map(|pid| pid.parse::<u64>().unwrap()),
            );
        }
    }
    found.sort_unstable();
    found
}

fn ppid(pid: u64) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    stat.rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

fn custody(seen: &[Value]) -> &Value {
    seen.iter()
        .find(|value| value["event"] == "custody")
        .unwrap()
}

fn roots_started(seen: &[Value]) -> Vec<&Value> {
    seen.iter()
        .filter(|value| value["event"] == "root-pid1-started")
        .collect()
}

/// The quiet peer used by survival tests: inserts each prompt without
/// acknowledging it until a restarted owner initializes again, then acts
/// normally and exits after one acknowledgement.
fn quiet_until_reattach(state: &Path, extra: &[&str]) -> Vec<String> {
    let mut args = vec![
        "--mode",
        "insert-then-silent",
        "--on-reinit",
        "normal",
        "--exit-after-acks",
        "1",
    ];
    args.extend_from_slice(extra);
    peer(state, &args)
}

/// (h) The owner is SIGKILLed while two deliveries are inserted but
/// unanswered (quiet, owed work). Root PID 1 and both peers stay alive: the
/// owner's death kills nothing. Placement while it ran: owner, then root
/// PID 1 as its direct child, then one work PID 1 per harness, then the
/// harness; nothing else. A restarted owner attaches to the same root PID 1
/// by its recorded identity and token, reattaches to both surviving peers
/// (no relaunch, same processes), resumes their sessions and resubmits the
/// SAME keys through `AcpClient`. The dedup peer inserts once; the
/// non-dedup peer inserts twice; neither restored ACK is at-most-once.
#[test]
fn owner_killed_with_quiet_work_leaves_root_and_peers_alive_and_restart_reattaches() {
    let dir = Scratch::new("survive");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([
                { "id": "dedup", "argv": quiet_until_reattach(&dir.state("dedup"), &[]), "messages": ["hello"] },
                { "id": "plain", "argv": quiet_until_reattach(&dir.state("plain"), &["--no-dedup"]), "messages": ["hello"] },
            ]),
        ),
    );
    let root = first.until("root pid1", |value| value["event"] == "root-pid1-started");
    assert_eq!(root["parent"], "this-owner");
    let root_pid = root["pid"].as_u64().unwrap();
    let owner_pid = u64::from(first.supervisor_pid());
    let mut peers = Vec::new();
    for id in ["dedup", "plain"] {
        peers.push(first.event(id, "launched")["pid"].as_u64().unwrap());
        wait_state(&dir.state(id), |state| {
            state["insertions"].as_array().unwrap().len() == 1
        });
    }
    peers.sort_unstable();
    // Process classes and counts: one root PID 1 per root, one work PID 1
    // and one harness per work. No other process in the tree.
    assert_eq!(children(owner_pid), vec![root_pid]);
    let works = children(root_pid);
    assert_eq!(works.len(), 2, "one work PID 1 per harness");
    let mut harnesses = Vec::new();
    for work in &works {
        let below = children(*work);
        assert_eq!(below.len(), 1, "a work PID 1 has exactly its harness");
        assert!(
            children(below[0]).is_empty(),
            "a harness has no wrapper below it"
        );
        harnesses.extend(below);
    }
    harnesses.sort_unstable();
    assert_eq!(
        harnesses, peers,
        "each harness's parent is its own work PID 1"
    );

    let seen = first.kill();
    assert!(seen.iter().all(|value| value["event"] != "terminal"));
    assert!(seen.iter().all(|value| value["event"] != "ack"));
    // A while after the owner's death: nothing it owned was killed.
    std::thread::sleep(QUIET_WINDOW);
    assert!(alive(root_pid), "root PID 1 must survive its owner");
    for peer in &peers {
        assert!(alive(*peer), "quiet peer {peer} must survive the owner");
    }
    // Orphaned (reparented to this test, a subreaper), still holding its work.
    assert_eq!(ppid(root_pid), u64::from(std::process::id()));
    assert_eq!(children(root_pid), works);

    let mut second = Run::start(&dir, &recover(&dir));
    // Checked before delivery proceeds, so a wrong custody choice fails here.
    let found = second.until("custody", |value| value["event"] == "custody");
    assert_eq!(found["outcome"], "attached", "{found}");
    assert_eq!(found["incarnation"], 1);
    assert_eq!(found["survivors"], 2);
    for id in ["dedup", "plain"] {
        let first = second.until("first custody event", |value| {
            value["harness"] == id
                && ["reattached", "launched", "prior-end-unknown", "prior-exit"]
                    .contains(&value["event"].as_str().unwrap_or_default())
        });
        assert_eq!(
            first["event"], "reattached",
            "{id}: survivor kept, not dropped: {first}"
        );
    }
    let (terminal, status, seen) = second.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    assert!(roots_started(&seen).is_empty(), "no new incarnation");
    let recovered = seen
        .iter()
        .find(|value| value["event"] == "intent-recovered")
        .unwrap();
    assert_eq!(recovered["generation"], 2);
    assert_eq!(recovered["prior_attempts_unknown"], 2);
    for id in ["dedup", "plain"] {
        let state = read_state(&dir.state(id));
        let prompts = state["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 2, "{id}");
        assert_eq!(prompts[0]["key"], prompts[1]["key"], "{id}: same key");
        assert_eq!(prompts[0]["session"], prompts[1]["session"], "{id}");
        assert_eq!(
            state["launches"].as_array().unwrap().len(),
            1,
            "{id}: same process"
        );
        let reattached = events(&seen, id, "reattached");
        assert_eq!(reattached.len(), 1, "{id}");
        assert!(peers.contains(&reattached[0]["pid"].as_u64().unwrap()));
        assert_eq!(events(&seen, id, "session-resumed").len(), 1, "{id}");
        assert!(
            events(&seen, id, "launched").is_empty(),
            "{id}: no relaunch"
        );
        let record = harness(&terminal, id);
        assert_eq!(record["launches"], 0);
        assert_eq!(record["reattached"], 1);
        assert_eq!(record["exits"], json!(["code:0"]));
        let message = &record["messages"][0];
        assert_eq!(message["state"], "acknowledged");
        assert_eq!(message["label"], "duplicate-unknown", "{id}");
        assert_eq!(message["at_most_once"], false);
        assert_eq!(message["ack_generation"], 2);
        assert_eq!(message["attempts"], 2);
        assert_eq!(message["prior_unknown"], 1);
        assert_eq!(message["closures"], 0, "owner death is not a closure");
        assert_eq!(message["completion"], "not-observed");
    }
    assert_eq!(
        read_state(&dir.state("dedup"))["insertions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        harness(&terminal, "dedup")["messages"][0]["recovered"],
        true
    );
    assert_eq!(
        read_state(&dir.state("plain"))["insertions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        harness(&terminal, "plain")["messages"][0]["recovered"],
        false
    );
    // Not its parent: only the exit is observed, never its status.
    assert_eq!(
        terminal["root_pid1"]["outcome"],
        "released-exit-observed-by-pidfd"
    );
    assert_eq!(terminal["root_pid1"]["parent"], false);
    assert!(terminal["root_pid1"]["status"].is_null());
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM attempt WHERE generation = 1 AND outcome = 'unknown-prior-owner'"
        ),
        2
    );
    assert_eq!(count(&conn, "SELECT count(*) FROM incarnation"), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM work WHERE outcome = 'code:0' AND observer = 'work-pid1-wait'"
        ),
        2
    );
}

/// A socket-endpoint harness: the owner chooses its socket under the
/// store, connects to it rather than to stdio, and a restarted owner takes
/// the surviving harness back by connecting to the same socket again. The
/// owner's death closed only its connection, not the harness's endpoint.
#[test]
fn socket_endpoint_survives_owner_and_restart_reconnects_to_it() {
    let dir = Scratch::new("socket");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{
                "id": "sock",
                "argv": quiet_until_reattach(&dir.state("sock"), &[]),
                "endpoint": "unix-socket",
                "messages": ["hello"],
            }]),
        ),
    );
    let launched = first.event("sock", "launched");
    let peer_pid = launched["pid"].as_u64().unwrap();
    let work = launched["work"].as_i64().unwrap();
    first.event("sock", "endpoint-connected");
    let opened = first.event("sock", "session-opened");
    wait_state(&dir.state("sock"), |state| {
        state["insertions"].as_array().unwrap().len() == 1
    });
    let socket = dir.store().join("acp").join(format!("w{work}.sock"));
    assert!(socket.exists(), "the owner-chosen socket path");

    let seen = first.kill();
    assert!(seen.iter().all(|value| value["event"] != "ack"));
    std::thread::sleep(QUIET_WINDOW);
    assert!(alive(peer_pid), "the harness survives its owner");

    let mut second = Run::start(&dir, &recover(&dir));
    let found = second.until("custody", |value| value["event"] == "custody");
    assert_eq!(found["outcome"], "attached", "{found}");
    assert_eq!(second.event("sock", "reattached")["pid"], peer_pid);
    second.event("sock", "endpoint-connected");
    let resumed = second.event("sock", "session-resumed");
    assert_eq!(resumed["session"], opened["session"]);
    let (terminal, status, seen) = second.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    assert!(events(&seen, "sock", "launched").is_empty(), "no relaunch");
    let record = harness(&terminal, "sock");
    assert_eq!(record["exits"], json!(["code:0"]));
    let message = &record["messages"][0];
    assert_eq!(message["label"], "duplicate-unknown");
    assert_eq!(message["recovered"], true);
    let state = read_state(&dir.state("sock"));
    assert_eq!(
        state["launches"].as_array().unwrap().len(),
        1,
        "same process"
    );
    assert_eq!(state["insertions"].as_array().unwrap().len(), 1);
    assert!(!socket.exists(), "removed after the harness's observed end");
}

/// (i) Observed closures persist across an owner kill, so the cap is
/// reached across restarts and a further restart does not reset it. The
/// second closure is the surviving harness's exit when the restarted owner
/// attaches to it, as reported by its work PID 1.
#[test]
fn closures_and_outage_cap_persist_across_restart() {
    let dir = Scratch::new("cap");
    let modes = "exit-before-ack-always,insert-then-silent";
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            2,
            json!([{ "id": "flaky", "argv": peer(&dir.state("flaky"), &["--launch-modes", modes, "--on-reinit", "exit"]), "messages": ["x"] }]),
        ),
    );
    first.event("flaky", "closure-observed");
    let survivor = first.until("second launch", |value| {
        value["event"] == "launched" && value["launch"] == 2
    })["pid"]
        .as_u64()
        .unwrap();
    wait_state(&dir.state("flaky"), |state| {
        state["prompts"].as_array().unwrap().len() == 2
    });
    first.kill();
    assert!(alive(survivor));

    let (terminal, status, seen) = Run::start(&dir, &recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    let record = harness(&terminal, "flaky");
    assert_eq!(record["launches"], 0);
    assert_eq!(record["reattached"], 1);
    assert_eq!(record["exits"], json!(["code:1"]));
    let message = &record["messages"][0];
    assert_eq!(message["label"], "outage");
    assert_eq!(message["closures"], 2);
    assert_eq!(message["attempts"], 2);
    assert_eq!(message["prior_unknown"], 1);
    assert_eq!(events(&seen, "flaky", "outage")[0]["closures"], 2);
    assert!(!alive(survivor));

    let (terminal, status, seen) = Run::start(&dir, &recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    assert_eq!(custody(&seen)["outcome"], "fresh");
    assert_eq!(harness(&terminal, "flaky")["launches"], 0);
    assert_eq!(harness(&terminal, "flaky")["reattached"], 0);
    let message = &harness(&terminal, "flaky")["messages"][0];
    assert_eq!(message["label"], "outage");
    assert_eq!(
        message["closures"], 2,
        "recovered closures are the persisted ones"
    );
    assert_eq!(message["attempts"], 2);
    assert!(events(&seen, "flaky", "launched").is_empty());
    assert_eq!(
        read_state(&dir.state("flaky"))["launches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

/// (j) While an owner is live, a second instance for the same root is
/// refused before it writes anything; a different root proceeds and leaves
/// the first root's store untouched.
#[test]
fn second_owner_is_refused_and_other_root_is_unaffected() {
    let dir = Scratch::new("dup");
    let other = Scratch::new("other");
    let mut owner = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "quiet", "argv": peer(&dir.state("quiet"), &["--mode", "silent"]), "messages": ["x"] }]),
        ),
    );
    wait_state(&dir.state("quiet"), |state| {
        !state["prompts"].as_array().unwrap().is_empty()
    });
    let before = {
        let conn = db(&dir);
        (
            count(&conn, "SELECT count(*) FROM owner"),
            count(&conn, "SELECT count(*) FROM attempt"),
        )
    };
    assert_eq!(before, (1, 1));
    for request in [
        recover(&dir),
        spec(
            &dir,
            1,
            json!([{ "id": "q", "argv": ["/bin/false"], "messages": [] }]),
        ),
    ] {
        let mut duplicate = Run::start(&dir, &request);
        let first = duplicate.until("first line", |value| value["event"] != "intent-received");
        assert_eq!(first["event"], "terminal", "duplicate proceeded: {first}");
        let (terminal, status, seen) = duplicate.terminal();
        assert_eq!(terminal["status"], "store-refused");
        assert_eq!(terminal["reason"], "owner-live");
        assert_eq!(status.code(), Some(65));
        assert!(seen.iter().all(|value| value["event"] != "launched"));
    }

    let (terminal, status, _) = Run::start(&other, &spec(
        &other,
        1,
        json!([{ "id": "fine", "argv": peer(&other.state("fine"), &["--exit-after-acks", "1"]), "messages": ["y"] }]),
    ))
    .terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));

    let conn = db(&dir);
    assert_eq!(
        (
            count(&conn, "SELECT count(*) FROM owner"),
            count(&conn, "SELECT count(*) FROM attempt"),
            count(
                &conn,
                "SELECT count(*) FROM message WHERE ack_label IS NOT NULL"
            ),
        ),
        (1, 1, 0)
    );
    assert!(events(owner.drain_now(), "quiet", "authority-lost").is_empty());
    owner.cancel();
    let (terminal, status, _) = owner.terminal();
    assert_eq!(terminal["status"], "cancelled");
    assert_eq!(status.code(), Some(2));
    assert_eq!(terminal["owed_history"], "retained-in-store");
}

/// (k) A crash after interface acceptance and before the intent commit
/// leaves nothing durable: no `intent-committed` was reported, and a
/// recover request finds no intent. A foreign SQLite writer lock (held by
/// this test on its own fresh store) holds the commit back.
#[test]
fn interface_acceptance_before_commit_is_not_durable() {
    let dir = Scratch::new("precommit");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dir.store())
        .unwrap();
    let blocker = rusqlite::Connection::open(dir.store().join("intent.sqlite3")).unwrap();
    let mode: String = blocker
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    blocker.execute_batch("BEGIN EXCLUSIVE").unwrap();

    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &[]), "messages": ["x"] }]),
        ),
    );
    let accepted = run.until("intent-received", |value| {
        value["event"] == "intent-received"
    });
    assert_eq!(accepted["stage"], "accepted-by-interface");
    assert_eq!(accepted["durable"], false);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        run.drain_now()
            .iter()
            .all(|value| value["event"] != "intent-committed")
    );
    let seen = run.kill();
    assert!(
        seen.iter()
            .all(|value| value["event"] != "intent-committed")
    );
    blocker.execute_batch("ROLLBACK").unwrap();
    drop(blocker);

    let (terminal, status, _) = Run::start(&dir, &recover(&dir)).terminal();
    assert_eq!(terminal["status"], "store-refused");
    assert_eq!(terminal["reason"], "no-durable-intent");
    assert_eq!(status.code(), Some(65));
    assert!(!dir.state("h").exists(), "nothing was launched");
}

/// (l) An owner-restart loop with unresolved outcomes cannot buy unlimited
/// attempts: attempts persist across owner kills and count toward the
/// intent's delivery-attempt budget, separately from observed closures.
/// The quiet peer survives both owner kills and is resubmitted to by
/// reattachment. Once the budget is used up, a recovery stops the message
/// as `attempts-exhausted` with the unknown attempts still unknown and no
/// closure, outage or acknowledgement invented, and still holds the
/// surviving peer (never dropped as gone) until explicit cancel.
#[test]
fn owner_restart_loop_exhausts_persisted_attempt_budget() {
    let dir = Scratch::new("budget");
    let state = dir.state("quiet");
    let request = json!({
        "store": dir.store(),
        "intent": {
            "outage_closure_cap": 5,
            "delivery_attempt_cap": 2,
            "cwd": "/",
            "harnesses": [{ "id": "quiet", "argv": peer(&state, &["--mode", "insert-then-silent"]), "messages": ["x"] }],
        },
    });
    let mut survivor = None;
    for (round, request) in [request, recover(&dir)].into_iter().enumerate() {
        let mut owner = Run::start(&dir, &request);
        let event = if round == 0 { "launched" } else { "reattached" };
        let pid = owner.event("quiet", event)["pid"].as_u64().unwrap();
        assert_eq!(
            *survivor.get_or_insert(pid),
            pid,
            "the same surviving process"
        );
        wait_state(&state, |state| {
            state["prompts"].as_array().unwrap().len() == round + 1
        });
        owner.kill();
        assert!(alive(pid));
    }
    let survivor = survivor.unwrap();

    let mut run = Run::start(&dir, &recover(&dir));
    let first = run.until("first quiet event", |value| value["harness"] == "quiet");
    assert_eq!(first["event"], "nothing-deliverable", "{first}");
    run.event("quiet", "holding-survivor");
    assert!(
        alive(survivor),
        "exhausted budget does not drop or kill a survivor"
    );
    run.cancel();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert_eq!(status.code(), Some(2));
    assert!(events(&seen, "quiet", "launched").is_empty());
    let record = harness(&terminal, "quiet");
    assert_eq!(record["launches"], 0);
    assert_eq!(record["reattached"], 1);
    assert_eq!(record["exits"], json!(["signal:9"]));
    let message = &record["messages"][0];
    assert_eq!(message["state"], "owed");
    assert_eq!(message["label"], "attempts-exhausted");
    assert_eq!(message["attempts"], 2);
    assert_eq!(message["closures"], 0, "owner death is not a closure");
    assert!(!alive(survivor));

    let mut run = Run::start(&dir, &recover(&dir));
    let first = run.until("first quiet event", |value| value["harness"] == "quiet");
    assert_eq!(first["event"], "nothing-deliverable", "{first}");
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    assert_eq!(custody(&seen)["outcome"], "fresh");
    assert_eq!(harness(&terminal, "quiet")["launches"], 0);
    assert_eq!(read_state(&state)["launches"].as_array().unwrap().len(), 1);
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM attempt WHERE outcome = 'unknown-prior-owner'"
        ),
        2
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM attempt WHERE outcome LIKE 'ack:%'"
        ),
        0
    );
    assert_eq!(count(&conn, "SELECT closures FROM message"), 0);
}

/// (m) Within one owner, the budget counts attempts made by earlier
/// generations too: one unknown attempt from a killed owner plus two
/// observed-closure attempts use a budget of three. The survivor's exit at
/// reattachment is an observed closure but no new attempt.
#[test]
fn attempt_budget_counts_earlier_generations_before_sending() {
    let dir = Scratch::new("budget-run");
    let state = dir.state("flaky");
    let mut first = Run::start(
        &dir,
        &json!({
            "store": dir.store(),
            "intent": {
                "outage_closure_cap": 5,
                "delivery_attempt_cap": 3,
                "cwd": "/",
                "harnesses": [{ "id": "flaky", "argv": peer(&state, &["--launch-modes", "insert-then-silent,exit-before-ack-always", "--on-reinit", "exit"]), "messages": ["x"] }],
            },
        }),
    );
    let survivor = first.event("flaky", "launched")["pid"].as_u64().unwrap();
    wait_state(&state, |state| {
        !state["prompts"].as_array().unwrap().is_empty()
    });
    first.kill();
    assert!(alive(survivor));

    let (terminal, status, seen) = Run::start(&dir, &recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    let record = harness(&terminal, "flaky");
    assert_eq!(record["reattached"], 1);
    assert_eq!(record["launches"], 2);
    assert_eq!(record["exits"], json!(["code:1", "code:1", "code:1"]));
    let message = &record["messages"][0];
    assert_eq!(message["label"], "attempts-exhausted");
    assert_eq!(message["attempts"], 3);
    assert_eq!(message["prior_unknown"], 1);
    assert_eq!(message["closures"], 3);
    assert!(events(&seen, "flaky", "outage").is_empty());
    assert_eq!(events(&seen, "flaky", "attempts-exhausted").len(), 1);
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 3);
    assert_eq!(read_state(&state)["launches"].as_array().unwrap().len(), 3);
}

/// (n) While the owner is dead, one surviving peer exits on its own (status
/// 7) and another stays quiet. Its work PID 1, its actual parent, waited it;
/// root PID 1 kept that report. The restarted owner reports that exit as
/// exactly what was observed (`code:7`, by the work PID 1's wait), counts it
/// as a closure, relaunches with the same key, and reattaches to the quiet
/// survivor.
#[test]
fn peer_exit_while_owner_dead_is_reported_by_its_actual_waiter() {
    let dir = Scratch::new("waiter");
    let gate = dir.0.join("exit-now");
    let gate_arg = gate.display().to_string();
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([
                { "id": "ends", "argv": peer(&dir.state("ends"), &["--launch-modes", "insert-then-silent,normal", "--exit-when-file", &gate_arg, "--exit-after-acks", "1"]), "messages": ["one"] },
                { "id": "stays", "argv": quiet_until_reattach(&dir.state("stays"), &[]), "messages": ["two"] },
            ]),
        ),
    );
    let root_pid = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let ends = first.event("ends", "launched")["pid"].as_u64().unwrap();
    let stays = first.event("stays", "launched")["pid"].as_u64().unwrap();
    for id in ["ends", "stays"] {
        wait_state(&dir.state(id), |state| {
            state["insertions"].as_array().unwrap().len() == 1
        });
    }
    let ends_watch = watch(ends);
    first.kill();
    std::fs::write(&gate, b"").unwrap();
    wait_gone(&ends_watch);
    assert!(alive(stays));
    assert!(alive(root_pid));

    let (terminal, status, seen) = Run::start(&dir, &recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    let found = custody(&seen);
    assert_eq!(found["outcome"], "attached");
    assert_eq!(found["survivors"], 1);
    assert_eq!(found["receipts"], 1);
    let prior = events(&seen, "ends", "prior-exit");
    assert_eq!(prior.len(), 1);
    assert_eq!(prior[0]["status"], "code:7");
    assert_eq!(prior[0]["observer"], "work-pid1-wait");
    let closure = events(&seen, "ends", "closure-observed");
    assert_eq!(closure[0]["cause"], "exit-observed-while-owner-absent");
    let record = harness(&terminal, "ends");
    assert_eq!(record["prior_exits"], json!(["code:7"]));
    assert_eq!(record["reattached"], 0);
    assert_eq!(record["launches"], 1);
    assert_eq!(record["exits"], json!(["code:0"]));
    let message = &record["messages"][0];
    assert_eq!(message["state"], "acknowledged");
    assert_eq!(message["closures"], 1);
    assert_eq!(message["recovered"], true);
    let state = read_state(&dir.state("ends"));
    let prompts = state["prompts"].as_array().unwrap();
    assert_eq!(prompts[0]["key"], prompts[1]["key"]);
    let record = harness(&terminal, "stays");
    assert_eq!(record["reattached"], 1);
    assert_eq!(record["launches"], 0);
    assert_eq!(record["messages"][0]["state"], "acknowledged");
}

/// Kills this test's watched root PID 1 and waits it as its parent (it was
/// reparented here when its owner died). Returns the raw wait status.
fn kill_and_reap_root(dir: &Scratch, pid: i32) -> i32 {
    let fd = dir.root_fd(pid);
    // SAFETY: pidfd_send_signal on our verified root PID 1's pidfd.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    assert_eq!(rc, 0);
    let mut status = 0;
    // SAFETY: waitpid on one exact pid, our reparented child.
    assert_eq!(unsafe { libc::waitpid(pid, &raw mut status, 0) }, pid);
    status
}

/// (o) While the owner is dead, root PID 1 itself is killed (by this test,
/// its parent after reparenting, which alone sees its status). Its
/// namespace ends with it, taking the quiet peer. The restarted owner
/// finds the recorded incarnation absent and reports exactly that: no exit
/// observed, no status, and the peer ended with the root namespace, status
/// unknown. No closure is counted; nothing is fabricated. A new recorded
/// incarnation under the same root identity relaunches with the same key.
#[test]
fn root_pid1_death_while_owner_dead_is_unknown_not_fabricated() {
    let dir = Scratch::new("absent");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &["--launch-modes", "insert-then-silent,normal", "--exit-after-acks", "1"]), "messages": ["x"] }]),
        ),
    );
    let root_pid = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_i64()
        .unwrap();
    let peer_pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    wait_state(&dir.state("h"), |state| {
        state["insertions"].as_array().unwrap().len() == 1
    });
    let root_id = first.until("started", |value| value["event"] == "started")["root_id"].clone();
    first.kill();
    let status = kill_and_reap_root(&dir, i32::try_from(root_pid).unwrap());
    assert!(libc::WIFSIGNALED(status));
    assert!(!alive(peer_pid), "the peer ends with its root namespace");

    let (terminal, status, seen) = Run::start(&dir, &recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    let found = custody(&seen);
    assert_eq!(found["outcome"], "absent");
    assert_eq!(found["observed"], "absent-exit-not-observed");
    assert_eq!(found["root_id"], root_id, "same durable root identity");
    let unknown = events(&seen, "h", "prior-end-unknown");
    assert_eq!(unknown.len(), 1);
    assert_eq!(
        unknown[0]["meaning"],
        "ended-with-root-namespace-status-unknown"
    );
    assert!(
        events(&seen, "h", "prior-exit").is_empty(),
        "no fabricated exit"
    );
    let started = roots_started(&seen);
    assert_eq!(started.len(), 1);
    assert_eq!(started[0]["incarnation"], 2);
    let record = harness(&terminal, "h");
    assert_eq!(record["prior_unknown_ends"], 1);
    assert_eq!(record["prior_exits"], json!([]));
    assert_eq!(record["launches"], 1);
    assert_eq!(record["exits"], json!(["code:0"]));
    assert_eq!(
        record["messages"][0]["closures"], 0,
        "unknown end is not a closure"
    );
    assert_eq!(record["messages"][0]["state"], "acknowledged");
    let conn = db(&dir);
    let ended: String = conn
        .query_row("SELECT ended FROM incarnation WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(ended, "absent-exit-not-observed");
    let (outcome, observer): (String, Option<String>) = conn
        .query_row(
            "SELECT outcome, observer FROM work WHERE incarnation = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(outcome, "ended-with-root-namespace-status-unknown");
    assert_eq!(observer, None);
}

fn seqpacket(path: &Path) -> OwnedFd {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: plain socket/connect on a new descriptor and local address.
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0);
        assert!(fd >= 0);
        let fd = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let bytes = path.as_os_str().as_bytes();
        assert!(bytes.len() < addr.sun_path.len());
        for (slot, byte) in addr.sun_path.iter_mut().zip(bytes) {
            *slot = *byte as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1) as libc::socklen_t;
        assert_eq!(
            libc::connect(fd.as_raw_fd(), (&raw const addr).cast(), len),
            0
        );
        fd
    }
}

/// Sends one request to a root PID 1 custody socket and returns its reply.
fn probe(path: &Path, request: &Value) -> Value {
    let socket = seqpacket(path);
    let mut file = std::fs::File::from(socket);
    file.write_all(request.to_string().as_bytes()).unwrap();
    let mut buffer = vec![0u8; 65536];
    let read = std::io::Read::read(&mut file, &mut buffer).unwrap();
    serde_json::from_slice(&buffer[..read]).unwrap()
}

/// (p) Root PID 1 grants authority only by positive attribution: a peer
/// that does not present this incarnation's token from the root's private
/// store, or presents it with a generation not newer than the admitted
/// owner's, is refused, and the live owner is unaffected. Two roots are
/// independent: killing one root's owner leaves the other root's PID 1,
/// peers and owner untouched, and its recovery attaches only to its own
/// root PID 1.
#[test]
fn unattributed_peers_are_refused_and_roots_are_independent() {
    let a = Scratch::new("scope-a");
    let b = Scratch::new("scope-b");
    let mut owner_a = Run::start(
        &a,
        &spec(
            &a,
            1,
            json!([{ "id": "quiet", "argv": peer(&a.state("quiet"), &["--mode", "silent"]), "messages": ["x"] }]),
        ),
    );
    let mut owner_b = Run::start(
        &b,
        &spec(
            &b,
            1,
            json!([{ "id": "quiet", "argv": quiet_until_reattach(&b.state("quiet"), &[]), "messages": ["y"] }]),
        ),
    );
    let root_a = owner_a.until("root a", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let root_b = owner_b.until("root b", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    assert_ne!(root_a, root_b);
    let peer_a = owner_a.event("quiet", "launched")["pid"].as_u64().unwrap();
    let peer_b = owner_b.event("quiet", "launched")["pid"].as_u64().unwrap();
    wait_state(&a.state("quiet"), |state| {
        !state["prompts"].as_array().unwrap().is_empty()
    });
    wait_state(&b.state("quiet"), |state| {
        state["insertions"].as_array().unwrap().len() == 1
    });

    let socket = a.store().join("pid1-1.sock");
    let token: String = db(&a)
        .query_row("SELECT token FROM incarnation WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let refusals = [
        (
            json!({ "op": "spawn", "work": "w99", "argv": ["/bin/true"] }),
            "peer-unattributed",
        ),
        (
            json!({ "op": "hello", "token": "0".repeat(32), "generation": 99 }),
            "peer-unattributed",
        ),
        (
            json!({ "op": "hello", "token": token, "generation": 1 }),
            "stale-generation",
        ),
    ];
    for (request, reason) in refusals {
        let reply = probe(&socket, &request);
        assert_eq!(reply["event"], "refused", "{request}: {reply}");
        assert_eq!(reply["reason"], reason, "{request}");
    }
    assert!(
        owner_a
            .drain_now()
            .iter()
            .all(|value| value["event"] != "detached")
    );
    assert_eq!(
        read_state(&a.state("quiet"))["launches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    owner_b.kill();
    assert!(
        alive(root_a) && alive(peer_a),
        "root a is untouched by root b's owner death"
    );
    assert!(alive(root_b) && alive(peer_b));
    let (terminal, status, seen) = Run::start(&b, &recover(&b)).terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    assert_eq!(custody(&seen)["outcome"], "attached");
    assert_eq!(events(&seen, "quiet", "reattached")[0]["pid"], peer_b);
    assert!(alive(root_a) && alive(peer_a));

    owner_a.cancel();
    let (terminal, status, _) = owner_a.terminal();
    assert_eq!(terminal["status"], "cancelled");
    assert_eq!(status.code(), Some(2));
    assert_eq!(harness(&terminal, "quiet")["exits"], json!(["signal:9"]));
}

/// (q) A recorded incarnation is adopted only if the process at its pid is
/// still exactly it (start time and boot id). Here the store's recorded
/// start time no longer matches the running root PID 1 (as after pid
/// reuse). The restarted owner neither adopts nor signals that process:
/// it treats the recorded incarnation as not running, starts a new
/// incarnation, and relaunches. The unadopted process and its peer are
/// left alive (this test's cleanup ends them).
#[test]
fn mismatched_incarnation_is_neither_adopted_nor_signalled() {
    let dir = Scratch::new("identity");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &["--launch-modes", "insert-then-silent,normal", "--exit-after-acks", "1"]), "messages": ["x"] }]),
        ),
    );
    let old_root = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let old_peer = first.event("h", "launched")["pid"].as_u64().unwrap();
    wait_state(&dir.state("h"), |state| {
        state["insertions"].as_array().unwrap().len() == 1
    });
    first.kill();
    rusqlite::Connection::open(dir.store().join("intent.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE incarnation SET start_time = start_time + 1 WHERE id = 1",
            [],
        )
        .unwrap();

    let mut second = Run::start(&dir, &recover(&dir));
    let found = second.until("custody", |value| value["event"] == "custody");
    assert_eq!(found["outcome"], "absent", "{found}");
    assert_eq!(found["observed"], "recorded-process-not-running");
    let (terminal, status, seen) = second.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    let record = harness(&terminal, "h");
    assert_eq!(record["reattached"], 0, "never adopted");
    assert_eq!(record["prior_unknown_ends"], 1);
    assert_eq!(record["launches"], 1);
    let started = roots_started(&seen);
    assert_eq!(started.len(), 1);
    assert_ne!(started[0]["pid"], old_root);
    assert!(alive(old_root), "unadopted process is not signalled");
    assert!(alive(old_peer));
}

/// Waits up to `bound` for this test's own supervisor to exit by itself.
/// `None` means it was still running: a bounded observation of a wait that
/// has not returned, not by itself a reason it waits.
fn exits_within(run: &mut Run, bound: Duration) -> Option<ExitStatus> {
    let deadline = std::time::Instant::now() + bound;
    loop {
        if let Some(status) = run.child.try_wait().unwrap() {
            return Some(status);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Supersedes this root's current owner the way a newer owner does: a
/// hello on root PID 1's custody socket with the incarnation's token from
/// the root's private store and a newer generation.
///
/// CONFIGURED, not a production actor: no current producer supersedes a
/// live owner (the cooperative store lock refuses a second normal claim,
/// and nothing here bypasses it). This test process is the stand-in. It
/// does not touch the store or its generation fence, and it closes its
/// connection at once, leaving root PID 1 with its live work and no owner.
fn supersede(dir: &Scratch, generation: i64) {
    let token: String = db(dir)
        .query_row("SELECT token FROM incarnation WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    let reply = probe(
        &dir.store().join("pid1-1.sock"),
        &json!({ "op": "hello", "token": token, "generation": generation }),
    );
    assert_eq!(reply["event"], "attached", "{reply}");
}

fn incarnation_ended(dir: &Scratch) -> Option<String> {
    db(dir)
        .query_row("SELECT ended FROM incarnation WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap()
}

/// (r) O1, outgoing parent. The owner that started root PID 1 has its only
/// message durably acknowledged and its harness quiet (alive, nothing more
/// to say), so no further store write could reveal a lost fence. A newer
/// owner supersedes it (configured, see `supersede`). The outgoing owner
/// must treat that as authority loss at run level and return promptly,
/// leaving the root and its quiet work to the successor: nothing killed, no
/// wait for the root's end, no ended incarnation.
#[test]
fn superseded_parent_owner_with_acknowledged_quiet_work_returns_and_kills_nothing() {
    let dir = Scratch::new("sup-p");
    let mut owner = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &[]), "messages": ["x"] }]),
        ),
    );
    let root = owner.until("root pid1", |value| value["event"] == "root-pid1-started");
    assert_eq!(root["parent"], "this-owner");
    let root_pid = root["pid"].as_u64().unwrap();
    let peer_pid = owner.event("h", "launched")["pid"].as_u64().unwrap();
    owner.event("h", "ack");
    owner.event("h", "idle");

    supersede(&dir, 2);
    owner.event("h", "detached");
    let exited = exits_within(&mut owner, Duration::from_secs(5));
    assert!(
        exited.is_some(),
        "superseded parent owner still running 5 s after detaching: it waits on \
         work it no longer holds\nseen: {:#?}",
        owner.drain_now()
    );
    let (terminal, status, seen) = owner.terminal();
    assert_eq!(terminal["status"], "authority-lost", "{terminal}");
    assert_eq!(status.code(), Some(5));
    assert_eq!(terminal["root_pid1"]["outcome"], "left-to-successor");
    assert!(terminal["root_pid1"]["status"].is_null());
    let record = harness(&terminal, "h");
    assert_eq!(record["detached"], 1);
    assert_eq!(record["exits"], json!([]), "no fabricated end");
    assert_eq!(record["wait_failures"], json!([]));
    assert_eq!(record["messages"][0]["state"], "acknowledged");
    assert!(events(&seen, "h", "exited").is_empty());
    assert!(
        seen.iter()
            .all(|value| value["event"] != "cancel-requested")
    );
    // Quiet survival: the successor's work was neither killed nor ended.
    std::thread::sleep(QUIET_WINDOW);
    assert!(alive(root_pid), "root PID 1 survives its superseded parent");
    assert!(alive(peer_pid), "quiet work survives its superseded owner");
    assert_eq!(
        read_state(&dir.state("h"))["launches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(incarnation_ended(&dir), None, "no end was observed");
}

/// (s) O1 for a recovered owner, and O3. A restarted owner attached to the
/// surviving root PID 1 (not its parent) is holding a survivor whose
/// message is already acknowledged. A newer owner supersedes it
/// (configured). It leaves the root to the successor at run level, and its
/// missing release reply is not recorded as an ended incarnation: the next
/// recovery still finds the possibly-living incarnation and, since it
/// cannot attach (the stand-in holds a generation it does not exceed),
/// reports it owned-unattached rather than inferring it gone and starting
/// a new one.
#[test]
fn superseded_recovered_owner_keeps_its_incarnation_discoverable() {
    let dir = Scratch::new("sup-r");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &[]), "messages": ["x"] }]),
        ),
    );
    let root_pid = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let peer_pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    first.event("h", "ack");
    first.kill();

    let mut second = Run::start(&dir, &recover(&dir));
    assert_eq!(
        second.until("custody", |value| value["event"] == "custody")["outcome"],
        "attached"
    );
    second.event("h", "holding-survivor");
    supersede(&dir, 3);
    second.event("h", "detached");
    let (terminal, status, _) = second.terminal();
    assert_eq!(terminal["status"], "authority-lost", "{terminal}");
    assert_eq!(status.code(), Some(5));
    assert_eq!(terminal["root_pid1"]["outcome"], "left-to-successor");
    assert_eq!(harness(&terminal, "h")["exits"], json!([]));
    assert!(alive(root_pid) && alive(peer_pid), "nothing killed");
    assert_eq!(
        incarnation_ended(&dir),
        None,
        "a missing release reply is not an observed end"
    );

    let (terminal, status, seen) = Run::start(&dir, &recover(&dir)).terminal();
    let found = custody(&seen);
    assert_eq!(found["outcome"], "owned-unattached", "{found}");
    assert_eq!(found["incarnation"], 1);
    assert!(roots_started(&seen).is_empty(), "never inferred gone");
    assert_eq!(terminal["status"], "owned-unattached");
    assert_eq!(status.code(), Some(4));
    assert!(alive(root_pid) && alive(peer_pid));
    assert_eq!(incarnation_ended(&dir), None);
}

/// (t) O2. While the owner is dead, a work PID 1 is killed before it can
/// report its harness's wait (this test signals that exact process through
/// a pidfd it verified is root PID 1's child holding that harness; the
/// harness ends with its namespace). Root PID 1 reports the work PID 1's
/// own status, without a harness status. The restarted owner records that
/// as an unknown end, not a closure: with a closure cap of 1 the message is
/// not declared an outage; it is relaunched with the same key and
/// acknowledged.
#[test]
fn work_pid1_end_without_harness_wait_is_unknown_not_a_closure() {
    let dir = Scratch::new("nowait");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([
                { "id": "ends", "argv": peer(&dir.state("ends"), &["--launch-modes", "insert-then-silent,normal", "--exit-after-acks", "1"]), "messages": ["one"] },
                { "id": "stays", "argv": quiet_until_reattach(&dir.state("stays"), &[]), "messages": ["two"] },
            ]),
        ),
    );
    let root_pid = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let ends = first.event("ends", "launched")["pid"].as_u64().unwrap();
    let stays = first.event("stays", "launched")["pid"].as_u64().unwrap();
    for id in ["ends", "stays"] {
        wait_state(&dir.state(id), |state| {
            state["insertions"].as_array().unwrap().len() == 1
        });
    }
    let works = children(root_pid);
    let target = *works
        .iter()
        .find(|work| children(**work) == vec![ends])
        .expect("work PID 1 of the ends harness");
    let work_fd = pidfd(i32::try_from(target).unwrap()).unwrap();
    // Verified after opening, so the pidfd names that exact work PID 1.
    assert_eq!(ppid(target), root_pid);
    assert_eq!(children(target), vec![ends]);
    first.kill();
    // SAFETY: pidfd_send_signal on the verified work PID 1's pidfd.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            work_fd.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    assert_eq!(rc, 0);
    let deadline = std::time::Instant::now() + WATCHDOG;
    while children(root_pid).contains(&target) {
        assert!(std::time::Instant::now() < deadline, "watchdog: root reap");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!alive(ends), "the harness ended with its work namespace");
    assert!(alive(stays) && alive(root_pid));

    let (terminal, status, seen) = Run::start(&dir, &recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    let found = custody(&seen);
    assert_eq!(found["outcome"], "attached");
    assert_eq!(found["receipts"], 1);
    let unknown = events(&seen, "ends", "prior-end-unknown");
    assert_eq!(unknown.len(), 1, "{seen:#?}");
    assert_eq!(
        unknown[0]["meaning"],
        "ended-with-work-namespace-status-unknown"
    );
    assert_eq!(unknown[0]["work_pid1"], "signal:9");
    assert!(events(&seen, "ends", "prior-exit").is_empty());
    assert!(events(&seen, "ends", "closure-observed").is_empty());
    let record = harness(&terminal, "ends");
    assert_eq!(
        record["prior_exits"],
        json!([]),
        "no harness status invented"
    );
    assert_eq!(record["prior_unknown_ends"], 1);
    assert_eq!(record["launches"], 1);
    assert_eq!(record["exits"], json!(["code:0"]));
    let message = &record["messages"][0];
    assert_eq!(message["closures"], 0, "no harness wait, no closure");
    assert_eq!(message["state"], "acknowledged");
    let state = read_state(&dir.state("ends"));
    let prompts = state["prompts"].as_array().unwrap();
    assert_eq!(prompts[0]["key"], prompts[1]["key"]);
    let (outcome, observer): (String, Option<String>) = db(&dir)
        .query_row(
            "SELECT outcome, observer FROM work
             WHERE incarnation = 1 AND harness = 0 ORDER BY id LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(outcome, "ended-with-work-namespace-status-unknown");
    assert_eq!(observer, None);
    assert_eq!(harness(&terminal, "stays")["reattached"], 1);
}

fn owner_event<'a>(seen: &'a [Value], event: &str) -> Vec<&'a Value> {
    seen.iter()
        .filter(|value| value["event"] == event)
        .collect()
}

/// U7: Bash work started inside a harness's own namespace reaches that
/// root's owner through the ingress named in its environment, and only
/// there. It is accepted only after a durable record, attributed to the
/// requesting harness and the one owner input open on it, run as its own
/// work under root PID 1 (pid 2 of a new namespace), and its output and its
/// end, from its work PID 1's wait, come back to the requester; the
/// harness's answer names that input as its native parent.
#[test]
fn in_root_bash_reaches_its_own_owner_with_attributed_output_and_waited_end() {
    let dir = Scratch::new("bash-own");
    let run = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{
                "id": "a",
                "argv": peer(&dir.state("a"), &["--exit-after-acks", "1"]),
                "messages": ["bash:echo pid=$$; echo err >&2; exit 3"],
            }]),
        ),
    );
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        terminal["bash"],
        json!({ "accepted": 1, "refused": 0, "not_run": 0, "ended": 1, "end_unknown": 0, "open": 0 })
    );
    let listening = owner_event(&seen, "bash-ingress")[0];
    assert_eq!(listening["listening"], true);
    let accepted = owner_event(&seen, "bash-accepted")[0];
    assert_eq!(accepted["harness"], "a");
    assert_eq!(
        accepted["input_attribution"], "single-open-input",
        "{accepted}"
    );
    assert_eq!(accepted["inputs_open"][0]["index"], 0);
    assert_eq!(accepted["argv"][0], "/bin/sh");
    let ended = owner_event(&seen, "bash-ended")[0];
    assert_eq!(ended["bash"], "end", "{ended}");
    assert_eq!(ended["status"], "code:3");
    assert_eq!(ended["observer"], "work-pid1-wait");
    assert_eq!(ended["requester"], "connected");
    // What the requester got back: its own exit code and the output (stderr
    // joined), from a process that is pid 2 of its own new namespace.
    let answer = events(&seen, "a", "agent-message")[0];
    let text = answer["text"].as_str().unwrap();
    assert!(text.contains("exit=Some(3)"), "{text}");
    assert!(text.contains("stdout=pid=2\nerr\n"), "{text}");
    assert!(
        text.contains("\"input_attribution\":\"single-open-input\""),
        "{text}"
    );
    assert_eq!(answer["input"], 0);
    assert_eq!(answer["input_attribution"], "native-parent");
    let ack = events(&seen, "a", "ack")[0];
    assert_eq!(answer["parent_message_id"], ack["message_id"]);
    let turn = events(&seen, "a", "turn-end");
    assert_eq!(turn.len(), 1);
    assert_eq!(turn[0]["input"], 0);
    assert_eq!(turn[0]["own_output"], true);
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM work w JOIN bash_run b ON b.work = w.id
             WHERE w.kind = 'bash' AND w.outcome = 'code:3' AND w.observer = 'work-pid1-wait'"
        ),
        1
    );
}

/// U7 refusal: a process outside every harness namespace of the root (this
/// test) is refused visibly and nothing is recorded or run; there is no
/// fallback. The refusal is reported by the owner.
#[test]
fn bash_from_outside_every_harness_namespace_is_refused_and_nothing_runs() {
    let dir = Scratch::new("bash-out");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "a", "argv": peer(&dir.state("a"), &["--mode", "silent"]), "messages": ["hi"] }]),
        ),
    );
    let listening = run.until("bash ingress", |value| value["event"] == "bash-ingress");
    run.event("a", "launched");
    let marker = dir.0.join("ran");
    let mut stream =
        std::os::unix::net::UnixStream::connect(listening["path"].as_str().unwrap()).unwrap();
    // The owner may refuse and close before reading this: not an error.
    let _ = writeln!(
        stream,
        "{}",
        json!({ "v": 1, "op": "run", "argv": ["/bin/touch", marker], "cwd": "/" })
    );
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply).unwrap();
    let reply: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(reply["event"], "refused");
    assert_eq!(
        reply["reason"],
        "peer-unattributed: outside-every-harness-namespace"
    );
    let refused = run.until("bash refused", |value| value["event"] == "bash-refused");
    assert_eq!(refused["peer"]["pid"], std::process::id());
    run.cancel();
    let (terminal, _, seen) = run.terminal();
    assert_eq!(terminal["bash"]["refused"], 1, "{terminal}");
    assert_eq!(terminal["bash"]["accepted"], 0);
    assert!(owner_event(&seen, "bash-accepted").is_empty());
    assert!(!marker.exists(), "a refused request runs nothing");
    assert_eq!(
        count(&db(&dir), "SELECT count(*) FROM work WHERE kind = 'bash'"),
        0
    );
}

/// Per-input attribution: with immediate supply, a turn end is reported for
/// an input only from an idle the agent tagged at or after it, and says
/// whether any output named it. An untagged idle stays readiness only.
#[test]
fn turn_end_is_per_input_only_from_tagged_idle() {
    for tagged in [true, false] {
        let dir = Scratch::new("turns");
        let mut extra = vec!["--exit-after-acks", "2"];
        if !tagged {
            extra.push("--untagged");
        }
        let run = Run::start(
            &dir,
            &spec(
                &dir,
                3,
                json!([{ "id": "a", "argv": peer(&dir.state("a"), &extra), "messages": ["one", "two"] }]),
            ),
        );
        let (terminal, _, seen) = run.terminal();
        assert_eq!(terminal["status"], "ended", "{terminal}");
        let turns = events(&seen, "a", "turn-end");
        assert!(!events(&seen, "a", "idle").is_empty());
        if tagged {
            let inputs: Vec<&Value> = turns.iter().map(|turn| &turn["input"]).collect();
            assert_eq!(inputs, [&json!(0), &json!(1)]);
            assert!(turns.iter().all(|turn| turn["own_output"] == false));
        } else {
            assert!(turns.is_empty(), "untagged idle is no input's end");
        }
    }
}

/// U7 across owner death: a Bash run is its own work under root PID 1, so
/// the owner's death kills nothing. Its requester learns only that its end
/// is unknown (never success). A restarted owner reattaches the run, waits
/// for its end from its actual waiter and records it, with the requester
/// reported lost with the earlier owner.
#[test]
fn bash_run_survives_owner_death_and_requester_sees_unknown_not_success() {
    let dir = Scratch::new("bash-survive");
    let gate = dir.0.join("gate");
    let command = format!(
        "bash:while [ ! -e {} ]; do sleep 0.05; done; echo done",
        gate.display()
    );
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "a", "argv": peer(&dir.state("a"), &["--exit-after-acks", "1"]), "messages": [command] }]),
        ),
    );
    let started = first.until("bash started", |value| value["event"] == "bash-started");
    let bash_pid = started["pid"].as_u64().unwrap();
    let seen = first.kill();
    assert!(owner_event(&seen, "bash-ended").is_empty());
    // The requester saw its connection end without a final line.
    wait_state(&dir.state("a"), |state| state["bash"].is_array());
    let result = read_state(&dir.state("a"))["bash"][0].clone();
    let result = result.as_str().unwrap();
    assert!(result.contains("exit=Some(75)"), "{result}");
    assert!(result.contains("accepted-end-unknown"), "{result}");
    std::thread::sleep(QUIET_WINDOW);
    assert!(alive(bash_pid), "a Bash run survives its owner");

    let mut second = Run::start(&dir, &recover(&dir));
    let reattached = second.until("bash reattached", |value| {
        value["event"] == "bash-reattached"
    });
    assert_eq!(reattached["pid"].as_u64(), Some(bash_pid));
    std::fs::write(&gate, b"").unwrap();
    let (terminal, status, seen) = second.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
    let ended = owner_event(&seen, "bash-ended")[0];
    assert_eq!(ended["bash"], "end", "{ended}");
    assert_eq!(ended["status"], "code:0");
    assert_eq!(ended["observer"], "work-pid1-wait");
    assert_eq!(ended["requester"], "lost-with-prior-owner");
    assert_eq!(terminal["bash"]["ended"], 1);
    assert_eq!(
        count(
            &db(&dir),
            "SELECT count(*) FROM work WHERE kind = 'bash' AND outcome = 'code:0'"
        ),
        1
    );
}

/// A recover request naming its purpose.
fn recover_for(dir: &Scratch, purpose: &str) -> Value {
    json!({ "store": dir.store(), "recover": purpose })
}

/// A cancel recovery of a surviving root: the owner is SIGKILLed while one
/// delivery is inserted but unanswered; the `cancel` recovery attaches the
/// same root PID 1, has the survivor killed by its own work PID 1 and never
/// connects to it, so nothing is resumed or resubmitted (the peer still has
/// exactly one prompt) and nothing is launched. The message stays owed in
/// the store. Root PID 1 is released and, not being this owner's child,
/// only its exit is observed. A later `continue-attached` recovery then
/// finds no root to attach and starts nothing: `root-absent`, the debt
/// still reported.
#[test]
fn cancel_recovery_kills_the_survivor_and_delivers_nothing() {
    let dir = Scratch::new("cancelrec");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": quiet_until_reattach(&dir.state("h"), &[]), "messages": ["x"] }]),
        ),
    );
    let root_pid = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let peer_pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    wait_state(&dir.state("h"), |state| {
        state["insertions"].as_array().unwrap().len() == 1
    });
    first.kill();
    assert!(
        alive(root_pid) && alive(peer_pid),
        "owner death kills nothing"
    );

    let (terminal, status, seen) = Run::start(&dir, &recover_for(&dir, "cancel")).terminal();
    assert_eq!(custody(&seen)["outcome"], "attached");
    let cancel = seen
        .iter()
        .find(|value| value["event"] == "cancel-requested")
        .unwrap();
    assert_eq!(cancel["by"], "recover-cancel");
    assert_eq!(events(&seen, "h", "reattached").len(), 1);
    for nothing in [
        "endpoint-connected",
        "negotiated",
        "session-resumed",
        "session-opened",
        "ack",
        "launched",
    ] {
        assert!(
            events(&seen, "h", nothing).is_empty(),
            "{nothing}: {seen:#?}"
        );
    }
    assert!(roots_started(&seen).is_empty(), "no new incarnation");
    assert_eq!(status.code(), Some(i32::from(2u8)), "{terminal}");
    assert_eq!(terminal["status"], "cancelled");
    assert_eq!(terminal["recover"], "cancel");
    assert_eq!(terminal["all_harnesses_reaped"], true, "{terminal}");
    let record = harness(&terminal, "h");
    assert_eq!(record["exits"], json!(["signal:9"]), "{record}");
    assert_eq!(record["messages"][0]["state"], "owed");
    assert_eq!(record["messages"][0]["label"], "cancelled");
    assert_eq!(record["messages"][0]["prior_unknown"], 1);
    assert_eq!(
        read_state(&dir.state("h"))["prompts"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "nothing resubmitted under cancel"
    );
    assert!(!alive(peer_pid));
    assert_eq!(
        terminal["root_pid1"]["outcome"],
        "released-exit-observed-by-pidfd"
    );
    assert_eq!(terminal["root_pid1"]["parent"], false);
    assert!(terminal["root_pid1"]["status"].is_null());

    let (terminal, status, seen) =
        Run::start(&dir, &recover_for(&dir, "continue-attached")).terminal();
    assert_eq!(status.code(), Some(6), "{terminal}");
    assert_eq!(terminal["status"], "root-absent");
    assert_eq!(terminal["new_incarnation"], "not-started");
    assert_eq!(terminal["owed"], 1);
    let limited = seen
        .iter()
        .find(|value| value["event"] == "recover-limited")
        .unwrap();
    assert_eq!(limited["custody"], "no-unended-incarnation", "{limited}");
    assert!(roots_started(&seen).is_empty());
    assert!(events(&seen, "h", "launched").is_empty());
    assert_eq!(
        harness(&terminal, "h")["messages"][0]["label"],
        "root-absent"
    );
    let conn = db(&dir);
    assert_eq!(count(&conn, "SELECT count(*) FROM incarnation"), 1);
}

/// A purposeful recovery whose recorded root PID 1 is gone (killed while
/// no owner was attached) reports the absence as observed and the peer's
/// end as unknown, launches nothing and starts no incarnation, unlike the
/// plain recovery (see `root_pid1_death_while_owner_dead_is_unknown_not_fabricated`).
#[test]
fn purposeful_recovery_of_an_absent_root_starts_nothing() {
    let dir = Scratch::new("absentrec");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": quiet_until_reattach(&dir.state("h"), &[]), "messages": ["x"] }]),
        ),
    );
    let root_pid = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_i64()
        .unwrap();
    wait_state(&dir.state("h"), |state| {
        state["insertions"].as_array().unwrap().len() == 1
    });
    first.kill();
    let status = kill_and_reap_root(&dir, i32::try_from(root_pid).unwrap());
    assert!(libc::WIFSIGNALED(status));

    let (terminal, status, seen) = Run::start(&dir, &recover_for(&dir, "cancel")).terminal();
    assert_eq!(status.code(), Some(6), "{terminal}");
    assert_eq!(terminal["status"], "root-absent");
    assert_eq!(custody(&seen)["outcome"], "absent");
    assert_eq!(custody(&seen)["observed"], "absent-exit-not-observed");
    assert_eq!(events(&seen, "h", "prior-end-unknown").len(), 1);
    assert!(events(&seen, "h", "launched").is_empty());
    assert!(roots_started(&seen).is_empty());
    assert!(
        seen.iter()
            .all(|value| value["event"] != "cancel-requested")
    );
    assert_eq!(terminal["cancel_requested"], false);
    assert_eq!(terminal["owed"], 1);
    assert!(terminal["root_pid1"]["outcome"] == "none", "{terminal}");
    let conn = db(&dir);
    assert_eq!(count(&conn, "SELECT count(*) FROM incarnation"), 1);
}

#[test]
fn recover_purpose_with_an_intent_is_refused() {
    let dir = Scratch::new("recintent");
    let mut request = spec(
        &dir,
        1,
        json!([{ "id": "h", "argv": ["x"], "messages": [] }]),
    );
    request["recover"] = json!("cancel");
    let (terminal, status, _) = Run::start(&dir, &request).terminal();
    assert_eq!(status.code(), Some(64), "{terminal}");
    assert_eq!(terminal["status"], "spec-refused");
    assert!(!dir.store().exists(), "nothing written");
}
