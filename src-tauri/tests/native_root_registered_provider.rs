//! The Runner's `native-root` entry given a registered external provider
//! (Linux). The provider is a deterministic stand-in executable: it answers
//! `describe`, `policy.evaluate` and `resident.prepare` over the provider
//! contract, and its `resident.serve` runs the owner crate's deterministic
//! ACP v2 peer as the root's harness. No native harness, model, credential
//! or network; unprivileged user namespaces only.
//!
//! Ignored by default: it needs `oulipoly-root-supervisor`,
//! `oulipoly-root-pid1`, `oulipoly-root-bash` and
//! `oulipoly-acp-deterministic-peer` built beside the Runner
//! (`cargo build -p oulipoly-root-supervisor --bins` in the same target),
//! because the entry admits everything of its own, the owner included,
//! before it runs a provider.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};

const RUNNER: &str = env!("CARGO_BIN_EXE_oulipoly-agent-runner");
const WATCHDOG: Duration = Duration::from_secs(60);
const NEEDS: &str = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces";
const DECLARED: &str = "registered-root-declared-marker";

fn sibling(name: &str) -> PathBuf {
    let path = Path::new(RUNNER).parent().unwrap().join(name);
    assert!(path.is_file(), "{NEEDS}: no {}", path.display());
    path
}

