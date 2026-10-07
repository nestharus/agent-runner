//! The Runner's `native-root` entry given a registered external provider
//! (Linux). The provider is a deterministic stand-in executable: it answers
//! `describe`, `policy.evaluate` and `resident.prepare` over the provider
//! contract, and its `resident.serve` runs the owner crate's deterministic
//! ACP v2 peer as the root's harness. No native harness, model, credential
//! or network; unprivileged user namespaces only. The real-adapter control
//! additionally serves finite fake natives through both delivered adapters.
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
/// in its argv after `--peer`. Selected, it advertises exploration, echoes
/// an offer and reports `fake_explore` (a stand-in native name) for it;
/// it records what each serve's configuration and process environment
/// carry of an offer. The peer's `explore:` uses the `oulipoly-root-child`
/// beside it, which a test offers as the requester: stand-in native and
/// configuration evidence, not a provider's bridge.
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
    record["process_offer"] = os.environ.get("OULIPOLY_EXPLORATION_V1")
    served = json.loads(open(sys.argv[sys.argv.index("--config") + 1], "rb").read())
    record["config_offer"] = (served.get("env") or {{}}).get("OULIPOLY_EXPLORATION_V1")
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
if request and request["host"].get("env", {{}}).get("OULIPOLY_HOST_EXPLORATION_V1") == "1":
    caps["exploration_v1"] = True
exploration_env = request and request["params"].get("launch", {{}}).get("env", {{}}).get("OULIPOLY_EXPLORATION_V1")
if exploration_env:
    offered = json.loads(exploration_env)
    markers.append({{"name": "oulipoly.exploration/v1", "value": {{"protocol": offered["protocol"],
        "routes": offered["routes"], "tool": "fake_explore", "ingress_env": offered["ingress_env"]}}}})
    marker["native_tools"].append("fake_explore")
{edit}
if op == "describe":
    answer({{"provider_id": "stand-in-external", "display_name": "Stand-in external provider",
            "contract_versions": ["oulipoly.provider/v1"], "preferred_contract": "oulipoly.provider/v1",
            "capabilities": caps}})
