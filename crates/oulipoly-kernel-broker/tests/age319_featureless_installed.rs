#![cfg(all(target_os = "linux", not(feature = "age319-private-broker-fixture")))]

use base64::Engine as _;
use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::identity::PinnedProcess;
use oulipoly_kernel_broker::installed_launch_ledger::InstalledLaunchLedger;
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::protocol::{self, EntryRoute, Operation};
use oulipoly_kernel_broker::successor_launch::{self, Ledger as SuccessorLaunchLedger};
use oulipoly_state::mailbox::{EmptyV30BootstrapIdentity, FreshBashSourceEvent, FreshV30Lane};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[test]
fn disposable_root_featureless_l_help_lost_reply_and_duplicate() {
    let Ok(runner_image) = std::env::var("AGE319_FEATURELESS_RUNNER_BIN") else {
        return;
    };
    let Ok(bash_image) = std::env::var("AGE319_FEATURELESS_BASH_BIN") else {
        return;
    };
    if std::env::var_os("AGE319_FEATURELESS_CHILD").is_none() {
        let cases: &[bool] = if std::env::var_os("AGE319_FEATURELESS_BRIDGE_ONLY_V1").is_some()
            || std::env::var_os("AGE319_FEATURELESS_ONLY_BUSY").is_some()
        {
            &[false]
        } else if std::env::var_os("AGE319_FEATURELESS_ONLY_SUCCESSOR").is_some() {
            &[true]
        } else {
            &[false, true]
        };
        for &successor_case in cases {
            let mut command = Command::new("unshare");
            command
                .args(["-Urpfm", "--mount-proc"])
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "disposable_root_featureless_l_help_lost_reply_and_duplicate",
                    "--nocapture",
                ])
                .env("AGE319_FEATURELESS_CHILD", "1")
                .env("AGE319_FEATURELESS_RUNNER_BIN", &runner_image)
                .env("AGE319_FEATURELESS_BASH_BIN", &bash_image);
            if successor_case {
                command.env("AGE319_FEATURELESS_SUCCESSOR_CASE", "1");
            }
            let status = command.status().unwrap();
            assert!(
                status.success(),
                "featureless disposable root failed (successor={successor_case})"
            );
        }
        return;
    }
    let successor_case = std::env::var_os("AGE319_FEATURELESS_SUCCESSOR_CASE").is_some();
    let prior_sync_case =
        !successor_case && std::env::var_os("AGE319_FEATURELESS_ONLY_BUSY").is_none();

    // TMPDIR keeps every disposable artifact under the owned writer. State
    // still checks this directory and all its ancestors in the namespace.
    let temp = tempfile::Builder::new()
        .prefix("age319-featureless-")
        .tempdir()
        .unwrap();
    let installed = temp.path().join("installed");
    fs::create_dir(&installed).unwrap();
    let runner = installed.join("oulipoly-agent-runner");
    let broker_image = installed.join("oulipoly-kernel-broker");
    let launcher = installed.join("oulipoly-installed-launcher");
    let bash = installed.join("agent-bash");
    fs::copy(runner_image, &runner).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"), &broker_image).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_oulipoly-installed-launcher"), &launcher).unwrap();
    fs::copy(bash_image, &bash).unwrap();
    for image in [&runner, &broker_image, &launcher, &bash] {
        fs::set_permissions(image, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let manifest = installed.join("install-v1.json");
    let pair = InstalledPair {
        schema: 2,
        version: env!("CARGO_PKG_VERSION").into(),
        generation: uuid::Uuid::new_v4().to_string(),
        runner_sha256: digest(&runner),
        broker_sha256: digest(&broker_image),
        launcher_sha256: Some(digest(&launcher)),
        bash_sha256: Some(digest(&bash)),
    };
    fs::write(&manifest, serde_json::to_vec(&pair).unwrap()).unwrap();
    fs::set_permissions(&manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let state = temp.path().join("state");
    let source = EmptyV30BootstrapIdentity::bootstrap_at(&state).unwrap();
    let activation = FirstInstallActivation::activate_at(
        &state,
        PairPaths {
            manifest: &manifest,
            runner: &runner,
            broker: &broker_image,
            launcher: &launcher,
            bash: &bash,
        },
        true,
    )
    .unwrap();
    assert_eq!(
        activation.source.source_generation,
        source.source_generation
    );
    // A procfd path names this same test-owned directory without exceeding
    // Unix socket pathname limits in a deeply nested writer checkout.
    let socket_directory = File::open(temp.path()).unwrap();
    let socket = std::path::PathBuf::from(format!(
        "/proc/{}/fd/{}/control.sock",
        std::process::id(),
        socket_directory.as_raw_fd(),
    ));
    let broker_log = temp.path().join("broker.err");
    let mut broker = Command::new(&broker_image)
        .env_clear()
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &bash)
        .stderr(Stdio::from(File::create(&broker_log).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while protocol::observe_entry_gate_at(&socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
        assert!(
            broker.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(&broker_log).unwrap()
        );
        assert!(Instant::now() < deadline, "Broker did not open fresh route");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        protocol::request_at(&socket, Operation::ReserveV30Entry)
            .unwrap()
            .starts_with("error ")
    );
    for (entry_arg, copied_fd) in ["--help", successor_launch::ENTRY_ARG]
        .into_iter()
        .flat_map(|arg| [false, true].into_iter().map(move |copied| (arg, copied)))
    {
        let mut direct = Command::new(&runner);
        direct
            .arg(entry_arg)
            .env_clear()
            .env("HOME", temp.path())
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest);
        if copied_fd {
            direct.env("OULIPOLY_KERNEL_CONNECTED_CONTROL_FD_V1", "3");
            direct.env(successor_launch::FD_ENV, "3");
        }
        direct.stdout(Stdio::null()).stderr(Stdio::null());
        assert!(!direct.status().unwrap().success());
    }
    let unsupported_err = temp.path().join("unsupported.err");
    let unsupported = Command::new(&launcher)
        .arg("--unsupported-mode")
        .env_clear()
        .env("HOME", temp.path())
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&unsupported_err).unwrap()))
        .status()
        .unwrap();
    assert!(!unsupported.success());
    assert!(
        fs::read_to_string(&unsupported_err)
            .unwrap()
            .contains("unsupported fresh-only CLI mode")
    );
    let wrong_image = installed.join("copied-launcher");
    fs::copy(&launcher, &wrong_image).unwrap();
    fs::set_permissions(&wrong_image, fs::Permissions::from_mode(0o755)).unwrap();
    let wrong_image_err = temp.path().join("wrong-image.err");
    let wrong_image_status = Command::new(&wrong_image)
        .arg("--help")
        .env_clear()
        .env("HOME", temp.path())
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&wrong_image_err).unwrap()))
        .status()
        .unwrap();
    assert!(!wrong_image_status.success());
    assert!(
        fs::read_to_string(&wrong_image_err)
            .unwrap()
            .contains("running executable is not the installed image")
    );
    let stale_manifest = installed.join("stale-install.json");
    let mut stale_pair = pair.clone();
    stale_pair.generation = uuid::Uuid::new_v4().to_string();
    fs::write(&stale_manifest, serde_json::to_vec(&stale_pair).unwrap()).unwrap();
    fs::set_permissions(&stale_manifest, fs::Permissions::from_mode(0o600)).unwrap();
    let stale_err = temp.path().join("stale.err");
    let stale = Command::new(&launcher)
        .arg("--help")
        .env_clear()
        .env("HOME", temp.path())
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &stale_manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&stale_err).unwrap()))
        .status()
        .unwrap();
    assert!(!stale.success());
    assert!(
        fs::read_to_string(&stale_err)
            .unwrap()
            .contains("installed launcher generation mismatch")
    );
    assert_eq!(
        fs::read_dir(state.join("installed-launches"))
            .unwrap()
            .count(),
        0
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    let launch_out = temp.path().join("launch.out");
    let launch_err = temp.path().join("launch.err");
    let mut child = Command::new(&launcher)
        .arg("--help")
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &request_id)
        .env("OULIPOLY_AGE319_PRIVATE_DROP_L_REPLY_V1", "1")
        .env("OULIPOLY_AGE319_PRIVATE_DUPLICATE_L_V1", "1")
        .stdout(Stdio::from(File::create(&launch_out).unwrap()))
        .stderr(Stdio::from(File::create(&launch_err).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    let exit = loop {
        if let Some(exit) = child.try_wait().unwrap() {
            break exit;
        }
        assert!(
            Instant::now() < deadline,
            "launcher timed out: {} / {}",
            fs::read_to_string(&launch_err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        exit.success(),
        "launcher: {} / broker: {}",
        fs::read_to_string(&launch_err).unwrap(),
        fs::read_to_string(&broker_log).unwrap()
    );
    let records: Vec<_> = fs::read_dir(state.join("installed-launches"))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(records.len(), 1);
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(records[0].path()).unwrap()).unwrap();
    assert_eq!(record["request_id"], request_id);
    assert_eq!(record["pair_generation"], pair.generation);
    assert_eq!(record["source_generation"], source.source_generation);
    assert_eq!(record["owner_uid"], 0);
    let root_id = record["root_id"].as_str().unwrap();
    assert!(
        state
            .join("installed-normal-terminals")
            .join(format!("{request_id}.json"))
            .exists()
    );
    let ledger =
        InstalledLaunchLedger::open(&state, &pair.generation, &source.source_generation).unwrap();
    assert_eq!(
        ledger
            .read_terminal(&request_id)
            .unwrap()
            .unwrap()
            .exit_code,
        0
    );
    assert!(
        state
            .join("entries")
            .join(format!("{root_id}.json"))
            .exists()
    );
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    let wrong_owner = Command::new(&launcher)
        .arg("--help")
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &request_id)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!wrong_owner.success());
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    assert_eq!(
        fs::read_dir(state.join("installed-launches"))
            .unwrap()
            .count(),
        1
    );
    let config_home = temp.path().join("config-home");
    let config = config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(config.join("models")).unwrap();
    let provider = temp.path().join("provider.sh");
    let effect = temp.path().join("provider-effect");
    let cwd_effect = temp.path().join("provider-cwd");
    let prompt_effect = temp.path().join("provider-prompt");
    fs::write(
        &provider,
        b"#!/bin/sh\nprintf 'one\\n' >> \"$AGE319_EFFECT_FILE\"\npwd -P > \"$AGE319_CWD_FILE\"\ncat > \"$AGE319_PROMPT_FILE\"\ncat \"$AGE319_PROMPT_FILE\"\n",
    )
    .unwrap();
    fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        config.join("providers.toml"),
        format!(
            "[local]\ncommand = {}\nquota_account_id = 'physical-local'\n",
            serde_json::to_string(provider.to_str().unwrap()).unwrap()
        ),
    )
    .unwrap();
    fs::write(
        config.join("models/fixture-model.toml"),
        "[[providers]]\nname = 'local'\n",
    )
    .unwrap();
    let normal_id = uuid::Uuid::new_v4().to_string();
    let normal_out = temp.path().join("normal.out");
    let normal_err = temp.path().join("normal.err");
    let mut normal = Command::new(&launcher)
        .args(["--model", "fixture-model", "hello fixture"])
        .current_dir(temp.path())
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_CONFIG_HOME", &config_home)
        .env("OULIPOLY_DATA_DIR", temp.path().join("data"))
        .env("AGE319_EFFECT_FILE", &effect)
        .env("AGE319_CWD_FILE", &cwd_effect)
        .env("AGE319_PROMPT_FILE", &prompt_effect)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &normal_id)
        .stdout(Stdio::from(File::create(&normal_out).unwrap()))
        .stderr(Stdio::from(File::create(&normal_err).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    let normal_status = loop {
        if let Some(status) = normal.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            normal.kill().unwrap();
            panic!(
                "normal launcher timed out: {} / broker: {}",
                fs::read_to_string(&normal_err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        normal_status.success(),
        "normal: {} / broker: {}",
        fs::read_to_string(&normal_err).unwrap(),
        fs::read_to_string(&broker_log).unwrap()
    );
    assert_eq!(fs::read_to_string(&effect).unwrap(), "one\n");
    assert_eq!(
        fs::read_to_string(&cwd_effect).unwrap().trim_end(),
        temp.path().to_str().unwrap()
    );
    assert!(
        fs::read_to_string(&prompt_effect)
            .unwrap()
            .contains("hello fixture")
    );
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 2);
    assert!(
        state
            .join("installed-normal-terminals")
            .join(format!("{normal_id}.json"))
            .exists()
    );
    assert_eq!(
        ledger.read_terminal(&normal_id).unwrap().unwrap().exit_code,
        0
    );
    let terminal = if !prior_sync_case {
        ledger.read_terminal(&normal_id).unwrap().unwrap()
    } else {
        fs::write(
        &provider,
        b"#!/bin/sh\nexport OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1=\"$AGE319_BASH_CONTROL_SOCKET\"\n\"$AGE319_BASH_IMAGE\" run --delivery sync -- /bin/sh -c 'printf \"bash\\n\" >> \"$AGE319_EFFECT_FILE\"'\n",
    )
    .unwrap();
        let bash_id = uuid::Uuid::new_v4().to_string();
        let bash_out = temp.path().join("bash.out");
        let bash_err = temp.path().join("bash.err");
        let mut bash_command = Command::new(&launcher);
        bash_command
            .args(["--model", "fixture-model", "hello bash"])
            .env_clear()
            .env("HOME", temp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("OULIPOLY_CONFIG_HOME", &config_home)
            .env("OULIPOLY_DATA_DIR", temp.path().join("data"))
            .env("AGE319_EFFECT_FILE", &effect)
            .env("AGE319_BASH_IMAGE", &bash)
            .env("AGE319_BASH_CONTROL_SOCKET", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &bash_id)
            .stdout(Stdio::from(File::create(&bash_out).unwrap()))
            .stderr(Stdio::from(File::create(&bash_err).unwrap()));
        if !successor_case {
            bash_command
                .env("AGE319_PRIVATE_ORDINARY_DROP_C_REPLY_V1", "1")
                .env("AGE319_PRIVATE_ORDINARY_DROP_K_REPLY_V1", "1")
                .env("AGE319_PRIVATE_ORDINARY_DROP_Q_REPLY_V1", "1")
                .env("AGE319_PRIVATE_ORDINARY_DROP_W_REPLY_V1", "1")
                .env("AGE319_PRIVATE_SYNC_DROP_BEGIN_REPLY_V1", "1");
        }
        let mut bash_launch = bash_command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        let bash_status = loop {
            if let Some(status) = bash_launch.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                bash_launch.kill().unwrap();
                panic!(
                    "Bash launcher timed out: {} / output: {} / effect: {} / broker: {}",
                    fs::read_to_string(&bash_err).unwrap(),
                    fs::read_to_string(&bash_out).unwrap(),
                    fs::read_to_string(&effect).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        assert!(
            bash_status.success(),
            "Bash: {} / broker: {}",
            fs::read_to_string(&bash_err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        assert_eq!(fs::read_to_string(&effect).unwrap(), "one\nbash\n");
        assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 3);
        let terminal = ledger.read_terminal(&bash_id).unwrap().unwrap();
        assert_eq!(terminal.exit_code, 0);
        assert!(terminal.physical.pid1_echild_receipt);
        assert!(terminal.physical.pid1_parent_wait_proof);
        assert_eq!(terminal.physical.source_effect.accepted, 0);
        assert_eq!(
            terminal.physical.work_retired,
            terminal.physical.work_records
        );
        let fresh = rusqlite::Connection::open(state.join("v30/state.db")).unwrap();
        let events: Vec<String> = {
            let mut statement = fresh
                .prepare("SELECT receipt_json FROM fresh_bash_selected_event")
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        let [event] = events.as_slice() else {
            panic!(
                "expected one selected Bash source event, got {}",
                events.len()
            );
        };
        let selected: FreshBashSourceEvent = serde_json::from_str(event).unwrap();
        assert_eq!(selected.root_id, terminal.physical.root_id);
        assert!(selected.tree_drained && selected.output_closed);
        let publications: i64 = fresh
            .query_row(
                "SELECT count(*) FROM fresh_bash_sync_publication",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(publications, 1);
        drop(fresh);
        terminal
    };

    // The provider exits before the held Bash child reaches W. Normal PID1
    // retains the child's physical reservation and the original D-bound
    // Runner stays live through Q; the provider cannot supply F bytes.
    fs::write(
        &provider,
        br#"#!/bin/sh
export OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1="$AGE319_BASH_CONTROL_SOCKET"
response=$("$AGE319_BASH_IMAGE" run --delivery async -- /bin/sh -c 'while [ ! -f "$AGE319_CHILD_RELEASE_FILE" ]; do sleep 0.02; done; printf "async\n" >> "$AGE319_EFFECT_FILE"; printf "async-child-out\n"; printf "async-child-err\n" >&2') || exit
printf '%s\n' "$response"
if [ -n "$AGE319_PROVIDER_HOLD_FILE" ]; then
  while [ ! -f "$AGE319_PROVIDER_HOLD_FILE" ]; do sleep 0.02; done
fi
"#,
    )
    .unwrap();
    let async_id = uuid::Uuid::new_v4().to_string();
    let async_out = temp.path().join("async.out");
    let async_err = temp.path().join("async.err");
    let child_release = temp.path().join("async-child-release");
    let provider_release = temp.path().join("async-provider-release");
    let mut async_command = Command::new(&launcher);
    async_command
        .args(["--model", "fixture-model", "hello async"])
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_CONFIG_HOME", &config_home)
        .env("OULIPOLY_DATA_DIR", temp.path().join("data"))
        .env("AGE319_EFFECT_FILE", &effect)
        .env("AGE319_BASH_IMAGE", &bash)
        .env("AGE319_BASH_CONTROL_SOCKET", &socket)
        .env("AGE319_CHILD_RELEASE_FILE", &child_release)
        .env(
            "AGE319_PROVIDER_HOLD_FILE",
            if successor_case {
                ""
            } else {
                provider_release.to_str().unwrap()
            },
        )
        .env("AGE319_TEST_FEATURELESS_DROP_START_REPLY_V1", "1")
        .env("AGE319_TEST_FEATURELESS_DROP_F_REPLY_V1", "1")
        .env("AGE319_TEST_FEATURELESS_DROP_ACK_REPLY_V1", "1")
        .env("AGE319_TEST_FEATURELESS_REPLAY_ACK_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &async_id)
        .stdout(Stdio::from(File::create(&async_out).unwrap()))
        .stderr(Stdio::from(File::create(&async_err).unwrap()));
    let mut async_launch = async_command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let fresh = rusqlite::Connection::open(state.join("v30/state.db")).unwrap();
        let count: i64 = fresh
            .query_row("SELECT count(*) FROM fresh_bash_child", [], |row| {
                row.get(0)
            })
            .unwrap();
        if count == if prior_sync_case { 2 } else { 1 } {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "async C absent: {}",
            fs::read_to_string(&async_err).unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(async_launch.try_wait().unwrap().is_none());
    let (async_root, async_parent): (String, String) =
        rusqlite::Connection::open(state.join("v30/state.db"))
            .unwrap()
            .query_row(
                "SELECT root_id,json_extract(receipt_json,'$.parent_work_grant_id')
             FROM fresh_bash_child WHERE root_id!=?1",
                [&terminal.physical.root_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    let physical = state.join("v30/normal-provider").join(&async_parent);
    if successor_case {
        while !physical.join("provider-exit.json").exists() {
            assert!(
                Instant::now() < deadline,
                "provider did not exit before child W: {} / {}",
                fs::read_to_string(&async_err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    } else {
        assert!(physical.join("provider-start.json").exists());
        assert!(!physical.join("provider-exit.json").exists());
    }
    assert!(
        !physical.join("q.json").exists(),
        "normal Q preceded live child"
    );
    let lane = FreshV30Lane::open_at(&state).unwrap();
    let (_, original_actor) = lane.released_handoff_for_root(&async_root).unwrap();
    let pinned_original = PinnedProcess::open(original_actor.host_pid).unwrap();
    pinned_original.verify().unwrap();
    assert!(
        !pinned_original.exited().unwrap(),
        "original Runner exited before W"
    );
    assert_eq!(
        pinned_original.starttime_ticks,
        original_actor.starttime_ticks
    );
    assert!(
        pinned_original
            .same_executable_as(&File::open(&runner).unwrap())
            .unwrap()
    );
    assert!(async_launch.try_wait().unwrap().is_none());
    fs::write(&child_release, b"release").unwrap();
    let selected: FreshBashSourceEvent = loop {
        let fresh = rusqlite::Connection::open(state.join("v30/state.db")).unwrap();
        let values: Vec<String> = fresh
            .prepare("SELECT receipt_json FROM fresh_bash_selected_event")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        if values.len() == if prior_sync_case { 2 } else { 1 } {
            let selected: Vec<FreshBashSourceEvent> = values
                .iter()
                .map(|value| serde_json::from_str(value).unwrap())
                .collect();
            break selected
                .into_iter()
                .find(|value| value.root_id != terminal.physical.root_id)
                .unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "async selected W absent: {} / {}",
            fs::read_to_string(&async_err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(selected.tree_drained && selected.output_closed);
    let selection = selected.normal_provider_selection.as_ref().unwrap();
    assert_eq!(selection.admission_id, async_parent);
    assert_eq!(
        selection.mode,
        if successor_case { "sleeping" } else { "busy" }
    );
    if successor_case {
        let wait: serde_json::Value =
            serde_json::from_slice(&fs::read(physical.join("provider-exit.json")).unwrap())
                .unwrap();
        assert_eq!(wait["admission_id"], selection.admission_id);
        assert_eq!(
            wait["provider_wait_status"],
            selection.provider_wait_status.unwrap()
        );
    } else {
        let provider = selection.provider.as_ref().unwrap();
        let start: serde_json::Value =
            serde_json::from_slice(&fs::read(physical.join("provider-start.json")).unwrap())
                .unwrap();
        assert_eq!(start["host_pid"], provider.host_pid);
        assert_eq!(start["starttime_ticks"], provider.starttime_ticks);
        assert_eq!(start["local_pid"], selection.provider_local_pid.unwrap());
        let live = PinnedProcess::open(provider.host_pid).unwrap();
        assert_eq!(live.starttime_ticks, provider.starttime_ticks);
        assert!(
            !live.exited().unwrap(),
            "busy W provider exited before selection"
        );
        let (root, actor) = lane.released_handoff_for_root(&async_root).unwrap();
        let session = lane.read_session(&root.d_key).unwrap().unwrap();
        let pending = lane
            .read_private_root_terminal(&root, &actor, &session)
            .unwrap();
        assert_eq!(pending.selected_child_event, Some(selected.clone()));
        assert!(pending.execution.is_none(), "parent Q preceded busy W");
        fs::write(&provider_release, b"release").unwrap();
    }
    if std::env::var_os("AGE319_FEATURELESS_BRIDGE_ONLY_V1").is_some() {
        let (root, actor) = lane.released_handoff_for_root(&async_root).unwrap();
        let session = lane.read_session(&root.d_key).unwrap().unwrap();
        assert!(
            !state
                .join("v30/fresh-provider")
                .join(format!("{}.fresh-grant.json", root.handoff_id))
                .exists(),
            "normal K unexpectedly used the old physical grant"
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        let read = loop {
            let read = lane
                .settle_private_root_terminal(&root, &actor, &session)
                .unwrap();
            if read.execution_state == "success" {
                break read;
            }
            assert!(
                Instant::now() < deadline,
                "normal K/Q terminal bridge did not settle: {read:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        let parent = &read.execution.as_ref().unwrap().parent;
        assert_eq!(parent.grant_id, async_parent);
        assert_eq!(parent.work_id, async_parent);
        assert_eq!(parent.outcome, "exit_success");
        assert_eq!(read.selected_child_event, Some(selected));
        async_launch.kill().unwrap();
        async_launch.wait().unwrap();
        broker.kill().unwrap();
        broker.wait().unwrap();
        return;
    }
    let async_status = loop {
        if let Some(status) = async_launch.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "async launcher timed out: stderr={} stdout={} candidate={} starts={} candidates={} broker={} state={}",
            fs::read_to_string(&async_err).unwrap(),
            fs::read_to_string(&async_out).unwrap(),
            fs::read_to_string(temp.path().join("successor-child.err")).unwrap_or_default(),
            fs::read_dir(state.join("installed-successor-starts"))
                .unwrap()
                .count(),
            fs::read_dir(state.join("installed-successor-candidates"))
                .unwrap()
                .count(),
            fs::read_to_string(&broker_log).unwrap(),
            {
                let lane = FreshV30Lane::open_at(&state).unwrap();
                let (root, actor) = lane.released_handoff_for_root(&selected.root_id).unwrap();
                let session = lane.read_session(&root.d_key).unwrap().unwrap();
                format!(
                    "{:?}",
                    lane.read_private_root_terminal(&root, &actor, &session)
                )
            }
        );
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        async_status.success(),
        "async: {} / candidate: {} / broker: {}",
        fs::read_to_string(&async_err).unwrap(),
        fs::read_to_string(temp.path().join("successor-child.err")).unwrap_or_default(),
        fs::read_to_string(&broker_log).unwrap()
    );
    assert_eq!(
        fs::read_to_string(&effect).unwrap(),
        if prior_sync_case {
            "one\nbash\nasync\n"
        } else {
            "one\nasync\n"
        }
    );
    let dispatch: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&async_out).unwrap()).unwrap();
    assert_eq!(dispatch["delivery_mode"], "async");
    assert_eq!(dispatch["dispatch_state"], "broker-k-consumed");
    assert_eq!(dispatch["request_id"], selected.request_id);
    let (released, actor) = lane.released_handoff_for_root(&selected.root_id).unwrap();
    let session = lane.read_session(&released.d_key).unwrap().unwrap();
    let settled = lane
        .read_private_root_terminal(&released, &actor, &session)
        .unwrap();
    assert_eq!(settled.execution_state, "success");
    assert_eq!(settled.notification_state, "acked");
    if std::env::var_os("AGE319_FEATURELESS_ONLY_BUSY").is_some() {
        broker.kill().unwrap();
        broker.wait().unwrap();
        return;
    }
    let async_terminal = ledger.read_terminal(&async_id).unwrap().unwrap();
    let lane = FreshV30Lane::open_at(&state).unwrap();
    let obligation = lane
        .read_bash_wake_obligation(&selected.request_id)
        .unwrap()
        .expect("selected async W lacks durable wake obligation");
    assert_eq!(obligation.source_id, selected.source_id);
    assert_eq!(obligation.attempt_id, selected.attempt_id);
    assert_eq!(obligation.root_id, selected.root_id);
    assert_eq!(obligation.owner_generation, selected.owner_generation);
    assert_eq!(obligation.lane_id, selected.lane_id);
    assert_eq!(obligation.source_generation, selected.source_generation);
    let (original_root, original_actor) =
        lane.released_handoff_for_root(&selected.root_id).unwrap();
    assert_eq!(
        obligation.session_id,
        lane.read_session(&original_root.d_key)
            .unwrap()
            .unwrap()
            .session_id
    );
    assert_eq!(obligation.original_identity, original_actor);
    if successor_case {
        let decision = lane
            .read_bash_wake_successor_decision(&selected.request_id, &original_actor)
            .unwrap()
            .unwrap_or_else(|| {
                panic!(
                    "original did not choose selected W: terminal={async_terminal:?} marker={} original_stderr={} broker={}",
                    selection.mode,
                    fs::read_to_string(&async_err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                )
            });
        let launch_ledger =
            SuccessorLaunchLedger::open(&state, &pair.generation, &source.source_generation)
                .unwrap();
        let start = launch_ledger
            .read_start(&decision.offer_request_id)
            .unwrap()
            .expect("Broker did not persist start");
        let candidate = launch_ledger
            .read_candidate(&decision.offer_request_id)
            .unwrap()
            .expect("Broker did not persist candidate");
        assert_eq!(start.decision, decision);
        assert_eq!(start.original.host_pid, original_actor.host_pid);
        assert_ne!(candidate.process.host_pid, original_actor.host_pid);
        assert_eq!(candidate.owner_uid, start.owner_uid);
        let candidate_actor = oulipoly_state::mailbox::FreshRecipientIdentity {
            host_pid: candidate.process.host_pid,
            boot_id: candidate.process.boot_id.clone(),
            starttime_ticks: candidate.process.starttime_ticks,
            pidns_dev: candidate.process.pidns_dev,
            pidns_ino: candidate.process.pidns_ino,
        };
        let offer = lane
            .read_successor_offer(&decision.offer_request_id, &candidate_actor)
            .unwrap()
            .expect("installed candidate did not offer");
        assert_eq!(offer.root_id, obligation.root_id);
        assert_eq!(offer.session_id, obligation.session_id);
        assert_eq!(offer.seq, obligation.seq);
        assert_eq!(offer.source_id, obligation.source_id);
        assert_eq!(offer.attempt_id, obligation.attempt_id);
        assert_eq!(offer.payload_sha256, obligation.payload_sha256);
        let ack = async_terminal
            .successor_ack
            .as_ref()
            .expect("installed terminal omitted successor ACK");
        assert_eq!(async_terminal.exit_code, 0);
        assert_eq!(ack.offer_request_id, decision.offer_request_id);
        assert_eq!(ack.successor_identity, candidate_actor);
        assert!(async_terminal.original_receipt.is_none());
        assert_eq!(
            fs::read_dir(state.join("installed-successor-starts"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(state.join("installed-successor-candidates"))
                .unwrap()
                .count(),
            1
        );
        broker.kill().unwrap();
        broker.wait().unwrap();
        let mut broker = Command::new(&broker_image)
            .env_clear()
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &broker_image)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &bash)
            .stderr(Stdio::from(File::create(&broker_log).unwrap()))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while protocol::observe_entry_gate_at(&socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
            let status = broker.try_wait().unwrap();
            assert!(
                status.is_none(),
                "successor Broker restart exited {status:?}: {}",
                fs::read_to_string(&broker_log).unwrap()
            );
            assert!(
                Instant::now() < deadline,
                "successor Broker restart did not reopen: {}",
                fs::read_to_string(&broker_log).unwrap()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let reopened =
            SuccessorLaunchLedger::open(&state, &pair.generation, &source.source_generation)
                .unwrap();
        assert_eq!(
            reopened.read_start(&decision.offer_request_id).unwrap(),
            Some(start)
        );
        assert_eq!(
            reopened.read_candidate(&decision.offer_request_id).unwrap(),
            Some(candidate)
        );
        assert_eq!(
            ledger.read_terminal(&async_id).unwrap().unwrap(),
            async_terminal
        );
        broker.kill().unwrap();
        broker.wait().unwrap();
        return;
    }
    assert!(
        lane.read_bash_wake_successor_decision(&selected.request_id, &original_actor)
            .unwrap()
            .is_none(),
        "an original-recipient F must not invent a successor choice"
    );
    let wake_count: i64 = rusqlite::Connection::open(state.join("v30/state.db"))
        .unwrap()
        .query_row(
            "SELECT count(*) FROM fresh_bash_wake_obligation WHERE root_id=?1",
            [&selected.root_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(wake_count, 1);
    assert_eq!(async_terminal.exit_code, 0);
    assert!(async_terminal.physical.pid1_echild_receipt);
    assert!(async_terminal.physical.pid1_parent_wait_proof);
    assert_eq!(
        async_terminal.physical.work_retired,
        async_terminal.physical.work_records
    );
    let receipt_identity = async_terminal
        .original_receipt
        .as_ref()
        .expect("installed terminal omitted original receipt identity");
    assert_eq!(
        async_terminal.physical.original_receipt.as_ref(),
        Some(receipt_identity)
    );
    let owner_physical: serde_json::Value =
        serde_json::from_str(&async_terminal.owner.physical_proof_json).unwrap();
    assert_eq!(
        owner_physical["original_receipt"],
        serde_json::to_value(receipt_identity).unwrap()
    );
    let receipt_path = Path::new(&receipt_identity.receipt_path);
    assert!(receipt_path.starts_with(temp.path().join("state-original-receipts")));
    assert_eq!(digest(receipt_path), receipt_identity.receipt_sha256);
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(receipt["payload_base64"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        obligation.payload_sha256,
        format!("{:x}", Sha256::digest(&bytes))
    );
    assert_eq!(obligation.payload_byte_len, bytes.len() as i64);
    assert_eq!(obligation.seq, receipt["grant"]["seq"].as_i64().unwrap());
    assert_eq!(
        receipt["grant"]["payload_sha256"],
        format!("{:x}", Sha256::digest(&bytes))
    );
    let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(payload["source"], serde_json::to_value(&selected).unwrap());
    assert_eq!(
        payload["stdout_bytes"],
        serde_json::json!(b"async-child-out\n".to_vec())
    );
    assert_eq!(
        payload["stderr_bytes"],
        serde_json::json!(b"async-child-err\n".to_vec())
    );
    assert_eq!(receipt["grant"]["root_id"], async_terminal.physical.root_id);
    let sidecar = rusqlite::Connection::open(state.join("v30/sidecar/pid-identity.db")).unwrap();
    let (phase, attempts, basis): (String, i64, String) = sidecar
        .query_row(
            "SELECT g.phase,m.delivery_attempts,e.basis FROM fresh_recipient_grant g
         JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
         JOIN fresh_recipient_ack_evidence e ON e.grant_id=g.grant_id
         WHERE g.grant_id=?1",
            [receipt["grant"]["grant_id"].as_str().unwrap()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (phase.as_str(), attempts, basis.as_str()),
        ("acked", 1, "manual_ack")
    );
    assert_eq!(
        fs::read_dir(state.join("entries")).unwrap().count(),
        if prior_sync_case { 4 } else { 3 }
    );

    // Reopen both Broker loops from durable State before asking for another E.
    broker.kill().unwrap();
    broker.wait().unwrap();
    let mut broker = Command::new(&broker_image)
        .env_clear()
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &bash)
        .stderr(Stdio::from(File::create(&broker_log).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while protocol::observe_entry_gate_at(&socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
        assert!(
            broker.try_wait().unwrap().is_none(),
            "restarted Broker: {}",
            fs::read_to_string(&broker_log).unwrap()
        );
        assert!(Instant::now() < deadline, "Broker restart did not reopen");
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(
        ledger.read_terminal(&async_id).unwrap().unwrap(),
        async_terminal
    );
    assert_eq!(
        FreshV30Lane::open_at(&state)
            .unwrap()
            .read_bash_wake_obligation(&selected.request_id)
            .unwrap(),
        Some(obligation.clone())
    );
    assert!(
        FreshV30Lane::open_at(&state)
            .unwrap()
            .read_bash_wake_successor_decision(&selected.request_id, &original_actor)
            .unwrap()
            .is_none()
    );
    let stored_receipt = fs::read(receipt_path).unwrap();
    fs::set_permissions(receipt_path, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(receipt_path, b"tampered original receiver receipt").unwrap();
    assert!(ledger.read_terminal(&async_id).is_err());
    let lane = FreshV30Lane::open_at(&state).unwrap();
    let (released, actor) = lane
        .released_handoff_for_root(&async_terminal.physical.root_id)
        .unwrap();
    let session = lane.read_session(&released.d_key).unwrap().unwrap();
    let changed = lane
        .read_private_root_terminal(&released, &actor, &session)
        .unwrap();
    assert_ne!(changed.notification_state, "acked");
    assert!(
        oulipoly_kernel_broker::root_drain::exact_original_receipt_for_root(
            &lane,
            &async_terminal.physical.root_id
        )
        .is_err()
    );
    let tamper_err = temp.path().join("tamper-entry.err");
    let mut tamper_entry = Command::new(&launcher)
        .args(["--model", "fixture-model", "tampered prior receipt"])
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_CONFIG_HOME", &config_home)
        .env("OULIPOLY_DATA_DIR", temp.path().join("data"))
        .env("AGE319_EFFECT_FILE", &effect)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env(
            "OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1",
            uuid::Uuid::new_v4().to_string(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&tamper_err).unwrap()))
        .spawn()
        .unwrap();
    let tamper_deadline = Instant::now() + Duration::from_secs(15);
    let tamper_status = loop {
        if let Some(status) = tamper_entry.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= tamper_deadline {
            tamper_entry.kill().unwrap();
            panic!("tampered prior receipt did not refuse entry");
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        !tamper_status.success(),
        "tampered receipt admitted a new root"
    );
    assert_eq!(
        fs::read_to_string(&effect).unwrap(),
        if prior_sync_case {
            "one\nbash\nasync\n"
        } else {
            "one\nasync\n"
        }
    );
    assert_eq!(
        fs::read_dir(state.join("entries")).unwrap().count(),
        if prior_sync_case { 4 } else { 3 }
    );
    fs::write(receipt_path, &stored_receipt).unwrap();
    fs::set_permissions(receipt_path, fs::Permissions::from_mode(0o400)).unwrap();
    let restored = lane
        .read_private_root_terminal(&released, &actor, &session)
        .unwrap();
    assert_eq!(restored.original_receipt.as_ref(), Some(receipt_identity));

    // A later E must revalidate the fully closed async root as history.
    fs::write(
        &provider,
        b"#!/bin/sh\ncat >/dev/null\nprintf 'later\\n' >> \"$AGE319_EFFECT_FILE\"\nprintf 'later-ok\\n'\n",
    )
    .unwrap();
    let later_id = uuid::Uuid::new_v4().to_string();
    let later_err = temp.path().join("later.err");
    let mut later = Command::new(&launcher)
        .args(["--model", "fixture-model", "hello later"])
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_CONFIG_HOME", &config_home)
        .env("OULIPOLY_DATA_DIR", temp.path().join("data"))
        .env("AGE319_EFFECT_FILE", &effect)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &later_id)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&later_err).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let later_status = loop {
        if let Some(status) = later.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            later.kill().unwrap();
            panic!(
                "later launcher timed out: {} / broker: {}",
                fs::read_to_string(&later_err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert!(
        later_status.success(),
        "later: {} / broker: {}",
        fs::read_to_string(&later_err).unwrap(),
        fs::read_to_string(&broker_log).unwrap()
    );
    assert_eq!(
        fs::read_to_string(&effect).unwrap(),
        if prior_sync_case {
            "one\nbash\nasync\nlater\n"
        } else {
            "one\nasync\nlater\n"
        }
    );
    assert_eq!(
        fs::read_dir(state.join("entries")).unwrap().count(),
        if prior_sync_case { 5 } else { 4 }
    );
    assert_eq!(
        ledger.read_terminal(&later_id).unwrap().unwrap().exit_code,
        0
    );

    broker.kill().unwrap();
    broker.wait().unwrap();
}
