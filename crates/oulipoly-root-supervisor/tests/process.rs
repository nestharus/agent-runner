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

/// The isolation every owner here declares: these tests run unprivileged
/// (a root owner refuses the declaration before launching anything).
fn expected_isolation() -> &'static str {
    "unprivileged-userns-pidns"
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
        self.0
            .canonicalize()
            .unwrap()
            .join(format!("{harness}.json"))
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
        Self::start_in(dir, spec, &[SUPERVISOR.to_owned()])
    }

    /// Starts the supervisor through `argv`, which must end by exec'ing it
    /// (so that this test's child is the supervisor itself).
    fn start_in(dir: &Scratch, spec: &Value, argv: &[String]) -> Self {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]);
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

    /// Writes one raw control line.
    fn control(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
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
            "workload": { "isolation": "unprivileged-userns" },
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
            "workload": { "isolation": "unprivileged-userns" },
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
                "workload": { "isolation": "unprivileged-userns" },
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
        json!({ "accepted": 1, "refused": 0, "not_run": 0, "ended": 1, "end_unknown": 0, "open": 0, "output_open": 0, "output_requests": 0 })
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
    for request in [
        json!({ "v": 1, "op": "run", "argv": ["/bin/touch", marker], "cwd": "/" }),
        json!({ "v": 1, "op": "output", "root_id": "supplied-marker", "work": 2 }),
        json!({ "v": 1, "op": "accept", "root_id": "supplied-marker", "work": 2, "bytes": 0, "sha256": "0".repeat(64) }),
    ] {
        let mut stream =
            std::os::unix::net::UnixStream::connect(listening["path"].as_str().unwrap()).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let _ = writeln!(stream, "{request}");
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply).unwrap();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["event"], "refused");
        assert_eq!(
            reply["reason"],
            "peer-unattributed: outside-every-harness-namespace"
        );
    }
    let refused = run.until("bash refused", |value| value["event"] == "bash-refused");
    assert_eq!(refused["peer"]["pid"], std::process::id());
    run.cancel();
    let (terminal, _, seen) = run.terminal();
    assert_eq!(terminal["bash"]["refused"], 3, "{terminal}");
    assert_eq!(terminal["bash"]["accepted"], 0);
    assert!(owner_event(&seen, "bash-accepted").is_empty());
    assert!(owner_event(&seen, "bash-output-accepted").is_empty());
    assert_eq!(
        count(&db(&dir), "SELECT count(*) FROM bash_output_accept"),
        0
    );
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
    assert_eq!(ended["retained"]["state"], "partial");
    assert_eq!(ended["retained"]["received"], Value::Null);
    assert_eq!(ended["retained"]["bytes"], 5);
    assert_eq!(ended["retained"]["losses"][0]["reason"], "owner-changed");
    let stored: (String, Option<i64>, i64) = db(&dir)
        .query_row(
            "SELECT state, received, retained FROM bash_output WHERE work = ?1",
            [ended["work"].as_i64().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(stored, ("partial".into(), None, 5));
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

/// Owner-level controls whose `control` number and `event` match.
fn by_control<'a>(seen: &'a [Value], event: &str, control: u64) -> Vec<&'a Value> {
    seen.iter()
        .filter(|value| value["event"] == event && value["control"] == control)
        .collect()
}

/// The peer emits both reply and tagged idle before each insertion ACK.
/// Correlation must use that ACK's native identity, and the ended input
/// must allow another live input and close rather than remaining open.
#[test]
fn turn_before_ack_preserves_correlation_follow_up_and_close() {
    let dir = Scratch::new("early");
    let state = dir.state("a");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{
                "id": "a", "argv": peer(&state, &["--turn-before-ack"]), "messages": ["one"]
            }]),
        ),
    );
    let mut ids = Vec::new();
    for (index, text) in [(0, "one"), (1, "two")] {
        if index == 1 {
            run.control(&json!({ "cmd": "send", "text": text }).to_string());
            assert_eq!(run.event("a", "follow-up-admitted")["input"], index);
        }
        // Read any reply, including a wrongly unattributed one: the old
        // ordering fails here immediately instead of hiding behind a wait.
        let reply = run.until("early reply", |value| {
            value["event"] == "agent-message" && value["text"] == text
        });
        assert_eq!(reply["input"], index, "{reply}");
        assert_eq!(reply["input_attribution"], "native-parent");
        let ack = run.until("insertion ACK", |value| {
            value["event"] == "ack" && value["index"] == index
        });
        let turn = run.until("early tagged turn end", |value| {
            value["event"] == "turn-end" && value["input"] == index
        });
        assert_eq!(ack["durable"], true);
        assert_eq!(reply["parent_message_id"], ack["message_id"]);
        assert_eq!(turn["message_id"], ack["message_id"]);
        assert_eq!(turn["last_user_message_id"], ack["message_id"]);
        assert_eq!(turn["session"], "sess-1");
        assert_eq!(turn["own_output"], true);
        ids.push(ack["message_id"].clone());
    }
    assert_ne!(ids[0], ids[1]);
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert_eq!(terminal["status"], "closed");
    assert_eq!(terminal["owed"], 0);
    assert_eq!(terminal["cancel_requested"], false);
    assert_eq!(terminal["all_harnesses_reaped"], true);
    let a = harness(&terminal, "a");
    assert_eq!(a["launches"], 1);
    assert_eq!(a["exits"], json!(["signal:9"]));
    for message in a["messages"].as_array().unwrap() {
        assert_eq!(message["completion"], "not-observed");
    }
    assert_eq!(events(&seen, "a", "agent-message").len(), 2);
    assert_eq!(events(&seen, "a", "turn-end").len(), 2);
    assert!(events(&seen, "a", "follow-up-refused").is_empty());
    let peer = read_state(&state);
    assert_eq!(peer["sessions"], json!(["sess-1"]));
    assert_eq!(peer["prompts"].as_array().unwrap().len(), 2);
}

/// Colliding tags from a historical or never-opened session cannot end
/// this worker's input or enable its next admission. A later genuine idle
/// must release it, without counting the other session's reply as its output.
#[test]
fn prior_session_evidence_cannot_release_current_input() {
    off_session_evidence_cannot_release_current_input("sess-prior", false);
}

#[test]
fn unknown_session_evidence_cannot_release_current_input() {
    off_session_evidence_cannot_release_current_input("sess-unknown", true);
}

fn off_session_evidence_cannot_release_current_input(other: &str, after_ack: bool) {
    let dir = Scratch::new("session-fence");
    let state = dir.state("a");
    if other == "sess-prior" {
        std::fs::write(
            &state,
            json!({
                "launches": [], "sessions": [other], "prompts": [],
                "insertions": [], "fault_used": false,
            })
            .to_string(),
        )
        .unwrap();
    }
    let gate = dir.0.join("current-turn");
    let mut extra = vec![
        "--off-session-turn",
        other,
        "--turn-gate",
        gate.to_str().unwrap(),
    ];
    if after_ack {
        extra.push("--off-session-after-ack");
    }
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{
                "id": "a", "argv": peer(&state, &extra), "messages": ["one"]
            }]),
        ),
    );
    let opened = run.event("a", "session-opened");
    assert_ne!(opened["session"], other);
    let ack = run.event("a", "ack");
    run.until("injected evidence consumed", |v| {
        v["event"] == "notice" && v["title"] == "off-session-sent"
    });
    // This is the recipient admission effect, not a predicate-only test.
    run.control(r#"{"cmd":"send","text":"must-stay-blocked"}"#);
    let decision = run.until("overlap decision", |v| {
        v["event"] == "follow-up-refused" || v["event"] == "follow-up-admitted"
    });
    assert_eq!(
        decision["event"], "follow-up-refused",
        "{other}: {decision}"
    );
    assert_eq!(decision["reason"], "input-open");
    assert!(
        events(&run.seen, "a", "turn-end").is_empty(),
        "{:#?}",
        run.seen
    );
    let reply = events(&run.seen, "a", "agent-message")[0];
    assert_eq!(reply["session"], other);
    assert_eq!(reply["parent_message_id"], ack["message_id"]);
    assert!(
        reply["input"].is_null(),
        "off-session reply was attributed: {reply}"
    );
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 1);
    std::fs::write(&gate, b"release").unwrap();
    let turn = run.event("a", "turn-end");
    assert_eq!(turn["session"], opened["session"]);
    assert_eq!(turn["message_id"], ack["message_id"]);
    assert_eq!(
        turn["own_output"], false,
        "off-session reply poisoned own_output"
    );
    run.control(r#"{"cmd":"send","text":"echo:CURRENT"}"#);
    run.event("a", "follow-up-admitted");
    let turn = run.until("valid next turn", |v| {
        v["event"] == "turn-end" && v["input"] == 1
    });
    assert_eq!(turn["session"], opened["session"]);
    assert_eq!(turn["own_output"], true);
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert_eq!(terminal["owed"], 0);
    assert_eq!(terminal["all_harnesses_reaped"], true);
    assert_eq!(events(&seen, "a", "turn-end").len(), 2);
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 2);
}

/// Updates do not constitute insertion receipts. Without the correlated
/// response, unknown-session output/idle leaves the actual message owed.
#[test]
fn unknown_session_updates_without_ack_leave_input_owed() {
    let dir = Scratch::new("unknown-no-ack");
    let state = dir.state("a");
    let run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{
                "id": "a", "argv": peer(&state, &[
                    "--mode", "off-session-no-ack", "--off-session-turn", "sess-unknown"
                ]), "messages": ["one"]
            }]),
        ),
    );
    let (terminal, _, seen) = run.terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(terminal["owed"], 1);
    assert_eq!(terminal["all_harnesses_reaped"], true);
    assert!(events(&seen, "a", "ack").is_empty());
    assert!(events(&seen, "a", "turn-end").is_empty());
    let reply = events(&seen, "a", "agent-message")[0];
    assert_eq!(reply["session"], "sess-unknown");
    assert!(reply["input"].is_null());
    let message = &harness(&terminal, "a")["messages"][0];
    assert_eq!(message["state"], "owed");
    assert_eq!(message["attempts"], 1);
    assert_eq!(
        count(
            &db(&dir),
            "SELECT count(*) FROM message WHERE ack_label IS NOT NULL"
        ),
        0
    );
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 1);
}

/// A rejected RPC is no proof of non-insertion. It blocks ordinary
/// admission/close, creates no ACK, and never grants replay on recovery.
#[test]
fn sdk_resident_rejected_turn_preserves_uncertainty_and_blocks_replay() {
    for (shape, code, declaration) in [
        ("rejected", -32011, "no-non-insertion-declaration"),
        ("not-inserted", -32010, "not-inserted"),
    ] {
        let dir = Scratch::new(shape);
        let state = dir.state("h");
        let prepared = agent_provider_contract::resident_session::ResidentPrepareResult::v1(
            vec!["resident.serve".to_owned()],
            "1".repeat(64),
        );
        let mut request = spec(
            &dir,
            3,
            json!([{
                "id":"h", "argv":peer(&state, &["--resident-evidence", shape]),
                "messages":["synthetic prompt"], "resident":prepared,
            }]),
        );
        request["intent"]["cwd"] = json!(std::env::current_dir().unwrap());
        let mut run = Run::start(&dir, &request);
        assert_eq!(
            run.event("h", "resident-session-started")["canonical_binding"],
            "unbound"
        );
        let rejected = run.event("h", "rejected");
        assert_eq!(rejected["code"], code);
        assert_eq!(rejected["endpoint_declaration"], declaration);
        assert_eq!(rejected["physical_non_insertion"], "not-established");
        assert_eq!(rejected["hold"], "unresolved-input");
        assert_eq!(rejected["exit"], "cancel-or-peer-exit");
        assert_eq!(rejected["insertion"], "unresolved");
        assert_eq!(rejected["retry"], "not-authorized");
        assert_eq!(rejected["native_report"]["custody"], "complete");
        assert_eq!(rejected["native_report"]["status_code"], 0);
        assert_eq!(rejected["endpoint_record_error"], true);
        assert_eq!(rejected["unresolved_attempts"], 1);
        run.control(r#"{"cmd":"send","text":"must not reach peer"}"#);
        assert_eq!(run.event("h", "follow-up-refused")["reason"], "input-open");
        run.control(r#"{"cmd":"close"}"#);
        run.event("h", "close-not-applied");
        run.cancel();
        let (terminal, status, seen) = run.terminal();
        assert_eq!(status.code(), Some(2));
        let message = &harness(&terminal, "h")["messages"][0];
        assert_eq!(message["state"], "owed");
        assert_eq!(message["label"], "rejected-unresolved");
        assert_eq!(message["unresolved_attempts"], 1);
        assert_eq!(message["turn_end"], "not-recorded");
        assert!(events(&seen, "h", "ack").is_empty());
        assert!(events(&seen, "h", "turn-end").is_empty());
        assert!(
            !serde_json::to_string(&seen)
                .unwrap()
                .contains("PRIVATE-RESIDENT-PAYLOAD")
        );
        let conn = db(&dir);
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM attempt WHERE outcome = 'rejected-unresolved'"
            ),
            1
        );
        assert_eq!(
            count(
                &conn,
                "SELECT count(*) FROM message WHERE stop = 'rejected-unresolved' AND ack_label IS NULL"
            ),
            1
        );
        drop(conn);
        let recovered = Run::start(&dir, &recover(&dir));
        let (terminal, _, _) = recovered.terminal();
        assert_eq!(harness(&terminal, "h")["launches"], 0);
        assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 1);
    }
}

/// SDK resume consumes the stored native session after an observed peer
/// exit. Same-key dedup is transport evidence, not native at-most-once proof.
#[test]
fn sdk_resident_resume_keeps_session_and_transport_history() {
    let dir = Scratch::new("sdk-resume");
    let state = dir.state("h");
    let prepared = agent_provider_contract::resident_session::ResidentPrepareResult::v1(
        vec!["resident.serve".to_owned()],
        "3".repeat(64),
    );
    let mut request = spec(
        &dir,
        3,
        json!([{
            "id":"h", "argv":peer(&state, &["--resident-evidence","absent", "--mode","exit-before-ack-once", "--exit-after-acks","1"]),
            "messages":["synthetic prompt"], "resident":prepared,
        }]),
    );
    request["intent"]["cwd"] = json!(std::env::current_dir().unwrap());
    let (terminal, status, seen) = Run::start(&dir, &request).terminal();
    assert_eq!(status.code(), Some(0), "{terminal}");
    let starts = events(&seen, "h", "resident-session-started");
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0]["resumed"], false);
    assert_eq!(starts[1]["resumed"], true);
    assert_eq!(starts[0]["session"], starts[1]["session"]);
    assert!(starts.iter().all(|s| s["canonical_binding"] == "unbound"));
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 2);
    assert_eq!(
        read_state(&state)["insertions"].as_array().unwrap().len(),
        1
    );
    let message = &harness(&terminal, "h")["messages"][0];
    assert_eq!(message["state"], "acknowledged");
    assert_eq!(message["attempts"], 2);
    assert_eq!(message["closures"], 1);
}

/// Real host transport and owner: safe evidence projections, own versus
/// covering tags, contradictory post-idle publication diagnostics, and
/// continued host admission when a bounded quiet window expires.
#[test]
fn sdk_resident_turn_evidence_and_late_diagnostics_keep_their_scope() {
    for (shape, report_state) in [
        ("late", "valid"),
        ("later", "valid"),
        ("covering", "valid"),
        ("incomplete", "valid"),
        ("absent", "absent"),
        ("null", "null"),
        ("invalid", "invalid-or-unsupported"),
    ] {
        let dir = Scratch::new(shape);
        let state = dir.state("h");
        let prepared = agent_provider_contract::resident_session::ResidentPrepareResult::v1(
            vec!["resident.serve".to_owned()],
            "2".repeat(64),
        );
        let mut request = spec(
            &dir,
            3,
            json!([{
                "id":"h", "argv":peer(&state, &["--resident-evidence",shape]),
                "messages":["synthetic prompt"], "resident":prepared,
            }]),
        );
        request["intent"]["cwd"] = json!(std::env::current_dir().unwrap());
        let mut run = Run::start(&dir, &request);
        let start = run.event("h", "resident-session-started");
        assert_eq!(start["canonical_binding"], "unbound");
        assert_eq!(start["canonical_publication"], "not-established");
        let ack = run.event("h", "ack");
        let end = run.event("h", "turn-end");
        assert_eq!(end["message_id"], ack["message_id"]);
        assert_eq!(end["native_report"]["state"], report_state);
        assert_eq!(end["own_turn_end"], shape != "covering");
        assert_eq!(end["native_report_for"], end["last_user_message_id"]);
        assert_eq!(end["endpoint_durability"], "not-established");
        assert_eq!(end["canonical_publication"], "not-established");
        if shape == "incomplete" {
            assert_eq!(end["native_report"]["custody"], "incomplete");
        }
        if shape == "late" {
            let diagnostic = run.event("h", "endpoint-record-error");
            assert_eq!(diagnostic["message_id"], ack["message_id"]);
            assert_eq!(diagnostic["input"], 0);
            assert_eq!(diagnostic["endpoint_durability"], "contrary-diagnostic");
        }
        // Silence after idle must not demand an endless diagnostic wait.
        run.control(r#"{"cmd":"send","text":"echo:NEXT"}"#);
        assert_eq!(run.event("h", "follow-up-admitted")["input"], 1);
        run.until("second tagged end", |v| {
            v["event"] == "turn-end" && v["input"] == 1
        });
        if shape == "later" {
            let diagnostic = events(&run.seen, "h", "endpoint-record-error");
            assert_eq!(diagnostic.len(), 1);
            assert_eq!(diagnostic[0]["message_id"], ack["message_id"]);
            assert_eq!(
                diagnostic[0]["input"], 0,
                "later report belongs to first input"
            );
        }
        run.control(r#"{"cmd":"close"}"#);
        let (terminal, status, seen) = run.terminal();
        assert_eq!(status.code(), Some(7), "{terminal}");
        assert!(
            !serde_json::to_string(&seen)
                .unwrap()
                .contains("PRIVATE-RESIDENT-PAYLOAD")
        );
        assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 2);
        let prepared_saved: String = db(&dir)
            .query_row("SELECT resident FROM harness WHERE position=0", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&prepared_saved).unwrap(),
            serde_json::to_value(prepared).unwrap()
        );
    }
}

