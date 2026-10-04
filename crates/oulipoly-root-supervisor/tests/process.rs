//! Process-level controls: the real supervisor binary owning real
//! deterministic-peer subprocesses. Deterministic peers prove this crate's
//! contract, not real-harness behaviour.
//!
//! The only timer here is a test watchdog: if an expected line never
//! arrives, the test kills its own supervisor (its peers die with it) and
//! fails, rather than hanging. The supervisor itself has no timer. A
//! supervisor this test launched is also killed when its `Run` is dropped
//! (including on a failed assertion) and, through a parent-death signal,
//! when the test process itself dies, so no test leaves an owner behind.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_oulipoly-root-supervisor");
const PEER: &str = env!("CARGO_BIN_EXE_oulipoly-acp-deterministic-peer");
const WATCHDOG: Duration = Duration::from_secs(60);
const QUIET_WINDOW: Duration = Duration::from_secs(2);

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "root-supervisor-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
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
    fn drop(&mut self) {
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
}

impl Run {
    fn start(spec: &Value) -> Self {
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
    let run = Run::start(&spec(
        &dir,
        3,
        json!([
            { "id": "a", "argv": peer(&dir.state("a"), &["--exit-after-acks", "2"]), "messages": ["one", "two"] },
            { "id": "b", "argv": peer(&dir.state("b"), &["--exit-after-acks", "1"]), "messages": ["three"] },
        ]),
    ));
    let supervisor = run.supervisor_pid();
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
        // The peer's parent is the supervisor process, not this test.
        assert_eq!(launches[0]["ppid"], supervisor);
        assert_ne!(launches[0]["pid"], supervisor);
        assert_eq!(events(&seen, id, "exited")[0]["reaped"], "exact-child");
    }
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
    let run = Run::start(&spec(
        &dir,
        3,
        json!([
            { "id": "dedup", "argv": peer(&dir.state("dedup"), &["--mode", "exit-before-ack-once", "--exit-after-acks", "1"]), "messages": ["hello"] },
            { "id": "plain", "argv": peer(&dir.state("plain"), &["--mode", "exit-before-ack-once", "--no-dedup", "--exit-after-acks", "1"]), "messages": ["hello"] },
        ]),
    ));
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
    let mut run = Run::start(&spec(
        &dir,
        1,
        json!([
            { "id": "quiet", "argv": peer(&dir.state("quiet"), &["--mode", "silent"]), "messages": ["are you there"] },
            { "id": "busy", "argv": peer(&dir.state("busy"), &["--exit-after-acks", "1"]), "messages": ["hi"] },
        ]),
    ));
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
    let run = Run::start(&spec(
        &dir,
        3,
        json!([
            { "id": "flaky", "argv": peer(&dir.state("flaky"), &["--mode", "exit-before-ack-always"]), "messages": ["x", "y"] },
            { "id": "fine", "argv": peer(&dir.state("fine"), &["--exit-after-acks", "1"]), "messages": ["z"] },
        ]),
    ));
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
    let mut run = Run::start(&spec(
        &dir,
        1,
        json!([
            { "id": "deaf", "argv": peer(&dir.state("deaf"), &["--mode", "close-stdin"]), "messages": ["hello"] },
            { "id": "ok", "argv": peer(&dir.state("ok"), &["--exit-after-acks", "1"]), "messages": ["hi"] },
        ]),
    ));
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
    let mut run = Run::start(&spec(
        &dir,
        1,
        json!([
            { "id": "stays", "argv": peer(&dir.state("stays"), &[]), "messages": ["a"] },
            { "id": "goes", "argv": peer(&dir.state("goes"), &["--exit-after-acks", "1"]), "messages": ["b"] },
        ]),
    ));
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
    let run = Run::start(&spec(&dir, 0, json!([])));
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

/// Waits until a watched peer of a killed supervisor has exited (it dies by
/// its parent-death signal or stdin EOF). Test watchdog only.
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

/// (h) The real owner process is SIGKILLed while a delivery is unanswered
/// (inserted, no ACK). A new instance on the same store classifies that
/// attempt unknown, resumes the recorded session and resubmits the SAME
/// key. The dedup peer inserts once; the non-dedup peer inserts twice and
/// the ACK is duplicate-unknown. Neither restored ACK is at-most-once.
#[test]
fn owner_killed_mid_delivery_recovers_same_keys_on_restart() {
    let dir = Scratch::new("restart");
    let mut first = Run::start(&spec(
        &dir,
        3,
        json!([
            { "id": "dedup", "argv": peer(&dir.state("dedup"), &["--launch-modes", "insert-then-silent,normal", "--exit-after-acks", "1"]), "messages": ["hello"] },
            { "id": "plain", "argv": peer(&dir.state("plain"), &["--launch-modes", "insert-then-silent,normal", "--no-dedup", "--exit-after-acks", "1"]), "messages": ["hello"] },
        ]),
    ));
    first.until("intent-committed", |value| {
        value["event"] == "intent-committed"
    });
    let mut old_peers = Vec::new();
    for id in ["dedup", "plain"] {
        old_peers.push(watch(first.event(id, "launched")["pid"].as_u64().unwrap()));
        wait_state(&dir.state(id), |state| {
            state["insertions"].as_array().unwrap().len() == 1
        });
    }
    let seen = first.kill();
    assert!(seen.iter().all(|value| value["event"] != "terminal"));
    assert!(seen.iter().all(|value| value["event"] != "ack"));
    for peer in &old_peers {
        wait_gone(peer);
    }

    let second = Run::start(&recover(&dir));
    let (terminal, status, seen) = second.terminal();
    assert_eq!(terminal["status"], "ended", "{terminal}");
    assert_eq!(status.code(), Some(0));
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
        assert_eq!(state["launches"].as_array().unwrap().len(), 2, "{id}");
        assert_eq!(events(&seen, id, "session-resumed").len(), 1, "{id}");
        let message = &harness(&terminal, id)["messages"][0];
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
    let conn = db(&dir);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM attempt WHERE generation = 1 AND outcome = 'unknown-prior-owner'"
        ),
        2
    );
}

