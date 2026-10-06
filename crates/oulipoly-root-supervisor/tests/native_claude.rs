//! The real supervisor owning native Claude harnesses provisioned by
//! [`provision_claude`]: the shipped ACP v2 receiver
//! (`native/claude/acp-v2-receiver.mjs`) on the published Agent SDK and
//! ACP SDK, with the stand-in `tests/fixtures/fake-claude.mjs` installed
//! where the SDK's platform package keeps the Claude Code executable. The
//! stand-in is not Claude Code; it speaks the stream-json control protocol
//! subset the SDK drives, so these tests check the receiver, the SDK
//! wiring and the owner contract, not Claude Code, a login or a model.
//!
//! Ignored by default. Needs `OULIPOLY_NATIVE_CLAUDE_DEPS` (`npm ci
//! --ignore-scripts` of `native/claude/package.json` beside its lockfile),
//! `OULIPOLY_NATIVE_NODE` (the Node runtime) and `AGENT_BASH_BIN` (an
//! agent-bash binary with root v1). Nothing here needs the network; run it
//! inside `unshare --user --map-current-user --net --mount --pid --fork
//! --mount-proc` so the fixture has none and everything ends with it. The
//! private `/proc` is needed: the owner reads its work's PID namespace
//! there; with the host's `/proc` launches fail or Bash is refused as
//! unattributed.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use oulipoly_root_supervisor::native::BashAuthority;
use oulipoly_root_supervisor::native_claude::{
    CLAUDE_EXECUTABLE, ClaudeSetup, Effort, provision_claude,
};
use serde_json::{Value, json};

const SUPERVISOR: &str = env!("CARGO_BIN_EXE_oulipoly-root-supervisor");
const WATCHDOG: Duration = Duration::from_secs(90);
const MODEL: &str = "claude-opus-5-5";

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set")))
}

struct Fixture {
    dir: PathBuf,
    project: PathBuf,
    store_dir: PathBuf,
    record: PathBuf,
    argv: Vec<String>,
    receipt: Value,
    env: Vec<(String, String)>,
}

impl Fixture {
    /// One provisioned harness; `bash` is `Some(list)` for an allow list,
    /// `None` for `trusted-task`.
    fn new(scenario: &str, bash: Option<&[&str]>, ack_timeout_s: Option<u32>) -> Self {
        let real = env_path("OULIPOLY_NATIVE_CLAUDE_DEPS");
        let node = env_path("OULIPOLY_NATIVE_NODE");
        let agent_bash = env_path("AGENT_BASH_BIN");
        let base = std::env::var("OULIPOLY_NATIVE_SCRATCH")
            .map_or_else(|_| std::env::temp_dir(), PathBuf::from);
        static FIXTURES: AtomicUsize = AtomicUsize::new(0);
        // A pid alone repeats across fresh PID namespaces.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = base.join(format!(
            "native-claude-{stamp}-{}-{}",
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
        mkdir(&dir);
        let deps = overlay(&real, &dir.join("deps"), &node);
        let project = dir.join("project");
        mkdir(&project);
        let home = dir.join("home");
        mkdir(&home);
        // The store is only named; the stand-in never writes it either.
        let store_dir = dir.join("claude-store");
        let setup = ClaudeSetup {
            dir: dir.join("launch").to_str().unwrap().to_owned(),
            deps: deps.to_str().unwrap().to_owned(),
            node: node.to_str().unwrap().to_owned(),
            agent_bash_bin: agent_bash.to_str().unwrap().to_owned(),
            bash_allow: bash
                .map(|list| list.iter().map(|s| (*s).to_owned()).collect())
                .unwrap_or_default(),
            bash_authority: if bash.is_none() {
                Some(BashAuthority::TrustedTask)
            } else {
                None
            },
            model: MODEL.to_owned(),
            effort: Effort::High,
            config_dir: store_dir.to_str().unwrap().to_owned(),
            start_timeout_s: Some(30),
            ack_timeout_s,
            explore: None,
        };
        let launch = provision_claude(&setup, None).unwrap();
        let record = dir.join("record.jsonl");
        let env = vec![
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("HOME".to_owned(), home.to_str().unwrap().to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("FAKE_CLAUDE_SCENARIO".to_owned(), scenario.to_owned()),
            (
                "FAKE_CLAUDE_RECORD".to_owned(),
                record.to_str().unwrap().to_owned(),
            ),
            // The stand-in's own session record (not a Claude store).
            (
                "FAKE_CLAUDE_SESSIONS".to_owned(),
                dir.join("fake-sessions.jsonl").to_str().unwrap().to_owned(),
            ),
            // Ambient credentials and redirects that must not reach Claude Code.
            (
                "ANTHROPIC_API_KEY".to_owned(),
                "fixture-not-a-key".to_owned(),
            ),
            (
                "ANTHROPIC_BASE_URL".to_owned(),
                "http://127.0.0.1:9".to_owned(),
            ),
            (
                "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
                "fixture-not-a-token".to_owned(),
            ),
            ("CLAUDE_CONFIG_DIR".to_owned(), "/wrong-store".to_owned()),
        ];
        Self {
            dir,
            project,
            store_dir,
            record,
            argv: launch.argv.clone(),
            receipt: launch.to_json(),
            env,
        }
    }

    fn spec(&self, store: &str, message: &str) -> Value {
        self.spec_with_caps(store, message, 1)
    }

    /// The owner's existing relaunch-after-observed-exit path needs caps
    /// above 1 to relaunch at all; this fixture setting does not change the
    /// installed front door's caps.
    fn spec_with_caps(&self, store: &str, message: &str, caps: u32) -> Value {
        json!({
            "store": self.dir.join(store),
            "intent": {
                "outage_closure_cap": caps,
                "delivery_attempt_cap": caps,
                "cwd": self.project,
                "workload": { "isolation": "unprivileged-userns" },
                "harnesses": [{
                    "id": "claude",
                    "argv": self.argv,
                    "endpoint": "stdio",
                    "messages": [message],
                }],
            },
        })
    }

    fn records(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.record)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn launch_record(&self) -> Value {
        self.records()
            .into_iter()
            .find(|value| value.get("argv").is_some())
            .expect("stand-in started")
    }

    /// Every stand-in start's argv, in order.
    fn launches(&self) -> Vec<Vec<String>> {
        self.records()
            .iter()
            .filter_map(|value| value.get("argv"))
            .map(|argv| {
                argv.as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap().to_owned())
                    .collect()
            })
            .collect()
    }
}

