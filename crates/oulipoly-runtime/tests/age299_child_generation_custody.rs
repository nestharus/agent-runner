#![cfg(target_os = "linux")]

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::executor::cli::{
    EffectiveExecuteRequest, execute_effective, execute_interactive_with_result_and_model_identity,
};
use oulipoly_state::CompositeInvocationId;
use rusqlite::Connection;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const HELPER_ENV: &str = "AGE299_CUSTODY_HELPER";
const MODE_ENV: &str = "AGE299_CUSTODY_MODE";
const PROVIDER_ENV: &str = "AGE299_CUSTODY_PROVIDER";
const SUCCESS_ENV: &str = "AGE299_CUSTODY_SUCCESS";
const PID_FILE_ENV: &str = "OULIPOLY_CHILD_PID_FILE";
const FAULT_ENV: &str = "OULIPOLY_CHILD_CUSTODY_TEST_FAULT";
const READY_FILE_ENV: &str = "OULIPOLY_CHILD_CUSTODY_TEST_READY_FILE";
const INVOCATION_UUID: &str = "71717171-7171-4171-8171-717171717171";

#[test]
fn headless_fault_boundaries_reap_before_fenced_generation_exit() {
    for fault in [
        "identity_capture",
        "headless_stdout_drain",
        "headless_stderr_drain",
        "headless_stdin",
        "headless_status_poll",
        "headless_live_quota",
    ] {
        run_fault_case("headless", fault, false);
    }
}

#[test]
fn direct_interactive_fault_boundaries_reap_before_fenced_generation_exit() {
    for fault in ["identity_capture", "direct_signal_install", "direct_wait"] {
        run_fault_case("direct", fault, false);
    }
}

#[test]
fn plain_pty_fault_boundaries_reap_before_fenced_generation_exit() {
    for fault in [
        "identity_capture",
        "plain_signal_install",
        "plain_raw_terminal",
        "plain_relay",
    ] {
        run_fault_case("plain", fault, true);
    }
}

#[test]
fn observed_pty_fault_boundaries_reap_before_fenced_generation_exit() {
    for fault in [
        "identity_capture",
        "observed_signal_install",
        "observed_raw_terminal",
        "observed_writer_clone",
        "observed_relay",
    ] {
        run_fault_case("observed", fault, true);
    }
}

#[test]
fn successful_paths_reap_and_complete_the_exact_generation_orderly() {
    for (mode, with_pty) in [
        ("headless", false),
        ("direct", false),
        ("plain", true),
        ("observed", true),
    ] {
        run_success_case(mode, with_pty);
    }
}

#[test]
fn helper_runs_fault_case() {
    if std::env::var_os(HELPER_ENV).is_none() {
        return;
    }
    let mode = std::env::var(MODE_ENV).unwrap();
    let provider_path = PathBuf::from(std::env::var_os(PROVIDER_ENV).unwrap());
    let pid_path = PathBuf::from(std::env::var_os(PID_FILE_ENV).unwrap());
    let provider = fixture_provider(&provider_path);
    let invocation = invocation_env();

    let outcome = match mode.as_str() {
        "headless" => {
            let model = fixture_model(provider.clone());
            execute_effective(EffectiveExecuteRequest {
                model: &model,
                provider: &provider,
                provider_index: 0,
                prompt_mode: PromptMode::Stdin,
                prompt: "custody probe",
                working_dir: None,
                models_dir: None,
                extra_inputs: &HashMap::new(),
                parent_invocation_env: Some(&invocation),
            })
            .map(|result| (result.exit_code, result.terminal_reason))
        }
        "direct" | "plain" | "observed" => execute_interactive_with_result_and_model_identity(
            &provider,
            None,
            Some(&invocation),
            None,
            Some("custody-model"),
        )
        .map(|result| (result.exit_code, result.terminal_reason)),
        _ => panic!("unknown custody mode {mode}"),
    };
    let success = std::env::var_os(SUCCESS_ENV).is_some();
    if success {
        assert_eq!(outcome.expect("custody success path must complete").0, 0);
    } else {
        let error = outcome.expect_err("custody fault must fail");
        assert!(error.contains("injected child custody failure"), "{error}");
    }

    let pid: libc::pid_t = fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_reaped(pid);
    assert_one_terminal_generation(if success {
        "orderly_completion"
    } else {
        "abnormal_termination"
    });
}

