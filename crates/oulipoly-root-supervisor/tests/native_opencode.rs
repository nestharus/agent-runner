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
//! The only timer is a test watchdog; on expiry the test cancels its
//! supervisor and fails.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_oulipoly-root-supervisor");
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

struct Fixture {
    dir: PathBuf,
    opencode: PathBuf,
    project: PathBuf,
    env: Vec<(String, String)>,
}

impl Fixture {
    fn new(model: Option<&Model>) -> Self {
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
        let dir = base.join(format!("native-{}", std::process::id()));
        let mkdir = |path: &Path| {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(path)
                .unwrap();
        };
        mkdir(&dir);
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
        match model {
            None => {
                env.push(("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1".to_owned()));
                env.push(("OULIPOLY_ACP_V2_NO_REPLY", "1".to_owned()));
            }
            Some(model) => {
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
        }
        env.push(("OPENCODE_CONFIG_CONTENT", config.to_string()));
        let env = env
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
        Self {
            dir,
            opencode,
            project,
            env,
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
                "harnesses": [{
                    "id": "opencode",
                    // `serve` loads a directory's instance, and so its
                    // plugins, only per HTTP request; `acp` loads its cwd's
                    // at startup. Its own stdio ACP surface is never spoken
                    // to: the owner talks only to the endpoint's socket.
                    "argv": [self.opencode, "acp", "--hostname", "127.0.0.1", "--port", "0"],
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
            .envs(self.env.iter().map(|(key, value)| (key, value)))
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
    let fixture = Fixture::new(None);
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
    let fixture = Fixture::new(Some(&model));

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