elif op == "policy.evaluate":
    flags = request["params"]["launch"].get("peer_flags", [])
    env = {{"NATIVE_POLICY": "1", "OULIPOLY_TOOL_MEDIATION_V1": mediation_env}}
    if exploration_env:
        env["OULIPOLY_EXPLORATION_V1"] = exploration_env
    answer({{"accepted": True, "argv": ["native", "--peer"] + flags, "env": env,
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
        Self::spawn(&["native-root", "--request"], &path)
    }

    /// `native-root --recover` of `dir`'s store for `purpose`, with the
    /// recovering owner's controls on stdin.
    fn recover(dir: &Path, purpose: &str) -> Self {
        let path = dir.join(format!("recover-{purpose}.json"));
        std::fs::write(
            &path,
            json!({ "store": dir.join("store"), "purpose": purpose,
                    "env": { "PATH": "/usr/bin:/bin" } })
            .to_string(),
        )
        .unwrap();
        Self::spawn(&["native-root", "--recover"], &path)
    }

    fn spawn(args: &[&str], path: &Path) -> Self {
        let mut child = Command::new(RUNNER)
            .args(args)
            .arg(path)
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
    match std::env::var_os("OULIPOLY_TEST_SCRATCH_DIR") {
        Some(root) => tempfile::tempdir_in(root).unwrap(),
        None => tempfile::tempdir().unwrap(),
    }
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

/// The entry dies after its registered root's one input is settled (tagged
/// turn end); the harness survives the owner. An explicit
/// `continue-attached` recovery through the entry runs no provider
/// operation. With a harness that declared live reattachment, a new input
/// is admitted by the new owner and answered in the same native session;
/// without it, `send` is refused naming the unavailable conversation. In
/// both, the caller's close ends the run as `closed` (87), with no cancel.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn registered_provider_settled_root_is_continued_or_held_truthfully_by_recovery() {
    for declared in [true, false] {
        let dir = scratch();
        let executable = provider(dir.path(), "");
        let flags: &[&str] = if declared { &["--live-reattach"] } else { &[] };
        let mut run = Run::start(
            dir.path(),
            &request(dir.path(), &executable, flags, "echo:first"),
        );
        let serve_pid = run.event("launched")["pid"].as_u64().unwrap();
        run.until("turn-end 0", |value| {
            value["event"] == "turn-end" && value["input"] == 0
        });
        run.child.kill().unwrap();
        run.child.wait().unwrap();
        run.stdin = None;
        drop(run);
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            !ended(serve_pid),
            "the harness outlives the entry and its owner"
        );
        let before = calls(dir.path()).len();

        let mut recovery = Run::recover(dir.path(), "continue-attached");
        let found = recovery.event("recovered-conversation");
        recovery.control(json!({ "cmd": "send", "text": "echo:after owner loss", "ref": "r-1" }));
        if declared {
            assert_eq!(found["state"], "live-usable", "{found}");
            assert_eq!(recovery.event("follow-up-admitted")["input"], 1);
            let reply = recovery.until("reply to input 1", |value| {
                value["event"] == "agent-message" && value["input"] == 1
            });
            assert_eq!(reply["text"], "after owner loss", "{reply}");
            recovery.until("turn-end 1", |value| {
                value["event"] == "turn-end" && value["input"] == 1
            });
        } else {
            assert_eq!(found["state"], "unavailable", "{found}");
            assert_eq!(found["reason"], "capability-absent");
            let refused = recovery.event("follow-up-refused");
            assert_eq!(refused["reason"], "conversation-unavailable", "{refused}");
        }
        recovery.control(json!({ "cmd": "close" }));
        let (terminal, code, seen) = recovery.end();
        assert_eq!(code, Some(87), "{terminal}\n{seen:#?}");
        let owner = seen
            .iter()
            .find(|value| value["event"] == "terminal")
            .unwrap();
        assert_eq!(owner["status"], "closed", "{owner}");
        assert!(
            seen.iter()
                .all(|value| value["event"] != "cancel-requested")
        );
        wait_ended(serve_pid, "resident harness");
        assert_eq!(
            calls(dir.path()).len(),
            before,
            "recovery ran no provider operation"
        );
        let peer: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("launch/provider/peer-state.json")).unwrap(),
        )
        .unwrap();
        let prompts = peer["prompts"].as_array().unwrap().len();
        assert_eq!(prompts, if declared { 2 } else { 1 }, "{peer}");
        assert_eq!(peer["launches"].as_array().unwrap().len(), 1, "{peer}");
    }
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

/// Fake natives record their own proc view and the SDK's running actor before
/// emitting provider-specific consumption evidence. They make no tool calls.
const NATIVE_FACTS: &str = r#"#!/usr/bin/python3
import glob, json, os, sys
args = sys.argv[1:]
pid = os.getpid()
stat = open('/proc/self/stat').read()
nspid = [line.split()[1:] for line in open('/proc/self/status') if line.startswith('NSpid:')][0]
actors = []
for path in glob.glob(os.environ['ACTOR_ROOT'] + '/**/*.json', recursive=True):
    try:
        record = json.load(open(path))
    except (ValueError, OSError):
        continue
    if isinstance(record, dict) and record.get('actor_id') == pid:
        actors.append({'path': path, 'incarnation': record.get('incarnation'), 'phase': record.get('phase')})
facts = {'argv': args, 'pid': pid, 'pgid': os.getpgid(0),
         'proc_self_link': os.readlink('/proc/self'), 'proc_self_pid': int(stat.split()[0]),
         'start_ticks': stat.rsplit(')', 1)[1].split()[19], 'nspid': nspid,
         'pidns': os.readlink('/proc/self/ns/pid'), 'boot_id': open('/proc/sys/kernel/random/boot_id').read().strip(),
         'actors': actors, 'tools': os.environ.get('OULIPOLY_TOOL_MEDIATION_V1'),
         'ingress': 'OULIPOLY_ROOT_BASH_V1' in os.environ, 'sentinel': os.environ.get('SENTINEL')}
def emit(event):
    print(json.dumps(event), flush=True)
def record(prompt, session):
    facts.update(prompt=prompt, native_session=session)
    with open(os.environ['CALLS'], 'a') as f:
        f.write(json.dumps(facts) + '\n')
"#;