/// `closed` (7), the harness's actual end a signal, never `cancelled`.
#[test]
fn follow_up_reaches_the_same_session_and_close_is_not_cancel() {
    let dir = Scratch::new("follow");
    let state = dir.state("a");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&state, &[]), "messages": ["one"] }]),
        ),
    );
    let first = run.event("a", "turn-end");
    assert_eq!(first["input"], 0, "{first}");
    // Controls 1-6: none is admitted.
    for line in [
        "not json",
        r#"{"cmd":"send"}"#,
        r#"{"cmd":"send","text":""}"#,
        r#"{"cmd":"send","text":"x","harness":"nope"}"#,
        r#"{"cmd":"send","text":"x","extra":1}"#,
        r#"{"cmd":"bogus"}"#,
    ] {
        run.control(line);
    }
    run.control(&json!({ "cmd": "send", "text": "two", "ref": "r2" }).to_string());
    let admitted = run.event("a", "follow-up-admitted");
    assert_eq!(admitted["control"], 7, "{admitted}");
    assert_eq!(admitted["ref"], "r2");
    assert_eq!(admitted["input"], 1);
    assert_eq!(admitted["durable"], true);
    let ack = run.until("ack of input 1", |value| {
        value["event"] == "ack" && value["index"] == 1
    });
    let turn = run.until("turn-end of input 1", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    assert_eq!(turn["message_id"], ack["message_id"], "{turn}");
    assert_eq!(turn["session"], first["session"], "same session");
    // One write, so the later lines reach the owner before the run can end.
    run.control(&format!(
        "{}\n{}\n{}",
        r#"{"cmd":"close"}"#,
        json!({ "cmd": "send", "text": "three", "ref": "r3" }),
        r#"{"cmd":"close"}"#,
    ));
    let close = run.until("close-requested", |value| {
        value["event"] == "close-requested"
    });
    assert_eq!(close["control"], 8);
    let (terminal, status, seen) = run.terminal();

    for control in 1..=6 {
        assert!(by_control(&seen, "follow-up-admitted", control).is_empty());
        assert!(by_control(&seen, "follow-up-received", control).is_empty());
    }
    assert_eq!(
        by_control(&seen, "control-refused", 1)[0]["reason"],
        "malformed"
    );
    for control in 2..=5 {
        let refused = by_control(&seen, "follow-up-refused", control);
        assert_eq!(refused.len(), 1, "control {control}: {seen:#?}");
        assert_eq!(refused[0]["admitted"], false);
    }
    assert_eq!(
        by_control(&seen, "follow-up-refused", 4)[0]["reason"],
        "unknown-harness"
    );
    assert_eq!(
        by_control(&seen, "control-refused", 6)[0]["reason"],
        "unknown-command"
    );
    let after = by_control(&seen, "follow-up-refused", 9);
    assert_eq!(after[0]["reason"], "input-closed", "{after:?}");
    assert_eq!(after[0]["ref"], "r3");
    assert_eq!(
        by_control(&seen, "control-refused", 10)[0]["reason"],
        "close-already-requested"
    );
    assert_eq!(events(&seen, "a", "follow-up-admitted").len(), 1);
    assert_eq!(events(&seen, "a", "close-stopping").len(), 1);

    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    assert_eq!(terminal["cancel_requested"], false);
    assert_eq!(terminal["close_requested"], true);
    assert_eq!(terminal["owed"], 0);
    assert_eq!(terminal["records_complete"], true);
    assert_eq!(terminal["all_harnesses_reaped"], true);
    let a = harness(&terminal, "a");
    assert_eq!(a["exits"], json!(["signal:9"]), "{a}");
    assert_eq!(a["close"], "owner-stop-attempted-after-turns-ended");
    assert_eq!(a["launches"], 1);
    assert_eq!(a["messages"][0]["origin"], "intent");
    assert_eq!(a["messages"][1]["origin"], "follow-up");
    assert_eq!(a["messages"][1]["label"], "accepted");
    assert_eq!(a["messages"][1]["completion"], "not-observed");

    let peer = read_state(&state);
    assert_eq!(peer["sessions"], json!(["sess-1"]));
    let prompts = peer["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 2, "{peer}");
    assert!(prompts.iter().all(|prompt| prompt["session"] == "sess-1"));
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message WHERE origin = 'follow-up' AND idx = 1 AND control = 7 AND caller_ref = 'r2' AND ack_label = 'accepted'"
        ),
        1
    );
}

/// Overlap at the worker's admission check is refused: while an input's
/// turn remains open a follow-up is not admitted or sent. A close waits
/// for that turn; with a turn that never ends, only cancel ends the run,
/// as `cancelled`, with the close not followed through.
#[test]
fn overlapping_follow_up_is_refused_and_close_waits_for_the_open_turn() {
    let dir = Scratch::new("overlap");
    let state = dir.state("a");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&state, &["--no-idle"]), "messages": ["one"] }]),
        ),
    );
    let pid = run.event("a", "launched")["pid"].as_u64().unwrap();
    run.event("a", "ack");
    run.control(&json!({ "cmd": "send", "text": "two" }).to_string());
    let refused = run.event("a", "follow-up-refused");
    assert_eq!(refused["reason"], "input-open", "{refused}");
    assert_eq!(refused["control"], 1);
    run.control(r#"{"cmd":"close"}"#);
    run.until("close-requested", |value| {
        value["event"] == "close-requested"
    });
    std::thread::sleep(QUIET_WINDOW);
    let seen = run.drain_now();
    assert!(events(seen, "a", "close-stopping").is_empty(), "{seen:#?}");
    assert!(events(seen, "a", "exited").is_empty());
    let deferred = events(seen, "a", "close-deferred");
    assert_eq!(deferred.len(), 1, "{seen:#?}");
    assert_eq!(deferred[0]["reason"], "input-open");
    assert!(events(seen, "a", "close-not-applied").is_empty());
    assert!(alive(pid), "a close does not stop an open turn");
    run.cancel();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert_eq!(status.code(), Some(2));
    assert_eq!(terminal["close_requested"], true);
    let a = harness(&terminal, "a");
    assert_eq!(a["close"], "not-stopped-by-close");
    assert_eq!(a["exits"], json!(["signal:9"]));
    assert_eq!(a["messages"].as_array().unwrap().len(), 1);
    assert!(events(&seen, "a", "follow-up-admitted").is_empty());
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 1);
}

/// An admitted follow-up is owed debt like an intent message: sent but
/// never acknowledged, it stays owed in the terminal and the store,
/// labelled by why this instance stopped, with no acknowledgement claimed.
#[test]
fn admitted_follow_up_without_ack_stays_owed_debt() {
    let dir = Scratch::new("debt");
    let state = dir.state("a");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&state, &["--silent-after-acks", "1"]), "messages": ["one"] }]),
        ),
    );
    run.event("a", "turn-end");
    run.control(&json!({ "cmd": "send", "text": "two", "ref": "d" }).to_string());
    let admitted = run.event("a", "follow-up-admitted");
    assert_eq!(admitted["input"], 1);
    wait_state(&state, |peer| {
        peer["prompts"].as_array().unwrap().len() == 2
    });
    run.cancel();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert_eq!(status.code(), Some(2));
    assert_eq!(terminal["owed"], 1);
    assert_eq!(terminal["owed_history"], "retained-in-store");
    let message = &harness(&terminal, "a")["messages"][1];
    assert_eq!(message["origin"], "follow-up");
    assert_eq!(message["state"], "owed");
    assert_eq!(message["label"], "cancelled");
    assert_eq!(message["attempts"], 1);
    assert!(
        !seen
            .iter()
            .any(|value| value["event"] == "ack" && value["index"] == 1)
    );
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message WHERE idx = 1 AND origin = 'follow-up' AND ack_label IS NULL"
        ),
        1
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM attempt WHERE idx = 1"),
        1
    );
}

/// A declaration this owner cannot honour is refused before anything is
/// written or launched: a non-root owner never runs a declared host-root
/// root, whatever user it names, and never picks its isolation from its
/// euid. (These tests run unprivileged.)
#[test]
fn host_root_declared_by_a_non_root_owner_is_refused_before_any_effect() {
    let dir = std::env::temp_dir().join(format!("root-supervisor-decl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    for user in ["root", "nobody"] {
        let request = json!({
            "store": dir.join("s"),
            "intent": {
                "outage_closure_cap": 1,
                "delivery_attempt_cap": 1,
                "cwd": "/",
                "workload": { "isolation": "host-root", "user": user, "ipc_dir": dir.join("ipc") },
                "harnesses": [{ "id": "h", "argv": ["/bin/true"], "messages": ["x"] }],
            },
        });
        let mut child = Command::new(SUPERVISOR)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
        let output = child.wait_with_output().unwrap();
        let terminal: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(output.status.code(), Some(64), "{terminal}");
        assert_eq!(terminal["status"], "spec-refused", "{terminal}");
        assert!(
            terminal["reason"]
                .as_str()
                .unwrap()
                .contains("host-root declared but the owner is euid"),
            "{terminal}"
        );
        assert!(!dir.join("s").exists() && !dir.join("ipc").exists());
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---------------------------------------------------------------------------
// Registered children and namespace-wide stop. Deterministic peers stand in
// for both parent and child harnesses; nothing here is a model, Luna,
// OpenCode or Claude.

const CHILD_CLIENT: &str = env!("CARGO_BIN_EXE_oulipoly-root-child");

/// A create request whose intent allows children on `routes`.
fn child_spec(
    dir: &Scratch,
    harnesses: Value,
    routes: Value,
    starts: u32,
    concurrent: u32,
) -> Value {
    let mut spec = spec(dir, 1, harnesses);
    spec["intent"]["children"] = json!({
        "routes": routes,
        "max_starts": starts,
        "max_concurrent": concurrent,
        "launch_base": dir.0.join("children"),
    });
    spec
}

/// A fixed child route running a deterministic peer.
fn peer_route(dir: &Scratch, name: &str, extra: &[&str]) -> Value {
    json!({ "harness": "fixed", "argv": peer(&dir.state(name), extra), "endpoint": "stdio" })
}

/// Waits for the file a fixture process inside a work writes once.
fn wait_file(path: &Path) -> String {
    let deadline = std::time::Instant::now() + WATCHDOG;
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "watchdog: fixture file {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The pids, in this test's namespace, of every process this test's
/// `/proc` shows in PID namespace `ns` (`pid:[inode]`) with pid `local`
/// there: what a host-side observer resolves from a work's own view.
fn host_pids_in(ns: &str, local: i32) -> Vec<i32> {
    let local = local.to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| {
            std::fs::read_link(format!("/proc/{pid}/ns/pid"))
                .is_ok_and(|link| link.to_str() == Some(ns))
                && std::fs::read_to_string(format!("/proc/{pid}/status")).is_ok_and(|status| {
                    status
                        .lines()
                        .find_map(|line| line.strip_prefix("NSpid:"))
                        .and_then(|pids| pids.split_whitespace().last())
                        == Some(local.as_str())
                })
        })
        .collect()
}

/// The pid in this test's namespace of the fixture process that wrote its
/// own pid and PID namespace, as its work's `/proc` names them, to `path`.
fn wait_pid(path: &Path) -> i32 {
    let text = wait_file(path);
    let (local, ns) = text
        .trim()
        .split_once(' ')
        .unwrap_or_else(|| panic!("pid file: {text}"));
    let local: i32 = local.parse().unwrap();
    let pids = host_pids_in(ns, local);
    assert_eq!(pids.len(), 1, "{text}: {pids:?}");
    pids[0]
}

/// A shell line that writes its own pid and PID namespace (as its work's
/// own /proc names them) to `path`, ignores TERM, HUP and INT, leaves its
/// session and outlives its starter: an adversarial descendant, not a
/// harness or a Bash run.
fn adversary(path: &Path) -> String {
    let path = path.display();
    format!(
        "setsid /bin/sh -c 'trap \"\" TERM HUP INT; echo $$ $(readlink /proc/self/ns/pid) > {path}.part && mv {path}.part {path}; exec sleep 600' </dev/null >/dev/null 2>&1 &"
    )
}

/// One line of what a shell in a work sees of its own `/proc`: the pid
/// `/proc/self` names (through the shell's own redirect, so the shell
/// itself), its own pid (`$$`), its PID namespace, the `NSpid` fields its
/// `/proc` shows, and the options of the topmost `/proc` mount it sees.
const PROC_FACTS: &str = r#"read p r < /proc/self/stat; while read k v; do [ "$k" = NSpid: ] && n=$v; done < /proc/self/status; set -- $n; n=$(IFS=,; echo "$*"); m=$(while read a b c d e f g; do [ "$e" = /proc ] && echo "$g"; done < /proc/self/mountinfo | tail -n 1); echo "proc-view self=$p own=$$ ns=$(readlink /proc/self/ns/pid) nspid=$n propagation=${m:-none}""#;

/// The facts of a [`PROC_FACTS`] line in `text`.
fn proc_facts(text: &str) -> std::collections::HashMap<String, String> {
    let line = text
        .lines()
        .find_map(|line| line.split_once("proc-view ").map(|(_, facts)| facts))
        .unwrap_or_else(|| panic!("no proc-view line: {text}"));
    line.split_whitespace()
        .filter_map(|fact| fact.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// The `/proc` mounts this test's own mount namespace has, as mountinfo
/// lines: a work's private `/proc` must not appear among them.
fn own_proc_mounts() -> Vec<String> {
    std::fs::read_to_string("/proc/self/mountinfo")
        .unwrap()
        .lines()
        .filter(|line| line.contains(" - proc "))
        .map(str::to_owned)
        .collect()
}

/// The workload invariant: a harness's descendants and a Bash run each see
/// a `/proc` of their own work's PID namespace (their own pid, no host pid
/// fields, a mount that propagates nothing back), while the owner's host
/// view still resolves the same process, attributes Bash from it, and the
/// host's mounts are unchanged.
#[test]
fn work_sees_a_proc_of_its_own_pid_namespace_and_host_attribution_holds() {
    let dir = Scratch::new("proc-view");
    let host_mounts = own_proc_mounts();
    let facts_file = dir.0.join("harness.facts");
    let spawned = format!(
        "{{ {PROC_FACTS}; }} > {path}.part && mv {path}.part {path}; exec sleep 600",
        path = facts_file.display()
    );
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&dir.state("a"), &[]), "messages": [
                format!("spawn:{spawned}"),
                format!("bash:{PROC_FACTS}"),
            ] }]),
        ),
    );
    let launched = run.event("a", "launched");
    let harness = launched["pid"].as_i64().unwrap();
    let harness_ns = std::fs::read_link(format!("/proc/{harness}/ns/pid")).unwrap();

    // A descendant of the harness, in the harness's own namespace.
    let inside = proc_facts(&wait_file(&facts_file));
    assert_eq!(inside["self"], inside["own"], "{inside:?}");
    assert_eq!(
        inside["nspid"], inside["own"],
        "no host pid field: {inside:?}"
    );
    assert_eq!(Path::new(&inside["ns"]), harness_ns, "{inside:?}");
    assert!(
        !inside["propagation"].contains("shared:"),
        "the work's /proc propagates to a peer group: {inside:?}"
    );
    // The owner's host view resolves that same process, in that namespace,
    // as a descendant of the launched harness.
    let own: i32 = inside["own"].parse().unwrap();
    let resolved = host_pids_in(&inside["ns"], own);
    assert_eq!(resolved.len(), 1, "{inside:?}: {resolved:?}");
    let mut ancestor = resolved[0];
    while ancestor > 1 && i64::from(ancestor) != harness {
        ancestor = i32::try_from(ppid(u64::try_from(ancestor).unwrap())).unwrap();
    }
    assert_eq!(i64::from(ancestor), harness, "{resolved:?}");
    let descendant_fd = pidfd(resolved[0]).expect("descendant pidfd");
    assert_eq!(
        own_proc_mounts(),
        host_mounts,
        "a work's mount reached the host"
    );

    // A Bash run: pid 2 of its own new namespace, seen as such by its own
    // /proc, and still attributed by the owner to the harness that asked.
    let answer = run.until("bash reply", |value| {
        value["harness"] == "a" && value["event"] == "agent-message" && value["input"] == 1
    });
    let text = answer["text"].as_str().unwrap();
    assert!(text.contains("exit=Some(0)"), "{text}");
    let bash = proc_facts(text);
    assert_eq!(bash["self"], "2", "{bash:?}");
    assert_eq!(bash["own"], "2", "{bash:?}");
    assert_eq!(bash["nspid"], "2", "{bash:?}");
    assert_ne!(Path::new(&bash["ns"]), harness_ns, "{bash:?}");
    assert!(!bash["propagation"].contains("shared:"), "{bash:?}");
    let accepted = owner_event(run.drain_now(), "bash-accepted")[0].clone();
    assert_eq!(accepted["harness"], "a", "{accepted}");

    run.until("turn end", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    let exited_event = events(&seen, "a", "exited")[0];
    assert_eq!(exited_event["namespace"]["drained"], true, "{exited_event}");
    assert!(
        exited(&descendant_fd, 5_000),
        "the harness's descendant survived the close"
    );
    assert_eq!(own_proc_mounts(), host_mounts);
}

