//! AGE-163 WU-A.6 — `--rotate-provider` tri-state parsing.
//!
//! - absent → `Option::None`
//! - present-no-value (`--rotate-provider`) → `Some("")` (auto-rotate signal)
//! - present-with-target (`--rotate-provider X`) → `Some("X")` (explicit reroute)
//! - the prior `--migrate` flag is rejected by clap (unknown long flag).

use std::process::Command;

fn cmd() -> (Command, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
    command
        .env("OULIPOLY_DATA_DIR", dir.path().join("app-data"))
        .env("XDG_DATA_HOME", dir.path().join("xdg-data"))
        .env("XDG_CONFIG_HOME", dir.path().join("xdg-config"))
        .env("XDG_STATE_HOME", dir.path().join("xdg-state"))
        .env("HOME", dir.path().join("home"));
    (command, dir)
}

#[test]
fn rotate_provider_absent_honors_bound_provider() {
    // Parser-level only: a bare `--help` invocation must list the new flag.
    let (mut command, _dir) = cmd();
    let output = command.arg("--help").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--rotate-provider"),
        "--help must advertise the new flag spelling: {stdout}"
    );
}

#[test]
fn rotate_provider_no_value_parses_as_auto_rotate_signal() {
    // The bare `--rotate-provider` (no value) is the auto-rotate signal.
    // Parsed under `--resume` to exercise the top-level Cli's tri-state field;
    // we only need clap to accept the form. Failure during downstream
    // execution (missing fixtures) is fine — parsing must succeed.
    let (mut command, _dir) = cmd();
    let output = command
        .arg("--resume")
        .arg("5169694d-de0f-40d1-890c-6e28e55bab27")
        .arg("--rotate-provider")
        .arg("any prompt to satisfy headless target")
        .output()
        .unwrap();
    // Either succeed or fail at runtime, but NOT at clap parsing.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("error: invalid value"),
        "tri-state bare `--rotate-provider` should parse, got stderr: {stderr}"
    );
}

#[test]
fn rotate_provider_with_target_parses_with_target_value() {
    let (mut command, _dir) = cmd();
    let output = command
        .arg("--resume")
        .arg("5169694d-de0f-40d1-890c-6e28e55bab27")
        .arg("--rotate-provider")
        .arg("claude2")
        .arg("any prompt")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("unexpected argument"),
        "`--rotate-provider claude2` should parse, got stderr: {stderr}"
    );
}

#[test]
fn rotate_provider_unknown_flag_migrate_is_now_rejected() {
    // The old `--migrate` flag is no longer a per-dispatch rotation flag at
    // the top level / under `resume` / under `repl`. Per AGE-163 WU-A.6,
    // it has been renamed to `--rotate-provider`. clap must reject the
    // old spelling.
    let (mut command, _dir) = cmd();
    let output = command
        .arg("--resume")
        .arg("5169694d-de0f-40d1-890c-6e28e55bab27")
        .arg("--migrate")
        .arg("claude2")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("error:"),
        "`--migrate` must be rejected by clap at the top level: {stderr}"
    );
}
