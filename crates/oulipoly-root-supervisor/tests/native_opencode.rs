//! The real supervisor owning a real native OpenCode host whose in-process
//! ACP v2 endpoint (`native/opencode/acp-v2-endpoint.ts`) listens on the
//! Unix socket the owner chose. Ignored by default: it needs the public
//! dependencies of `native/opencode/package.json` installed (`npm ci
//! --ignore-scripts`) in the directory named by `OULIPOLY_NATIVE_DEPS`.
//!
//! The first test's host is insertion-only (`OULIPOLY_ACP_V2_NO_REPLY=1`): no
//! model turn starts, and no model, provider or credential is configured.
//! The second runs real model turns: it also needs `OULIPOLY_NATIVE_MODEL`
//! (`provider/model`, a model OpenCode's OpenAI ChatGPT-OAuth path accepts)
//! and `OULIPOLY_NATIVE_AUTH`, a private OpenCode `auth.json` copied into the
//! fixture's own data directory and never printed; it needs outbound network
//! to that provider instead of loopback-only. Every host
//! gets a fresh HOME/XDG state under the scratch directory, shared by the
//! three runs so that the second host process sees the first one's native
//! conversation. Run it with loopback-only networking, e.g. inside
//! `unshare --user --map-current-user --net --pid --fork`, which also
//! bounds cleanup: everything in that PID namespace ends with it.
//!
//! The third launches a host provisioned only by the shipped owner setup
//! entry (`oulipoly-native-opencode-setup`): its launch directory holds the
//! actual agent-bash `bash` tool behind the native permission gate
//! `native/opencode/bash-policy-tool.ts` and the root's deny-default
//! policy, and its turns are driven by a scripted loopback stand-in for a
//! model (no model, provider or credential). It also needs
//! `OULIPOLY_AGENT_BASH_TOOL`, the matching agent-bash
//! `integrations/opencode/tools/bash.ts`, and `AGENT_BASH_BIN`, that
//! source's `agent-bash` binary. Its loopback must be up (in a new network
//! namespace, `ip link set lo up` before dropping capabilities).
//!
//! The only timer is a test watchdog; on expiry the test cancels its
//! supervisor and fails.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_oulipoly-root-supervisor");
const SETUP: &str = env!("CARGO_BIN_EXE_oulipoly-native-opencode-setup");
const WATCHDOG: Duration = Duration::from_secs(150);

/// A real configured model for the second test.
struct Model {
    id: String,
    auth: PathBuf,
}

impl Model {
    fn from_env() -> Self {
        let id = std::env::var("OULIPOLY_NATIVE_MODEL").expect("OULIPOLY_NATIVE_MODEL must be set");
        let auth = PathBuf::from(
            std::env::var("OULIPOLY_NATIVE_AUTH").expect("OULIPOLY_NATIVE_AUTH must be set"),
        );
        Self { id, auth }
    }
}