/// Where a work cannot get a `/proc` of its own (here: an enclosing user
/// namespace allows no further mount namespaces, while PID namespaces are
/// still allowed), the harness is not started against the host's `/proc`:
/// its start fails visibly as `workload-proc`, and it never runs.
#[test]
fn work_that_cannot_get_its_own_proc_is_refused_and_never_runs() {
    let dir = Scratch::new("proc-refused");
    // SAFETY: getuid/getgid have no preconditions.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let wrapper = [
        "/usr/bin/unshare".to_owned(),
        "--map-root-user".to_owned(),
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        format!(
            "echo 0 > /proc/sys/user/max_mnt_namespaces && exec /usr/bin/unshare --map-user={uid} --map-group={gid} {SUPERVISOR}"
        ),
    ];
    let state = dir.state("a");
    let mut run = Run::start_in(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&state, &[]), "messages": ["echo:never"] }]),
        ),
        &wrapper,
    );
    let failed = run.event("a", "launch-failed");
    let reason = failed["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("workload-proc: mount namespace: "),
        "{failed}"
    );
    run.cancel();
    let (terminal, _status, seen) = run.terminal();
    assert!(!state.exists(), "the harness ran: {terminal}");
    assert!(events(&seen, "a", "ack").is_empty());
}

fn child_result(seen: &[Value]) -> Vec<&Value> {
    owner_event(seen, "child-result")
}

/// G1/G2/G3: a parent asks for a child; the child's answer comes back to the
/// parent's own requester only, with lineage durable before `accepted`;
/// the child's turn end, its stop and its waited end are distinct facts;
/// the parent's own turn end and close remain the parent's.
#[test]
fn registered_child_answers_its_parent_with_lineage_and_is_stopped_after_its_turn() {
    let dir = Scratch::new("child-answer");
    let mut run = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "parent", "argv": peer(&dir.state("parent"), &[]), "messages": ["explore:echo:echo:wired via lib.rs"] }]),
            json!({ "echo": peer_route(&dir, "child", &[]) }),
            4,
            2,
        ),
    );
    let accepted = run.until("child accepted", |value| value["event"] == "child-accepted");
    assert_eq!(accepted["durable"], true);
    assert_eq!(accepted["parent"], "parent");
    assert_eq!(accepted["route"], "echo");
    assert_eq!(accepted["child"], "child-1");
    assert_eq!(
        accepted["input_attribution"], "single-open-input",
        "{accepted}"
    );
    let parent_launch = run.event("parent", "launched");
    assert_eq!(accepted["parent_work"], parent_launch["work"]);
    let parent_turn = run.until("parent turn end", |value| {
        value["harness"] == "parent" && value["event"] == "turn-end" && value["input"] == 0
    });
    assert_eq!(parent_turn["own_output"], true);
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert_eq!(terminal["status"], "closed");
    assert_eq!(terminal["records_complete"], true);
    // Only the intent harness is a terminal harness record.
    assert_eq!(terminal["harnesses"].as_array().unwrap().len(), 1);
    // The child's own events are its own and marked as a child's.
    let child_turns = events(&seen, "child-1", "turn-end");
    assert_eq!(child_turns.len(), 1);
    assert_eq!(child_turns[0]["child"]["parent"], "parent");
    let child_message = events(&seen, "child-1", "agent-message")[0];
    assert_eq!(child_message["text"], "wired via lib.rs");
    // The child was stopped after its turn (a request), and its end and its
    // namespace's drain were reported by its waiters (facts).
    let stopping = events(&seen, "child-1", "close-stopping")[0];
    assert_eq!(stopping["signalled"], true);
    let exited = events(&seen, "child-1", "exited")[0];
    assert_eq!(exited["status"], "signal:9");
    assert_eq!(exited["stop_requested"], true);
    assert_eq!(exited["namespace"]["drained"], true, "{exited}");
    let result = child_result(&seen)[0];
    assert_eq!(result["outcome"], "answered");
    assert_eq!(result["answer"], "wired via lib.rs");
    assert_eq!(result["turn_end"]["stop_reason"], "end_turn");
    assert_eq!(result["end"]["status"], "signal:9");
    // Content and lifecycle are separate facts in the result.
    assert_eq!(result["lifecycle"]["end"], "observed", "{result}");
    assert_eq!(result["lifecycle"]["bash_runs_open"], 0);
    assert_eq!(
        result["lifecycle"]["budget"],
        "still charged (release pending)"
    );
    // The parent got exactly that result on its own connection; its own
    // answer and turn end are the parent's, after the child's.
    let parent_answer = events(&seen, "parent", "agent-message")[0];
    let text = parent_answer["text"].as_str().unwrap();
    assert!(text.starts_with("exit=Some(0)"), "{text}");
    assert!(
        text.contains(r#""outcome":"answered""#) && text.contains("wired via lib.rs"),
        "{text}"
    );
    let position = |pred: &dyn Fn(&Value) -> bool| seen.iter().position(pred).unwrap();
    assert!(
        position(&|v| v["harness"] == "child-1" && v["event"] == "turn-end")
            < position(&|v| v["harness"] == "parent" && v["event"] == "turn-end")
    );
    // The parent was stopped by the caller's close, not by the child.
    assert_eq!(harness(&terminal, "parent")["exits"], json!(["signal:9"]));
    assert_eq!(terminal["children"]["starts"], 1);
    assert_eq!(terminal["children"]["children"][0]["outcome"], "answered");
    assert_eq!(terminal["children"]["children"][0]["requester"], "written");
    // Durable lineage: the child row names the exact parent work.
    let conn = db(&dir);
    let (parent, parent_work, route, outcome): (i64, i64, String, String) = conn
        .query_row(
            "SELECT parent, parent_work, route, outcome FROM child",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        (parent, route.as_str(), outcome.as_str()),
        (0, "echo", "answered")
    );
    assert_eq!(Some(parent_work), parent_launch["work"].as_i64());
    let text: String = conn
        .query_row("SELECT m.text FROM message m JOIN harness h ON h.position = m.harness WHERE h.kind = 'child'", [], |row| row.get(0))
        .unwrap();
    assert!(
        text.starts_with("You are a registered read-only orientation explorer"),
        "{text}"
    );
    assert!(text.ends_with("\n\necho:wired via lib.rs"), "{text}");
    // The child's peer saw one session and one prompt: never replayed.
    let child = read_state(&dir.state("child"));
    assert_eq!(child["launches"].as_array().unwrap().len(), 1);
    assert_eq!(child["prompts"].as_array().unwrap().len(), 1);
}

/// An exec failure is not a no-process-start: the work was launched and
/// waited even though the requested executable did not run.
#[test]
fn child_exec_failure_reports_started_work_separately_from_exec() {
    let dir = Scratch::new("child-exec-fault");
    let mut run = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "parent", "argv": peer(&dir.state("parent"), &[]),
                 "messages": ["explore:broken:q"] }]),
            json!({ "broken": { "harness": "fixed", "endpoint": "stdio",
                            "argv": [dir.0.join("does-not-exist")] } }),
            4,
            2,
        ),
    );
    run.until("parent received exec-fault result", |value| {
        value["harness"] == "parent" && value["event"] == "turn-end"
    });
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, _, seen) = run.terminal();
    let failed = events(&seen, "child-1", "launch-failed")[0];
    assert_eq!(failed["not_started"], false, "{failed}");
    assert!(failed["work"].is_number());
    let result = child_result(&seen)[0];
    assert_eq!(result["launch"]["not_started"], false);
    assert_eq!(result["lifecycle"]["end"], "observed");
    assert!(!result["end"].is_null());
    let text = events(&seen, "parent", "agent-message")[0]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains(r#""not_started":false"#), "{text}");
    assert_eq!(terminal["children"]["children"][0]["record"]["launches"], 1);
}

/// G2/G5/G7: refusals record and start nothing: no child policy; a route
/// the policy does not name; a child asking for a grandchild (depth 1);
/// a request from a Bash run's namespace (not a harness's).
#[test]
fn child_requests_are_refused_without_policy_route_or_depth_and_from_bash() {
    // No policy: refused.
    let dir = Scratch::new("child-none");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "p", "argv": peer(&dir.state("p"), &[]), "messages": ["explore:echo:echo:x"] }]),
        ),
    );
    let refused = run.until("refused", |value| value["event"] == "child-refused");
    assert_eq!(refused["reason"], "children-not-enabled");
    run.until("turn end", |value| value["event"] == "turn-end");
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, _, seen) = run.terminal();
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(terminal["children"]["refused"], 1);
    assert_eq!(terminal["children"]["starts"], 0);
    let text = events(&seen, "p", "agent-message")[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        text.starts_with("exit=Some(65)") && text.contains("children-not-enabled"),
        "{text}"
    );
    assert_eq!(
        count(
            &db(&dir),
            "SELECT count(*) FROM harness WHERE kind = 'child'"
        ),
        0
    );

    // Route outside the policy, a grandchild, and a Bash-namespace request.
    let dir = Scratch::new("child-refuse");
    let mut run = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "p", "argv": peer(&dir.state("p"), &[]), "messages": ["explore:other:echo:x"] }]),
            json!({ "echo": peer_route(&dir, "c", &[]) }),
            4,
            2,
        ),
    );
    run.until("route refused", |value| {
        value["event"] == "child-refused" && value["reason"] == "route-not-allowed"
    });
    run.until("turn end", |value| {
        value["event"] == "turn-end" && value["input"] == 0
    });
    run.control(
        &json!({ "cmd": "send", "text": "explore:echo:explore:echo:grandchild" }).to_string(),
    );
    let depth = run.until("depth refused", |value| {
        value["event"] == "child-refused"
            && value["reason"]
                .as_str()
                .is_some_and(|r| r.starts_with("depth"))
    });
    assert_eq!(depth["harness"], "child-1", "{depth}");
    assert!(
        depth["reason"].as_str().unwrap().starts_with("depth"),
        "{depth}"
    );
    run.until("turn end 1", |value| {
        value["harness"] == "p" && value["event"] == "turn-end" && value["input"] == 1
    });
    // A Bash run gets no ingress variable; name the socket explicitly, as a
    // shell could: its namespace is still not a harness's.
    let socket = owner_event(run.drain_now(), "bash-ingress")[0]["path"]
        .as_str()
        .unwrap()
        .to_owned();
    run.control(
        &json!({ "cmd": "send", "text": format!("bash:OULIPOLY_ROOT_BASH_V1={socket} {CHILD_CLIENT} echo from-bash") })
            .to_string(),
    );
    run.until("turn end 2", |value| {
        value["harness"] == "p" && value["event"] == "turn-end" && value["input"] == 2
    });
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, _, seen) = run.terminal();
    assert_eq!(terminal["status"], "closed", "{terminal}");
    // One child (the one that asked for a grandchild); no grandchild row.
    assert_eq!(terminal["children"]["starts"], 1);
    assert_eq!(
        count(
            &db(&dir),
            "SELECT count(*) FROM harness WHERE kind = 'child'"
        ),
        1
    );
    let grandchild = child_result(&seen)[0]["answer"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        grandchild.contains(r#""event":"refused""#) && grandchild.contains("depth"),
        "{grandchild}"
    );
    // From a Bash run's namespace: the ingress refuses before reading.
    let refused = owner_event(&seen, "bash-refused");
    assert!(
        refused
            .iter()
            .any(|value| value["reason"] == "peer-unattributed: outside-every-harness-namespace"),
        "{refused:?}"
    );
    let bash = events(&seen, "p", "agent-message")[2]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(bash.contains("peer-unattributed"), "{bash}");
}

/// G2 budget: at most `max_concurrent` children at once (a third request
/// is refused, not queued) and at most `max_starts` over the root's life.
/// Cancel then stops the live children; their ends and drains are waited.
#[test]
fn child_budget_refuses_over_concurrency_and_starts_and_cancel_stops_children() {
    let dir = Scratch::new("child-budget");
    let out = dir.0.join("out");
    std::fs::create_dir(&out).unwrap();
    let ask = |n: u32| format!("{CHILD_CLIENT} quiet q{n} > {}/r{n} 2>&1", out.display());
    let mut run = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "p", "argv": peer(&dir.state("p"), &[]), "messages": [
                format!("spawn:{} & {} & sleep 0.5; {}", ask(1), ask(2), ask(3))
            ] }]),
            json!({ "quiet": peer_route(&dir, "q", &["--mode", "silent"]) }),
            3,
            2,
        ),
    );
    let refused = run.until("concurrency refusal", |value| {
        value["event"] == "child-refused"
    });
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .starts_with("budget-concurrent: 2 of 2"),
        "{refused}"
    );
    run.until("first child launched", |value| {
        value["harness"] == "child-1" && value["event"] == "launched"
    });
    run.until("second child launched", |value| {
        value["harness"] == "child-2" && value["event"] == "launched"
    });
    run.cancel();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(2), "{terminal}");
    assert_eq!(terminal["status"], "cancelled");
    let children = &terminal["children"];
    assert_eq!(children["starts"], 2, "{children}");
    assert_eq!(children["refused"], 1);
    for result in child_result(&seen) {
        assert_eq!(result["outcome"], "stopped", "{result}");
        assert_eq!(result["stopped"], "cancelled");
        assert_eq!(result["end"]["status"], "signal:9");
        assert_eq!(result["end"]["namespace"]["drained"], true);
    }
    assert_eq!(child_result(&seen).len(), 2);

    // Starts: max_starts 2, sequential children; the third is refused.
    let dir = Scratch::new("child-starts");
    let mut run = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "p", "argv": peer(&dir.state("p"), &[]), "messages": ["explore:echo:echo:one"] }]),
            json!({ "echo": peer_route(&dir, "c", &[]) }),
            2,
            2,
        ),
    );
    for (input, text) in [(1, "explore:echo:echo:two"), (2, "explore:echo:echo:three")] {
        run.until("turn end", |value| {
            value["harness"] == "p" && value["event"] == "turn-end" && value["input"] == input - 1
        });
        run.control(&json!({ "cmd": "send", "text": text }).to_string());
    }
    let refused = run.until("starts refusal", |value| value["event"] == "child-refused");
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .starts_with("budget-starts: 2 of 2"),
        "{refused}"
    );
    run.until("turn end 2", |value| {
        value["harness"] == "p" && value["event"] == "turn-end" && value["input"] == 2
    });
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, _, _) = run.terminal();
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(terminal["children"]["starts"], 2);
    assert_eq!(
        count(
            &db(&dir),
            "SELECT count(*) FROM child WHERE outcome = 'answered'"
        ),
        2
    );
}

/// G3: the requester going away (its process killed) stops its child and
/// the child's own Bash, including a TERM-ignoring descendant that left
/// its session; the child's Bash was attributed to the child, not the
/// parent; nothing is retried and the parent's root closes normally.
#[test]
fn requester_loss_stops_the_child_and_its_attributed_bash_with_descendants() {
    let dir = Scratch::new("child-loss");
    let pid_file = dir.0.join("descendant.pid");
    let child_question = format!("bash:{} sleep 300", adversary(&pid_file));
    let mut run = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "p", "argv": peer(&dir.state("p"), &[]), "messages": [
                format!("spawn:{CHILD_CLIENT} work '{}' > /dev/null 2>&1", child_question.replace('\'', "'\\''"))
            ] }]),
            json!({ "work": peer_route(&dir, "c", &[]) }),
            4,
            2,
        ),
    );
    let accepted = run.until("child accepted", |value| value["event"] == "child-accepted");
    let bash = run.until("child bash", |value| value["event"] == "bash-accepted");
    assert_eq!(
        bash["harness"], "child-1",
        "the child's Bash is the child's: {bash}"
    );
    let child_launch = events(run.drain_now(), "child-1", "launched")[0].clone();
    assert_eq!(bash["harness_work"], child_launch["work"]);
    let descendant = wait_pid(&pid_file);
    let descendant_fd = pidfd(descendant).expect("descendant pidfd");
    run.until("parent turn end", |value| {
        value["harness"] == "p" && value["event"] == "turn-end"
    });
    // The requester is this root's process: kill exactly it (pidfd).
    let requester = i32::try_from(accepted["requester_pid"].as_i64().unwrap()).unwrap();
    let requester_fd = pidfd(requester).expect("requester pidfd");
    let started = std::time::Instant::now();
    // SAFETY: pidfd_send_signal on a pidfd naming the verified requester.
    assert_eq!(
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                requester_fd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        },
        0
    );
    let stopping = run.until("child stopping", |value| value["event"] == "child-stopping");
    assert_eq!(stopping["reason"], "requester-gone");
    let result = run.until("child result", |value| value["event"] == "child-result");
    assert_eq!(result["outcome"], "stopped", "{result}");
    assert_eq!(result["stopped"], "requester-gone");
    assert_eq!(result["end"]["namespace"]["drained"], true);
    let bash_end = run.until("child bash end", |value| value["event"] == "bash-ended");
    assert_eq!(bash_end["work"], bash["work"]);
    assert_eq!(bash_end["bash"], "end", "{bash_end}");
    assert!(
        exited(&descendant_fd, 10_000),
        "the child's Bash descendant was not stopped"
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert_eq!(terminal["children"]["children"][0]["outcome"], "stopped");
    assert_eq!(terminal["children"]["children"][0]["requester"], "gone");
    assert_eq!(terminal["bash"]["accepted"], 1);
    assert_eq!(terminal["bash"]["ended"], 1);
    assert!(owner_event(&seen, "relaunch").is_empty());
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM child WHERE outcome = 'stopped:requester-gone'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM bash_run b JOIN work w ON w.id = b.work WHERE w.harness = 1 AND b.requester_work IS NOT NULL"
        ),
        1
    );
}

