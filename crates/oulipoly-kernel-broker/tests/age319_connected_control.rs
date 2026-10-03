#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

use base64::Engine as _;
use oulipoly_kernel_broker::RootRecord;
use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::protocol::{
    self, EntryRoute, Operation, observe_entry_gate_at, request_at,
};
use oulipoly_state::mailbox::{EmptyV30BootstrapIdentity, FreshV30Lane};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

#[test]
fn root_mapped_connected_l_reaches_ordinary_help_with_exact_one_use_custody() {
    run_connected_case(false, ProofFault::None);
}

#[test]
fn root_mapped_connected_diagnostics_help_closes_without_effect() {
    run_connected_case(false, ProofFault::Diagnostics);
}

#[test]
fn root_mapped_connected_help_revalidates_offline_close_after_restart() {
    run_connected_case(false, ProofFault::Restart);
}

#[test]
fn root_mapped_connected_help_refuses_tampered_no_effect_certificate() {
    run_connected_case(false, ProofFault::OfflineTamper);
}

#[test]
fn root_mapped_connected_l_runs_normal_model_and_closes_owner() {
    run_connected_case(true, ProofFault::None);
}

#[test]
fn root_mapped_connected_l_runs_real_bash_sync_child_and_closes_owner() {
    run_connected_case_with_bash(true, ProofFault::None, true, false);
}

/// One fresh root owns two admitted sync Bash children. Each child keeps its
/// own C/W/response record, and the root terminal accounts for both.
#[test]
fn root_mapped_connected_l_runs_two_real_bash_sync_children_and_closes_owner() {
    run_connected_case_with_sync_children(2);
}

#[test]
fn root_mapped_connected_l_runs_three_real_bash_sync_children_and_closes_owner() {
    run_connected_case_with_sync_children(3);
}

#[test]
fn root_mapped_connected_l_delivers_real_bash_async_to_recipient_and_acks() {
    run_connected_case_with_bash(true, ProofFault::None, true, true);
}

#[test]
fn root_mapped_connected_l_delivers_real_bash_async_to_admitted_successor_and_acks() {
    run_connected_case_with_bash_mode(true, ProofFault::None, true, true, true, 1);
}

#[test]
fn root_mapped_connected_l_successor_ack_revalidates_after_broker_restart() {
    run_connected_case_with_bash_mode(true, ProofFault::Restart, true, true, true, 1);
}

#[test]
fn root_mapped_connected_l_successor_changed_receipt_refuses_certificate() {
    run_connected_case_with_bash_mode(
        true,
        ProofFault::SuccessorReceiptTamper,
        true,
        true,
        true,
        1,
    );
}

#[test]
fn root_mapped_connected_l_refuses_missing_q_after_certificate() {
    run_connected_case(true, ProofFault::MissingQ);
}

#[test]
fn root_mapped_connected_l_refuses_changed_q_after_certificate() {
    run_connected_case(true, ProofFault::ChangedQ);
}