/// What a fixture's native host runs.
enum Host<'a> {
    /// Insertion only: no model turn starts.
    InsertionOnly,
    /// Real configured model turns.
    Model(&'a Model),
    /// Provisioned by the owner setup entry with this root's `bash`
    /// policy fields (`bash_allow` or `bash_authority`); turns answered by
    /// a [`ScriptedModel`].
    ScriptedBash { base_url: &'a str, bash: Value },
}

struct Fixture {
    dir: PathBuf,
    opencode: PathBuf,
    project: PathBuf,
    env: Vec<(String, String)>,
    /// The harness argv; `None`: the OpenCode binary itself.
    argv: Option<Vec<String>>,
    /// The environment `opencode export` reads the host's store with.
    export_env: Vec<(String, String)>,
    /// The owner setup entry's receipt, for a provisioned host.
    receipt: Option<Value>,
}

impl Fixture {
    fn new(host: Host) -> Self {
        let deps = PathBuf::from(
            std::env::var("OULIPOLY_NATIVE_DEPS").expect("OULIPOLY_NATIVE_DEPS must be set"),
        );
        let opencode = deps.join("node_modules/opencode-linux-x64/bin/opencode");
        assert!(opencode.is_file(), "no OpenCode binary at {opencode:?}");
        // The endpoint resolves its SDK from `deps/node_modules`.
        let endpoint = deps.join("acp-v2-endpoint.ts");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("native/opencode/acp-v2-endpoint.ts"),
            &endpoint,
        )
        .unwrap();
        let base = std::env::var("OULIPOLY_NATIVE_SCRATCH")
            .map_or_else(|_| std::env::temp_dir(), PathBuf::from);
        // One directory per fixture: tests in one process run concurrently.
        static FIXTURES: AtomicUsize = AtomicUsize::new(0);
        let dir = base.join(format!(
            "native-{}-{}",
            std::process::id(),
            FIXTURES.fetch_add(1, Ordering::Relaxed)
        ));
        let mkdir = |path: &Path| {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .unwrap();
        };
        // A fresh directory: a PID namespace repeats pids across runs.
        mkdir(&base);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .unwrap_or_else(|error| panic!("fixture {dir:?}: {error} (use a fresh scratch)"));
        let project = dir.join("project");
        for sub in [
            "home",
            "project",
            "xdg/config",
            "xdg/data",
            "xdg/cache",
            "xdg/state",
        ] {
            mkdir(&dir.join(sub));
        }
        let path = |sub: &str| dir.join(sub).to_str().unwrap().to_owned();
        let mut config = json!({
            "plugin": [format!("file://{}", endpoint.display())],
            "autoupdate": false,
            "share": "disabled",
        });
        let mut env = vec![
            ("PATH", "/usr/bin:/bin".to_owned()),
            ("HOME", path("home")),
            ("XDG_CONFIG_HOME", path("xdg/config")),
            ("XDG_DATA_HOME", path("xdg/data")),
            ("XDG_CACHE_HOME", path("xdg/cache")),
            ("XDG_STATE_HOME", path("xdg/state")),
            ("OPENCODE_DISABLE_AUTOUPDATE", "1".to_owned()),
            ("OPENCODE_DISABLE_PROJECT_CONFIG", "1".to_owned()),
            ("OPENCODE_DISABLE_CLAUDE_CODE", "1".to_owned()),
            ("OPENCODE_DISABLE_LSP_DOWNLOAD", "1".to_owned()),
            ("OPENCODE_DISABLE_SHARE", "1".to_owned()),
            ("OULIPOLY_ACP_V2_LOG", path("acp-v2.log")),
        ];
        if let Ok(trace) = std::env::var("OULIPOLY_ACP_V2_TRACE") {
            env.push(("OULIPOLY_ACP_V2_TRACE", trace));
        }
        match host {
            Host::InsertionOnly => {
                env.push(("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1".to_owned()));
                env.push(("OULIPOLY_ACP_V2_NO_REPLY", "1".to_owned()));
            }
            Host::Model(model) => {
                // The built-in (default) plugins carry OpenCode's own OpenAI
                // ChatGPT-OAuth support, which reads `<data>/opencode/auth.json`.
                let data = dir.join("xdg/data/opencode");
                mkdir(&data);
                std::fs::copy(&model.auth, data.join("auth.json")).unwrap();
                let (provider, name) = model.id.split_once('/').expect("provider/model");
                config["model"] = json!(model.id);
                // Every tool call needs permission, so the turn that asks for
                // one shows the route it takes; nothing else may be used.
                config["permission"] = json!({ "*": "ask" });
                // No extra model call to title the conversation.
                config["agent"] = json!({ "title": { "disable": true } });
                config["provider"] = json!({ provider: { "models": { name: {
                    "name": name,
                    "options": {
                        "reasoningEffort": "high",
                        "reasoningSummary": "auto",
                        "include": ["reasoning.encrypted_content"],
                        "store": false,
                    },
                }}}});
            }
            Host::ScriptedBash { base_url, bash } => {
                // Only the shipped owner setup entry provisions this host.
                let mut setup = json!({
                    "dir": dir.join("launch"),
                    "deps": deps,
                    "agent_bash_tool": std::env::var("OULIPOLY_AGENT_BASH_TOOL")
                        .expect("OULIPOLY_AGENT_BASH_TOOL must be set"),
                    "agent_bash_bin": std::env::var("AGENT_BASH_BIN")
                        .expect("AGENT_BASH_BIN must be set"),
                    "model": "fixture/scripted",
                    "provider": { "fixture": {
                        "npm": "@ai-sdk/openai-compatible",
                        "name": "fixture",
                        // Not a credential: the stand-in reads no header.
                        "options": { "baseURL": base_url, "apiKey": "fixture-not-a-credential" },
                        "models": { "scripted": { "name": "scripted", "tool_call": true } },
                    }},
                });
                for (key, value) in bash.as_object().unwrap() {
                    setup[key] = value.clone();
                }
                let mut child = Command::new(SETUP)
                    .env_clear()
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap();
                writeln!(child.stdin.take().unwrap(), "{setup}").unwrap();
                let output = child.wait_with_output().unwrap();
                let launch: Value = serde_json::from_slice(&output.stdout).unwrap();
                println!("setup: {launch}");
                assert!(output.status.success(), "{launch}");
                let receipt = launch.clone();
                // The host's own HOME/XDG and policy come from its argv.
                // The owner keeps its HOME: root v1 Bash work runs in the
                // owner's environment, not the requester's.
                env.retain(|(key, _)| {
                    *key == "PATH" || *key == "HOME" || key.starts_with("OULIPOLY_")
                });
                let argv = launch["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap().to_owned())
                    .collect();
                let launch_env: Vec<(String, String)> = launch["env"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
                    .collect();
                // A caller's inline config would join the policy; the
                // launch removes it, which this one checks.
                let mut env: Vec<(String, String)> = env
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect();
                env.push((
                    "OPENCODE_CONFIG_CONTENT".to_owned(),
                    json!({ "permission": { "*": "allow", "bash": "allow" } }).to_string(),
                ));
                return Self {
                    receipt: Some(receipt),
                    export_env: launch_env
                        .into_iter()
                        .chain([("PATH".to_owned(), "/usr/bin:/bin".to_owned())])
                        .collect(),
                    dir,
                    opencode,
                    project,
                    env,
                    argv: Some(argv),
                };
            }
        }
        env.push(("OPENCODE_CONFIG_CONTENT", config.to_string()));
        let env: Vec<(String, String)> = env
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        Self {
            dir,
            opencode,
            project,
            export_env: env.clone(),
            env,
            argv: None,
            receipt: None,
        }
    }

    /// A create request for a new root store with one native host.
    fn spec(&self, store: &str, session: Option<&str>, message: &str) -> Value {
        json!({
            "store": self.dir.join(store),
            "intent": {
                "outage_closure_cap": 1,
                "delivery_attempt_cap": 1,
                "cwd": self.project,
                // Run in an outer user namespace fixture: unprivileged.
                "workload": { "isolation": "unprivileged-userns" },
                "harnesses": [{
                    "id": "opencode",
                    // `serve` loads a directory's instance, and so its
                    // plugins, only per HTTP request; `acp` loads its cwd's
                    // at startup. Its own stdio ACP surface is never spoken
                    // to: the owner talks only to the endpoint's socket.
                    "argv": self.argv.clone().map_or_else(
                        || json!([self.opencode, "acp", "--hostname", "127.0.0.1", "--port", "0"]),
                        |argv| json!(argv),
                    ),
                    "endpoint": "unix-socket",
                    "session": session,
                    "messages": [message],
                }],
            },
        })
    }

    /// The native conversation as OpenCode itself exports it, with no plugin
    /// loaded and no host running: `(message id, role, text parts)`.
    fn export(&self, session: &str) -> Vec<(String, String, Vec<String>)> {
        let output = Command::new(&self.opencode)
            .args(["export", "--pure", session])
            .current_dir(&self.project)
            .env_clear()
            .envs(self.export_env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "export: {output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        let start = text.find('{').expect("json in export output");
        let exported: Value = serde_json::from_str(&text[start..]).unwrap();
        exported["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| {
                let texts = message["parts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|part| part["type"] == "text")
                    .map(|part| part["text"].as_str().unwrap().to_owned())
                    .collect();
                (
                    message["info"]["id"].as_str().unwrap().to_owned(),
                    message["info"]["role"].as_str().unwrap().to_owned(),
                    texts,
                )
            })
            .collect()
    }
}

impl Fixture {
    /// Every tool part of the native conversation as OpenCode itself
    /// exports it: `(tool, status, input, output or error)`.
    fn tool_parts(&self, session: &str) -> Vec<(String, String, Value, String)> {
        let output = Command::new(&self.opencode)
            .args(["export", "--pure", session])
            .current_dir(&self.project)
            .env_clear()
            .envs(self.export_env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "export: {output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        let start = text.find('{').expect("json in export output");
        let exported: Value = serde_json::from_str(&text[start..]).unwrap();
        exported["messages"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|message| message["parts"].as_array().unwrap().iter())
            .filter(|part| part["type"] == "tool")
            .map(|part| {
                let state = &part["state"];
                let result = state["output"]
                    .as_str()
                    .or_else(|| state["error"].as_str())
                    .unwrap_or_default();
                (
                    part["tool"].as_str().unwrap().to_owned(),
                    state["status"].as_str().unwrap().to_owned(),
                    state["input"].clone(),
                    result.to_owned(),
                )
            })
            .collect()
    }
}

/// A loopback stand-in for a model, never a model or provider: an
/// OpenAI-compatible streaming chat-completions endpoint answering from a
/// script, the turn's user message:
///
/// * `RUN <command>`: calls `bash` once with that command;
/// * `CALLS <json>`: makes the `[tool, arguments]` calls of that array,
///   one per model request, the next after each tool result;
/// * anything else: answers `NO-SCRIPT`.
///
/// Once its calls are made it answers the text `DONE`. It keeps every
/// request (path and body), any path.
struct ScriptedModel {
    base_url: String,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl ScriptedModel {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let kept = Arc::clone(&kept);
                std::thread::spawn(move || Self::serve(stream, &kept));
            }
        });
        Self { base_url, requests }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    fn serve(mut stream: TcpStream, kept: &Mutex<Vec<Value>>) {
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
        let path = start
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_owned();
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        kept.lock()
            .unwrap()
            .push(json!({ "path": path, "body": body }));
        if !path.ends_with("/chat/completions") {
            let _ = stream.write_all(
                b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            );
            return;
        }
        let chunk = |delta: Value, finish: Value| {
            json!({
                "id": "scripted",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "scripted",
                "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
            })
        };
        let messages = body["messages"].as_array().cloned().unwrap_or_default();
        let user_text = |message: &Value| match &message["content"] {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        };
        // The turn's user message and the tool results since it.
        let user = messages
            .iter()
            .rposition(|message| message["role"] == "user");
        let text = user.map(|at| user_text(&messages[at])).unwrap_or_default();
        let results = user.map_or(0, |at| {
            messages[at..]
                .iter()
                .filter(|message| message["role"] == "tool")
                .count()
        });
        let calls: Vec<(String, Value)> = if let Some(command) = text.strip_prefix("RUN ") {
            vec![("bash".to_owned(), json!({ "command": command }))]
        } else if let Some(calls) = text.strip_prefix("CALLS ") {
            serde_json::from_str::<Vec<(String, Value)>>(calls).unwrap_or_default()
        } else {
            Vec::new()
        };
        let chunks = match calls.get(results) {
            Some((name, arguments)) => vec![
                chunk(
                    json!({ "role": "assistant", "tool_calls": [{
                        "index": 0,
                        "id": format!("call_scripted_{}", results + 1),
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": arguments.to_string(),
                        },
                    }]}),
                    Value::Null,
                ),
                chunk(json!({}), json!("tool_calls")),
            ],
            None => {
                let text = if results > 0 { "DONE" } else { "NO-SCRIPT" };
                vec![
                    chunk(json!({ "role": "assistant", "content": text }), Value::Null),
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
}

struct Run {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    seen: Vec<Value>,
}

impl Run {
    fn start(fixture: &Fixture, spec: &Value) -> Self {
        let mut child = Command::new(SUPERVISOR)
            .env_clear()
            .envs(fixture.env.iter().map(|(key, value)| (key, value)))
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
                println!("supervisor: {line}");
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
            assert_ne!(value["event"], "terminal", "terminal before {what}");
        }
    }

    fn event(&mut self, event: &str) -> Value {
        self.until(event, |value| value["event"] == event)
    }

    /// Cancels and returns the terminal report and exit code.
    fn cancel(mut self) -> (Value, Option<i32>) {
        writeln!(self.stdin, "{}", json!({ "cmd": "cancel" })).unwrap();
        let terminal = self.event("terminal");
        let status = self.child.wait().unwrap();
        (terminal, status.code())
    }
}

impl Drop for Run {
    /// On a failed assertion: cancel, so the owner has its harness killed by
    /// its work PID 1, then wait for the owner's own exit.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = writeln!(self.stdin, "{}", json!({ "cmd": "cancel" }));
            let _ = self.child.wait();
        }
    }
}

fn assert_cancelled_and_reaped(terminal: &Value, code: Option<i32>) {
    assert_eq!(code, Some(2), "{terminal}");
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert_eq!(terminal["all_harnesses_reaped"], true, "{terminal}");
    assert_eq!(terminal["root_pid1"]["end_observed"], true, "{terminal}");
}

/// A new native conversation is opened through one host; a second host
/// process, started by a second root, resumes exactly that conversation
/// and inserts after its history; a root naming a conversation the host
/// does not have is refused, not opened anew.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_DEPS (public OpenCode + ACP SDK) and loopback-only networking"]
fn native_host_opens_then_resumes_its_conversation_over_the_owner_socket() {
    let fixture = Fixture::new(Host::InsertionOnly);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let first_text = format!("OULIPOLY_NATIVE_FIRST_{stamp}");
    let second_text = format!("OULIPOLY_NATIVE_SECOND_{stamp}");

    let mut run = Run::start(&fixture, &fixture.spec("root-a", None, &first_text));
    run.event("endpoint-connected");
    let session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let first = run.event("ack");
    assert_eq!(first["label"], "accepted", "{first}");
    let first_id = first["message_id"].as_str().unwrap().to_owned();
    let (terminal, code) = run.cancel();
    assert_cancelled_and_reaped(&terminal, code);
    assert_eq!(
        terminal["harnesses"][0]["messages"][0]["state"],
        "acknowledged"
    );

    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-b", Some(&session), &second_text),
    );
    run.event("endpoint-connected");
    let resumed = run.event("session-resumed");
    assert_eq!(resumed["session"], session.as_str(), "{resumed}");
    let second = run.event("ack");
    assert_eq!(second["label"], "accepted", "{second}");
    let second_id = second["message_id"].as_str().unwrap().to_owned();
    let (terminal, code) = run.cancel();
    assert_cancelled_and_reaped(&terminal, code);

    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-c", Some("ses_doesnotexist000000000000"), "unsent"),
    );
    let failed = run.event("session-failed");
    assert!(
        failed["label"]
            .as_str()
            .unwrap()
            .starts_with("session-rejected-"),
        "{failed}"
    );
    let (terminal, code) = run.cancel();
    assert_cancelled_and_reaped(&terminal, code);
    let message = &terminal["harnesses"][0]["messages"][0];
    assert_eq!(message["state"], "owed", "{terminal}");
    assert!(
        message["label"]
            .as_str()
            .unwrap()
            .starts_with("session-rejected-"),
        "{terminal}"
    );

    let exported = fixture.export(&session);
    println!("export: {exported:?}");
    assert_eq!(
        exported,
        vec![
            (first_id, "user".to_owned(), vec![first_text]),
            (second_id, "user".to_owned(), vec![second_text]),
        ]
    );
    for store in ["root-a", "root-b", "root-c"] {
        let sockets = std::fs::read_dir(fixture.dir.join(store).join("acp"))
            .unwrap()
            .count();
        assert_eq!(sockets, 0, "{store}: socket left after observed end");
    }
}