/// G4 (no children): a stop ends the whole work, not only its leader. A
/// harness's TERM-ignoring, session-leaving descendant used to hold the
/// work (and so the run) after the harness was killed; a Bash run whose
/// leader already exited used to ignore the stop. Both now drain promptly,
/// and a process of the same user outside the root is never signalled.
#[test]
fn stop_drains_every_process_of_the_work_namespace_and_nothing_outside() {
    // Close: harness descendant.
    let dir = Scratch::new("ns-close");
    let pid_file = dir.0.join("h.pid");
    let mut bystander = Command::new("/bin/sh")
        .args(["-c", "trap '' TERM HUP INT; exec sleep 600"])
        .spawn()
        .unwrap();
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&dir.state("a"), &[]), "messages": [format!("spawn:{}", adversary(&pid_file))] }]),
        ),
    );
    let descendant = wait_pid(&pid_file);
    let descendant_fd = pidfd(descendant).expect("descendant pidfd");
    run.until("turn end", |value| value["event"] == "turn-end");
    let started = std::time::Instant::now();
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "{:?}",
        started.elapsed()
    );
    let exited_event = events(&seen, "a", "exited")[0];
    assert_eq!(exited_event["status"], "signal:9");
    assert_eq!(exited_event["namespace"]["drained"], true, "{exited_event}");
    assert!(exited_event["namespace"]["others_reaped"].as_u64().unwrap() >= 1);
    assert!(
        exited(&descendant_fd, 5_000),
        "descendant survived the close stop"
    );
    assert!(
        alive(u64::from(bystander.id())),
        "a process outside the root was signalled"
    );

    // Cancel: a Bash run whose leader exited while a descendant holds it.
    let dir = Scratch::new("ns-cancel");
    let pid_file = dir.0.join("b.pid");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&dir.state("a"), &[]), "messages": [format!("bash:{}", adversary(&pid_file))] }]),
        ),
    );
    let descendant = wait_pid(&pid_file);
    let descendant_fd = pidfd(descendant).expect("descendant pidfd");
    run.until("bash started", |value| value["event"] == "bash-started");
    // The leader has exited; the run is held only by the descendant.
    assert!(!exited(&descendant_fd, 500));
    run.cancel();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(2), "{terminal}");
    let cancel = owner_event(&seen, "cancel-requested")[0];
    assert!(cancel["signalled"].as_u64().unwrap() >= 1, "{cancel}");
    let ended = owner_event(&seen, "bash-ended")[0];
    assert_eq!(ended["bash"], "end", "{ended}");
    assert_eq!(ended["status"], "code:0", "the leader's own exit: {ended}");
    assert!(exited(&descendant_fd, 5_000), "descendant survived cancel");
    assert_eq!(terminal["bash"]["ended"], 1);
    assert!(
        alive(u64::from(bystander.id())),
        "a process outside the root was signalled"
    );
    // Exact cleanup of this test's own bystander.
    bystander.kill().unwrap();
    bystander.wait().unwrap();
}

/// G3 recovery: an owner killed while its child runs; a recovering owner
/// never continues, reconnects or redelivers the child. Its survivor is
/// stopped and its end recorded as observed; the child is lost visibly.
#[test]
fn recovered_child_is_stopped_never_continued() {
    let dir = Scratch::new("child-recover");
    let mut run = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "p", "argv": peer(&dir.state("p"), &["--on-reinit", "normal"]), "messages": [
                format!("spawn:{CHILD_CLIENT} quiet q > /dev/null 2>&1")
            ] }]),
            json!({ "quiet": peer_route(&dir, "q", &["--mode", "silent"]) }),
            4,
            2,
        ),
    );
    run.until("child launched", |value| {
        value["harness"] == "child-1" && value["event"] == "launched"
    });
    run.until("parent turn end", |value| {
        value["harness"] == "p" && value["event"] == "turn-end"
    });
    // The child holds its one prompt (never acknowledged) when its owner dies.
    wait_state(&dir.state("q"), |state| {
        state["prompts"].as_array().is_some_and(|p| p.len() == 1)
    });
    let _ = run.kill();
    let mut spec = recover(&dir);
    spec["recover"] = json!("cancel");
    let run = Run::start(&dir, &spec);
    let (terminal, _, seen) = run.terminal();
    let stopping = owner_event(&seen, "child-recovered-stopping");
    assert_eq!(stopping.len(), 1, "{seen:#?}");
    let prior = owner_event(&seen, "child-prior-end")[0];
    assert_eq!(prior["status"], "signal:9", "{prior}");
    assert_eq!(prior["observer"], "work-pid1-wait");
    assert_eq!(
        terminal["children"]["children"][0]["outcome"],
        "lost-with-prior-owner"
    );
    // Never relaunched or delivered to again.
    let child = read_state(&dir.state("q"));
    assert_eq!(child["launches"].as_array().unwrap().len(), 1);
    assert_eq!(child["prompts"].as_array().unwrap().len(), 1);
    assert_eq!(
        count(
            &db(&dir),
            "SELECT count(*) FROM child WHERE outcome = 'lost-with-prior-owner'"
        ),
        1
    );
}

/// A fake native `claude -p --input-format stream-json --output-format
/// stream-json` for a real provider adapter. It records what its own
/// `/proc` shows of itself (pid, start time, `NSpid`, PID namespace, boot
/// id) and every launch record under `ACTOR_ROOT` that names its pid as
/// the actor, runs [`PROC_FACTS`] through the root's Bash ingress with
/// `ROOT_BASH`, then echoes the submitted user record (the adapter's
/// consumption evidence) and answers once. No real native or model runs.
const FAKE_NATIVE: &str = r#"#!/usr/bin/python3
import glob, json, os, subprocess, sys
args = sys.argv[1:]
line = sys.stdin.readline()
pid = os.getpid()
stat = open('/proc/self/stat').read()
nspid = [l.split()[1:] for l in open('/proc/self/status').read().splitlines() if l.startswith('NSpid:')][0]
actors = []
for path in glob.glob(os.environ['ACTOR_ROOT'] + '/**/*.json', recursive=True):
    try:
        record = json.load(open(path))
    except Exception:
        continue
    if isinstance(record, dict) and record.get('actor_id') == pid:
        actors.append({'path': path, 'incarnation': record.get('incarnation'), 'phase': record.get('phase')})
bash = subprocess.run([os.environ['ROOT_BASH'], '--', '/bin/sh', '-c', os.environ['PROC_FACTS']],
                      capture_output=True, text=True)
with open(os.environ['CALLS'], 'a') as f:
    f.write(json.dumps({'argv': args, 'pid': pid, 'pgid': os.getpgid(0),
                        'proc_self_pid': int(stat.split()[0]),
                        'start_ticks': stat.rsplit(')', 1)[1].split()[19],
                        'nspid': nspid, 'pidns': os.readlink('/proc/self/ns/pid'),
                        'boot_id': open('/proc/sys/kernel/random/boot_id').read().strip(),
                        'actors': actors,
                        'bash': {'exit': bash.returncode, 'stdout': bash.stdout, 'stderr': bash.stderr[-2000:]}}) + '\n')
message = json.loads(line)
prompt = message['message']['content'][0]['text']
options, i = {}, 0
while i < len(args):
    if args[i] in ['--append-system-prompt', '--model', '--input-format', '--output-format', '--resume', '--session-id']:
        options[args[i]] = args[i + 1]
        i += 2
    else:
        i += 1
session = options.get('--resume', options.get('--session-id'))
def emit(event):
    print(json.dumps(event), flush=True)
emit({'type': 'system', 'subtype': 'init', 'session_id': session, 'model': 'fixture'})
emit({'type': 'user', 'uuid': message['uuid'], 'session_id': session, 'message': message['message']})
emit({'type': 'assistant', 'parent_tool_use_id': None, 'message': {'content': [{'type': 'text', 'text': 'reply to %s' % prompt}]}})
emit({'type': 'result', 'subtype': 'success', 'is_error': False, 'stop_reason': 'end_turn', 'session_id': session})
"#;

/// One provider/v1 operation of `adapter`, run as this test with only
/// `PATH` and `home`: its result, or a panic with what it said.
fn provider_op(
    adapter: &str,
    home: &Path,
    operation: &str,
    data_root: Option<&Path>,
    params: Value,
) -> Value {
    let env = if operation == "policy.evaluate" {
        json!({})
    } else {
        json!({ "OULIPOLY_HOST_RESIDENT_SESSION_V1": "1" })
    };
    let request = json!({
        "contract": "oulipoly.provider/v1",
        "request_id": format!("proc-view-{operation}"),
        "provider_instance_id": null,
        "host": { "app": "oulipoly-agent-runner", "app_version": null, "platform": "linux",
                  "working_directory": null, "config_root": null, "data_root": data_root,
                  "env": env, "deadline_unix_ms": null },
        "params": params,
    });
    let mut child = Command::new(adapter)
        .arg(operation)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    assert!(
        output.status.success() && envelope["ok"] == true,
        "{operation}: {:?} {envelope} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    envelope["result"].clone()
}

/// The workload invariant with a real resident provider: a real provider
/// adapter (`OULIPOLY_PROVIDER_ADAPTER`, a built `agent-runner-claude`)
/// describes, evaluates and prepares itself, and the owner serves its
/// resident endpoint as a plain stdio harness (no Runner front door or
/// registration path here) with a fake native. Two inputs are acknowledged
/// and answered on one native session; each native turn's provider actor
/// record names that native's own incarnation, as the native's own `/proc`
/// shows it in the harness's PID namespace; and each native's Bash request
/// through the root's ingress is attributed to the harness and runs as pid
/// 2 of its own namespace, which its own `/proc` also shows.
#[test]
#[ignore = "needs OULIPOLY_PROVIDER_ADAPTER (a built agent-runner-claude)"]
fn real_provider_adapter_records_its_own_native_incarnation_through_the_owner() {
    let adapter = std::env::var("OULIPOLY_PROVIDER_ADAPTER")
        .expect("OULIPOLY_PROVIDER_ADAPTER: a built agent-runner-claude");
    let dir = Scratch::new("provider-proc");
    let home = dir.0.join("home");
    let project = dir.0.join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let native = dir.0.join("claude");
    std::fs::write(&native, FAKE_NATIVE).unwrap();
    std::fs::set_permissions(&native, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let calls = dir.0.join("native-calls");
    let data_root = dir.0.join("launch/provider");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&data_root)
        .unwrap();

    let described = provider_op(&adapter, &home, "describe", None, json!({}));
    assert_eq!(
        described["capabilities"]["resident_session_v1"], true,
        "{described}"
    );
    let settings = json!({
        "settings_id": "proc-view-witness",
        "mode": "headless",
        "model": { "name": "claude-opus", "provider_args": ["--model", "opus"],
                   "inputs": { "prompt": null, "named": {} } },
        "launch": { "command": native, "prompt_mode": "stdin",
                    "env": { "CALLS": calls, "ACTOR_ROOT": data_root,
                             "ROOT_BASH": env!("CARGO_BIN_EXE_oulipoly-root-bash"),
                             "PROC_FACTS": PROC_FACTS } },
    });
    let policy = provider_op(&adapter, &home, "policy.evaluate", None, settings.clone());
    assert_eq!(policy["accepted"], true, "{policy}");
    let launch = json!({
        "settings_id": settings["settings_id"], "mode": settings["mode"], "model": settings["model"],
        "argv": policy["argv"], "env": policy["env"],
    });
    let prepared = provider_op(
        &adapter,
        &home,
        "resident.prepare",
        Some(&data_root),
        json!({ "protocol": "oulipoly.resident_session/v1", "launch": launch }),
    );
    assert_eq!(prepared["invocation"]["endpoint"], "stdio", "{prepared}");
    let mut argv = vec![adapter.clone()];
    argv.extend(
        prepared["invocation"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().to_owned()),
    );

    let mut spec = spec(
        &dir,
        1,
        json!([{ "id": "provider", "argv": argv, "endpoint": "stdio", "messages": ["first"] }]),
    );
    spec["intent"]["cwd"] = json!(project);
    // The owner, and so the served endpoint, has only PATH and the
    // fixture's HOME.
    let wrapper = [
        "/usr/bin/env".to_owned(),
        "-i".to_owned(),
        "PATH=/usr/bin:/bin".to_owned(),
        format!("HOME={}", home.display()),
        SUPERVISOR.to_owned(),
    ];
    let mut run = Run::start_in(&dir, &spec, &wrapper);
    let launched = run.event("provider", "launched");
    let harness = launched["pid"].as_i64().unwrap();
    let harness_ns = std::fs::read_link(format!("/proc/{harness}/ns/pid")).unwrap();
    let ack0 = run.until("ack 0", |value| {
        value["event"] == "ack" && value["index"] == 0
    });
    let reply0 = run.until("reply 0", |value| {
        value["event"] == "agent-message" && value["input"] == 0
    });
    assert_eq!(reply0["text"], "reply to first", "{reply0}");
    let turn0 = run.until("turn-end 0", |value| {
        value["event"] == "turn-end" && value["input"] == 0
    });
    assert_eq!(turn0["message_id"], ack0["message_id"], "{turn0}");
    run.control(&json!({ "cmd": "send", "text": "second", "ref": "f1" }).to_string());
    run.event("provider", "follow-up-admitted");
    let reply1 = run.until("reply 1", |value| {
        value["event"] == "agent-message" && value["input"] == 1
    });
    assert_eq!(reply1["text"], "reply to second", "{reply1}");
    let turn1 = run.until("turn-end 1", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    assert_eq!(turn1["session"], turn0["session"]);
    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert_eq!(terminal["status"], "closed", "{terminal}");
    let accepted = owner_event(&seen, "bash-accepted");
    assert_eq!(accepted.len(), 2, "{seen:#?}");
    for (input, accepted) in accepted.iter().enumerate() {
        assert_eq!(accepted["harness"], "provider", "{accepted}");
        assert_eq!(accepted["inputs_open"][0]["index"], input, "{accepted}");
    }
    let ended = owner_event(&seen, "bash-ended");
    assert_eq!(ended.len(), 2, "{seen:#?}");
    assert!(
        ended.iter().all(|end| end["status"] == "code:0"),
        "{ended:?}"
    );
    assert_eq!(terminal["owed"], 0);
    assert_eq!(terminal["all_harnesses_reaped"], true);

    let natives: Vec<Value> = std::fs::read_to_string(&calls)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(natives.len(), 2, "{natives:?}");
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
    for call in &natives {
        println!("native: {call}");
        let pid = call["pid"].as_i64().unwrap();
        // Its own /proc names it, with no host pid fields, in the harness's
        // PID namespace; it leads its own process group (the actor).
        assert_eq!(call["proc_self_pid"], pid, "{call}");
        assert_eq!(call["nspid"], json!([pid.to_string()]), "{call}");
        assert_eq!(
            Path::new(call["pidns"].as_str().unwrap()),
            harness_ns,
            "{call}"
        );
        assert_eq!(call["pgid"], pid, "{call}");
        // The provider recorded exactly that incarnation as its actor.
        let expected = format!(
            "linux:{}:{}",
            boot.trim(),
            call["start_ticks"].as_str().unwrap()
        );
        let actors = call["actors"].as_array().unwrap();
        assert_eq!(actors.len(), 1, "{call}");
        assert_eq!(actors[0]["incarnation"], expected.as_str(), "{call}");
        assert_eq!(call["bash"]["exit"], 0, "{call}");
        let bash = proc_facts(call["bash"]["stdout"].as_str().unwrap());
        assert_eq!(bash["self"], "2", "{call}");
        assert_eq!(bash["own"], "2", "{call}");
        assert_eq!(bash["nspid"], "2", "{call}");
        assert_ne!(Path::new(&bash["ns"]), harness_ns, "{call}");
    }
}

/// Starts a root whose one harness answers `echo:first`, waits until that
/// input's tagged turn end, then kills the owner. Returns the survivor's
/// host pid; the root PID 1 and the peer outlive the owner.
fn settled_then_owner_killed(dir: &Scratch, extra: &[&str]) -> u64 {
    let mut first = Run::start(
        dir,
        &spec(
            dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), extra), "messages": ["echo:first"] }]),
        ),
    );
    let pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    let negotiated = first.event("h", "negotiated");
    assert_eq!(
        negotiated["live_reattach"],
        extra.contains(&"--live-reattach") || extra.contains(&"--live-reattach-first"),
        "{negotiated}"
    );
    if !extra.contains(&"--no-idle") {
        first.event("h", "turn-end");
    } else {
        first.event("h", "ack");
    }
    first.kill();
    assert!(alive(pid), "owner death kills nothing");
    pid
}

fn prompts(dir: &Scratch) -> usize {
    read_state(&dir.state("h"))["prompts"]
        .as_array()
        .unwrap()
        .len()
}