/// The stand-in provider. `edit` is Python run before it answers, with
/// `op`, `request` and `caps` in scope. Its `resident.serve` reads the
/// prepared configuration and runs the peer with the flags the policy put
/// in its argv after `--peer`.
fn provider(dir: &Path, edit: &str) -> PathBuf {
    let path = dir.join("provider");
    let source = format!(
        r#"#!/usr/bin/python3
import hashlib, json, os, sys
CALLS = {calls:?}
PEER = {peer:?}
op = sys.argv[1]
request = None if op == "resident.serve" else json.loads(sys.stdin.read())
record = {{"op": op, "euid": os.geteuid(), "argv": sys.argv[1:], "cwd": os.getcwd(), "request": request}}
if op == "resident.serve":
    record["env"] = sorted(os.environ)
    record["declared"] = os.environ.get("REGISTERED_WITNESS")
with open(CALLS, "a") as f:
    f.write(json.dumps(record) + "\n")
def answer(result):
    print(json.dumps({{"contract": "oulipoly.provider/v1", "request_id": request["request_id"], "ok": True, "result": result}}))
selected = request is not None and request["host"].get("env", {{}}).get("OULIPOLY_HOST_RESIDENT_SESSION_V1") == "1"
caps = {{"launch": True, "policy": True, "quota": False, "session": False, "terminal": False,
        "rotation": False, "discovery": False, "settings": False, "setup_brain": False,
        "setup": False, "migration": False}}
if selected:
    caps["resident_session_v1"] = True
mediation_env = request and request["params"].get("launch", {{}}).get("env", {{}}).get("OULIPOLY_TOOL_MEDIATION_V1")
mediation = json.loads(mediation_env) if mediation_env else None
marker = dict(mediation or {{}})
marker.pop("requester", None)
marker.update(tool="fake_mediated_bash", native_tools=["fake_mediated_bash"])
markers = [{{"name": "oulipoly.tool_mediation/v1", "value": marker}}]
if request and request["host"].get("env", {{}}).get("OULIPOLY_HOST_TOOL_MEDIATION_V1") == "1":
    caps["tool_mediation_v1"] = True
{edit}
if op == "describe":
    answer({{"provider_id": "stand-in-external", "display_name": "Stand-in external provider",
            "contract_versions": ["oulipoly.provider/v1"], "preferred_contract": "oulipoly.provider/v1",
            "capabilities": caps}})
elif op == "policy.evaluate":
    flags = request["params"]["launch"].get("peer_flags", [])
    answer({{"accepted": True, "argv": ["native", "--peer"] + flags, "env": {{"NATIVE_POLICY": "1", "OULIPOLY_TOOL_MEDIATION_V1": mediation_env}},
            "stdin": None, "prompt": None, "diagnostics": [], "markers": markers}})
elif op == "resident.prepare":
    data = request["host"]["data_root"]
    config = json.dumps(request["params"]["launch"], sort_keys=True).encode()
    digest = hashlib.sha256(config).hexdigest()
    os.makedirs(os.path.join(data, "configs"), mode=0o700)
    path = os.path.join(data, "configs", digest + ".json")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.write(fd, config)
    os.close(fd)
    answer({{"protocol": "oulipoly.resident_session/v1",
            "invocation": {{"args": ["resident.serve", "--config", path], "endpoint": "stdio"}},
            "acp": {{"protocol_version": 2, "schema": "schema-v2.0.0-alpha.7", "dedup_contract": 1}},
            "config_sha256": digest,
            "operations": ["initialize", "session/new", "session/resume", "session/prompt",
                           "session/cancel", "session/close", "session/list"]}})
elif op == "resident.serve":
    path = sys.argv[sys.argv.index("--config") + 1]
    config = open(path, "rb").read()
    assert os.path.basename(path) == hashlib.sha256(config).hexdigest() + ".json"
    argv = json.loads(config)["argv"]
    flags = argv[argv.index("--peer") + 1:]
    state = os.path.join(os.path.dirname(os.path.dirname(path)), "peer-state.json")
    os.execv(PEER, [PEER, "--state", state] + flags)
else:
    sys.exit(3)
"#,
        calls = dir.join("calls").to_string_lossy(),
        peer = sibling("oulipoly-acp-deterministic-peer").to_string_lossy(),
    );
    std::fs::write(&path, source).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn requester(dir: &Path) -> PathBuf {
    let path = dir.join("agent-bash");
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn calls(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn ops(dir: &Path) -> Vec<String> {
    calls(dir)
        .iter()
        .map(|call| call["op"].as_str().unwrap().to_owned())
        .collect()
}

/// A fresh root request naming the registered provider at `executable`.
fn request(dir: &Path, executable: &Path, peer_flags: &[&str], message: &str) -> Value {
    json!({
        "store": dir.join("store"),
        "launch_dir": dir.join("launch"),
        "cwd": dir,
        "env": { "PATH": "/usr/bin:/bin", "REGISTERED_WITNESS": DECLARED },
        "messages": [message],
        "outage_closure_cap": 2,
        "delivery_attempt_cap": 2,
        "provider": {
            "executable": executable,
            "settings": {
                "settings_id": "stand-in",
                "mode": "arg",
                "model": { "name": "stand-in-model", "provider_args": [],
                           "inputs": { "prompt": null, "named": {} } },
                "launch": { "peer_flags": peer_flags },
            },
            "env": { "PROVIDER_ONLY": "1" },
            "agent_bash_bin": requester(dir),
            "bash_authority": "trusted-task",
        },
        "workload": { "isolation": "unprivileged-userns" },
    })
}

struct Run {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Value>,
    seen: Vec<Value>,
}

impl Run {
    fn start(dir: &Path, request: &Value) -> Self {
        let path = dir.join("request.json");
        std::fs::write(&path, request.to_string()).unwrap();
        let mut child = Command::new(RUNNER)
            .args(["native-root", "--request"])
            .arg(&path)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
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
        }
    }

    fn event(&mut self, event: &str) -> Value {
        self.until(event, |value| value["event"] == event)
    }

    fn entry(&mut self, stage: &str) -> Value {
        self.until(stage, |value| value["entry"] == stage)
    }

    fn control(&mut self, line: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    /// The entry's terminal line and exit code.
    fn end(mut self) -> (Value, Option<i32>, Vec<Value>) {
        let terminal = self.entry("terminal");
        let status = self.child.wait().unwrap();
        (terminal, status.code(), std::mem::take(&mut self.seen))
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

fn scratch() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// An ordinary registered root: the provider is described, evaluated and
/// prepared once each, as this entry; the owner runs the prepared harness
/// (the registered executable with the prepared arguments) under its own
/// ACP v2 negotiation, labelled with the provider's declared id. Inputs
/// are attributed to one native session, a follow-up reaches that session,
/// in-root Bash goes through the root's ingress with the root's declared
/// environment, and close ends the run as `closed`.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn registered_provider_root_runs_its_prepared_harness_through_the_owner() {
    let dir = scratch();
    let executable = provider(dir.path(), "");
    let mut run = Run::start(
        dir.path(),
        &request(dir.path(), &executable, &[], "echo:first"),
    );
    let described = run.entry("provider-described");
    assert_eq!(described["provider_id"], "stand-in-external", "{described}");
    assert_eq!(described["agreed_contract"], "oulipoly.provider/v1");
    assert_eq!(described["resident_session"], 1);
    let setup = run.entry("setup-completed");
    let launch = &setup["launch"];
    assert_eq!(setup["harness"], "registered-provider", "{setup}");
    assert_eq!(launch["provider_id"], "stand-in-external");
    assert_eq!(launch["argv"][0], executable.to_string_lossy().as_ref());
    assert_eq!(launch["argv"][1], "resident.serve");
    assert_eq!(launch["endpoint"], "stdio");
    assert_eq!(
        launch["executable_identity_sha256"].as_str().unwrap().len(),
        64
    );
    assert_eq!(
        launch["tools"]["bash"],
        json!({ "authority": "trusted-task" })
    );
    assert_eq!(launch["tools"]["ingress_env"], "OULIPOLY_ROOT_BASH_V1");
    assert_eq!(
        launch["template_env"],
        json!(["NATIVE_POLICY", "OULIPOLY_TOOL_MEDIATION_V1"])
    );
    assert!(
        launch["data_root_handed_over"]
            .as_str()
            .unwrap()
            .starts_with("none")
    );
    assert!(
        !setup.to_string().contains(DECLARED),
        "values echoed: {setup}"
    );

    let launched = run.event("launched");
    assert_eq!(launched["harness"], "stand-in-external", "{launched}");
    let serve_pid = launched["pid"].as_u64().unwrap();
    let at = |event: &'static str, field: &'static str, index: u64| {
        move |value: &Value| value["event"] == event && value[field] == index
    };
    let ack0 = run.until("ack 0", at("ack", "index", 0));
    let reply0 = run.until("reply 0", at("agent-message", "input", 0));
    let turn0 = run.until("turn-end 0", at("turn-end", "input", 0));
    assert_eq!(reply0["text"], "first", "{reply0}");
    assert_eq!(turn0["message_id"], ack0["message_id"]);
    let session = turn0["session"].clone();

    run.control(
        json!({ "cmd": "send", "text": "bash:printf '%s' \"$REGISTERED_WITNESS\"", "ref": "f1" }),
    );
    let admitted = run.event("follow-up-admitted");
    assert_eq!(admitted["input"], 1, "{admitted}");
    let reply1 = run.until("reply 1", at("agent-message", "input", 1));
    assert!(
        reply1["text"].as_str().unwrap().contains(DECLARED),
        "{reply1}"
    );
    assert_eq!(reply1["session"], session);
    let turn1 = run.until("turn-end 1", at("turn-end", "input", 1));
    assert_eq!(turn1["session"], session);
    let accepted = run.event("bash-accepted");
    assert_eq!(accepted["harness"], "stand-in-external", "{accepted}");
    assert_eq!(accepted["session"], session);
    assert_eq!(accepted["inputs_open"][0]["index"], 1);
    let ended_bash = run.event("bash-ended");
    assert_eq!(ended_bash["work"], accepted["work"]);
    assert_eq!(ended_bash["status"], "code:0", "{ended_bash}");

    run.control(json!({ "cmd": "close" }));
    let (terminal, code, seen) = run.end();
    assert_eq!(terminal["stage"], "owner-ended", "{terminal}");
    assert_eq!(code, Some(87), "{terminal}");
    let owner = seen
        .iter()
        .find(|value| value["event"] == "terminal")
        .expect("owner terminal");
    assert_eq!(owner["status"], "closed", "{owner}");
    assert_eq!(owner["owed"], 0);
    assert_eq!(owner["harnesses"][0]["id"], "stand-in-external");
    assert_eq!(owner["harnesses"][0]["launches"], 1);
    wait_ended(serve_pid, "resident harness");

    // Each provider operation exactly once, the harness once.
    assert_eq!(
        ops(dir.path()),
        [
            "describe",
            "policy.evaluate",
            "resident.prepare",
            "resident.serve"
        ]
    );
    let calls = calls(dir.path());
    // Describe and prepare offered the resident session; prepare's data
    // root is the root's own, under its launch directory.
    for index in [0, 2] {
        assert_eq!(
            calls[index]["request"]["host"]["env"]["OULIPOLY_HOST_RESIDENT_SESSION_V1"],
            "1"
        );
        assert_eq!(calls[index]["request"]["host"]["env"]["PROVIDER_ONLY"], "1");
    }
    assert_eq!(
        calls[2]["request"]["host"]["data_root"],
        dir.path()
            .join("launch/provider")
            .to_string_lossy()
            .as_ref()
    );
    // The policy's argv and env, plus Runner's tool policy, are the template.
    let template = &calls[2]["request"]["params"]["launch"];
    assert_eq!(template["argv"], json!(["native", "--peer"]));
    let tools: Value = serde_json::from_str(
        template["env"]["OULIPOLY_TOOL_MEDIATION_V1"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(tools["bash"], json!({ "authority": "trusted-task" }));
    // The harness runs in the intent's cwd with the root's environment and
    // ingress, never the provider's own operation environment.
    let serve = &calls[3];
    assert_eq!(
        serve["argv"],
        launch["argv"].as_array().unwrap()[1..]
            .iter()
            .cloned()
            .collect::<Value>()
    );
    assert_eq!(serve["cwd"], dir.path().to_string_lossy().as_ref());
    assert_eq!(serve["declared"], DECLARED);
    let env: Vec<&str> = serve["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(env.contains(&"OULIPOLY_ROOT_BASH_V1"), "{env:?}");
    assert!(!env.contains(&"PROVIDER_ONLY"), "{env:?}");
    assert!(
        !env.contains(&"OULIPOLY_ACP_V2_SOCKET"),
        "stdio endpoint: {env:?}"
    );
    assert_eq!(
        setup["env"]["native_host"]["set_by_owner"],
        json!(["OULIPOLY_ROOT_BASH_V1"])
    );
    assert_eq!(launch["tool_mediation"], 1);
    assert_eq!(launch["effective_mediation"]["tool"], "fake_mediated_bash");
    assert_eq!(
        launch["effective_mediation"]["bash"],
        launch["tools"]["bash"]
    );
    assert_eq!(
        launch["effective_mediation"]["native_tools"],
        json!(["fake_mediated_bash"])
    );
    for call in &calls[..3] {
        assert_eq!(
            call["request"]["host"]["env"]["OULIPOLY_HOST_TOOL_MEDIATION_V1"],
            "1"
        );
    }
    assert_eq!(
        calls[1]["request"]["params"]["launch"]["env"]["OULIPOLY_TOOL_MEDIATION_V1"],
        template["env"]["OULIPOLY_TOOL_MEDIATION_V1"]
    );
    let peer: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("launch/provider/peer-state.json")).unwrap(),
    )
    .unwrap();
    let prompts = peer["prompts"].as_array().unwrap();
    assert_eq!(prompts.len(), 2, "{peer}");
    assert!(
        prompts
            .iter()
            .all(|prompt| prompt["session"] == prompts[0]["session"])
    );
}

/// The owner's recipient session fence holds for a registered harness:
/// evidence from a session the harness never opened, though tagged with
/// the current input, neither ends that input nor admits a follow-up and is
/// not attributed; the genuine idle then releases it and the next input is
/// admitted on the opened session.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn registered_provider_root_keeps_the_recipient_session_fence() {
    let dir = scratch();
    let executable = provider(dir.path(), "");
    let gate = dir.path().join("current-turn");
    let gate_flag = gate.to_string_lossy().into_owned();
    let flags = [
        "--off-session-turn",
        "sess-unknown",
        "--off-session-after-ack",
        "--turn-gate",
        gate_flag.as_str(),
    ];
    let mut run = Run::start(dir.path(), &request(dir.path(), &executable, &flags, "one"));
    let opened = run.event("session-opened");
    assert_ne!(opened["session"], "sess-unknown");
    assert_eq!(opened["harness"], "stand-in-external", "{opened}");
    let ack = run.event("ack");
    run.until("off-session evidence", |value| {
        value["event"] == "notice" && value["title"] == "off-session-sent"
    });
    run.control(json!({ "cmd": "send", "text": "must-stay-blocked" }));
    let decision = run.until("overlap decision", |value| {
        value["event"] == "follow-up-refused" || value["event"] == "follow-up-admitted"
    });
    assert_eq!(decision["event"], "follow-up-refused", "{decision}");
    assert_eq!(decision["reason"], "input-open");
    let reply = run.event("agent-message");
    assert_eq!(reply["session"], "sess-unknown");
    assert!(reply["input"].is_null(), "attributed: {reply}");
    assert!(run.seen.iter().all(|value| value["event"] != "turn-end"));
    std::fs::write(&gate, b"release").unwrap();
    let turn = run.event("turn-end");
    assert_eq!(turn["session"], opened["session"]);
    assert_eq!(turn["message_id"], ack["message_id"]);
    assert_eq!(turn["own_output"], false);
    run.control(json!({ "cmd": "send", "text": "echo:CURRENT" }));
    run.event("follow-up-admitted");
    let next = run.until("next turn", |value| {
        value["event"] == "turn-end" && value["input"] == 1
    });
    assert_eq!(next["session"], opened["session"]);
    assert_eq!(next["own_output"], true);
    run.control(json!({ "cmd": "close" }));
    let (terminal, code, seen) = run.end();
    assert_eq!(code, Some(87), "{terminal}");
    let owner = seen
        .iter()
        .find(|value| value["event"] == "terminal")
        .expect("owner terminal");
    assert_eq!(owner["status"], "closed", "{owner}");
    assert_eq!(owner["owed"], 0);
    assert_eq!(
        ops(dir.path())
            .iter()
            .filter(|op| *op == "resident.serve")
            .count(),
        1
    );
}

/// A registered root whose turn never ends is cancelled through the entry's
/// control: the owner kills the prepared harness and ends `cancelled`.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn registered_provider_root_is_cancelled_by_its_owner() {
    let dir = scratch();
    let executable = provider(dir.path(), "");
    let mut run = Run::start(
        dir.path(),
        &request(dir.path(), &executable, &["--no-idle"], "echo:never-ends"),
    );
    let serve_pid = run.event("launched")["pid"].as_u64().unwrap();
    run.until("ack 0", |value| {
        value["event"] == "ack" && value["index"] == 0
    });
    run.control(json!({ "cmd": "cancel" }));
    let (terminal, code, seen) = run.end();
    assert_eq!(code, Some(82), "{terminal}");
    let owner = seen
        .iter()
        .find(|value| value["event"] == "terminal")
        .unwrap();
    assert_eq!(owner["status"], "cancelled", "{owner}");
    assert_eq!(owner["cancel_requested"], true);
    wait_ended(serve_pid, "resident harness");
    assert_eq!(
        ops(dir.path())
            .iter()
            .filter(|op| *op == "resident.serve")
            .count(),
        1
    );
}

/// The entry dies with its registered root's harness live; the harness
/// survives the owner. A `cancel` recovery of the store attaches the root
/// and ends its work, starting no harness and running no provider
/// operation.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn registered_provider_root_survives_its_entry_and_is_cancelled_by_recovery() {
    let dir = scratch();
    let executable = provider(dir.path(), "");
    let mut run = Run::start(
        dir.path(),
        &request(dir.path(), &executable, &["--no-idle"], "echo:held"),
    );
    let serve_pid = run.event("launched")["pid"].as_u64().unwrap();
    run.until("ack 0", |value| {
        value["event"] == "ack" && value["index"] == 0
    });
    run.child.kill().unwrap();
    run.child.wait().unwrap();
    drop(run);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !ended(serve_pid),
        "the harness outlives the entry and its owner"
    );
    let before = calls(dir.path()).len();

    let recover = dir.path().join("recover.json");
    std::fs::write(
        &recover,
        json!({ "store": dir.path().join("store"), "purpose": "cancel",
                "env": { "PATH": "/usr/bin:/bin" } })
        .to_string(),
    )
    .unwrap();
    let output = Command::new(RUNNER)
        .args(["native-root", "--recover"])
        .arg(&recover)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let owner = lines
        .iter()
        .find(|value| value["event"] == "terminal")
        .unwrap_or_else(|| panic!("{lines:#?}"));
    assert_eq!(owner["status"], "cancelled", "{owner}");
    assert_eq!(output.status.code(), Some(82), "{lines:#?}");
    wait_ended(serve_pid, "resident harness");
    assert_eq!(
        calls(dir.path()).len(),
        before,
        "recovery ran no provider operation"
    );
}

/// Selected tool authority never admits an accepted but unmediated policy.
#[test]
#[ignore = "needs owner binaries beside Runner"]
fn missing_or_contradictory_mediation_refuses_without_setup_or_owner() {
    for edit in [
        "caps.pop('tool_mediation_v1', None)",
        "caps['tool_mediation_v1'] = False",
        "caps.pop('tool_mediation_v1', None); caps['tool_mediation_v2'] = True",
        "if op == 'policy.evaluate':\n    markers = []",
        "if op == 'policy.evaluate':\n    marker['bash'] = {'allow': ['other-command']}",
        "if op == 'policy.evaluate':\n    marker['ingress_env'] = 'OTHER_INGRESS'",
        "if op == 'policy.evaluate':\n    marker['native_tools'] = []",
        "if op == 'policy.evaluate':\n    mediation_env = None",
    ] {
        let dir = scratch();
        let executable = provider(dir.path(), edit);
        let (terminal, code, _) =
            Run::start(dir.path(), &request(dir.path(), &executable, &[], "first")).end();
        assert_eq!(code, Some(65), "{edit}: {terminal}");
        assert_eq!(terminal["stage"], "provider-refused");
        assert_eq!(terminal["effects"]["runner_setup"], "none");
        assert_eq!(terminal["effects"]["provider"], "unknown: it ran");
        assert_eq!(terminal["retry"], "do-not-replay-as-unrun");
        assert!(!dir.path().join("launch").exists(), "{edit}");
        assert!(!dir.path().join("store").exists(), "{edit}");
        let seen = ops(dir.path());
        assert!(
            seen == ["describe"] || seen == ["describe", "policy.evaluate"],
            "{edit}: {seen:?}"
        );
    }
}

/// A described provider that does not advertise the resident session is
/// the provider's refusal (65): it ran; this entry made nothing.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn provider_without_a_resident_session_is_refused_after_it_ran() {
    let dir = scratch();
    let executable = provider(dir.path(), "caps.pop('resident_session_v1', None)");
    let (terminal, code, seen) =
        Run::start(dir.path(), &request(dir.path(), &executable, &[], "m")).end();
    assert_eq!(code, Some(65), "{terminal}");
    assert_eq!(terminal["stage"], "provider-refused");
    assert_eq!(terminal["operation"], "describe");
    assert_eq!(terminal["effects"]["runner_setup"], "none");
    assert_eq!(terminal["effects"]["provider"], "unknown: it ran");
    assert!(
        terminal["reason"]
            .as_str()
            .unwrap()
            .contains("not substituted by an embedded harness")
    );
    assert!(seen.iter().all(|line| line["entry"] != "setup-completed"));
    assert_eq!(ops(dir.path()), ["describe"]);
    assert!(!dir.path().join("store").exists() && !dir.path().join("launch").exists());
}

