//! Process-level controls: the real supervisor binary owning real
//! deterministic-peer subprocesses. Deterministic peers prove this crate's
//! contract, not real-harness behaviour.
//!
//! The only timer here is a test watchdog: if an expected line never
//! arrives, the test kills its own supervisor (its peers die with it) and
//! fails, rather than hanging. The supervisor itself has no timer.

use std::io::{BufRead, BufReader, Write};
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
        let mut child = Command::new(SUPERVISOR)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
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
        (terminal, status, self.seen)
    }

    fn supervisor_pid(&self) -> u32 {
        self.child.id()
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
    // A zombie still answers kill(0); read its state instead.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| {
            stat.rsplit_once(')')
                .map(|(_, rest)| rest.trim_start().chars().next())
        })
        .flatten()
        .is_some_and(|state| state != 'Z' && state != 'X')
}

fn events<'a>(seen: &'a [Value], harness: &str, event: &str) -> Vec<&'a Value> {
    seen.iter()
        .filter(|value| value["harness"] == harness && value["event"] == event)
        .collect()
}

fn spec(cap: u32, harnesses: Value) -> Value {
    json!({ "outage_closure_cap": cap, "cwd": "/", "harnesses": harnesses })
}

/// (a) A separate supervisor process concurrently owns two peer processes,
/// reaps both exactly, and ends normally only when nothing is owed.
#[test]
fn separate_process_owns_two_harnesses_and_ends_after_exact_reaping() {
    let dir = Scratch::new("own");
    let run = Run::start(&spec(
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
/// reports it retained-undelivered and observes its termination exit.
#[test]
fn silent_harness_stays_owed_while_other_delivers_until_cancel() {
    let dir = Scratch::new("silent");
    let mut run = Run::start(&spec(
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
    assert_eq!(quiet["messages"][0]["state"], "retained-undelivered");
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
        3,
        json!([
            { "id": "flaky", "argv": peer(&dir.state("flaky"), &["--mode", "exit-before-ack-always"]), "messages": ["x", "y"] },
            { "id": "fine", "argv": peer(&dir.state("fine"), &["--exit-after-acks", "1"]), "messages": ["z"] },
        ]),
    ));
    let (terminal, status, seen) = run.terminal();
    assert_eq!(terminal["status"], "ended-owed", "{terminal}");
    assert_eq!(status.code(), Some(3));
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
    assert_eq!(flaky["messages"][0]["state"], "retained-undelivered");
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
    let stays = harness(&terminal, "stays");
    assert_eq!(stays["messages"][0]["state"], "acknowledged");
    assert_eq!(stays["exits"], json!(["signal:9"]));
    assert!(!alive(stays_pid));
}

#[test]
fn invalid_spec_launches_nothing() {
    let run = Run::start(&json!({ "outage_closure_cap": 0, "cwd": "/", "harnesses": [] }));
    let (terminal, status, _) = run.terminal();
    assert_eq!(terminal["status"], "spec-refused");
    assert_eq!(status.code(), Some(64));
}
