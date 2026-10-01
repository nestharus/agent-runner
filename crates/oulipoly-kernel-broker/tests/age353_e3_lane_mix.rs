#![cfg(all(target_os = "linux", not(feature = "age319-private-broker-fixture")))]
//! AGE-353 E3 construction witness: concurrent Bash call paths beside
//! looping v30 clients and Main control traffic on one featureless Broker.
//!
//! The Broker, Runner, installed launcher and Bash images are the real
//! featureless builds; State, roots, providers and lane/Main synchronization
//! are real and owned by this disposable user namespace. Each normal root's
//! original Runner polls its provider Q every 50 ms (a Runner-image looping
//! client) while its provider makes one async Bash call (four serial v30
//! connections). Test threads add control-socket traffic and non-Runner
//! v30 pollers. The run's phase records are summarized for the caller.

use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::phase_record;
use oulipoly_kernel_broker::protocol::{self, EntryRoute, Operation};
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const ROOTS: usize = 4;
const BASH_CONNECTIONS: [i64; 4] = [0x90, b'X' as i64, b'c' as i64, b'^' as i64];

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn wait_for(
    deadline: Instant,
    what: &str,
    mut done: impl FnMut() -> bool,
    detail: impl Fn() -> String,
) {
    while !done() {
        assert!(Instant::now() < deadline, "{what}: {}", detail());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn age353_e3_concurrent_bash_calls_beside_looping_clients() {
    let Ok(runner_image) = std::env::var("AGE353_E3_RUNNER_BIN") else {
        return;
    };
    let Ok(bash_image) = std::env::var("AGE353_E3_BASH_BIN") else {
        return;
    };
    let output = PathBuf::from(
        std::env::var("AGE353_E3_OUTPUT_DIR")
            .expect("AGE353_E3_OUTPUT_DIR names the summary destination"),
    );
    if std::env::var_os("AGE353_E3_CHILD").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "age353_e3_concurrent_bash_calls_beside_looping_clients",
                "--nocapture",
            ])
            .env("AGE353_E3_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success(), "E3 lane mix fixture failed");
        return;
    }
    let pollers: usize = std::env::var("AGE353_E3_V30_POLLERS")
        .map(|value| value.parse().unwrap())
        .unwrap_or(2);
    let control_clients: usize = std::env::var("AGE353_E3_CONTROL_CLIENTS")
        .map(|value| value.parse().unwrap())
        .unwrap_or(2);
    let roots: usize = std::env::var("AGE353_E3_ROOTS")
        .map(|value| value.parse().unwrap())
        .unwrap_or(ROOTS);
    // Seconds after the common start at which each root's call begins. The
    // default is P3's observed spacing (s2, b2, b1, s1); a burst is heavier.
    let staggers: Vec<f64> = std::env::var("AGE353_E3_STAGGERS")
        .unwrap_or_else(|_| "0,1.07,1.79,3.41".into())
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect();

    // State refuses storage below a group/world-writable ancestor. An owned
    // directory may be bound over /mnt in this private mount namespace so
    // every artifact stays in that directory without changing host paths.
    let parent = match std::env::var_os("AGE353_E3_TMP_BIND") {
        Some(source) => {
            let status = Command::new("mount")
                .args(["--bind"])
                .arg(&source)
                .arg("/mnt")
                .status()
                .unwrap();
            assert!(status.success(), "private bind of {source:?} failed");
            PathBuf::from("/mnt")
        }
        None => std::env::temp_dir(),
    };
    let temp = tempfile::Builder::new()
        .prefix("age353-e3-")
        .tempdir_in(parent)
        .unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let installed = temp.path().join("installed");
    fs::create_dir(&installed).unwrap();
    let runner = installed.join("oulipoly-agent-runner");
    let broker_image = installed.join("oulipoly-kernel-broker");
    let launcher = installed.join("oulipoly-installed-launcher");
    let bash = installed.join("agent-bash");
    // A baseline/corrected comparison runs this same fixture against
    // retained images; by default it uses this build's own.
    fs::copy(&runner_image, &runner).unwrap();
    fs::copy(
        std::env::var("AGE353_E3_BROKER_BIN")
            .unwrap_or_else(|_| env!("CARGO_BIN_EXE_oulipoly-kernel-broker").into()),
        &broker_image,
    )
    .unwrap();
    fs::copy(
        std::env::var("AGE353_E3_LAUNCHER_BIN")
            .unwrap_or_else(|_| env!("CARGO_BIN_EXE_oulipoly-installed-launcher").into()),
        &launcher,
    )
    .unwrap();
    fs::copy(&bash_image, &bash).unwrap();
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
    EmptyV30BootstrapIdentity::bootstrap_at(&state).unwrap();
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
    let socket_directory = File::open(temp.path()).unwrap();
    let socket = PathBuf::from(format!(
        "/proc/{}/fd/{}/control.sock",
        std::process::id(),
        socket_directory.as_raw_fd(),
    ));
    let v30 = socket.with_file_name("v30.sock");
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
    let log = || fs::read_to_string(&broker_log).unwrap_or_default();
    wait_for(
        Instant::now() + Duration::from_secs(60),
        "Broker did not open fresh route",
        || protocol::observe_entry_gate_at(&socket).ok() == Some(EntryRoute::FreshOnlyOpen),
        &log,
    );

    let config_home = temp.path().join("config-home");
    let config = config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(config.join("models")).unwrap();
    let provider = temp.path().join("provider.sh");
    // The provider waits for a common start, makes one timed async Bash
    // call, then holds so its root keeps polling Q until released.
    fs::write(
        &provider,
        br#"#!/bin/sh
export OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1="$AGE319_BASH_CONTROL_SOCKET"
printf ready > "$E3_READY_FILE"
while [ ! -f "$E3_GO_FILE" ]; do sleep 0.02; done
sleep "$E3_STAGGER"
start=$(date +%s%N)
response=$("$AGE319_BASH_IMAGE" run --delivery async -- /bin/sh -c 'while [ ! -f "$E3_CHILD_RELEASE_FILE" ]; do sleep 0.02; done; printf "async\n" >> "$E3_EFFECT_FILE"' 2> "$E3_BASH_ERR_FILE")
status=$?
end=$(date +%s%N)
printf '%s %s %s\n' "$start" "$end" "$status" > "$E3_CALL_FILE"
[ "$status" -eq 0 ] || exit "$status"
printf '%s\n' "$response"
while [ ! -f "$E3_PROVIDER_RELEASE_FILE" ]; do sleep 0.02; done
"#,
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

    let go = temp.path().join("go");
    let child_release = temp.path().join("child-release");
    let provider_release = temp.path().join("provider-release");
    let effect = temp.path().join("effect");
    let mut launches: Vec<(Child, PathBuf, PathBuf, PathBuf, PathBuf)> = Vec::new();
    for index in 0..roots {
        let out = temp.path().join(format!("launch-{index}.out"));
        let err = temp.path().join(format!("launch-{index}.err"));
        let call = temp.path().join(format!("call-{index}"));
        let ready = temp.path().join(format!("ready-{index}"));
        let child = Command::new(&launcher)
            .args(["--model", "fixture-model", "hello e3"])
            .current_dir(temp.path())
            .env_clear()
            .env("HOME", temp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("OULIPOLY_CONFIG_HOME", &config_home)
            .env(
                "OULIPOLY_DATA_DIR",
                temp.path().join(format!("data-{index}")),
            )
            .env("AGE319_BASH_IMAGE", &bash)
            .env("AGE319_BASH_CONTROL_SOCKET", &socket)
            .env("E3_READY_FILE", &ready)
            .env("E3_GO_FILE", &go)
            .env(
                "E3_STAGGER",
                format!("{:.2}", staggers[index % staggers.len()]),
            )
            .env("E3_CALL_FILE", &call)
            .env("E3_BASH_ERR_FILE", call.with_extension("err"))
            .env("E3_CHILD_RELEASE_FILE", &child_release)
            .env("E3_PROVIDER_RELEASE_FILE", &provider_release)
            .env("E3_EFFECT_FILE", &effect)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env(
                "OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1",
                uuid::Uuid::new_v4().to_string(),
            )
            .stdout(Stdio::from(File::create(&out).unwrap()))
            .stderr(Stdio::from(File::create(&err).unwrap()))
            .spawn()
            .unwrap();
        launches.push((child, out, err, call, ready));
    }
    let deadline = Instant::now() + Duration::from_secs(240);
    let launch_errors = |launches: &[(Child, PathBuf, PathBuf, PathBuf, PathBuf)]| {
        launches
            .iter()
            .map(|(_, _, err, _, _)| fs::read_to_string(err).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" | ")
    };
    for (child, _, err, _, ready) in launches.iter_mut() {
        wait_for(
            deadline,
            "provider not ready",
            || ready.exists() || child.try_wait().unwrap().is_some(),
            || {
                format!(
                    "{} / {}",
                    fs::read_to_string(&*err).unwrap_or_default(),
                    log()
                )
            },
        );
        if !ready.exists() {
            // Keep the records of a refused launch for diagnosis.
            if let Ok(records) = phase_record::read_all(&state) {
                fs::create_dir_all(&output).unwrap();
                let label = std::env::var("AGE353_E3_LABEL").unwrap_or_else(|_| "run".into());
                let raw: String = records.iter().map(|record| format!("{record}\n")).collect();
                fs::write(output.join(format!("{label}-refused-records.jsonl")), raw).unwrap();
            }
            panic!(
                "launcher exited early: {} / broker: {}",
                fs::read_to_string(&*err).unwrap_or_default(),
                log()
            );
        }
    }
    // Every original Runner is now in its 50 ms Q loop. Let the loops and
    // the extra traffic settle before the calls start.
    let stop = Arc::new(AtomicBool::new(false));
    let traffic_counts = Arc::new(AtomicU64::new(0));
    let mut traffic = Vec::new();
    for _ in 0..pollers {
        let (stop, counts, v30) = (Arc::clone(&stop), Arc::clone(&traffic_counts), v30.clone());
        traffic.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = protocol::request_at(&v30, Operation::ReadStateRoute);
                counts.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(50));
            }
        }));
    }
    for _ in 0..control_clients {
        let (stop, counts, socket) = (
            Arc::clone(&stop),
            Arc::clone(&traffic_counts),
            socket.clone(),
        );
        traffic.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = protocol::observe_entry_gate_at(&socket);
                counts.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(50));
            }
        }));
    }
    std::thread::sleep(Duration::from_secs(2));
    fs::write(&go, b"go").unwrap();
    for (child, _, err, call, _) in launches.iter_mut() {
        wait_for(
            deadline,
            "Bash call did not finish",
            || call.exists() || child.try_wait().unwrap().is_some(),
            || {
                format!(
                    "{} / {}",
                    fs::read_to_string(&*err).unwrap_or_default(),
                    log()
                )
            },
        );
    }
    // Keep the loopers polling a little past the last call.
    std::thread::sleep(Duration::from_millis(500));
    stop.store(true, Ordering::Relaxed);
    for thread in traffic {
        thread.join().unwrap();
    }
    let calls: Vec<(u64, u64, i32)> = launches
        .iter()
        .map(|(_, _, err, call, _)| {
            let text = fs::read_to_string(call).unwrap_or_else(|_| {
                panic!(
                    "call absent: {}",
                    fs::read_to_string(err).unwrap_or_default()
                )
            });
            let fields: Vec<_> = text.split_whitespace().collect();
            (
                fields[0].parse().unwrap(),
                fields[1].parse().unwrap(),
                fields[2].parse().unwrap(),
            )
        })
        .collect();
    for (index, call) in calls.iter().enumerate() {
        assert_eq!(
            call.2,
            0,
            "Bash call {index} failed: {} / Bash: {} / broker: {}",
            launch_errors(&launches),
            fs::read_to_string(launches[index].3.with_extension("err")).unwrap_or_default(),
            log()
        );
    }
    // As in the busy case: every child's W is selected while its provider is
    // still live, then the providers exit and each root settles.
    fs::write(&child_release, b"release").unwrap();
    wait_for(
        deadline,
        "selected W absent",
        || {
            rusqlite::Connection::open(state.join("v30/state.db"))
                .and_then(|fresh| {
                    fresh.query_row(
                        "SELECT count(*) FROM fresh_bash_selected_event",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                })
                .is_ok_and(|count| count == roots as i64)
        },
        &log,
    );
    fs::write(&provider_release, b"release").unwrap();
    for (child, out, err, _, _) in launches.iter_mut() {
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!(
                    "launcher timed out: {} / {}",
                    fs::read_to_string(&*err).unwrap_or_default(),
                    log()
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        assert!(
            status.success(),
            "launcher: {} / broker: {}",
            fs::read_to_string(&*err).unwrap_or_default(),
            log()
        );
        let dispatch: serde_json::Value =
            serde_json::from_str(fs::read_to_string(&*out).unwrap().lines().next().unwrap())
                .unwrap();
        assert_eq!(dispatch["delivery_mode"], "async");
        assert_eq!(dispatch["dispatch_state"], "broker-k-consumed");
    }
    assert_eq!(fs::read_to_string(&effect).unwrap().lines().count(), roots);
    broker.kill().unwrap();
    broker.wait().unwrap();
    // A Broker built before phase records still runs the same mix.
    if std::env::var_os("AGE353_E3_WITHOUT_RECORDS").is_some() {
        let wholes: Vec<f64> = calls
            .iter()
            .map(|(start, end, _)| (end - start) as f64 / 1e9)
            .collect();
        println!("AGE353_E3_CALL_WHOLE_S {wholes:?}");
        return;
    }

    let records = phase_record::read_all(&state).unwrap();
    let summary = summarize(&records, &calls, traffic_counts.load(Ordering::Relaxed));
    fs::create_dir_all(&output).unwrap();
    let label = std::env::var("AGE353_E3_LABEL").unwrap_or_else(|_| "run".into());
    fs::write(
        output.join(format!("{label}-summary.json")),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
    let mut raw = String::new();
    for record in &records {
        raw.push_str(&record.to_string());
        raw.push('\n');
    }
    fs::write(output.join(format!("{label}-records.jsonl")), raw).unwrap();
    fs::write(output.join(format!("{label}-broker.err")), log()).unwrap();
    println!(
        "{}",
        serde_json::to_string_pretty(&summary["calls"]).unwrap()
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&summary["totals"]).unwrap()
    );
    if std::env::var_os("AGE353_E3_REQUIRE_TARGETS").is_some() {
        for call in summary["calls"].as_array().unwrap() {
            assert!(call["whole_s"].as_f64().unwrap() < 5.0, "{call}");
            assert!(
                call["worst_pre_challenge_s"].as_f64().unwrap() <= 5.0,
                "{call}"
            );
        }
    }
}

fn num(value: &serde_json::Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}

fn seconds(ns: u64) -> f64 {
    ns as f64 / 1e9
}

/// Joins each provider-timed Bash call with its four recorded connections
/// (same peer PID) and attributes every connection's wait before its
/// challenge, its handler and its sub-phases.
fn summarize(
    records: &[serde_json::Value],
    calls: &[(u64, u64, i32)],
    traffic: u64,
) -> serde_json::Value {
    let start = records
        .iter()
        .find(|record| record["k"] == "start")
        .expect("start record");
    let wall_to_mono = |wall: u64| wall - num(start, "wall_ns") + num(start, "mono_ns");
    let requests: Vec<_> = records
        .iter()
        .filter(|record| record["k"] == "req")
        .collect();
    let mut by_pid: BTreeMap<i64, Vec<&serde_json::Value>> = BTreeMap::new();
    for request in &requests {
        if request["img"] == "bash" {
            by_pid
                .entry(request["pid"].as_i64().unwrap())
                .or_default()
                .push(request);
        }
    }
    let mut call_summaries = Vec::new();
    // Each call is the earliest still-unclaimed Bash process whose first
    // connection lies inside the call's provider-timed window.
    let mut claimed = Vec::new();
    let mut order: Vec<usize> = (0..calls.len()).collect();
    order.sort_by_key(|&index| calls[index].0);
    let mut assigned = BTreeMap::new();
    for index in order {
        let (mono_start, mono_end) = (wall_to_mono(calls[index].0), wall_to_mono(calls[index].1));
        let pid = by_pid
            .iter()
            .filter(|(pid, _)| !claimed.contains(*pid))
            .filter(|(_, connections)| {
                let first = connections
                    .iter()
                    .map(|request| num(request, "acc"))
                    .min()
                    .unwrap();
                first >= mono_start && first <= mono_end
            })
            .min_by_key(|(_, connections)| {
                connections
                    .iter()
                    .map(|request| num(request, "acc"))
                    .min()
                    .unwrap()
            })
            .map(|(pid, _)| *pid)
            .unwrap_or_else(|| panic!("no recorded Bash connections for call {index}"));
        claimed.push(pid);
        assigned.insert(index, pid);
    }
    for (index, (wall_start, wall_end, _)) in calls.iter().enumerate() {
        let (mono_start, mono_end) = (wall_to_mono(*wall_start), wall_to_mono(*wall_end));
        let pid = assigned[&index];
        let mut connections = by_pid[&pid].clone();
        connections.sort_by_key(|request| num(request, "acc"));
        let ops: Vec<i64> = connections
            .iter()
            .map(|request| request["op"].as_i64().unwrap())
            .collect();
        assert_eq!(ops, BASH_CONNECTIONS, "call {index} connection opcodes");
        let mut previous_end = mono_start;
        let mut details = Vec::new();
        let (mut pre_total, mut handler_total, mut worst_pre) = (0u64, 0u64, 0u64);
        for request in &connections {
            let challenge = num(request, "chl");
            let pre = challenge.saturating_sub(previous_end);
            let handler = num(request, "he").saturating_sub(num(request, "hs"));
            // Requests of other clients whose handlers ran on the same lane
            // between this connection's previous reply and its own handler.
            let others: u64 = requests
                .iter()
                .filter(|other| {
                    other["pid"] != request["pid"]
                        && other["lane"] == request["lane"]
                        && num(other, "hs") > 0
                })
                .map(|other| {
                    let (from, to) = (num(other, "hs"), num(other, "he"));
                    let (window_from, window_to) = (previous_end, num(request, "hs"));
                    to.min(window_to).saturating_sub(from.max(window_from))
                })
                .sum();
            pre_total += pre;
            handler_total += handler;
            worst_pre = worst_pre.max(pre);
            details.push(serde_json::json!({
                "op": request["op"],
                "lane": request["lane"],
                "pre_challenge_s": seconds(pre),
                "challenge_to_handler_s": seconds(num(request, "hs").saturating_sub(challenge)),
                "handler_s": seconds(handler),
                "reply_write_s": seconds(num(request, "rw").saturating_sub(num(request, "he"))),
                "other_clients_handler_s_during_wait": seconds(others),
                "parent_s": seconds(num(request, "parent")),
                "accounting_s": seconds(num(request, "acct")),
                "accounted_entries": request["acct_entries"],
                "fence_wait_s": seconds(num(request, "fence")),
                "bridge_wait_s": seconds(num(request, "bridge")),
                "state_s": seconds(num(request, "state")),
                "sqlite_busy_s": seconds(num(request, "sqlite_busy")),
            }));
            previous_end = num(request, "rw");
        }
        let whole = mono_end.saturating_sub(mono_start);
        call_summaries.push(serde_json::json!({
            "call": index,
            "bash_pid": pid,
            "whole_s": seconds(whole),
            "pre_challenge_total_s": seconds(pre_total),
            "handler_total_s": seconds(handler_total),
            "worst_pre_challenge_s": seconds(worst_pre),
            "wait_share": pre_total as f64 / whole.max(1) as f64,
            "connections": details,
        }));
    }
    let mut lanes: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    for request in &requests {
        let entry = lanes
            .entry(format!(
                "{}:{}",
                request["lane"].as_str().unwrap(),
                request["img"].as_str().unwrap()
            ))
            .or_default();
        entry.0 += 1;
        // A request whose read failed never started a handler.
        if num(request, "hs") > 0 {
            entry.1 += num(request, "he").saturating_sub(num(request, "hs"));
        }
        entry.2 += num(request, "bridge");
        entry.3 += num(request, "fence");
    }
    let lane_totals: BTreeMap<_, _> = lanes
        .into_iter()
        .map(|(key, (count, handler, bridge, fence))| {
            (
                key,
                serde_json::json!({
                    "requests": count,
                    "handler_s": seconds(handler),
                    "bridge_wait_s": seconds(bridge),
                    "fence_wait_s": seconds(fence),
                }),
            )
        })
        .collect();
    let windows: Vec<_> = records
        .iter()
        .filter(|record| record["k"] == "main")
        .collect();
    let mut main = BTreeMap::new();
    for key in [
        "reap",
        "bridge",
        "advance",
        "control",
        "idle_duty",
        "idle_sleep",
    ] {
        main.insert(
            key,
            seconds(windows.iter().map(|window| num(window, key)).sum()),
        );
    }
    for key in ["it", "bridge_n", "advance_n", "control_n", "idle_n"] {
        main.insert(
            key,
            windows.iter().map(|window| num(window, key)).sum::<u64>() as f64,
        );
    }
    serde_json::json!({
        "calls": call_summaries,
        "totals": {
            "requests_by_lane_and_image": lane_totals,
            "main_windows": windows.len(),
            "main": main,
            "test_traffic_requests": traffic,
            "records": records.len(),
        },
    })
}