/// (i) Observed closures persist across an owner kill, so the cap is
/// reached across restarts and a further restart does not reset it.
#[test]
fn closures_and_outage_cap_persist_across_restart() {
    let dir = Scratch::new("cap");
    let modes = "exit-before-ack-always,insert-then-silent,exit-before-ack-always";
    let mut first = Run::start(&spec(
        &dir,
        2,
        json!([{ "id": "flaky", "argv": peer(&dir.state("flaky"), &["--launch-modes", modes]), "messages": ["x"] }]),
    ));
    first.event("flaky", "closure-observed");
    let peer = watch(
        first.until("second launch", |value| {
            value["event"] == "launched" && value["launch"] == 2
        })["pid"]
            .as_u64()
            .unwrap(),
    );
    wait_state(&dir.state("flaky"), |state| {
        state["prompts"].as_array().unwrap().len() == 2
    });
    first.kill();
    wait_gone(&peer);

    let (terminal, status, seen) = Run::start(&recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    let message = &harness(&terminal, "flaky")["messages"][0];
    assert_eq!(message["label"], "outage");
    assert_eq!(message["closures"], 2);
    assert_eq!(message["attempts"], 3);
    assert_eq!(message["prior_unknown"], 1);
    assert_eq!(events(&seen, "flaky", "outage")[0]["closures"], 2);
    assert_eq!(harness(&terminal, "flaky")["launches"], 1);

    let (terminal, status, seen) = Run::start(&recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    assert_eq!(harness(&terminal, "flaky")["launches"], 0);
    let message = &harness(&terminal, "flaky")["messages"][0];
    assert_eq!(message["label"], "outage");
    assert_eq!(
        message["closures"], 2,
        "recovered closures are the persisted ones"
    );
    assert_eq!(message["attempts"], 3);
    assert!(events(&seen, "flaky", "launched").is_empty());
    assert_eq!(
        read_state(&dir.state("flaky"))["launches"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

/// (j) While an owner is live, a second instance for the same root is
/// refused before it writes anything; a different root proceeds and leaves
/// the first root's store untouched.
#[test]
fn second_owner_is_refused_and_other_root_is_unaffected() {
    let dir = Scratch::new("dup");
    let other = Scratch::new("other");
    let mut owner = Run::start(&spec(
        &dir,
        1,
        json!([{ "id": "quiet", "argv": peer(&dir.state("quiet"), &["--mode", "silent"]), "messages": ["x"] }]),
    ));
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
        let mut duplicate = Run::start(&request);
        let first = duplicate.until("first line", |value| value["event"] != "intent-received");
        assert_eq!(first["event"], "terminal", "duplicate proceeded: {first}");
        let (terminal, status, seen) = duplicate.terminal();
        assert_eq!(terminal["status"], "store-refused");
        assert_eq!(terminal["reason"], "owner-live");
        assert_eq!(status.code(), Some(65));
        assert!(seen.iter().all(|value| value["event"] != "launched"));
    }

    let (terminal, status, _) = Run::start(&spec(
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

    let mut run = Run::start(&spec(
        &dir,
        1,
        json!([{ "id": "h", "argv": peer(&dir.state("h"), &[]), "messages": ["x"] }]),
    ));
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

    let (terminal, status, _) = Run::start(&recover(&dir)).terminal();
    assert_eq!(terminal["status"], "store-refused");
    assert_eq!(terminal["reason"], "no-durable-intent");
    assert_eq!(status.code(), Some(65));
    assert!(!dir.state("h").exists(), "nothing was launched");
}

/// (l) An owner-restart loop with unresolved outcomes cannot buy unlimited
/// attempts: attempts persist across owner kills and count toward the
/// intent's delivery-attempt budget, separately from observed closures.
/// Once used up, a recovery stops the message as `attempts-exhausted`
/// before launching anything, with the unknown attempts still unknown and
/// no closure, outage or acknowledgement invented.
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
    for (round, request) in [request, recover(&dir)].into_iter().enumerate() {
        let mut owner = Run::start(&request);
        let peer = watch(owner.event("quiet", "launched")["pid"].as_u64().unwrap());
        wait_state(&state, |state| {
            state["prompts"].as_array().unwrap().len() == round + 1
        });
        owner.kill();
        wait_gone(&peer);
    }

    for _ in 0..2 {
        let mut run = Run::start(&recover(&dir));
        let first = run.until("first quiet event", |value| value["harness"] == "quiet");
        assert_eq!(first["event"], "nothing-deliverable", "{first}");
        let (terminal, status, seen) = run.terminal();
        assert_eq!(terminal["status"], "ended-owed", "{terminal}");
        assert_eq!(status.code(), Some(3));
        assert!(events(&seen, "quiet", "launched").is_empty());
        let record = harness(&terminal, "quiet");
        assert_eq!(record["launches"], 0);
        let message = &record["messages"][0];
        assert_eq!(message["state"], "owed");
        assert_eq!(message["label"], "attempts-exhausted");
        assert_eq!(message["attempts"], 2);
        assert_eq!(message["closures"], 0, "owner death is not a closure");
    }
    assert_eq!(read_state(&state)["launches"].as_array().unwrap().len(), 2);
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
/// observed-closure attempts use a budget of three, so the second closure
/// authorizes no further relaunch although the closure cap is not reached.
#[test]
fn attempt_budget_counts_earlier_generations_before_sending() {
    let dir = Scratch::new("budget-run");
    let state = dir.state("flaky");
    let mut first = Run::start(&json!({
        "store": dir.store(),
        "intent": {
            "outage_closure_cap": 5,
            "delivery_attempt_cap": 3,
            "cwd": "/",
            "harnesses": [{ "id": "flaky", "argv": peer(&state, &["--launch-modes", "insert-then-silent,exit-before-ack-always"]), "messages": ["x"] }],
        },
    }));
    let peer = watch(first.event("flaky", "launched")["pid"].as_u64().unwrap());
    wait_state(&state, |state| {
        !state["prompts"].as_array().unwrap().is_empty()
    });
    first.kill();
    wait_gone(&peer);

    let (terminal, status, seen) = Run::start(&recover(&dir)).terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
    let message = &harness(&terminal, "flaky")["messages"][0];
    assert_eq!(message["label"], "attempts-exhausted");
    assert_eq!(message["attempts"], 3);
    assert_eq!(message["prior_unknown"], 1);
    assert_eq!(message["closures"], 2);
    assert!(events(&seen, "flaky", "outage").is_empty());
    assert_eq!(events(&seen, "flaky", "attempts-exhausted").len(), 1);
    assert_eq!(harness(&terminal, "flaky")["launches"], 2);
    assert_eq!(read_state(&state)["prompts"].as_array().unwrap().len(), 3);
}