/// The value of an SDK `--name=value` argument, if present.
fn eq_flag<'a>(argv: &'a [String], name: &str) -> Option<&'a str> {
    let prefix = format!("{name}=");
    argv.iter().find_map(|arg| arg.strip_prefix(&prefix))
}

/// `real`'s `node_modules` seen through symlinks, except the SDK's Linux
/// x64 platform package, which holds the stand-in executable.
fn overlay(real: &Path, deps: &Path, node: &Path) -> PathBuf {
    let modules = deps.join("node_modules");
    let scoped = modules.join("@anthropic-ai");
    std::fs::create_dir_all(scoped.join("claude-agent-sdk-linux-x64")).unwrap();
    std::fs::copy(
        real.join("package-lock.json"),
        deps.join("package-lock.json"),
    )
    .unwrap();
    for entry in std::fs::read_dir(real.join("node_modules")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != "@anthropic-ai" {
            std::os::unix::fs::symlink(entry.path(), modules.join(entry.file_name())).unwrap();
        }
    }
    for entry in std::fs::read_dir(real.join("node_modules/@anthropic-ai")).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() != "claude-agent-sdk-linux-x64" {
            std::os::unix::fs::symlink(entry.path(), scoped.join(entry.file_name())).unwrap();
        }
    }
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake-claude.mjs"),
    )
    .unwrap();
    let executable = deps.join(CLAUDE_EXECUTABLE);
    std::fs::write(&executable, format!("#!{}\n{source}", node.display())).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    deps.to_path_buf()
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
            if what != "terminal" {
                assert_ne!(value["event"], "terminal", "terminal before {what}");
            }
        }
    }

    fn event(&mut self, event: &str) -> Value {
        self.until(event, |value| value["event"] == event)
    }

    fn control(&mut self, control: Value) {
        writeln!(self.stdin, "{control}").unwrap();
    }

    /// The owner's own end, without any control from this test.
    fn end(mut self) -> (Value, Option<i32>, Vec<Value>) {
        let terminal = self.event("terminal");
        let status = self.child.wait().unwrap();
        let seen = std::mem::take(&mut self.seen);
        (terminal, status.code(), seen)
    }

    fn notices(&self) -> Vec<(String, String)> {
        self.seen
            .iter()
            .filter(|value| value["event"] == "notice")
            .map(|value| {
                (
                    value["severity"].as_str().unwrap_or_default().to_owned(),
                    value["title"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = writeln!(self.stdin, "{}", json!({ "cmd": "cancel" }));
            let _ = self.child.wait();
        }
    }
}

fn has(values: &[Value], event: &str) -> bool {
    values.iter().any(|value| value["event"] == event)
}

/// A trusted-task harness: one input's turn runs a command through the
/// root's own Bash ingress, its linked answer and tagged turn end reach the
/// owner, a follow-up gets a later ordered id and its own linkage, and
/// close stops the harness after both turns ended. The stand-in records
/// the constructed Claude Code launch.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn trusted_task_turn_runs_attributed_bash_and_links_answers_to_inputs() {
    const COMMAND: &str = "printf oulipoly-claude-bash";
    let fixture = Fixture::new("normal", None, None);
    println!("receipt: {}", fixture.receipt);
    let mut run = Run::start(&fixture, &fixture.spec("root", &format!("RUN {COMMAND}")));
    let opened = run.event("session-opened");
    let ack = run.event("ack");
    assert_eq!(ack["label"], "accepted", "{ack}");
    let first = ack["message_id"].as_str().unwrap().to_owned();
    let accepted = run.event("bash-accepted");
    assert_eq!(
        accepted["argv"],
        json!(["bash", "-lc", COMMAND]),
        "{accepted}"
    );
    let answer = run.until("linked answer", |value| {
        value["event"] == "agent-message"
            && value["text"].as_str().unwrap_or("").starts_with("DONE")
    });
    assert_eq!(answer["input"], 0, "{answer}");
    assert_eq!(answer["parent_message_id"], first.as_str(), "{answer}");
    assert_eq!(answer["input_attribution"], "native-parent", "{answer}");
    assert_eq!(
        answer["text"],
        "DONE Root v1 work ended: exited with code 0 (code:0, observer work-pid1-wait); output complete (full stream counted, closed, matched by the end).",
        "{answer}"
    );
    let end = run.until("turn end 0", |value| {
        value["event"] == "turn-end" && value["input"] == 0
    });
    assert_eq!(end["stop_reason"], "end_turn", "{end}");
    assert_eq!(end["own_output"], true, "{end}");

    run.control(json!({ "cmd": "send", "text": "second input", "ref": "f1" }));
    run.event("follow-up-admitted");
    let ack = run.until("ack 1", |value| {
        value["event"] == "ack" && value["index"] == 1
    });
    let second = ack["message_id"].as_str().unwrap().to_owned();
    assert!(second > first, "{first} < {second}");
    let answer = run.until("answer 1", |value| {
        value["event"] == "agent-message"
            && value["input"] == 1
            && value["text"] == "ECHO second input"
    });
    assert_eq!(answer["parent_message_id"], second.as_str());
    let end = run.until("turn end 1", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    assert_eq!(end["stop_reason"], "end_turn", "{end}");
    run.control(json!({ "cmd": "close" }));
    let (terminal, code, seen) = run.end();
    println!("terminal: {terminal}");
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(code, Some(7), "{terminal}");
    assert_eq!(terminal["bash"]["accepted"], 1, "{terminal}");
    assert!(!has(&seen, "request-refused"), "{seen:#?}");

    // The constructed launch, as the stand-in saw it.
    let launch = fixture.launch_record();
    println!("launch: {launch}");
    let argv: Vec<&str> = launch["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    let after = |flag: &str| {
        argv.iter()
            .position(|arg| *arg == flag)
            .map(|at| argv[at + 1])
            .unwrap_or_else(|| panic!("no {flag} in {argv:?}"))
    };
    assert_eq!(after("--model"), MODEL);
    assert_eq!(after("--effort"), "high");
    assert_eq!(after("--permission-mode"), "dontAsk");
    assert_eq!(after("--tools"), "Read,Write,Edit");
    assert_eq!(
        after("--allowedTools"),
        "Read,Write,Edit,mcp__oulipoly__bash"
    );
    let denied = after("--disallowedTools");
    for name in ["Bash", "Agent", "Task", "Monitor", "WebFetch"] {
        assert!(denied.split(',').any(|tool| tool == name), "{denied}");
    }
    for flag in [
        "--setting-sources=",
        "--strict-mcp-config",
        "--replay-user-messages",
        "--input-format",
    ] {
        assert!(argv.contains(&flag), "{flag}: {argv:?}");
    }
    // A fresh session: Claude Code is asked for a new session under exactly
    // the id the owner recorded, never to resume or continue one.
    let launches = fixture.launches();
    assert_eq!(launches.len(), 1, "{launches:?}");
    assert_eq!(
        eq_flag(&launches[0], "--session-id"),
        opened["session"].as_str(),
        "{opened} {argv:?}"
    );
    assert_eq!(eq_flag(&launches[0], "--resume"), None, "{argv:?}");
    for flag in ["--continue", "--fork-session", "--no-session-persistence"] {
        assert!(!argv.contains(&flag), "{flag}: {argv:?}");
    }
    // No external MCP configuration: the one server is the receiver's own,
    // named in the SDK's initialize request; no hooks are registered.
    assert!(!argv.contains(&"--mcp-config"), "{argv:?}");
    let initialize = fixture
        .records()
        .into_iter()
        .find_map(|value| value.get("initialize").cloned())
        .expect("initialize recorded");
    assert_eq!(
        initialize["sdkMcpServers"],
        json!(["oulipoly"]),
        "{initialize}"
    );
    assert!(
        initialize["hooks"].is_null() || initialize["hooks"] == json!({}),
        "{initialize}"
    );
    let env = &launch["env"];
    assert_eq!(
        env["CLAUDE_CONFIG_DIR"],
        fixture.store_dir.to_str().unwrap()
    );
    assert_eq!(env["ENABLE_TOOL_SEARCH"], "false");
    assert_eq!(env["DISABLE_AUTOUPDATER"], "1");
    for name in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OULIPOLY_ROOT_BASH_V1",
        "NODE_OPTIONS",
    ] {
        assert!(env.get(name).is_none(), "{name} reached Claude Code: {env}");
    }
    assert_eq!(launch["cwd"], fixture.project.to_str().unwrap());
    let records = fixture.records();
    let listed = records
        .iter()
        .find_map(|value| value.get("tools_list"))
        .unwrap();
    let tools = listed["mcp_response"]["result"]["tools"]
        .as_array()
        .unwrap();
    assert_eq!(tools.len(), 1, "{listed}");
    assert_eq!(tools[0]["name"], "bash");
    assert_eq!(tools[0]["_meta"]["anthropic/alwaysLoad"], true, "{listed}");
    let users: Vec<&Value> = records
        .iter()
        .filter_map(|value| value.get("user"))
        .collect();
    assert_eq!(users.len(), 2, "{records:?}");
    assert!(
        users.iter().all(|user| user["client_composed"] == true),
        "{users:?}"
    );
    // Claude Code's store was only named: neither setup nor the stand-in
    // made it.
    assert!(!fixture.store_dir.exists());
}

/// The real receiver + published SDK's MCP Bash contract, a stand-in CLI,
/// and actual owner/namespace custody. Acquires all bytes past the inline
/// bound and explicitly accepts the exact identity twice without new runs.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn retained_output_is_acquired_and_locally_accepted_through_the_supported_mcp_tool() {
    const COMMAND: &str = "python3 -c 'import sys; sys.stdout.buffer.write(bytes((i * 37) % 256 for i in range(100000)))'; exit 7";
    let fixture = Fixture::new("normal", None, None);
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root", &format!("RETAIN {COMMAND}")),
    );
    run.event("ack");
    let answer = run.until("retained answer", |v| {
        v["event"] == "agent-message"
            && v["text"]
                .as_str()
                .unwrap_or("")
                .starts_with("DONE RETAINED")
    });
    let ended = run.until("turn end", |v| v["event"] == "turn-end");
    assert_eq!(ended["stop_reason"], "end_turn");
    run.control(json!({ "cmd": "close" }));
    let (terminal, code, seen) = run.end();
    assert_eq!(code, Some(7));
    assert_eq!(terminal["bash"]["accepted"], 1);
    assert_eq!(terminal["bash"]["ended"], 1);
    assert_eq!(terminal["bash"]["open"], 0);
    assert_eq!(terminal["bash"]["output_open"], 0);
    assert_eq!(terminal["bash"]["output_requests"], 101); // 98 pages + wrong/first/repeat
    let bash_end = seen.iter().find(|v| v["event"] == "bash-ended").unwrap();
    assert_eq!(bash_end["status"], "code:7");
    assert_eq!(bash_end["output"]["state"], "closed");
    assert_eq!(bash_end["retained"]["state"], "complete");
    assert_eq!(bash_end["retained"]["bytes"], 100000);
    assert_eq!(
        std::fs::metadata(fixture.dir.join("root/output"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(fixture.dir.join(format!(
            "root/output/{}",
            bash_end["work"].as_i64().unwrap()
        )))
        .unwrap()
        .permissions()
        .mode()
            & 0o777,
        0o600
    );
    let records = fixture.records();
    let acquired = records
        .iter()
        .find(|v| v.get("acquired_b64").is_some())
        .unwrap();
    let expected: Vec<u8> = (0..100000).map(|i| ((i * 37) % 256) as u8).collect();
    use sha2::Digest;
    assert_eq!(acquired["identity"], bash_end["retained"]["identity"]);
    assert_eq!(acquired["pages"], 98);
    // Exact byte oracle uses Base64 generated independently by the fixture
    // requester helper, not a mere count/hash/definition check.
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::new();
    for chunk in expected.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - i * 8));
        for i in 0..4 {
            b64.push(if i <= chunk.len() {
                alphabet[((n >> (18 - i * 6)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    assert_eq!(acquired["acquired_b64"], b64);
    assert_eq!(
        bash_end["retained"]["sha256"],
        format!("{:x}", sha2::Sha256::digest(&expected))
    );
    let receipts: Vec<_> = seen
        .iter()
        .filter(|v| v["event"] == "bash-output-accepted")
        .collect();
    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0]["repeat"], false);
    assert_eq!(receipts[1]["repeat"], true);
    assert_eq!(receipts[0]["receipt"], receipts[1]["receipt"]);
    assert_eq!(receipts[0]["receipt"]["work"], bash_end["work"]);
    assert_eq!(receipts[0]["receipt"]["bytes"], 100000);
    assert!(
        answer["text"]
            .as_str()
            .unwrap()
            .contains("bytes=100000 pages=98")
    );
    let conn = rusqlite::Connection::open(fixture.dir.join("root/intent.sqlite3")).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM bash_output_accept", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    if let Ok(path) = std::env::var("OULIPOLY_NATIVE_OUTPUT_EVIDENCE") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({ "records": records, "events": seen, "terminal": terminal }),
            )
            .unwrap(),
        )
        .unwrap();
    }
    drop(conn);
    std::fs::remove_dir_all(&fixture.dir).unwrap();
}

