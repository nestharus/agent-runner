#![cfg(all(target_os = "linux", not(feature = "age319-private-broker-fixture")))]

use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_launch_ledger::InstalledLaunchLedger;
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::protocol::{self, EntryRoute, Operation};
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
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
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "disposable_root_featureless_l_help_lost_reply_and_duplicate",
                "--nocapture",
            ])
            .env("AGE319_FEATURELESS_CHILD", "1")
            .env("AGE319_FEATURELESS_RUNNER_BIN", runner_image)
            .env("AGE319_FEATURELESS_BASH_BIN", bash_image)
            .status()
            .unwrap();
        assert!(status.success(), "featureless disposable root failed");
        return;
    }

    // State's trust check includes every ancestor. /tmp and this worktree's
    // shared parent are writable, so use a short-lived root-owned home child.
    let home = std::env::var_os("HOME").expect("disposable test needs a trusted home directory");
    let temp = tempfile::Builder::new()
        .prefix("age319-featureless-")
        .tempdir_in(home)
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
    let socket = temp.path().join("control.sock");
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
    let deadline = Instant::now() + Duration::from_secs(10);
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
    for copied_fd in [false, true] {
        let mut direct = Command::new(&runner);
        direct
            .arg("--help")
            .env_clear()
            .env("HOME", temp.path())
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest);
        if copied_fd {
            direct.env("OULIPOLY_KERNEL_CONNECTED_CONTROL_FD_V1", "3");
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
            .contains("OULIPOLY_INSTALLED_LAUNCH_GAP=")
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
            .contains("OULIPOLY_INSTALLED_LAUNCH_GAP=")
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
    let deadline = Instant::now() + Duration::from_secs(25);
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
    let deadline = Instant::now() + Duration::from_secs(30);
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
    fs::write(
        &provider,
        b"#!/bin/sh\nexport OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1=\"$AGE319_BASH_CONTROL_SOCKET\"\n\"$AGE319_BASH_IMAGE\" run --delivery sync -- /bin/sh -c 'printf \"bash\\n\" >> \"$AGE319_EFFECT_FILE\"'\n",
    )
    .unwrap();
    let bash_id = uuid::Uuid::new_v4().to_string();
    let bash_out = temp.path().join("bash.out");
    let bash_err = temp.path().join("bash.err");
    let mut bash_launch = Command::new(&launcher)
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
        .stderr(Stdio::from(File::create(&bash_err).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(35);
    let bash_status = loop {
        if let Some(status) = bash_launch.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            bash_launch.kill().unwrap();
            panic!(
                "Bash launcher timed out: {} / broker: {}",
                fs::read_to_string(&bash_err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let bash_error = fs::read_to_string(&bash_err).unwrap();
    assert!(
        !bash_status.success(),
        "Bash unexpectedly crossed the closed scope probe"
    );
    assert!(
        bash_error.contains("fresh Bash parent probe refused: error Bash child scope uncertain"),
        "Bash: {bash_error} / broker: {}",
        fs::read_to_string(&broker_log).unwrap()
    );
    assert_eq!(fs::read_to_string(&effect).unwrap(), "one\n");
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 3);
    assert_eq!(
        ledger.read_terminal(&bash_id).unwrap().unwrap().exit_code,
        bash_status.code().unwrap() as u8
    );
    broker.kill().unwrap();
    broker.wait().unwrap();
}
