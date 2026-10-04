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
use std::sync::Mutex;
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

/// Every `authorization` header value the scripted stand-in received.
static AUTHORIZATIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"))
}

/// A loopback stand-in for a model: to `RUN <command>` it calls `bash`
/// once; after a tool result it answers `DONE`; to `SAY <text>` it answers
/// `<text>`; otherwise `NO-SCRIPT`.
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
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse().unwrap();
            } else if name.eq_ignore_ascii_case("authorization") {
                AUTHORIZATIONS.lock().unwrap().push(value.trim().to_owned());
            }
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
                text.strip_prefix("SAY ").unwrap_or("NO-SCRIPT")
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
        for sub in ["owner-home", "project", "tmp"] {
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
                // Temporary files of the root's processes (OpenCode's
                // runtime extracts libraries) stay in the root's directory.
                "TMPDIR": dir.join("tmp"),
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
            // Run in an outer user namespace fixture: unprivileged.
            "workload": { "isolation": "unprivileged-userns" },
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

    /// Writes control lines to the entry's stdin in one write.
    fn control(&mut self, lines: &[Value]) {
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(text.as_bytes()).unwrap();
        stdin.flush().unwrap();
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
        .env("TMPDIR", Path::new(cwd).parent().unwrap().join("tmp"))
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

/// Runs one root to the native denial or the Bash result of `runs`,
/// checking the environment at the owner and the native host and the
/// reported policy against the config file, then cancels it.
fn run_root(root: &Root, runs: Option<&str>) -> (Value, String) {
    let allowed = runs.is_some();
    let mut run = root.start();
    let setup = run.entry("setup-completed");
    let policy = &setup["launch"]["policy"];
    let config = std::fs::read_to_string(
        Path::new(root.request["launch_dir"].as_str().unwrap())
            .join("xdg/config/opencode/opencode.json"),
    )
    .unwrap();
    assert!(
        config.ends_with(&format!(
            r#""permission":{}}}"#,
            policy["native"].as_str().unwrap()
        )),
        "{policy} {config}"
    );
    match root.request["opencode"].get("bash_authority") {
        Some(authority) => assert_eq!(policy["bash"], *authority, "{policy}"),
        None => assert_eq!(
            policy["bash"]["allow"], root.request["opencode"]["bash_allow"],
            "{policy}"
        ),
    }
    assert_eq!(policy["other"], "deny", "{policy}");
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
    if let Some(command) = runs {
        let accepted = run.event("bash-accepted");
        assert_eq!(
            accepted["argv"],
            json!(["bash", "-lc", command]),
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
    let (launch, session) = run_root(&allow, Some(ALLOWED));
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
    let (launch, session) = run_root(&deny, None);
    let parts = tool_parts(&launch, deny.request["cwd"].as_str().unwrap(), &session);
    println!("unnamed parts: {parts:?}");
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert_eq!(parts[0].0, "error", "{parts:?}");
    assert_eq!(parts[0].1, UNNAMED);

    // trusted task: the caller's explicit open `bash` authority runs the
    // same command no list names, through the root's Bash ingress.
    let mut open = Root::new(&scratch, "t", &base_url, &format!("RUN {UNNAMED}"));
    let opencode = open.request["opencode"].as_object_mut().unwrap();
    opencode.remove("bash_allow");
    opencode.insert("bash_authority".to_owned(), json!("trusted-task"));
    let (launch, session) = run_root(&open, Some(UNNAMED));
    assert_eq!(
        launch["policy"]["native"],
        r#"{"*":"deny","bash":{"*":"allow"}}"#
    );
    let parts = tool_parts(&launch, open.request["cwd"].as_str().unwrap(), &session);
    println!("trusted parts: {parts:?}");
    assert_eq!(parts.len(), 1, "{parts:?}");
    assert_eq!(parts[0].0, "completed", "{parts:?}");
    assert!(
        parts[0].2.ends_with(&format!(
            // `printf` reuses its format for the extra `x`.
            "{DECLARED}:unset:{}x::",
            open.request["env"]["HOME"].as_str().unwrap()
        )),
        "{parts:?}"
    );

    // Both forms at once, or an unknown authority, are refused before any
    // effect: nothing is widened by guessing.
    for (name, authority) in [("tb", json!("trusted-task")), ("tx", json!("all"))] {
        let mut both = Root::new(&scratch, name, &base_url, "RUN true");
        both.request["opencode"]["bash_authority"] = authority;
        let mut run = both.start();
        let entry = run.entry("terminal");
        let code = run.child.wait().unwrap().code();
        assert_eq!(entry["stage"], "refused", "{entry}");
        assert_eq!(entry["effects"], "none", "{entry}");
        assert_eq!(code, Some(64), "{entry}");
        assert!(!Path::new(both.request["launch_dir"].as_str().unwrap()).exists());
    }

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

/// A further communication to the same live native session, then an
/// explicit close. Each input has its own correlated insertion ACK, scripted
/// reply (by native parent) and idle-tag turn end in one native session of
/// one host launch. The close stops the host through its work PID 1 once
/// both turns ended: owner `closed` (7), entry 87, the host's end a signal,
/// never `cancelled`/82. The scripted stand-in is not a model: this shows
/// routing and observation levels, not processing.
#[test]
#[ignore = "needs the root supervisor built beside the Runner, OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN, OULIPOLY_NATIVE_SCRATCH and loopback"]
fn native_root_follow_up_reaches_the_live_session_and_close_is_not_cancel() {
    let scratch = PathBuf::from(var("OULIPOLY_NATIVE_SCRATCH"));
    let base_url = scripted_model();
    let root = Root::new(&scratch, "c", &base_url, "SAY first");
    let mut run = root.start();
    run.entry("setup-completed");
    let host_pid = run.event("launched")["pid"].as_u64().unwrap();
    let session = run.event("session-opened")["session"].clone();
    let at = |event: &'static str, field: &'static str, index: u64| {
        move |value: &Value| value["event"] == event && value[field] == index
    };

    let ack0 = run.until("ack 0", at("ack", "index", 0));
    let reply0 = run.until("reply to input 0", at("agent-message", "input", 0));
    let turn0 = run.until("turn-end 0", at("turn-end", "input", 0));
    assert_eq!(reply0["text"], "first", "{reply0}");
    assert_eq!(reply0["parent_message_id"], ack0["message_id"]);
    assert_eq!(turn0["message_id"], ack0["message_id"]);
    assert_eq!(turn0["own_output"], true);

    run.control(&[json!({ "cmd": "send", "text": "SAY second", "ref": "f1" })]);
    let received = run.event("follow-up-received");
    assert_eq!(received["stage"], "queued-not-admitted", "{received}");
    let admitted = run.event("follow-up-admitted");
    assert_eq!(admitted["input"], 1, "{admitted}");
    assert_eq!(admitted["ref"], "f1");
    assert_eq!(admitted["durable"], true);
    let ack1 = run.until("ack 1", at("ack", "index", 1));
    assert_ne!(ack1["message_id"], ack0["message_id"]);
    let reply1 = run.until("reply to input 1", at("agent-message", "input", 1));
    assert_eq!(reply1["text"], "second", "{reply1}");
    assert_eq!(reply1["input_attribution"], "native-parent");
    assert_eq!(reply1["session"], session);
    let turn1 = run.until("turn-end 1", at("turn-end", "input", 1));
    assert_eq!(turn1["message_id"], ack1["message_id"]);
    assert_eq!(turn1["session"], session);
    assert_eq!(turn1["stop_reason"], "end_turn");
    assert_eq!(turn1["own_output"], true);
    assert!(!ended(host_pid), "the host is still live after two turns");

    run.control(&[
        json!({ "cmd": "close" }),
        json!({ "cmd": "send", "text": "SAY third", "ref": "f2" }),
    ]);
    let close = run.event("close-requested");
    let refused = run.event("follow-up-refused");
    assert_eq!(refused["reason"], "input-closed", "{refused}");
    assert_eq!(refused["ref"], "f2");
    assert_eq!(run.event("close-stopping")["signalled"], true);
    let (owner, entry, code, seen) = run.finish();
    println!("close: {close}");

    let count = |event: &str| seen.iter().filter(|value| value["event"] == event).count();
    assert_eq!(count("launched"), 1, "one host launch");
    assert_eq!(count("session-opened"), 1, "one native session");
    assert_eq!(count("follow-up-admitted"), 1);
    assert_eq!(count("ack"), 2);
    assert_eq!(count("cancel-requested"), 0);
    assert_eq!(owner["status"], "closed", "{owner}");
    assert_eq!(owner["cancel_requested"], false);
    assert_eq!(owner["close_requested"], true);
    assert_eq!(owner["owed"], 0);
    assert_eq!(owner["all_harnesses_reaped"], true);
    assert_eq!(owner["root_pid1"]["end_observed"], true, "{owner}");
    let host = &owner["harnesses"][0];
    assert_eq!(host["exits"], json!(["signal:9"]), "{host}");
    assert_eq!(host["close"], "owner-stop-attempted-after-turns-ended");
    assert_eq!(host["messages"][1]["origin"], "follow-up");
    assert_eq!(host["messages"][1]["completion"], "not-observed");
    assert_eq!(entry["stage"], "owner-ended", "{entry}");
    assert_eq!(entry["owner_exit"], 7, "{entry}");
    assert_eq!(code, Some(87), "{entry}");
    wait_ended(host_pid, "native host");
}

/// The prepared host-root trial's in-root command: the Bash run's real and
/// effective ids and groups, its `no_new_privs` and effective capabilities,
/// then the disposable setuid-root identity probe's effective uid.
const IDENTITY: &str = r#"printf 'uid=%s euid=%s gid=%s groups=%s|' "$(id -ru)" "$(id -u)" "$(id -rg)" "$(id -G)"; awk '/^(NoNewPrivs|CapEff):/ {printf "%s|", $0}' /proc/self/status; "$OULIPOLY_SETUID_PROBE" -u"#;

/// `(uid, gid)` of a host user, from the host's own `id`.
fn host_ids(user: &str) -> (u32, u32) {
    let id = |flag: &str| {
        let output = Command::new("/usr/bin/id")
            .args([flag, user])
            .output()
            .unwrap();
        assert!(output.status.success(), "id {flag} {user}: {output:?}");
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
    };
    (id("-u"), id("-g"))
}

/// `(real, effective, saved, fs)` uids of a live process, from its status.
fn proc_uids(pid: u64) -> Option<Vec<u32>> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("Uid:").map(|rest| {
            rest.split_whitespace()
                .filter_map(|id| id.parse().ok())
                .collect()
        })
    })
}

fn proc_field(pid: u64, name: &str) -> Option<String> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix(name).map(|rest| rest.trim().to_owned()))
}

