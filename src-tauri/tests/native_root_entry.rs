//! The Runner's `native-root` entry starting fresh native OpenCode ACP v2
//! roots (Linux). Ignored by default: it needs `oulipoly-root-supervisor`
//! and `oulipoly-root-pid1` built next to the Runner binary, the native
//! dependencies (`OULIPOLY_NATIVE_DEPS`, see the root supervisor's
//! `native` module), the matching agent-bash tool and binary
//! (`OULIPOLY_AGENT_BASH_TOOL`, `AGENT_BASH_BIN`), a short scratch
//! directory (`OULIPOLY_NATIVE_SCRATCH`, for socket paths) and loopback.
//! Turns are answered by a scripted loopback stand-in for a model: no
//! model, provider, credential or network.
//!
//! The Runner itself runs with ambient variables the request does not
//! declare, among them an inline config allowing everything; the request
//! declares the root's whole environment, including a config the native
//! launch removes. What reaches the owner, the native host and an in-root
//! Bash command is read where each actually is: `/proc/<pid>/environ` and
//! the command's own output in the native conversation.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const RUNNER: &str = env!("CARGO_BIN_EXE_oulipoly-agent-runner");
const WATCHDOG: Duration = Duration::from_secs(150);
const DECLARED: &str = "oulipoly-declared-marker";
const AMBIENT: &str = "oulipoly-ambient-marker";
const ALLOWED: &str =
    r#"printf %s:%s:%s "$OULIPOLY_WITNESS_DECLARED" "${OULIPOLY_WITNESS_AMBIENT-unset}" "$HOME""#;
const UNNAMED: &str =
    r#"printf %s:%s:%s "$OULIPOLY_WITNESS_DECLARED" "${OULIPOLY_WITNESS_AMBIENT-unset}" "$HOME" x"#;
const ALLOW_ALL: &str = r#"{"permission":{"*":"allow","bash":"allow"}}"#;
const RECOVER: &str = "oulipoly-recover-marker";

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

/// A loopback stand-in for a model: to `RUN <command>` it calls `bash`
/// once; after a tool result it answers `DONE`; otherwise `NO-SCRIPT`.
fn scripted_model() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            std::thread::spawn(move || serve(stream));
        }
    });
    base_url
}

