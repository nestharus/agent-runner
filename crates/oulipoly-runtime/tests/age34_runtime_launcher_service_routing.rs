#![cfg(unix)]

use oulipoly_config::ProviderConfig;
use oulipoly_runtime::executor::cli;
use oulipoly_runtime::repl_default_provider;
use oulipoly_runtime::services::{
    LauncherServiceOutput, LauncherServicePort, LauncherServiceRequest,
};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

struct FixtureScript {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

fn fixture_script(body: &str) -> FixtureScript {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("provider.sh");
    std::fs::write(
        &path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
    )
    .expect("write provider");
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod provider");
    FixtureScript { _dir: dir, path }
}

fn interactive_provider(script: &FixtureScript) -> ProviderConfig {
    ProviderConfig {
        name: "interactive-provider".to_string(),
        command: script.path.to_string_lossy().into_owned(),
        args: vec!["one-shot-only".to_string()],
        interactive_args: Some(vec!["launch".to_string()]),
        resume: None,
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
    }
}

// Risk: R-A4 / proposal T11 - launcher service routing must stay equivalent to
// execute_interactive(provider, working_dir, None, None), preserving exit code,
// working directory, inherited protocol, and no parent-env/resume payload.
// Level: unit.
// Source: AGE-34 contract "LauncherServiceRequest / LauncherServiceOutput";
// assumption A4.
#[test]
fn runtime_launcher_service_matches_direct_interactive_delegate() {
    let working_dir = tempfile::tempdir().expect("working dir");
    let marker = tempfile::NamedTempFile::new().expect("marker");
    let script = fixture_script(&format!(
        r#"test "$PWD" = "{cwd}"
test "${{OULIPOLY_PARENT_INVOCATION-}}" = ""
test "$1" = "launch"
printf 'arg=%s cwd=%s' "$1" "$PWD" > "{marker}"
exit 23"#,
        cwd = working_dir.path().display(),
        marker = marker.path().display()
    ));
    let provider = interactive_provider(&script);

    let direct =
        cli::execute_interactive(&provider, Some(working_dir.path()), None, None).expect("direct");
    let direct_marker = std::fs::read_to_string(marker.path()).expect("direct marker");
    std::fs::write(marker.path(), "").expect("clear marker");

    let service: &dyn LauncherServicePort = &repl_default_provider::RuntimeLauncherService;
    let LauncherServiceOutput { exit_code } = service
        .launch(LauncherServiceRequest {
            provider: provider.clone(),
            working_dir: Some(working_dir.path().to_path_buf()),
        })
        .expect("service launch");

    assert_eq!(exit_code, direct);
    assert_eq!(
        std::fs::read_to_string(marker.path()).unwrap(),
        direct_marker
    );
}