/// Custody refuses a provider below a group-writable directory before
/// anything runs (64, effects none); so does a request naming a provider
/// with an embedded harness.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn untrusted_or_ambiguous_registrations_run_nothing() {
    let dir = scratch();
    let open = dir.path().join("open");
    std::fs::create_dir(&open).unwrap();
    let executable = provider(&open, "");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
    let (terminal, code, _) =
        Run::start(dir.path(), &request(dir.path(), &executable, &[], "m")).end();
    assert_eq!(code, Some(64), "{terminal}");
    assert_eq!(terminal["effects"], "none");
    assert!(
        terminal["reason"]
            .as_str()
            .unwrap()
            .contains("writable by group or others"),
        "{terminal}"
    );
    assert!(calls(&open).is_empty());

    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut both = request(dir.path(), &executable, &[], "m");
    both["opencode"] = json!({ "deps": "/d", "agent_bash_tool": "/t", "agent_bash_bin": "/b", "bash_allow": ["true"] });
    let (terminal, code, _) = Run::start(dir.path(), &both).end();
    assert_eq!(code, Some(64), "{terminal}");
    assert!(terminal["reason"].as_str().unwrap().contains("exactly one"));
    assert!(calls(&open).is_empty());
}

/// A provider whose `resident.prepare` answers outside the extension is a
/// setup failure after this entry's launch directory was made (73): the
/// owner is never started.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn bad_preparation_is_a_setup_failure_with_provider_effects() {
    let dir = scratch();
    let executable = provider(
        dir.path(),
        "if op == 'resident.prepare':\n    answer({'protocol': 'oulipoly.resident_session/v1'})\n    sys.exit(0)",
    );
    let (terminal, code, seen) =
        Run::start(dir.path(), &request(dir.path(), &executable, &[], "m")).end();
    assert_eq!(code, Some(73), "{terminal}");
    assert_eq!(terminal["stage"], "setup-failed");
    assert_eq!(terminal["effects"], "possible");
    assert_eq!(terminal["provider"], "unknown: it ran");
    assert!(seen.iter().all(|line| line["entry"] != "owner-started"));
    assert_eq!(
        ops(dir.path()),
        ["describe", "policy.evaluate", "resident.prepare"]
    );
    assert!(dir.path().join("launch/provider").is_dir());
    assert!(!dir.path().join("store").exists());
}