fn serve(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut start = String::new();
    reader.read_line(&mut start).unwrap();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    if !start.contains("/chat/completions") {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
        return;
    }
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let last = body["messages"]
        .as_array()
        .and_then(|messages| messages.last().cloned())
        .unwrap_or(Value::Null);
    let text = match &last["content"] {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect(),
        _ => String::new(),
    };
    let chunk = |delta: Value, finish: Value| {
        json!({ "id": "scripted", "object": "chat.completion.chunk", "created": 0,
                "model": "scripted",
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }] })
    };
    let chunks = match text.strip_prefix("RUN ") {
        Some(command) if last["role"] == "user" => vec![
            chunk(
                json!({ "role": "assistant", "tool_calls": [{ "index": 0,
                    "id": "call_scripted_1", "type": "function",
                    "function": { "name": "bash",
                        "arguments": json!({ "command": command }).to_string() } }] }),
                Value::Null,
            ),
            chunk(json!({}), json!("tool_calls")),
        ],
        _ => {
            let answer = if last["role"] == "tool" {
                "DONE"
            } else {
                "NO-SCRIPT"
            };
            vec![
                chunk(
                    json!({ "role": "assistant", "content": answer }),
                    Value::Null,
                ),
                chunk(json!({}), json!("stop")),
            ]
        }
    };
    let mut out = String::from(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n",
    );
    for chunk in chunks {
        out.push_str(&format!("data: {chunk}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

struct Root {
    dir: PathBuf,
    request: Value,
}

impl Root {
    fn new(scratch: &Path, name: &str, base_url: &str, message: &str) -> Self {
        let dir = scratch.join(name);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .unwrap_or_else(|error| panic!("{dir:?}: {error} (use a fresh scratch)"));
        for sub in ["owner-home", "project"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(dir.join(sub))
                .unwrap();
        }
        let request = json!({
            "store": dir.join("s"),
            "launch_dir": dir.join("launch"),
            "cwd": dir.join("project"),
            "env": {
                "PATH": "/usr/bin:/bin",
                "HOME": dir.join("owner-home"),
                "OULIPOLY_WITNESS_DECLARED": DECLARED,
                // Declared, but the native launch removes it from its host.
                "OPENCODE_CONFIG_CONTENT": ALLOW_ALL,
            },
            "messages": [message],
            "outage_closure_cap": 1,
            "delivery_attempt_cap": 1,
            "opencode": {
                "deps": var("OULIPOLY_NATIVE_DEPS"),
                "agent_bash_tool": var("OULIPOLY_AGENT_BASH_TOOL"),
                "agent_bash_bin": var("AGENT_BASH_BIN"),
                "bash_allow": [ALLOWED],
                "model": "fixture/scripted",
                "provider": { "fixture": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "fixture",
                    // Not a credential: the stand-in reads no header.
                    "options": { "baseURL": base_url, "apiKey": "fixture-not-a-credential" },
                    "models": { "scripted": { "name": "scripted", "tool_call": true } },
                }},
            },
        });
        Self { dir, request }
    }

    fn start(&self) -> Run {
        let path = self.dir.join("request.json");
        std::fs::write(&path, self.request.to_string()).unwrap();
        self.entry("--request", &path)
    }

    /// The recovering owner's whole declared environment.
    fn recover_env(&self) -> Value {
        json!({
            "PATH": "/usr/bin:/bin",
            "HOME": self.dir.join("recover-home"),
            "OULIPOLY_WITNESS_RECOVER": RECOVER,
        })
    }

    fn recover(&self, purpose: &str) -> Run {
        let path = self.dir.join(format!("recover-{purpose}.json"));
        let request = json!({
            "store": self.request["store"],
            "purpose": purpose,
            "env": self.recover_env(),
        });
        std::fs::write(&path, request.to_string()).unwrap();
        self.entry("--recover", &path)
    }

    fn entry(&self, form: &str, path: &Path) -> Run {
        // The Runner's own environment: ambient, never declared.
        let mut child = Command::new(RUNNER)
            .args(["native-root", form])
            .arg(path)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.dir.join("owner-home"))
            .env("OULIPOLY_WITNESS_AMBIENT", AMBIENT)
            .env("OPENCODE_PERMISSION", ALLOW_ALL)
            .env("OPENCODE_CONFIG_CONTENT", ALLOW_ALL)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                println!("entry: {line}");
                let value = serde_json::from_str(&line).expect("json line");
                if tx.send(value).is_err() {
                    return;
                }
            }
        });
        Run {
            child,
            stdin: Some(stdin),
            lines,
            seen: Vec::new(),
        }
    }
}

struct Run {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Value>,
    seen: Vec<Value>,
}

impl Run {
    fn until(&mut self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        if let Some(value) = self.seen.iter().find(|value| pred(value)) {
            return value.clone();
        }
        loop {
            let Ok(value) = self.lines.recv_timeout(WATCHDOG) else {
                panic!("watchdog: no {what}\nseen: {:#?}", self.seen);
            };
            self.seen.push(value.clone());
            if pred(&value) {
                return value;
            }
            assert_ne!(value["entry"], "terminal", "entry terminal before {what}");
        }
    }

    fn event(&mut self, event: &str) -> Value {
        self.until(event, |value| value["event"] == event)
    }

    fn entry(&mut self, stage: &str) -> Value {
        self.until(stage, |value| value["entry"] == stage)
    }

    /// Cancels through the entry's stdin; returns the owner's and the
    /// entry's terminal lines and the entry's exit code.
    fn cancel(mut self) -> (Value, Value, Option<i32>) {
        let mut stdin = self.stdin.take().unwrap();
        writeln!(stdin, "{}", json!({ "cmd": "cancel" })).unwrap();
        drop(stdin);
        let owner = self.event("terminal");
        let entry = self.entry("terminal");
        let status = self.child.wait().unwrap();
        (owner, entry, status.code())
    }
}

impl Run {
    /// SIGKILLs the entry itself and returns what it relayed.
    fn kill_entry(mut self) -> Vec<Value> {
        self.child.kill().unwrap();
        let status = self.child.wait().unwrap();
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(9)
        );
        while let Ok(value) = self.lines.recv_timeout(Duration::from_secs(5)) {
            self.seen.push(value);
        }
        std::mem::take(&mut self.seen)
    }

    /// The owner's and the entry's terminal lines and the entry's exit
    /// code, without sending anything.
    fn finish(mut self) -> (Value, Value, Option<i32>, Vec<Value>) {
        let owner = self.event("terminal");
        let entry = self.entry("terminal");
        let status = self.child.wait().unwrap();
        (owner, entry, status.code(), std::mem::take(&mut self.seen))
    }

    fn saw(&self, event: &str) -> bool {
        self.seen.iter().any(|value| value["event"] == event)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            if let Some(mut stdin) = self.stdin.take() {
                let _ = writeln!(stdin, "{}", json!({ "cmd": "cancel" }));
            }
            let _ = self.child.wait();
        }
    }
}