/// One real configured model turn per root: root A opens a conversation and
/// its turn answers; root B resumes it with a turn that asks for a tool,
/// whose permission request reaches the owner, is refused there and is
/// rejected natively. Each turn's output, refusal and end reach the owner
/// over ACP v2 as reports distinct from the insertion ACK.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_DEPS, OULIPOLY_NATIVE_MODEL, OULIPOLY_NATIVE_AUTH and network to the provider; spends real model turns"]
fn native_host_runs_a_configured_model_turn_over_the_owner_socket() {
    let model = Model::from_env();
    let fixture = Fixture::new(Host::Model(&model));

    let mut run = Run::start(
        &fixture,
        &fixture.spec(
            "root-a",
            None,
            "Reply with exactly the single word PONG and nothing else. Do not use any tools.",
        ),
    );
    run.event("endpoint-connected");
    let session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    let ack = run.event("ack");
    assert_eq!(ack["label"], "accepted", "{ack}");
    let idle = run.event("idle");
    let answer = run.event("agent-message");
    println!("turn 1: {answer}\n{idle}");
    assert_eq!(idle["stop_reason"], "end_turn", "{idle}");
    assert!(
        answer["text"].as_str().unwrap().contains("PONG"),
        "{answer}"
    );
    let (terminal, code) = run.cancel();
    assert_cancelled_and_reaped(&terminal, code);

    let mut run = Run::start(
        &fixture,
        &fixture.spec(
            "root-b",
            Some(&session),
            "Use your bash tool exactly once to run: echo oulipoly-permission-probe . \
             If the tool is refused, do not retry; reply with the single word REFUSED.",
        ),
    );
    run.event("endpoint-connected");
    run.event("session-resumed");
    let ack = run.event("ack");
    assert_eq!(ack["label"], "accepted", "{ack}");
    let idle = run.event("idle");
    println!("turn 2: {idle}\nseen: {:#?}", run.seen);
    let refused = run.event("request-refused");
    assert_eq!(refused["method"], "session/request_permission", "{refused}");
    let notice = run.event("notice");
    assert_eq!(notice["severity"], "warning", "{notice}");
    let (terminal, code) = run.cancel();
    assert_cancelled_and_reaped(&terminal, code);

    let exported = fixture.export(&session);
    println!("export: {exported:?}");
    let roles: Vec<&str> = exported.iter().map(|(_, role, _)| role.as_str()).collect();
    assert_eq!(roles.first(), Some(&"user"));
    assert!(roles.contains(&"assistant"), "{exported:?}");
}