/// Background Bash through the supported MCP tool. The command waits on a
/// gate this test creates only after the first turn has ended, so its end
/// cannot precede the tool's answer or that turn's end. `close` is sent at
/// that turn end (as the public caller does); the harness must stay live
/// until the owner's completion input is acknowledged and its tagged turn
/// ends. In that later turn the stand-in reads every retained byte the
/// completion names and accepts it; nothing polls before it.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn background_bash_completion_reaches_the_live_session_as_a_later_turn() {
    let fixture = Fixture::new("normal", None, None);
    let gate = fixture.dir.join("gate");
    let command = format!(
        "while [ ! -e {} ]; do sleep 0.05; done; python3 -c 'import sys; sys.stdout.write(\"\".join(chr(97 + i % 26) for i in range(3000)))'; exit 7",
        gate.display()
    );
    let mut run = Run::start(
        &fixture,
        &fixture.spec("root", &format!("BACKGROUND {command}")),
    );
    let started = run.until("started answer", |v| {
        v["event"] == "agent-message" && v["text"].as_str().unwrap_or("").starts_with("STARTED ")
    });
    assert!(
        started["text"]
            .as_str()
            .unwrap()
            .contains("background work accepted and started (reference=rv1w:"),
        "{started}"
    );
    let first = run.until("first turn end", |v| {
        v["event"] == "turn-end" && v["input"] == 0
    });
    assert_eq!(first["stop_reason"], "end_turn");
    assert!(!gate.exists());
    assert!(
        !has(&run.seen, "bash-ended"),
        "the command cannot have ended before its gate"
    );
    run.control(json!({ "cmd": "close" }));
    run.event("close-requested");
    std::fs::write(&gate, b"").unwrap();
    let (terminal, code, seen) = run.end();
    let at =
        |pred: &dyn Fn(&Value) -> bool| seen.iter().position(|v| pred(v)).expect("event present");
    let bash_end = seen
        .iter()
        .find(|v| v["event"] == "bash-ended")
        .unwrap()
        .clone();
    let work = bash_end["work"].as_i64().unwrap();
    assert_eq!(bash_end["status"], "code:7");
    assert_eq!(bash_end["observer"], "work-pid1-wait");
    assert_eq!(bash_end["requester"], "detached-async");
    assert_eq!(bash_end["retained"]["state"], "complete");
    assert_eq!(bash_end["retained"]["bytes"], 3000);
    let identity = bash_end["retained"]["identity"]
        .as_str()
        .unwrap()
        .to_owned();
    let admitted = at(&|v| v["event"] == "bash-async-completion-admitted" && v["work"] == work);
    assert_eq!(seen[admitted]["input"], 1);
    let ack = at(&|v| v["event"] == "ack" && v["index"] == 1);
    let answer = at(&|v| v["event"] == "agent-message" && v["input"] == 1);
    let completed = seen[answer]["text"].as_str().unwrap();
    assert_eq!(
        completed,
        format!("COMPLETED work={work} status=code:7 bytes=3000 pages=3 accepted={identity}")
    );
    let second = at(&|v| v["event"] == "turn-end" && v["input"] == 1);
    let settled =
        at(&|v| v["event"] == "async-owed" && v["change"] == "turn-ended" && v["work"] == work);
    assert_eq!(seen[settled]["owed_async"], 0);
    let close = at(&|v| v["event"] == "close-requested");
    let stopping = at(&|v| v["event"] == "close-stopping");
    let ended = at(&|v| v["event"] == "bash-ended");
    let accepted = at(&|v| v["event"] == "bash-output-accepted");
    assert!(
        close < ended
            && ended < admitted
            && admitted < ack
            && ack < accepted
            && accepted < second
            && second <= settled
            && settled < stopping,
        "order: close {close} ended {ended} admitted {admitted} ack {ack} accepted {accepted} turn {second} settled {settled} stop {stopping}"
    );
    assert_eq!(code, Some(7));
    assert_eq!(terminal["status"], "closed");
    assert_eq!(terminal["async"]["accepted"], 1);
    assert_eq!(terminal["async"]["turn_ended"], 1);
    assert_eq!(terminal["async"]["undelivered"], json!([]));
    assert_eq!(terminal["async"]["owed"], 0);
    assert_eq!(
        terminal["bash"]["output_requests"], 4,
        "3 pages + 1 acceptance, all after delivery"
    );
    let records = fixture.records();
    let acquired = records
        .iter()
        .find(|v| v.get("completion_facts").is_some())
        .unwrap();
    let expected: String = (0..3000)
        .map(|i| char::from(b'a' + (i % 26) as u8))
        .collect();
    use std::io::Read as _;
    let mut decoded = Vec::new();
    std::process::Command::new("base64")
        .arg("-d")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(acquired["acquired_b64"].as_str().unwrap().as_bytes())
                .unwrap();
            child
                .stdout
                .take()
                .unwrap()
                .read_to_end(&mut decoded)
                .unwrap();
            child.wait().unwrap()
        })
        .unwrap();
    assert_eq!(String::from_utf8(decoded).unwrap(), expected);
    assert_eq!(acquired["completion_facts"]["accepted_locally"], false);
    assert_eq!(acquired["completion_facts"]["end"]["status"], "code:7");
    if let Ok(path) = std::env::var("OULIPOLY_NATIVE_ASYNC_EVIDENCE") {
        std::fs::write(
            path,
            serde_json::to_vec_pretty(
                &json!({ "records": records, "events": seen, "terminal": terminal }),
            )
            .unwrap(),
        )
        .unwrap();
    }
    std::fs::remove_dir_all(&fixture.dir).unwrap();
}

