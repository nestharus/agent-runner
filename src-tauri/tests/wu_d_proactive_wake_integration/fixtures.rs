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

    // Publish a real native parent before inserting historical recovery inputs.
    // The completed invocation is history; the next operational entry elects
    // its own owner/driver and obtains new custody through normal admission.
    pub(crate) fn establish_recovery_parent(&self, session_id: &str) {
        let path = self.root().join("provider.py");
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, original.replace(crate::SESSION, session_id)).unwrap();
        let output = self.run_agent("establish recovery recipient");
        fs::write(&path, original).unwrap();
        crate::validators::assert_success(&output);
        crate::liveness::wait_until(
            "initial native owner retired before historical setup",
            || {
                self.mailbox()
                    .completion_continuation_owner()
                    .unwrap()
                    .is_none()
            },
        );
        let runtime = self
            .mailbox()
            .wake_session_reader()
            .session_metadata(session_id)
            .unwrap()
            .expect("actual native recipient metadata");
        let id = runtime
            .invocation_uuid
            .as_deref()
            .expect("actual parent invocation");
        let parent = self.state().get_invocation_by_uuid(id).unwrap().unwrap();
        assert_eq!(parent.provider_session_id.as_deref(), Some(session_id));
        assert!(parent.finished_at.is_some());
        println!("real recovery parent session={session_id} invocation={id}");
    }

    pub(crate) fn seed_recovery_control(&self) {
        let session = "77777777-7777-4777-8777-777777777777";
        self.seed_session_turn_for(crate::PROVIDER, session, "positive-control-turn");
        self.seed_idle_runtime_for(session, crate::PROVIDER, crate::MODEL);
        self.seed_mailbox_for(session, "h-positive-control", None);
        crate::wake_claim_setup::seed_dead_wake_claim_for(
            self,
            session,
            "historical-positive-control",
            601,
        );
    }

    pub(crate) fn assert_recovery_control(&self) {
        let prompt =
            crate::liveness::wait_for_file(&self.prompt_file("recovery-positive-control.txt"));
        crate::validators::assert_prompt_contains_handle(&prompt, "h-positive-control");
        crate::liveness::wait_until(
            "positive control delivered under actual native owner",
            || {
                crate::liveness::delivered_rows_without_pending_or_claim(
                    self,
                    "77777777-7777-4777-8777-777777777777",
                    1,
                )
            },
        );
        self.assert_recovery_drained("77777777-7777-4777-8777-777777777777");
    }

    pub(crate) fn assert_recovery_drained(&self, session_id: &str) {
        // ACK/claim release is not physical custody discharge.
        crate::liveness::wait_until("original native activation drain integrated", || {
            self.sidecar_conn().query_row(
                "SELECT COUNT(*) > 0 AND SUM(phase='drained' AND integrated=1 AND drain_receipt LIKE '%ECHILD%')=COUNT(*) FROM completion_continuation_attempt WHERE session_id=?1",
                [session_id], |row| row.get::<_, bool>(0)).unwrap()
        });
        let conn = self.sidecar_conn();
        let mut stmt = conn.prepare(
            "SELECT attempt_id,custodian_identity,launcher_identity,runtime_generation_uuid,drain_receipt FROM completion_continuation_attempt WHERE session_id=?1").unwrap();
        let rows = stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .unwrap();
        for row in rows {
            let row = row.unwrap();
            let custodian: serde_json::Value = serde_json::from_str(&row.1).unwrap();
            let launcher: serde_json::Value = serde_json::from_str(&row.2).unwrap();
            assert_ne!(
                custodian["pid"].as_u64(),
                Some(u64::from(std::process::id()))
            );
            assert_ne!(
                launcher["pid"].as_u64(),
                Some(u64::from(std::process::id()))
            );
            println!("actual native drain session={session_id} attempt={row:?}");
        }
    }

    pub(crate) fn run_startup_recovery(&self, session_id: &str) -> Output {
        // Inspection is not the producer. An actual zero-TTL advisory lease
        // request enters the supported session-mutation startup-recovery lane;
        // it cannot hold the recipient paused during native wake admission.
        let owner_before = self.mailbox().completion_continuation_owner().unwrap();
        let rows_before = self.mailbox().list_mailbox(session_id, true).unwrap();
        crate::validators::assert_success(&self.run_mailbox_list(session_id));
        if owner_before.is_none() {
            assert!(
                self.mailbox()
                    .completion_continuation_owner()
                    .unwrap()
                    .is_none(),
                "inspection must not elect an owner"
            );
            assert_eq!(
                serde_json::to_value(self.mailbox().list_mailbox(session_id, true).unwrap())
                    .unwrap(),
                serde_json::to_value(rows_before).unwrap()
            );
        }
        let mut cmd = Command::new(crate::parse::runner_bin());
        cmd.args(["session", "pause-handshake", session_id, "--ttl-ms", "0"]);
        let output = self.run(cmd);
        println!(
            "operational recovery receipt: {} stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
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