/// R1, B2: an explicit `continue-attached` recovery of a live parent whose
/// one input is settled (acknowledged, tagged turn end recorded), whose
/// harness declared the live reattachment contract, negotiates with the
/// same live process again and resumes its recorded session without
/// resubmitting anything. A new caller input is durably admitted under the
/// new generation before it is delivered, and answered in the same native
/// session. After a further owner death, the same caller `ref` retried
/// against a third owner is reported as the earlier admission and is not
/// delivered again. Close then ends the survivor like a first owner's close.
#[test]
fn continue_attached_reopens_a_settled_survivor_that_declared_live_reattach() {
    let dir = Scratch::new("reopen");
    let pid = settled_then_owner_killed(&dir, &["--live-reattach"]);
    assert_eq!(prompts(&dir), 1);

    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    assert_eq!(
        second.until("custody", |value| value["event"] == "custody")["outcome"],
        "attached"
    );
    assert_eq!(second.event("h", "reattached")["pid"].as_u64(), Some(pid));
    let reopened = second.event("h", "recovered-conversation");
    assert_eq!(reopened["state"], "live-usable", "{reopened}");
    assert_eq!(reopened["turns"], "settled");
    assert_eq!(reopened["capability"], "declared");
    assert_eq!(reopened["session"], "sess-1");
    assert_eq!(prompts(&dir), 1, "nothing settled is resubmitted");
    second.control(&json!({ "cmd": "send", "text": "echo:second", "ref": "r-2" }).to_string());
    let admitted = second.event("h", "follow-up-admitted");
    assert_eq!(admitted["input"], 1, "{admitted}");
    assert_eq!(admitted["durable"], true);
    let reply = second.until("reply to input 1", |value| {
        value["event"] == "agent-message" && value["input"] == 1
    });
    assert_eq!(reply["text"], "second", "{reply}");
    assert_eq!(reply["session"], "sess-1");
    second.until("turn-end of input 1", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    let seen = second.kill();
    assert!(events(&seen, "h", "launched").is_empty());
    assert!(events(&seen, "h", "session-opened").is_empty());
    assert!(alive(pid));
    assert_eq!(prompts(&dir), 2);
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message WHERE idx = 1 AND caller_ref = 'r-2'
             AND admitted_generation = 2 AND ack_generation = 2
             AND turn_end_generation = 2"
        ),
        1
    );
    drop(conn);

    let mut third = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let reopened = third.event("h", "recovered-conversation");
    assert_eq!(reopened["state"], "live-usable", "{reopened}");
    third.control(&json!({ "cmd": "send", "text": "echo:second", "ref": "r-2" }).to_string());
    let duplicate = third.event("h", "follow-up-duplicate");
    assert_eq!(duplicate["earlier"]["input"], 1, "{duplicate}");
    assert_eq!(duplicate["earlier"]["admitted_generation"], 2);
    assert_eq!(duplicate["earlier"]["acknowledged"], true);
    assert_eq!(duplicate["earlier"]["turn_end"], "tagged-idle-recorded");
    third.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = third.terminal();
    assert!(events(&seen, "h", "follow-up-admitted").is_empty());
    assert_eq!(events(&seen, "h", "close-stopping").len(), 1);
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    assert_eq!(terminal["owed"], 0);
    let record = harness(&terminal, "h");
    assert_eq!(record["exits"], json!(["signal:9"]), "{record}");
    assert_eq!(record["launches"], 0);
    assert_eq!(record["reattached"], 1);
    assert_eq!(record["recovered_conversation"]["state"], "live-usable");
    assert_eq!(record["messages"].as_array().unwrap().len(), 2);
    assert!(!alive(pid));
    let peer = read_state(&dir.state("h"));
    assert_eq!(peer["prompts"].as_array().unwrap().len(), 2, "{peer}");
    assert_eq!(peer["sessions"], json!(["sess-1"]));
    assert_eq!(
        peer["launches"].as_array().unwrap().len(),
        1,
        "same process"
    );
}

/// R1, B1 floor (the reported failure: follow-up refused as
/// `not-in-conversation`, close holding the survivor until cancel): a
/// settled survivor whose harness never declared live reattachment is not
/// connected to. The recovery says so, a `send` is refused naming that
/// state, and a close ends the survivor through its work PID 1 and the run
/// closes, without a cancel.
#[test]
fn continue_attached_without_declared_reattach_is_unavailable_and_close_ends_it() {
    let dir = Scratch::new("floor");
    let pid = settled_then_owner_killed(&dir, &[]);
    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let found = second.event("h", "recovered-conversation");
    assert_eq!(found["state"], "unavailable", "{found}");
    assert_eq!(found["reason"], "capability-absent");
    assert_eq!(found["turns"], "settled");
    second.event("h", "holding-survivor");
    second.control(&json!({ "cmd": "send", "text": "echo:second", "ref": "r-2" }).to_string());
    let refused = second.until("follow-up-refused", |value| {
        value["event"] == "follow-up-refused"
    });
    assert_eq!(refused["reason"], "conversation-unavailable", "{refused}");
    assert_eq!(refused["conversation"]["reason"], "capability-absent");
    second.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = second.terminal();
    let stopping = events(&seen, "h", "close-stopping");
    assert_eq!(stopping.len(), 1, "{seen:#?}");
    assert_eq!(stopping[0]["held"], true);
    for nothing in [
        "reopen-attempt",
        "negotiated",
        "session-resumed",
        "ack",
        "launched",
    ] {
        assert!(events(&seen, "h", nothing).is_empty(), "{nothing}");
    }
    assert!(
        seen.iter()
            .all(|value| value["event"] != "cancel-requested")
    );
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    let record = harness(&terminal, "h");
    assert_eq!(record["exits"], json!(["signal:9"]), "{record}");
    assert_eq!(record["close"], "owner-stop-attempted-after-turns-ended");
    assert_eq!(record["recovered_conversation"]["state"], "unavailable");
    assert_eq!(prompts(&dir), 1);
    assert!(!alive(pid));
}

/// An acknowledged input whose turn end was never tagged leaves the
/// survivor's turn state unknown: even with the contract declared, no
/// conversation is attempted, a `send` is refused naming that state, and
/// a close is not applied to it (a close never cuts a turn that may be
/// open). Only cancel ends it.
#[test]
fn continue_attached_with_an_unended_turn_is_unknown_and_close_is_not_applied() {
    let dir = Scratch::new("unknownturn");
    let pid = settled_then_owner_killed(&dir, &["--live-reattach", "--no-idle"]);
    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let found = second.event("h", "recovered-conversation");
    assert_eq!(found["state"], "unknown", "{found}");
    assert_eq!(found["reason"], "turn-end-unrecorded");
    assert_eq!(found["capability"], "declared");
    second.control(&json!({ "cmd": "send", "text": "echo:second" }).to_string());
    let refused = second.until("follow-up-refused", |value| {
        value["event"] == "follow-up-refused"
    });
    assert_eq!(refused["conversation"]["state"], "unknown", "{refused}");
    second.control(r#"{"cmd":"close"}"#);
    let not_applied = second.event("h", "close-not-applied");
    assert_eq!(not_applied["reason"], "turn-state-unknown");
    std::thread::sleep(QUIET_WINDOW);
    assert!(alive(pid), "close does not cut a possibly open turn");
    second.cancel();
    let (terminal, status, seen) = second.terminal();
    assert!(events(&seen, "h", "reopen-attempt").is_empty());
    assert!(events(&seen, "h", "close-stopping").is_empty());
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert_eq!(status.code(), Some(2));
    assert_eq!(harness(&terminal, "h")["exits"], json!(["signal:9"]));
    assert_eq!(prompts(&dir), 1);
}

/// ROOT's gate-only recovery goal: a retry refusal cannot settle an earlier
/// unknown insertion for admission or ordinary close, under either capability
/// declaration. This exercises the owner that actually receives the refusal.
#[test]
fn retry_refusal_with_declared_reattach_keeps_prior_turn_unresolved() {
    retry_refusal_keeps_prior_turn_unresolved(false);
}

#[test]
fn retry_refusal_without_reattach_keeps_prior_turn_unresolved() {
    retry_refusal_keeps_prior_turn_unresolved(true);
}

fn retry_refusal_keeps_prior_turn_unresolved(absent: bool) {
    let dir = Scratch::new("retry-refusal-unknown");
    // A work changes cwd; /proc/self/cwd names that new cwd there. Use
    // the physical owned path for the script and its data, retaining the
    // short host IPC path independently.
    let physical = dir.0.canonicalize().unwrap();
    let script = physical.join("peer.py");
    std::fs::write(&script, include_str!("fixtures/unknown_turn_retry_peer.py")).unwrap();
    let mut argv = vec![
        "/usr/bin/python3".to_owned(),
        script.display().to_string(),
        physical.display().to_string(),
    ];
    if absent {
        argv.push("--no-reattach".to_owned());
    }
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": argv, "messages": ["original"] }]),
        ),
    );
    let pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    first.event("h", "session-opened");
    wait_file(&dir.0.join("prior-insertion.json"));
    first.kill();
    assert!(alive(pid));

    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    assert_eq!(second.event("h", "rejected")["code"], -32011);
    second.control(r#"{"cmd":"send","ref":"distinct","text":"new caller"}"#);
    let decision = second.until("admission decision", |v| {
        v["event"] == "follow-up-refused" || v["event"] == "follow-up-admitted"
    });
    assert_eq!(decision["event"], "follow-up-refused", "{decision}");
    second.control(r#"{"cmd":"close"}"#);
    assert_eq!(
        second.event("h", "close-not-applied")["reason"],
        "delivery-unresolved"
    );
    assert!(alive(pid), "ordinary close must preserve the unknown turn");
    assert!(alive(u64::from(second.supervisor_pid())));
    second.cancel();
    let (terminal, status, seen) = second.terminal();
    assert_eq!(status.code(), Some(2), "{terminal}");
    assert_eq!(terminal["status"], "cancelled");
    assert_eq!(harness(&terminal, "h")["close"], "not-stopped-by-close");
    assert!(events(&seen, "h", "close-stopping").is_empty());
    let conn = db(&dir);
    assert_eq!(count(&conn, "SELECT count(*) FROM message"), 1);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM attempt WHERE outcome = 'unknown-prior-owner'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM attempt WHERE outcome = 'rejected-unresolved'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message WHERE ack_generation IS NOT NULL OR turn_end_generation IS NOT NULL"
        ),
        0
    );
    let wire = std::fs::read_to_string(dir.0.join("wire.jsonl")).unwrap();
    let requests: Vec<Value> = wire
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|v| v["direction"] == "recv" && v["value"]["method"] == "session/prompt")
        .collect();
    assert_eq!(requests.len(), 2, "no new caller reached the peer");
    assert_eq!(
        requests[0]["value"]["params"]["_meta"],
        requests[1]["value"]["params"]["_meta"]
    );
}

/// The declaration recorded at the first negotiation is necessary, not
/// sufficient: a harness that does not declare it again when the new owner
/// negotiates is not conversed with (`capability-withdrawn`), and its
/// recovery falls back to the held floor that a close ends.
#[test]
fn reattachment_not_declared_again_falls_back_to_the_held_floor() {
    let dir = Scratch::new("withdrawn");
    let pid = settled_then_owner_killed(&dir, &["--live-reattach-first"]);
    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    second.event("h", "reopen-attempt");
    let found = second.event("h", "recovered-conversation");
    assert_eq!(found["state"], "unavailable", "{found}");
    assert_eq!(found["reason"], "capability-withdrawn");
    second.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = second.terminal();
    assert!(events(&seen, "h", "session-resumed").is_empty());
    assert_eq!(events(&seen, "h", "close-stopping").len(), 1);
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    assert_eq!(prompts(&dir), 1);
    assert!(!alive(pid));
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT live_reattach FROM work WHERE kind = 'harness'"
        ),
        0,
        "the current negotiation's declaration is recorded"
    );
}

/// After owner loss, a reopened parent asks for a registered child from a
/// new input admitted under the new generation (not from a descendant
/// that predates the loss): the child is admitted by the new owner with
/// lineage to the same surviving parent work, answers, and its answer
/// reaches the parent's turn.
#[test]
fn reopened_parent_requests_a_child_from_new_input() {
    let dir = Scratch::new("reopen-child");
    let mut first = Run::start(
        &dir,
        &child_spec(
            &dir,
            json!([{ "id": "parent", "argv": peer(&dir.state("parent"), &["--live-reattach"]), "messages": ["echo:first"] }]),
            json!({ "echo": peer_route(&dir, "child", &[]) }),
            4,
            2,
        ),
    );
    let work = first.event("parent", "launched")["work"].clone();
    first.event("parent", "turn-end");
    first.kill();

    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let reopened = second.event("parent", "recovered-conversation");
    assert_eq!(reopened["state"], "live-usable", "{reopened}");
    second.control(
        &json!({ "cmd": "send", "text": "explore:echo:echo:after owner loss", "ref": "c-1" })
            .to_string(),
    );
    assert_eq!(second.event("parent", "follow-up-admitted")["input"], 1);
    let accepted = second.until("child accepted", |value| value["event"] == "child-accepted");
    assert_eq!(accepted["parent_work"], work, "{accepted}");
    second.until("parent turn end of input 1", |value| {
        value["harness"] == "parent" && value["event"] == "turn-end" && value["input"] == 1
    });
    second.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = second.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    let result = child_result(&seen)[0];
    assert_eq!(result["outcome"], "answered", "{result}");
    assert_eq!(result["answer"], "after owner loss");
    let answer = events(&seen, "parent", "agent-message")
        .into_iter()
        .find(|value| value["input"] == 1)
        .unwrap();
    assert!(
        answer["text"]
            .as_str()
            .unwrap()
            .contains("after owner loss"),
        "{answer}"
    );
    let conn = db(&dir);
    let (admitted, parent_work): (i64, i64) = conn
        .query_row(
            "SELECT admitted_generation, parent_work FROM child",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(admitted, 2);
    assert_eq!(Some(parent_work), work.as_i64());
    assert_eq!(
        read_state(&dir.state("parent"))["launches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

/// A reopened survivor that ends after inserting a new input without
/// acknowledging it leaves that input owed: its observed exit is a
/// closure, and the input is relaunched with the same key like any owed
/// input, in the recorded session (the peer's dedup returns the earlier
/// insertion). It is never dropped because the conversation was a reopened
/// one.
#[test]
fn reopened_conversation_closed_with_new_input_owed_relaunches_it_with_the_same_key() {
    let dir = Scratch::new("reopen-closure");
    let pid = settled_then_owner_killed(
        &dir,
        &["--live-reattach", "--on-reinit", "exit-before-ack-once"],
    );
    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    assert_eq!(
        second.event("h", "recovered-conversation")["state"],
        "live-usable"
    );
    second.control(&json!({ "cmd": "send", "text": "echo:second", "ref": "r-2" }).to_string());
    assert_eq!(second.event("h", "follow-up-admitted")["input"], 1);
    let closure = second.event("h", "closure-observed");
    assert_eq!(closure["index"], 1, "{closure}");
    let relaunch = second.event("h", "relaunch");
    assert_eq!(relaunch["same_key"], true);
    let launched = second.event("h", "launched");
    assert_ne!(launched["pid"].as_u64(), Some(pid));
    let ack = second.until("ack of input 1", |value| {
        value["event"] == "ack" && value["index"] == 1
    });
    assert_eq!(ack["recovered"], true, "{ack}");
    second.until("turn-end of input 1", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    second.control(r#"{"cmd":"close"}"#);
    let (terminal, status, _) = second.terminal();
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    let record = harness(&terminal, "h");
    assert_eq!(record["messages"][1]["closures"], 1, "{record}");
    let peer = read_state(&dir.state("h"));
    assert_eq!(peer["insertions"].as_array().unwrap().len(), 2, "{peer}");
    assert_eq!(peer["sessions"], json!(["sess-1"]));
}

/// One background (`async`) Bash run asked for by the harness's own
/// descendant (the test-owned `fixtures/async_requester.py`, started by the
/// peer's `spawn:`), which waits for `gate` and then writes `mark`.
struct AsyncRun {
    out: PathBuf,
    gate: PathBuf,
    mark: PathBuf,
}

impl AsyncRun {
    fn new(dir: &Scratch, name: &str) -> Self {
        let script = dir.0.join("async_requester.py");
        if !script.exists() {
            std::fs::write(&script, include_str!("fixtures/async_requester.py")).unwrap();
        }
        Self {
            out: dir.0.join(format!("{name}.replies")),
            gate: dir.0.join(format!("{name}.gate")),
            mark: dir.0.join(format!("{name}.ended")),
        }
    }

    /// The intent message that makes the harness ask for it.
    fn message(&self, dir: &Scratch) -> String {
        format!(
            "spawn:/usr/bin/python3 {} {} {} {}",
            dir.0.join("async_requester.py").display(),
            self.out.display(),
            self.gate.display(),
            self.mark.display()
        )
    }

    /// Its accepted work, once the requester was told `detached`.
    fn work(&self) -> i64 {
        let replies: Value = serde_json::from_str(&wait_file(&self.out)).unwrap();
        let replies = replies.as_array().unwrap();
        assert_eq!(replies.last().unwrap()["event"], "detached", "{replies:?}");
        assert_eq!(replies[0]["delivery"], "async", "{replies:?}");
        replies[0]["work"].as_i64().unwrap()
    }

    /// Lets it end, and waits until it ran to its end.
    fn end(&self) {
        std::fs::write(&self.gate, "").unwrap();
        wait_file(&self.mark);
    }
}

fn owner_events<'a>(seen: &'a [Value], event: &str, work: i64) -> Vec<&'a Value> {
    seen.iter()
        .filter(|value| value["event"] == event && value["work"] == work)
        .collect()
}

fn inherited_record(account: &Value, work: i64) -> &Value {
    account["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["work"] == work)
        .unwrap_or_else(|| panic!("no inherited record of work {work}: {account}"))
}

/// R2-A: a known async promise of a parent whose owner died is kept for the
/// same requester work once that conversation is recovered live-usable.
/// Run X ends while no owner is attached (end before any offer); run Y is
/// still live at recovery and ends under the second owner. Each completion
/// is admitted exactly once under generation 2 (never by generation 1),
/// carried through ACK and tagged turn end on the same surviving process.
/// A third owner, after another owner death, offers neither again and
/// accounts both as `turn-ended` with their generation-2 admission.
#[test]
fn known_async_promise_is_recovered_once_for_the_same_requester_work() {
    let dir = Scratch::new("recovered-async");
    let (x, y) = (AsyncRun::new(&dir, "x"), AsyncRun::new(&dir, "y"));
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &["--live-reattach"]),
                     "messages": [x.message(&dir), y.message(&dir)] }]),
        ),
    );
    let pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    let parent = first.event("h", "launched")["work"].as_i64().unwrap();
    let (x_work, y_work) = (x.work(), y.work());
    first.until("turn-end of input 1", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    let seen = first.kill();
    assert_eq!(owner_events(&seen, "async-owed", x_work).len(), 1);
    assert!(
        seen.iter()
            .all(|value| value["event"] != "bash-async-completion-offered"),
        "nothing ended under the first owner"
    );
    x.end();
    // Its work PID 1's receipt follows its end; give root PID 1 its write.
    std::thread::sleep(QUIET_WINDOW);
    assert!(alive(pid));

    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let account = second.until("inherited account", |value| {
        value["event"] == "inherited-completion-account"
    });
    let before = inherited_record(&account["inherited_async"], x_work);
    assert_eq!(before["completion"], "delivery-unknown", "{before}");
    assert_eq!(before["admission"]["state"], "never-admitted");
    assert_eq!(before["requester_work"], parent);
    let reopened = second.event("h", "recovered-conversation");
    assert_eq!(reopened["state"], "live-usable", "{reopened}");
    let recovered = second.until("x recovered", |value| {
        value["event"] == "bash-async-completion-recovered" && value["work"] == x_work
    });
    assert_eq!(recovered["accepted_generation"], 1, "{recovered}");
    assert_eq!(recovered["recipient_work"], parent);
    let admitted = second.until("x admitted", |value| {
        value["event"] == "bash-async-completion-admitted" && value["work"] == x_work
    });
    assert_eq!(
        admitted["recovered"]["accepted_generation"], 1,
        "{admitted}"
    );
    let x_input = admitted["input"].clone();
    second.until("x turn end", |value| {
        value["event"] == "turn-end" && value["input"] == x_input
    });
    second.until("x settled", |value| {
        value["event"] == "async-owed" && value["work"] == x_work && value["change"] == "turn-ended"
    });
    y.end();
    let admitted = second.until("y admitted", |value| {
        value["event"] == "bash-async-completion-admitted" && value["work"] == y_work
    });
    assert_eq!(
        admitted["recovered"]["accepted_generation"], 1,
        "{admitted}"
    );
    let y_input = admitted["input"].clone();
    second.until("y turn end", |value| {
        value["event"] == "turn-end" && value["input"] == y_input
    });
    second.until("y settled", |value| {
        value["event"] == "async-owed" && value["work"] == y_work && value["change"] == "turn-ended"
    });
    let seen = second.kill();
    for work in [x_work, y_work] {
        assert_eq!(
            owner_events(&seen, "bash-async-completion-admitted", work).len(),
            1,
            "work {work} admitted once"
        );
    }
    assert!(events(&seen, "h", "launched").is_empty(), "same process");
    assert_eq!(prompts(&dir), 4, "two intent inputs and two completions");

    let mut third = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let account = third.until("inherited account", |value| {
        value["event"] == "inherited-completion-account"
    });
    for work in [x_work, y_work] {
        let record = inherited_record(&account["inherited_async"], work);
        assert_eq!(record["completion"], "turn-ended", "{record}");
        assert_eq!(record["resolved_generation"], 2);
        assert_eq!(record["accepted_generation"], 1);
        assert_eq!(record["admission"]["admitted_generation"], 2);
        assert_eq!(record["admission"]["acknowledged"], true);
    }
    assert_eq!(
        third.event("h", "recovered-conversation")["state"],
        "live-usable"
    );
    std::thread::sleep(QUIET_WINDOW);
    third.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = third.terminal();
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    for nothing in [
        "bash-async-completion-recovered",
        "bash-async-completion-admitted",
        "bash-async-completion-not-reoffered",
    ] {
        assert!(
            seen.iter().all(|value| value["event"] != nothing),
            "{nothing}: {seen:#?}"
        );
    }
    assert_eq!(terminal["async"]["recovered"], 0);
    assert_eq!(prompts(&dir), 4, "nothing offered again");
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message WHERE completion_work IS NOT NULL
             AND producer = 'owner' AND admitted_generation = 2"
        ),
        2
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM bash_run WHERE delivery_mode = 'async'
             AND completion_outcome = 'turn-ended' AND completion_generation = 2"
        ),
        2
    );
}

