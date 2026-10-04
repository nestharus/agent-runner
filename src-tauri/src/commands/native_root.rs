//! `native-root`: start one fresh native OpenCode ACP v2 root (Linux,
//! opt-in, source build).
//!
//! The request file names everything the root gets: a new launch directory
//! and a new store, the native setup inputs, the messages, and **the whole
//! environment** of the root. The per-root owner (`oulipoly-root-supervisor`,
//! next to this binary) is started with only that environment; nothing of
//! this process's environment passes through. Root PID 1 inherits it, and
//! in-root Bash work runs in it unchanged. The native host gets it too, less
//! what its launch argv removes or overrides (its own HOME and XDG
//! directories, among others), plus the owner's ingress and socket
//! variables. Stdout says which names reach where, never a value.
//!
//! Stdout carries JSON lines. This entry's own lines have an `entry` key;
//! every other line is the owner's, relayed unchanged. Stdin lines after
//! start go to the owner unchanged (its controls; `{"cmd":"cancel"}`), and
//! stdin EOF closes the owner's stdin, which is not a cancel.
//!
//! Exit status, one meaning each:
//!
//! * `0`: the owner ended (`ended`): every harness's end observed, nothing owed.
//! * `82`, `83`, `84`, `85`: the owner's own class 2 to 5 (`cancelled`,
//!   `ended-owed`, `incomplete` or `owned-unattached`, `authority-lost` or
//!   `store-failed`). Owed work stays in the store, for an explicit recovery
//!   by its owner, never by replaying this request.
//! * `64`: the request was refused before any effect.
//! * `73`: setup construction failed: the launch directory may hold partial
//!   effects. Do not replay.
//! * `66`: setup completed, then the owner refused the request or its store
//!   (its 64 or 65): the launch directory stays. Do not replay.
//! * `69`: setup completed, and the owner process could not be started.
//! * `70`: the owner was started but ended without a known class (a signal
//!   or another status): the store says what happened.
//! * `74`: this entry could not write to its stdout: its lines, the owner's
//!   included, were not all delivered, whatever the owner's end.
//!
//! An `ack` is insertion, an `idle` is readiness and a native error is a
//! native result; none of them is completion of processing, and this entry
//! adds no such claim. It does not recover, cancel by itself, retry or
//! select accounts, and it touches no Runner state.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oulipoly_root_supervisor::native::{
    OpenCodeSetup, OpenCodeSetupError, REMOVED_ENV, provision_opencode,
};
use oulipoly_root_supervisor::{Endpoint, HarnessSpec, Intent, Request};
use serde::Deserialize;
use serde_json::{Map, Value, json};

const OWNER_BINARY: &str = "oulipoly-root-supervisor";

const EXIT_REFUSED: i32 = 64;
const EXIT_OWNER_REFUSED: i32 = 66;
const EXIT_OWNER_NOT_STARTED: i32 = 69;
const EXIT_OWNER_UNKNOWN: i32 = 70;
const EXIT_SETUP_FAILED: i32 = 73;
const EXIT_RELAY_FAILED: i32 = 74;
/// Added to the owner's own nonzero classes so none reads as a Runner or
/// argument error.
const OWNER_CLASS_BASE: i32 = 80;

/// The request file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRootRequest {
    /// Absolute path of the root's store; must not exist.
    store: String,
    /// Absolute path of the native host's launch directory; must not exist.
    launch_dir: String,
    /// Absolute working directory of the root's harness.
    cwd: String,
    /// The root's whole environment (see the module docs).
    env: BTreeMap<String, String>,
    /// Messages delivered to the native host, in order.
    messages: Vec<String>,
    outage_closure_cap: u32,
    delivery_attempt_cap: u32,
    /// The native setup inputs (`deps`, `agent_bash_tool`, `agent_bash_bin`,
    /// `bash_allow`, optional `model` and `provider`).
    opencode: NativeSetup,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeSetup {
    deps: String,
    agent_bash_tool: String,
    agent_bash_bin: String,
    bash_allow: Vec<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<Map<String, Value>>,
}

/// Where this entry's own and relayed lines go.
struct Out {
    failed: AtomicBool,
}

impl Out {
    fn line(&self, line: &str) {
        if self.failed.load(Ordering::SeqCst) {
            return;
        }
        let mut out = io::stdout().lock();
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
    }

    fn entry(&self, value: Value) {
        self.line(&value.to_string());
    }

    fn exit(&self, code: i32) -> i32 {
        if self.failed.load(Ordering::SeqCst) {
            EXIT_RELAY_FAILED
        } else {
            code
        }
    }
}