#[test]
fn root_mapped_connected_l_revalidates_certificate_after_broker_restart() {
    run_connected_case(true, ProofFault::Restart);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProofFault {
    None,
    MissingQ,
    ChangedQ,
    Restart,
    OfflineTamper,
    SuccessorReceiptTamper,
    Diagnostics,
}

fn run_connected_case(normal: bool, fault: ProofFault) {
    run_connected_case_with_bash(normal, fault, false, false);
}

fn run_connected_case_with_bash(
    normal: bool,
    fault: ProofFault,
    real_bash: bool,
    async_bash: bool,
) {
    run_connected_case_with_bash_mode(normal, fault, real_bash, async_bash, false, 1);
}

fn run_connected_case_with_sync_children(children: usize) {
    run_connected_case_with_bash_mode(true, ProofFault::None, true, false, false, children);
}

fn run_connected_case_with_bash_mode(
    normal: bool,
    fault: ProofFault,
    real_bash: bool,
    async_bash: bool,
    successor_admission: bool,
    sync_children: usize,
) {
    assert!(!async_bash || real_bash);
    assert!(sync_children == 1 || (real_bash && !async_bash && !successor_admission));
    let Ok(runner_bin) = std::env::var("AGE319_CONNECTED_RUNNER_BIN") else {
        return;
    };
    let bash_bin = real_bash.then(|| {
        std::env::var("AGE319_CONNECTED_BASH_BIN")
            .expect("real Bash case requires AGE319_CONNECTED_BASH_BIN")
    });
    if std::env::var_os("AGE319_CONNECTED_CHILD_TEST").is_none() {
        let test_name = if sync_children == 2 {
            "root_mapped_connected_l_runs_two_real_bash_sync_children_and_closes_owner"
        } else if sync_children == 3 {
            "root_mapped_connected_l_runs_three_real_bash_sync_children_and_closes_owner"
        } else if successor_admission {
            match fault {
                ProofFault::None => {
                    "root_mapped_connected_l_delivers_real_bash_async_to_admitted_successor_and_acks"
                }
                ProofFault::Restart => {
                    "root_mapped_connected_l_successor_ack_revalidates_after_broker_restart"
                }
                ProofFault::SuccessorReceiptTamper => {
                    "root_mapped_connected_l_successor_changed_receipt_refuses_certificate"
                }
                _ => unreachable!(),
            }
        } else {
            match (normal, fault, real_bash, async_bash) {
                (true, ProofFault::None, true, true) => {
                    "root_mapped_connected_l_delivers_real_bash_async_to_recipient_and_acks"
                }
                (true, ProofFault::None, true, false) => {
                    "root_mapped_connected_l_runs_real_bash_sync_child_and_closes_owner"
                }
                (false, ProofFault::None, false, false) => {
                    "root_mapped_connected_l_reaches_ordinary_help_with_exact_one_use_custody"
                }
                (false, ProofFault::Diagnostics, false, false) => {
                    "root_mapped_connected_diagnostics_help_closes_without_effect"
                }
                (false, ProofFault::Restart, false, false) => {
                    "root_mapped_connected_help_revalidates_offline_close_after_restart"
                }
                (false, ProofFault::OfflineTamper, false, false) => {
                    "root_mapped_connected_help_refuses_tampered_no_effect_certificate"
                }
                (true, ProofFault::None, false, false) => {
                    "root_mapped_connected_l_runs_normal_model_and_closes_owner"
                }
                (true, ProofFault::MissingQ, false, false) => {
                    "root_mapped_connected_l_refuses_missing_q_after_certificate"
                }
                (true, ProofFault::ChangedQ, false, false) => {
                    "root_mapped_connected_l_refuses_changed_q_after_certificate"
                }
                (true, ProofFault::Restart, false, false) => {
                    "root_mapped_connected_l_revalidates_certificate_after_broker_restart"
                }
                _ => {
                    unreachable!()
                }
            }
        };
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env("AGE319_CONNECTED_CHILD_TEST", "1")
            .env("AGE319_CONNECTED_RUNNER_BIN", runner_bin)
            .envs(
                bash_bin
                    .as_ref()
                    .map(|path| ("AGE319_CONNECTED_BASH_BIN", path)),
            )
            .status()
            .unwrap();
        assert!(status.success(), "root-mapped connected fixture failed");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let data_dir = temp.path().join("data");
    fs::create_dir(&data_dir).unwrap();
    let config_home = temp.path().join("config-home");
    let effect = temp.path().join("provider-effect");
    let recipient = temp.path().join("recipient");
    if async_bash {
        fs::create_dir(&recipient).unwrap();
    }
    if normal {
        let config = config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(config.join("models")).unwrap();
        let provider = temp.path().join("provider.sh");
        fs::write(
            &provider,
            if async_bash {
                br#"#!/bin/sh
export OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1="$AGE319_BASH_CONTROL_SOCKET"
response=$("$AGE319_BASH_IMAGE" run --delivery async -- /bin/sh -c 'while [ ! -f "$AGE319_CHILD_RELEASE_FILE" ]; do sleep 0.02; done; printf "one\n" >> "$AGE319_EFFECT_FILE"; printf "async-child-out\n"; printf "async-child-err\n" >&2') || exit
printf '%s\n' "$response"
"#.as_slice()
            } else if real_bash && sync_children > 1 {
                format!(
                    "#!/bin/sh\nexport OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1=\"$AGE319_BASH_CONTROL_SOCKET\"\nfor _ in $(seq {sync_children}); do\n\"$AGE319_BASH_IMAGE\" run --delivery sync -- /bin/sh -c 'printf \"one\\n\" >> \"$AGE319_EFFECT_FILE\"; printf \"bash-child-out\\n\"; printf \"bash-child-err\\n\" >&2' || exit\nprintf '\\n'\ndone\n"
                )
                .into_bytes()
                .leak() as &[u8]
            } else if real_bash {
                b"#!/bin/sh\nexport OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1=\"$AGE319_BASH_CONTROL_SOCKET\"\n\"$AGE319_BASH_IMAGE\" run --delivery sync -- /bin/sh -c 'printf \"one\\n\" >> \"$AGE319_EFFECT_FILE\"; printf \"bash-child-out\\n\"; printf \"bash-child-err\\n\" >&2'\n".as_slice()
            } else {
                b"#!/bin/sh\nprintf 'one\\n' >> \"$AGE319_EFFECT_FILE\"\nprintf 'normal-provider-out:'\ncat\nprintf 'normal-provider-err\\n' >&2\n".as_slice()
            },
        )
        .unwrap();
        fs::set_permissions(&provider, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            config.join("providers.toml"),
            format!(
                "[local]\ncommand = {}\nquota_account_id = 'physical-local'\n",
                serde_json::to_string(provider.to_str().unwrap()).unwrap(),
            ),
        )
        .unwrap();
        fs::write(
            config.join("models/fixture-model.toml"),
            "[[providers]]\nname = 'local'\n",
        )
        .unwrap();
    }
    let installed = temp.path().join("installed");
    fs::create_dir(&installed).unwrap();
    let runner = installed.join("oulipoly-agent-runner");
    let broker_image = installed.join("oulipoly-kernel-broker");
    let launcher = installed.join("oulipoly-installed-launcher");
    let bash = installed.join("agent-bash");
    fs::copy(&runner_bin, &runner).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"), &broker_image).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_oulipoly-installed-launcher"), &launcher).unwrap();
    fs::copy(
        bash_bin.as_ref().map_or(broker_image.as_path(), Path::new),
        &bash,
    )
    .unwrap();
    if real_bash {
        assert_eq!(
            digest(&bash),
            digest(Path::new(bash_bin.as_deref().unwrap()))
        );
        assert_ne!(digest(&bash), digest(&broker_image));
        assert_ne!(digest(&bash), digest(&runner));
        assert_ne!(digest(&bash), digest(&launcher));
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
    assert_eq!(activation.bash.sha256, pair.bash_sha256.as_deref().unwrap());
    let socket = temp.path().join("control.sock");
    let broker_log = temp.path().join("broker.err");
    let mut broker = Command::new(&broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &broker_image)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &bash)
        .env("OULIPOLY_AGE319_PRIVATE_CONNECTED_CONTROL_V1", "1")
        .env_remove("OULIPOLY_DATA_DIR")
        .stderr(Stdio::from(File::create(&broker_log).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while observe_entry_gate_at(&socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
        assert!(
            broker.try_wait().unwrap().is_none(),
            "Broker exited: {}",
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        assert!(Instant::now() < deadline, "Broker did not activate");
        std::thread::sleep(Duration::from_millis(20));
    }
    let direct = |copied: bool| {
        let mut command = Command::new(&runner);
        command
            .arg("--help")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if copied {
            command.env("OULIPOLY_KERNEL_CONNECTED_CONTROL_FD_V1", "3");
        }
        assert!(
            !command.status().unwrap().success(),
            "direct Runner entered"
        );
    };
    direct(false);
    direct(true);
    let forged_socket = || {
        let (candidate, _peer) = UnixStream::pair().unwrap();
        let inherited = candidate.as_raw_fd();
        let mut command = Command::new(&runner);
        command
            .arg("--help")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_CONNECTED_CONTROL_FD_V1", "3")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(inherited, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        assert!(
            !command.status().unwrap().success(),
            "unrelated socket forged grant"
        );
    };
    forged_socket();
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 0);
    let pause = temp.path().join("connected-pause");
    fs::create_dir(&pause).unwrap();
    let certificate_pause = temp.path().join("certificate-pause");
    fs::create_dir(&certificate_pause).unwrap();
    let caller_out = temp.path().join("caller.out");
    let caller_err = temp.path().join("caller.err");
    let mut launch = Command::new(&launcher);
    if fault == ProofFault::Diagnostics {
        launch.args(["diagnostics", "--help"]);
    } else {
        launch.arg(if normal { "--model" } else { "--help" });
    }
    if normal {
        launch.args(["fixture-model", "hello fixture"]);
    }
    let mut launcher_child = launch
        .env_clear()
        .env("HOME", temp.path())
        .env("PATH", "/usr/bin:/bin")
        .env("OULIPOLY_AGE319_PRIVATE_DUPLICATE_L_V1", "1")
        .env("OULIPOLY_AGE319_PRIVATE_CONNECTED_STATUS_V1", "1")
        .env("AGE319_PRIVATE_CONNECTED_ORDINARY_CHILD_V1", "1")
        .envs(normal.then_some(("AGE319_PRIVATE_CONNECTED_NORMAL_MODEL_V1", "1")))
        .envs(normal.then_some(("OULIPOLY_CONFIG_HOME", &config_home)))
        .envs(normal.then_some(("OULIPOLY_DATA_DIR", &data_dir)))
        .envs(normal.then_some(("AGE319_EFFECT_FILE", &effect)))
        .envs(real_bash.then_some(("AGE319_BASH_IMAGE", &bash)))
        .envs(real_bash.then_some(("AGE319_BASH_CONTROL_SOCKET", &socket)))
        .envs(async_bash.then_some((
            "AGE319_CHILD_RELEASE_FILE",
            temp.path().join("child-release"),
        )))
        .envs(async_bash.then_some(("AGE319_CONNECTED_ASYNC_RECIPIENT_DIR_V1", &recipient)))
        .envs(successor_admission.then_some(("AGE319_CONNECTED_SUCCESSOR_ADMISSION_V1", "1")))
        .env("AGE319_PRIVATE_CONNECTED_J_REPLAY_V1", "1")
        .env("AGE319_CONNECTED_PAUSE_DIR", &pause)
        .envs(
            (!matches!(fault, ProofFault::None | ProofFault::Diagnostics))
                .then_some(("AGE319_CONNECTED_CERTIFICATE_PAUSE_DIR", &certificate_pause)),
        )
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1",
            &pair.generation,
        )
        .stdout(Stdio::from(File::create(&caller_out).unwrap()))
        .stderr(Stdio::from(File::create(&caller_err).unwrap()))
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pause.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "control Runner did not accept connected grant: caller={} broker={} launcher={:?}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default(),
            launcher_child.try_wait().unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        observe_entry_gate_at(&socket).unwrap(),
        EntryRoute::FreshOnlyOpen
    );
    assert!(
        request_at(&socket, Operation::ReserveV30Entry)
            .unwrap()
            .starts_with("error ")
    );
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 0);
    fs::write(pause.join("release"), b"release").unwrap();
    let launch_record = loop {
        let mut files = fs::read_dir(state.join("installed-launches")).unwrap();
        if let Some(file) = files.next() {
            let record: serde_json::Value =
                serde_json::from_slice(&fs::read(file.unwrap().path()).unwrap()).unwrap();
            break record;
        }
        assert!(Instant::now() < deadline, "L did not publish");
        std::thread::sleep(Duration::from_millis(20));
    };
    let root_id = launch_record["root_id"].as_str().unwrap().to_owned();
    assert_eq!(launch_record["pair_generation"], pair.generation);
    assert_eq!(launch_record["source_generation"], source.source_generation);
    let entry = state.join("entries").join(format!("{root_id}.json"));
    while !entry.exists() {
        assert!(
            Instant::now() < deadline,
            "connected E did not reserve exact ledger root"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let reserved: serde_json::Value = loop {
        if let Ok(record) = fs::read(&entry).and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(std::io::Error::other)
        }) {
            break record;
        }
        assert!(
            Instant::now() < deadline,
            "connected E entry remained incomplete"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(reserved["root_id"], root_id);
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    let deadline = Instant::now() + Duration::from_secs(if async_bash { 50 } else { 20 });
    let joined = loop {
        let Ok(record) = fs::read(&entry).and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(std::io::Error::other)
        }) else {
            assert!(
                Instant::now() < deadline,
                "connected J entry remained incomplete"
            );
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        if record["join_consumed"] == true && record["joined_child"].is_object() {
            break record;
        }
        assert!(
            Instant::now() < deadline,
            "connected P/G/A/J stalled: entry={record} caller={} broker={}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(joined["guardian"].is_object());
    assert_eq!(joined["prepared_guardian"], joined["guardian"]);
    assert!(joined["domain_id"].is_string());
    assert!(joined["supervisor_authority_id"].is_string());
    assert_ne!(joined["entry"], joined["guardian"]);
    assert_ne!(joined["joined_child"], joined["guardian"]);
    let handoff_path = state
        .join("released-handoffs")
        .join(format!("{root_id}.json"));
    let handoff: serde_json::Value = loop {
        if let Ok(bytes) = fs::read(&handoff_path)
            && let Ok(handoff) = serde_json::from_slice(&bytes)
        {
            break handoff;
        }
        assert!(
            Instant::now() < deadline,
            "connected release/U stalled: entry={} caller={} broker={}",
            fs::read_to_string(&entry).unwrap_or_default(),
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(handoff["old_release"]["prepared"]["root_id"], root_id);
    assert_eq!(handoff["old_release"]["prepared"]["entry"], joined["entry"]);
    assert_eq!(
        handoff["old_release"]["prepared"]["guardian"],
        joined["guardian"]
    );
    assert_eq!(
        handoff["old_release"]["prepared"]["joined_child"],
        joined["joined_child"]
    );
    assert_eq!(
        handoff["root_work_intent"]["kind"],
        if normal {
            "normal_cli"
        } else if fault == ProofFault::Diagnostics {
            "cli_diagnostics"
        } else {
            "cli_help"
        }
    );
    let d_key = handoff["d_key"].as_str().unwrap();
    let fresh_db = state.join("v30/state.db");
    loop {
        let present = rusqlite::Connection::open_with_flags(
            &fresh_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()
        .and_then(|db| {
            db.query_row(
                "SELECT count(*) FROM fresh_lane_session_admission WHERE request_id=?1",
                [d_key],
                |row| row.get::<_, i64>(0),
            )
            .ok()
        });
        if present == Some(1) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "connected D stalled: handoff={handoff} caller={} broker={}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    if async_bash {
        let normal_store = state.join("v30/normal-provider");
        let physical = loop {
            if let Ok(mut entries) = fs::read_dir(&normal_store)
                && let Some(Ok(entry)) = entries.next()
            {
                break entry.path();
            }
            assert!(
                Instant::now() < deadline,
                "normal physical grant absent: caller={} broker={}",
                fs::read_to_string(&caller_err).unwrap_or_default(),
                fs::read_to_string(&broker_log).unwrap_or_default(),
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        while !physical.join("provider-exit.json").exists() {
            assert!(
                Instant::now() < deadline,
                "normal provider did not exit before async child release: caller={} broker={}",
                fs::read_to_string(&caller_err).unwrap_or_default(),
                fs::read_to_string(&broker_log).unwrap_or_default(),
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !physical.join("q.json").exists(),
            "normal Q preceded live child"
        );
        fs::write(temp.path().join("child-release"), b"release").unwrap();
    }
    let exit_path = state.join("installed-control-exits").join(format!(
        "{}.json",
        launch_record["request_id"].as_str().unwrap()
    ));
    let control_exit: serde_json::Value = loop {
        if let Ok(bytes) = fs::read(&exit_path)
            && let Ok(exit) = serde_json::from_slice(&bytes)
        {
            break exit;
        }
        assert!(
            Instant::now() < deadline,
            "actual control wait was not retained: caller={} stdout={} broker={} bash_files={:?} normal_files={:?}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&caller_out).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default(),
            fs::read_dir(state.join("v30/fresh-provider"))
                .ok()
                .map(|dir| dir
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name())
                    .collect::<Vec<_>>()),
            fs::read_dir(state.join("v30/normal-provider"))
                .ok()
                .map(|dir| dir
                    .filter_map(Result::ok)
                    .map(|entry| {
                        let path = entry.path();
                        (
                            path.clone(),
                            fs::read_dir(&path).ok().map(|files| {
                                files
                                    .filter_map(Result::ok)
                                    .map(|file| {
                                        (
                                            file.file_name(),
                                            fs::read_to_string(file.path())
                                                .unwrap_or_default()
                                                .chars()
                                                .take(300)
                                                .collect::<String>(),
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                    })
                    .collect::<Vec<_>>()),
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(control_exit["request_id"], launch_record["request_id"]);
    assert_eq!(
        control_exit["pair_generation"],
        launch_record["pair_generation"]
    );
    assert_eq!(
        control_exit["source_generation"],
        launch_record["source_generation"]
    );
    assert_eq!(control_exit["root_id"], root_id);
    assert_eq!(control_exit["launcher"], launch_record["launcher"]);
    assert_eq!(control_exit["control"], reserved["entry"]);
    assert_eq!(control_exit["e_consumed"], true);
    if successor_admission {
        let offer: serde_json::Value = serde_json::from_slice(
            &fs::read(recipient.join("successor-offer.json")).unwrap_or_else(|error| {
                panic!(
                    "successor offer absent: {error}; caller={} broker={}",
                    fs::read_to_string(&caller_err).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default()
                )
            }),
        )
        .unwrap();
        let read: serde_json::Value =
            serde_json::from_slice(&fs::read(recipient.join("successor-admission.json")).unwrap())
                .unwrap();
        assert_eq!(read["offer"], offer);
        assert_ne!(offer["original_identity"], offer["successor_identity"]);
        assert_eq!(offer["root_id"], root_id);
        let state_db = rusqlite::Connection::open(&fresh_db).unwrap();
        let side_db =
            rusqlite::Connection::open(state.join("v30/sidecar/pid-identity.db")).unwrap();
        let original_attachment: String = state_db.query_row(
            "SELECT recipient_identity FROM fresh_lane_recipient_attachment WHERE session_id=?1",
            [offer["session_id"].as_str().unwrap()], |r| r.get(0)).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&original_attachment).unwrap(),
            offer["original_identity"]
        );
        let state_generation: String = state_db.query_row(
            "SELECT generation FROM fresh_lane_successor_admission WHERE session_id=?1 AND seq=?2",
            rusqlite::params![offer["session_id"].as_str().unwrap(),offer["seq"].as_i64().unwrap()],
            |r| r.get(0)).unwrap();
        let side_generation: String = side_db
            .query_row(
                "SELECT generation FROM fresh_successor_admission WHERE session_id=?1 AND seq=?2",
                rusqlite::params![
                    offer["session_id"].as_str().unwrap(),
                    offer["seq"].as_i64().unwrap()
                ],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state_generation, side_generation);
        assert_eq!(state_generation, offer["generation"]);
        assert_eq!(
            side_db
                .query_row("SELECT count(*) FROM fresh_recipient_grant", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            side_db
                .query_row(
                    "SELECT count(*) FROM fresh_recipient_ack_evidence",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        let fack: serde_json::Value = serde_json::from_slice(
            &fs::read(recipient.join("successor-fack.json")).unwrap_or_else(|error| {
                panic!(
                    "successor F/ACK absent: {error}; caller={} broker={}",
                    fs::read_to_string(&caller_err).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default()
                )
            }),
        )
        .unwrap();
        let delivery = &fack["delivery"];
        let grant = &delivery["grant"];
        assert_eq!(delivery["generation"], state_generation);
        assert_eq!(delivery["offer_request_id"], offer["offer_request_id"]);
        assert_eq!(grant["source_id"], offer["source_id"]);
        assert_eq!(grant["attempt_id"], offer["attempt_id"]);
        assert_eq!(grant["phase"], "unknown");
        assert_eq!(fack["ack"]["grant"]["phase"], "acked");
        assert_eq!(fack["ack"]["generation"], state_generation);
        let bytes = fs::read(recipient.join("successor-received.bin")).unwrap();
        assert_eq!(grant["payload_sha256"], digest_bytes(&bytes));
        assert_eq!(grant["payload_byte_len"], bytes.len());
        let retained_path: String = side_db
            .query_row(
                "SELECT payload_file_path FROM mailbox WHERE session_id=?1 AND seq=?2",
                rusqlite::params![
                    grant["session_id"].as_str().unwrap(),
                    grant["seq"].as_i64().unwrap()
                ],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            bytes,
            fs::read(retained_path).unwrap(),
            "successor received bytes differ from Broker-retained source"
        );
        let event: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(event["source"]["tree_drained"], true);
        assert_eq!(event["source"]["output_closed"], true);
        assert_eq!(event["source"]["wait_status"], 0);
        let child_grant = event["source"]["physical_grant_id"].as_str().unwrap();
        let physical = state.join("v30/fresh-provider");
        assert!(physical.join(format!("{child_grant}.drain.json")).exists());
        assert!(
            physical
                .join(format!("{child_grant}.consumed.json"))
                .exists()
        );
        assert_eq!(fs::read_to_string(&effect).unwrap(), "one\n");
        for (stream, expected) in [
            ("stdout", b"async-child-out\n".as_slice()),
            ("stderr", b"async-child-err\n".as_slice()),
        ] {
            assert_eq!(
                fs::read(physical.join(format!("{child_grant}.{stream}"))).unwrap(),
                expected
            );
            let raw: Vec<u8> =
                serde_json::from_value(event[format!("{stream}_bytes")].clone()).unwrap();
            assert_eq!(raw, expected);
        }
        let (phase, attempts, receipt_sha, ack_sha): (String, i64, String, String) = side_db
            .query_row(
                "SELECT g.phase,m.delivery_attempts,c.receipt_sha256,e.receipt_sha256
                 FROM fresh_successor_grant g
                 JOIN fresh_successor_receipt c ON c.grant_id=g.grant_id
                 JOIN fresh_successor_ack_evidence e ON e.grant_id=g.grant_id
                 JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
                 JOIN fresh_successor_admission a ON a.generation=g.generation
                 JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
                 WHERE g.grant_id=?1 AND g.delivery_request_id=?2
                   AND a.generation=?3 AND a.offer_request_id=g.offer_request_id
                   AND a.session_id=g.session_id AND a.seq=g.seq
                   AND a.source_id=g.source_id AND a.attempt_id=g.attempt_id
                   AND a.successor_identity=g.successor_identity
                   AND g.payload_sha256=a.payload_sha256
                   AND g.payload_byte_len=a.payload_byte_len
                   AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
                   AND r.payload_sha256=g.payload_sha256
                   AND r.payload_byte_len=g.payload_byte_len
                   AND m.delivered_by_invocation_uuid=g.grant_id
                   AND m.delivered_at=e.acknowledged_at
                   AND e.delivery_token_sha256=c.delivery_token_sha256",
                rusqlite::params![
                    grant["grant_id"].as_str().unwrap(),
                    fack["delivery_request_id"].as_str().unwrap(),
                    state_generation
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(phase, "acked");
        assert_eq!(attempts, 1);
        assert_eq!(receipt_sha, ack_sha);
        assert_eq!(receipt_sha, fack["receipt"]["receipt_sha256"]);
        let receipt_path = state
            .parent()
            .unwrap()
            .join("state-successor-receipts/0")
            .join(format!("{}.json", grant["grant_id"].as_str().unwrap()));
        let receipt_meta = fs::symlink_metadata(&receipt_path).unwrap();
        assert_eq!(receipt_meta.permissions().mode() & 0o777, 0o400);
        let receiver_receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(receiver_receipt["generation"], state_generation);
        assert_eq!(receiver_receipt["source_id"], offer["source_id"]);
        assert_eq!(receiver_receipt["attempt_id"], offer["attempt_id"]);
        assert_eq!(receiver_receipt["payload_sha256"], grant["payload_sha256"]);
        assert_eq!(
            receiver_receipt["successor_identity"],
            offer["successor_identity"]
        );
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(recipient.join("successor-terminal.json")).unwrap())
                .unwrap();
        let ack = &terminal["terminal"]["successor_ack"];
        assert_eq!(terminal["terminal"]["notification_state"], "acked");
        assert_eq!(
            terminal["terminal"]["notification_origin"],
            "admitted_successor"
        );
        assert_eq!(
            terminal["terminal"]["ack_basis"],
            "successor_receiver_receipt_ack"
        );
        assert_eq!(ack["generation"], state_generation);
        assert_eq!(ack["grant_id"], grant["grant_id"]);
        assert_eq!(ack["receipt_sha256"], receipt_sha);
        assert_eq!(ack["recipient_uid"], 0);
    }
    assert_eq!(
        control_exit["code"],
        0,
        "caller={} broker={} handoff={handoff} bash_files={:?}",
        fs::read_to_string(&caller_err).unwrap_or_default(),
        fs::read_to_string(&broker_log).unwrap_or_default(),
        fs::read_dir(state.join("v30/fresh-provider"))
            .ok()
            .map(|dir| dir
                .filter_map(Result::ok)
                .map(|entry| (
                    entry.file_name(),
                    fs::read_to_string(entry.path())
                        .unwrap_or_default()
                        .chars()
                        .take(300)
                        .collect::<String>()
                ))
                .collect::<Vec<_>>()),
    );
    assert!(control_exit["signal"].is_null());
    let fresh_sidecar = state.join("v30/sidecar/pid-identity.db");
    loop {
        let allocated = rusqlite::Connection::open_with_flags(
            &fresh_sidecar,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .ok()
        .and_then(|db| {
            db.query_row(
                "SELECT count(*) FROM fresh_lane_session WHERE request_id=?1",
                [d_key],
                |row| row.get::<_, i64>(0),
            )
            .ok()
        });
        if allocated == Some(1) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "connected D sidecar stalled: caller={} broker={}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = loop {
        let output = fs::read_to_string(&caller_out).unwrap_or_default();
        if output.contains(if async_bash {
            "\"dispatch_state\":\"broker-k-consumed\""
        } else if real_bash {
            "\"dispatch_state\":\"sync-child-result\""
        } else if normal {
            "normal-provider-out:"
        } else {
            "Usage:"
        }) {
            break output;
        }
        assert!(
            Instant::now() < deadline,
            "ordinary entry did not run after D: stdout={output} caller={} broker={}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(
        output.contains(if async_bash {
            "\"schema_version\":30"
        } else if real_bash {
            "\"schema_version\":31"
        } else if normal {
            "normal-provider-out:hello fixture"
        } else if fault == ProofFault::Diagnostics {
            "diagnostics"
        } else {
            "oulipoly-agent-runner"
        }),
        "unexpected ordinary output: {output}"
    );
    let mut late_set = None;
    if normal {
        let root: RootRecord =
            serde_json::from_slice(&fs::read(state.join(format!("{root_id}.json"))).unwrap())
                .unwrap();
        let drained = loop {
            let reply = protocol::root_drain_readback_at(&socket, &root, false).unwrap();
            let read: serde_json::Value =
                serde_json::from_str(reply.strip_prefix("root-drain-v1 ").unwrap()).unwrap();
            if read["owner_close_proof"]["root_id"] == root_id {
                break read;
            }
            assert!(
                Instant::now() < deadline,
                "normal owner close stalled: read={read} broker={}",
                fs::read_to_string(&broker_log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(drained["normal"]["physical"]["state"], "drained");
        assert_eq!(drained["normal"]["publication"]["state"], "settled");
        let broker_errors = fs::read_to_string(&broker_log).unwrap_or_default();
        assert!(
            !broker_errors.contains("normal owner close progression blocked"),
            "normal close warning: {broker_errors}"
        );
        assert_eq!(
            fs::read_to_string(&effect).unwrap(),
            "one\n".repeat(sync_children)
        );
        if async_bash && !successor_admission {
            assert_real_bash_async_child(
                &state, &fresh_db, &root_id, &handoff, &output, &recipient,
            );
        } else if real_bash && !async_bash {
            let output = if sync_children > 1 {
                fs::read_to_string(&caller_out).unwrap()
            } else {
                output.clone()
            };
            late_set = assert_real_bash_sync_children(
                &state,
                &fresh_db,
                &root_id,
                &handoff,
                &output,
                sync_children,
            );
        } else if !real_bash {
            assert!(
                fs::read_to_string(&caller_err)
                    .unwrap()
                    .contains("normal-provider-err")
            );
        }
        assert_eq!(drained["pid1_echild_receipt"], true);
        assert_eq!(drained["pid1_terminal_proof"], true);
        assert_eq!(drained["pid1_exact_live"], false);
        assert_eq!(drained["pid1_parent_wait_proof"], true);
        assert_eq!(drained["source_physical_retired"], 0);
        assert_eq!(drained["work_retired"], 0);
        assert_eq!(drained["entry_physical_settled"], true);
        assert_eq!(drained["entry_unsettled"], false);
        assert_eq!(drained["owner_close_intent"]["root"]["root_id"], root_id);
        assert_eq!(
            drained["owner_close_proof"]["owner_generation"],
            handoff["old_release"]["prepared"]["owner_generation"]
        );
        let db = rusqlite::Connection::open_with_flags(
            &fresh_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let k_count: i64 = db
            .query_row("SELECT count(*) FROM fresh_normal_provider_k", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(k_count, 1, "normal K was not exact-once");
        let admission_id: String = db
            .query_row(
                "SELECT json_extract(k_json, '$.admission_id') FROM fresh_normal_provider_k",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let physical = state.join("v30/normal-provider").join(&admission_id);
        let q: serde_json::Value =
            serde_json::from_slice(&fs::read(physical.join("q.json")).unwrap()).unwrap();
        let parent: serde_json::Value =
            serde_json::from_slice(&fs::read(physical.join("parent-wait.json")).unwrap()).unwrap();
        assert_eq!(q["admission_id"], admission_id);
        assert_eq!(q["tree_drained"], true);
        assert_eq!(q["provider_wait_status"], 0);
        assert_eq!(parent["admission_id"], admission_id);
        assert_eq!(
            fs::read(physical.join("stdout")).unwrap(),
            if real_bash {
                output.as_bytes()
            } else {
                b"normal-provider-out:hello fixture"
            },
        );
        assert_eq!(
            fs::read(physical.join("stderr")).unwrap(),
            if real_bash {
                b"".as_slice()
            } else {
                b"normal-provider-err\n"
            },
        );
        assert!(physical.join("caller-settled.json").exists());
        assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    } else {
        loop {
            let returned: String = rusqlite::Connection::open_with_flags(
                &fresh_db,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap()
            .query_row(
                "SELECT state FROM fresh_root_effect WHERE handoff_id=?1",
                [handoff["handoff_id"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
            if returned == "returned_success" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "ordinary --help did not return success: state={returned} caller={} broker={}",
                fs::read_to_string(&caller_err).unwrap_or_default(),
                fs::read_to_string(&broker_log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let root: RootRecord =
            serde_json::from_slice(&fs::read(state.join(format!("{root_id}.json"))).unwrap())
                .unwrap();
        let drained = loop {
            let reply = protocol::root_drain_readback_at(&socket, &root, false).unwrap();
            let read: serde_json::Value =
                serde_json::from_str(reply.strip_prefix("root-drain-v1 ").unwrap()).unwrap();
            if read["owner_close_proof"]["root_id"] == root_id {
                break read;
            }
            assert!(
                Instant::now() < deadline,
                "offline owner close stalled: read={read} broker={}",
                fs::read_to_string(&broker_log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(drained["offline"]["effect"]["state"], "returned_success");
        assert_eq!(drained["grant_records"], 0);
        assert_eq!(drained["native_records"], 0);
        assert_eq!(drained["work_records"], 0);
        assert_eq!(drained["source_physical_records"], 0);
        assert_eq!(drained["source_effect"]["accepted"], 0);
        assert_eq!(drained["pid1_echild_receipt"], true);
        assert_eq!(drained["pid1_parent_wait_proof"], true);
        assert_eq!(drained["entry_physical_settled"], true);
        assert_eq!(drained["owner_close_intent"]["root"]["root_id"], root_id);
    }
    // The normal close scanner may race D's multi-store publication. After
    // the root returns, the invocation/session/registration readback must
    // resolve to the exact released child rather than remain conflicted.
    let lane = FreshV30Lane::open_at(&state).unwrap();
    let (released, actor) = lane.released_handoff_for_root(&root_id).unwrap();
    let session = lane.read_session(&released.d_key).unwrap().unwrap();
    lane.require_released_invocation(&released, &actor, &session)
        .unwrap();
    if async_bash {
        let terminal = lane
            .read_private_root_terminal(&released, &actor, &session)
            .unwrap();
        assert_eq!(
            terminal.notification_origin,
            if successor_admission {
                "admitted_successor"
            } else {
                "original_c_notify"
            }
        );
        assert_eq!(terminal.notification_state, "acked");
        assert_eq!(
            terminal.ack_basis.as_deref(),
            Some(if successor_admission {
                "successor_receiver_receipt_ack"
            } else {
                "manual_ack"
            })
        );
    }
    let terminal_path = state.join("installed-normal-terminals").join(format!(
        "{}.json",
        launch_record["request_id"].as_str().unwrap()
    ));
    if !matches!(fault, ProofFault::None | ProofFault::Diagnostics) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !certificate_pause.join("ready").exists() {
            assert!(
                Instant::now() < deadline,
                "launcher did not observe certificate: broker={}",
                fs::read_to_string(&broker_log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(&terminal_path).unwrap()).unwrap();
        match fault {
            ProofFault::MissingQ => {
                let admission = terminal["physical"]["normal"]["physical"]["q"]["admission_id"]
                    .as_str()
                    .unwrap();
                fs::remove_file(
                    state
                        .join("v30/normal-provider")
                        .join(admission)
                        .join("q.json"),
                )
                .unwrap();
            }
            ProofFault::ChangedQ => {
                let admission = terminal["physical"]["normal"]["physical"]["q"]["admission_id"]
                    .as_str()
                    .unwrap();
                let q_path = state
                    .join("v30/normal-provider")
                    .join(admission)
                    .join("q.json");
                let mut q: serde_json::Value =
                    serde_json::from_slice(&fs::read(&q_path).unwrap()).unwrap();
                q["provider_wait_status"] = serde_json::json!(1);
                fs::write(q_path, serde_json::to_vec(&q).unwrap()).unwrap();
            }
            ProofFault::Restart => {
                broker.kill().unwrap();
                broker.wait().unwrap();
                broker = Command::new(&broker_image)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &broker_image)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &bash)
                    .env("OULIPOLY_AGE319_PRIVATE_CONNECTED_CONTROL_V1", "1")
                    .env_remove("OULIPOLY_DATA_DIR")
                    .stderr(Stdio::from(
                        File::options().append(true).open(&broker_log).unwrap(),
                    ))
                    .spawn()
                    .unwrap();
                let deadline = Instant::now() + Duration::from_secs(10);
                while observe_entry_gate_at(&socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
                    assert!(
                        broker.try_wait().unwrap().is_none(),
                        "restarted Broker exited: {}",
                        fs::read_to_string(&broker_log).unwrap_or_default()
                    );
                    assert!(
                        Instant::now() < deadline,
                        "restarted Broker did not activate"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            ProofFault::OfflineTamper => {
                let mut changed = terminal.clone();
                changed["physical"]["offline"]["effect"]["state"] = serde_json::json!("started");
                fs::write(&terminal_path, serde_json::to_vec(&changed).unwrap()).unwrap();
            }
            ProofFault::SuccessorReceiptTamper => {
                let grant_id = terminal["successor_ack"]["grant_id"].as_str().unwrap();
                let receipt_path = state
                    .parent()
                    .unwrap()
                    .join("state-successor-receipts/0")
                    .join(format!("{grant_id}.json"));
                fs::set_permissions(receipt_path, fs::Permissions::from_mode(0o600)).unwrap();
                let root: RootRecord = serde_json::from_slice(
                    &fs::read(state.join(format!("{root_id}.json"))).unwrap(),
                )
                .unwrap();
                assert!(
                    protocol::root_drain_readback_at(&socket, &root, false).is_err(),
                    "closed owner readback accepted changed receiver receipt"
                );
            }
            ProofFault::None | ProofFault::Diagnostics => unreachable!(),
        }
        fs::write(certificate_pause.join("release"), b"release").unwrap();
    }
    let caller_deadline = Instant::now() + Duration::from_secs(if async_bash { 50 } else { 10 });
    let launcher_status = loop {
        if let Some(status) = launcher_child.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < caller_deadline,
            "installed caller did not complete: caller={} broker={}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    if matches!(
        fault,
        ProofFault::None | ProofFault::Restart | ProofFault::Diagnostics
    ) {
        assert_eq!(
            launcher_status.code(),
            Some(0),
            "caller={} broker={}",
            fs::read_to_string(&caller_err).unwrap_or_default(),
            fs::read_to_string(&broker_log).unwrap_or_default()
        );
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(&terminal_path).unwrap()).unwrap();
        assert_eq!(terminal["request"], launch_record);
        assert_eq!(terminal["control_exit"], control_exit);
        assert_eq!(terminal["exit_code"], 0);
        assert_eq!(terminal["physical"]["root_id"], root_id);
        assert_eq!(terminal["physical"]["pid1_echild_receipt"], true);
        assert_eq!(terminal["physical"]["pid1_parent_wait_proof"], true);
        if normal {
            assert_eq!(
                terminal["physical"]["normal"]["physical"]["q"]["tree_drained"],
                true
            );
            assert_eq!(
                terminal["physical"]["normal"]["publication"]["state"],
                "settled"
            );
        } else {
            assert_eq!(terminal["protocol"], "installed-offline-terminal-v1");
            assert_eq!(
                terminal["physical"]["offline"]["effect"]["state"],
                "returned_success"
            );
            assert!(terminal["physical"]["normal"].is_null());
            assert_eq!(terminal["physical"]["work_records"], 0);
            assert_eq!(terminal["physical"]["source_physical_records"], 0);
        }
        assert_eq!(terminal["owner"]["root_id"], root_id);
        if successor_admission {
            let root_terminal: serde_json::Value = serde_json::from_slice(
                &fs::read(recipient.join("successor-terminal.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                terminal["successor_ack"],
                root_terminal["terminal"]["successor_ack"]
            );
            assert_eq!(terminal["successor_ack"]["root_id"], root_id);
            assert_eq!(
                terminal["owner"]["root_id"],
                terminal["successor_ack"]["root_id"]
            );
            let owner_physical: serde_json::Value =
                serde_json::from_str(terminal["owner"]["physical_proof_json"].as_str().unwrap())
                    .unwrap();
            assert_eq!(owner_physical["successor_ack"], terminal["successor_ack"]);
            assert_eq!(
                terminal["physical"]["successor_ack"],
                terminal["successor_ack"]
            );
        }
    } else {
        assert_eq!(launcher_status.code(), Some(70));
        let diagnostic = fs::read_to_string(&caller_err).unwrap();
        assert!(
            diagnostic.contains("Broker reported unknown terminal evidence"),
            "{diagnostic}"
        );
        if normal {
            assert_eq!(fs::read_to_string(&effect).unwrap(), "one\n");
        }
    }
    assert!(!temp.path().join("state.db").exists());
    assert!(!temp.path().join("pid-identity.db").exists());
    assert!(
        request_at(&socket, Operation::ReserveV30Entry)
            .unwrap()
            .starts_with("error ")
    );
    direct(false);
    direct(true);
    forged_socket();
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    assert_eq!(
        source.source_generation,
        EmptyV30BootstrapIdentity::readback_at(&state)
            .unwrap()
            .source_generation
    );
    if let Some((request_ids, execution)) = late_set {
        assert_child_set_readback_and_late_admission(&state, &root_id, &request_ids, &execution);
        // State's retained refusal is not the only reader: the Broker asked
        // again for the same close no longer certifies it.
        let root: RootRecord =
            serde_json::from_slice(&fs::read(state.join(format!("{root_id}.json"))).unwrap())
                .unwrap();
        if let Ok(reply) = protocol::root_drain_readback_at(&socket, &root, false) {
            let read: serde_json::Value =
                serde_json::from_str(reply.strip_prefix("root-drain-v1 ").unwrap()).unwrap();
            assert!(
                read["owner_close_proof"].is_null(),
                "Broker still certifies close after a late C: {read}"
            );
        }
    }
    broker.kill().unwrap();
    broker.wait().unwrap();
}

/// Every admitted sync child keeps its own C, W, response record, K, drain
/// and output. The committed root terminal accounts for exactly that set.
fn assert_real_bash_sync_children(
    state: &Path,
    fresh_db: &Path,
    root_id: &str,
    handoff: &serde_json::Value,
    output: &str,
    children: usize,
) -> Option<(std::collections::BTreeSet<String>, serde_json::Value)> {
    let reports: Vec<&str> = if children == 1 {
        vec![output]
    } else {
        output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect()
    };
    assert_eq!(
        reports.len(),
        children,
        "one sync report per child: {output}"
    );
    let mut request_ids = std::collections::BTreeSet::new();
    let mut grant_ids = std::collections::BTreeSet::new();
    for report in reports {
        let (request_id, grant_id) =
            assert_real_bash_sync_child(state, fresh_db, root_id, handoff, report, children);
        assert!(request_ids.insert(request_id), "duplicate child request");
        assert!(grant_ids.insert(grant_id), "duplicate child K");
    }
    let db =
        rusqlite::Connection::open_with_flags(fresh_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let execution: String = db
        .query_row(
            "SELECT execution_json FROM fresh_root_terminal WHERE handoff_id=?1",
            [handoff["handoff_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let execution: serde_json::Value = serde_json::from_str(&execution).unwrap();
    assert_eq!(execution["outcome"], "success");
    let accounted: std::collections::BTreeSet<String> = if children == 1 {
        assert!(
            execution.get("children").is_none(),
            "one-child record shape changed"
        );
        [execution["child_request_id"].as_str().unwrap().to_owned()].into()
    } else {
        assert!(execution["child_request_id"].is_null());
        assert!(execution["child_event"].is_null());
        let members = execution["children"].as_array().unwrap();
        assert_eq!(members.len(), children);
        members
            .iter()
            .map(|member| {
                assert_eq!(member["event"]["request_id"], member["request_id"]);
                assert_eq!(member["event"]["root_id"], root_id);
                member["request_id"].as_str().unwrap().to_owned()
            })
            .collect()
    };
    assert_eq!(accounted, request_ids, "root terminal child set differs");
    // The late admission is made only after the caller has read its own
    // certified result; every closure reader revokes acceptance after it.
    (children > 1).then_some((request_ids, execution))
}

/// Reads the closed root through the actual State API, then admits one more
/// child after the set froze. The late member is retained as unresolved
/// custody and never discharged; the committed set does not change.
fn assert_child_set_readback_and_late_admission(
    state: &Path,
    root_id: &str,
    request_ids: &std::collections::BTreeSet<String>,
    execution: &serde_json::Value,
) {
    let mut lane = FreshV30Lane::open_at(state).unwrap();
    let (released, actor) = lane.released_handoff_for_root(root_id).unwrap();
    let session = lane.read_session(&released.d_key).unwrap().unwrap();
    let read = lane
        .read_private_root_terminal(&released, &actor, &session)
        .unwrap();
    assert_eq!(read.terminal_state, "execution_completed");
    assert_eq!(read.notification_state, "response_only");
    assert_eq!(read.notification_origin, "child_set");
    assert!(read.child_request_id.is_none());
    assert!(read.unresolved_child_request_ids.is_empty());
    assert!(read.late_child_request_ids.is_empty());
    assert!(read.unknown_stages.is_empty(), "{:?}", read.unknown_stages);
    assert_eq!(read.refusal, None);
    let members: std::collections::BTreeSet<String> = read
        .children
        .iter()
        .map(|child| {
            assert_eq!(child.notification_state, "response_only");
            assert_eq!(child.listener_policy.as_deref(), Some("response_only"));
            assert_eq!(
                child.selected_event.as_ref().unwrap().request_id,
                child.request_id
            );
            child.request_id.clone()
        })
        .collect();
    assert_eq!(&members, request_ids);
    assert!(
        !lane
            .no_root_child_requests(&released, &actor, &session)
            .unwrap()
    );

    let first = read.children[0].selected_event.as_ref().unwrap();
    let mut late_actor = actor.clone();
    late_actor.starttime_ticks += 1_000_000;
    let late_id = uuid::Uuid::new_v4().to_string();
    lane.admit_bash_child(
        &late_id,
        &released,
        &actor,
        &late_actor,
        &first.parent_work_grant_id,
        &first.parent_work_id,
        oulipoly_state::mailbox::FreshBashListenerPolicy::ResponseOnly,
    )
    .unwrap();
    let settled = lane
        .settle_private_root_terminal(&released, &actor, &session)
        .unwrap();
    for late in [
        settled,
        lane.read_private_root_terminal(&released, &actor, &session)
            .unwrap(),
    ] {
        assert_eq!(late.late_child_request_ids, vec![late_id.clone()]);
        assert_eq!(late.unresolved_child_request_ids, vec![late_id.clone()]);
        assert_eq!(
            late.refusal.as_deref(),
            Some("child_admission_after_freeze")
        );
        assert_eq!(
            late.terminal_state,
            "execution_completed_child_admission_pending"
        );
        assert!(
            late.unknown_stages
                .contains(&format!("child_c_after_freeze:{late_id}"))
        );
        assert_eq!(late.children.len(), request_ids.len());
        assert_eq!(
            serde_json::to_value(late.execution.as_ref().unwrap()).unwrap(),
            *execution,
            "frozen set changed after a late admission"
        );
    }
}

fn assert_real_bash_sync_child(
    state: &Path,
    fresh_db: &Path,
    root_id: &str,
    handoff: &serde_json::Value,
    output: &str,
    children: usize,
) -> (String, String) {
    let report: serde_json::Value = serde_json::from_str(output).unwrap();
    assert_eq!(report["schema_version"], 31);
    assert_eq!(report["dispatch_state"], "sync-child-result");
    assert_eq!(report["publication"]["phase"], "unknown");
    assert_eq!(report["publication"]["outcome"], "exited");
    assert_eq!(report["publication"]["exit_code"], 0);
    let child = &report["publication"]["child"];
    let event = &report["publication"]["event"];
    let request_id = child["request_id"].as_str().unwrap();
    let grant_id = event["physical_grant_id"].as_str().unwrap();
    assert_eq!(child["root_id"], root_id);
    assert_eq!(child["root_handoff_id"], handoff["handoff_id"]);
    assert_eq!(child["parent_invocation_uuid"], handoff["invocation_uuid"]);
    assert_eq!(
        child["listener_policy"].as_str().unwrap_or("response_only"),
        "response_only"
    );
    assert_eq!(event["request_id"], request_id);
    assert_eq!(event["source_id"], child["handle"]);
    assert_eq!(event["attempt_id"], child["invocation_uuid"]);
    assert_eq!(event["parent_work_grant_id"], child["parent_work_grant_id"]);
    assert_eq!(event["parent_work_id"], child["parent_work_id"]);
    assert_eq!(event["tree_drained"], true);
    assert_eq!(event["output_closed"], true);
    assert_eq!(event["wait_status"], 0);
    let db =
        rusqlite::Connection::open_with_flags(fresh_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    for (table, column, expected) in [
        ("fresh_bash_child", "receipt_json", child),
        ("fresh_bash_selected_event", "receipt_json", event),
        (
            "fresh_bash_sync_publication",
            "receipt_json",
            &report["publication"],
        ),
    ] {
        let rows: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            rows, children as i64,
            "{table} must hold exactly one C/W/response record per child"
        );
        let stored: String = db
            .query_row(
                &format!("SELECT {column} FROM {table} WHERE request_id=?1"),
                [request_id],
                |row| row.get(0),
            )
            .unwrap();
        let mut stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
        if table == "fresh_bash_child" {
            assert_eq!(stored["session"]["session_id"], "");
            stored["session"] = child["session"].clone();
        }
        assert_eq!(stored, *expected);
    }
    let physical = state.join("v30/fresh-provider");
    let child_consumptions = fs::read_dir(&physical)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .ends_with(".consumed.json")
        })
        .count();
    assert_eq!(
        child_consumptions, children,
        "exactly one Bash child K per child"
    );
    let consumed: serde_json::Value = serde_json::from_slice(
        &fs::read(physical.join(format!("{grant_id}.consumed.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(consumed["id"], grant_id);
    assert_eq!(consumed["binding"]["grant_key"], request_id);
    let parent_grant = child["parent_work_grant_id"].as_str().unwrap();
    let parent_k: String = db
        .query_row(
            "SELECT k_json FROM fresh_normal_provider_k WHERE handoff_id=?1",
            [handoff["handoff_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let parent_k: serde_json::Value = serde_json::from_str(&parent_k).unwrap();
    assert_eq!(parent_k["admission_id"], parent_grant);
    assert_eq!(child["parent_work_id"], parent_grant);
    let parent: serde_json::Value = serde_json::from_slice(
        &fs::read(
            state
                .join("v30/normal-provider")
                .join(parent_grant)
                .join("bash-parent.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(parent["admission_id"], parent_k["admission_id"]);
    assert_eq!(parent["plan_sha256"], parent_k["plan_sha256"]);
    assert_eq!(parent["handoff_id"], handoff["handoff_id"]);
    assert_eq!(parent["root_id"], root_id);
    assert!(parent["pid1_host_pid"].as_i64().unwrap() > 0);
    assert!(parent["pid1_starttime_ticks"].as_u64().unwrap() > 0);
    let selection: serde_json::Value = serde_json::from_slice(
        &fs::read(physical.join(format!("{request_id}.child-work-selection.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(selection["role"], "bash-child-ordinary-tree-v1");
    assert_eq!(selection["child_request_id"], request_id);
    assert_eq!(selection["child_d_key"], child["d_key"]);
    assert_eq!(
        selection["binding"]["causal_parent"]["grant_id"],
        parent_grant
    );
    assert_eq!(
        selection["binding"]["causal_parent"]["work_id"],
        child["parent_work_id"]
    );
    let source_event: serde_json::Value = serde_json::from_slice(
        &fs::read(physical.join(format!("{grant_id}.source-event.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(source_event, *event);
    let drain: serde_json::Value =
        serde_json::from_slice(&fs::read(physical.join(format!("{grant_id}.drain.json"))).unwrap())
            .unwrap();
    assert_eq!(drain["grant_id"], grant_id);
    assert_eq!(drain["zero_remaining"], true);
    assert_eq!(drain["cancelled"], false);
    for (stream, expected) in [
        ("stdout", b"bash-child-out\n".as_slice()),
        ("stderr", b"bash-child-err\n".as_slice()),
    ] {
        let bytes = fs::read(physical.join(format!("{grant_id}.{stream}"))).unwrap();
        assert_eq!(bytes, expected);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(report[format!("{stream}_base64")].as_str().unwrap())
                .unwrap(),
            expected
        );
        assert_eq!(event[format!("{stream}_len")], expected.len());
        assert_eq!(
            event[format!("{stream}_sha256")],
            format!("{:x}", Sha256::digest(expected))
        );
    }
    (request_id.to_owned(), grant_id.to_owned())
}

fn assert_real_bash_async_child(
    state: &Path,
    fresh_db: &Path,
    root_id: &str,
    handoff: &serde_json::Value,
    output: &str,
    recipient: &Path,
) {
    let dispatch: serde_json::Value = serde_json::from_str(output).unwrap();
    assert_eq!(dispatch["schema_version"], 30);
    assert_eq!(dispatch["dispatch_state"], "broker-k-consumed");
    assert_eq!(dispatch["delivery_mode"], "async");
    assert_eq!(dispatch["effects_possible"], true);
    let request_id = dispatch["request_id"].as_str().unwrap();
    let grant_id = dispatch["physical_grant_id"].as_str().unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(recipient.join("receipt.json")).unwrap()).unwrap();
    let ack: serde_json::Value =
        serde_json::from_slice(&fs::read(recipient.join("ack.json")).unwrap()).unwrap();
    let delivery = &receipt["grant"];
    assert_eq!(delivery["phase"], "unknown");
    assert_eq!(ack["kind"], "readback");
    assert_eq!(ack["grant"]["phase"], "acked");
    for key in [
        "grant_id",
        "session_id",
        "seq",
        "source_id",
        "attempt_id",
        "lane_id",
        "source_generation",
        "root_id",
        "owner_generation",
        "payload_sha256",
        "payload_byte_len",
    ] {
        assert_eq!(ack["grant"][key], delivery[key], "ACK changed {key}");
    }
    assert_eq!(delivery["root_id"], root_id);
    let payload = base64::engine::general_purpose::STANDARD
        .decode(receipt["payload_base64"].as_str().unwrap())
        .unwrap();
    let payload_value: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!(receipt["observed_sha256"], digest_bytes(&payload));
    assert_eq!(delivery["payload_sha256"], digest_bytes(&payload));
    assert_eq!(delivery["payload_byte_len"], payload.len());
    assert_eq!(payload_value["protocol"], "fresh-bash-complete-v30");
    let event = &payload_value["source"];
    assert_eq!(event["request_id"], request_id);
    assert_eq!(event["source_id"], delivery["source_id"]);
    assert_eq!(event["attempt_id"], delivery["attempt_id"]);
    assert_eq!(event["physical_grant_id"], grant_id);
    assert_eq!(event["root_id"], root_id);
    assert_ne!(event["session_id"], delivery["session_id"]);
    assert_eq!(event["tree_drained"], true);
    assert_eq!(event["output_closed"], true);
    assert_eq!(event["wait_status"], 0);
    for (key, expected) in [
        ("stdout", b"async-child-out\n".as_slice()),
        ("stderr", b"async-child-err\n".as_slice()),
    ] {
        let raw: Vec<u8> =
            serde_json::from_value(payload_value[format!("{key}_bytes")].clone()).unwrap();
        assert_eq!(raw, expected);
        assert_eq!(event[format!("{key}_len")], expected.len());
        assert_eq!(event[format!("{key}_sha256")], digest_bytes(expected));
        assert_eq!(
            fs::read(
                state
                    .join("v30/fresh-provider")
                    .join(format!("{grant_id}.{key}"))
            )
            .unwrap(),
            expected
        );
    }
    let db =
        rusqlite::Connection::open_with_flags(fresh_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let (child_json, policy): (String, String) = db
        .query_row(
            "SELECT c.receipt_json,l.listener_policy FROM fresh_bash_child c
             JOIN fresh_bash_listener_registration l ON l.request_id=c.request_id
             WHERE c.request_id=?1",
            [request_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let child: serde_json::Value = serde_json::from_str(&child_json).unwrap();
    assert_eq!(policy, "notify");
    assert_eq!(child["listener_policy"], "notify");
    assert_eq!(child["handle"], delivery["source_id"]);
    assert_eq!(child["invocation_uuid"], delivery["attempt_id"]);
    assert_eq!(child["root_id"], root_id);
    assert_eq!(child["root_handoff_id"], handoff["handoff_id"]);
    assert_eq!(child["parent_invocation_uuid"], handoff["invocation_uuid"]);
    assert_eq!(child["parent_work_grant_id"], event["parent_work_grant_id"]);
    assert_eq!(child["parent_work_id"], event["parent_work_id"]);
    let child_session: String = db
        .query_row(
            "SELECT session_id FROM fresh_lane_session_admission WHERE request_id=?1",
            [child["d_key"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(event["session_id"], child_session);
    let selected: String = db
        .query_row(
            "SELECT receipt_json FROM fresh_bash_selected_event WHERE request_id=?1",
            [request_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&selected).unwrap(),
        *event
    );
    let (notify_source, notify_session): (String, String) = db
        .query_row(
            "SELECT source_id,listener_session_id FROM fresh_bash_notify_request WHERE request_id=?1",
            [request_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(notify_source, delivery["source_id"]);
    assert_eq!(notify_session, delivery["session_id"]);
    let physical = state.join("v30/fresh-provider");
    let consumed: serde_json::Value = serde_json::from_slice(
        &fs::read(physical.join(format!("{grant_id}.consumed.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(consumed["id"], grant_id);
    assert_eq!(consumed["binding"]["grant_key"], request_id);
    let source_event: serde_json::Value = serde_json::from_slice(
        &fs::read(physical.join(format!("{grant_id}.source-event.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(source_event, *event);
    assert!(physical.join(format!("{grant_id}.drain.json")).exists());
    assert_eq!(
        fs::read_dir(&physical)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .ends_with(".consumed.json"))
            .count(),
        1,
        "async Bash child K repeated"
    );
    let parent_grant = child["parent_work_grant_id"].as_str().unwrap();
    let parent_k: String = db
        .query_row(
            "SELECT k_json FROM fresh_normal_provider_k WHERE handoff_id=?1",
            [handoff["handoff_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    let parent_k: serde_json::Value = serde_json::from_str(&parent_k).unwrap();
    assert_eq!(parent_k["admission_id"], parent_grant);
    let sidecar = rusqlite::Connection::open_with_flags(
        state.join("v30/sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (phase, delivered_at, attempts, ack_basis, token_sha, recipient_identity): (
        String,
        String,
        i64,
        String,
        String,
        String,
    ) = sidecar
        .query_row(
            "SELECT g.phase,m.delivered_at,m.delivery_attempts,e.basis,e.delivery_token_sha256,
                    g.recipient_identity
             FROM fresh_recipient_grant g
             JOIN mailbox m ON m.session_id=g.session_id AND m.seq=g.seq
             JOIN fresh_recipient_ack_evidence e ON e.grant_id=g.grant_id
             JOIN fresh_recipient_row_source r ON r.session_id=g.session_id AND r.seq=g.seq
             WHERE g.grant_id=?1 AND g.delivery_request_id=?2
               AND g.source_id=?3 AND g.attempt_id=?4
               AND g.payload_sha256=?5 AND g.payload_byte_len=?6
               AND r.source_id=g.source_id AND r.attempt_id=g.attempt_id
               AND r.payload_sha256=g.payload_sha256 AND r.payload_byte_len=g.payload_byte_len
               AND e.acknowledged_at=m.delivered_at AND g.acknowledged_at=m.delivered_at",
            rusqlite::params![
                delivery["grant_id"].as_str().unwrap(),
                receipt["delivery_request_id"].as_str().unwrap(),
                delivery["source_id"].as_str().unwrap(),
                delivery["attempt_id"].as_str().unwrap(),
                delivery["payload_sha256"].as_str().unwrap(),
                delivery["payload_byte_len"].as_i64().unwrap(),
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(phase, "acked");
    assert!(!delivered_at.is_empty());
    assert_eq!(attempts, 1);
    assert_eq!(ack_basis, "manual_ack");
    assert_eq!(
        token_sha,
        digest_bytes(delivery["delivery_token"].as_str().unwrap().as_bytes())
    );
    let lane = FreshV30Lane::open_at(state).unwrap();
    let (_, root_actor) = lane.released_handoff_for_root(root_id).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&recipient_identity).unwrap(),
        serde_json::to_value(root_actor).unwrap()
    );
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