/// Cancel while a background completion is owed: the run is killed, its
/// true wait is a signal, and its completion is settled undelivered with
/// the cancel as reason. Nothing claims a delivered or processed turn.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn cancel_with_an_owed_background_completion_settles_it_undelivered() {
    let fixture = Fixture::new("normal", None, None);
    let mut run = Run::start(&fixture, &fixture.spec("root", "BACKGROUND sleep 600"));
    run.until("first turn end", |v| {
        v["event"] == "turn-end" && v["input"] == 0
    });
    run.control(json!({ "cmd": "close" }));
    run.event("close-requested");
    run.control(json!({ "cmd": "cancel" }));
    let (terminal, _code, seen) = run.end();
    let bash_end = seen.iter().find(|v| v["event"] == "bash-ended").unwrap();
    let work = bash_end["work"].clone();
    assert!(
        bash_end["status"].as_str().unwrap().starts_with("signal:"),
        "{bash_end}"
    );
    assert_eq!(terminal["status"], "cancelled");
    assert_eq!(terminal["async"]["accepted"], 1);
    assert_eq!(terminal["async"]["turn_ended"], 0);
    assert_eq!(
        terminal["async"]["undelivered"],
        json!([{ "harness": "claude", "work": work, "reason": "cancelled" }])
    );
    assert!(!has(&seen, "bash-async-completion-admitted"));
    assert!(!seen.iter().any(|v| v["event"] == "ack" && v["index"] == 1));
    std::fs::remove_dir_all(&fixture.dir).unwrap();
}