pub(crate) fn run(request_path: &Path) -> Result<i32, String> {
    let out = Arc::new(Out {
        failed: AtomicBool::new(false),
    });
    let refused = |reason: String| {
        out.entry(json!({
            "entry": "terminal",
            "stage": "refused",
            "reason": reason,
            "effects": "none",
        }));
        Ok(out.exit(EXIT_REFUSED))
    };
    let request = match read_request(request_path) {
        Ok(request) => request,
        Err(reason) => return refused(reason),
    };
    let owner = match owner_binary() {
        Ok(owner) => owner,
        Err(reason) => return refused(reason),
    };
    let setup = OpenCodeSetup {
        dir: request.launch_dir.clone(),
        deps: request.opencode.deps.clone(),
        agent_bash_tool: request.opencode.agent_bash_tool.clone(),
        agent_bash_bin: request.opencode.agent_bash_bin.clone(),
        bash_allow: request.opencode.bash_allow.clone(),
        model: request.opencode.model.clone(),
        provider: request.opencode.provider.clone(),
    };
    // The owner's own checks first, so its refusal cannot follow setup's
    // effects. They read only that the argv is non-empty; every
    // provisioned argv is.
    if let Err(reason) = owner_request(&request, vec!["/usr/bin/env".to_owned()]).validate() {
        return refused(format!("owner request: {reason}"));
    }
    let launch = match provision_opencode(&setup) {
        Ok(launch) => launch,
        Err(OpenCodeSetupError::InputInvalid(reason)) => {
            return refused(format!("setup: {reason}"));
        }
        Err(OpenCodeSetupError::ConstructionFailed(reason)) => {
            out.entry(json!({
                "entry": "terminal",
                "stage": "setup-failed",
                "reason": reason,
                "launch_dir": request.launch_dir,
                "effects": "possible",
                "retry": "do-not-replay",
            }));
            return Ok(out.exit(EXIT_SETUP_FAILED));
        }
    };
    let launch_names: Vec<&str> = launch.env.iter().map(|(name, _)| name.as_str()).collect();
    out.entry(json!({
        "entry": "setup-completed",
        "launch": launch.to_json(),
        "owner": owner,
        "env": env_reach(&request.env, &launch_names),
    }));
    let owner_request = owner_request(&request, launch.argv.clone());
    let line = serde_json::to_string(&owner_request).map_err(|error| error.to_string())?;
    let mut child = match Command::new(&owner)
        .env_clear()
        .envs(&request.env)
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            out.entry(json!({
                "entry": "terminal",
                "stage": "owner-not-started",
                "reason": error.to_string(),
                "launch_dir": request.launch_dir,
                "setup": "retained",
                "retry": "do-not-replay",
            }));
            return Ok(out.exit(EXIT_OWNER_NOT_STARTED));
        }
    };
    out.entry(json!({ "entry": "owner-started", "pid": child.id(), "store": request.store }));
    let mut stdin = child.stdin.take().expect("owner stdin");
    // A failed write means the owner is already gone; its exit says why.
    let delivered = writeln!(stdin, "{line}")
        .and_then(|()| stdin.flush())
        .is_ok();
    if delivered {
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if writeln!(stdin, "{line}")
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
    } else {
        drop(stdin);
    }
    // Every owner line is read to its end, delivered or not: an undrained
    // owner can delay its own cancel.
    let stdout = child.stdout.take().expect("owner stdout");
    for line in BufReader::new(stdout).lines() {
        match line {
            Ok(line) => out.line(&line),
            Err(_) => break,
        }
    }
    let status = child
        .wait()
        .map_err(|error| format!("owner wait: {error}"))?;
    let (stage, code) = match status.code() {
        Some(0) => ("owner-ended", 0),
        Some(class @ 2..=5) => ("owner-ended", OWNER_CLASS_BASE + class),
        Some(64 | 65) => ("owner-refused", EXIT_OWNER_REFUSED),
        _ => ("owner-outcome-unknown", EXIT_OWNER_UNKNOWN),
    };
    out.entry(json!({
        "entry": "terminal",
        "stage": stage,
        "owner_exit": status.code(),
        "owner_signal": std::os::unix::process::ExitStatusExt::signal(&status),
        "store": request.store,
        "launch_dir": request.launch_dir,
        "setup": "retained",
        "retry": "do-not-replay",
    }));
    Ok(out.exit(code))
}