/// `(uid, mode)` of a path, not following a final symlink.
fn owned(path: impl AsRef<Path>) -> (u32, u32) {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path.as_ref())
        .unwrap_or_else(|error| panic!("{}: {error}", path.as_ref().display()));
    (meta.uid(), meta.mode() & 0o7777)
}

/// A fresh host-root request: the root's own directory is traversable,
/// its project and Bash HOME are the work user's, and the declared
/// environment names the setuid probe for the in-root command.
fn host_root(scratch: &Path, name: &str, base_url: &str, user: &str, ids: (u32, u32)) -> Root {
    use std::os::unix::fs::PermissionsExt;
    let mut root = Root::new(scratch, name, base_url, &format!("RUN {IDENTITY}"));
    std::fs::set_permissions(&root.dir, std::fs::Permissions::from_mode(0o711)).unwrap();
    for sub in ["owner-home", "project", "tmp"] {
        std::os::unix::fs::chown(root.dir.join(sub), Some(ids.0), Some(ids.1)).unwrap();
    }
    root.request["workload"] = json!({ "isolation": "host-root", "user": user });
    root.request["opencode"]["bash_allow"] = json!([IDENTITY]);
    root.request["env"]["OULIPOLY_SETUID_PROBE"] = json!(var("OULIPOLY_SETUID_PROBE"));
    root
}