/// An allow list: a command it does not name is refused by the tool and
/// never reaches the root's Bash ingress; the turn still ends normally.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn allow_list_refuses_an_unnamed_command_without_running_it() {
    let fixture = Fixture::new("normal", Some(&["printf named"]), None);
    let mut run = Run::start(&fixture, &fixture.spec("root", "RUN printf unnamed"));
    run.event("ack");
    let answer = run.until("answer", |value| {
        value["event"] == "agent-message" && value["input"] == 0
    });
    assert!(
        answer["text"]
            .as_str()
            .unwrap()
            .starts_with("DONE Denied by this root's bash policy"),
        "{answer}"
    );
    let end = run.until("turn end", |value| value["event"] == "turn-end");
    assert_eq!(end["stop_reason"], "end_turn");
    run.control(json!({ "cmd": "close" }));
    let (terminal, _, seen) = run.end();
    assert_eq!(terminal["bash"]["accepted"], 0, "{terminal}");
    assert!(!has(&seen, "bash-accepted"), "{seen:#?}");
    let launch = fixture.launch_record();
    let argv: Vec<&str> = launch["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    let tools = argv.iter().position(|arg| *arg == "--tools").unwrap();
    assert_eq!(argv[tools + 1], "", "{argv:?}");
}

/// No consumption echo within the bound: no ACK, a visible error notice,
/// and the harness ends by itself; the owner ends without a cancel, long
/// before any caller deadline, and nothing is replayed.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn missing_consumption_echo_is_a_bounded_visible_end() {
    let fixture = Fixture::new("no-echo", None, Some(2));
    let run = Run::start(&fixture, &fixture.spec("root", "hello"));
    let started = std::time::Instant::now();
    let (terminal, code, seen) = run.end();
    println!("terminal ({:?}): {terminal}", started.elapsed());
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(!has(&seen, "ack"), "{seen:#?}");
    assert!(!has(&seen, "relaunch"), "{seen:#?}");
    assert!(code.is_some(), "{terminal}");
    let users = fixture
        .records()
        .iter()
        .filter(|value| value.get("user").is_some())
        .count();
    assert_eq!(users, 1, "sent once, never replayed");
    let notices: Vec<&Value> = seen
        .iter()
        .filter(|value| value["event"] == "notice")
        .collect();
    assert!(
        notices
            .iter()
            .any(|value| value["title"] == "claude receiver ending"
                && value["description"]
                    .as_str()
                    .unwrap_or("")
                    .contains("no consumption echo")),
        "{seen:#?}"
    );
}