/// A fake native `claude -p --input-format stream-json --output-format
/// stream-json` for a real Claude provider adapter: it echoes the submitted
/// user record on the native session it was given and answers once. No real
/// native harness or model runs.
const FAKE_NATIVE_CLAUDE: &str = r#"#!/usr/bin/python3
import json, os, sys
args = sys.argv[1:]
line = sys.stdin.readline()
with open(os.environ['CALLS'], 'a') as f:
    f.write(json.dumps({'argv': args, 'tools': os.environ.get('OULIPOLY_TOOL_MEDIATION_V1')}) + '\n')
message = json.loads(line)
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
emit({'type': 'assistant', 'parent_tool_use_id': None, 'message': {'content': [{'type': 'text', 'text': 'reply'}]}})
emit({'type': 'result', 'subtype': 'success', 'is_error': False, 'stop_reason': 'end_turn', 'session_id': session})
"#;

fn real_adapter(variable: &str) -> PathBuf {
    let path = std::env::var(variable)
        .unwrap_or_else(|_| panic!("{variable}: a built provider adapter executable"));
    PathBuf::from(path)
}

/// The real Claude provider adapter (`OULIPOLY_REAL_CLAUDE_ADAPTER`, built
/// from its merged source) as a registered root, with a fake native: Runner
/// describes it through the contract crate, evaluates its settings, adds its
/// tool policy, has it prepare under the root's private data root, and the
/// owner serves its resident endpoint as the root's harness, labelled with
/// its declared id: ACP v2 and message-key dedup are negotiated and a native
/// session is opened. Cancel then ends the root and its harness.
///
/// Native turns of a real adapter inside the owner's work namespace are not
/// claimed here: the provider execution lifecycle identifies its native
/// process through `/proc`, which in the work PID namespace is the host's.
#[test]
#[ignore = "needs OULIPOLY_REAL_CLAUDE_ADAPTER (a built agent-runner-claude) and the owner binaries beside the Runner"]
fn real_claude_adapter_is_prepared_and_served_as_a_registered_root() {
    let adapter = real_adapter("OULIPOLY_REAL_CLAUDE_ADAPTER");
    let dir = scratch();
    let native = dir.path().join("claude");
    std::fs::write(&native, FAKE_NATIVE_CLAUDE).unwrap();
    std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o755)).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut value = request(dir.path(), &adapter, &[], "first");
    value["provider"]["settings"] = json!({
        "settings_id": "claude-witness",
        "mode": "headless",
        "model": { "name": "claude-opus", "provider_args": ["--model", "opus"],
                   "inputs": { "prompt": null, "named": {} } },
        "launch": { "command": native, "prompt_mode": "stdin",
                    "env": { "CALLS": dir.path().join("native-calls") } },
    });
    value["provider"]["env"] = json!({ "HOME": home });
    let mut run = Run::start(dir.path(), &value);
    let described = run.entry("provider-described");
    assert_eq!(described["provider_id"], "claude", "{described}");
    assert_eq!(described["agreed_contract"], "oulipoly.provider/v1");
    assert_eq!(described["resident_session"], 1);
    let setup = run.entry("setup-completed");
    let launch = &setup["launch"];
    assert_eq!(
        launch["argv"][0],
        adapter.to_string_lossy().as_ref(),
        "{setup}"
    );
    assert_eq!(launch["argv"][1], "resident.serve");
    assert_eq!(launch["acp"]["protocol_version"], 2);
    assert_eq!(
        launch["template_env"],
        json!(["CALLS", "OULIPOLY_TOOL_MEDIATION_V1"])
    );
    // The prepared configuration is private to the serving identity through
    // its directories: the data root (this entry's, 0700) and the adapter's
    // own configuration directory (0700).
    let config = launch["argv"][3].as_str().unwrap();
    assert!(
        config.starts_with(dir.path().join("launch/provider").to_str().unwrap()),
        "{config}"
    );
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir.path().join("launch/provider")), 0o700);
    assert_eq!(mode(Path::new(config).parent().unwrap()), 0o700, "{config}");
    let recorded: Value = serde_json::from_str(&std::fs::read_to_string(config).unwrap()).unwrap();
    let tools: Value = serde_json::from_str(
        recorded["launch"]["env"]["OULIPOLY_TOOL_MEDIATION_V1"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(tools["bash"], json!({ "authority": "trusted-task" }));

    let launched = run.event("launched");
    assert_eq!(launched["harness"], "claude", "{launched}");
    let serve_pid = launched["pid"].as_u64().unwrap();
    let negotiated = run.event("negotiated");
    assert_eq!(negotiated["dedup_contract"], true, "{negotiated}");
    let opened = run.event("session-opened");
    assert_eq!(opened["harness"], "claude");
    run.control(json!({ "cmd": "cancel" }));
    let (terminal, code, seen) = run.end();
    assert_eq!(code, Some(82), "{terminal}");
    let owner = seen
        .iter()
        .find(|value| value["event"] == "terminal")
        .expect("owner terminal");
    assert_eq!(owner["status"], "cancelled", "{owner}");
    wait_ended(serve_pid, "resident harness");
}

/// A wrapper around a real provider adapter whose `describe` answer gains
/// advertisements this entry does not know: a preferred v2 contract beside
/// v1, a resident session v2 and an unknown structured capability.
fn future_advertising(dir: &Path, adapter: &Path) -> PathBuf {
    let path = dir.join("future-advertising");
    let source = format!(
        r#"#!/usr/bin/python3
import json, subprocess, sys
ADAPTER = {adapter:?}
if sys.argv[1] != "describe":
    import os
    os.execv(ADAPTER, [ADAPTER] + sys.argv[1:])
answer = json.loads(subprocess.run([ADAPTER, "describe"], input=sys.stdin.buffer.read(), capture_output=True, check=True).stdout)
result = answer["result"]
result["contract_versions"] = ["oulipoly.provider/v2", "oulipoly.provider/v1"]
result["preferred_contract"] = "oulipoly.provider/v2"
result["capabilities"]["resident_session_v2"] = True
result["capabilities"]["future_capability"] = {{"shape": [1, 2]}}
print(json.dumps(answer))
"#,
        adapter = adapter.to_string_lossy()
    );
    std::fs::write(&path, source).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Real provider adapters (`OULIPOLY_REAL_CLAUDE_ADAPTER`,
/// `OULIPOLY_REAL_CODEX_ADAPTER`) described on the wire through the contract
/// crate's admission and selection: as they answer, and with advertisements
/// newer than this entry's. Settings the adapters refuse then end each root
/// as the provider's refusal (65): it ran; this entry made nothing.
#[test]
#[ignore = "needs OULIPOLY_REAL_CLAUDE_ADAPTER and OULIPOLY_REAL_CODEX_ADAPTER (built adapters) and the owner binaries beside the Runner"]
fn real_adapters_are_described_through_the_contract_crate_with_future_advertisements() {
    for (variable, id) in [
        ("OULIPOLY_REAL_CLAUDE_ADAPTER", "claude"),
        ("OULIPOLY_REAL_CODEX_ADAPTER", "codex"),
    ] {
        let adapter = real_adapter(variable);
        for future in [false, true] {
            let dir = scratch();
            let executable = if future {
                future_advertising(dir.path(), &adapter)
            } else {
                adapter.clone()
            };
            let mut value = request(dir.path(), &executable, &[], "m");
            value["provider"]["settings"] = json!({
                "settings_id": "refused", "mode": "no-such-mode",
                "model": { "name": "m", "provider_args": [], "inputs": { "prompt": null, "named": {} } },
            });
            value["provider"]["env"] = json!({ "HOME": dir.path() });
            let (terminal, code, seen) = Run::start(dir.path(), &value).end();
            let described = seen
                .iter()
                .find(|line| line["entry"] == "provider-described")
                .unwrap_or_else(|| panic!("{id} future={future}: {seen:#?}"));
            assert_eq!(described["provider_id"], id, "{described}");
            assert_eq!(described["agreed_contract"], "oulipoly.provider/v1");
            assert_eq!(described["resident_session"], 1);
            let preferred = if future {
                "oulipoly.provider/v2"
            } else {
                "oulipoly.provider/v1"
            };
            assert_eq!(described["preferred_contract"], preferred, "{described}");
            assert_eq!(code, Some(65), "{id} future={future}: {terminal}");
            assert_eq!(terminal["stage"], "provider-refused");
            assert_eq!(terminal["operation"], "policy.evaluate", "{terminal}");
            assert_eq!(terminal["effects"]["provider"], "unknown: it ran");
            assert!(!dir.path().join("launch").exists() && !dir.path().join("store").exists());
        }
    }
}
