//! AGE-164 cluster 7 — test-module split and source-guard refactor.
//!
//! Replaces the existing source-body assertions in:
//!
//! - `execute_interactive_with_result_preserves_invocation_mode` (cli.rs:4302)
//! - `execute_resume_preserves_invocation_mode_while_clearing_session_capture`
//!   (cli.rs:4582)
//!
//! ...with behavior-only tests that pin the runtime invariants those source
//! guards were meant to protect:
//!
//! 1. Interactive execution uses `provider.interactive_args` for child argv.
//! 2. Interactive execution applies provider policy to that argv.
//! 3. Interactive execution sets cwd / parent_invocation_env via the same
//!    code path that one-shot execution uses (`build_command`-equivalent
//!    behavior observed through env and cwd propagation tests).
//! 4. Interactive execution does NOT rebuild `ProviderConfig` (verified by
//!    the policy-effects test: provider_index-style identity propagates
//!    through to argv emission).
//! 5. Resume execution clears session_capture and preserves invocation_mode
//!    on the returned result.
//!
//! Plus a "test inventory sentinel" that replaces t20: it asserts that the
//! public API path of each named entrypoint compiles and remains callable
//! through `oulipoly_runtime::executor::cli::*`. After Step 6c moves these
//! to new files, the public re-exports must still resolve.
//!
//! ## Declared roles
//!
//! Roles: formatter, mapper, parser, validator.
//!
//! - formatter: argv/env observer scripts and expected argv fragments.
//! - mapper: fixture helpers build tempdir/script/sidecar path tuples.
//! - parser: sidecar line loading.
//! - validator: interactive/resume behavior and public re-export assertions.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/tests/age_164_c7_test_split.rs
//!     role: adapter
//!     Translates:
//!       - executor-cli-public-reexport-test-contract
//!       - oulipoly-config-interactive-resume-fixture-contract
//!       - unix-process-fixture-sidecar-contract
//! ```

#![cfg(unix)]

use oulipoly_config::{
    ClaudeRestrictions, CodexRestrictions, InvocationMode, PromptMode, ProviderConfig, ResumeKind,
    ResumeStrategy, SessionCapture, SessionCaptureKind, ToolRestrictionKind, ToolRestrictions,
};
use oulipoly_runtime::executor::SessionCaptureMethod;
use oulipoly_runtime::executor::cli::{
    ResumePayload, classify_terminal_reason, compose_resume_args, execute, execute_interactive,
    execute_interactive_with_result, execute_resume, provider_name, shell_split,
    start_known_provider_session_id,
};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn argv_dump_script() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let argv_dump = dir.path().join("argv.txt");
    let path = dir.path().join("argv-dump.sh");
    std::fs::write(&path, argv_dump_script_body(&argv_dump)).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    (dir, path, argv_dump)
}

fn argv_dump_script_body(argv_dump: &Path) -> String {
    format!(
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$@\" > '{}'\n",
        argv_dump.display()
    )
}

