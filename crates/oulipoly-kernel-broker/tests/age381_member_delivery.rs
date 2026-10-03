#![cfg(all(target_os = "linux", not(feature = "age319-private-broker-fixture")))]
//! AGE-381: one fresh root, several concurrent async (notify) Bash children.
//! The installed (featureless) Runner is the root's own recipient. Each
//! member's W must be delivered to that recipient, receipted and ACKed on
//! its own, and every closure consumer must agree with the whole member set.
//!
//! Run with the same images as `age319_featureless_installed`:
//! `AGE319_FEATURELESS_RUNNER_BIN`, `AGE319_FEATURELESS_BASH_BIN`, and a
//! `TMPDIR` whose ancestors are neither group- nor world-writable.

use base64::Engine as _;
use oulipoly_kernel_broker::first_install_activation::{FirstInstallActivation, PairPaths};
use oulipoly_kernel_broker::installed_launch_ledger::InstalledLaunchLedger;
use oulipoly_kernel_broker::installed_pair::InstalledPair;
use oulipoly_kernel_broker::protocol::{self, EntryRoute};
use oulipoly_kernel_broker::registry::RootRecord;
use oulipoly_state::mailbox::{
    EmptyV30BootstrapIdentity, FreshBashListenerPolicy, FreshBashSourceEvent,
    FreshRootTerminalReadback, FreshV30Lane,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// The provider stays live until every W is selected.
    Busy,
    /// The provider exits before any child reaches W.
    Sleeping,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    name: &'static str,
    mode: Mode,
    notify: usize,
    quiet_sibling: bool,
    /// The recipient stops after the second member's receipt is certified,
    /// before its ACK; that member's child also exits with a failure.
    partial: bool,
    /// After an accepted close, a late C is admitted and the Broker's
    /// owner-close readback is asked again.
    late_after_close: bool,
}

const CASES: &[Case] = &[
    Case {
        name: "one_notify_late_c_after_close_is_refused_by_broker_readback",
        mode: Mode::Busy,
        notify: 1,
        quiet_sibling: false,
        partial: false,
        late_after_close: true,
    },
    Case {
        name: "busy_one_notify_with_quiet_sibling_is_delivered_and_acked",
        mode: Mode::Busy,
        notify: 1,
        quiet_sibling: true,
        partial: false,
        late_after_close: false,
    },
    Case {
        name: "busy_three_concurrent_notify_members_are_each_delivered_and_acked",
        mode: Mode::Busy,
        notify: 3,
        quiet_sibling: false,
        partial: false,
        late_after_close: true,
    },
    Case {
        name: "sleeping_one_notify_with_quiet_sibling_is_delivered_and_acked",
        mode: Mode::Sleeping,
        notify: 1,
        quiet_sibling: true,
        partial: false,
        late_after_close: false,
    },
    Case {
        name: "sleeping_three_concurrent_notify_members_are_each_delivered_and_acked",
        mode: Mode::Sleeping,
        notify: 3,
        quiet_sibling: false,
        partial: false,
        late_after_close: false,
    },
    Case {
        name: "busy_partial_ack_and_failed_member_stay_unaccepted_across_broker_restart",
        mode: Mode::Busy,
        notify: 3,
        quiet_sibling: false,
        partial: true,
        late_after_close: false,
    },
    Case {
        name: "busy_ten_concurrent_notify_members_are_each_delivered_and_acked",
        mode: Mode::Busy,
        notify: 10,
        quiet_sibling: false,
        partial: false,
        late_after_close: false,
    },
    Case {
        name: "sleeping_ten_concurrent_notify_members_are_each_delivered_and_acked",
        mode: Mode::Sleeping,
        notify: 10,
        quiet_sibling: false,
        partial: false,
        late_after_close: false,
    },
];

#[test]
fn one_notify_late_c_after_close_is_refused_by_broker_readback() {
    run_case(CASES[0]);
}