fn run_fault_case(mode: &str, fault: &str, with_pty: bool) {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    let provider = fixture_provider_script(dir.path());
    let pid_path = dir.path().join("child.pid");
    let ready_path = dir.path().join("child.ready");
    let mut command = helper_command(mode, fault, &provider, &pid_path, &ready_path, &data_dir);
    let status = if with_pty {
        let pty = OuterPty::open();
        attach_outer_pty(&mut command, &pty);
        command.spawn().unwrap().wait().unwrap()
    } else {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if mode == "direct" {
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        command.spawn().unwrap().wait().unwrap()
    };
    assert!(status.success(), "{mode}/{fault} helper failed: {status:?}");
}

fn run_success_case(mode: &str, with_pty: bool) {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).unwrap();
    let provider = fixture_provider_script(dir.path());
    let pid_path = dir.path().join("child.pid");
    let ready_path = dir.path().join("child.ready");
    let mut command = helper_command(
        mode,
        "no_fault",
        &provider,
        &pid_path,
        &ready_path,
        &data_dir,
    );
    command.env(SUCCESS_ENV, "1");
    let status = if with_pty {
        let pty = OuterPty::open();
        attach_outer_pty(&mut command, &pty);
        command.spawn().unwrap().wait().unwrap()
    } else {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        if mode == "direct" {
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        command.spawn().unwrap().wait().unwrap()
    };
    assert!(status.success(), "{mode} success helper failed: {status:?}");
}

fn helper_command(
    mode: &str,
    fault: &str,
    provider: &Path,
    pid_path: &Path,
    ready_path: &Path,
    data_dir: &Path,
) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("--exact")
        .arg("helper_runs_fault_case")
        .arg("--nocapture")
        .env(HELPER_ENV, "1")
        .env(MODE_ENV, mode)
        .env(PROVIDER_ENV, provider)
        .env(PID_FILE_ENV, pid_path)
        .env(READY_FILE_ENV, ready_path)
        .env(FAULT_ENV, fault)
        .env("OULIPOLY_DATA_DIR", data_dir)
        .env("TERM", "xterm-256color")
        .env(
            "OULIPOLY_INTERACTIVE_TUI",
            if mode == "observed" { "1" } else { "0" },
        );
    command
}

fn fixture_provider_script(dir: &Path) -> PathBuf {
    let path = dir.join("custody-provider.sh");
    fs::write(
        &path,
        r#"#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$$" > "${OULIPOLY_CHILD_PID_FILE}"
: > "${OULIPOLY_CHILD_CUSTODY_TEST_READY_FILE}"
if [[ "${AGE299_CUSTODY_SUCCESS:-0}" == "1" ]]; then
  sleep 0.1
  exit 0
fi
sleep 30
"#,
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
        name: "custody-provider".to_string(),
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

fn fixture_model(provider: ProviderConfig) -> ModelConfig {
    ModelConfig {
        name: "custody-model".to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: vec![provider],
        inputs: Vec::new(),
        provider: None,
    }
}

fn invocation_env() -> String {
    serde_json::to_string(&CompositeInvocationId {
        source: "custody-test".to_string(),
        id: INVOCATION_UUID.to_string(),
    })
    .unwrap()
}

fn assert_reaped(pid: libc::pid_t) {
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "child remained live or zombie"
    );
    assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD),
        "exact child was not reaped"
    );
}

fn assert_one_terminal_generation(expected_reason: &str) {
    let data_dir = PathBuf::from(std::env::var_os("OULIPOLY_DATA_DIR").unwrap());
    let connection = Connection::open(data_dir.join("pid-identity.db")).unwrap();
    let (count, exited, reason): (i64, i64, String) = connection
        .query_row(
            "SELECT COUNT(*),
                    SUM(lifecycle_state = 'exited'),
                    MIN(terminal_reason)
             FROM runtime_generation
             WHERE spawn_invocation_uuid = ?1",
            [INVOCATION_UUID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(exited, 1);
    assert_eq!(reason, expected_reason);
}

struct OuterPty {
    master: File,
    slave: File,
}

impl OuterPty {
    fn open() -> Self {
        let winsize = libc::winsize {
            ws_row: 30,
            ws_col: 100,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let mut master_fd = -1;
        let mut slave_fd = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master_fd,
                    &mut slave_fd,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    &winsize,
                )
            },
            0
        );
        Self {
            master: unsafe { File::from_raw_fd(master_fd) },
            slave: unsafe { File::from_raw_fd(slave_fd) },
        }
    }
}

fn attach_outer_pty(command: &mut Command, pty: &OuterPty) {
    let stdin = pty.slave.try_clone().unwrap();
    let stdout = pty.slave.try_clone().unwrap();
    let stderr = pty.slave.try_clone().unwrap();
    let slave_fd = pty.slave.as_raw_fd();
    let master_fd = pty.master.as_raw_fd();
    command
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::tcsetpgrp(slave_fd, libc::getpid()) == -1 {
                return Err(io::Error::last_os_error());
            }
            if master_fd > 2 {
                libc::close(master_fd);
            }
            Ok(())
        });
    }
}