/// Claude Code ends mid-turn: the acknowledged input's turn ends visibly
/// as `_claude_exited` and the harness ends by itself.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn claude_code_exit_mid_turn_ends_the_input_visibly() {
    let fixture = Fixture::new("crash", None, None);
    let mut run = Run::start(&fixture, &fixture.spec("root", "hello"));
    run.event("ack");
    let end = run.until("turn end", |value| value["event"] == "turn-end");
    assert_eq!(end["stop_reason"], "_claude_exited", "{end}");
    let (terminal, code, _) = run.end();
    println!("terminal: {terminal}");
    assert!(code.is_some(), "{terminal}");
}

/// Error, unattributed, model and permission outcomes reach the owner as
/// notices and stop reasons, each turn ending without a hang.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn error_unattributed_model_and_denial_outcomes_are_visible() {
    for (scenario, stop, severity, title) in [
        (
            "error-result",
            "_claude_error",
            "error",
            "claude assistant error: model_not_found",
        ),
        (
            "unattributed",
            "_claude_unattributed",
            "error",
            "claude result answers no acknowledged input",
        ),
        (
            "model-mismatch",
            "end_turn",
            "warning",
            "claude model differs",
        ),
        ("denial", "end_turn", "warning", "claude permission denials"),
        (
            "sessionless-attribution",
            "end_turn",
            "warning",
            "claude consumption echo names no session",
        ),
    ] {
        let fixture = Fixture::new(scenario, None, None);
        let mut run = Run::start(&fixture, &fixture.spec("root", "hello"));
        run.event("ack");
        let end = run.until("turn end", |value| value["event"] == "turn-end");
        assert_eq!(end["stop_reason"], stop, "{scenario}: {end}");
        run.control(json!({ "cmd": "close" }));
        let notices = run.notices();
        let (terminal, code, seen) = run.end();
        let notices: Vec<(String, String)> = seen
            .iter()
            .filter(|value| value["event"] == "notice")
            .map(|value| {
                (
                    value["severity"].as_str().unwrap_or_default().to_owned(),
                    value["title"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .chain(notices)
            .collect();
        assert!(
            notices.iter().any(|(s, t)| s == severity && t == title),
            "{scenario}: {notices:?}"
        );
        assert_eq!(terminal["status"], "closed", "{scenario}: {terminal}");
        assert_eq!(code, Some(7));
        if scenario == "model-mismatch" {
            // The answer is still linked: the warning does not refuse it.
            assert!(seen.iter().any(|value| value["event"] == "agent-message"
                && value["input"] == 0
                && value["text"] == "ECHO hello"));
        }
    }
}

/// Runs a session's first input to its turn end, then a follow-up at whose
/// receipt the stand-in stops the receiver (SIGKILL) before any echo, so
/// the follow-up is owed when the receiver stops. Returns the run, the
/// recorded session and the first input's ACK id.
fn first_turn_then_receiver_stop(fixture: &Fixture) -> (Run, String, String) {
    let mut run = Run::start(fixture, &fixture.spec_with_caps("root", "first input", 2));
    let opened = run.event("session-opened");
    let session = opened["session"].as_str().unwrap().to_owned();
    let ack = run.until("ack 0", |value| {
        value["event"] == "ack" && value["index"] == 0
    });
    let first = ack["message_id"].as_str().unwrap().to_owned();
    run.until("turn end 0", |value| {
        value["event"] == "turn-end" && value["input"] == 0
    });
    run.control(json!({ "cmd": "send", "text": "second input", "ref": "f1" }));
    run.event("follow-up-admitted");
    run.event("relaunch");
    (run, session, first)
}

/// After the receiver stops, a new receiver continues the same recorded
/// Claude Code session: Claude Code is asked to resume exactly that id
/// (no new id, no fork), the owner reports the same session resumed, and
/// only the follow-up's own consumption is acknowledged and linked. The
/// session's earlier history that Claude Code emits is neither forwarded
/// nor taken as the follow-up's ACK, answer or turn end.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn stopped_receiver_continues_the_same_recorded_session() {
    let fixture = Fixture::new("receiver-stops-on-second", None, None);
    let (mut run, session, first) = first_turn_then_receiver_stop(&fixture);
    let resumed = run.event("session-resumed");
    assert_eq!(resumed["session"], session.as_str(), "{resumed}");
    let ack = run.until("ack 1", |value| {
        value["event"] == "ack" && value["index"] == 1
    });
    // An earlier attempt reached a receiver that stopped without an ACK:
    // its insertion is unknown, so this ACK cannot claim at-most-once.
    assert_eq!(ack["label"], "duplicate-unknown", "{ack}");
    let second = ack["message_id"].as_str().unwrap().to_owned();
    assert!(second > first, "{first} < {second}");
    let answer = run.until("answer 1", |value| {
        value["event"] == "agent-message"
            && value["input"] == 1
            && value["text"] == "ECHO second input"
    });
    assert_eq!(answer["parent_message_id"], second.as_str(), "{answer}");
    let end = run.until("turn end 1", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    assert_eq!(end["stop_reason"], "end_turn", "{end}");
    run.control(json!({ "cmd": "close" }));
    let (terminal, code, seen) = run.end();
    println!("terminal: {terminal}");
    assert_eq!(terminal["status"], "closed", "{terminal}");
    assert_eq!(code, Some(7), "{terminal}");
    assert_eq!(
        seen.iter()
            .filter(|value| value["event"] == "session-opened")
            .count(),
        1,
        "no second, fresh session: {seen:#?}"
    );
    assert!(
        !seen.iter().any(|value| value["event"] == "agent-message"
            && value["text"].as_str().unwrap_or("").contains("PRIOR")),
        "earlier history forwarded: {seen:#?}"
    );
    let warned = |title: &str| {
        seen.iter().any(|value| {
            value["event"] == "notice" && value["severity"] == "warning" && value["title"] == title
        })
    };
    assert!(warned("claude assistant output not forwarded"), "{seen:#?}");
    assert!(warned("claude result not forwarded"), "{seen:#?}");
    assert_eq!(
        seen.iter()
            .filter(|value| value["event"] == "ack" && value["index"] == 0)
            .count(),
        1,
        "{seen:#?}"
    );

    let launches = fixture.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert_eq!(
        eq_flag(&launches[0], "--session-id"),
        Some(session.as_str())
    );
    assert_eq!(eq_flag(&launches[0], "--resume"), None);
    assert_eq!(eq_flag(&launches[1], "--resume"), Some(session.as_str()));
    assert_eq!(
        eq_flag(&launches[1], "--session-id"),
        None,
        "{:?}",
        launches[1]
    );
    for flag in ["--fork-session", "--continue", "--no-session-persistence"] {
        assert!(!launches[1].iter().any(|arg| arg == flag), "{flag}");
    }
    let records = fixture.records();
    let users: Vec<&Value> = records
        .iter()
        .filter_map(|value| value.get("user"))
        .collect();
    // Input 0 and the follow-up's stopped attempt, then its one resend.
    assert_eq!(users.len(), 3, "{users:?}");
    assert!(
        users
            .iter()
            .all(|user| user["session_id"] == session.as_str()),
        "{users:?}"
    );
    let history = records
        .iter()
        .find_map(|value| value.get("emitted_history_of"))
        .expect("the resumed stand-in emitted earlier history");
    assert_eq!(history, &users[0]["uuid"], "{records:?}");
}

/// A recorded session Claude Code cannot find: the resume fails visibly,
/// nothing is acknowledged and no new session takes its place.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn unavailable_recorded_session_fails_without_a_fresh_substitute() {
    let fixture = Fixture::new("forget-session", None, None);
    let (run, session, _) = first_turn_then_receiver_stop(&fixture);
    let (terminal, code, seen) = run.end();
    println!("terminal: {terminal}");
    assert!(code.is_some(), "{terminal}");
    let failed = seen
        .iter()
        .find(|value| value["event"] == "session-failed")
        .unwrap_or_else(|| panic!("no session-failed: {seen:#?}"));
    assert!(
        failed["label"]
            .as_str()
            .unwrap_or("")
            .starts_with("session-rejected-"),
        "{failed}"
    );
    assert!(!has(&seen, "session-resumed"), "{seen:#?}");
    assert_eq!(
        seen.iter()
            .filter(|value| value["event"] == "session-opened")
            .count(),
        1,
        "{seen:#?}"
    );
    assert!(
        !seen
            .iter()
            .any(|value| value["event"] == "ack" && value["index"] == 1),
        "{seen:#?}"
    );
    assert!(
        !seen
            .iter()
            .any(|value| value["event"] == "agent-message" && value["input"] == 1),
        "{seen:#?}"
    );
    let launches = fixture.launches();
    assert_eq!(launches.len(), 2, "{launches:?}");
    assert_eq!(eq_flag(&launches[1], "--resume"), Some(session.as_str()));
    assert_eq!(eq_flag(&launches[1], "--session-id"), None);
    // The follow-up was never sent to the process that could not resume.
    let users = fixture
        .records()
        .iter()
        .filter(|value| value.get("user").is_some())
        .count();
    assert_eq!(users, 2, "input 0 and the stopped attempt only");
}

/// Claude Code continuing under another session id than the one resumed
/// (a silently fresh or forked conversation): the receiver acknowledges
/// nothing, forwards nothing and ends visibly; the follow-up is refused.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn mismatched_resumed_session_acknowledges_nothing() {
    let fixture = Fixture::new("resume-forks", None, None);
    let (mut run, session, _) = first_turn_then_receiver_stop(&fixture);
    let resumed = run.event("session-resumed");
    assert_eq!(resumed["session"], session.as_str(), "{resumed}");
    let rejected = run.until("follow-up refused", |value| {
        value["event"] == "rejected" && value["index"] == 1
    });
    println!("rejected: {rejected}");
    let (terminal, code, seen) = run.end();
    println!("terminal: {terminal}");
    assert!(code.is_some(), "{terminal}");
    assert!(
        !seen
            .iter()
            .any(|value| value["event"] == "ack" && value["index"] == 1),
        "{seen:#?}"
    );
    assert!(
        !seen
            .iter()
            .any(|value| value["event"] == "agent-message" && value["input"] == 1),
        "{seen:#?}"
    );
    assert!(
        seen.iter().any(|value| value["event"] == "notice"
            && value["title"] == "claude receiver ending"
            && value["description"]
                .as_str()
                .unwrap_or("")
                .contains(&format!("not {session}"))),
        "{seen:#?}"
    );
}

/// The SDK stamps only the first top-level block. Prior blocks and their
/// named/unnamed result cannot leak into, or supply the stop reason for,
/// a current reply, even once that reply already has a parent.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn prior_multiblock_turns_do_not_supply_current_output_or_turn_end() {
    for scenario in [
        "prior-blocks-named-before",
        "prior-blocks-unnamed-before",
        "prior-blocks-named-after",
        "prior-blocks-unnamed-after",
    ] {
        let fixture = Fixture::new(scenario, None, Some(2));
        let (mut run, session, _) = first_turn_then_receiver_stop(&fixture);
        let resumed = run.event("session-resumed");
        assert_eq!(resumed["session"], session.as_str());
        let ack = run.until("ack 1", |v| v["event"] == "ack" && v["index"] == 1);
        let end = run.until("current turn end", |v| {
            v["event"] == "turn-end" && v["input"] == 1
        });
        assert_eq!(end["stop_reason"], "end_turn", "{scenario}: {end}");
        run.control(json!({ "cmd": "close" }));
        let (terminal, code, seen) = run.end();
        assert_eq!(terminal["status"], "closed", "{scenario}: {terminal}");
        assert_eq!(code, Some(7));
        assert!(
            !seen.iter().any(|v| v["event"] == "agent-message"
                && v["text"].as_str().unwrap_or("").contains("PRIOR")),
            "{scenario}: prior blocks leaked: {seen:#?}"
        );
        assert!(
            seen.iter().any(|v| v["event"] == "agent-message"
                && v["text"] == "ECHO second input"
                && v["parent_message_id"] == ack["message_id"]),
            "{scenario}: current continuation lost: {seen:#?}"
        );
        assert_eq!(
            seen.iter()
                .filter(|v| v["event"] == "turn-end" && v["input"] == 1)
                .count(),
            1
        );
        assert_eq!(
            seen.iter()
                .filter(|v| v["event"] == "ack" && v["index"] == 1)
                .count(),
            1
        );
        std::fs::remove_dir_all(&fixture.dir).unwrap();
    }
}

