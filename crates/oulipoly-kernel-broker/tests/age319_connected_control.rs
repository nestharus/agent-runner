#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]

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
    run_connected_case(false);
}

#[test]
fn root_mapped_connected_l_runs_normal_model_and_closes_owner() {
    run_connected_case(true);
}

fn run_connected_case(normal: bool) {
    let Ok(runner_bin) = std::env::var("AGE319_CONNECTED_RUNNER_BIN") else {
        return;
    };
    if std::env::var_os("AGE319_CONNECTED_CHILD_TEST").is_none() {
        let test_name = if normal {
            "root_mapped_connected_l_runs_normal_model_and_closes_owner"
        } else {
            "root_mapped_connected_l_reaches_ordinary_help_with_exact_one_use_custody"
        };
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", test_name, "--nocapture"])
            .env("AGE319_CONNECTED_CHILD_TEST", "1")
            .env("AGE319_CONNECTED_RUNNER_BIN", runner_bin)
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
    if normal {
        let config = config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(config.join("models")).unwrap();
        let provider = temp.path().join("provider.sh");
        fs::write(
            &provider,
            b"#!/bin/sh\nprintf 'one\\n' >> \"$AGE319_EFFECT_FILE\"\nprintf 'normal-provider-out:'\ncat\nprintf 'normal-provider-err\\n' >&2\n",
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
    fs::copy(&broker_image, &bash).unwrap();
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
    FirstInstallActivation::activate_at(
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
    let caller_out = temp.path().join("caller.out");
    let caller_err = temp.path().join("caller.err");
    let mut launch = Command::new(&launcher);
    launch.arg(if normal { "--model" } else { "--help" });
    if normal {
        launch.args(["fixture-model", "hello fixture"]);
    }
    let launcher_status = launch
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
        .env("AGE319_PRIVATE_CONNECTED_J_REPLAY_V1", "1")
        .env("AGE319_CONNECTED_PAUSE_DIR", &pause)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1",
            &pair.generation,
        )
        .stdout(Stdio::from(File::create(&caller_out).unwrap()))
        .stderr(Stdio::from(File::create(&caller_err).unwrap()))
        .status()
        .unwrap();
    assert_eq!(launcher_status.code(), Some(70)); // L remains pending/unknown.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pause.join("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "control Runner did not accept connected grant"
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
    let reserved: serde_json::Value = serde_json::from_slice(&fs::read(&entry).unwrap()).unwrap();
    assert_eq!(reserved["root_id"], root_id);
    assert_eq!(fs::read_dir(state.join("entries")).unwrap().count(), 1);
    let deadline = Instant::now() + Duration::from_secs(20);
    let joined = loop {
        let record: serde_json::Value = serde_json::from_slice(&fs::read(&entry).unwrap()).unwrap();
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
        if normal { "normal_cli" } else { "cli_help" }
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
            "actual control wait was not retained: broker={}",
            fs::read_to_string(&broker_log).unwrap_or_default()
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
    assert_eq!(
        control_exit["code"],
        0,
        "caller={} broker={} handoff={handoff}",
        fs::read_to_string(&caller_err).unwrap_or_default(),
        fs::read_to_string(&broker_log).unwrap_or_default()
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
        if output.contains(if normal {
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
    assert!(output.contains(if normal {
        "normal-provider-out:hello fixture"
    } else {
        "oulipoly-agent-runner"
    }));
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
        assert_eq!(fs::read_to_string(&effect).unwrap(), "one\n");
        assert!(
            fs::read_to_string(&caller_err)
                .unwrap()
                .contains("normal-provider-err")
        );
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
            b"normal-provider-out:hello fixture"
        );
        assert_eq!(
            fs::read(physical.join("stderr")).unwrap(),
            b"normal-provider-err\n"
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
    }
    // The normal close scanner may race D's multi-store publication. After
    // the root returns, the invocation/session/registration readback must
    // resolve to the exact released child rather than remain conflicted.
    let lane = FreshV30Lane::open_at(&state).unwrap();
    let (released, actor) = lane.released_handoff_for_root(&root_id).unwrap();
    let session = lane.read_session(&released.d_key).unwrap().unwrap();
    lane.require_released_invocation(&released, &actor, &session)
        .unwrap();
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
    broker.kill().unwrap();
    broker.wait().unwrap();
}
