//! The Runner's `native-root` entry given a registered external provider
//! instead of an embedded harness (Linux). The provider is a deterministic
//! stand-in executable speaking the provider contract's `describe`: no
//! native harness, model, credential or network. Needs nothing built
//! beside the Runner: the entry resolves the provider before it looks for
//! its owner.
#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

const RUNNER: &str = env!("CARGO_BIN_EXE_oulipoly-agent-runner");
const CONTRACT: &str = "oulipoly.provider/v1";

/// Writes a stand-in provider that records each operation it is asked for
/// in `<dir>/calls`, then prints `response` (or exits 1 when `None`).
fn provider(dir: &Path, response: Option<Value>) -> PathBuf {
    let path = dir.join("provider");
    let answer = match response {
        Some(response) => format!("printf '%s\\n' '{response}'"),
        None => "exit 1".to_owned(),
    };
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\ncat > /dev/null\n{answer}\n",
            dir.join("calls").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn described(provider_id: &str) -> Value {
    json!({
        "contract": CONTRACT,
        "request_id": "native-root-describe",
        "ok": true,
        "result": {
            "provider_id": provider_id,
            "display_name": "Stand-in external provider",
            "contract_versions": [CONTRACT],
            "preferred_contract": CONTRACT,
            "capabilities": {
                "launch": true, "policy": true, "quota": false, "session": true,
                "terminal": true, "rotation": false, "discovery": false,
                "settings": false, "setup_brain": false, "setup": false,
                "migration": false,
            },
        },
    })
}

struct Outcome {
    code: Option<i32>,
    lines: Vec<Value>,
    calls: String,
}

/// Runs the entry on a fresh root request naming `harness` fields.
fn run(dir: &Path, harness: Value) -> Outcome {
    let mut request = json!({
        "store": dir.join("store"),
        "launch_dir": dir.join("launch"),
        "cwd": dir,
        "env": { "PATH": "/usr/bin:/bin" },
        "messages": ["hello"],
        "outage_closure_cap": 1,
        "delivery_attempt_cap": 1,
        "workload": { "isolation": "unprivileged-userns" },
    });
    for (name, value) in harness.as_object().unwrap() {
        request[name] = value.clone();
    }
    let path = dir.join("request.json");
    std::fs::write(&path, request.to_string()).unwrap();
    let output = Command::new(RUNNER)
        .args(["native-root", "--request"])
        .arg(&path)
        .output()
        .unwrap();
    let lines = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        !dir.join("store").exists() && !dir.join("launch").exists(),
        "a refusal made the root's store or launch directory"
    );
    Outcome {
        code: output.status.code(),
        lines,
        calls: std::fs::read_to_string(dir.join("calls")).unwrap_or_default(),
    }
}

/// A described provider is named in a `provider-described` line, then
/// refused before any effect: provider/v1 cannot declare the resident
/// harness the root needs, and no embedded harness is taken instead.
#[test]
fn registered_provider_is_described_then_refused_without_an_embedded_harness() {
    let dir = tempfile::tempdir().unwrap();
    let executable = provider(dir.path(), Some(described("stand-in-external")));
    let outcome = run(
        dir.path(),
        json!({ "provider": { "executable": executable } }),
    );
    assert_eq!(outcome.code, Some(64), "{:?}", outcome.lines);
    assert_eq!(outcome.calls, "describe\n");
    assert_eq!(outcome.lines.len(), 2, "{:?}", outcome.lines);
    let described = &outcome.lines[0];
    assert_eq!(described["entry"], "provider-described");
    assert_eq!(described["provider_id"], "stand-in-external");
    assert_eq!(described["agreed_contract"], CONTRACT);
    assert_eq!(described["missing"], json!(["resident-acp-v2-harness"]));
    let terminal = &outcome.lines[1];
    assert_eq!(
        (&terminal["entry"], &terminal["stage"], &terminal["effects"]),
        (&json!("terminal"), &json!("refused"), &json!("none"))
    );
    let reason = terminal["reason"].as_str().unwrap();
    assert!(
        reason.contains("stand-in-external declares no resident-acp-v2-harness")
            && reason.contains("not substituted by an embedded harness"),
        "{reason}"
    );
}

/// Controls: a provider that cannot describe itself is refused as such,
/// with no description; a request naming an embedded harness as well never
/// runs the provider.
#[test]
fn registered_provider_refusals_name_their_cause_and_run_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let executable = provider(dir.path(), None);
    let failed = run(
        dir.path(),
        json!({ "provider": { "executable": executable } }),
    );
    assert_eq!(failed.code, Some(64));
    assert_eq!(failed.calls, "describe\n");
    assert_eq!(failed.lines.len(), 1, "{:?}", failed.lines);
    let reason = failed.lines[0]["reason"].as_str().unwrap();
    assert!(reason.contains("describe failed"), "{reason}");

    let dir = tempfile::tempdir().unwrap();
    let executable = provider(dir.path(), Some(described("stand-in-external")));
    let both = run(
        dir.path(),
        json!({
            "provider": { "executable": executable },
            "opencode": {
                "deps": "/d", "agent_bash_tool": "/t", "agent_bash_bin": "/b",
                "bash_allow": ["true"],
            },
        }),
    );
    assert_eq!(both.code, Some(64));
    assert_eq!(both.calls, "", "the provider was run");
    let reason = both.lines[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("exactly one of opencode, claude and provider"),
        "{reason}"
    );
}
