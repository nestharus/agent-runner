#![cfg(all(target_os = "linux", not(feature = "age319-private-broker-fixture")))]
//! AGE-353 serving witness: a natural 2+2 (two busy, two sleeping async Bash
//! roots) on one featureless Broker, each root carried through W, F, receipt,
//! ACK, caller publication, PID1 terminal and owner close until its launcher
//! reads the drained result.
//!
//! The Broker, Runner, installed launcher and Bash images are real
//! featureless builds; State, roots and providers are owned by this
//! disposable user namespace. Optional prior roots enlarge the retained N
//! before the measured 2+2. The run's phase records are summarized per
//! opcode and per root for the caller; `AGE353_SERVING_REQUIRE` turns the
//! whole-call bound and refusal count into assertions.

use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::phase_record;
use oulipoly_kernel_broker::protocol::{self, EntryRoute};
use oulipoly_state::mailbox::EmptyV30BootstrapIdentity;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TEST: &str = "age353_natural_two_busy_two_sleeping_through_owner_close";

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

const PROVIDER: &[u8] = br#"#!/bin/sh
export OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1="$AGE319_BASH_CONTROL_SOCKET"
mark() { printf '%s %s\n' "$1" "$(date +%s%N)" >> "$N_MARKS"; }
mark provider-start
while [ ! -f "$N_GO_FILE" ]; do sleep 0.02; done
sleep "$N_STAGGER"
start=$(date +%s%N)
response=$("$AGE319_BASH_IMAGE" run --delivery async -- /bin/sh -c '
  while [ ! -f "$N_CHILD_RELEASE" ]; do sleep 0.02; done
  printf "child-effect %s\n" "$(date +%s%N)" >> "$N_MARKS"
  printf "payload-out\n"; printf "payload-err\n" >&2') || { mark bash-refused; exit 71; }
end=$(date +%s%N)
printf '%s %s\n' "$start" "$end" > "$N_CALL_FILE"
printf '%s\n' "$response" > "$N_DISPATCH_FILE"
printf '%s\n' "$response"
mark dispatched
if [ "$N_MODE" = busy ]; then
  while [ ! -f "$N_PROVIDER_RELEASE" ]; do sleep 0.02; done
fi
mark provider-exit
"#;

struct Launch {
    label: String,
    busy: bool,
    child: Child,
    dir: PathBuf,
    started: u64,
    exited: Option<(u64, Option<i32>)>,
    step: u8,
}

impl Launch {
    fn marks(&self) -> String {
        fs::read_to_string(self.dir.join("marks")).unwrap_or_default()
    }

    fn has(&self, mark: &str) -> bool {
        self.marks().lines().any(|line| line.starts_with(mark))
    }

    fn request_id(&self) -> Option<String> {
        let text = fs::read_to_string(self.dir.join("dispatch")).ok()?;
        let value: serde_json::Value = serde_json::from_str(text.lines().next()?).ok()?;
        value["request_id"].as_str().map(str::to_owned)
    }
}

fn selected(state: &Path, request_id: &str) -> bool {
    let Ok(db) = rusqlite::Connection::open_with_flags(
        state.join("v30/state.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return false;
    };
    db.query_row(
        "SELECT count(*) FROM fresh_bash_selected_event
         WHERE json_extract(receipt_json,'$.request_id')=?1",
        [request_id],
        |row| row.get::<_, i64>(0),
    )
    .is_ok_and(|count| count > 0)
}

fn provider_exit_recorded(state: &Path, request_id: &str) -> bool {
    let Ok(db) = rusqlite::Connection::open_with_flags(
        state.join("v30/state.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return false;
    };
    let parent: Option<String> = db
        .query_row(
            "SELECT json_extract(receipt_json,'$.parent_work_grant_id')
             FROM fresh_bash_child WHERE request_id=?1",
            [request_id],
            |row| row.get(0),
        )
        .ok();
    parent.is_some_and(|parent| {
        state
            .join("v30/normal-provider")
            .join(parent)
            .join("provider-exit.json")
            .exists()
    })
}

struct Fixture {
    temp: PathBuf,
    state: PathBuf,
    socket: PathBuf,
    manifest: PathBuf,
    launcher: PathBuf,
    bash: PathBuf,
    config_home: PathBuf,
    negatives: bool,
}

/// Completion drivers seen in this namespace, by PID, with the wait status
/// once this process (the namespace's PID1, their adoptive parent) reaps
/// them. `None` while still running.
#[derive(Default)]
struct Drivers(BTreeMap<i32, Option<i32>>);

impl Drivers {
    fn observe(&mut self) {
        for entry in fs::read_dir("/proc").unwrap().flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            let cmdline = fs::read(entry.path().join("cmdline")).unwrap_or_default();
            if cmdline
                .split(|byte| *byte == 0)
                .any(|arg| arg == b"__completion-driver-v1")
            {
                self.0.entry(pid).or_insert(None);
            }
        }
        for (pid, status) in self.0.iter_mut().filter(|(_, status)| status.is_none()) {
            let mut raw = 0;
            if unsafe { libc::waitpid(*pid, &mut raw, libc::WNOHANG) } == *pid {
                *status = Some(raw);
            }
        }
    }
}

impl Fixture {
    fn launch(&self, label: &str, busy: bool, stagger: f64) -> Launch {
        let dir = self.temp.join(format!("launch-{label}"));
        fs::create_dir(&dir).unwrap();
        let started = phase_record::mono_ns();
        let child = Command::new(&self.launcher)
            .args(["--model", "fixture-model", "hello natural"])
            .current_dir(&self.temp)
            .env_clear()
            .env("HOME", &self.temp)
            .env("PATH", "/usr/bin:/bin")
            .env("OULIPOLY_CONFIG_HOME", &self.config_home)
            .env("OULIPOLY_DATA_DIR", dir.join("data"))
            .env("AGE319_BASH_IMAGE", &self.bash)
            .env("AGE319_BASH_CONTROL_SOCKET", &self.socket)
            .env("N_MODE", if busy { "busy" } else { "sleeping" })
            .env("N_MARKS", dir.join("marks"))
            .env("N_GO_FILE", self.temp.join("go"))
            .env("N_STAGGER", format!("{stagger:.2}"))
            .env("N_CALL_FILE", dir.join("call"))
            .env("N_DISPATCH_FILE", dir.join("dispatch"))
            .env("N_CHILD_RELEASE", dir.join("child-release"))
            .env("N_PROVIDER_RELEASE", dir.join("provider-release"))
            .envs(
                self.negatives
                    .then_some([
                        ("AGE319_TEST_FEATURELESS_SUCCESSOR_NEGATIVES_V1", "1"),
                        ("AGE319_TEST_FEATURELESS_REPLAY_ACK_V1", "1"),
                    ])
                    .into_iter()
                    .flatten(),
            )
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &self.manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &self.launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &self.socket)
            .env(
                "OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1",
                uuid::Uuid::new_v4().to_string(),
            )
            // A root refuses a TTY standard descriptor; never inherit one.
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(dir.join("out")).unwrap()))
            .stderr(Stdio::from(File::create(dir.join("err")).unwrap()))
            .spawn()
            .unwrap();
        Launch {
            label: label.into(),
            busy,
            child,
            dir,
            started,
            exited: None,
            step: 0,
        }
    }

    /// Advances each launch's holds in the harness order of the installed
    /// L1 (busy: dispatched, child, W selected, provider; sleeping:
    /// dispatched, provider exit, child) until every launcher exits.
    fn drive(
        &self,
        launches: &mut [Launch],
        drivers: &mut Drivers,
        bound: Duration,
        log: &dyn Fn() -> String,
    ) {
        let deadline = Instant::now() + bound;
        loop {
            drivers.observe();
            let mut all = true;
            for launch in launches.iter_mut() {
                if launch.exited.is_none()
                    && let Some(status) = launch.child.try_wait().unwrap()
                {
                    launch.exited = Some((phase_record::mono_ns(), status.code()));
                }
                if launch.exited.is_some() {
                    continue;
                }
                all = false;
                let release = |name: &str| fs::write(launch.dir.join(name), b"release").unwrap();
                match (launch.busy, launch.step) {
                    (_, 0) if launch.has("dispatched") => {
                        if launch.busy {
                            release("child-release");
                        }
                        launch.step = 1;
                    }
                    (true, 1) if launch.has("child-effect") => launch.step = 2,
                    (true, 2)
                        if launch
                            .request_id()
                            .is_some_and(|id| selected(&self.state, &id)) =>
                    {
                        release("provider-release");
                        launch.step = 3;
                    }
                    (false, 1)
                        if launch.has("provider-exit")
                            && launch
                                .request_id()
                                .is_some_and(|id| provider_exit_recorded(&self.state, &id)) =>
                    {
                        release("child-release");
                        launch.step = 3;
                    }
                    _ => {}
                }
            }
            if all {
                return;
            }
            if Instant::now() >= deadline {
                let detail: Vec<String> = launches
                    .iter()
                    .map(|launch| {
                        format!(
                            "{} step{} marks[{}] err[{}]",
                            launch.label,
                            launch.step,
                            launch.marks().replace('\n', ";"),
                            fs::read_to_string(launch.dir.join("err")).unwrap_or_default()
                        )
                    })
                    .collect();
                for launch in launches.iter_mut() {
                    let _ = launch.child.kill();
                }
                panic!("launches did not finish: {detail:?} / broker: {}", log());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn age353_natural_two_busy_two_sleeping_through_owner_close() {
    let Ok(runner_image) = std::env::var("AGE353_SERVING_RUNNER_BIN") else {
        return;
    };
    let Ok(bash_image) = std::env::var("AGE353_SERVING_BASH_BIN") else {
        return;
    };
    let output = PathBuf::from(
        std::env::var("AGE353_SERVING_OUTPUT_DIR")
            .expect("AGE353_SERVING_OUTPUT_DIR names the summary destination"),
    );
    if std::env::var_os("AGE353_SERVING_CHILD").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env("AGE353_SERVING_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success(), "natural serving fixture failed");
        return;
    }
    let modes: Vec<bool> = env_or("AGE353_SERVING_MODES", "b,b,s,s")
        .split(',')
        .map(|mode| mode == "b")
        .collect();
    // Seconds after the common start at which each root's call begins
    // (the installed L1's s2, b2, b1, s1 order, compressed).
    let staggers: Vec<f64> = env_or("AGE353_SERVING_STAGGERS", "0.3,0.15,0,0.45")
        .split(',')
        .map(|value| value.parse().unwrap())
        .collect();
    let prior: usize = env_or("AGE353_SERVING_PRIOR_ROOTS", "0").parse().unwrap();

    let parent = match std::env::var_os("AGE353_SERVING_TMP_BIND") {
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
        .prefix("age353-serving-")
        .tempdir_in(parent)
        .unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let installed = temp.path().join("installed");
    fs::create_dir(&installed).unwrap();
    let runner = installed.join("oulipoly-agent-runner");
    let broker_image = installed.join("oulipoly-kernel-broker");
    let launcher = installed.join("oulipoly-installed-launcher");
    let bash = installed.join("agent-bash");
    fs::copy(&runner_image, &runner).unwrap();
    fs::copy(
        std::env::var("AGE353_SERVING_BROKER_BIN")
            .unwrap_or_else(|_| env!("CARGO_BIN_EXE_oulipoly-kernel-broker").into()),
        &broker_image,
    )
    .unwrap();
    fs::copy(
        std::env::var("AGE353_SERVING_LAUNCHER_BIN")
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
    let broker_log = temp.path().join("broker.err");
    let start_broker = || {
        let mut broker = Command::new(&broker_image)
            .env_clear()
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1", &broker_image)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &bash)
            .stderr(Stdio::from(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&broker_log)
                    .unwrap(),
            ))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while protocol::observe_entry_gate_at(&socket).ok() != Some(EntryRoute::FreshOnlyOpen) {
            assert!(
                broker.try_wait().unwrap().is_none(),
                "{}",
                fs::read_to_string(&broker_log).unwrap_or_default()
            );
            assert!(Instant::now() < deadline, "Broker did not open fresh route");
            std::thread::sleep(Duration::from_millis(20));
        }
        broker
    };
    let mut broker = start_broker();
    let log = || fs::read_to_string(&broker_log).unwrap_or_default();

    let config_home = temp.path().join("config-home");
    let config = config_home.join("oulipoly-agent-runner");
    fs::create_dir_all(config.join("models")).unwrap();
    let provider = temp.path().join("provider.sh");
    fs::write(&provider, PROVIDER).unwrap();
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
    let fixture = Fixture {
        temp: temp.path().to_path_buf(),
        state: state.clone(),
        socket: socket.clone(),
        manifest: manifest.clone(),
        launcher: launcher.clone(),
        bash: bash.clone(),
        config_home,
        negatives: std::env::var_os("AGE353_SERVING_NEGATIVES").is_some(),
    };
    let mut drivers = Drivers::default();

    // Prior roots retain N before the measured run, in batches of the same
    // mix; a restart afterwards empties every process-local cache.
    fs::write(temp.path().join("go"), b"go").unwrap();
    let mut prior_done = 0;
    while prior_done < prior {
        let batch = (prior - prior_done).min(modes.len());
        let mut launches: Vec<Launch> = (0..batch)
            .map(|index| {
                fixture.launch(
                    &format!("prior-{}", prior_done + index),
                    modes[index % modes.len()],
                    0.0,
                )
            })
            .collect();
        fixture.drive(&mut launches, &mut drivers, Duration::from_secs(600), &log);
        for launch in &launches {
            assert_eq!(
                launch.exited.unwrap().1,
                Some(0),
                "prior {} failed: {}",
                launch.label,
                fs::read_to_string(launch.dir.join("err")).unwrap_or_default()
            );
        }
        prior_done += batch;
    }
    if prior > 0 && std::env::var_os("AGE353_SERVING_RESTART_AFTER_PRIOR").is_some() {
        broker.kill().unwrap();
        broker.wait().unwrap();
        broker = start_broker();
    }
    fs::remove_file(temp.path().join("go")).unwrap();

    let measured_from = phase_record::mono_ns();
    let mut launches: Vec<Launch> = modes
        .iter()
        .enumerate()
        .map(|(index, &busy)| {
            let label = format!("{}{}", if busy { "b" } else { "s" }, index + 1);
            fixture.launch(&label, busy, staggers[index % staggers.len()])
        })
        .collect();
    let ready_deadline = Instant::now() + Duration::from_secs(240);
    for launch in launches.iter_mut() {
        while !launch.has("provider-start") {
            if launch.child.try_wait().unwrap().is_some() {
                // Keep the records of a refused launch for diagnosis.
                if let Ok(records) = phase_record::read_all(&state) {
                    fs::create_dir_all(&output).unwrap();
                    let raw: String = records.iter().map(|record| format!("{record}\n")).collect();
                    let label = env_or("AGE353_SERVING_LABEL", "run");
                    fs::write(output.join(format!("{label}-refused-records.jsonl")), raw).unwrap();
                }
                panic!(
                    "launcher exited early: {} / broker: {}",
                    fs::read_to_string(launch.dir.join("err")).unwrap_or_default(),
                    log()
                );
            }
            assert!(Instant::now() < ready_deadline, "provider not ready");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let go = phase_record::mono_ns();
    fs::write(temp.path().join("go"), b"go").unwrap();
    let mut measured_drivers = Drivers::default();
    fixture.drive(
        &mut launches,
        &mut measured_drivers,
        Duration::from_secs(600),
        &log,
    );
    // Drivers exit after their guardian's D; reap the measured ones.
    let reap_deadline = Instant::now() + Duration::from_secs(30);
    while measured_drivers.0.values().any(Option::is_none) && Instant::now() < reap_deadline {
        measured_drivers.observe();
        std::thread::sleep(Duration::from_millis(20));
    }
    let measured_to = phase_record::mono_ns();
    std::thread::sleep(Duration::from_millis(300));
    broker.kill().unwrap();
    broker.wait().unwrap();

    let wall_offset = {
        let wall = phase_record::wall_ns();
        wall as i128 - phase_record::mono_ns() as i128
    };
    let to_mono = |wall: u64| (wall as i128 - wall_offset) as u64;
    let mut roots = Vec::new();
    for launch in &launches {
        let (exited, code) = launch.exited.unwrap();
        let err = fs::read_to_string(launch.dir.join("err")).unwrap_or_default();
        let call = fs::read_to_string(launch.dir.join("call")).unwrap_or_default();
        let fields: Vec<u64> = call
            .split_whitespace()
            .filter_map(|field| field.parse().ok())
            .collect();
        let whole = (fields.len() == 2).then(|| (fields[1] - fields[0]) as f64 / 1e9);
        let mark = |name: &str| {
            launch
                .marks()
                .lines()
                .find(|line| line.starts_with(name))
                .and_then(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
                .map(|wall| (to_mono(wall).saturating_sub(go)) as f64 / 1e9)
        };
        roots.push(serde_json::json!({
            "label": launch.label,
            "mode": if launch.busy { "busy" } else { "sleeping" },
            "launcher_exit": code,
            "launcher_err_tail": err.chars().rev().take(400).collect::<String>().chars().rev().collect::<String>(),
            "whole_call_s": whole,
            "launch_to_launcher_exit_s": (exited - launch.started) as f64 / 1e9,
            "go_to_dispatched_s": mark("dispatched"),
            "go_to_child_effect_s": mark("child-effect"),
            "go_to_launcher_exit_s": (exited.saturating_sub(go)) as f64 / 1e9,
            "driver_gaps": fs::read_dir(launch.dir.join("data/kernel-v30-driver-gaps"))
                .map(|dir| {
                    dir.flatten()
                        .map(|gap| fs::read_to_string(gap.path()).unwrap_or_default())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        }));
    }
    let (records, _) = phase_record::read_all_counted(&state).unwrap();
    let summary = serde_json::json!({
        "modes": env_or("AGE353_SERVING_MODES", "b,b,s,s"),
        "prior_roots": prior,
        "retained_entries_after": fs::read_dir(state.join("entries")).map(|d| d.count()).unwrap_or(0),
        "measured_s": (measured_to - measured_from) as f64 / 1e9,
        "roots": roots,
        "negatives": fixture.negatives,
        "driver_wait_statuses": measured_drivers.0.values().collect::<Vec<_>>(),
        "requests": summarize(&records, measured_from, measured_to),
        "main": main_totals(&records, measured_from, measured_to),
    });
    fs::create_dir_all(&output).unwrap();
    let label = env_or("AGE353_SERVING_LABEL", "run");
    fs::write(
        output.join(format!("{label}-summary.json")),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
    let raw: String = records.iter().map(|record| format!("{record}\n")).collect();
    fs::write(output.join(format!("{label}-records.jsonl")), raw).unwrap();
    fs::write(output.join(format!("{label}-broker.err")), log()).unwrap();
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    for launch in &launches {
        assert_eq!(
            launch.exited.unwrap().1,
            Some(0),
            "{} failed: {}",
            launch.label,
            fs::read_to_string(launch.dir.join("err")).unwrap_or_default()
        );
    }
    // Normal handoffs send no request the Broker must refuse. With the
    // disposable negatives, every sleeping original's replayed start,
    // wrong-offer start and wrong-offer approval, and every busy original's
    // ACK replay, were refused and the handoff still completed.
    let sleeping = launches.iter().filter(|launch| !launch.busy).count() as u64;
    let busy = launches.len() as u64 - sleeping;
    assert_eq!(
        summary["requests"]["refused_general"].as_u64(),
        Some(if fixture.negatives {
            3 * sleeping + busy
        } else {
            0
        }),
        "refused F requests"
    );
    // A driver with nothing to repair exits 0 and retains no gap.
    for root in summary["roots"].as_array().unwrap() {
        assert_eq!(root["driver_gaps"], serde_json::json!([]), "{root}");
    }
    assert_eq!(
        measured_drivers.0.len(),
        launches.len(),
        "one driver per root"
    );
    for status in measured_drivers.0.values() {
        assert_eq!(*status, Some(0), "driver wait status");
    }
    observability_records(&launches, &measured_drivers, &records, &state, &output);
    if std::env::var_os("AGE353_SERVING_REQUIRE").is_some() {
        for root in summary["roots"].as_array().unwrap() {
            assert!(root["whole_call_s"].as_f64().unwrap() <= 5.0, "{root}");
        }
    }
}

/// The observability records of this real 2+2, checked against what this
/// process observed independently: each driver's own outcome record against
/// the wait status this namespace PID1 reaped; each retained root's
/// owner-close selection timeline through its completing visit; each
/// successor's spawn and exit. Written beside the summary for collection.
fn observability_records(
    launches: &[Launch],
    drivers: &Drivers,
    records: &[serde_json::Value],
    state: &Path,
    output: &Path,
) {
    let mut outcomes = Vec::new();
    for launch in launches {
        let directory = launch.dir.join("data/kernel-v30-driver-outcomes");
        let files: Vec<PathBuf> = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{}: no driver outcome: {error}", launch.label))
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(files.len(), 1, "{}: one driver per root", launch.label);
        let lines: Vec<serde_json::Value> = fs::read_to_string(&files[0])
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2, "{}: {lines:?}", launch.label);
        let (begin, end) = (&lines[0], &lines[1]);
        assert_eq!(
            (begin["event"].as_str(), end["event"].as_str()),
            (Some("begin"), Some("end"))
        );
        let pid = begin["pid"].as_i64().unwrap() as i32;
        let raw = drivers
            .0
            .get(&pid)
            .copied()
            .flatten()
            .unwrap_or_else(|| panic!("{}: driver {pid} was not reaped here", launch.label));
        // The record's exit is the status its adoptive parent reaped.
        assert!(libc::WIFEXITED(raw), "{}: {raw}", launch.label);
        assert_eq!(
            end["exit"].as_i64(),
            Some(i64::from(libc::WEXITSTATUS(raw)))
        );
        assert_eq!(
            end["result"],
            if libc::WEXITSTATUS(raw) == 0 {
                "ok"
            } else {
                "error"
            }
        );
        assert!(
            files[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(begin["root"].as_str().unwrap())
        );
        outcomes.push(serde_json::json!({"label": launch.label, "begin": begin, "end": end, "wait_status": raw}));
    }
    let entries: Vec<String> = fs::read_dir(state.join("entries"))
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .collect();
    let mut timelines = serde_json::Map::new();
    for root in &entries {
        let admitted: Vec<_> = records
            .iter()
            .filter(|record| record["k"] == "oca" && record["root"] == root.as_str())
            .collect();
        assert_eq!(admitted.len(), 1, "{root} admitted once");
        let visits: Vec<_> = records
            .iter()
            .filter(|record| record["k"] == "ocv" && record["root"] == root.as_str())
            .collect();
        let last = visits
            .last()
            .unwrap_or_else(|| panic!("{root} never selected"));
        assert_eq!(last["out"], "complete", "{root}: {visits:?}");
        assert_eq!(
            visits
                .iter()
                .filter(|visit| visit["out"] == "complete")
                .count(),
            1,
            "a completed root is never selected again"
        );
        let gaps: Vec<u64> = visits
            .windows(2)
            .map(|pair| num(pair[1], "from") - num(pair[0], "from"))
            .collect();
        let steps: Vec<&str> = visits
            .iter()
            .filter_map(|visit| visit["step"].as_str())
            .collect();
        timelines.insert(
            root.clone(),
            serde_json::json!({
                "admitted_at": admitted[0]["at"],
                "visits": visits.len(),
                "max_revisit_ns": gaps.iter().max(),
                "completed_at": last["to"],
                "completing_step": last["step"],
                "steps": steps,
            }),
        );
    }
    let sleeping = launches.iter().filter(|launch| !launch.busy).count();
    let spawned: Vec<_> = records
        .iter()
        .filter(|record| record["k"] == "succ_spawn")
        .collect();
    assert_eq!(spawned.len(), sleeping, "one successor per sleeping root");
    for spawn in &spawned {
        let exits: Vec<_> = records
            .iter()
            .filter(|record| {
                record["k"] == "succ_exit"
                    && record["offer"] == spawn["offer"]
                    && record["pid"] == spawn["pid"]
            })
            .collect();
        assert!(
            exits
                .iter()
                .any(|exit| exit["how"] == "exited" && exit["code"] == 0),
            "{spawn}: {exits:?}"
        );
        assert!(
            exits
                .iter()
                .all(|exit| exit["how"] != "signaled"
                    && !(exit["how"] == "exited" && exit["code"] != 0)),
            "{exits:?}"
        );
    }
    let label = env_or("AGE353_SERVING_LABEL", "run");
    fs::write(
        output.join(format!("{label}-observability.json")),
        serde_json::to_vec_pretty(&serde_json::json!({
            "driver_outcomes": outcomes,
            "owner_close_timelines": timelines,
            "successors": spawned.len(),
        }))
        .unwrap(),
    )
    .unwrap();
    for launch in launches {
        let target = output.join(format!("{label}-{}-driver-outcomes", launch.label));
        let _ = fs::create_dir_all(&target);
        for entry in fs::read_dir(launch.dir.join("data/kernel-v30-driver-outcomes"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let _ = fs::copy(entry.path(), target.join(entry.file_name()));
        }
    }
}

fn num(value: &serde_json::Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}

/// Per lane and opcode: count, refusals, handler, thread CPU and queue
/// (accept to handler start) for requests accepted inside the window.
fn summarize(records: &[serde_json::Value], from: u64, to: u64) -> serde_json::Value {
    let mut groups: BTreeMap<String, [u64; 6]> = BTreeMap::new();
    let mut refused_general = 0;
    for request in records.iter().filter(|record| {
        record["k"] == "req" && num(record, "acc") >= from && num(record, "acc") <= to
    }) {
        let key = format!(
            "{}:{}",
            request["lane"].as_str().unwrap_or("?"),
            request["op"]
        );
        let entry = groups.entry(key).or_default();
        entry[0] += 1;
        if request["ok"] == false {
            entry[1] += 1;
            if request["lane"] == "v30" && request["op"] == 70 {
                refused_general += 1;
            }
        }
        if num(request, "hs") > 0 {
            let handler = num(request, "he").saturating_sub(num(request, "hs"));
            entry[2] += handler;
            entry[3] = entry[3].max(handler);
            entry[4] += num(request, "cpu");
            entry[5] += num(request, "hs").saturating_sub(num(request, "acc"));
        }
    }
    let groups: BTreeMap<_, _> = groups
        .into_iter()
        .map(|(key, [count, refused, handler, max, cpu, queue])| {
            (
                key,
                serde_json::json!({
                    "n": count,
                    "refused": refused,
                    "handler_s": handler as f64 / 1e9,
                    "handler_max_s": max as f64 / 1e9,
                    "cpu_s": cpu as f64 / 1e9,
                    "queue_s": queue as f64 / 1e9,
                }),
            )
        })
        .collect();
    serde_json::json!({ "by_lane_op": groups, "refused_general": refused_general })
}

fn main_totals(records: &[serde_json::Value], from: u64, to: u64) -> serde_json::Value {
    let windows: Vec<_> = records
        .iter()
        .filter(|record| {
            record["k"] == "main" && num(record, "to") >= from && num(record, "from") <= to
        })
        .collect();
    let mut totals = BTreeMap::new();
    for key in [
        "advance",
        "advance_cpu",
        "control",
        "bridge",
        "idle_sleep",
        "idle_duty",
    ] {
        totals.insert(
            key,
            serde_json::json!(windows.iter().map(|w| num(w, key)).sum::<u64>() as f64 / 1e9),
        );
    }
    for key in ["it", "advance_n", "adv_readback", "adv_acted", "control_n"] {
        totals.insert(
            key,
            serde_json::json!(windows.iter().map(|w| num(w, key)).sum::<u64>()),
        );
    }
    serde_json::json!(totals)
}