/// An unknown-UUID result is visibly unresolved, not a current terminal.
/// Quiet acknowledged work stays open past its receipt interval until the
/// caller explicitly cancels. This replaces the superseded O4 bounded-end
/// oracle; the test's observation window is not a receiver completion clock.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn fresh_unknown_uuid_result_is_visible_and_waits_for_explicit_cancellation() {
    let fixture = Fixture::new("unknown-result", None, Some(2));
    let mut run = Run::start(&fixture, &fixture.spec("root", "hello"));
    run.event("ack");
    let notice = run.until("unresolved result notice", |v| {
        v["event"] == "notice" && v["title"] == "claude result attribution unresolved"
    });
    assert!(
        notice["description"]
            .as_str()
            .unwrap()
            .contains("explicit cancellation")
    );
    let until = std::time::Instant::now() + Duration::from_millis(2600);
    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
        match run.lines.recv_timeout(left) {
            Ok(value) => {
                assert_ne!(
                    value["event"], "turn-end",
                    "unknown result falsely ended current input: {value}"
                );
                assert_ne!(
                    value["event"], "terminal",
                    "quiet current work was stopped: {value}"
                );
                run.seen.push(value);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(error) => panic!("receiver disappeared during quiet work: {error}"),
        }
    }
    assert!(!has(&run.seen, "turn-end"), "{:#?}", run.seen);
    run.control(json!({ "cmd": "cancel" }));
    let (terminal, code, seen) = run.end();
    assert_eq!(terminal["status"], "cancelled", "{terminal}");
    assert!(code.is_some());
    assert_eq!(seen.iter().filter(|v| v["event"] == "ack").count(), 1);
    assert!(!has(&seen, "agent-message"), "{seen:#?}");
    assert!(!has(&seen, "relaunch"), "{seen:#?}");
    std::fs::remove_dir_all(&fixture.dir).unwrap();
}