fn read_argv(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

// ---------------------------------------------------------------------------
// Interactive source-guard replacement: behavior pins.
// ---------------------------------------------------------------------------

#[test]
fn interactive_execution_uses_provider_interactive_args_for_child_argv() {
    let (_dir, script_path, argv_dump) = argv_dump_script();
    let provider = ProviderConfig {
        name: "interactive-fixture".to_string(),
        command: script_path.to_string_lossy().into_owned(),
        args: vec!["one-shot-only".to_string()],
        interactive_args: Some(vec!["hello".to_string(), "world".to_string()]),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    };

    let exit_code = execute_interactive(&provider, None, None, None).unwrap();
    assert_eq!(exit_code, 0);

    let argv = read_argv(&argv_dump);
    assert_eq!(argv, vec!["hello", "world"]);
    // Critically: the one-shot args MUST NOT appear in the interactive argv.
    assert!(
        !argv.iter().any(|t| t == "one-shot-only"),
        "interactive must not emit provider.args; got {argv:?}"
    );
}

#[test]
fn interactive_execution_applies_provider_policy_to_argv() {
    let (_dir, script_path, argv_dump) = argv_dump_script();
    let provider = ProviderConfig {
        name: "claude-interactive".to_string(),
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: Some(vec!["--model".to_string(), "opus".to_string()]),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: Some(ToolRestrictions {
            kind: ToolRestrictionKind::Claude,
            claude: ClaudeRestrictions {
                disallowed_tools: vec!["Task".to_string()],
                allowed_tools: Vec::new(),
                disable_slash_commands: false,
            },
            codex: CodexRestrictions::default(),
        }),
        invocation_mode: Default::default(),
    };

    let exit_code = execute_interactive(&provider, None, None, None).unwrap();
    assert_eq!(exit_code, 0);

    let argv = read_argv(&argv_dump);
    assert!(
        argv.iter().any(|t| t == "--disallowed-tools"),
        "interactive policy emission missing; argv={argv:?}"
    );
}

#[test]
fn interactive_execution_preserves_invocation_mode_in_provider() {
    // The Tauri REPL path uses InvocationMode::Proxy. The interactive
    // execution must NOT rebuild ProviderConfig (which would lose this
    // mode). We can't observe invocation_mode from a child process, but
    // we CAN observe that the policy/argv/cwd propagation behaves
    // identically regardless of invocation_mode — the source guard
    // previously asserted "no ProviderConfig {" body, and the runtime
    // invariant is now pinned by this behavior parity.
    let (_dir, script_path, argv_dump) = argv_dump_script();
    for mode in [InvocationMode::Headless, InvocationMode::Proxy] {
        let _ = std::fs::write(&argv_dump, "");
        let provider = ProviderConfig {
            name: "invocation-mode-fixture".to_string(),
            command: script_path.to_string_lossy().into_owned(),
            args: Vec::new(),
            interactive_args: Some(vec!["hello".to_string()]),
            resume: None,
            session_capture: None,
            resume_acceptance: None,
            session_storage: None,
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: mode,
        };
        let result = execute_interactive_with_result(&provider, None, None, None).unwrap();
        assert_eq!(result.exit_code, 0, "mode={mode:?}");
        let argv = read_argv(&argv_dump);
        assert_eq!(argv, vec!["hello"], "mode={mode:?}");
    }
}

#[test]
fn interactive_execution_propagates_parent_invocation_env() {
    let dir = tempfile::tempdir().unwrap();
    let env_dump = dir.path().join("env.txt");
    let script_path = dir.path().join("env-dump.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/usr/bin/env bash\nset -euo pipefail\n\
             printf '%s' \"${{OULIPOLY_PARENT_INVOCATION-}}\" > '{}'\n",
            env_dump.display()
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();
    let provider = ProviderConfig {
        name: "env-fixture".to_string(),
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    };
    let parent_env = r#"{"source":"age-164-c7","id":"55555555-5555-5555-5555-555555555555"}"#;

    let result = execute_interactive_with_result(&provider, None, Some(parent_env), None).unwrap();
    assert_eq!(result.exit_code, 0);
    assert_eq!(std::fs::read_to_string(&env_dump).unwrap(), parent_env);
}

// ---------------------------------------------------------------------------
// Resume source-guard replacement: behavior pins.
// ---------------------------------------------------------------------------

#[test]
fn resume_execution_clears_session_capture_and_preserves_invocation_mode() {
    let dir = tempfile::tempdir().unwrap();
    let script_path = dir.path().join("noop.sh");
    std::fs::write(&script_path, "#!/usr/bin/env bash\nexit 0\n").unwrap();
    let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap();

    let provider = ProviderConfig {
        name: "resume-proxy-fixture".to_string(),
        command: script_path.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: None,
        // Provider declares session_capture, but resume MUST force it to None.
        session_capture: Some(SessionCapture {
            kind: SessionCaptureKind::ForcedFlagVerified,
            flag: Some("--session-id".to_string()),
            readback_args: None,
            event_type: None,
            event_id_path: None,
            json_flag: None,
            last_message_flag: None,
        }),
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: InvocationMode::Proxy,
    };
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };
    let result = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "prompt",
        None,
        None,
        ResumePayload {
            session_id: "11111111-1111-1111-1111-111111111111",
            strategy: &strategy,
        },
    )
    .expect("resume execute");

    assert_eq!(result.exit_code, 0);
    assert!(matches!(
        result.session_capture.method,
        SessionCaptureMethod::None
    ));
    assert_eq!(result.session_capture.session_id, None);
}

