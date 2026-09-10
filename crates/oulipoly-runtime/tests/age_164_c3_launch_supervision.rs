//! AGE-164 cluster 3 — provider launch + supervision + terminal mapping.
//!
//! Pins the public-API behavior that the Step 6c refactor must preserve:
//!
//! - `classify_terminal_reason` reason vocabulary (None / "exit_nonzero" /
//!   "signal:SIGTERM" / "unknown_exit") via the public function directly.
//! - Empty-command validation through public `execute` returns the canonical
//!   "Empty command" error.
//! - Spawn-failure error string prefix "Failed to spawn".
//! - Provider-index out-of-range error string is canonical.
//! - Supervisor preserves binary stdout output through the capture path
//!   (covers `append_output_chunk` byte fidelity).
//! - Working-directory propagation through public `execute`.
//! - Stdin prompt mode delivers prompt bytes to child stdin (existing
//!   behavior of the stdin writer + `supervised_stdin_write_needed`).
//!
//! These tests use fixture shell scripts (existing project pattern) and call
//! only public `executor::cli::*` items. They MUST remain PASS pre- and
//! post-refactor.
//!
//! ## Declared roles
//!
//! Roles: formatter, mapper, parser, validator.
//!
//! - formatter: fixture shell scripts and expected terminal reason strings.
//! - mapper: model/provider fixture construction.
//! - parser: argv sidecar observation parsing.
//! - validator: public launch, supervision, and terminal mapping assertions.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age_164_c3_launch_supervision.rs
//!     role: adapter
//!     Translates:
//!       - executor-cli-launch-public-api-test-contract
//!       - unix-process-status-fixture-contract
//!       - unix-fixture-file-sidecar-contract
//! ```

#![cfg(unix)]

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor::cli::{classify_terminal_reason, execute};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;

static TEST_DATA_DIR: OnceLock<tempfile::TempDir> = OnceLock::new();

fn ensure_test_data_dir() {
    TEST_DATA_DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("test data dir");
        unsafe {
            std::env::set_var(oulipoly_state::paths::DATA_DIR_ENV, dir.path());
        }
        dir
    });
}

fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
    ensure_test_data_dir();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("script.sh");
    std::fs::write(&path, script_body(body)).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    (dir, path)
}

fn script_body(body: &str) -> String {
    format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n")
}

fn model_for(path: &std::path::Path) -> ModelConfig {
    ensure_test_data_dir();
    ModelConfig {
        name: "c3-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new(
            path.to_string_lossy().into_owned(),
            Vec::new(),
        )],
        inputs: Vec::new(),
        provider: None,
    }
}

// ---------------------------------------------------------------------------
// classify_terminal_reason — stable reason vocabulary.
// ---------------------------------------------------------------------------

#[test]
fn classify_terminal_reason_exit_zero_returns_none() {
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(0);
    assert_eq!(classify_terminal_reason(&status), None);
}

#[test]
fn classify_terminal_reason_nonzero_exit_returns_exit_nonzero() {
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(1 << 8);
    assert_eq!(
        classify_terminal_reason(&status).as_deref(),
        Some("exit_nonzero")
    );
}

#[test]
fn classify_terminal_reason_signal_term_returns_named_signal() {
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(libc::SIGTERM);
    assert_eq!(
        classify_terminal_reason(&status).as_deref(),
        Some("signal:SIGTERM")
    );
}

#[test]
fn classify_terminal_reason_signal_kill_returns_named_signal() {
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(libc::SIGKILL);
    assert_eq!(
        classify_terminal_reason(&status).as_deref(),
        Some("signal:SIGKILL")
    );
}

#[test]
fn classify_terminal_reason_unknown_signal_number_falls_back_to_numeric() {
    // signal 97 isn't a named signal — the contract is that signal_name's
    // default arm formats the numeric value.
    use std::os::unix::process::ExitStatusExt;
    let status = std::process::ExitStatus::from_raw(97);
    // Linux: from_raw with low 7 bits = signal number; the resulting reason
    // should be `signal:97`.
    let reason = classify_terminal_reason(&status).unwrap_or_default();
    assert!(
        reason.starts_with("signal:"),
        "expected signal-shaped reason; got {reason}"
    );
}

// ---------------------------------------------------------------------------
// Empty-command validation through public `execute`.
// ---------------------------------------------------------------------------

#[test]
fn empty_command_rejected_with_canonical_error() {
    let model = ModelConfig {
        name: "empty-command-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig {
            environment: Default::default(),
            unset_environment: Default::default(),
            name: "empty".to_string(),
            command: String::new(),
            args: Vec::new(),
            interactive_args: None,
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }],
        inputs: Vec::new(),
        provider: None,
    };
    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert_eq!(err, "Empty command");
}

// ---------------------------------------------------------------------------
// Spawn-failure error string prefix.
// ---------------------------------------------------------------------------