/// An admitted completion is never admitted again. (1) Its ACK and tagged
/// turn end were durable when its owner was lost before resolving the
/// completion (this cut is produced by clearing that one resolution in the
/// store while no owner runs): the next claim reconciles it `turn-ended`.
/// (2) Its insertion was never acknowledged: it stays `delivery-unknown`
/// across two further owners, with its one admission; the existing owed
/// input resubmission of that same message (same key, at-most-once
/// unproven) is not a second logical offer and settles nothing.
#[test]
fn admitted_completion_is_reconciled_or_stays_unknown_never_readmitted() {
    // (1) ACK and tagged turn end durable, completion unresolved.
    let dir = Scratch::new("admitted-settled");
    let x = AsyncRun::new(&dir, "x");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &["--live-reattach"]),
                     "messages": [x.message(&dir)] }]),
        ),
    );
    let x_work = x.work();
    first.event("h", "turn-end");
    x.end();
    first.until("x settled", |value| {
        value["event"] == "async-owed" && value["work"] == x_work && value["change"] == "turn-ended"
    });
    first.kill();
    let conn = rusqlite::Connection::open(dir.store().join("intent.sqlite3")).unwrap();
    conn.execute(
        "UPDATE bash_run SET completion_outcome = NULL, completion_reason = NULL,
                             completion_generation = NULL WHERE work = ?1",
        [x_work],
    )
    .unwrap();
    drop(conn);
    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let recovered = second.until("intent-recovered", |value| {
        value["event"] == "intent-recovered"
    });
    assert_eq!(
        recovered["completions_reconciled"],
        json!([x_work]),
        "{recovered}"
    );
    assert_eq!(
        second.event("h", "recovered-conversation")["state"],
        "live-usable"
    );
    std::thread::sleep(QUIET_WINDOW);
    second.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = second.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert!(owner_events(&seen, "bash-async-completion-admitted", x_work).is_empty());
    assert!(owner_events(&seen, "bash-async-completion-recovered", x_work).is_empty());
    let record = inherited_record(&terminal["async"]["inherited"], x_work);
    assert_eq!(record["completion"], "turn-ended", "{record}");
    assert_eq!(
        record["reason"],
        "reconciled-durable-ack-and-tagged-turn-end"
    );
    assert_eq!(record["resolved_generation"], 2);
    assert_eq!(record["admission"]["admitted_generation"], 1);
    assert_eq!(prompts(&dir), 2);

    // (2) Admitted, never acknowledged.
    let dir = Scratch::new("admitted-unknown");
    let x = AsyncRun::new(&dir, "x");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h",
                     "argv": peer(&dir.state("h"), &["--live-reattach", "--silent-after-acks", "1"]),
                     "messages": [x.message(&dir)] }]),
        ),
    );
    let pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    let x_work = x.work();
    first.event("h", "turn-end");
    x.end();
    let admitted = first.until("x admitted", |value| {
        value["event"] == "bash-async-completion-admitted" && value["work"] == x_work
    });
    assert_eq!(admitted["input"], 1);
    wait_state(&dir.state("h"), |state| {
        state["prompts"]
            .as_array()
            .is_some_and(|prompts| prompts.len() == 2)
    });
    first.kill();
    for generation in [2, 3] {
        let mut owner = Run::start(&dir, &recover_for(&dir, "continue-attached"));
        let account = owner.until("inherited account", |value| {
            value["event"] == "inherited-completion-account"
        });
        let record = inherited_record(&account["inherited_async"], x_work);
        assert_eq!(record["completion"], "delivery-unknown", "{record}");
        assert_eq!(record["admission"]["admitted_generation"], 1);
        assert_eq!(record["admission"]["acknowledged"], false);
        assert_eq!(owner.event("h", "reattached")["pid"].as_u64(), Some(pid));
        // The owed message itself is resubmitted with its key (existing path).
        owner.until("resubmission attempt", |value| {
            value["event"] == "session-resumed"
        });
        std::thread::sleep(QUIET_WINDOW);
        let seen = if generation == 2 {
            owner.kill()
        } else {
            owner.cancel();
            let (terminal, status, seen) = owner.terminal();
            assert_eq!(status.code(), Some(2), "{terminal}");
            let record = inherited_record(&terminal["async"]["inherited"], x_work);
            assert_eq!(record["completion"], "delivery-unknown", "{record}");
            seen
        };
        for nothing in [
            "bash-async-completion-recovered",
            "bash-async-completion-admitted",
            "bash-async-completion-reconciled",
        ] {
            assert!(
                seen.iter().all(|value| value["event"] != nothing),
                "{nothing}: {seen:#?}"
            );
        }
        assert!(
            seen.iter()
                .all(|value| value["event"] != "recovered-conversation"),
            "an owed input is not a settled survivor"
        );
    }
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM message WHERE completion_work IS NOT NULL"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM bash_run WHERE delivery_mode = 'async' AND completion_outcome IS NULL"
        ),
        1
    );
}

/// R2-B floor: without the declared live reattachment contract, the
/// recovered survivor is held and its never-admitted promise is not offered;
/// it stays `delivery-unknown` while the requester work lives. A close then
/// ends that requester work, and only once its end is recorded is the
/// promise `undelivered` (`requester-ended-before-admission`).
#[test]
fn absent_capability_keeps_promise_unknown_until_its_requester_ends() {
    let dir = Scratch::new("recovered-async-absent");
    let x = AsyncRun::new(&dir, "x");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &[]), "messages": [x.message(&dir)] }]),
        ),
    );
    let pid = first.event("h", "launched")["pid"].as_u64().unwrap();
    let x_work = x.work();
    first.event("h", "turn-end");
    first.kill();
    x.end();
    std::thread::sleep(QUIET_WINDOW);
    let mut second = Run::start(&dir, &recover_for(&dir, "continue-attached"));
    let found = second.event("h", "recovered-conversation");
    assert_eq!(found["state"], "unavailable", "{found}");
    assert_eq!(found["reason"], "capability-absent");
    let record = inherited_record(&found["inherited_async"], x_work);
    assert_eq!(record["completion"], "delivery-unknown", "{record}");
    assert_eq!(record["admission"]["state"], "never-admitted");
    second.event("h", "holding-survivor");
    std::thread::sleep(QUIET_WINDOW);
    second.control(r#"{"cmd":"close"}"#);
    let undelivered = second.until("requester-ended undelivered", |value| {
        value["event"] == "inherited-completion-undelivered"
    });
    assert_eq!(undelivered["works"], json!([x_work]), "{undelivered}");
    let (terminal, status, seen) = second.terminal();
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    assert!(!alive(pid));
    assert!(seen.iter().all(|value| {
        value["event"] != "bash-async-completion-admitted"
            && value["event"] != "bash-async-completion-recovered"
    }));
    let record = inherited_record(&terminal["async"]["inherited"], x_work);
    assert_eq!(record["completion"], "undelivered", "{record}");
    assert_eq!(record["reason"], "requester-ended-before-admission");
    assert_eq!(record["admission"]["state"], "never-admitted");
    assert_eq!(prompts(&dir), 1, "nothing offered");
}

/// The fresh path is unchanged: a first owner's own completion is linked
/// to its work, admitted once, carried through ACK and tagged turn end. A
/// close requested while it is still owed is reported deferred (and then
/// applied), not refused.
#[test]
fn fresh_completion_is_linked_and_close_waits_for_it_as_deferred() {
    let dir = Scratch::new("fresh-async");
    let x = AsyncRun::new(&dir, "x");
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            3,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &[]), "messages": [x.message(&dir)] }]),
        ),
    );
    let x_work = x.work();
    run.event("h", "turn-end");
    run.control(r#"{"cmd":"close"}"#);
    let deferred = run.event("h", "close-deferred");
    assert_eq!(deferred["reason"], "async-completion-owed", "{deferred}");
    x.end();
    let admitted = run.until("x admitted", |value| {
        value["event"] == "bash-async-completion-admitted" && value["work"] == x_work
    });
    assert_eq!(admitted["recovered"], Value::Null, "{admitted}");
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(status.code(), Some(7));
    assert!(events(&seen, "h", "close-not-applied").is_empty());
    assert_eq!(events(&seen, "h", "close-stopping").len(), 1);
    assert_eq!(terminal["async"]["accepted"], 1, "{terminal}");
    assert_eq!(terminal["async"]["turn_ended"], 1);
    assert_eq!(terminal["async"]["recovered"], 0);
    assert_eq!(terminal["async"]["owed"], 0);
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            &format!(
                "SELECT count(*) FROM message WHERE completion_work = {x_work}
                 AND producer = 'owner' AND turn_end_generation = 1"
            )
        ),
        1
    );
}

// ---------------------------------------------------------------------------
// Root control face: `oulipoly.session_control/v3` at the owner (see the
// crate's `control` module). Deterministic peers and unprivileged
// namespaces only; claims are checked with the shared contract's own
// reference operations, not a local copy of their meaning.

use agent_provider_contract::session_control as sc;

/// The root's requester as this owner names it: the uid its work runs as.
fn requester() -> String {
    // SAFETY: geteuid has no preconditions.
    format!("uid:{}", unsafe { libc::geteuid() })
}

fn control_request(key: &str, operation: &str, addressed: &Value) -> Value {
    json!({
        "kind": "request",
        "protocol": sc::PROTOCOL,
        "request_key": key,
        "requester": requester(),
        "addressed": addressed,
        "operation": operation,
        "scope": { "root": addressed["root"] },
    })
}

fn claim(value: &Value) -> sc::Record {
    sc::Record::decode(value).unwrap_or_else(|error| panic!("{error}: {value}"))
}

impl Run {
    /// The first line after everything seen so far that matches.
    fn until_new(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let start = self.seen.len();
        loop {
            if let Some(value) = self.seen[start..].iter().find(|value| pred(value)) {
                return value.clone();
            }
            match self.lines.recv_timeout(WATCHDOG) {
                Ok(value) => {
                    self.observe(&value);
                    self.seen.push(value);
                }
                Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    panic!("watchdog: no {what}\nseen: {:#?}", self.seen);
                }
            }
        }
    }

    /// The next record of `kind` answering `key`.
    fn answer(&mut self, kind: &str, key: &str) -> Value {
        self.until_new(&format!("{kind} for {key}"), |value| {
            value["kind"] == kind && value["request_key"] == key
        })
    }

    /// Sends a request and collects its kept answer up to the outcome.
    fn ask(&mut self, request: &Value) -> Vec<Value> {
        let key = request["request_key"].as_str().unwrap().to_owned();
        let start = self.seen.len();
        self.control(&request.to_string());
        self.answer("outcome", &key);
        self.seen[start..]
            .iter()
            .filter(|value| value["request_key"] == key.as_str() && value["kind"].is_string())
            .cloned()
            .collect()
    }

    fn inspect(&mut self) -> sc::ControlState {
        self.control(r#"{"cmd":"inspect"}"#);
        let state = self.until_new("control state", |value| value["kind"] == "control_state");
        self.until_new("settlement", |value| value["event"] == "settlement");
        match claim(&state) {
            sc::Record::ControlState(state) => state,
            other => panic!("{other:?}"),
        }
    }
}

/// The requester-side trace of one request over the claims it received.
fn trace(request: &Value, answers: &[Value]) -> sc::RequestTrace {
    let sc::Record::Request(request) = claim(request) else {
        panic!("not a request");
    };
    let mut trace = sc::RequestTrace::new(request).unwrap();
    for answer in answers {
        trace.accept(&claim(answer)).unwrap();
    }
    trace
}

fn kinds(answers: &[Value]) -> Vec<&str> {
    answers
        .iter()
        .map(|value| value["kind"].as_str().unwrap())
        .collect()
}