const FAKE_NATIVE_CLAUDE: &str = r#"
message = json.loads(sys.stdin.readline())
prompt = message['message']['content'][0]['text']
options = {args[i]: args[i + 1] for i in range(len(args) - 1) if args[i] in ['--resume', '--session-id']}
session = options.get('--resume', options.get('--session-id'))
record(prompt, session)
emit({'type': 'system', 'subtype': 'init', 'session_id': session, 'model': 'fixture'})
emit({'type': 'user', 'uuid': message['uuid'], 'session_id': session, 'message': message['message']})
emit({'type': 'assistant', 'parent_tool_use_id': None, 'message': {'content': [{'type': 'text', 'text': 'reply to %s' % prompt}]}})
emit({'type': 'result', 'subtype': 'success', 'is_error': False, 'stop_reason': 'end_turn', 'session_id': session})
"#;

const FAKE_NATIVE_CODEX: &str = r#"
prompt = sys.stdin.read()
session = args[args.index('resume') + 1] if 'resume' in args else '11111111-2222-4333-8444-555555555555'
# A genuine adapter prerequisite for resume is a rollout in this fake account.
root = os.path.join(os.environ['CODEX_HOME'], 'sessions')
os.makedirs(root, mode=0o700, exist_ok=True)
with open(os.path.join(root, 'rollout-fixture.jsonl'), 'w') as f:
    f.write(json.dumps({'type': 'session_meta', 'payload': {'id': session, 'cwd': os.getcwd()}}) + '\n')
record(prompt, session)
emit({'type': 'thread.started', 'thread_id': session})
emit({'type': 'turn.started'})
emit({'type': 'item.completed', 'item': {'type': 'agent_message', 'text': 'reply to %s' % prompt}})
emit({'type': 'turn.completed', 'usage': {'input_tokens': 1, 'output_tokens': 1}})
"#;

/// Copies a finished scenario's directory (request, provider calls,
/// prepared configurations, store) to `OULIPOLY_TEST_RETAIN_DIR/<name>`
/// when that is set, for later reading.
fn retain(dir: &Path, name: &str) {
    if let Some(root) = std::env::var_os("OULIPOLY_TEST_RETAIN_DIR") {
        let to = Path::new(&root).join(name);
        let status = Command::new("cp")
            .args(["-a", "--no-dereference"])
            .arg(dir)
            .arg(&to)
            .status()
            .unwrap();
        println!("retained {name} at {}: {status}", to.display());
    }
}

/// A registered parent with one registered child route, the child being
/// the same stand-in provider, offered through `oulipoly-root-child`.
fn exploring(dir: &Path, executable: &Path, message: &str) -> Value {
    let mut value = request(dir, executable, &[], message);
    value["provider"]["root_child_bin"] = json!(sibling("oulipoly-root-child"));
    value["children"] = json!({
        "routes": { "luna": { "registered": {
            "executable": executable,
            "settings": value["provider"]["settings"].clone(),
            "env": { "PROVIDER_ONLY": "1" },
            "agent_bash_bin": value["provider"]["agent_bash_bin"].clone(),
        }, "slots": 2 } },
        "max_starts": 2, "max_concurrent": 1,
    });
    value
}