fn environ(pid: u64) -> BTreeMap<String, String> {
    let bytes = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
    bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('=').unwrap_or((&entry, ""));
            (key.to_owned(), value.to_owned())
        })
        .collect()
}

/// Every tool part of a native conversation, read by OpenCode itself from
/// the launch's own data directory: `(status, input command, output)`.
fn tool_parts(launch: &Value, cwd: &str, session: &str) -> Vec<(String, String, String)> {
    let opencode = Path::new(&var("OULIPOLY_NATIVE_DEPS"))
        .join("node_modules/opencode-linux-x64/bin/opencode");
    let output = Command::new(opencode)
        .args(["export", "--pure", session])
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(
            launch["env"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key, value.as_str().unwrap())),
        )
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "export: {output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let exported: Value = serde_json::from_str(&text[text.find('{').unwrap()..]).unwrap();
    exported["messages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|message| message["parts"].as_array().unwrap().iter())
        .filter(|part| part["type"] == "tool")
        .map(|part| {
            let state = &part["state"];
            (
                state["status"].as_str().unwrap().to_owned(),
                state["input"]["command"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                state["output"]
                    .as_str()
                    .or_else(|| state["error"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

/// Runs one root to the native denial or Bash result, checking the
/// environment at the owner and the native host, then cancels it.
fn run_root(root: &Root, allowed: bool) -> (Value, String) {
    let mut run = root.start();
    let setup = run.entry("setup-completed");
    let reach = &setup["env"];
    assert_eq!(reach["ambient"], "none", "{setup}");
    assert_eq!(
        reach["native_host"]["removed_by_launch"],
        json!(["OPENCODE_CONFIG_CONTENT"]),
        "{setup}"
    );
    assert_eq!(
        reach["native_host"]["overridden_by_launch"],
        json!(["HOME"])
    );
    let text = setup.to_string();
    assert!(
        !text.contains(DECLARED) && !text.contains(AMBIENT),
        "values echoed: {text}"
    );

    // The owner received exactly the declared environment.
    let owner_pid = run.entry("owner-started")["pid"].as_u64().unwrap();
    let declared: BTreeMap<String, String> =
        serde_json::from_value(root.request["env"].clone()).unwrap();
    assert_eq!(environ(owner_pid), declared, "owner environment");

    // The native host: declared, less what its launch removes or sets.
    let host_pid = run.event("launched")["pid"].as_u64().unwrap();
    run.event("endpoint-connected");
    let host = environ(host_pid);
    let launch_home = format!("{}/home", root.request["launch_dir"].as_str().unwrap());
    assert_eq!(
        host.get("OULIPOLY_WITNESS_DECLARED").map(String::as_str),
        Some(DECLARED)
    );
    assert_eq!(host.get("HOME"), Some(&launch_home), "host HOME");
    for absent in [
        "OULIPOLY_WITNESS_AMBIENT",
        "OPENCODE_CONFIG_CONTENT",
        "OPENCODE_PERMISSION",
    ] {
        assert!(!host.contains_key(absent), "host has {absent}");
    }
    assert!(host.contains_key("OULIPOLY_ROOT_BASH_V1"));

    let session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let ack = run.event("ack");
    assert_eq!(ack["label"], "accepted", "{ack}");
    if allowed {
        let accepted = run.event("bash-accepted");
        assert_eq!(
            accepted["argv"],
            json!(["bash", "-lc", ALLOWED]),
            "{accepted}"
        );
        run.event("bash-ended");
    }
    let idle = run.event("idle");
    assert_eq!(idle["stop_reason"], "end_turn", "{idle}");
    assert_eq!(run.event("agent-message")["text"], "DONE");
    assert!(
        !run.seen
            .iter()
            .any(|value| value["event"] == "request-refused"),
        "{:#?}",
        run.seen
    );
    if !allowed {
        assert!(
            !run.seen
                .iter()
                .any(|value| value["event"] == "bash-accepted"),
            "{:#?}",
            run.seen
        );
    }
    // Only now: an ack, an idle and the native result are not an end.
    let (owner, entry, code) = run.cancel();
    assert_eq!(owner["status"], "cancelled", "{owner}");
    assert_eq!(owner["all_harnesses_reaped"], true, "{owner}");
    assert_eq!(owner["root_pid1"]["end_observed"], true, "{owner}");
    assert_eq!(owner["bash"]["accepted"], u64::from(allowed), "{owner}");
    assert_eq!(entry["stage"], "owner-ended", "{entry}");
    assert_eq!(entry["owner_exit"], 2, "{entry}");
    assert_eq!(code, Some(82), "{entry}");
    (setup["launch"].clone(), session)
}

#[test]
#[ignore = "needs the root supervisor built beside the Runner, OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN, OULIPOLY_NATIVE_SCRATCH and loopback"]
fn native_root_entry_starts_fresh_roots_with_only_the_declared_environment() {
    let beside = Path::new(RUNNER).parent().unwrap();
    for binary in ["oulipoly-root-supervisor", "oulipoly-root-pid1"] {
        assert!(
            beside.join(binary).is_file(),
            "build {binary} beside the Runner"
        );
    }
    let scratch = PathBuf::from(var("OULIPOLY_NATIVE_SCRATCH"));
    let base_url = scripted_model();

    // allowed: the command runs through the root's Bash ingress, in the
    // declared environment (declared HOME, no ambient variable).
    let allow = Root::new(&scratch, "a", &base_url, &format!("RUN {ALLOWED}"));
    let (launch, session) = run_root(&allow, true);
    let parts = tool_parts(&launch, allow.request["cwd"].as_str().unwrap(), &session);
    println!("allowed parts: {parts:?}");
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert_eq!(parts[0].0, "completed", "{parts:?}");
    let expected = format!(
        "{DECLARED}:unset:{}",
        allow.request["env"]["HOME"].as_str().unwrap()
    );
    assert!(
        parts[0]
            .2
            .starts_with("Root v1 work ended: exited with code 0")
            && parts[0].2.ends_with(&expected),
        "{parts:?}"
    );

    // not named: a native denial, nothing reaches the owner.
    let deny = Root::new(&scratch, "d", &base_url, &format!("RUN {UNNAMED}"));
    let (launch, session) = run_root(&deny, false);
    let parts = tool_parts(&launch, deny.request["cwd"].as_str().unwrap(), &session);
    println!("unnamed parts: {parts:?}");
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert_eq!(parts[0].0, "error", "{parts:?}");
    assert_eq!(parts[0].1, UNNAMED);

    // A root is started fresh only: an existing store is refused before
    // any effect, so no launch directory appears.
    let again = Root::new(&scratch, "r", &base_url, "RUN true");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(again.request["store"].as_str().unwrap())
        .unwrap();
    let mut run = again.start();
    let entry = run.entry("terminal");
    let code = run.child.wait().unwrap().code();
    assert_eq!(entry["stage"], "refused", "{entry}");
    assert_eq!(entry["effects"], "none", "{entry}");
    assert_eq!(code, Some(64), "{entry}");
    assert!(!Path::new(again.request["launch_dir"].as_str().unwrap()).exists());
}

/// Whether `pid` has ended (gone, or a zombie nobody reaped).
fn ended(pid: u64) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat[stat.rfind(')').unwrap() + 2..].starts_with('Z'),
    }
}

fn wait_ended(pid: u64, what: &str) {
    let deadline = std::time::Instant::now() + WATCHDOG;
    while !ended(pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "watchdog: {what} {pid} still running"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn event_in<'a>(seen: &'a [Value], event: &str) -> Option<&'a Value> {
    seen.iter().find(|value| value["event"] == event)
}

/// Every live (not zombie) process whose command line names `path`.
fn live_under(path: &Path) -> Vec<u64> {
    let needle = path.display().to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u64>().ok())
        .filter(|pid| u64::from(std::process::id()) != *pid)
        .filter(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline"))
                .is_ok_and(|cmdline| String::from_utf8_lossy(&cmdline).contains(&needle))
                && !ended(*pid)
        })
        .collect()
}

/// A recovery: the entry names the recovering owner's declared names, the
/// owner gets exactly that environment, and the store's root is attached.
fn recovering(run: &mut Run, root: &Root, purpose: &str) -> u64 {
    let line = run.entry("recovery");
    assert_eq!(line["purpose"], purpose, "{line}");
    assert_eq!(line["env"]["ambient"], "none", "{line}");
    assert_eq!(line["env"]["attached_work"], "original-root-environment");
    assert!(!line.to_string().contains(RECOVER), "values echoed: {line}");
    let owner = run.entry("owner-started")["pid"].as_u64().unwrap();
    let declared: BTreeMap<String, String> = serde_json::from_value(root.recover_env()).unwrap();
    assert_eq!(environ(owner), declared, "recovering owner environment");
    owner
}

/// The entry dies during a held native run; its owner dies with it, while
/// root PID 1 and the native host survive. A public `continue-attached`
/// recovery attaches that same root PID 1 and delivers what was owed to the
/// surviving host, whose work keeps the original environment. Its entry
/// dies too; a public `cancel` recovery then ends the root's work without
/// connecting to it, and a later recovery finds no root and starts none.
/// A second root, its entry killed before delivery, is cancelled with its
/// message still owed: nothing is delivered under cancel.
#[test]
#[ignore = "needs the root supervisor built beside the Runner, OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN, OULIPOLY_NATIVE_SCRATCH and loopback"]
fn native_root_survives_entry_death_and_is_continued_or_cancelled_by_attachment() {
    let scratch = PathBuf::from(var("OULIPOLY_NATIVE_SCRATCH"));
    let base_url = scripted_model();

    // A: entry killed once the native host is launched, before delivery.
    let a = Root::new(&scratch, "ka", &base_url, &format!("RUN {ALLOWED}"));
    let mut run = a.start();
    let launch = run.entry("setup-completed")["launch"].clone();
    let owner1 = run.entry("owner-started")["pid"].as_u64().unwrap();
    let root_line = run.event("root-pid1-started");
    assert_eq!(root_line["parent"], "this-owner", "{root_line}");
    let root_pid1 = root_line["pid"].as_u64().unwrap();
    let host = run.event("launched")["pid"].as_u64().unwrap();
    let seen = run.kill_entry();
    assert!(
        event_in(&seen, "ack").is_none(),
        "delivered before the kill: {seen:#?}"
    );
    assert!(event_in(&seen, "terminal").is_none());
    wait_ended(owner1, "owner after its entry's death");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!ended(root_pid1), "root PID 1 survives its owner");
    assert!(!ended(host), "the native host survives its owner");

    // Continue by attachment: same root PID 1, same host, original env.
    let mut cont = a.recover("continue-attached");
    let owner2 = recovering(&mut cont, &a, "continue-attached");
    let custody = cont.event("custody");
    assert_eq!(custody["outcome"], "attached", "{custody}");
    let reattached = cont.event("reattached");
    assert_eq!(reattached["pid"].as_u64(), Some(host), "{reattached}");
    cont.event("endpoint-connected");
    let ack = cont.event("ack");
    println!("continued ack label: {}", ack["label"]);
    let accepted = cont.event("bash-accepted");
    assert_eq!(
        accepted["argv"],
        json!(["bash", "-lc", ALLOWED]),
        "{accepted}"
    );
    cont.event("bash-ended");
    assert_eq!(cont.event("agent-message")["text"], "DONE");
    let session = cont
        .seen
        .iter()
        .find_map(|value| {
            (value["event"] == "session-opened" || value["event"] == "session-resumed")
                .then(|| value["session"].as_str().map(str::to_owned))
                .flatten()
        })
        .expect("session");
    assert!(
        !cont.saw("launched") && !cont.saw("root-pid1-started"),
        "{:#?}",
        cont.seen
    );
    let host_env = environ(host);
    assert_eq!(
        host_env
            .get("OULIPOLY_WITNESS_DECLARED")
            .map(String::as_str),
        Some(DECLARED)
    );
    assert!(!host_env.contains_key("OULIPOLY_WITNESS_RECOVER"));
    // The held run's entry dies again: owner gone, work kept.
    let seen = cont.kill_entry();
    assert!(event_in(&seen, "terminal").is_none());
    wait_ended(owner2, "continuing owner after its entry's death");
    assert!(!ended(host) && !ended(root_pid1));

    // Cancel by attachment: nothing connected, delivered or launched.
    let mut cancel = a.recover("cancel");
    recovering(&mut cancel, &a, "cancel");
    assert_eq!(cancel.event("custody")["outcome"], "attached");
    assert_eq!(cancel.event("cancel-requested")["by"], "recover-cancel");
    let (owner, entry, code, seen) = cancel.finish();
    for nothing in [
        "endpoint-connected",
        "session-resumed",
        "session-opened",
        "ack",
        "launched",
        "root-pid1-started",
    ] {
        assert!(
            event_in(&seen, nothing).is_none(),
            "{nothing} under cancel: {seen:#?}"
        );
    }
    assert_eq!(owner["status"], "cancelled", "{owner}");
    assert_eq!(owner["recover"], "cancel");
    assert_eq!(owner["all_harnesses_reaped"], true, "{owner}");
    assert_eq!(
        owner["harnesses"][0]["exits"],
        json!(["signal:9"]),
        "{owner}"
    );
    assert_eq!(
        owner["harnesses"][0]["messages"][0]["state"],
        "acknowledged"
    );
    assert_eq!(
        owner["root_pid1"]["outcome"], "released-exit-observed-by-pidfd",
        "{owner}"
    );
    assert_eq!(owner["root_pid1"]["parent"], false);
    assert!(owner["root_pid1"]["status"].is_null());
    assert_eq!(owner["root_pid1"]["end_observed"], true);
    assert_eq!(entry["stage"], "owner-ended", "{entry}");
    assert_eq!(entry["owner_exit"], 2);
    assert_eq!(entry["relay"], "complete");
    assert_eq!(entry["retry"], "explicit-recovery-only");
    assert_eq!(code, Some(82), "{entry}");
    assert!(ended(host) && ended(root_pid1));
    let parts = tool_parts(&launch, a.request["cwd"].as_str().unwrap(), &session);
    println!("continued parts: {parts:?}");
    let expected = format!(
        "{DECLARED}:unset:{}",
        a.request["env"]["HOME"].as_str().unwrap()
    );
    assert!(
        parts.len() == 1 && parts[0].0 == "completed" && parts[0].2.ends_with(&expected),
        "attached work ran in the original environment: {parts:?}"
    );

    // Nothing left to attach: no new incarnation.
    let mut again = a.recover("continue-attached");
    recovering(&mut again, &a, "continue-attached");
    let (owner, entry, code, seen) = again.finish();
    assert_eq!(owner["status"], "root-absent", "{owner}");
    assert_eq!(owner["new_incarnation"], "not-started");
    assert_eq!(owner["owed"], 0, "{owner}");
    let limited = event_in(&seen, "recover-limited").expect("recover-limited");
    assert_eq!(limited["custody"], "no-unended-incarnation", "{limited}");
    assert!(
        event_in(&seen, "launched").is_none() && event_in(&seen, "root-pid1-started").is_none()
    );
    assert_eq!(entry["owner_exit"], 6, "{entry}");
    assert_eq!(code, Some(86), "{entry}");

    // B: cancelled with its message owed; nothing delivered under cancel.
    let b = Root::new(&scratch, "kb", &base_url, "RUN true");
    let mut run = b.start();
    let owner1 = run.entry("owner-started")["pid"].as_u64().unwrap();
    let host = run.event("launched")["pid"].as_u64().unwrap();
    let seen = run.kill_entry();
    assert!(event_in(&seen, "ack").is_none(), "{seen:#?}");
    wait_ended(owner1, "owner after its entry's death");
    assert!(!ended(host));
    let mut cancel = b.recover("cancel");
    recovering(&mut cancel, &b, "cancel");
    let (owner, entry, code, seen) = cancel.finish();
    for nothing in [
        "endpoint-connected",
        "session-opened",
        "ack",
        "launched",
        "root-pid1-started",
    ] {
        assert!(
            event_in(&seen, nothing).is_none(),
            "{nothing} under cancel: {seen:#?}"
        );
    }
    assert_eq!(owner["status"], "cancelled", "{owner}");
    let message = &owner["harnesses"][0]["messages"][0];
    assert_eq!(message["state"], "owed", "{owner}");
    assert_eq!(message["label"], "cancelled", "{owner}");
    assert_eq!(code, Some(82), "{entry}");
    assert!(ended(host));
    let mut again = b.recover("continue-attached");
    recovering(&mut again, &b, "continue-attached");
    let (owner, _, code, _) = again.finish();
    assert_eq!(owner["status"], "root-absent", "{owner}");
    assert_eq!(owner["owed"], 1, "debt still reported: {owner}");
    assert_eq!(code, Some(86));

    // A missing store is refused before any effect.
    let missing = Root::new(&scratch, "km", &base_url, "RUN true");
    let mut run = missing.recover("cancel");
    let entry = run.entry("terminal");
    assert_eq!(entry["stage"], "refused", "{entry}");
    assert_eq!(run.child.wait().unwrap().code(), Some(64));
    assert!(!Path::new(missing.request["store"].as_str().unwrap()).exists());

    let left = live_under(&scratch);
    assert!(
        left.is_empty(),
        "processes left under the scratch: {left:?}"
    );
}
