#![cfg(unix)]

mod fixtures;

use agent_runner_lib::process::{OutputSpec, StdinSpec};
use agent_runner_lib::setup::detection::{detect_all_with_runner, detect_single_cli_with_runner};
use fixtures::b2_process_runner::*;

/// Risk: T18 (Claude detection command selection preservation)
/// Source: proposal §8 T18; contract §3 setup::detection Claude branch
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn detection_claude_branch_uses_which_version_and_auth_status_runner_specs() {
    isolated_process_env(|| {
        let runner = FakeProcessRunner::new();
        runner.push_stdout(b"/usr/bin/claude\n");
        runner.push_stdout(b"claude 1.2.3\n");
        runner.push_stdout(
            br#"{"loggedIn":true,"email":"dev@example.com","authMethod":"oauth","subscriptionType":"pro"}"#,
        );

        let info = detect_single_cli_with_runner("claude", &runner);

        assert_eq!(info.name, "claude");
        assert!(info.installed);
        assert_eq!(info.path.as_deref(), Some("/usr/bin/claude"));
        assert!(info.version.as_deref().unwrap_or("").contains("1.2.3"));
        assert_eq!(info.profiles.len(), 1);
        assert_eq!(info.profiles[0].id, "dev@example.com");
        assert_eq!(info.profiles[0].auth_method, "oauth");
        let calls = runner.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].program, "which");
        assert_eq!(calls[0].args, vec!["claude"]);
        assert_eq!(calls[1].program, "claude");
        assert_eq!(calls[1].args, vec!["--version"]);
        assert_eq!(calls[2].program, "claude");
        assert_eq!(calls[2].args, vec!["auth", "status"]);
        assert!(calls.iter().all(|call| call.stdin == StdinSpec::Null));
        assert!(calls.iter().all(|call| call.stdout == OutputSpec::Capture));
        assert!(calls.iter().all(|call| call.stderr == OutputSpec::Capture));
        assert!(calls.iter().all(|call| call.timeout.is_none()));
    });
}

/// Risk: T18 (Codex detection command selection preservation)
/// Source: proposal §8 T18; contract §3 setup::detection Codex branch
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn detection_codex_branch_uses_login_status_runner_spec() {
    isolated_process_env(|| {
        let runner = FakeProcessRunner::new();
        runner.push_stdout(b"/usr/bin/codex\n");
        runner.push_stdout(b"codex 0.12.0\n");
        runner.push_stdout(b"Logged in using ChatGPT\n");

        let info = detect_single_cli_with_runner("codex", &runner);

        assert_eq!(info.name, "codex");
        assert!(info.installed);
        let calls = runner.calls();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].program, "which");
        assert_eq!(calls[0].args, vec!["codex"]);
        assert_eq!(calls[1].program, "codex");
        assert_eq!(calls[1].args, vec!["--version"]);
        assert_eq!(calls[2].program, "codex");
        assert_eq!(calls[2].args, vec!["login", "status"]);
    });
}

/// Risk: T18 (missing CLI classification stops before branch-specific commands)
/// Source: proposal §8 T18; contract §3 setup::detection detect_cli
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn detection_missing_cli_only_runs_which_and_reports_uninstalled() {
    isolated_process_env(|| {
        let runner = FakeProcessRunner::new();
        runner.push_stderr_exit("not found", 1);

        let info = detect_single_cli_with_runner("claude", &runner);

        assert_eq!(info.name, "claude");
        assert!(!info.installed);
        assert!(info.path.is_none());
        assert_eq!(runner.calls().len(), 1);
        assert_eq!(runner.only_call().program, "which");
    });
}

/// Risk: T12/T18 (detect-all wrapper threads the same runner through every branch)
/// Source: proposal §8 T12/T18; contract §3 detect_all_with_runner
/// Level: component
/// Fixture source: src-tauri/tests/fixtures/b2_process_runner.rs
#[test]
fn detection_detect_all_with_runner_attempts_supported_cli_path_lookups() {
    isolated_process_env(|| {
        let runner = FakeProcessRunner::new();
        for _ in 0..4 {
            runner.push_stderr_exit("missing", 1);
        }

        let report = detect_all_with_runner(&runner);

        assert_eq!(report.clis.len(), 4);
        let calls = runner.calls();
        let lookups: Vec<_> = calls
            .iter()
            .map(|call| (call.program.as_str(), call.args.as_slice()))
            .collect();
        assert!(
            lookups
                .iter()
                .any(|(program, args)| *program == "which" && *args == ["claude"])
        );
        assert!(
            lookups
                .iter()
                .any(|(program, args)| *program == "which" && *args == ["codex"])
        );
        assert!(
            lookups
                .iter()
                .any(|(program, args)| *program == "which" && *args == ["gemini"])
        );
        assert!(
            lookups
                .iter()
                .any(|(program, args)| *program == "which" && *args == ["opencode"])
        );
    });
}