/// Runs the prototype requester against a live root's Bash ingress from
/// outside every harness namespace, as `as_user` (or as this process).
fn outside_request(ingress: &str, cwd: &Path, as_user: Option<(u32, u32)>) -> std::process::Output {
    use std::os::unix::process::CommandExt;
    let requester = Path::new(RUNNER)
        .parent()
        .unwrap()
        .join("oulipoly-root-bash");
    let mut command = Command::new(requester);
    command
        .args(["--", "true"])
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_ROOT_BASH_V1", ingress)
        .stdin(Stdio::null());
    if let Some((uid, gid)) = as_user {
        command.gid(gid).uid(uid);
    }
    command.output().unwrap()
}

/// THE PREPARED HOST-ROOT TRIAL. Run once, as host uid 0, by ROOT only,
/// inside the trial script's own network, PID and mount namespaces with
/// loopback up; never by a model. The owner and its custody run as host
/// root; the native OpenCode host (answered by the scripted loopback
/// stand-in) and the in-root Bash run must run as `OULIPOLY_WORKLOAD_USER`
/// with that user's normal host capability (shown by the disposable
/// setuid-root probe `OULIPOLY_SETUID_PROBE`), and no other process of the
/// trial may be host root.
#[test]
#[ignore = "PRIVILEGED TRIAL: host uid 0 only, via the prepared trial script; needs the root supervisor binaries beside the Runner, OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN, OULIPOLY_NATIVE_SCRATCH, OULIPOLY_WORKLOAD_USER, OULIPOLY_SETUID_PROBE and loopback"]
fn native_root_host_root_owner_runs_its_native_host_and_bash_as_the_work_user() {
    // SAFETY: geteuid has no preconditions.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "the privileged trial runs as host root only"
    );
    let user = var("OULIPOLY_WORKLOAD_USER");
    let (uid, gid) = host_ids(&user);
    assert_ne!(uid, 0, "the work user must not be root");
    let scratch = PathBuf::from(var("OULIPOLY_NATIVE_SCRATCH"));
    let base_url = scripted_model();

    // Pre-model refusals: nothing is created, nothing launched.
    for (name, workload, reason) in [
        (
            "x0",
            json!({ "isolation": "host-root", "user": "root" }),
            "is uid 0",
        ),
        (
            "x1",
            json!({ "isolation": "host-root", "user": "oulipoly-no-such-user" }),
            "no host user",
        ),
        (
            "x2",
            json!({ "isolation": "unprivileged-userns" }),
            "unprivileged-userns declared but the owner is euid 0",
        ),
    ] {
        let mut refused = host_root(&scratch, name, &base_url, &user, (uid, gid));
        refused.request["workload"] = workload;
        let mut run = refused.start();
        let entry = run.entry("terminal");
        let code = run.child.wait().unwrap().code();
        println!("refusal {name}: {entry}");
        assert_eq!(entry["stage"], "refused", "{entry}");
        assert_eq!(entry["effects"], "none", "{entry}");
        assert!(
            entry["reason"].as_str().unwrap().contains(reason),
            "{entry}"
        );
        assert_eq!(code, Some(64), "{entry}");
        for path in ["launch_dir", "store"] {
            assert!(
                !Path::new(refused.request[path].as_str().unwrap()).exists(),
                "{path}"
            );
        }
    }

    let root = host_root(&scratch, "h", &base_url, &user, (uid, gid));
    let launch = PathBuf::from(root.request["launch_dir"].as_str().unwrap());
    let store = PathBuf::from(root.request["store"].as_str().unwrap());
    let ipc = launch.join("ipc");
    let mut run = root.start();
    let setup = run.entry("setup-completed");
    assert_eq!(setup["workload"]["isolation"], "host-root-pidns", "{setup}");
    assert_eq!(setup["workload"]["uid"], uid, "{setup}");
    let owner = run.entry("owner-started")["pid"].as_u64().unwrap();
    assert_eq!(proc_uids(owner), Some(vec![0; 4]), "owner identity");
    let resolved = run.event("workload");
    assert_eq!(resolved["resolved"]["identity"]["uid"], uid, "{resolved}");
    let started = run.event("started");
    assert_eq!(started["isolation"], "host-root-pidns", "{started}");
    let pid1 = run.event("root-pid1-started");
    assert_eq!(pid1["workload"]["uid"], uid, "{pid1}");
    assert_eq!(pid1["observed"]["uid"], json!([0, 0, 0, 0]), "{pid1}");
    let pid1 = pid1["pid"].as_u64().unwrap();
    let launched = run.event("launched");
    let host = launched["pid"].as_u64().unwrap();
    assert_eq!(
        launched["identity"]["uid"],
        json!([uid, uid, uid, uid]),
        "{launched}"
    );
    assert_eq!(
        launched["identity"]["gid"],
        json!([gid, gid, gid, gid]),
        "{launched}"
    );
    assert_eq!(
        launched["identity"]["no_new_privs"],
        json!([0]),
        "{launched}"
    );
    run.event("endpoint-connected");
    assert_eq!(proc_uids(host), Some(vec![uid; 4]), "native host identity");

    // Placement: the store is the owner's alone; the native config is
    // readable but not the work user's; the work user's HOME, data and IPC.
    assert_eq!(owned(&store), (0, 0o700), "store");
    assert_eq!(owned(launch.as_path()), (0, 0o750), "launch dir");
    assert_eq!(owned(launch.join("home")), (uid, 0o700), "native HOME");
    assert_eq!(owned(launch.join("xdg/data")), (uid, 0o700), "native data");
    let config = launch.join("xdg/config/opencode");
    assert_eq!(owned(&config), (0, 0o1770), "native config dir");
    assert_eq!(
        owned(config.join("opencode.json")),
        (0, 0o640),
        "native policy"
    );
    assert_eq!(
        owned(config.join("tool/bash.ts")),
        (0, 0o640),
        "native gate"
    );
    assert_eq!(owned(&ipc), (0, 0o711), "ipc dir");
    assert_eq!(owned(ipc.join("acp")), (uid, 0o700), "harness socket dir");
    assert_eq!(owned(ipc.join("bash.sock")).0, uid, "ingress socket");

    let session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(run.event("ack")["label"], "accepted");
    let accepted = run.event("bash-accepted");
    assert_eq!(
        accepted["argv"],
        json!(["bash", "-lc", IDENTITY]),
        "{accepted}"
    );
    let bash_started = run.event("bash-started");
    println!("bash started: {bash_started}");
    if !bash_started["identity"].is_null() {
        assert_eq!(
            bash_started["identity"]["uid"],
            json!([uid, uid, uid, uid]),
            "{bash_started}"
        );
    }
    let ended = run.event("bash-ended");
    println!("bash ended: {ended}");
    let idle = run.event("idle");
    assert_eq!(idle["stop_reason"], "end_turn", "{idle}");
    assert_eq!(run.event("agent-message")["text"], "DONE");

    // Census of this trial's own PID namespace while the native host
    // lives: host root only for this test, the Runner entry, the owner,
    // root PID 1 and work PID 1s; everything else is the work user.
    let me = u64::from(std::process::id());
    let entry = u64::from(run.child.id());
    let mut census = Vec::new();
    for proc_entry in std::fs::read_dir("/proc").unwrap() {
        let Some(pid) = proc_entry
            .ok()
            .and_then(|e| e.file_name().to_str()?.parse::<u64>().ok())
        else {
            continue;
        };
        let Some(uids) = proc_uids(pid) else { continue };
        let name = proc_field(pid, "Name:").unwrap_or_default();
        let ppid: u64 = proc_field(pid, "PPid:")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        census.push((pid, ppid, name, uids));
    }
    println!("census: {census:?}");
    for (pid, ppid, name, uids) in &census {
        let custody = *pid == me
            || *pid == entry
            || *pid == owner
            || *pid == pid1
            || (*ppid == pid1 && name.starts_with("oulipoly-root-p"));
        if uids.contains(&0) {
            assert!(
                custody,
                "host-root process outside custody: {pid} {name} {uids:?}"
            );
        } else {
            assert_eq!(uids, &vec![uid; 4], "unexpected identity: {pid} {name}");
        }
    }
    assert!(
        census.iter().any(|(pid, ..)| *pid == host),
        "native host in census"
    );

    // Owner IPC refuses peers it cannot attribute: host root outside every
    // harness namespace, and the work user outside every harness namespace.
    let ingress = ipc.join("bash.sock");
    let ingress = ingress.to_str().unwrap();
    let project = PathBuf::from(root.request["cwd"].as_str().unwrap());
    for (as_user, reason) in [
        (None, "peer-unattributed: other-uid"),
        (
            Some((uid, gid)),
            "peer-unattributed: outside-every-harness-namespace",
        ),
    ] {
        let output = outside_request(ingress, &project, as_user);
        println!("outside {as_user:?}: {output:?}");
        assert_eq!(output.status.code(), Some(69), "{output:?}");
        let refused = run.event("bash-refused");
        assert_eq!(refused["reason"], reason, "{refused}");
        run.seen.retain(|value| value != &refused);
    }

    let (owner_end, entry_end, code) = run.cancel();
    assert_eq!(owner_end["status"], "cancelled", "{owner_end}");
    assert_eq!(owner_end["all_harnesses_reaped"], true, "{owner_end}");
    assert_eq!(owner_end["root_pid1"]["end_observed"], true, "{owner_end}");
    assert_eq!(owner_end["bash"]["accepted"], 1, "{owner_end}");
    assert_eq!(owner_end["bash"]["refused"], 2, "{owner_end}");
    assert_eq!(code, Some(82), "{entry_end}");

    // What the Bash run itself reported, read by OpenCode as the work user.
    let parts = {
        use std::os::unix::process::CommandExt;
        let opencode = Path::new(&var("OULIPOLY_NATIVE_DEPS"))
            .join("node_modules/opencode-linux-x64/bin/opencode");
        let output = Command::new(opencode)
            .args(["export", "--pure", &session])
            .current_dir(&project)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("TMPDIR", root.dir.join("tmp"))
            .envs(
                setup["launch"]["env"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k, v.as_str().unwrap())),
            )
            .stdin(Stdio::null())
            .gid(gid)
            .uid(uid)
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
                part["state"]["output"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    println!("bash run output: {parts:?}");
    assert_eq!(parts.len(), 1, "{parts:?}");
    let output = parts[0].trim_end();
    assert!(
        output.starts_with("Root v1 work ended: exited with code 0"),
        "{output}"
    );
    let expected = format!("uid={uid} euid={uid} gid={gid} ");
    assert!(output.contains(&expected), "bash run identity: {output}");
    assert!(
        output.contains("NoNewPrivs:\t0|"),
        "bash run no_new_privs: {output}"
    );
    assert!(
        output.ends_with("|0"),
        "setuid probe effective uid: {output}"
    );
}

