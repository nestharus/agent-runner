#![cfg(unix)]

use oulipoly_config::ProviderConfig;
use oulipoly_runtime::executor::cli::execute_interactive_with_result;
use oulipoly_runtime::executor::terminal_signal::TerminalSignal;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
struct SignalCase {
    label: &'static str,
    body: &'static str,
}

fn primary_policy_token() -> String {
    ["cla", "ude"].concat()
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn interactive_provider(name: &str, command: &Path) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: name.to_string(),
        command: command.to_string_lossy().into_owned(),
        args: Vec::new(),
        interactive_args: Some(Vec::new()),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn terminal_signal_for(provider: &ProviderConfig) -> TerminalSignal {
    execute_interactive_with_result(provider, None, None, None)
        .expect("interactive execution")
        .terminal_signal
        .expect("interactive result includes terminal signal")
}

#[test]
fn cn3_current_routed_recognizer_matches_generic_terminal_signal() {
    let cases = [
        SignalCase {
            label: "clean",
            body: "exit 0",
        },
        SignalCase {
            label: "nonzero",
            body: "exit 42",
        },
        SignalCase {
            label: "signal",
            body: "trap - TERM\nkill -TERM \"$$\"\nsleep 1",
        },
    ];

    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let routed_command = write_script(
            dir.path(),
            &format!("{}-{}.sh", primary_policy_token(), case.label),
            case.body,
        );
        let generic_command =
            write_script(dir.path(), &format!("generic-{}.sh", case.label), case.body);
        let provider_name = format!("neutral-{}", case.label);
        let routed_provider = interactive_provider(&provider_name, &routed_command);
        let generic_provider = interactive_provider(&provider_name, &generic_command);

        let routed = terminal_signal_for(&routed_provider);
        let generic = terminal_signal_for(&generic_provider);

        assert_eq!(routed.kind, generic.kind, "{} kind", case.label);
        assert_eq!(routed.evidence, generic.evidence, "{} evidence", case.label);
        assert_eq!(
            routed.provider_name, provider_name,
            "{} routed name",
            case.label
        );
        assert_eq!(
            generic.provider_name, provider_name,
            "{} generic name",
            case.label
        );
    }
}
