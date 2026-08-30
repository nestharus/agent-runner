//! ## Declared roles
//!
//! Roles: accessor.
//!
//! TEST: isolated filesystem, environment, and process accessors for the
//! proactive wake integration fixture.

use crate::{MODEL, SESSION};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

pub(crate) struct Fixture {
    pub(crate) dir: Option<tempfile::TempDir>,
    pub(crate) config_home: PathBuf,
    pub(crate) data_home: PathBuf,
    pub(crate) state_home: PathBuf,
    pub(crate) home_dir: PathBuf,
    pub(crate) app_config_dir: PathBuf,
    pub(crate) models_dir: PathBuf,
    pub(crate) work_dir: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("xdg-config");
        let data_home = dir.path().join("xdg-data");
        let state_home = dir.path().join("xdg-state");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let work_dir = dir.path().join("work");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&state_home).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        Self {
            dir: Some(dir),
            config_home,
            data_home,
            state_home,
            home_dir,
            app_config_dir,
            models_dir,
            work_dir,
        }
    }

    pub(crate) fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    pub(crate) fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    pub(crate) fn run(&self, mut cmd: Command) -> Output {
        self.prepare_command(&mut cmd);
        cmd.output().unwrap()
    }

    fn prepare_command(&self, cmd: &mut Command) {
        cmd.env("XDG_CONFIG_HOME", &self.config_home)
            .env_remove("OULIPOLY_CONFIG_HOME")
            .env("XDG_DATA_HOME", &self.data_home)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("HOME", &self.home_dir)
            .env("AGENT_BASH_AGENT_RUNNER_BIN", crate::parse::runner_bin())
            .env("WU_D_WORK_DIR", &self.work_dir)
            .env(
                "OULIPOLY_DATA_DIR",
                self.data_home.join("oulipoly-agent-runner"),
            )
            .env_remove("OULIPOLY_AUTO_WAKE")
            .env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID")
            .env_remove("OULIPOLY_AUTO_WAKE_TOKEN")
            .env_remove("OULIPOLY_AUTO_WAKE_COUNT")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .current_dir(self.root());
    }

    pub(crate) fn run_agent(&self, prompt: &str) -> Output {
        let cmd = self.agent_command(prompt);
        self.run(cmd)
    }

    fn agent_command(&self, prompt: &str) -> Command {
        let mut cmd = Command::new(crate::parse::runner_bin());
        cmd.arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(prompt);
        cmd
    }

    pub(crate) fn run_resume(&self) -> Output {
        self.run(self.resume_command())
    }

    pub(crate) fn run_resume_with_retry_base(&self, retry_base_milliseconds: u64) -> Output {
        let mut cmd = self.resume_command();
        cmd.env(
            oulipoly_core::AutoWakeEnvironmentVariable::RETRY_BASE_MILLISECONDS.name(),
            retry_base_milliseconds.to_string(),
        );
        self.run(cmd)
    }

    pub(crate) fn run_auto_wake_resume(
        &self,
        claim_token: &str,
        chronological_attempt_count: i64,
        retry_base_milliseconds: u64,
    ) -> Output {
        let mut cmd = self.resume_command();
        self.prepare_command(&mut cmd);
        cmd.env(
            oulipoly_core::AutoWakeEnvironmentVariable::MARKER.name(),
            "1",
        )
        .env(
            oulipoly_core::AutoWakeEnvironmentVariable::SESSION_ID.name(),
            SESSION,
        )
        .env(
            oulipoly_core::AutoWakeEnvironmentVariable::CLAIM_TOKEN.name(),
            claim_token,
        )
        .env(
            oulipoly_core::AutoWakeEnvironmentVariable::COUNT.name(),
            chronological_attempt_count.to_string(),
        )
        .env(
            oulipoly_core::AutoWakeEnvironmentVariable::RETRY_BASE_MILLISECONDS.name(),
            retry_base_milliseconds.to_string(),
        );
        cmd.output().unwrap()
    }

    fn resume_command(&self) -> Command {
        let mut cmd = Command::new(crate::parse::runner_bin());
        cmd.arg("resume")
            .arg("-m")
            .arg(MODEL)
            .arg("--session-id")
            .arg(SESSION)
            .arg("--models-dir")
            .arg(&self.models_dir);
        cmd
    }

    pub(crate) fn run_mailbox_list(&self, session_id: &str) -> Output {
        self.run(self.mailbox_list_command(session_id))
    }

    fn mailbox_list_command(&self, session_id: &str) -> Command {
        let mut cmd = Command::new(crate::parse::runner_bin());
        cmd.arg("mailbox")
            .arg("list")
            .arg("--session-id")
            .arg(session_id)
            .arg("--json");
        cmd
    }

    pub(crate) fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }
    pub(crate) fn prompt_file(&self, name: &str) -> PathBuf {
        self.work_dir.join(name)
    }

    fn root(&self) -> &std::path::Path {
        self.dir.as_ref().expect("fixture directory").path()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking()
            && let Some(directory) = self.dir.take()
        {
            eprintln!(
                "preserved failed proactive-wake fixture at {}",
                directory.keep().display()
            );
        }
    }
}