/// The in-root command of the authenticated roots: whether the Bash run
/// inherited the host's server password or an inline auth, and its uid.
const SECRETS_ABSENT: &str = r#"printf 'pw=%s auth=%s uid=%s' "${OPENCODE_SERVER_PASSWORD+set}" "${OPENCODE_AUTH_CONTENT+set}" "$(id -u)""#;
/// The fixture provider's key, given only in the private auth file.
const AUTH_KEY: &str = "fixture-auth-file-key-not-a-credential";
/// Stand-ins for the shape of a subscription entry; the provider is not
/// enabled, so nothing uses them.
const OAUTH_ACCESS: &str = "fixture-oauth-access-not-a-token";

/// Makes `root` an authenticated request: its fixture provider's key only
/// in a private auth file the request names (beside an unused OAuth entry
/// of the subscription's shape), and the command `SECRETS_ABSENT`.
fn authenticated(root: &mut Root, owner: Option<(u32, u32)>) -> PathBuf {
    use std::os::unix::fs::OpenOptionsExt;
    let auth = root.dir.join("auth.json");
    let text = json!({
        "fixture": { "type": "api", "key": AUTH_KEY },
        "openai": { "type": "oauth", "refresh": "", "access": OAUTH_ACCESS,
            "expires": 4_102_444_800_000_u64, "accountId": "fixture-account" },
    })
    .to_string();
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&auth)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .unwrap();
    if let Some((uid, gid)) = owner {
        std::os::unix::fs::chown(&auth, Some(uid), Some(gid)).unwrap();
    }
    root.request["opencode"]["auth"] = json!(auth);
    let options = &mut root.request["opencode"]["provider"]["fixture"]["options"];
    options.as_object_mut().unwrap().remove("apiKey");
    root.request["opencode"]["bash_allow"] = json!([SECRETS_ABSENT]);
    root.request["messages"] = json!([format!("RUN {SECRETS_ABSENT}")]);
    auth
}