#[test]
fn busy_one_notify_with_quiet_sibling_is_delivered_and_acked() {
    run_case(CASES[1]);
}

#[test]
fn busy_three_concurrent_notify_members_are_each_delivered_and_acked() {
    run_case(CASES[2]);
}

#[test]
fn sleeping_one_notify_with_quiet_sibling_is_delivered_and_acked() {
    run_case(CASES[3]);
}

#[test]
fn sleeping_three_concurrent_notify_members_are_each_delivered_and_acked() {
    run_case(CASES[4]);
}

#[test]
fn busy_partial_ack_and_failed_member_stay_unaccepted_across_broker_restart() {
    run_case(CASES[5]);
}

/// The size each authentic parent carries: ten live members at once.
#[test]
fn busy_ten_concurrent_notify_members_are_each_delivered_and_acked() {
    run_case(CASES[6]);
}

#[test]
fn sleeping_ten_concurrent_notify_members_are_each_delivered_and_acked() {
    run_case(CASES[7]);
}

fn digest(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}

fn run_case(case: Case) {
    let Ok(runner_image) = std::env::var("AGE319_FEATURELESS_RUNNER_BIN") else {
        return;
    };
    let Ok(bash_image) = std::env::var("AGE319_FEATURELESS_BASH_BIN") else {
        return;
    };
    if std::env::var_os("AGE381_MEMBER_DELIVERY_CHILD").is_none() {
        let status = Command::new("unshare")
            .args(["-Urpfm", "--mount-proc"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", case.name, "--nocapture"])
            .env("AGE381_MEMBER_DELIVERY_CHILD", "1")
            .env("AGE319_FEATURELESS_RUNNER_BIN", &runner_image)
            .env("AGE319_FEATURELESS_BASH_BIN", &bash_image)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "member delivery case {} failed",
            case.name
        );
        return;
    }
    Fixture::start(&runner_image, &bash_image).run(case);
}

struct Fixture {
    temp: tempfile::TempDir,
    state: PathBuf,
    socket: PathBuf,
    _socket_directory: File,
    broker_image: PathBuf,
    runner: PathBuf,
    launcher: PathBuf,
    bash: PathBuf,
    manifest: PathBuf,
    pair: InstalledPair,
    source_generation: String,
    broker: Child,
    broker_log: PathBuf,
}

impl Fixture {
    fn start(runner_image: &str, bash_image: &str) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("age381-member-")
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
        let mut fixture = Self {
            broker: Command::new("true").spawn().unwrap(),
            temp,
            state,
            socket,
            _socket_directory: socket_directory,
            broker_image,
            runner,
            launcher,
            bash,
            manifest,
            pair,
            source_generation: source.source_generation,
            broker_log,
        };
        fixture.broker.wait().unwrap();
        fixture.spawn_broker();
        fixture
    }

    fn spawn_broker(&mut self) {
        self.broker = Command::new(&self.broker_image)
            .env_clear()
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &self.state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &self.socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &self.runner)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &self.manifest)
            .env(
                "OULIPOLY_KERNEL_BROKER_FIXTURE_BROKER_V1",
                &self.broker_image,
            )
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &self.launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", &self.bash)
            .stderr(Stdio::from(
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.broker_log)
                    .unwrap(),
            ))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while protocol::observe_entry_gate_at(&self.socket).ok() != Some(EntryRoute::FreshOnlyOpen)
        {
            assert!(
                self.broker.try_wait().unwrap().is_none(),
                "Broker exited: {}",
                self.log()
            );
            assert!(
                Instant::now() < deadline,
                "Broker did not open: {}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn restart_broker(&mut self) {
        self.broker.kill().unwrap();
        self.broker.wait().unwrap();
        self.spawn_broker();
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.broker_log).unwrap_or_default()
    }

    fn path(&self, name: &str) -> PathBuf {
        self.temp.path().join(name)
    }

    fn fresh(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.state.join("v30/state.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    fn sidecar(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.state.join("v30/sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }

    fn terminal(&self, root_id: &str) -> FreshRootTerminalReadback {
        let lane = FreshV30Lane::open_at(&self.state).unwrap();
        let (root, actor) = lane.released_handoff_for_root(root_id).unwrap();
        let session = lane.read_session(&root.d_key).unwrap().unwrap();
        lane.read_private_root_terminal(&root, &actor, &session)
            .unwrap()
    }

    /// The Broker's owner-close readback for the exact root. `Ok(None)` is
    /// a readback without an accepted close.
    fn owner_close(&self, root_id: &str) -> Result<Option<serde_json::Value>, String> {
        let root: RootRecord =
            serde_json::from_slice(&fs::read(self.state.join(format!("{root_id}.json"))).unwrap())
                .unwrap();
        let reply = protocol::root_drain_readback_at(&self.socket, &root, false)
            .map_err(|error| error.to_string())?;
        let reply = reply
            .strip_prefix("root-drain-v1 ")
            .ok_or_else(|| reply.clone())?;
        let read: serde_json::Value = serde_json::from_str(reply).unwrap();
        Ok(read["owner_close_proof"]
            .is_object()
            .then(|| read["owner_close_proof"].clone()))
    }

    fn assert_no_close(&self, root_id: &str, when: &str) {
        if let Ok(Some(proof)) = self.owner_close(root_id) {
            panic!("Broker certified close {when}: {proof}");
        }
    }

    fn write_provider(&self) -> PathBuf {
        let config = self.path("config-home/oulipoly-agent-runner");
        fs::create_dir_all(config.join("models")).unwrap();
        let provider = self.path("provider.sh");
        // Every async dispatch returns at K, so all notify children are live
        // together before any is released. A sync sibling runs to its end.
        fs::write(
            &provider,
            br#"#!/bin/sh
export OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1="$AGE319_BASH_CONTROL_SOCKET"
i=0
while [ "$i" -lt "$AGE381_NOTIFY_CHILDREN" ]; do
  i=$((i + 1))
  status=0
  if [ "$i" = "$AGE381_FAILING_MEMBER" ]; then status=3; fi
  AGE381_MEMBER="$i" AGE381_STATUS="$status" "$AGE319_BASH_IMAGE" run --delivery async -- /bin/sh -c 'while [ ! -f "$AGE319_CHILD_RELEASE_FILE" ]; do sleep 0.02; done; printf "async-%s\n" "$AGE381_MEMBER" >> "$AGE319_EFFECT_FILE"; printf "async-child-out-%s\n" "$AGE381_MEMBER"; printf "async-child-err-%s\n" "$AGE381_MEMBER" >&2; exit "$AGE381_STATUS"' || exit
done
if [ "$AGE381_QUIET_SIBLING" = 1 ]; then
  "$AGE319_BASH_IMAGE" run --delivery sync -- /bin/sh -c 'printf "quiet\n" >> "$AGE319_EFFECT_FILE"; printf "quiet-out\n"' > /dev/null || exit
fi
if [ -n "$AGE319_PROVIDER_HOLD_FILE" ]; then
  while [ ! -f "$AGE319_PROVIDER_HOLD_FILE" ]; do sleep 0.02; done
fi
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
        provider
    }

    fn run(mut self, case: Case) {
        self.write_provider();
        let request_id = uuid::Uuid::new_v4().to_string();
        let out = self.path("launch.out");
        let err = self.path("launch.err");
        let effect = self.path("effect");
        let child_release = self.path("child-release");
        let provider_release = self.path("provider-release");
        let mut launch = Command::new(&self.launcher);
        launch
            .args(["--model", "fixture-model", "hello members"])
            .current_dir(self.temp.path())
            .env_clear()
            .env("HOME", self.temp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("OULIPOLY_CONFIG_HOME", self.path("config-home"))
            .env("OULIPOLY_DATA_DIR", self.path("data"))
            .env("AGE319_EFFECT_FILE", &effect)
            .env("AGE319_BASH_IMAGE", &self.bash)
            .env("AGE319_BASH_CONTROL_SOCKET", &self.socket)
            .env("AGE319_CHILD_RELEASE_FILE", &child_release)
            .env(
                "AGE319_PROVIDER_HOLD_FILE",
                match case.mode {
                    Mode::Busy => provider_release.to_str().unwrap(),
                    Mode::Sleeping => "",
                },
            )
            .env("AGE381_NOTIFY_CHILDREN", case.notify.to_string())
            .env(
                "AGE381_QUIET_SIBLING",
                if case.quiet_sibling { "1" } else { "0" },
            )
            .env(
                "AGE381_FAILING_MEMBER",
                if case.partial { "2" } else { "0" },
            )
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", &self.manifest)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1", &self.launcher)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &self.socket)
            .env("OULIPOLY_AGE319_PRIVATE_REQUEST_ID_V1", &request_id)
            .stdout(Stdio::from(File::create(&out).unwrap()))
            .stderr(Stdio::from(File::create(&err).unwrap()));
        if case.partial {
            launch.env("AGE381_TEST_FEATURELESS_STOP_AFTER_SECOND_RECEIPT_V1", "1");
        }
        let mut launch = launch.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(240);
        let diagnose = |fixture: &Self| {
            format!(
                "launch={} out={} broker={}",
                fs::read_to_string(&err).unwrap_or_default(),
                fs::read_to_string(&out).unwrap_or_default(),
                fixture.log()
            )
        };

        // Every notify C is admitted while none has a W: the children are
        // concurrently in flight under one root.
        let (root_id, notify_ids) = loop {
            let rows: Vec<(String, String, String)> = {
                let fresh = self.fresh();
                let mut statement = fresh
                    .prepare(
                        "SELECT c.root_id,c.request_id,l.listener_policy FROM fresh_bash_child c
                         JOIN fresh_bash_listener_registration l USING(request_id)",
                    )
                    .unwrap();
                statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .unwrap()
                    .map(Result::unwrap)
                    .collect()
            };
            let notify: BTreeSet<String> = rows
                .iter()
                .filter(|row| row.2 == "notify")
                .map(|row| row.1.clone())
                .collect();
            if notify.len() == case.notify {
                let roots: BTreeSet<&String> = rows.iter().map(|row| &row.0).collect();
                assert_eq!(roots.len(), 1, "children span roots: {rows:?}");
                break (rows[0].0.clone(), notify);
            }
            assert!(
                Instant::now() < deadline && launch.try_wait().unwrap().is_none(),
                "notify C absent: {}",
                diagnose(&self)
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let selected_count = |fixture: &Self| -> i64 {
            fixture
                .fresh()
                .query_row(
                    "SELECT count(*) FROM fresh_bash_selected_event e JOIN fresh_bash_child c USING(request_id) WHERE c.root_id=?1",
                    [&root_id],
                    |row| row.get(0),
                )
                .unwrap()
        };
        let expected_members = case.notify + usize::from(case.quiet_sibling);
        if case.quiet_sibling {
            while selected_count(&self) < 1 {
                assert!(
                    Instant::now() < deadline,
                    "quiet W absent: {}",
                    diagnose(&self)
                );
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert_eq!(
            selected_count(&self),
            i64::from(case.quiet_sibling),
            "a notify child reached W before its release"
        );
        // Admitted C without W stay retained custody before the freeze.
        {
            let lane = FreshV30Lane::open_at(&self.state).unwrap();
            let (root, actor) = lane.released_handoff_for_root(&root_id).unwrap();
            let session = lane.read_session(&root.d_key).unwrap().unwrap();
            let held = lane
                .settle_private_root_terminal(&root, &actor, &session)
                .unwrap();
            assert!(held.execution.is_none(), "terminal froze with live members");
            let unresolved: BTreeSet<String> =
                held.unresolved_child_request_ids.iter().cloned().collect();
            assert_eq!(unresolved, notify_ids);
            assert_ne!(held.refusal, None);
        }
        self.assert_no_close(&root_id, "while members are held");

        let parent_admission: String = self
            .fresh()
            .query_row(
                "SELECT json_extract(receipt_json,'$.parent_work_grant_id') FROM fresh_bash_child
                 WHERE root_id=?1 LIMIT 1",
                [&root_id],
                |row| row.get(0),
            )
            .unwrap();
        let physical = self
            .state
            .join("v30/normal-provider")
            .join(&parent_admission);
        match case.mode {
            Mode::Busy => {
                assert!(!physical.join("provider-exit.json").exists());
                fs::write(&child_release, b"release").unwrap();
                while selected_count(&self) < expected_members as i64 {
                    assert!(
                        Instant::now() < deadline,
                        "busy W absent: {}",
                        diagnose(&self)
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(
                    !physical.join("provider-exit.json").exists(),
                    "busy provider exited before every W"
                );
                fs::write(&provider_release, b"release").unwrap();
            }
            Mode::Sleeping => {
                while !physical.join("provider-exit.json").exists() {
                    assert!(
                        Instant::now() < deadline,
                        "provider did not exit: {}",
                        diagnose(&self)
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                fs::write(&child_release, b"release").unwrap();
            }
        }
        let status = loop {
            if let Some(status) = launch.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                launch.kill().unwrap();
                panic!(
                    "launcher timed out: {} terminal={:?}",
                    diagnose(&self),
                    self.terminal(&root_id)
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        };

        let events: Vec<FreshBashSourceEvent> = {
            let fresh = self.fresh();
            let mut statement = fresh
                .prepare(
                    "SELECT e.receipt_json FROM fresh_bash_selected_event e
                     JOIN fresh_bash_child c USING(request_id) WHERE c.root_id=?1",
                )
                .unwrap();
            statement
                .query_map([&root_id], |row| row.get::<_, String>(0))
                .unwrap()
                .map(|value| serde_json::from_str(&value.unwrap()).unwrap())
                .collect()
        };
        assert_eq!(events.len(), expected_members);
        for event in events
            .iter()
            .filter(|event| notify_ids.contains(&event.request_id))
        {
            let selection = event.normal_provider_selection.as_ref().unwrap();
            assert_eq!(
                selection.mode,
                match case.mode {
                    Mode::Busy => "busy",
                    Mode::Sleeping => "sleeping",
                }
            );
        }

        if case.partial {
            self.assert_partial(&root_id, &notify_ids, status.success(), &diagnose);
            return;
        }
        assert!(
            status.success(),
            "launcher failed: {} terminal={:?}",
            diagnose(&self),
            self.terminal(&root_id)
        );
        let mut expected_effect: Vec<String> =
            (1..=case.notify).map(|i| format!("async-{i}")).collect();
        if case.quiet_sibling {
            expected_effect.push("quiet".into());
        }
        let mut effect_lines: Vec<String> = fs::read_to_string(&effect)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        effect_lines.sort();
        expected_effect.sort();
        assert_eq!(effect_lines, expected_effect, "each child ran exactly once");

        let terminal = self.terminal(&root_id);
        assert_eq!(terminal.execution_state, "success");
        assert_eq!(terminal.notification_state, "acked", "{terminal:?}");
        assert_eq!(terminal.refusal, None, "{terminal:?}");
        assert!(terminal.unresolved_child_request_ids.is_empty());
        let members = self.assert_members_settled(&terminal, &notify_ids, case);
        let ledger = InstalledLaunchLedger::open(
            &self.state,
            &self.pair.generation,
            &self.source_generation,
        )
        .unwrap();
        let certificate = ledger.read_terminal(&request_id).unwrap().unwrap();
        assert_eq!(certificate.exit_code, 0);
        let proof: serde_json::Value =
            serde_json::from_str(&certificate.owner.physical_proof_json).unwrap();
        if members.len() >= 2 {
            // The accepted owner close names every member and its own
            // settlement, not one scalar ACK standing for the set.
            let named: BTreeSet<String> = proof["child_members"]
                .as_array()
                .unwrap_or_else(|| panic!("owner close proof lacks members: {proof}"))
                .iter()
                .map(|member| member["request_id"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(named, members, "{proof}");
        }
        assert!(self.owner_close(&root_id).unwrap().is_some());

        if case.late_after_close {
            self.assert_late_c_refused(&root_id, &events[0]);
        }
    }

    /// Each notify member has its own row, its own grant or successor, its
    /// own retained receipt of exactly its own W bytes, and its own ACK.
    fn assert_members_settled(
        &self,
        terminal: &FreshRootTerminalReadback,
        notify_ids: &BTreeSet<String>,
        case: Case,
    ) -> BTreeSet<String> {
        let expected_basis = match case.mode {
            Mode::Busy => "manual_ack",
            Mode::Sleeping => "successor_receiver_receipt_ack",
        };
        let members: Vec<(String, Option<i64>, String, Option<String>)> =
            if terminal.children.is_empty() {
                vec![(
                    terminal.child_request_id.clone().unwrap(),
                    terminal.mailbox_seq,
                    terminal.notification_state.clone(),
                    terminal.ack_basis.clone(),
                )]
            } else {
                assert_eq!(terminal.notification_origin, "child_set");
                terminal
                    .children
                    .iter()
                    .map(|child| {
                        (
                            child.request_id.clone(),
                            child.mailbox_seq,
                            child.notification_state.clone(),
                            child.ack_basis.clone(),
                        )
                    })
                    .collect()
            };
        let lane = FreshV30Lane::open_at(&self.state).unwrap();
        let (root, _) = lane.released_handoff_for_root(&terminal.root_id).unwrap();
        let session = lane.read_session(&root.d_key).unwrap().unwrap();
        let sidecar = self.sidecar();
        let mut seqs = BTreeSet::new();
        let mut proofs = BTreeSet::new();
        for (request_id, seq, state, basis) in &members {
            if !notify_ids.contains(request_id) {
                assert_eq!(state, "response_only");
                continue;
            }
            assert_eq!(state, "acked", "member {request_id}: {terminal:?}");
            assert_eq!(basis.as_deref(), Some(expected_basis));
            let seq = seq.unwrap();
            assert!(seqs.insert(seq), "two members share mailbox row {seq}");
            let retained = lane
                .lookup_payload(&session.lane_id, &session.session_id, seq)
                .unwrap();
            let payload: serde_json::Value = serde_json::from_slice(&retained).unwrap();
            assert_eq!(payload["source"]["request_id"], request_id.as_str());
            let (proof, received) = match case.mode {
                Mode::Busy => {
                    let (grant_id, phase, uid, receipt_sha): (String, String, u32, String) =
                        sidecar
                            .query_row(
                                "SELECT g.grant_id,g.phase,c.recipient_uid,c.receipt_sha256
                                 FROM fresh_recipient_grant g
                                 JOIN fresh_original_receipt c ON c.grant_id=g.grant_id
                                 JOIN fresh_recipient_ack_evidence e ON e.grant_id=g.grant_id
                                 WHERE g.session_id=?1 AND g.seq=?2",
                                rusqlite::params![session.session_id, seq],
                                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                            )
                            .unwrap();
                    assert_eq!(phase, "acked");
                    let path = lane.original_receipt_path(&grant_id, uid).unwrap();
                    assert_eq!(digest(&path), receipt_sha);
                    let receipt: serde_json::Value =
                        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                    assert_eq!(receipt["grant"]["seq"], seq);
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(receipt["payload_base64"].as_str().unwrap())
                        .unwrap();
                    (grant_id, bytes)
                }
                Mode::Sleeping => {
                    let (generation, receipt_sha): (String, String) = sidecar
                        .query_row(
                            "SELECT e.generation,e.receipt_sha256 FROM fresh_successor_ack_evidence e
                             JOIN fresh_successor_grant g ON g.grant_id=e.grant_id
                             WHERE e.session_id=?1 AND e.seq=?2 AND g.phase='acked'",
                            rusqlite::params![session.session_id, seq],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .unwrap();
                    (format!("{generation}:{receipt_sha}"), retained.clone())
                }
            };
            assert_eq!(
                received, retained,
                "member {request_id} received other bytes"
            );
            assert!(proofs.insert(proof), "two members share one delivery");
        }
        assert_eq!(seqs.len(), notify_ids.len());
        members.into_iter().map(|member| member.0).collect()
    }

    fn assert_late_c_refused(&self, root_id: &str, first: &FreshBashSourceEvent) {
        let mut lane = FreshV30Lane::open_at(&self.state).unwrap();
        let (released, actor) = lane.released_handoff_for_root(root_id).unwrap();
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
            FreshBashListenerPolicy::Notify,
        )
        .unwrap();
        let read = self.terminal(root_id);
        assert_eq!(read.unresolved_child_request_ids, vec![late_id.clone()]);
        assert_ne!(read.refusal, None);
        // State's refusal alone is not enough: the Broker re-asked for the
        // same accepted close must not still certify it.
        match self.owner_close(root_id) {
            Ok(None) | Err(_) => {}
            Ok(Some(proof)) => {
                panic!("Broker still certifies close after late C {late_id}: {proof}")
            }
        }
    }

    fn assert_partial(
        &mut self,
        root_id: &str,
        notify_ids: &BTreeSet<String>,
        launcher_succeeded: bool,
        diagnose: &dyn Fn(&Self) -> String,
    ) {
        assert!(!launcher_succeeded, "partial delivery reported success");
        let lane = FreshV30Lane::open_at(&self.state).unwrap();
        let (root, actor) = lane.released_handoff_for_root(root_id).unwrap();
        let session = lane.read_session(&root.d_key).unwrap().unwrap();
        // Record the complete physical work; the parent Q exists.
        lane.settle_private_root_terminal(&root, &actor, &session)
            .unwrap();
        for restarted in [false, true] {
            if restarted {
                self.restart_broker();
            }
            let terminal = self.terminal(root_id);
            assert_eq!(terminal.execution_state, "failure", "{terminal:?}");
            assert_eq!(terminal.notification_state, "child_set_pending");
            let states: Vec<&str> = terminal
                .children
                .iter()
                .filter(|child| notify_ids.contains(&child.request_id))
                .map(|child| child.notification_state.as_str())
                .collect();
            assert_eq!(
                states.iter().filter(|state| **state == "acked").count(),
                1,
                "exactly one member ACKed: {states:?} {}",
                diagnose(self)
            );
            // The second member's receipt is certified, but receipt alone
            // never counts as ACK.
            let receipted_unacked: i64 = self
                .sidecar()
                .query_row(
                    "SELECT count(*) FROM fresh_original_receipt c
                     JOIN fresh_recipient_grant g ON g.grant_id=c.grant_id
                     WHERE g.phase!='acked'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(receipted_unacked, 1, "{states:?}");
            // Physical failure with notification pending is not settled.
            assert_ne!(terminal.refusal, None, "{terminal:?}");
            assert!(
                lane.begin_private_root_publication(&root, &actor, &session, b"partial")
                    .is_err(),
                "State accepted publication with a pending member"
            );
            self.assert_no_close(
                root_id,
                &format!("with a pending member (restarted={restarted})"),
            );
        }
    }
}