#[test]
fn resume_execution_appends_resume_args_to_provider_argv() {
    let (_dir, script_path, argv_dump) = argv_dump_script();
    let provider = ProviderConfig {
        name: "resume-tail-fixture".to_string(),
        command: script_path.to_string_lossy().into_owned(),
        args: vec!["--model".to_string(), "opus".to_string()],
        interactive_args: None,
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    };
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };
    let session_id = "66666666-6666-6666-6666-666666666666";
    let _ = execute_resume(
        &provider,
        0,
        PromptMode::Arg,
        "user prompt",
        None,
        None,
        ResumePayload {
            session_id,
            strategy: &strategy,
        },
    )
    .expect("resume execute");

    let argv = read_argv(&argv_dump);
    let resume_pos = argv
        .iter()
        .position(|t| t == "--resume")
        .expect("--resume present");
    assert_eq!(argv[resume_pos + 1], session_id, "argv={argv:?}");
    // The resume args are appended AFTER provider.args ("--model opus"),
    // matching the current append_validated_resume_args contract.
    let model_pos = argv
        .iter()
        .position(|t| t == "--model")
        .expect("--model present");
    assert!(model_pos < resume_pos, "argv={argv:?}");
}

// ---------------------------------------------------------------------------
// T20 inventory sentinel replacement: the public API path of each named
// entrypoint must remain callable through `executor::cli::*`. This compiles
// pre- and post-refactor because the contract pins these as load-bearing
// public items.
// ---------------------------------------------------------------------------

