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
    root_dir: PathBuf,
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
        let dir = std::env::var_os("WU_D_OUTER_FIXTURE_ROOT")
            .is_none()
            .then(|| tempfile::tempdir().unwrap());
        let root_dir = std::env::var_os("WU_D_OUTER_FIXTURE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| dir.as_ref().unwrap().path().to_path_buf());
        let config_home = root_dir.join("xdg-config");
        let data_home = root_dir.join("xdg-data");
        let state_home = root_dir.join("xdg-state");
        let home_dir = root_dir.join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let work_dir = root_dir.join("work");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&state_home).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        Self {
            dir,
            root_dir,
            config_home,
            data_home,
            state_home,
            home_dir,
            app_config_dir,
            models_dir,
            work_dir,
        }
    }

    // The outer native Runner owns the live admission lease. Its provider launches
    // this same test; the endpoint and invocation authority come from that launch,
    // never from a seeded owner row or a fabricated environment value.
    pub(crate) fn run_under_outer_owner(&self, node: &str) -> bool {
        if std::env::var_os("WU_D_OUTER_FIXTURE_ROOT").is_some() {
            assert!(std::env::var_os("OULIPOLY_COMPLETION_ENDPOINT").is_some());
            return false;
        }
        let hook = format!(
            "exec {} --exact {} --nocapture",
            shell_quote(&std::env::current_exe().unwrap().to_string_lossy()),
            shell_quote(node)
        );
        let script = crate::fake_provider::provider_script(&hook, "", "outer-unused.txt")
            .replace(crate::SESSION, crate::cases_basic::OUTER_SESSION);
        self.write_provider(&script);
        let mut cmd = self.agent_command("run admitted fixture provider");
        self.prepare_command(&mut cmd);
        cmd.env("WU_D_OUTER_FIXTURE_ROOT", self.root());
        let output = cmd.output().unwrap();
        println!(
            "outer fixture stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(output.status.success(), "outer fixture failed: {output:?}");
        true
    }

    pub(crate) fn assert_missing_owner_rejected(&self) {
        let mut command = self.agent_command("must not launch without inherited owner");
        self.prepare_command(&mut command);
        command.env_remove("OULIPOLY_COMPLETION_ENDPOINT");
        let output = command.output().unwrap();
        assert!(
            !output.status.success(),
            "missing owner unexpectedly admitted"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("ancestor has no inherited completion owner"),
            "{output:?}"
        );
        println!("missing inherited owner rejected: {stderr}");
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
            .env("OULIPOLY_CONFIG_HOME", &self.config_home)
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
            .env_remove("AGENT_BASH_OWNER_SESSION_ID")
            .env_remove("AGENT_BASH_OWNER_INVOCATION_UUID")
            .current_dir(self.root());
        if std::env::var_os("WU_D_OUTER_FIXTURE_ROOT").is_some() {
            cmd.env(
                "OULIPOLY_PARENT_INVOCATION",
                std::env::var("OULIPOLY_PARENT_INVOCATION").unwrap(),
            );
        }
        let helper = self.root().join("agent-bash/agent-bash");
        if helper.is_file() {
            cmd.env("AGENT_BASH_BIN", helper).env(
                "AGENT_BASH_AGENT_RUNNER_BIN",
                self.root().join("runner/oulipoly-agent-runner"),
            );
        }
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

    pub(crate) fn write_executable(&self, name: &str, body: &str) -> PathBuf {
        let path = self.root().join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    pub(crate) fn install_agent_bash(&self, source: &std::path::Path) -> PathBuf {
        let agent_bash_dir = self.root().join("agent-bash");
        let runner_dir = self.root().join("runner");
        fs::create_dir_all(&agent_bash_dir).unwrap();
        fs::create_dir_all(&runner_dir).unwrap();

        let agent_bash = agent_bash_dir.join("agent-bash");
        fs::copy(source, &agent_bash).unwrap();
        let runner = runner_dir.join("oulipoly-agent-runner");
        link_or_copy(std::path::Path::new(crate::parse::runner_bin()), &runner);

        fs::write(
            agent_bash_dir.join("agent-bash.toml"),
            format!(
                "state_root = {:?}\nagent_runner_bin = {:?}\n",
                self.state_home.join("agent-bash").display().to_string(),
                runner.display().to_string(),
            ),
        )
        .unwrap();
        fs::write(
            runner_dir.join("config.toml"),
            format!(
                "data_dir = {:?}\nconfig_home = {:?}\n",
                self.data_home
                    .join("oulipoly-agent-runner")
                    .display()
                    .to_string(),
                self.config_home.display().to_string(),
            ),
        )
        .unwrap();
        agent_bash
    }

    pub(crate) fn prompt_file(&self, name: &str) -> PathBuf {
        self.work_dir.join(name)
    }

    fn root(&self) -> &std::path::Path {
        &self.root_dir
    }
}

fn link_or_copy(source: &std::path::Path, destination: &std::path::Path) {
    if fs::hard_link(source, destination).is_err() {
        fs::copy(source, destination).unwrap();
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

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