fn read_request(path: &Path) -> Result<NativeRootRequest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("request: {error}"))?;
    let request: NativeRootRequest =
        serde_json::from_str(&text).map_err(|error| format!("request: {error}"))?;
    for (name, path) in [
        ("store", &request.store),
        ("launch_dir", &request.launch_dir),
    ] {
        if !path.starts_with('/') {
            return Err(format!("{name} must be absolute"));
        }
        // Fresh roots only: an existing path may be an earlier root's.
        if std::fs::symlink_metadata(path).is_ok() {
            return Err(format!("{name} exists; a root is started fresh only"));
        }
    }
    for (name, value) in &request.env {
        if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
            return Err(format!(
                "env name {name:?} or its value is not an environment entry"
            ));
        }
        if name == oulipoly_root_supervisor::bash::BASH_ENV
            || name == oulipoly_root_supervisor::SOCKET_ENV
        {
            return Err(format!("env {name} is the owner's to set"));
        }
    }
    if request.messages.is_empty() {
        return Err("messages names nothing to deliver".to_owned());
    }
    Ok(request)
}

fn owner_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("runner binary: {error}"))?;
    let owner = exe
        .parent()
        .ok_or("runner binary has no directory")?
        .join(OWNER_BINARY);
    if !owner.is_file() {
        return Err(format!(
            "no {OWNER_BINARY} next to this runner ({}); build it in the same target",
            owner.display()
        ));
    }
    Ok(owner)
}

fn owner_request(request: &NativeRootRequest, argv: Vec<String>) -> Request {
    Request {
        store: request.store.clone(),
        intent: Some(Intent {
            outage_closure_cap: request.outage_closure_cap,
            delivery_attempt_cap: request.delivery_attempt_cap,
            cwd: request.cwd.clone(),
            harnesses: vec![HarnessSpec {
                id: "opencode".to_owned(),
                argv,
                endpoint: Endpoint::UnixSocket,
                session: None,
                messages: request.messages.clone(),
            }],
        }),
    }
}

/// Which declared names reach where: names only, never values.
fn env_reach(declared: &BTreeMap<String, String>, launch: &[&str]) -> Value {
    let names: Vec<&str> = declared.keys().map(String::as_str).collect();
    let removed: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| REMOVED_ENV.contains(name))
        .collect();
    let overridden: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| launch.contains(name))
        .collect();
    let inherited: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !removed.contains(name) && !overridden.contains(name))
        .collect();
    json!({
        "declared": names,
        "ambient": "none",
        "owner_root_pid1_and_bash": names,
        "native_host": {
            "inherited": inherited,
            "removed_by_launch": removed,
            "overridden_by_launch": overridden,
            "set_by_launch": launch,
            "set_by_owner": [
                oulipoly_root_supervisor::bash::BASH_ENV,
                oulipoly_root_supervisor::SOCKET_ENV,
            ],
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_reach_names_what_the_native_launch_replaces_and_never_values() {
        let declared = BTreeMap::from([
            ("HOME".to_owned(), "/owner-home-secret-path".to_owned()),
            ("OPENCODE_CONFIG_CONTENT".to_owned(), "{}".to_owned()),
            ("TOKEN".to_owned(), "secret-value".to_owned()),
        ]);
        let reach = env_reach(&declared, &["HOME", "XDG_CONFIG_HOME"]);
        assert_eq!(
            reach["declared"],
            json!(["HOME", "OPENCODE_CONFIG_CONTENT", "TOKEN"])
        );
        assert_eq!(reach["native_host"]["inherited"], json!(["TOKEN"]));
        assert_eq!(
            reach["native_host"]["overridden_by_launch"],
            json!(["HOME"])
        );
        assert_eq!(
            reach["native_host"]["removed_by_launch"],
            json!(["OPENCODE_CONFIG_CONTENT"])
        );
        let text = reach.to_string();
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn request_refuses_existing_paths_owner_variables_and_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh");
        let base = |store: &Path, env: Value| {
            json!({
                "store": store,
                "launch_dir": dir.path().join("launch"),
                "cwd": "/",
                "env": env,
                "messages": ["m"],
                "outage_closure_cap": 1,
                "delivery_attempt_cap": 1,
                "opencode": {
                    "deps": "/d", "agent_bash_tool": "/t", "agent_bash_bin": "/b",
                    "bash_allow": ["true"],
                },
            })
        };
        let read = |value: Value| {
            let path = dir.path().join("request.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_request(&path)
        };
        assert!(read(base(&fresh, json!({ "PATH": "/usr/bin" }))).is_ok());
        let existing = read(base(dir.path(), json!({}))).unwrap_err();
        assert!(existing.contains("fresh only"), "{existing}");
        let owned = read(base(&fresh, json!({ "OULIPOLY_ROOT_BASH_V1": "/x" }))).unwrap_err();
        assert!(owned.contains("owner's to set"), "{owned}");
        let mut extra = base(&fresh, json!({}));
        extra["inherit_env"] = json!(true);
        assert!(read(extra).unwrap_err().contains("unknown field"));
    }
}