/// Input hold is admission/input only and release reopens it through the
/// same authority. The first turn is still running (ACKed, its tagged end
/// gated) when the hold is acknowledged; that turn then ends while held,
/// a new caller input is refused `input-held`, and after release a new
/// input is admitted. Also: negotiation from the owner's advertisement, a
/// foreign requester, a malformed record (control-only diagnostic, the run
/// continues), a key conflict, faithful replay of an identical request,
/// and the warranted settlement reading at a closed, fully settled end.
#[test]
fn input_hold_holds_new_input_only_and_release_reopens_it() {
    let dir = Scratch::new("hold");
    let gate = dir.0.join("gate");
    let state = dir.state("a");
    let gate_arg = gate.display().to_string();
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{
                "id": "a",
                "argv": peer(&state, &["--off-session-turn", "sess-x", "--off-session-after-ack", "--turn-gate", &gate_arg]),
                "messages": ["one"],
            }]),
        ),
    );
    let announcement = run.until("announcement", |value| value["event"] == "session-control");
    assert_eq!(announcement["requester"], requester());
    let local = sc::Offer {
        operations: vec![
            sc::Operation::InputHold,
            sc::Operation::InputRelease,
            sc::Operation::Close,
        ],
        reports: vec![sc::Report::Inspection],
        facts: vec![
            sc::FactType::Insertion,
            sc::FactType::TaggedEnd,
            sc::FactType::LogicalSettlement,
        ],
    };
    let selected = sc::select(&local, &announcement["advertisement"]).unwrap();
    assert!(selected.operations.contains(&sc::Operation::InputHold));
    assert!(selected.operations.contains(&sc::Operation::InputRelease));
    run.until("insertion ACK", |value| {
        value["event"] == "ack" && value["index"] == 0
    });

    let state0 = run.inspect();
    assert_eq!(state0.input.state, sc::State::InputOpen);
    assert_eq!(state0.lifecycle.state, sc::State::Open);
    assert!(state0.pending.is_empty());
    let authority = serde_json::to_value(&state0.reporter).unwrap();
    assert_ne!(authority["incarnation"], "none");

    // Requester boundary: another requester is refused, nothing changes.
    let mut foreign = control_request("k-foreign", "input_hold", &authority);
    foreign["requester"] = json!("uid:4242424");
    let answers = run.ask(&foreign);
    assert_eq!(kinds(&answers), ["refusal", "outcome"]);
    assert_eq!(answers[0]["reason"], "not_permitted");
    // A malformed record is a control-only diagnostic.
    run.control(&json!({ "kind": "request", "protocol": sc::PROTOCOL }).to_string());
    let diagnostic = run.until_new("diagnostic", |value| {
        value["event"] == "session-control-unavailable"
    });
    assert_eq!(diagnostic["diagnostic"]["reason"], "invalid_record");

    let hold = control_request("k-hold", "input_hold", &authority);
    let sc::Record::Request(typed) = claim(&hold) else {
        unreachable!()
    };
    typed.agree(&selected).unwrap();
    let held = run.ask(&hold);
    assert_eq!(
        kinds(&held),
        ["receipt", "admission", "acknowledgment", "outcome"]
    );
    assert_eq!(held[2]["from"], "input_open");
    assert_eq!(held[2]["to"], "input_held");
    assert_eq!(held[1]["responder"], authority);
    assert_eq!(held[0]["durable"], true);
    let hold_trace = trace(&hold, &held);
    run.until_new("hold applied", |value| {
        value["event"] == "input-hold" && value["held"] == true
    });
    // The running turn continues and ends while held.
    std::fs::write(&gate, b"").unwrap();
    run.until("turn end while held", |value| {
        value["event"] == "turn-end" && value["input"] == 0
    });
    run.control(&json!({ "cmd": "send", "text": "two" }).to_string());
    let refused = run.until_new("held send", |value| value["event"] == "follow-up-refused");
    assert_eq!(refused["reason"], "input-held", "{refused}");
    // Exact submitted/original conflict stays separate after original final.
    let submitted = control_request("k-hold", "input_release", &authority);
    run.control(&submitted.to_string());
    let conflict = run.until_new("submission conflict", |value| value["kind"] == "conflict");
    let sc::Record::Conflict(answer) = claim(&conflict) else {
        panic!("not conflict")
    };
    let sc::Record::Request(submitted) = claim(&submitted) else {
        panic!("not request")
    };
    answer.answer_to(&submitted).unwrap();
    let mut kept = hold_trace.clone();
    assert!(matches!(
        kept.accept(&claim(&conflict)).unwrap(),
        sc::Step::SubmissionConflict { .. }
    ));
    assert_eq!(kept, hold_trace);
    // The identical request: the kept claims, replayed unchanged.
    let replay = run.ask(&hold);
    assert_eq!(replay, held, "faithful replay");
    assert_eq!(run.inspect().input.state, sc::State::InputHeld);

    let release = control_request("k-release", "input_release", &authority);
    let released = run.ask(&release);
    assert_eq!(released[2]["from"], "input_held");
    assert_eq!(released[2]["to"], "input_open");
    run.control(&json!({ "cmd": "send", "text": "two" }).to_string());
    assert_eq!(
        run.until_new("admitted", |value| value["event"] == "follow-up-admitted")["input"],
        1
    );
    run.until("second turn end", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    let now = run.inspect();
    assert_eq!(now.input.state, sc::State::InputOpen);
    assert_eq!(now.input.since.as_ref().unwrap().request_key, "k-release");
    assert_eq!(hold_trace.relate(&now), sc::Relation::Superseded);
    assert_eq!(
        trace(&release, &released).relate(&now),
        sc::Relation::Current
    );

    run.control(r#"{"cmd":"close"}"#);
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert_eq!(terminal["control"]["lifecycle"], "closing");
    let retirement = &terminal["session_control"]["retirement"];
    assert_eq!(retirement["eligible"], true, "{terminal}");
    let observations: Vec<&Value> = seen
        .iter()
        .filter(|value| value["kind"] == "observation")
        .collect();
    assert!(!observations.is_empty());
    for value in observations {
        assert!(matches!(claim(value), sc::Record::Observation(_)));
    }
    // Recorded hold: one durable transition each way, kept in the store.
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM control WHERE kind = 'acknowledgment'"
        ),
        3
    );
}

/// The U112 countermeaning at the control consumer: an input ACKed with no
/// tagged end, close acknowledged and deferred, then the peer actually
/// exits and is waited. The run is `closed`/7, yet the warranted reading of
/// that input is `owed` and retirement is not eligible.
#[test]
fn ack_without_tagged_end_then_waited_exit_is_not_retirement() {
    let dir = Scratch::new("acknoend");
    let exit = dir.0.join("exit");
    let exit_arg = exit.display().to_string();
    let mut run = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&dir.state("a"), &["--no-idle", "--exit-when-file", &exit_arg]), "messages": ["x"] }]),
        ),
    );
    run.until("insertion ACK", |value| {
        value["event"] == "ack" && value["index"] == 0
    });
    let authority = serde_json::to_value(run.inspect().reporter).unwrap();
    let closed = run.ask(&control_request("k-close", "close", &authority));
    assert_eq!(closed[2]["to"], "closing");
    std::fs::write(&exit, b"").unwrap();
    let (terminal, status, seen) = run.terminal();
    assert_eq!(status.code(), Some(7), "{terminal}");
    assert_eq!(terminal["status"], "closed");
    assert_eq!(harness(&terminal, "a")["exits"], json!(["code:7"]));
    let summary = &terminal["session_control"];
    assert_eq!(summary["retirement"]["eligible"], false, "{summary}");
    let input = summary["subjects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subject| subject["subject"]["input"] == "0")
        .unwrap();
    assert_eq!(input["reading"]["logical"], "owed", "{input}");
    assert_eq!(input["reading"]["basis"], "warranted");
    // The physical end is reported apart from the logical reading.
    let root = summary["subjects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|subject| subject["subject"].get("input").is_none())
        .unwrap();
    assert_eq!(
        root["reading"]["physical_custody"]["reading"], "not_observed",
        "{root}"
    );
    assert_eq!(summary["recorded_actor_custody"]["state"], "exited_waited");
    // The emitted observations, read again by the shared reference reader
    // under the reporter's lineage, say the same.
    let observations: Vec<sc::Observation> = seen
        .iter()
        .filter(|value| value["kind"] == "observation")
        .map(|value| match claim(value) {
            sc::Record::Observation(observation) => observation,
            other => panic!("{other:?}"),
        })
        .collect();
    let subject = observations
        .iter()
        .find(|o| o.subject.input.as_deref() == Some("0"))
        .unwrap()
        .subject
        .clone();
    let lineage = sc::Lineage {
        root: subject.root.clone(),
        authorities: vec![observations[0].reporter.clone()],
    };
    let reading = sc::read_settlement(&subject, Some(&lineage), &observations);
    assert_eq!(reading.logical, sc::LogicalReading::Owed);
}

/// An acknowledged close and the prior ACK survive owner death. The
/// successor attaches the same incarnation through a `recover` request
/// (admitted and acknowledged `attached` by the new generation, never by
/// the dead owner), keeps input closed, reports the close as current with
/// the original request as its basis, refuses a request still addressed to
/// the dead owner as `stale_authority`, and replays the kept close claims
/// unchanged. While the old owner lived, a recover was refused `owner_live`
/// and changed nothing.
#[test]
fn acknowledged_close_survives_owner_death_and_successor_attach() {
    let dir = Scratch::new("closelives");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "a", "argv": peer(&dir.state("a"), &["--no-idle", "--live-reattach"]), "messages": ["x"] }]),
        ),
    );
    let root_pid = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let peer_pid = first.event("a", "launched")["pid"].as_u64().unwrap();
    first.until("insertion ACK", |value| {
        value["event"] == "ack" && value["index"] == 0
    });
    let a1 = serde_json::to_value(first.inspect().reporter).unwrap();
    // Discovery's describer reads the live root's authority without
    // claiming or locking its store.
    let described = std::process::Command::new(SUPERVISOR)
        .args([
            "--describe",
            &dir.store().display().to_string(),
            "--requester",
            &requester(),
            "--describer",
            "test",
        ])
        .output()
        .unwrap();
    assert!(described.status.success());
    let entry: Value = serde_json::from_slice(&described.stdout).unwrap();
    assert!(matches!(claim(&entry), sc::Record::RootEntry(_)));
    assert_eq!(
        entry["authority"], a1,
        "the store's last record is this owner"
    );
    let close = control_request("k-close", "close", &a1);
    let closed = first.ask(&close);
    assert_eq!(
        kinds(&closed),
        ["receipt", "admission", "acknowledgment", "outcome"]
    );

    // A recover while this owner holds the root: refused, nothing written.
    let recover_request = control_request("k-recover", "recover", &a1);
    let request =
        json!({ "store": dir.store(), "recover": "continue-attached", "control": recover_request });
    let (terminal, status, seen) = Run::start(&dir, &request).terminal();
    assert_eq!(status.code(), Some(65), "{terminal}");
    let refusal = seen
        .iter()
        .find(|value| value["kind"] == "refusal")
        .unwrap();
    assert_eq!(refusal["reason"], "owner_live");
    assert_eq!(
        count(
            &db(&dir),
            "SELECT count(*) FROM control WHERE request_key = 'k-recover'"
        ),
        0
    );

    first.kill();
    assert!(alive(root_pid) && alive(peer_pid));

    let mut second = Run::start(&dir, &request);
    let announcement = second.until("announcement", |value| value["event"] == "session-control");
    assert_eq!(
        announcement["lifecycle"]["state"], "closing",
        "{announcement}"
    );
    assert_eq!(announcement["lifecycle"]["since"]["request_key"], "k-close");
    let a2 = announcement["authority"].clone();
    assert_eq!(a2["root"], a1["root"]);
    assert_eq!(a2["incarnation"], a1["incarnation"]);
    assert_ne!(a2["generation"], a1["generation"]);
    assert_ne!(a2["owner"], a1["owner"]);
    let attached = second.answer("outcome", "k-recover");
    assert_eq!(attached["result"], "acknowledged", "{attached}");
    let recovered: Vec<Value> = second
        .seen
        .iter()
        .filter(|value| value["request_key"] == "k-recover" && value["kind"].is_string())
        .cloned()
        .collect();
    assert_eq!(
        kinds(&recovered),
        ["receipt", "admission", "acknowledgment", "outcome"]
    );
    assert_eq!(recovered[2]["to"], "attached");
    assert_eq!(recovered[2]["responder"], a2);
    trace(&recover_request, &recovered);
    second.until("custody", |value| {
        value["event"] == "custody" && value["outcome"] == "attached"
    });

    second.control(&json!({ "cmd": "send", "text": "late" }).to_string());
    let refused = second.until_new("closed input", |value| {
        value["event"] == "follow-up-refused"
    });
    assert_eq!(refused["reason"], "input-closed", "{refused}");
    let now = second.inspect();
    assert_eq!(now.lifecycle.state, sc::State::Closing);
    assert_eq!(trace(&close, &closed).relate(&now), sc::Relation::Current);
    let stale = second.ask(&control_request("k-stale", "input_hold", &a1));
    assert_eq!(stale[0]["reason"], "stale_authority");
    assert_eq!(stale[0]["responder"], a2);
    let replay = second.ask(&close);
    assert_eq!(
        replay, closed,
        "the successor replays the kept claims unchanged"
    );

    second.cancel();
    let (terminal, _, _) = second.terminal();
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert_eq!(terminal["session_control"]["retirement"]["eligible"], false);
}