/// The TCP port the process `pid` listens on at 127.0.0.1, from its own
/// network namespace's table and its socket descriptors.
fn listening_port(pid: u64) -> u16 {
    let inodes: Vec<String> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|fd| {
            let link = std::fs::read_link(fd.ok()?.path()).ok()?;
            let link = link.to_str()?;
            Some(link.strip_prefix("socket:[")?.strip_suffix(']')?.to_owned())
        })
        .collect();
    let table = std::fs::read_to_string(format!("/proc/{pid}/net/tcp")).unwrap();
    let ports: Vec<u16> = table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (address, port) = fields.get(1)?.split_once(':')?;
            (address == "0100007F"
                && fields.get(3) == Some(&"0A")
                && inodes
                    .iter()
                    .any(|inode| Some(&inode.as_str()) == fields.get(9)))
            .then(|| u16::from_str_radix(port, 16).ok())?
        })
        .collect();
    assert_eq!(
        ports.len(),
        1,
        "listening ports of {pid}: {ports:?}\n{table}"
    );
    ports[0]
}

/// The status of one `GET /session` to the host's loopback server, with
/// the given Basic credential, if any.
fn http_status(port: u16, credential: Option<&str>) -> u16 {
    use base64::Engine;
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    let mut request =
        format!("GET /session HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nconnection: close\r\n");
    if let Some(credential) = credential {
        let encoded = base64::engine::general_purpose::STANDARD.encode(credential);
        request.push_str(&format!("authorization: Basic {encoded}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).unwrap();
    let mut status = String::new();
    BufReader::new(stream).read_line(&mut status).unwrap();
    status
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no HTTP status: {status:?}"))
}

/// Tool outputs of the native conversation, read by OpenCode itself from
/// the launch's data directory, as `work` when given.
fn tool_outputs(
    root: &Root,
    launch: &Value,
    session: &str,
    work: Option<(u32, u32)>,
) -> Vec<String> {
    use std::os::unix::process::CommandExt;
    let opencode = Path::new(&var("OULIPOLY_NATIVE_DEPS"))
        .join("node_modules/opencode-linux-x64/bin/opencode");
    let mut command = Command::new(opencode);
    command
        .args(["export", "--pure", session])
        .current_dir(root.request["cwd"].as_str().unwrap())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", root.dir.join("tmp"))
        .envs(
            launch["env"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, value)| (key, value.as_str().unwrap())),
        )
        .stdin(Stdio::null());
    if let Some((uid, gid)) = work {
        command.gid(gid).uid(uid);
    }
    let output = command.output().unwrap();
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
            part["state"]["output"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// Runs one authenticated root (see [`authenticated`]) through one
/// scripted turn and its cancel. `work` is the host-root work identity;
/// `None` is an unprivileged root, everything this process's.
fn authenticated_run(root: &Root, auth: &Path, work: Option<(u32, u32)>) {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: getuid has no preconditions.
    let uid = work.map_or_else(|| unsafe { libc::getuid() }, |(uid, _)| uid);
    let auth_text = std::fs::read_to_string(auth).unwrap();
    let launch_dir = PathBuf::from(root.request["launch_dir"].as_str().unwrap());
    let mut run = root.start();
    let setup = run.entry("setup-completed");
    let placed = &setup["launch"]["auth"];
    assert_eq!(placed["default_plugins"], "enabled", "{setup}");
    let password_file = launch_dir.join("secret/server-password");
    assert_eq!(placed["server_password_file"], json!(password_file));
    let auth_file = launch_dir.join("xdg/data/opencode/auth.json");
    assert_eq!(placed["auth_file"], json!(auth_file));
    let password = std::fs::read_to_string(&password_file).unwrap();
    let password = password.trim_end().to_owned();
    assert_eq!(password.len(), 64, "password length");
    assert_eq!(std::fs::read_to_string(&auth_file).unwrap(), auth_text);
    for (path, mode) in [
        (launch_dir.join("secret"), 0o700),
        (password_file.clone(), 0o600),
        (launch_dir.join("xdg/data/opencode"), 0o700),
        (auth_file.clone(), 0o600),
    ] {
        let meta = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!((meta.uid(), meta.mode() & 0o7777), (uid, mode), "{path:?}");
    }

    let host_pid = run.event("launched")["pid"].as_u64().unwrap();
    run.event("endpoint-connected");
    // The endpoint plugin's own client reached the protected server.
    let session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let port = listening_port(host_pid);
    let unauthenticated = http_status(port, None);
    let wrong = http_status(port, Some("opencode:not-the-password"));
    let authorized = http_status(port, Some(&format!("opencode:{password}")));
    println!("loopback {port}: none={unauthenticated} wrong={wrong} password={authorized}");
    assert_eq!((unauthenticated, wrong, authorized), (401, 401, 200));
    let host = environ(host_pid);
    assert_eq!(
        host.get("OPENCODE_SERVER_PASSWORD"),
        Some(&password),
        "host password"
    );
    for absent in ["OPENCODE_DISABLE_DEFAULT_PLUGINS", "OPENCODE_AUTH_CONTENT"] {
        assert!(!host.contains_key(absent), "host has {absent}");
    }

    assert_eq!(run.event("ack")["label"], "accepted");
    let accepted = run.event("bash-accepted");
    assert_eq!(
        accepted["argv"],
        json!(["bash", "-lc", SECRETS_ABSENT]),
        "{accepted}"
    );
    run.event("bash-ended");
    let idle = run.event("idle");
    assert_eq!(idle["stop_reason"], "end_turn", "{idle}");
    assert_eq!(run.event("agent-message")["text"], "DONE");
    let seen_by_model = AUTHORIZATIONS.lock().unwrap().clone();
    assert!(
        seen_by_model
            .iter()
            .any(|value| value == &format!("Bearer {AUTH_KEY}")),
        "the provider key from the auth file reached the stand-in: {seen_by_model:?}"
    );

    run.control(&[json!({ "cmd": "cancel" })]);
    let (owner, entry, code, seen) = run.finish();
    assert_eq!(owner["status"], "cancelled", "{owner}");
    assert_eq!(owner["all_harnesses_reaped"], true, "{owner}");
    assert_eq!(owner["root_pid1"]["end_observed"], true, "{owner}");
    assert_eq!(owner["bash"]["accepted"], 1, "{owner}");
    assert_eq!(code, Some(82), "{entry}");
    for value in &seen {
        let text = value.to_string();
        for secret in [password.as_str(), AUTH_KEY, OAUTH_ACCESS] {
            assert!(!text.contains(secret), "secret on stdout: {text}");
        }
    }
    // No refresh or other write-back reached the placed copy.
    assert_eq!(std::fs::read_to_string(&auth_file).unwrap(), auth_text);
    let outputs = tool_outputs(root, &setup["launch"], &session, work);
    println!("bash run output: {outputs:?}");
    assert_eq!(outputs.len(), 1, "{outputs:?}");
    // An unprivileged root's work runs as uid 0 of its own user namespace;
    // a host-root root's work is the work user's host uid.
    let expected = match work {
        Some((uid, _)) => format!("pw= auth= uid={uid}"),
        None => "pw= auth= uid=0".to_owned(),
    };
    assert!(
        outputs[0].trim_end().ends_with(&expected),
        "the Bash run inherited neither secret: {outputs:?}"
    );
}

/// An authenticated unprivileged root (in the loopback-only fixture): the
/// auth file is placed privately and read by the host, built-in plugins
/// load, and the host's loopback server refuses requests without its
/// password while the endpoint plugin's own client gets through. The
/// scripted stand-in is not a model and the OAuth entry is never used.
#[test]
#[ignore = "needs the root supervisor built beside the Runner, OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN, OULIPOLY_NATIVE_SCRATCH and loopback only"]
fn native_root_authenticated_host_requires_its_password_and_reads_a_private_auth_file() {
    let scratch = PathBuf::from(var("OULIPOLY_NATIVE_SCRATCH"));
    let base_url = scripted_model();
    let mut root = Root::new(&scratch, "u", &base_url, "unused");
    let auth = authenticated(&mut root, None);
    authenticated_run(&root, &auth, None);
}

/// THE PREPARED AUTHENTICATED HOST-ROOT SAMPLE. Run once, as host uid 0,
/// by ROOT only, inside its script's own network, PID and mount namespaces
/// with loopback up; never by a model. The same checks as the unprivileged
/// authenticated root, with a host-root owner and the auth file, password,
/// native host and Bash run all the work user's.
#[test]
#[ignore = "PRIVILEGED SAMPLE: host uid 0 only, via the prepared script; needs the root supervisor binaries beside the Runner, OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN, OULIPOLY_NATIVE_SCRATCH, OULIPOLY_WORKLOAD_USER and loopback only"]
fn native_root_host_root_authenticated_host_runs_as_the_work_user_behind_its_password() {
    // SAFETY: geteuid has no preconditions.
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "the privileged sample runs as host root only"
    );
    let user = var("OULIPOLY_WORKLOAD_USER");
    let (uid, gid) = host_ids(&user);
    assert_ne!(uid, 0, "the work user must not be root");
    let scratch = PathBuf::from(var("OULIPOLY_NATIVE_SCRATCH"));
    let base_url = scripted_model();
    let mut root = Root::new(&scratch, "a", &base_url, "unused");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root.dir, std::fs::Permissions::from_mode(0o711)).unwrap();
    }
    for sub in ["owner-home", "project", "tmp"] {
        std::os::unix::fs::chown(root.dir.join(sub), Some(uid), Some(gid)).unwrap();
    }
    root.request["workload"] = json!({ "isolation": "host-root", "user": user });
    // The caller's private auth file stays host root's: the entry reads it.
    let auth = authenticated(&mut root, None);
    authenticated_run(&root, &auth, Some((uid, gid)));
}
