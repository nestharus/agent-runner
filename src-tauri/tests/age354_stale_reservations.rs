#![cfg(target_os = "linux")]

mod provider_authority_fixture;

use oulipoly_state::{mailbox::MailboxDb, pid_identity};
use std::fs;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// Every fixture process is private. No native model, installed config, or UI.
struct OwnedChild(Child, bool);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        unsafe {
            libc::killpg(self.0.id() as i32, libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn zombie_admitted_owner_does_not_block_headless_or_either_interactive_entrypoint() {
    for mode in ["headless", "model-repl", "default-repl"] {
        // Physical private path stays within the portable Unix socket limit;
        // a symlink to a long data root would not exercise that contract.
        let dir = tempfile::Builder::new()
            .prefix("age354-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = dir.path();
        let home = root.join("home");
        let config_home = root.join("config");
        let app = config_home.join("oulipoly-agent-runner");
        let models = app.join("models");
        let data_home = root.join("data");
        let data = data_home.join("oulipoly-agent-runner");
        let tmp = root.join("tmp");
        for path in [&home, &models, &data, &tmp] {
            fs::create_dir_all(path).unwrap();
        }
        let marker = root.join("launched");
        let provider = root.join("fixture.sh");
        fs::write(
            &provider,
            format!(
                "#!/bin/sh\nprintf launched > '{}'\nprintf 'fixture finished\\n'\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(app.join("providers.toml"), provider_authority_fixture::with_explicit_provider_authority(
            &format!("[fixture]\ncommand = '{}'\nargs = []\ninteractive_args = []\nprompt_mode = 'arg'\n", provider.display())
        )).unwrap();
        // Default-provider selection uses account-name families, not endpoint IDs.
        fs::write(app.join("config.toml"), "default_provider = 'fixture'\n").unwrap();
        fs::write(
            models.join("fixture-model.toml"),
            "[[providers]]\nname = 'fixture'\nargs = []\n",
        )
        .unwrap();

        let mut zombie = OwnedChild(
            Command::new("/bin/sleep")
                .arg("60")
                .process_group(0)
                .spawn()
                .unwrap(),
            false,
        );
        let identity = pid_identity::read_live_process_identity(i64::from(zombie.0.id()))
            .unwrap()
            .unwrap();
        let sidecar = data.join("pid-identity.db");
        let mut db = MailboxDb::open(&sidecar).unwrap();
        db.session_admissions()
            .enqueue("zombie-admission", "zombie-owner", None, &identity, 1)
            .unwrap();
        assert!(matches!(
            db.session_admissions()
                .try_admit_next("old-claim", 0, 2)
                .unwrap(),
            oulipoly_state::mailbox::SessionAdmissionAttempt::Admitted(_)
        ));
        zombie.0.kill().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let stat = fs::read_to_string(format!("/proc/{}/stat", zombie.0.id())).unwrap();
            if stat.rsplit_once(") ").unwrap().1.starts_with('Z') {
                break;
            }
            assert!(Instant::now() < deadline, "fixture did not become zombie");
            std::thread::sleep(Duration::from_millis(5));
        }
        let stdout = root.join("stdout");
        let stderr = root.join("stderr");
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_DATA_HOME", &data_home)
            .env("TMPDIR", &tmp)
            .env("OULIPOLY_DATA_DIR", &data)
            .env("OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES", "1")
            .current_dir(root)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout).unwrap())
            .stderr(fs::File::create(&stderr).unwrap());
        match mode {
            "headless" => {
                command.args(["--model", "fixture-model", "probe"]);
            }
            "model-repl" => {
                command.args(["repl", "fixture-model"]);
            }
            _ => {
                command.arg("--new");
            }
        }
        let mut runner = OwnedChild(command.spawn().unwrap(), false);
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            // WNOWAIT keeps the exact fixture leader reserved until group cleanup.
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    runner.0.id(),
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            assert_eq!(result, 0);
            if unsafe { info.assume_init().si_pid() } != 0 {
                unsafe {
                    libc::killpg(runner.0.id() as i32, libc::SIGKILL);
                }
                let status = runner.0.wait().unwrap();
                runner.1 = true;
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "{mode} timed out: {}",
                fs::read_to_string(&stderr).unwrap()
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        let stderr_text = fs::read_to_string(&stderr).unwrap();
        if mode == "default-repl" {
            // This tiny provider deliberately reports no live session identity.
            // Admission/settlement must not fabricate user-workload success.
            assert_eq!(status.code(), Some(1), "{stderr_text}");
            assert!(
                stderr_text.contains("live_session_identity_unavailable"),
                "{stderr_text}"
            );
        } else {
            assert!(
                status.success(),
                "{mode}: {status}: stderr={stderr_text} stdout={}",
                fs::read_to_string(&stdout).unwrap()
            );
        }
        assert!(
            marker.exists(),
            "{mode}: successor did not enter provider startup"
        );
        assert_eq!(
            db.session_admissions()
                .row("zombie-owner")
                .unwrap()
                .unwrap()
                .state,
            "cancelled"
        );
        let conn = rusqlite::Connection::open(&sidecar).unwrap();
        let (state, generation): (String, Option<String>) = conn.query_row(
            "SELECT state, runtime_generation_uuid FROM session_admission_queue WHERE registration_identity != 'zombie-owner'", [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        assert_eq!(state, "settled", "{mode}");
        assert!(
            generation.is_some(),
            "{mode}: no Starting generation was published"
        );
        // Keep the original owner unreaped through the entire successor execution.
        assert!(
            fs::read_to_string(format!("/proc/{}/stat", zombie.0.id()))
                .unwrap()
                .rsplit_once(") ")
                .unwrap()
                .1
                .starts_with('Z')
        );
    }
}

#[test]
fn starting_creator_crash_releases_only_after_custody_for_all_entrypoints() {
    for barrier in ["before_spawn", "before_publication"] {
        for mode in ["headless", "model-repl", "default-repl"] {
            // Physical private path stays within the portable Unix socket limit;
            // a symlink to a long data root would not exercise that contract.
            let dir = tempfile::Builder::new()
                .prefix("age354-")
                .tempdir_in("/tmp")
                .unwrap();
            let root = dir.path();
            let home = root.join("home");
            let config_home = root.join("config");
            let app = config_home.join("oulipoly-agent-runner");
            let models = app.join("models");
            let data_home = root.join("data");
            let data = data_home.join("oulipoly-agent-runner");
            let tmp = root.join("tmp");
            for path in [&home, &models, &data, &tmp] {
                fs::create_dir_all(path).unwrap();
            }
            let marker = root.join("launched");
            let provider = root.join("fixture.sh");
            fs::write(
            &provider,
            format!(
                "#!/bin/sh\nprintf 'launched\\n' >> '{}'\ni=0; while [ ! -e '{}' ]; do i=$((i+1)); [ $i -lt 500 ] || exit 124; /bin/sleep 0.01; done\nprintf 'fixture finished\\n'\n",
                marker.display(), root.join("release-workload").display()
            ),
        )
        .unwrap();
            fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
            fs::write(app.join("providers.toml"), provider_authority_fixture::with_explicit_provider_authority(
            &format!("[fixture]\ncommand = '{}'\nargs = []\ninteractive_args = []\nprompt_mode = 'arg'\n", provider.display())
        )).unwrap();
            // Default-provider selection uses account-name families, not endpoint IDs.
            fs::write(app.join("config.toml"), "default_provider = 'fixture'\n").unwrap();
            fs::write(
                models.join("fixture-model.toml"),
                "[[providers]]\nname = 'fixture'\nargs = []\n",
            )
            .unwrap();

            let sidecar = data.join("pid-identity.db");
            let _db = MailboxDb::open(&sidecar).unwrap();
            let stdout = root.join("stdout");
            let stderr = root.join("stderr");
            let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
            command
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &home)
                .env("XDG_CONFIG_HOME", &config_home)
                .env("XDG_DATA_HOME", &data_home)
                .env("TMPDIR", &tmp)
                .env("OULIPOLY_DATA_DIR", &data)
                .env("OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES", "1")
                .current_dir(root)
                .process_group(0)
                .stdin(Stdio::null())
                .stdout(fs::File::create(&stdout).unwrap())
                .stderr(fs::File::create(&stderr).unwrap());
            match mode {
                "headless" => {
                    command.args(["--model", "fixture-model", "probe"]);
                }
                "model-repl" => {
                    command.args(["repl", "fixture-model"]);
                }
                _ => {
                    command.arg("--new");
                }
            }
            let ready = root.join("custody-ready");
            command
                .env("OULIPOLY_STARTING_CUSTODY_TEST_BARRIER", barrier)
                .env("OULIPOLY_STARTING_CUSTODY_TEST_READY", &ready);
            let mut creator = OwnedChild(command.spawn().unwrap(), false);
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready.exists() {
                assert!(
                    Instant::now() < deadline,
                    "{mode}/{barrier}: initial startup never reached crash barrier: {}",
                    fs::read_to_string(&stderr).unwrap()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            let conn = rusqlite::Connection::open(&sidecar).unwrap();
            let initial_generation: String = conn.query_row("SELECT generation_uuid FROM runtime_generation WHERE lifecycle_state = 'starting'", [], |row| row.get(0)).unwrap();
            // External launch's OS process exists before publication, but the
            // host intentionally has not sent its request on stdin yet. The
            // private core/state fixtures separately exercise live descendants.
            let held_workload = barrier == "before_publication" && mode != "headless";
            if held_workload {
                while !marker.exists() {
                    assert!(
                        Instant::now() < deadline,
                        "{mode}: direct unpublished workload never ran"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
            } else {
                fs::write(root.join("release-workload"), b"release").unwrap();
            }
            creator.0.kill().unwrap();
            while !fs::read_to_string(format!("/proc/{}/stat", creator.0.id()))
                .unwrap()
                .rsplit_once(") ")
                .unwrap()
                .1
                .starts_with('Z')
            {
                assert!(Instant::now() < deadline, "creator did not become zombie");
                std::thread::sleep(Duration::from_millis(5));
            }
            command
                .env_remove("OULIPOLY_STARTING_CUSTODY_TEST_BARRIER")
                .env_remove("OULIPOLY_STARTING_CUSTODY_TEST_READY");
            let mut runner = OwnedChild(command.spawn().unwrap(), false);
            let deadline = Instant::now() + Duration::from_secs(15);
            if held_workload {
                loop {
                    let queued: i64 = conn
                        .query_row("SELECT count(*) FROM session_admission_queue", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    if queued >= 2 {
                        break;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "successor never entered admission"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                let mut observer = MailboxDb::open(&sidecar).unwrap();
                assert!(matches!(
                    observer
                        .session_admissions()
                        .try_admit_next("test-observation", 0, 4)
                        .unwrap(),
                    oulipoly_state::mailbox::SessionAdmissionAttempt::LaunchMaterializing
                ));
                assert_eq!(fs::read_to_string(&marker).unwrap().lines().count(), 1);
                fs::write(root.join("release-workload"), b"release").unwrap();
            }
            let status = loop {
                // WNOWAIT keeps the exact fixture leader reserved until group cleanup.
                let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
                let result = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        runner.0.id(),
                        info.as_mut_ptr(),
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                assert_eq!(result, 0);
                if unsafe { info.assume_init().si_pid() } != 0 {
                    unsafe {
                        libc::killpg(runner.0.id() as i32, libc::SIGKILL);
                    }
                    let status = runner.0.wait().unwrap();
                    runner.1 = true;
                    break status;
                }
                assert!(
                    Instant::now() < deadline,
                    "{mode} timed out: {}",
                    fs::read_to_string(&stderr).unwrap()
                );
                std::thread::sleep(Duration::from_millis(25));
            };
            let stderr_text = fs::read_to_string(&stderr).unwrap();
            if mode == "default-repl" {
                // This tiny provider deliberately reports no live session identity.
                // Admission/settlement must not fabricate user-workload success.
                assert_eq!(status.code(), Some(1), "{stderr_text}");
                assert!(
                    stderr_text.contains("live_session_identity_unavailable"),
                    "{stderr_text}"
                );
            } else {
                assert!(
                    status.success(),
                    "{mode}: {status}: stderr={stderr_text} stdout={}",
                    fs::read_to_string(&stdout).unwrap()
                );
            }
            assert!(
                marker.exists(),
                "{mode}: successor did not enter provider startup"
            );
            let initial: (String, Option<String>) = conn.query_row("SELECT lifecycle_state, terminal_reason FROM runtime_generation WHERE generation_uuid = ?1", [&initial_generation], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
            assert_eq!(
                initial,
                ("exited".into(), Some("recovered_dead".into())),
                "{mode}/{barrier}"
            );
            assert_eq!(
                fs::read_to_string(&marker).unwrap().lines().count(),
                if held_workload { 2 } else { 1 },
                "{mode}/{barrier}"
            );
            let conn = rusqlite::Connection::open(&sidecar).unwrap();
            let (state, generation): (String, Option<String>) = conn.query_row(
            "SELECT state, runtime_generation_uuid FROM session_admission_queue WHERE runtime_generation_uuid != ?1", [&initial_generation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
            assert_eq!(state, "settled", "{mode}");
            assert!(
                generation.is_some(),
                "{mode}: no Starting generation was published"
            );
            // Keep the original owner unreaped through the entire successor execution.
            assert!(
                fs::read_to_string(format!("/proc/{}/stat", creator.0.id()))
                    .unwrap()
                    .rsplit_once(") ")
                    .unwrap()
                    .1
                    .starts_with('Z')
            );
        }
    }
}
