#![cfg(unix)]

use oulipoly_config::{ProviderConfig, ResumeKind, ResumeStrategy};
use oulipoly_runtime::executor::cli::{
    ResumePayload, execute_interactive_with_result_and_model_identity,
};
use oulipoly_state::CompositeInvocationId;
use oulipoly_state::mailbox::MailboxDb;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const INVOCATION_UUID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SESSION_ID: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const FALLBACK_HELPER_ENV: &str = "WU_E_FALLBACK_HELPER";
const PROVIDER_SCRIPT_ENV: &str = "WU_E_PROVIDER_SCRIPT";
const CHILD_RESULT_ENV: &str = "WU_E_CHILD_RESULT";

#[test]
fn no_controlling_terminal_fallback_records_no_pty_control_path() {
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    let data_home = dir.path().join("data");
    let runtime_dir = dir.path().join("runtime");
    let state_home = dir.path().join("state");
    let home_dir = dir.path().join("home");
    fs::create_dir_all(&config_home).unwrap();
    fs::create_dir_all(&data_home).unwrap();
    fs::create_dir_all(&runtime_dir).unwrap();
    fs::create_dir_all(&state_home).unwrap();
    fs::create_dir_all(&home_dir).unwrap();
    fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700)).unwrap();
    let app_data_dir = data_home.join("oulipoly-agent-runner");
    let sidecar_path = app_data_dir.join("pid-identity.db");
    let script = fixture_script(dir.path());
    let child_result = dir.path().join("child-result.txt");

    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.arg("--exact")
        .arg("helper_runs_no_controlling_terminal_fallback")
        .arg("--nocapture")
        .env(FALLBACK_HELPER_ENV, "1")
        .env(PROVIDER_SCRIPT_ENV, &script)
        .env(CHILD_RESULT_ENV, &child_result)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("OULIPOLY_DATA_DIR", &app_data_dir)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("XDG_STATE_HOME", &state_home)
        .env("HOME", &home_dir)
        .env_remove("OULIPOLY_PARENT_INVOCATION");
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "child failed: status={:?}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&child_result).unwrap(), "ok\n");

    let db = MailboxDb::open(&sidecar_path).unwrap();
    let runtime = db.session_runtime(SESSION_ID).unwrap().unwrap();
    assert_eq!(runtime.mode, "pty_interactive");
    assert!(runtime.pty_control_path.is_none());
}

#[test]
fn helper_runs_no_controlling_terminal_fallback() {
    if std::env::var_os(FALLBACK_HELPER_ENV).is_none() {
        return;
    }
    assert!(
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_err(),
        "setsid child unexpectedly retained a controlling terminal"
    );
    let script = PathBuf::from(std::env::var_os(PROVIDER_SCRIPT_ENV).unwrap());
    let child_result = PathBuf::from(std::env::var_os(CHILD_RESULT_ENV).unwrap());
    let provider = fixture_provider(&script);
    let strategy = ResumeStrategy {
        kind: ResumeKind::Flag,
        flag: Some("--resume".to_string()),
        subcommand: None,
    };
    let invocation_env = invocation_env();

    let result = execute_interactive_with_result_and_model_identity(
        &provider,
        None,
        Some(&invocation_env),
        Some(ResumePayload {
            session_id: SESSION_ID,
            strategy: &strategy,
        }),
        Some("fixture-model"),
    )
    .unwrap();
    assert_eq!(result.exit_code, 0);
    fs::write(child_result, "ok\n").unwrap();
}

fn fixture_script(dir: &Path) -> PathBuf {
    let path = dir.join("fixture-provider.sh");
    fs::write(
        &path,
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf 'ok\\n'\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn fixture_provider(script: &Path) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: "fixture-provider".to_string(),
        command: script.to_string_lossy().into_owned(),
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

fn invocation_env() -> String {
    serde_json::to_string(&CompositeInvocationId {
        source: "fixture".to_string(),
        id: INVOCATION_UUID.to_string(),
    })
    .unwrap()
}