/// G2 join, as stand-in native and configuration evidence only: the
/// stand-in provider and deterministic peer stand for a native CLI and
/// provider bridge. A registered parent with child routes selects
/// exploration and is offered exactly its configured route, the named
/// requester and the owner's ingress; its prepared configuration carries
/// that offer. Asking through that requester reaches the owner, which
/// admits prepared slots in order, refuses the child's own request (depth)
/// and refuses beyond the root's starts. The child's describe, policy and
/// prepare select and carry no exploration, and its serving process and
/// configuration carry no offer.
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash, oulipoly-root-child and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn registered_parent_explores_its_offered_route_through_the_owner() {
    let dir = scratch();
    let executable = provider(dir.path(), "");
    let root_child = sibling("oulipoly-root-child");
    let mut run = Run::start(
        dir.path(),
        &exploring(
            dir.path(),
            &executable,
            "explore:luna:explore:luna:grandchild",
        ),
    );
    let described = run.entry("provider-described");
    assert_eq!(described["exploration"], 1, "{described}");
    let child_described = run.until("child described", |value| {
        value["entry"] == "provider-described" && value["child_route"] == "luna"
    });
    assert_eq!(child_described["exploration"], Value::Null);
    let setup = run.entry("setup-completed");
    let launch = &setup["launch"];
    let offer = &launch["exploration"];
    assert_eq!(offer["protocol"], "oulipoly.exploration/v1", "{setup}");
    assert_eq!(offer["routes"], json!(["luna"]));
    assert_eq!(offer["requester"], root_child.to_string_lossy().as_ref());
    assert_eq!(offer["ingress_env"], "OULIPOLY_ROOT_BASH_V1");
    assert_eq!(
        offer["limits"],
        json!({ "max_starts": 2, "max_concurrent": 1 })
    );
    assert_eq!(launch["exploration_version"], 1);
    assert_eq!(launch["effective_exploration"]["tool"], "fake_explore");
    assert_eq!(launch["effective_exploration"]["routes"], json!(["luna"]));
    assert!(
        launch["effective_mediation"]["native_tools"]
            .as_array()
            .unwrap()
            .contains(&json!("fake_explore"))
    );
    assert!(
        launch["exploration_evidence"]
            .as_str()
            .unwrap()
            .contains("not admission")
    );
    let child_receipt = &setup["children"]["luna"];
    assert!(
        child_receipt["exploration"]
            .as_str()
            .unwrap()
            .starts_with("none offered or selected"),
        "{child_receipt}"
    );
    assert_eq!(child_receipt["slots"].as_array().unwrap().len(), 2);

    let parent = "stand-in-external";
    let reply = |run: &mut Run, input: u64| {
        let message = run.until("parent reply", move |value| {
            value["harness"] == parent
                && value["event"] == "agent-message"
                && value["input"] == input
        });
        run.until("parent turn end", move |value| {
            value["harness"] == parent && value["event"] == "turn-end" && value["input"] == input
        });
        message["text"].as_str().unwrap().to_owned()
    };
    // The child answered; its answer is its own refused request (depth).
    let first = reply(&mut run, 0);
    assert!(first.starts_with("exit=Some(0)"), "{first}");
    assert!(first.contains(r#""outcome":"answered""#), "{first}");
    assert!(
        first.contains("exit=Some(65)") && first.contains("depth"),
        "{first}"
    );
    run.control(json!({ "cmd": "send", "text": "explore:luna:echo:second", "ref": "e2" }));
    let second = reply(&mut run, 1);
    assert!(second.starts_with("exit=Some(0)"), "{second}");
    assert!(second.contains(r#""answer":"second""#), "{second}");
    // Beyond the root's starts: refused by the owner, nothing admitted.
    run.control(json!({ "cmd": "send", "text": "explore:luna:echo:third", "ref": "e3" }));
    let third = reply(&mut run, 2);
    assert!(third.starts_with("exit=Some(65)"), "{third}");
    run.control(json!({ "cmd": "close" }));
    let (terminal, code, seen) = run.end();
    assert_eq!(code, Some(87), "{terminal}");
    let refusals: Vec<&Value> = seen
        .iter()
        .filter(|value| value["event"] == "child-refused")
        .collect();
    assert_eq!(refusals.len(), 2, "{refusals:?}");
    let reason = |index: usize| refusals[index]["reason"].as_str().unwrap();
    assert!(reason(0).starts_with("depth"), "{refusals:?}");
    assert_ne!(refusals[0]["harness"], parent);
    assert!(reason(1).starts_with("budget-starts"), "{refusals:?}");
    assert_eq!(refusals[1]["harness"], parent);
    let results: Vec<&Value> = seen
        .iter()
        .filter(|value| value["event"] == "child-result")
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|result| result["outcome"] == "answered"));

    // Provider operations: the parent's three select exploration and its
    // policy and prepare carry the offer; the child's four select nothing.
    let calls = calls(dir.path());
    let ops: Vec<&str> = calls
        .iter()
        .map(|call| call["op"].as_str().unwrap())
        .collect();
    assert_eq!(
        ops,
        [
            "describe",
            "policy.evaluate",
            "resident.prepare",
            "describe",
            "policy.evaluate",
            "resident.prepare",
            "resident.prepare",
            "resident.serve",
            "resident.serve",
            "resident.serve",
        ]
    );
    for call in &calls[..3] {
        assert_eq!(
            call["request"]["host"]["env"]["OULIPOLY_HOST_EXPLORATION_V1"],
            "1"
        );
    }
    for call in &calls[1..3] {
        let carried: Value = serde_json::from_str(
            call["request"]["params"]["launch"]["env"]["OULIPOLY_EXPLORATION_V1"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(&carried, offer);
    }
    for call in &calls[3..7] {
        let text = call["request"].to_string();
        assert!(!text.contains("EXPLORATION"), "child operation: {text}");
    }
    // Serving: the parent's configuration carries the offer; no serving
    // process (parent or children) has it in its environment, and neither
    // child's configuration has it.
    let serves = &calls[7..];
    let parent_serve = serves
        .iter()
        .find(|serve| {
            serve["argv"][2]
                .as_str()
                .unwrap()
                .contains("/launch/provider/")
        })
        .unwrap();
    let served: Value =
        serde_json::from_str(parent_serve["config_offer"].as_str().unwrap()).unwrap();
    assert_eq!(&served, offer);
    let child_serves: Vec<&Value> = serves
        .iter()
        .filter(|serve| {
            serve["argv"][2]
                .as_str()
                .unwrap()
                .contains("/child-slots/luna/")
        })
        .collect();
    assert_eq!(child_serves.len(), 2);
    for serve in serves {
        assert_eq!(serve["process_offer"], Value::Null, "{serve}");
        assert!(
            serve["env"]
                .as_array()
                .unwrap()
                .contains(&json!("OULIPOLY_ROOT_BASH_V1"))
        );
    }
    for serve in &child_serves {
        assert_eq!(serve["config_offer"], Value::Null, "{serve}");
    }
    // Durable owner records: two children on the route, in slot order.
    let db = rusqlite::Connection::open_with_flags(
        dir.path().join("store/intent.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let rows: Vec<(String, String)> = db
        .prepare("SELECT route, outcome FROM child ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        rows,
        [
            ("luna".to_owned(), "answered".to_owned()),
            ("luna".to_owned(), "answered".to_owned())
        ]
    );
    drop(db);
    retain(dir.path(), "explores-offered-route");
}

/// Explicit refusals of the exploration join, with what ran: a parent with
/// routes but no requester, a caller's own offer in the root's environment
/// or a child's settings run nothing (64); a provider without exploration
/// is refused after describe alone, and a marker contradicting the offer
/// after policy, before any setup (65).
#[test]
#[ignore = "needs oulipoly-root-supervisor, oulipoly-root-pid1, oulipoly-root-bash, oulipoly-root-child and oulipoly-acp-deterministic-peer built beside the Runner (cargo build -p oulipoly-root-supervisor --bins) and unprivileged user namespaces"]
fn exploration_join_refusals_say_what_ran() {
    let refuse = |name: &str, edit: &dyn Fn(&mut Value), python: &str, code: i32, ran: &[&str]| {
        let dir = scratch();
        let executable = provider(dir.path(), python);
        let mut value = exploring(dir.path(), &executable, "echo:never");
        edit(&mut value);
        let run = Run::start(dir.path(), &value);
        let (terminal, status, _) = run.end();
        assert_eq!(status, Some(code), "{name}: {terminal}");
        assert_eq!(ops(dir.path()), ran, "{name}");
        assert!(!dir.path().join("launch").exists(), "{name}");
        assert!(!dir.path().join("store").exists(), "{name}");
        retain(dir.path(), &format!("refusal-{name}"));
        terminal
    };
    let missing = refuse(
        "no-requester",
        &|value| {
            value["provider"]
                .as_object_mut()
                .unwrap()
                .remove("root_child_bin");
        },
        "",
        64,
        &[],
    );
    assert!(
        missing["reason"]
            .as_str()
            .unwrap()
            .contains("root_child_bin"),
        "{missing}"
    );
    let ambient = refuse(
        "root-env-offer",
        &|value| value["env"]["OULIPOLY_EXPLORATION_V1"] = json!("{}"),
        "",
        64,
        &[],
    );
    assert!(
        ambient["reason"].as_str().unwrap().contains("to offer"),
        "{ambient}"
    );
    let child = refuse(
        "child-settings-offer",
        &|value| {
            value["children"]["routes"]["luna"]["registered"]["settings"]["launch"]["env"] =
                json!({ "OULIPOLY_EXPLORATION_V1": "{}" })
        },
        "",
        64,
        &[],
    );
    assert!(
        child["reason"]
            .as_str()
            .unwrap()
            .contains("this entry's to set"),
        "{child}"
    );
    let undeclared = refuse(
        "no-exploration",
        &|_| {},
        "caps.pop('exploration_v1', None)",
        65,
        &["describe"],
    );
    assert_eq!(undeclared["operation"], "describe");
    assert!(
        undeclared["reason"]
            .as_str()
            .unwrap()
            .contains("declares no exploration"),
        "{undeclared}"
    );
    let contradicted = refuse(
        "marker-routes",
        &|_| {},
        "if op == 'policy.evaluate':\n    markers[-1]['value']['routes'] = ['other']",
        65,
        &["describe", "policy.evaluate"],
    );
    assert_eq!(contradicted["operation"], "policy.evaluate");
    assert!(
        contradicted["reason"]
            .as_str()
            .unwrap()
            .contains("effective exploration"),
        "{contradicted}"
    );
}

fn real_adapter(variable: &str) -> PathBuf {
    let path = std::env::var(variable)
        .unwrap_or_else(|_| panic!("{variable}: a built provider adapter executable"));
    PathBuf::from(path)
}

/// Copies a completed, discharged fixture only when a qualification caller
/// asks to retain it. Production Runner never reads these native schemas.
fn retain_turn_fixture(source: &Path, name: &str) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else if entry.file_type().unwrap().is_file() {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    if let Some(root) = std::env::var_os("OULIPOLY_REGISTERED_TURN_EVIDENCE") {
        let root = PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        copy(source, &root.join(name));
    }
}

/// Both real adapter executables progress through registered administrative
/// policy/prepare to two actual tool-free fake-native turns and clean close.
/// The follow-up resumes the same native identity. Each running SDK actor
/// matches the native's own boot/start and namespace-local proc facts.
#[test]
#[ignore = "needs OULIPOLY_REAL_{CLAUDE,CODEX}_ADAPTER, OULIPOLY_REAL_CODEX_MODELS, owner binaries beside Runner and unprivileged user namespaces"]
fn real_adapters_serve_registered_turns_with_matching_native_actors() {
    for (variable, id, fake) in [
        ("OULIPOLY_REAL_CLAUDE_ADAPTER", "claude", FAKE_NATIVE_CLAUDE),
        ("OULIPOLY_REAL_CODEX_ADAPTER", "codex", FAKE_NATIVE_CODEX),
    ] {
        for allow in [false, true] {
            let adapter = real_adapter(variable);
            let dir = scratch();
            let native = dir.path().join(id);
            std::fs::write(&native, format!("{NATIVE_FACTS}{fake}")).unwrap();
            std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o755)).unwrap();
            let home = dir.path().join("home");
            std::fs::create_dir(&home).unwrap();
            let mut value = request(dir.path(), &adapter, &[], "first");
            if allow {
                value["provider"]
                    .as_object_mut()
                    .unwrap()
                    .remove("bash_authority");
                value["provider"]["bash_allow"] = json!(["printf allowed"]);
            }
            let env = json!({"CALLS": dir.path().join("native-calls"),
                "ACTOR_ROOT": dir.path().join("launch/provider"), "SENTINEL": "opaque-env"});
            value["provider"]["env"] = json!({"HOME": home});
            value["provider"]["settings"] = if id == "claude" {
                json!({"settings_id": "claude-witness", "mode": "headless",
                    "model": {"name": "claude-opus", "provider_args": ["--model", "opus"], "inputs": {"prompt": null, "named": {}}},
                    "launch": {"command": native, "prompt_mode": "stdin", "env": env}})
            } else {
                let config = dir.path().join("config/agent-runner-codex");
                std::fs::create_dir_all(&config).unwrap();
                let prompt = dir.path().join("system.md");
                std::fs::write(&prompt, "tool-free fixture\n").unwrap();
                std::fs::copy(
                    std::env::var("OULIPOLY_REAL_CODEX_MODELS").unwrap(),
                    dir.path().join("models.json"),
                )
                .unwrap();
                // Deliberately absent legacy dependencies: selected mediation
                // requires only native, prompt and the managed model catalog.
                std::fs::write(config.join("config.toml"), format!(
                    "codex_bin = {:?}\nbun_bin = {:?}\nbash_mcp_path = {:?}\nsystem_prompt_file = {:?}\nagent_bash_bin = {:?}\nagent_runner_bin = {:?}\n",
                    native, dir.path().join("missing-bun"), dir.path().join("missing-mcp.ts"), prompt,
                    dir.path().join("missing-agent-bash"), dir.path().join("missing-runner"))).unwrap();
                value["provider"]["config_root"] = json!(dir.path().join("config"));
                json!({"settings_id": "codex2", "mode": "stdin",
                    "model": {"name": "gpt-astra-high", "provider_args": ["-m", "gpt-6-astra", "-c", "model_reasoning_effort=\"high\""], "inputs": {"prompt": "prepare sentinel", "named": {}}},
                    "launch": {"argv": ["codex2", "exec", "--dangerously-bypass-approvals-and-sandbox", "-m", "gpt-6-astra", "-c", "model_reasoning_effort=\"high\""], "env": env}})
            };
            let mut run = Run::start(dir.path(), &value);
            let described = run.entry("provider-described");
            assert_eq!(described["provider_id"], id, "{described}");
            assert_eq!(described["agreed_contract"], "oulipoly.provider/v1");
            assert_eq!(described["resident_session"], 1);
            assert_eq!(described["tool_mediation"], 1);
            let setup = run.entry("setup-completed");
            let launch = &setup["launch"];
            assert_eq!(
                launch["argv"][0],
                adapter.to_string_lossy().as_ref(),
                "{setup}"
            );
            assert_eq!(launch["argv"][1], "resident.serve");
            assert_eq!(launch["acp"]["protocol_version"], 2);
            let config = launch["argv"][3].as_str().unwrap();
            assert!(config.starts_with(dir.path().join("launch/provider").to_str().unwrap()));
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir.path().join("launch/provider")), 0o700);
            assert_eq!(mode(Path::new(config).parent().unwrap()), 0o700);
            let recorded: Value =
                serde_json::from_str(&std::fs::read_to_string(config).unwrap()).unwrap();
            let policy = launch["tools"].clone();
            let encoded = recorded["launch"]["env"]["OULIPOLY_TOOL_MEDIATION_V1"]
                .as_str()
                .unwrap();
            assert_eq!(serde_json::from_str::<Value>(encoded).unwrap(), policy);
            let bash = if allow {
                json!({"allow": ["printf allowed"]})
            } else {
                json!({"authority": "trusted-task"})
            };
            assert_eq!(policy["bash"], bash);
            assert_eq!(launch["effective_mediation"]["bash"], bash);
            assert_eq!(policy["requester"], value["provider"]["agent_bash_bin"]);
            assert_eq!(recorded["launch"]["env"]["SENTINEL"], "opaque-env");
            let launched = run.event("launched");
            assert_eq!(launched["harness"], id);
            let serve_pid = launched["pid"].as_u64().unwrap();
            let harness_ns = std::fs::read_link(format!("/proc/{serve_pid}/ns/pid")).unwrap();
            let negotiated = run.event("negotiated");
            assert_eq!(negotiated["dedup_contract"], true);
            let opened = run.event("session-opened");
            let mut turns = Vec::new();
            for (index, prompt) in [(0, "first"), (1, "second")] {
                if index == 1 {
                    run.control(json!({"cmd": "send", "text": prompt, "ref": "f1"}));
                    assert_eq!(run.event("follow-up-admitted")["input"], 1);
                }
                let ack = run.until("native ACK", |v| v["event"] == "ack" && v["index"] == index);
                let reply = run.until("attributed reply", |v| {
                    v["event"] == "agent-message" && v["input"] == index
                });
                let turn = run.until("attributed turn end", |v| {
                    v["event"] == "turn-end" && v["input"] == index
                });
                assert_eq!(
                    reply["text"].as_str().unwrap().trim_end(),
                    format!("reply to {prompt}")
                );
                assert_eq!(ack["harness"], id);
                for event in [&reply, &turn] {
                    assert_eq!(event["harness"], id);
                    assert_eq!(event["session"], opened["session"]);
                }
                assert_eq!(reply["parent_message_id"], ack["message_id"]);
                assert_eq!(turn["message_id"], ack["message_id"]);
                assert_eq!(turn["own_output"], true);
                turns.push(turn);
            }
            assert_eq!(turns[1]["session"], turns[0]["session"]);
            run.control(json!({"cmd": "close"}));
            let (terminal, code, seen) = run.end();
            assert_eq!(code, Some(87), "{terminal}");
            let owner = seen.iter().find(|v| v["event"] == "terminal").unwrap();
            assert_eq!(owner["status"], "closed");
            assert_eq!(owner["owed"], 0);
            assert_eq!(owner["all_harnesses_reaped"], true);
            assert_eq!(owner["root_pid1"]["end_observed"], true);
            assert_eq!(owner["root_pid1"]["status"], "code:0");
            assert_eq!(owner["root_pid1"]["live"], json!([]));
            let exited = seen.iter().find(|v| v["event"] == "exited").unwrap();
            assert_eq!(exited["harness"], id);
            assert_eq!(exited["namespace"]["drained"], true);
            assert_eq!(exited["reaped"], "work-pid1-wait");
            assert!(ended(serve_pid), "harness still live after terminal");
            let natives: Vec<Value> = std::fs::read_to_string(dir.path().join("native-calls"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(natives.len(), 2, "{natives:?}");
            let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
            for (index, call) in natives.iter().enumerate() {
                println!("registered-native adapter={id} allow={allow} input={index}: {call}");
                let pid = call["pid"].as_i64().unwrap();
                assert_eq!(call["proc_self_link"], pid.to_string());
                assert_eq!(call["proc_self_pid"], pid);
                assert_eq!(call["nspid"], json!([pid.to_string()]));
                assert_eq!(Path::new(call["pidns"].as_str().unwrap()), harness_ns);
                assert_eq!(call["pgid"], pid);
                assert_eq!(call["boot_id"], boot.trim());
                let actors = call["actors"].as_array().unwrap();
                assert_eq!(actors.len(), 1, "{call}");
                assert_eq!(actors[0]["phase"], "running");
                let settled: Value = serde_json::from_str(
                    &std::fs::read_to_string(actors[0]["path"].as_str().unwrap()).unwrap(),
                )
                .unwrap();
                assert_eq!(settled["phase"], "complete");
                assert_eq!(settled["exit_code"], 0);
                assert!(settled["actor_id"].is_null());
                assert!(settled["incarnation"].is_null());
                assert_eq!(
                    actors[0]["incarnation"],
                    format!(
                        "linux:{}:{}",
                        boot.trim(),
                        call["start_ticks"].as_str().unwrap()
                    )
                );
                assert_eq!(
                    serde_json::from_str::<Value>(call["tools"].as_str().unwrap()).unwrap(),
                    policy
                );
                assert_eq!(call["ingress"], true);
                assert_eq!(call["sentinel"], "opaque-env");
                assert_eq!(call["prompt"], if index == 0 { "first" } else { "second" });
            }
            assert_ne!(
                natives[1]["actors"][0]["path"],
                natives[0]["actors"][0]["path"]
            );
            assert!(!natives[0]["native_session"].as_str().unwrap().is_empty());
            assert_eq!(natives[1]["native_session"], natives[0]["native_session"]);
            let argv = natives[1]["argv"].as_array().unwrap();
            let resume = if id == "claude" { "--resume" } else { "resume" };
            let at = argv
                .iter()
                .position(|arg| arg == resume)
                .expect("follow-up resumes");
            assert_eq!(argv[at + 1], natives[0]["native_session"]);
            std::fs::write(
                dir.path().join("events.json"),
                serde_json::to_vec_pretty(&seen).unwrap(),
            )
            .unwrap();
            retain_turn_fixture(
                dir.path(),
                &format!("{id}-{}", if allow { "allow" } else { "trusted" }),
            );
        }
    }
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