/// A host provisioned only by the owner setup entry loads the actual
/// agent-bash `bash` tool behind the native permission gate, and the
/// root's deny-default policy decides each command natively. The turns are
/// driven by a [`ScriptedModel`], not a model; the setup, the launch, the
/// tool, the permission evaluation, the owner's Bash ingress and the
/// command are real. The supervisor's environment carries an inline config
/// allowing everything, which the launch must remove.
///
/// * allowed (named in the setup): the command runs once through this
///   root's own Bash ingress and its root v1 result returns to the native
///   conversation.
/// * not named (the allowed command plus a suffix): native denial, no
///   permission request to the owner, nothing reaches the ingress.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN and loopback"]
fn owner_setup_launches_a_native_host_whose_policy_decides_the_agent_bash_tool() {
    const ALLOWED: &str = "printf oulipoly-native-allowed";
    const UNNAMED: &str = "printf oulipoly-native-allowed-unnamed";
    let model = ScriptedModel::start();
    let fixture = Fixture::new(Host::ScriptedBash {
        base_url: &model.base_url,
        bash: json!({ "bash_allow": [ALLOWED] }),
    });

    // The launch the setup provisioned: its own directory, removing
    // inherited config, with the root's policy in its config file.
    let argv = fixture.argv.clone().unwrap();
    assert_eq!(argv[0], "/usr/bin/env");
    assert!(
        argv.windows(2)
            .any(|pair| pair[0] == "-u" && pair[1] == "OPENCODE_CONFIG_CONTENT"),
        "{argv:?}"
    );
    let config_dir = fixture.dir.join("launch/xdg/config/opencode");
    let config = std::fs::read_to_string(config_dir.join("opencode.json")).unwrap();
    println!("launch config: {config}");
    let native = format!(r#"{{"*":"deny","bash":{{"*":"deny","{ALLOWED}":"allow"}}}}"#);
    assert!(
        config.ends_with(&format!(r#""permission":{native}}}"#)),
        "{config}"
    );
    // The receipt reports the policy the config holds.
    assert_eq!(
        fixture.receipt.as_ref().unwrap()["policy"],
        json!({ "bash": { "allow": [ALLOWED] }, "other": "deny", "native": native }),
    );
    for file in ["tool/bash.ts", "agent-bash/bash.ts", "acp-v2-endpoint.ts"] {
        assert!(config_dir.join(file).is_file(), "{file}");
    }

    // allowed
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-allow", None, &format!("RUN {ALLOWED}")),
    );
    run.event("endpoint-connected");
    let allowed_session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(run.event("ack")["label"], "accepted");
    let accepted = run.event("bash-accepted");
    assert_eq!(
        accepted["argv"],
        json!(["bash", "-lc", ALLOWED]),
        "{accepted}"
    );
    let ended = run.event("bash-ended");
    println!("allowed: {accepted}\n{ended}");
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
    let (terminal, code) = run.cancel();
    println!("allowed terminal: {terminal}");
    assert_cancelled_and_reaped(&terminal, code);
    assert_eq!(terminal["bash"]["accepted"], 1, "{terminal}");

    // not named
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-unnamed", None, &format!("RUN {UNNAMED}")),
    );
    run.event("endpoint-connected");
    let unnamed_session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(run.event("ack")["label"], "accepted");
    let idle = run.event("idle");
    println!("unnamed: {idle}");
    assert_eq!(idle["stop_reason"], "end_turn", "{idle}");
    assert_eq!(run.event("agent-message")["text"], "DONE");
    assert!(
        !run.seen.iter().any(|value| {
            value["event"] == "request-refused" || value["event"] == "bash-accepted"
        }),
        "{:#?}",
        run.seen
    );
    let (terminal, code) = run.cancel();
    println!("unnamed terminal: {terminal}");
    assert_cancelled_and_reaped(&terminal, code);
    assert_eq!(terminal["bash"]["accepted"], 0, "{terminal}");

    // What the native host offered the stand-in: only the agent-bash tool,
    // in place of the built-in, with its own arguments.
    let requests = model.requests();
    for request in &requests {
        let tools: Vec<&str> = request["body"]["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["function"]["name"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        println!("request {} tools {tools:?}", request["path"]);
    }
    assert_eq!(requests.len(), 4, "{requests:#?}");
    for request in &requests {
        let tools = request["body"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1, "{request}");
        assert_eq!(tools[0]["function"]["name"], "bash", "{request}");
    }
    let bash = &requests[0]["body"]["tools"][0]["function"];
    assert!(
        bash["description"]
            .as_str()
            .unwrap()
            .starts_with("Run a shell command under a detached supervisor."),
        "{bash}"
    );
    for arg in ["command", "handle", "delivery", "workdir"] {
        assert!(
            bash["parameters"]["properties"].get(arg).is_some(),
            "{bash}"
        );
    }

    // The native store's own record of each tool call, read from the
    // launch's own data directory.
    let allowed = fixture.tool_parts(&allowed_session);
    let unnamed = fixture.tool_parts(&unnamed_session);
    println!("allowed: {allowed:?}\nunnamed: {unnamed:?}");
    assert_eq!(allowed.len(), 1, "{allowed:?}");
    assert_eq!(allowed[0].0, "bash");
    assert_eq!(allowed[0].1, "completed", "{allowed:?}");
    assert_eq!(allowed[0].2["command"], ALLOWED);
    assert!(
        allowed[0]
            .3
            .starts_with("Root v1 work ended: exited with code 0")
            && allowed[0].3.contains("output-closed(bytes=23)")
            && allowed[0].3.ends_with("oulipoly-native-allowed"),
        "{allowed:?}"
    );
    assert_eq!(unnamed.len(), 1, "{unnamed:?}");
    assert_eq!(unnamed[0].1, "error", "{unnamed:?}");
    assert_eq!(unnamed[0].2["command"], UNNAMED);
}

/// Every model request's offered tool names.
fn offered_tools(requests: &[Value]) -> Vec<Vec<String>> {
    requests
        .iter()
        .filter(|request| {
            request["path"]
                .as_str()
                .unwrap()
                .ends_with("/chat/completions")
        })
        .map(|request| {
            request["body"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap().to_owned())
                .collect()
        })
        .collect()
}

/// A host provisioned by the owner setup entry with the caller's explicit
/// `trusted-task` Bash authority: any command the (scripted) model asks
/// `bash` for runs, each through this root's own Bash ingress, with no
/// permission request to the owner; every other native tool stays denied.
/// The turns are driven by a [`ScriptedModel`], not a model.
///
/// * open: two commands no list names (a pipeline, then a file read), in
///   one turn, each accepted by the ingress and returned to the turn;
/// * other tools: native `read`, `write`, `webfetch`, `task` and `glob`
///   calls do nothing observable (no file content returned, no file
///   written, no fetch reaches loopback), and a following `bash` call in
///   the same turn still runs;
/// * cancel while a `bash` command is running: the owner's cancel ends the
///   running work and the root, everything reaped;
/// * repeat: the same command in three consecutive steps runs each time.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_DEPS, OULIPOLY_AGENT_BASH_TOOL, AGENT_BASH_BIN and loopback"]
fn trusted_task_bash_runs_unlisted_commands_and_other_native_tools_stay_denied() {
    const PIPELINE: &str = "printf %s oulipoly-trusted-$((6*7)) | tr a-z A-Z";
    const MARKER: &str = "oulipoly-trusted-marker-7f3a";
    let model = ScriptedModel::start();
    let fixture = Fixture::new(Host::ScriptedBash {
        base_url: &model.base_url,
        bash: json!({ "bash_authority": "trusted-task" }),
    });
    let marker = fixture.project.join("marker.txt");
    std::fs::write(&marker, MARKER).unwrap();
    let written = fixture.project.join("written.txt");
    let fetched = format!("{}/oulipoly-webfetch-probe", model.base_url);

    // The receipt and the config file hold the same open-`bash` policy.
    let native = r#"{"*":"deny","bash":{"*":"allow"}}"#;
    let config =
        std::fs::read_to_string(fixture.dir.join("launch/xdg/config/opencode/opencode.json"))
            .unwrap();
    println!("launch config: {config}");
    assert!(
        config.ends_with(&format!(r#""permission":{native}}}"#)),
        "{config}"
    );
    assert_eq!(
        fixture.receipt.as_ref().unwrap()["policy"],
        json!({ "bash": "trusted-task", "other": "deny", "native": native }),
    );

    // open
    let cat = format!("cat {}", marker.display());
    let calls = json!([["bash", { "command": PIPELINE }], ["bash", { "command": cat }]]);
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-open", None, &format!("CALLS {calls}")),
    );
    run.event("endpoint-connected");
    let open_session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(run.event("ack")["label"], "accepted");
    let idle = run.event("idle");
    assert_eq!(idle["stop_reason"], "end_turn", "{idle}");
    assert_eq!(run.event("agent-message")["text"], "DONE");
    let accepted: Vec<Value> = run
        .seen
        .iter()
        .filter(|value| value["event"] == "bash-accepted")
        .map(|value| value["argv"].clone())
        .collect();
    assert_eq!(
        accepted,
        vec![
            json!(["bash", "-lc", PIPELINE]),
            json!(["bash", "-lc", cat])
        ],
        "{:#?}",
        run.seen
    );
    assert!(
        !run.seen
            .iter()
            .any(|value| value["event"] == "request-refused"),
        "{:#?}",
        run.seen
    );
    let (terminal, code) = run.cancel();
    println!("open terminal: {terminal}");
    assert_cancelled_and_reaped(&terminal, code);
    assert_eq!(terminal["bash"]["accepted"], 2, "{terminal}");
    let open = fixture.tool_parts(&open_session);
    println!("open: {open:?}");
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(
        open.iter()
            .all(|(tool, status, _, _)| tool == "bash" && status == "completed"),
        "{open:?}"
    );
    assert!(open[0].3.ends_with("OULIPOLY-TRUSTED-42"), "{open:?}");
    // A file's content returned through `bash` is visible to this check.
    assert!(open[1].3.ends_with(MARKER), "{open:?}");

    // other tools
    let calls = json!([
        ["read", { "filePath": marker }],
        ["write", { "filePath": written, "content": "written" }],
        ["webfetch", { "url": fetched, "format": "text" }],
        ["task", { "description": "probe", "prompt": "RUN true", "subagent_type": "general" }],
        ["glob", { "pattern": "*.txt", "path": fixture.project }],
        ["bash", { "command": "printf oulipoly-after-denied" }],
    ]);
    let requests_before = model.requests().len();
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-other", None, &format!("CALLS {calls}")),
    );
    run.event("endpoint-connected");
    let other_session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(run.event("ack")["label"], "accepted");
    let idle = run.event("idle");
    println!("other: {idle}\nseen: {:#?}", run.seen);
    let accepted: Vec<Value> = run
        .seen
        .iter()
        .filter(|value| value["event"] == "bash-accepted")
        .map(|value| value["argv"].clone())
        .collect();
    assert!(
        !run.seen
            .iter()
            .any(|value| value["event"] == "request-refused"),
        "{:#?}",
        run.seen
    );
    let (terminal, code) = run.cancel();
    println!("other terminal: {terminal}");
    assert_cancelled_and_reaped(&terminal, code);
    let other = fixture.tool_parts(&other_session);
    println!("other: {other:#?}");
    let requests = model.requests();
    println!(
        "paths: {:?}",
        requests
            .iter()
            .map(|request| &request["path"])
            .collect::<Vec<_>>()
    );
    assert!(!written.exists(), "native write created {written:?}");
    assert!(
        !requests
            .iter()
            .any(|request| request["path"] == "/v1/oulipoly-webfetch-probe"),
        "native webfetch reached loopback"
    );
    // Denied tools are not offered; a call to one ends as an unavailable
    // tool, nothing run.
    assert_eq!(other.len(), 6, "{other:#?}");
    for (tool, status, _, result) in &other[..5] {
        assert_eq!(status, "error", "{tool}: {result}");
        assert!(result.contains("unavailable tool"), "{tool}: {result}");
        assert!(!result.contains(MARKER), "{tool} returned the marker");
    }
    for request in &requests[requests_before..] {
        assert!(
            !request["body"].to_string().contains(MARKER)
                || request["body"]
                    .to_string()
                    .contains("oulipoly-after-denied"),
            "a denied tool returned the marker to the model: {request}"
        );
    }
    assert_eq!(
        accepted,
        vec![json!(["bash", "-lc", "printf oulipoly-after-denied"])],
        "{other:#?}"
    );
    assert_eq!(idle["stop_reason"], "end_turn", "{idle}");

    // cancel while running
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-cancel", None, "RUN sleep 600"),
    );
    run.event("endpoint-connected");
    assert_eq!(run.event("ack")["label"], "accepted");
    let accepted = run.event("bash-accepted");
    assert_eq!(accepted["argv"], json!(["bash", "-lc", "sleep 600"]));
    let (terminal, code) = run.cancel();
    println!("cancel terminal: {terminal}");
    assert_cancelled_and_reaped(&terminal, code);
    assert_eq!(
        terminal["bash"],
        json!({ "accepted": 1, "ended": 1, "end_unknown": 0, "not_run": 0, "open": 0, "refused": 0 }),
        "{terminal}"
    );

    // repeat: the same command three times in one turn
    let same = json!(["bash", { "command": "printf oulipoly-same" }]);
    let calls = json!([same, same, same]);
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root-same", None, &format!("CALLS {calls}")),
    );
    run.event("endpoint-connected");
    let same_session = run.event("session-opened")["session"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(run.event("ack")["label"], "accepted");
    let idle = run.event("idle");
    println!("same: {idle}\nseen: {:#?}", run.seen);
    let (terminal, code) = run.cancel();
    println!("same terminal: {terminal}");
    assert_cancelled_and_reaped(&terminal, code);
    let same = fixture.tool_parts(&same_session);
    println!("same: {same:#?}");
    // No native repeat guard stopped them (one call per model step).
    assert_eq!(same.len(), 3, "{same:#?}");
    assert!(
        same.iter().all(
            |(_, status, _, result)| status == "completed" && result.ends_with("oulipoly-same")
        ),
        "{same:#?}"
    );
    assert_eq!(terminal["bash"]["accepted"], 3, "{terminal}");
    assert_eq!(terminal["bash"]["ended"], 3, "{terminal}");

    // Only `bash` was ever offered.
    for tools in offered_tools(&model.requests()) {
        assert_eq!(tools, vec!["bash".to_owned()]);
    }
}