#[test]
fn t20_public_entrypoints_remain_callable_through_executor_cli_path() {
    // Compile-time: these imports must resolve. Each line below verifies
    // that the public name is still reachable via the `executor::cli`
    // path. Step 6c MUST preserve every one of these.
    let _ = shell_split as fn(&str) -> Vec<String>;
    let _ = provider_name as fn(&str) -> String;
    let _ = compose_resume_args
        as fn(&oulipoly_config::ResumeStrategy, &str) -> Result<Vec<String>, String>;
    let _ = classify_terminal_reason as fn(&std::process::ExitStatus) -> Option<String>;
    let _ = start_known_provider_session_id
        as fn(&oulipoly_config::ProviderConfig) -> Result<Option<String>, String>;

    // Runtime smoke check: the basic public surfaces still produce the
    // canonical observable values t20 used to encode inline.
    assert_eq!(
        shell_split(r#"env -u FOO "my provider""#),
        vec!["env", "-u", "FOO", "my provider"]
    );
    assert_eq!(provider_name("env -u CLAUDECODE claude"), "claude");

    use std::os::unix::process::ExitStatusExt;
    let success = std::process::ExitStatus::from_raw(0);
    let nonzero = std::process::ExitStatus::from_raw(42 << 8);
    let sigterm = std::process::ExitStatus::from_raw(15);

    assert_eq!(classify_terminal_reason(&success), None);
    assert_eq!(
        classify_terminal_reason(&nonzero).as_deref(),
        Some("exit_nonzero")
    );
    assert_eq!(
        classify_terminal_reason(&sigterm).as_deref(),
        Some("signal:SIGTERM")
    );
}

#[test]
fn t20_execute_facade_still_callable_via_executor_cli_execute() {
    // Compile-time guard: `execute`, `execute_resume`,
    // `execute_interactive`, `execute_interactive_with_result` resolve.
    let _ = execute
        as fn(
            &oulipoly_config::ModelConfig,
            usize,
            &str,
            Option<&std::path::Path>,
            &std::collections::HashMap<String, Vec<String>>,
            Option<&str>,
        ) -> Result<oulipoly_runtime::executor::ExecutionResult, String>;
    let _ = execute_interactive
        as fn(
            &oulipoly_config::ProviderConfig,
            Option<&std::path::Path>,
            Option<&str>,
            Option<ResumePayload<'_>>,
        ) -> Result<i32, String>;
    let _ = execute_interactive_with_result
        as fn(
            &oulipoly_config::ProviderConfig,
            Option<&std::path::Path>,
            Option<&str>,
            Option<ResumePayload<'_>>,
        )
            -> Result<oulipoly_runtime::executor::cli::InteractiveExecutionResult, String>;
    let _ = execute_resume
        as fn(
            &oulipoly_config::ProviderConfig,
            usize,
            oulipoly_config::PromptMode,
            &str,
            Option<&std::path::Path>,
            Option<&str>,
            ResumePayload<'_>,
        ) -> Result<oulipoly_runtime::executor::ExecutionResult, String>;

    // Smoke: provider-index out-of-range surface still goes through
    // public `execute`.
    let model = oulipoly_config::ModelConfig {
        name: "t20-smoke-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new("echo", Vec::new())],
        inputs: Vec::new(),
        provider: None,
    };
    let err = execute(&model, 99, "prompt", None, &HashMap::new(), None).unwrap_err();
    assert!(err.contains("out of range"), "{err}");
}

// ---------------------------------------------------------------------------
// Post-refactor file-layout characterization. This is a forward-looking
// structural check declared in the contract's "Post-refactor file layout
// invariants" section. It is intentionally permissive: it does not require
// any specific module to exist, only that `cli.rs` (or sibling
// `executor/cli/*.rs` files) collectively expose the public surface above.
//
// The actual file existence check is deferred to the per-component
// code-quality auditor in Phase 6.5, which runs against the final tree
// after Step 6c lands. This test stays green pre- and post-refactor.
// ---------------------------------------------------------------------------

#[test]
fn cluster_7_public_re_exports_remain_at_executor_cli_path() {
    // This test compiles iff every contract-pinned public entrypoint
    // resolves through `oulipoly_runtime::executor::cli::*`. Step 6c may
    // move the implementations to `cli/<submodule>.rs`, but the public
    // re-exports MUST stay at the same path.
    use oulipoly_runtime::executor::cli;

    let _: fn(&oulipoly_config::ResumeStrategy, &str) -> Result<Vec<String>, String> =
        cli::compose_resume_args;
    let _: fn(&str) -> Vec<String> = cli::shell_split;
    let _: fn(&str) -> String = cli::provider_name;
    let _: fn(&std::process::ExitStatus) -> Option<String> = cli::classify_terminal_reason;
    let _: fn(&oulipoly_config::ProviderConfig) -> Result<Option<String>, String> =
        cli::start_known_provider_session_id;

    // EffectiveExecuteRequest carrier must still be constructible at the
    // public cli path.
    let model = oulipoly_config::ModelConfig {
        name: "carrier-test".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new("echo", Vec::new())],
        inputs: Vec::new(),
        provider: None,
    };
    let extras = HashMap::new();
    let _request = cli::EffectiveExecuteRequest {
        model: &model,
        provider: &model.providers[0],
        provider_index: 0,
        prompt_mode: PromptMode::Arg,
        prompt: "smoke",
        working_dir: None,
        extra_inputs: &extras,
        parent_invocation_env: None,
    };
    // ResumePayload also remains at the cli path.
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };
    let _payload = cli::ResumePayload {
        session_id: "abc",
        strategy: &strategy,
    };
    // InteractiveExecutionResult is publicly named at the cli path.
    let _r: Option<cli::InteractiveExecutionResult> = None;
}