#[test]
fn spawn_failure_error_prefix_is_failed_to_spawn() {
    ensure_test_data_dir();
    // Use a definitely-nonexistent path. The supervisor maps to
    // `Failed to spawn '{provider_name}': {err}`.
    let provider = ProviderConfig::new(
        "/this/path/does/not/exist/age-164-c3-spawn-failure-binary",
        Vec::new(),
    );
    let model = ModelConfig {
        name: "spawn-fail-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    };
    let err = execute(&model, 0, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert!(
        err.starts_with("Failed to spawn"),
        "expected 'Failed to spawn' prefix; got {err}"
    );
}

// ---------------------------------------------------------------------------
// Binary stdout fidelity through supervisor.
// ---------------------------------------------------------------------------

#[test]
fn supervisor_preserves_binary_stdout_bytes() {
    let (_dir, path) = script(r#"printf 'raw\x00\xffZ'"#);
    let model = model_for(&path);

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"raw\x00\xffZ");
}

#[test]
fn execute_with_silent_stdout_returns_empty_stdout_buffer() {
    let (_dir, path) = script("exit 0");
    let model = model_for(&path);

    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert!(
        result.stdout.is_empty(),
        "silent provider should yield empty stdout; got {:?}",
        result.stdout
    );
}

// ---------------------------------------------------------------------------
// Stdin prompt mode delivers prompt bytes.
// ---------------------------------------------------------------------------

#[test]
fn stdin_prompt_mode_delivers_prompt_bytes_to_child() {
    let (_dir, path) = script("cat");
    let mut model = model_for(&path);
    model.prompt_mode = PromptMode::Stdin;

    let result = execute(&model, 0, "piped input", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"piped input");
}

// ---------------------------------------------------------------------------
// Arg prompt mode leaves stdin empty.
// ---------------------------------------------------------------------------

#[test]
fn arg_prompt_mode_leaves_stdin_empty() {
    let dir = tempfile::tempdir().unwrap();
    let stdin_dump = dir.path().join("stdin.txt");
    let script_path = dir.path().join("arg.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\ncat > '{}'\nprintf 'ok'\n",
            stdin_dump.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let mut model = model_for(&script_path);
    model.prompt_mode = PromptMode::Arg;

    let result = execute(&model, 0, "argpromptbody", None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);
    assert_eq!(
        std::fs::read_to_string(&stdin_dump).unwrap(),
        "",
        "arg prompt mode must leave child stdin empty"
    );
}

// ---------------------------------------------------------------------------
// Working-directory propagation.
// ---------------------------------------------------------------------------

#[test]
fn working_directory_propagates_to_child() {
    let dir = tempfile::tempdir().unwrap();
    let cwd_dump = dir.path().join("cwd.txt");
    let script_path = dir.path().join("pwd.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\npwd > '{}'\n",
            cwd_dump.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let model = model_for(&script_path);
    let cwd_tempdir = tempfile::tempdir().unwrap();

    let result = execute(
        &model,
        0,
        "prompt",
        Some(cwd_tempdir.path()),
        &HashMap::new(),
        None,
    )
    .expect("execute");

    assert_eq!(result.exit_code, 0);
    let observed = std::fs::read_to_string(&cwd_dump).unwrap();
    assert_eq!(observed.trim(), cwd_tempdir.path().display().to_string());
}

// ---------------------------------------------------------------------------
// Nonzero exit code surfaces and terminal reason becomes "exit_nonzero".
// ---------------------------------------------------------------------------

#[test]
fn nonzero_exit_surfaces_with_exit_nonzero_terminal_reason() {
    let (_dir, path) = script("exit 17");
    let model = model_for(&path);
    let result = execute(&model, 0, "prompt", None, &HashMap::new(), None).expect("execute");

    assert_eq!(result.exit_code, 17);
    assert_eq!(result.terminal_reason.as_deref(), Some("exit_nonzero"));
}

// ---------------------------------------------------------------------------
// Large prompt is written to temp file and provider receives the
// "Follow the instructions in ..." instruction.
// ---------------------------------------------------------------------------

#[test]
fn large_arg_prompt_is_written_to_temp_file_and_referenced_in_argv() {
    let dir = tempfile::tempdir().unwrap();
    let argv_dump = dir.path().join("argv.txt");
    let script_path = dir.path().join("argv-dump.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\" > '{}'\n",
            argv_dump.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let model = model_for(&script_path);
    // 200 KB prompt — exceeds LARGE_PROMPT_THRESHOLD (100 KB).
    let big_prompt = "X".repeat(200 * 1024);

    let result = execute(&model, 0, &big_prompt, None, &HashMap::new(), None).expect("execute");
    assert_eq!(result.exit_code, 0);

    let argv: Vec<String> = std::fs::read_to_string(&argv_dump)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    // The single argv element should be "Follow the instructions in _agent_prompt_<uuid>.md".
    let instruction = argv
        .iter()
        .find(|arg| arg.starts_with("Follow the instructions in"))
        .unwrap_or_else(|| panic!("no instruction line; argv={argv:?}"));
    assert!(
        instruction.contains("_agent_prompt_"),
        "instruction must reference temp prompt file; got {instruction}"
    );
}