/// The recorded incarnation's process identity no longer matches (a reused
/// pid stands in as a changed start time): a `recover` request is admitted
/// by the successor and refused `root_absent`, and nothing is started or
/// signalled before that answer.
#[test]
fn recover_of_a_mismatched_incarnation_is_root_absent_before_effects() {
    let dir = Scratch::new("recabsent");
    let mut first = Run::start(
        &dir,
        &spec(
            &dir,
            1,
            json!([{ "id": "h", "argv": peer(&dir.state("h"), &["--mode", "insert-then-silent"]), "messages": ["x"] }]),
        ),
    );
    let old_root = first.until("root pid1", |value| value["event"] == "root-pid1-started")["pid"]
        .as_u64()
        .unwrap();
    let old_peer = first.event("h", "launched")["pid"].as_u64().unwrap();
    wait_state(&dir.state("h"), |state| {
        state["insertions"].as_array().unwrap().len() == 1
    });
    let a1 = serde_json::to_value(first.inspect().reporter).unwrap();
    first.kill();
    rusqlite::Connection::open(dir.store().join("intent.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE incarnation SET start_time = start_time + 1 WHERE id = 1",
            [],
        )
        .unwrap();
    let recover_request = control_request("k-recover", "recover", &a1);
    let request =
        json!({ "store": dir.store(), "recover": "continue-attached", "control": recover_request });
    let (terminal, status, seen) = Run::start(&dir, &request).terminal();
    assert_eq!(status.code(), Some(6), "{terminal}");
    let answers: Vec<Value> = seen
        .iter()
        .filter(|value| value["request_key"] == "k-recover" && value["kind"].is_string())
        .cloned()
        .collect();
    assert_eq!(
        kinds(&answers),
        ["receipt", "admission", "refusal", "outcome"]
    );
    assert_eq!(answers[2]["reason"], "root_absent");
    assert_eq!(answers[2]["stage"], "transition");
    trace(&recover_request, &answers);
    assert!(roots_started(&seen).is_empty());
    assert!(events(&seen, "h", "launched").is_empty());
    assert!(
        alive(old_root) && alive(old_peer),
        "nothing signalled by a stored pid"
    );
    assert_eq!(terminal["session_control"]["retirement"]["eligible"], false);
}

const ROOT_BASH: &str = env!("CARGO_BIN_EXE_oulipoly-root-bash");

// Optional live v3 consumers. These use the shared decoder, selection and
// follower, and actual retained/database witnesses, not a second test wire.
use agent_provider_contract::live_stream::{self as live_wire, attachment as live_attach};
fn own_uid() -> u32 {
    unsafe { libc::geteuid() }
}
fn live_offer() -> live_wire::Offer {
    live_wire::Offer {
        channels: vec![live_wire::Channel::Combined, live_wire::Channel::Control],
        audiences: vec![live_wire::Audience::Scoped],
        max_data_bytes: 16 * 1024,
    }
}
fn with_live(mut spec: Value, grant: &str) -> Value {
    spec["live_output"] =
        json!({"grant":grant,"advertisement":live_wire::advertisement(&live_offer())});
    spec
}
fn live_read(conn: &mut BufReader<std::os::unix::net::UnixStream>) -> live_attach::Message {
    let mut line = String::new();
    assert!(
        conn.read_line(&mut line).unwrap() > 0,
        "unexpected live EOF"
    );
    live_attach::Message::decode_line(line.trim()).unwrap()
}
fn live_open(path: &str) -> BufReader<std::os::unix::net::UnixStream> {
    let start = std::time::Instant::now();
    let mut conn = loop {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(c) => break c,
            Err(e) => {
                assert!(start.elapsed() < WATCHDOG, "live setup: {e}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    };
    conn.set_read_timeout(Some(WATCHDOG)).unwrap();
    writeln!(
        conn,
        "{}",
        live_attach::Message::Hello(live_attach::Hello::new(
            live_attach::Role::Subscriber,
            &live_offer()
        ))
        .encode_line()
    )
    .unwrap();
    let mut reader = BufReader::new(conn);
    let live_attach::Message::Hello(hello) = live_read(&mut reader) else {
        panic!("broker hello");
    };
    hello
        .select(live_attach::Role::Subscriber, &live_offer())
        .unwrap();
    reader
}
fn live_directory(path: &str) -> Vec<live_wire::Descriptor> {
    let mut conn = live_open(path);
    writeln!(
        conn.get_mut(),
        "{}",
        live_attach::Message::List {}.encode_line()
    )
    .unwrap();
    let live_attach::Message::Directory(d) = live_read(&mut conn) else {
        panic!("directory");
    };
    d.streams
}
fn live_descriptor(path: &str, work: i64) -> live_wire::Descriptor {
    let start = std::time::Instant::now();
    loop {
        if let Some(d) = live_directory(path).into_iter().find(|d| {
            d.correlation.as_ref().unwrap().work.as_deref() == Some(work.to_string().as_str())
        }) {
            return d;
        }
        assert!(start.elapsed() < WATCHDOG, "optional work never registered");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn live_follow(
    path: &str,
    cursor: live_wire::Cursor,
) -> (
    BufReader<std::os::unix::net::UnixStream>,
    live_wire::Follower,
    Vec<live_wire::Record>,
) {
    let request = live_attach::Attach { cursor };
    let mut conn = live_open(path);
    writeln!(
        conn.get_mut(),
        "{}",
        live_attach::Message::Attach(request.clone()).encode_line()
    )
    .unwrap();
    let live_attach::Message::Attached(attached) = live_read(&mut conn) else {
        panic!("attached");
    };
    let selected =
        live_wire::select(&live_offer(), &live_wire::advertisement(&live_offer())).unwrap();
    let (follower, _) = live_attach::follow_attached(&selected, &request, &attached).unwrap();
    // Prefix knowledge arrived inside Attached; SDK setup consumed it.
    (conn, follower, attached.plan.prefix)
}
fn live_collect(
    mut conn: BufReader<std::os::unix::net::UnixStream>,
    mut follower: live_wire::Follower,
    mut records: Vec<live_wire::Record>,
) -> Vec<live_wire::Record> {
    if records.iter().any(|r| {
        matches!(
            r,
            live_wire::Record::Finalized(_) | live_wire::Record::Ended(_)
        )
    }) {
        return records;
    }
    loop {
        let live_attach::Message::Record { record } = live_read(&mut conn) else {
            panic!("record");
        };
        follower.accept(&record).unwrap();
        let terminal = matches!(
            record,
            live_wire::Record::Finalized(_) | live_wire::Record::Ended(_)
        );
        records.push(record);
        if terminal {
            break;
        }
    }
    records
}
fn live_bytes(records: &[live_wire::Record]) -> Vec<u8> {
    records
        .iter()
        .filter_map(|r| match r {
            live_wire::Record::Data(d) => Some(d.bytes().unwrap()),
            _ => None,
        })
        .flatten()
        .collect()
}
fn witness(dir: &Scratch, work: i64, ended: &Value) -> Value {
    let db = rusqlite::Connection::open_with_flags(
        dir.store().join("intent.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let stored: String = db
        .query_row("SELECT result FROM bash_run WHERE work=?1", [work], |r| {
            r.get(0)
        })
        .unwrap();
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored["status"], ended["status"]);
    assert_eq!(stored["work_pid1"], ended["work_pid1"]);
    assert_eq!(stored["retained"], ended["retained"]);
    assert_eq!(stored["output"], ended["output"]);
    stored
}

#[test]
fn granted_requester_follows_combined_output_to_a_custody_owner_final() {
    let dir = Scratch::new("live-v3-follow");
    let go = dir.0.join("go");
    let socket = dir.store().join("live.sock");
    let command = format!(
        r#"python3 -c 'import socket,sys; s=socket.socket(socket.AF_UNIX); s.connect(sys.argv[1]); s.sendall(b"{{\"op\":\"list\"}}\n"); print("inner:"+s.recv(4096).decode().strip())' {}; while [ ! -e {} ]; do sleep 0.02; done; seq 1 30000; echo err >&2; exit 3"#,
        socket.display(),
        go.display()
    );
    let mut run = Run::start(
        &dir,
        &with_live(
            spec(
                &dir,
                3,
                json!([{"id":"a","argv":peer(&dir.state("a"),&[]),"messages":[format!("bash:{command}")]}]),
            ),
            &format!("uid:{}", own_uid()),
        ),
    );
    let event = run.until("live configuration", |v| v["event"] == "live-output");
    assert_eq!(event["configured"], true);
    let path = event["path"].as_str().unwrap();
    let work = run.until("bash acceptance", |v| v["event"] == "bash-accepted")["work"]
        .as_i64()
        .unwrap();
    let d = live_descriptor(path, work);
    assert_eq!(d.channels, live_offer().channels);
    let (conn, follower, prefix) = live_follow(path, d.start());
    let following = std::thread::spawn(move || live_collect(conn, follower, prefix));
    std::fs::write(&go, b"").unwrap();
    let ended = run.until("bash end", |v| v["event"] == "bash-ended");
    let records = following.join().unwrap();
    assert!(matches!(
        records.last(),
        Some(live_wire::Record::Finalized(_))
    ));
    let stored = witness(&dir, work, &ended);
    assert_eq!(stored["status"], "code:3");
    assert_eq!(stored["retained"]["state"], "complete");
    let retained = std::fs::read(dir.store().join("output").join(work.to_string())).unwrap();
    assert!(retained.ends_with(b"30000\nerr\n"));
    let text = String::from_utf8_lossy(&retained);
    assert!(
        text.contains("not_authorized") && text.contains("inside-observed-root"),
        "{text}"
    );
    let delivered = live_bytes(&records);
    assert!(!delivered.is_empty());
    for r in &records {
        if let live_wire::Record::Data(d) = r {
            let bytes = d.bytes().unwrap();
            assert!(retained.windows(bytes.len()).any(|w| w == bytes));
            assert_eq!(d.channel, live_wire::DataChannel::Combined);
        }
    }
    assert!(records.iter().any(|r| matches!(
        r,
        live_wire::Record::Control(live_wire::ControlFrame {
            fact: live_wire::ControlFact::ExitObserved { code: Some(3), .. },
            ..
        })
    )));
    if let Some(live_wire::Record::Finalized(f)) = records.last() {
        assert_eq!(
            f.durable_reference,
            format!(
                "rv1w:{}:{work}",
                d.correlation.as_ref().unwrap().root.as_ref().unwrap()
            )
        );
    }
    run.cancel();
    let (owner, status, _) = run.terminal();
    assert_eq!(owner["status"], "cancelled");
    assert_eq!(status.code(), Some(2));
}

#[test]
fn stalled_subscriber_never_holds_up_the_relay_and_late_cursors_get_exact_gaps() {
    let dir = Scratch::new("live-v3-stall");
    let go = dir.0.join("go");
    let flow = dir.0.join("flow");
    const SIZE: usize = 4_000_000;
    let command = format!(
        "while [ ! -e {} ]; do sleep 0.02; done; {ROOT_BASH} -- /bin/sh -c \"while [ ! -e {} ]; do sleep 0.02; done; head -c {SIZE} /dev/zero | tr '\\0' a; echo done >&2\" > /dev/null 2>&1",
        go.display(),
        flow.display()
    );
    let mut run = Run::start(
        &dir,
        &with_live(
            spec(
                &dir,
                3,
                json!([{"id":"a","argv":peer(&dir.state("a"),&[]),"messages":[format!("spawn:{command}")]}]),
            ),
            &format!("uid:{}", own_uid()),
        ),
    );
    let event = run.until("live configuration", |v| v["event"] == "live-output");
    let path = event["path"].as_str().unwrap();
    live_directory(path);
    std::fs::write(&go, b"").unwrap();
    let work = run.until("bash acceptance", |v| v["event"] == "bash-accepted")["work"]
        .as_i64()
        .unwrap();
    let d = live_descriptor(path, work);
    let (stalled, _, _) = live_follow(path, d.start());
    std::fs::write(&flow, b"").unwrap();
    let ended = run.until("required end with nonreader", |v| {
        v["event"] == "bash-ended"
    });
    assert_eq!(ended["output_bytes"], SIZE + 5);
    assert_eq!(ended["retained"]["state"], "complete");
    witness(&dir, work, &ended);
    let retained = std::fs::read(dir.store().join("output").join(work.to_string())).unwrap();
    assert_eq!(retained.len(), SIZE + 5);
    let (conn, follower, prefix) = live_follow(path, d.start());
    let late = live_collect(conn, follower, prefix);
    assert!(late.iter().any(|r|matches!(r,live_wire::Record::Gap(g) if g.first==1 && g.reason==live_wire::GapReason::Evicted)));
    let bytes = live_bytes(&late);
    assert!(!bytes.is_empty() && bytes.len() <= 1024 * 1024);
    assert!(retained.ends_with(&bytes));
    let mut foreign = d.start();
    foreign.incarnation = "0".repeat(32);
    foreign.after_seq = 5;
    let (conn, f, prefix) = live_follow(path, foreign);
    let other = live_collect(conn, f, prefix);
    assert!(
        matches!(other.first(),Some(live_wire::Record::Discontinuity(d)) if d.previous_last_seq.is_none())
    );
    let mut ahead = d.start();
    ahead.after_seq = 100_000;
    let mut conn = live_open(path);
    writeln!(
        conn.get_mut(),
        "{}",
        live_attach::Message::Attach(live_attach::Attach { cursor: ahead }).encode_line()
    )
    .unwrap();
    assert!(
        matches!(live_read(&mut conn),live_attach::Message::Unavailable{diagnostic} if diagnostic.reason==live_wire::UnavailableReason::ProtocolViolation)
    );
    drop(stalled);
    run.cancel();
    let (terminal, _, _) = run.terminal();
    assert!(terminal["live_output"]["held"].as_u64().unwrap() <= 8 * 1024 * 1024);
}

#[test]
fn refused_or_absent_grant_disables_only_the_live_view() {
    let uid = format!("uid:{}", own_uid());
    for optional in [
        None,
        Some(json!(17)),
        Some(json!({"grant":uid,"extra":true})),
        Some(json!({"grant":uid,"advertisement":{"oulipoly.live_stream/v2":{}}})),
        Some(json!({"grant":uid,"advertisement":{"oulipoly.live_stream/v3":17}})),
        Some(
            json!({"grant":format!("uid:{}",own_uid()+1),"advertisement":live_wire::advertisement(&live_offer())}),
        ),
    ] {
        let dir = Scratch::new("live-v3-refused");
        let mut request = spec(
            &dir,
            3,
            json!([{"id":"a","argv":peer(&dir.state("a"),&["--exit-after-acks","1"]),"messages":["bash:echo ok"]}]),
        );
        if let Some(value) = &optional {
            request["live_output"] = value.clone();
        }
        let run = Run::start(&dir, &request);
        let (terminal, status, seen) = run.terminal();
        assert_eq!(status.code(), Some(0));
        assert_eq!(terminal["status"], "ended");
        let ended = owner_event(&seen, "bash-ended")[0];
        assert_eq!(ended["status"], "code:0");
        witness(&dir, ended["work"].as_i64().unwrap(), ended);
        let live = owner_event(&seen, "live-output");
        if optional.is_some() {
            assert_eq!(live[0]["listening"], false);
        } else {
            assert!(live.is_empty());
        }
        assert!(!dir.store().join("live.sock").exists());
    }
}

#[test]
fn takeover_is_a_new_incarnation_and_old_cursors_get_a_discontinuity() {
    let dir = Scratch::new("live-v3-takeover");
    let gate = dir.0.join("gate");
    let grant = format!("uid:{}", own_uid());
    let mut first = Run::start(
        &dir,
        &with_live(
            spec(
                &dir,
                3,
                json!([{"id":"a","argv":peer(&dir.state("a"),&[]),"messages":[format!("bash:echo before; while [ ! -e {} ]; do sleep 0.02; done; echo after",gate.display())]}]),
            ),
            &grant,
        ),
    );
    let event = first.until("live configuration", |v| v["event"] == "live-output");
    let path = event["path"].as_str().unwrap();
    let work = first.until("bash start", |v| v["event"] == "bash-started")["work"]
        .as_i64()
        .unwrap();
    let d = live_descriptor(path, work);
    let (mut conn, mut follower, _) = live_follow(path, d.start());
    while follower.cursor().after_seq == 0 {
        let live_attach::Message::Record { record } = live_read(&mut conn) else {
            panic!("record");
        };
        follower.accept(&record).unwrap();
        if follower.cursor().after_seq > 0 {
            break;
        }
    }
    let cursor = follower.cursor().clone();
    first.kill();
    let mut rest = String::new();
    assert_eq!(
        conn.read_line(&mut rest).unwrap(),
        0,
        "owner death supplies no invented terminal"
    );
    let mut request = recover(&dir);
    request["live_output"] =
        json!({"grant":grant,"advertisement":live_wire::advertisement(&live_offer())});
    let mut second = Run::start(&dir, &request);
    let event = second.until("live configuration", |v| v["event"] == "live-output");
    let path = event["path"].as_str().unwrap();
    second.until("reattached", |v| v["event"] == "bash-reattached");
    let new = live_descriptor(path, work);
    assert_eq!(new.stream_id, d.stream_id);
    assert_ne!(new.incarnation, d.incarnation);
    let (conn, follower, prefix) = live_follow(path, cursor);
    let following = std::thread::spawn(move || live_collect(conn, follower, prefix));
    std::fs::write(&gate, b"").unwrap();
    let ended = second.until("bash end", |v| v["event"] == "bash-ended");
    let records = following.join().unwrap();
    assert!(
        matches!(records.first(),Some(live_wire::Record::Discontinuity(d)) if d.previous_last_seq.is_none())
    );
    assert!(matches!(
        records.last(),
        Some(live_wire::Record::Finalized(_))
    ));
    let stored = witness(&dir, work, &ended);
    assert_eq!(stored["retained"]["state"], "partial");
    assert_eq!(stored["retained"]["losses"][0]["reason"], "owner-changed");
    assert_eq!(live_bytes(&records), b"after\n");
    second.cancel();
    second.terminal();
}

#[test]
fn live_prehello_and_quiet_followers_release_bounded_slots() {
    let dir = Scratch::new("live-v3-slots");
    let gate = dir.0.join("gate");
    let mut run = Run::start(
        &dir,
        &with_live(
            spec(
                &dir,
                3,
                json!([{"id":"a","argv":peer(&dir.state("a"),&[]),"messages":[format!("bash:while [ ! -e {} ]; do sleep 0.02; done; echo done",gate.display())]}]),
            ),
            &format!("uid:{}", own_uid()),
        ),
    );
    let event = run.until("configuration", |v| v["event"] == "live-output");
    let path = event["path"].as_str().unwrap();
    let work = run.until("acceptance", |v| v["event"] == "bash-accepted")["work"]
        .as_i64()
        .unwrap();
    let d = live_descriptor(path, work);
    let prehello: Vec<_> = (0..8)
        .map(|_| std::os::unix::net::UnixStream::connect(path).unwrap())
        .collect();
    std::thread::sleep(Duration::from_millis(250));
    let mut ninth = std::os::unix::net::UnixStream::connect(path).unwrap();
    ninth
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut byte = [0];
    assert_eq!(
        std::io::Read::read(&mut ninth, &mut byte).unwrap(),
        0,
        "capacity closes before creating a ninth worker"
    );
    drop(prehello);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(live_directory(path).len(), 1);
    let followers: Vec<_> = (0..8).map(|_| live_follow(path, d.start()).0).collect();
    drop(followers);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        live_directory(path).len(),
        1,
        "quiet EOF releases worker slots"
    );
    std::fs::write(&gate, b"").unwrap();
    let ended = run.until("required end", |v| v["event"] == "bash-ended");
    witness(&dir, work, &ended);
    run.cancel();
    run.terminal();
}

/// Start-time transport death takes existing relaunch policy; a session RPC
/// refusal stops with its code. Neither case submits any native input first.
#[test]
fn sdk_resident_start_death_and_refusal_remain_distinct() {
    for (mode, cap, launches, label) in [
        ("exit-at-start", 2, 2, "outage"),
        ("refuse-start", 2, 1, "session-rejected--32012"),
    ] {
        let dir = Scratch::new("sdk-start");
        let state = dir.state("h");
        let prepared = agent_provider_contract::resident_session::ResidentPrepareResult::v1(
            vec!["resident.serve".into()],
            "4".repeat(64),
        );
        let mut request = spec(
            &dir,
            cap,
            json!([{
                "id":"h", "argv":peer(&state, &["--resident-evidence", "absent", "--mode", mode]),
                "messages":["synthetic prompt"], "resident":prepared,
            }]),
        );
        request["intent"]["cwd"] = json!(std::env::current_dir().unwrap());
        let mut run = Run::start(&dir, &request);
        if mode == "refuse-start" {
            assert_eq!(run.event("h", "session-failed")["label"], label);
            run.cancel();
        }
        let (terminal, _, seen) = run.terminal();
        assert_eq!(harness(&terminal, "h")["launches"], launches, "{terminal}");
        assert_eq!(harness(&terminal, "h")["messages"][0]["label"], label);
        assert_eq!(
            events(&seen, "h", "session-peer-gone").len(),
            if mode == "exit-at-start" { 2 } else { 0 }
        );
        assert_eq!(
            events(&seen, "h", "session-failed").len(),
            if mode == "refuse-start" { 1 } else { 0 }
        );
        assert!(events(&seen, "h", "resident-start-refused").is_empty());
        assert!(events(&seen, "h", "ack").is_empty());
        assert!(read_state(&state)["prompts"].as_array().unwrap().is_empty());
        assert!(
            !serde_json::to_string(&seen)
                .unwrap()
                .contains("PRIVATE-START-PAYLOAD")
        );
    }
}

/// A prepared resident child uses the shared declaration and its requester
/// receives uncertainty and endpoint attribution, without gaining resend.
#[test]
fn sdk_resident_child_rejection_reaches_its_requester() {
    let dir = Scratch::new("sdk-child");
    let state = dir.state("child");
    let prepared = agent_provider_contract::resident_session::ResidentPrepareResult::v1(
        vec!["resident.serve".into()],
        "5".repeat(64),
    );
    let mut request = child_spec(
        &dir,
        json!([{"id":"parent", "argv":peer(&dir.state("parent"), &[]),
            "messages":["explore:echo:synthetic question"]}]),
        json!({"echo": {"harness":"prepared", "provider":"synthetic", "endpoint":"stdio",
            "slots":[{"argv":peer(&state, &["--resident-evidence", "not-inserted"]),
                "data_root":dir.0.canonicalize().unwrap().join("prepared"), "resident":prepared}]}}),
        1,
        1,
    );
    request["intent"]["cwd"] = json!(std::env::current_dir().unwrap());
    let mut run = Run::start(&dir, &request);
    run.until("child accepted", |v| v["event"] == "child-accepted");
    let rejected = run.event("child-1", "rejected");
    assert_eq!(rejected["endpoint_declaration"], "not-inserted");
    run.cancel();
    let (terminal, _, seen) = run.terminal();
    assert_eq!(terminal["all_harnesses_reaped"], true);
    assert!(events(&seen, "child-1", "ack").is_empty());
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 1);
    // Requester-stream projection is separately asserted by the ChildLink
    // socket control; this case exercises the prepared worker and custody.
    let text = serde_json::to_string(&seen).unwrap();
    assert!(!text.contains("PRIVATE-RESIDENT-PAYLOAD"));
}