/// Prior results before receipt and during an attributed current reply are
/// not failure evidence. The same resumed input completes after quiet work
/// longer than ack_timeout_s, with its own parent and its own stop reason.
#[test]
#[ignore = "needs OULIPOLY_NATIVE_CLAUDE_DEPS, OULIPOLY_NATIVE_NODE and AGENT_BASH_BIN"]
fn prior_results_do_not_limit_long_current_continuation() {
    for scenario in [
        "prior-blocks-named-before-ack",
        "prior-blocks-named-after-long",
    ] {
        let fixture = Fixture::new(scenario, None, Some(2));
        let (mut run, session, _) = first_turn_then_receiver_stop(&fixture);
        let resumed = run.event("session-resumed");
        assert_eq!(resumed["session"], session.as_str());
        let ack = run.until("ack 1", |v| v["event"] == "ack" && v["index"] == 1);
        let started = std::time::Instant::now();
        let end = run.until("long current turn end", |v| {
            v["event"] == "turn-end" && v["input"] == 1
        });
        assert_eq!(end["stop_reason"], "end_turn", "{scenario}: {end}");
        assert!(
            started.elapsed() > Duration::from_secs(2),
            "{scenario}: no long work encountered"
        );
        run.control(json!({ "cmd": "close" }));
        let (terminal, code, seen) = run.end();
        assert_eq!(terminal["status"], "closed", "{terminal}");
        assert_eq!(code, Some(7));
        assert!(
            !seen.iter().any(|v| v["event"] == "agent-message"
                && v["text"].as_str().unwrap_or("").contains("PRIOR")),
            "{scenario}: {seen:#?}"
        );
        for text in ["current progress after quiet work", "ECHO second input"] {
            assert!(
                seen.iter().any(|v| v["event"] == "agent-message"
                    && v["text"] == text
                    && v["parent_message_id"] == ack["message_id"]),
                "{scenario}: {seen:#?}"
            );
        }
        assert_eq!(
            seen.iter()
                .filter(|v| v["event"] == "turn-end" && v["input"] == 1)
                .count(),
            1
        );
        std::fs::remove_dir_all(&fixture.dir).unwrap();
    }
}
