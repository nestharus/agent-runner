//! Native runner + explicitly supplied Bash candidate, never a fake enqueue helper.
//! Each case re-execs in private user/network/PID/mount namespaces. PID namespace
//! teardown contains failure descendants; product NoDeadline is not modified.
#![cfg(target_os = "linux")]
#[path = "fixtures/bounded_runner_image.rs"]
mod bounded_runner_image;
#[cfg(feature = "age360-fault-fixtures")]
#[path = "fixtures/age360/custody_faults.rs"]
mod custody_faults;
#[path = "fixtures/age360/live_census.rs"]
mod live_census;
#[cfg(feature = "age360-fault-fixtures")]
#[path = "fixtures/age360/paired_faults.rs"]
mod paired_faults;
mod provider_authority_fixture;
#[path = "fixtures/age360/rejection.rs"]
mod rejection;
use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::pid_identity::read_live_process_identity;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
const SESSION: &str = "ses_age360_native_wake";
const MODEL: &str = "age360-native-model";
const PROVIDER: &str = "age360-native-provider";

fn runner() -> PathBuf {
    PathBuf::from(bounded_runner_image::runner_bin())
}

fn counterpart() -> PathBuf {
    let requested=std::env::var_os("AGE360_AGENT_BASH_BIN").expect("AGE360_AGENT_BASH_BIN must identify the exact built counterpart; no installed/fake fallback");
    let path = fs::canonicalize(requested).expect("counterpart must exist");
    assert!(
        path.is_file() && fs::metadata(&path).unwrap().permissions().mode() & 0o111 != 0,
        "counterpart must be executable"
    );
    path
}
fn private_case(paired: bool) -> bool {
    let name = std::thread::current().name().unwrap().to_owned();
    let net = fs::read_link("/proc/self/ns/net").unwrap();
    let bash = paired.then(counterpart);
    let runner = fs::canonicalize(runner()).expect("exact runner binary must exist");
    if std::env::var("AGE360_PRIVATE_CASE").as_deref() == Ok(&name) {
        assert_ne!(
            net.as_os_str(),
            std::env::var_os("AGE360_PARENT_NET").unwrap()
        );
        println!(
            "private case={name} net={} pid={}",
            net.display(),
            std::process::id()
        );
        println!(
            "runner={} sha256={}",
            runner.display(),
            oulipoly_state::completion_continuation::sha256(&fs::read(&runner).unwrap())
        );
        if let Some(bash) = &bash {
            println!(
                "bash={} sha256={}",
                bash.display(),
                oulipoly_state::completion_continuation::sha256(&fs::read(bash).unwrap())
            );
        }
        return false;
    }
    let homes = tempfile::tempdir().unwrap();
    let mut command = Command::new("timeout");
    command
        .args([
            "--kill-after=5s",
            "180s",
            "unshare",
            "--user",
            "--map-current-user",
            "--net",
            "--pid",
            "--fork",
            "--mount-proc",
            "--kill-child=KILL",
            "--",
        ])
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", homes.path())
        .env("XDG_CONFIG_HOME", homes.path().join("config"))
        .env("XDG_STATE_HOME", homes.path().join("state"))
        .env("AGE360_PRIVATE_CASE", &name)
        .env("AGE360_PARENT_NET", &net)
        .env("AGE360_RUNNER_BIN", runner);
    if let Some(tmpdir) = std::env::var_os("TMPDIR") {
        command.env("TMPDIR", tmpdir);
    }
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    if let Some(bash) = bash {
        command.env("AGE360_AGENT_BASH_BIN", bash);
    }
    let output = command.output().expect("private namespace execution");
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "private case={name} status={}\nstdout={}\nstderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}
struct Fixture {
    root: tempfile::TempDir,
    data: PathBuf,
    models: PathBuf,
    case: &'static str,
}
impl Fixture {
    fn new(case: &'static str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let app = config.join("oulipoly-agent-runner");
        let models = app.join("models");
        let data = root.path().join("data");
        fs::create_dir_all(&models).unwrap();
        fs::create_dir_all(&data).unwrap();
        fs::create_dir_all(root.path().join("home")).unwrap();
        let script = root.path().join("provider.py");
        fs::write(&script, include_str!("fixtures/age360/native-provider.py")).unwrap();
        fs::write(
            root.path().join("native-missing-output.py"),
            include_str!("fixtures/age360/native-missing-output.py"),
        )
        .unwrap();
        fs::write(
            root.path().join("missing-output-wire.json"),
            include_str!(
                "../../crates/oulipoly-state/tests/fixtures/age360-missing-output-wire.json"
            ),
        )
        .unwrap();
        let wrapper = root.path().join("provider.sh");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexec /usr/bin/python3 '{}' \"$@\"\n",
                script.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            models.join(format!("{MODEL}.toml")),
            format!("prompt_mode = \"arg\"\n[[providers]]\nname=\"{PROVIDER}\"\nargs=[]\n"),
        )
        .unwrap();
        fs::write(app.join("providers.toml"),provider_authority_fixture::with_explicit_provider_authority_at(&format!("[{PROVIDER}]\ncommand=\"age360-native-fixture\"\nargs=[]\nprompt_mode=\"arg\"\nsettings_id=\"{PROVIDER}\"\n"),"age360-native-wake",&wrapper)).unwrap();
        Self {
            root,
            data,
            models,
            case,
        }
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(runner());
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("OULIPOLY_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_STATE_HOME", self.root.path().join("spool"))
            .env("XDG_DATA_HOME", &self.data)
            .env("OULIPOLY_DATA_DIR", &self.data)
            .env(
                "AGE360_DESCENDANT",
                if self.root.path().join("test-descendant-enabled").exists() {
                    "1"
                } else {
                    "0"
                },
            )
            .env(
                "AGE360_DESCENDANT_GATE",
                self.root.path().join("release-descendant"),
            )
            .env("AGE360_ROOT", self.root.path())
            .env("AGE360_CASE", self.case)
            .env(
                "AGE360_NATIVE_WAKE_MARKER",
                self.root.path().join("resume-prompts.jsonl"),
            )
            .env(
                "AGE360_NATIVE_WAKE_GATE",
                self.root.path().join("release-resume"),
            )
            .env(
                "AGE360_WORKLOAD_GATE",
                self.root.path().join("release-workload"),
            )
            .env("AGENT_BASH_AGENT_RUNNER_BIN", runner())
            .env("AGE360_RUNNER_BIN", runner())
            .current_dir(self.root.path());
        if let Some(tmpdir) = std::env::var_os("TMPDIR") {
            cmd.env("TMPDIR", tmpdir);
        }
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            cmd.env("LLVM_PROFILE_FILE", profile);
        }
        #[cfg(feature = "age360-fault-fixtures")]
        cmd.env("AGE360_FAULT_ROOT", self.root.path()).env(
            "AGE360_FAULT_PARENT_NET",
            std::env::var_os("AGE360_PARENT_NET").unwrap(),
        );
        #[cfg(feature = "age360-fault-fixtures")]
        if self.root.path().join("wal-sync-hold.so").exists() {
            cmd.env("LD_PRELOAD", self.root.path().join("wal-sync-hold.so"))
                .env("AGE360_WAL_ROOT", self.root.path());
        }
        #[cfg(feature = "age360-fault-fixtures")]
        if self.root.path().join("identity-read-fail.so").exists() {
            cmd.env("LD_PRELOAD", self.root.path().join("identity-read-fail.so"))
                .env("AGE360_IDENTITY_ROOT", self.root.path());
        }
        let fault = match self.case {
            "publication_race" => Some("after-terminal-metadata"),
            "publication_error" | "publication_io_error" => Some("publication-error"),
            "hash_cancel" => Some("during-output-hash"),
            "missing_selection" => Some("selection-error"),
            "missing_pin" | "missing_short" => Some("before-output-capture"),
            _ => None,
        };
        if let Some(fault) = fault {
            cmd.env("AGENT_BASH_SOURCE_FAULT", fault);
        }
        if !matches!(self.case, "owner_only" | "native_missing") {
            cmd.env("AGE360_AGENT_BASH_BIN", counterpart());
        }
        cmd
    }
    fn start(&self) -> Child {
        self.start_with_hold(false)
    }
    fn start_with_hold(&self, hold: bool) -> Child {
        self.command()
            .env("AGE360_HOLD_INITIAL", if hold { "1" } else { "0" })
            .args(["-m", MODEL, "--models-dir"])
            .arg(&self.models)
            .arg("establish native completion session")
            .stdout(fs::File::create(self.root.path().join("native.stdout")).unwrap())
            .stderr(fs::File::create(self.root.path().join("native.stderr")).unwrap())
            .stdin(Stdio::null())
            .spawn()
            .unwrap()
    }
    fn sidecar_connection(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.data.join("pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }
    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open_historical_read_only(&self.data.join("pid-identity.db")).unwrap()
    }
    fn mailbox_for_poll(&self) -> Option<MailboxDb> {
        match MailboxDb::open_historical_read_only(&self.data.join("pid-identity.db")) {
            Ok(mailbox) => Some(mailbox),
            Err(error)
                if error.contains(
                    "SQLite source continued changing while retrying a read-only snapshot",
                ) =>
            {
                None
            }
            Err(error) => panic!("mailbox snapshot failed outside source churn: {error}"),
        }
    }
    fn gate(&self, name: &str) {
        fs::write(self.root.path().join(name), b"release\n").unwrap();
    }
    fn wait_initial(&self, child: &mut Child) {
        let status = wait(|| child.try_wait().unwrap());
        assert!(
            status.success(),
            "native initial status={status}\n{}",
            fs::read_to_string(self.root.path().join("native.stderr")).unwrap()
        );
    }
    fn owner(&self) -> oulipoly_state::mailbox::CompletionDomainOwner {
        wait(|| {
            MailboxDb::open_historical_read_only(&self.data.join("pid-identity.db"))
                .ok()?
                .completion_continuation_owner()
                .ok()?
        })
    }
    fn source(&self) -> oulipoly_state::completion_continuation::AdmittedSourceBinding {
        wait(|| {
            oulipoly_state::StateDb::open_historical_read_only(&self.data.join("state.db"))
                .ok()?
                .admitted_completion_continuations()
                .ok()?
                .into_iter()
                .next()
        })
    }
}
#[track_caller]
fn wait<T>(mut read: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(value) = read() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "bounded fixture observation expired, not a product timeout/pass"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn current_identity_matches(
    identity: &oulipoly_state::completion_continuation::SourceProcessIdentity,
) -> bool {
    read_live_process_identity(identity.pid)
        .ok()
        .flatten()
        .is_some_and(|live| {
            live.os_boot_id == identity.boot_id
                && live.os_pid_starttime_ticks == identity.starttime_ticks
        })
}
fn parent(pid: i64) -> i64 {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .unwrap()
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}
#[test]
fn native_owner_election_and_idle_retirement() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    let mut child = f.start();
    let owner = f.owner();
    assert_eq!(
        parent(owner.driver_identity.pid),
        owner.guardian_identity.pid
    );
    assert_ne!(owner.guardian_identity.pid, i64::from(child.id()));
    println!("native owner={}", serde_json::to_string(&owner).unwrap());
    f.wait_initial(&mut child);
    println!("initial native runner exited; waiting exact guardian reap");
    wait(|| {
        if !current_identity_matches(&owner.guardian_identity) {
            return Some(());
        }
        // This test is namespace PID1, so exited CG is our adopted child. A
        // zombie's /proc identity persists until actual wait, not owner liveness.
        let mut status = 0;
        let reaped = unsafe {
            libc::waitpid(
                owner.guardian_identity.pid as i32,
                &mut status,
                libc::WNOHANG,
            )
        };
        (reaped == owner.guardian_identity.pid as i32).then_some(())
    });
    assert!(
        f.mailbox()
            .completion_continuation_owner()
            .unwrap()
            .is_none()
    );
}
#[test]
fn native_dead_inherited_endpoint_rejects_admission_but_preserves_mailbox_read() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    let mut initial = f.start_with_hold(true);
    let owner = f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    let count = || {
        let db = rusqlite::Connection::open_with_flags(
            f.data.join("state.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        db.query_row("SELECT COUNT(*) FROM invocations", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    let before = count();
    let invalid = f.root.path().join("dead-owner.sock");
    let output = f
        .command()
        .env("OULIPOLY_COMPLETION_ENDPOINT", &invalid)
        .args(["-m", MODEL, "must not launch"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        count(),
        before,
        "unavailable inherited owner must fail before invocation admission"
    );
    let read = f
        .command()
        .env("OULIPOLY_COMPLETION_ENDPOINT", &invalid)
        .args(["mailbox", "list", "--session-id", SESSION, "--json"])
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    let wake = f
        .command()
        .env("OULIPOLY_COMPLETION_ENDPOINT", &invalid)
        .args(["mailbox", "resume", "--session-id", SESSION, "--json"])
        .output()
        .unwrap();
    assert!(
        !wake.status.success(),
        "a persisted owner row cannot substitute for a working endpoint"
    );
    assert_eq!(
        count(),
        before,
        "failed service join must not admit a recipient"
    );
    assert_eq!(f.owner().owner_generation, owner.owner_generation);
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
}
#[test]
fn native_unmarked_nested_entry_cannot_elect_descendant_owner() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("nested-probe-enabled");
    let mut initial = f.start();
    f.wait_initial(&mut initial);
    let rejection: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.path().join("nested-rejection.json")).unwrap())
            .unwrap();
    assert_ne!(rejection["rc"], 0);
    assert!(
        rejection["stderr"].as_str().unwrap().contains("ancestor"),
        "{rejection}"
    );
    assert!(
        !rejection["stderr"]
            .as_str()
            .unwrap()
            .contains("absent-nested-model"),
        "ancestry rejection must precede model resolution: {rejection}"
    );
}

fn paired_case(mode: &'static str) {
    let f = Fixture::new(mode);
    if matches!(mode, "ack_before_drain" | "manual_overlap") {
        f.gate("test-descendant-enabled");
    }
    #[cfg(feature = "age360-fault-fixtures")]
    if mode == "manual_overlap" {
        f.gate("wake-child-before-claim-admission.hold");
        f.gate("manual-after-claim-coordination-NativeBusy.hold");
    }
    #[cfg(feature = "age360-fault-fixtures")]
    paired_faults::prepare(&f);
    f.gate("release-resume");
    let mut initial = f.start();
    let owner = f.owner();
    let binding = f.source();
    let source = binding.registration().unwrap();
    assert_eq!(source.domain_id, owner.domain_id);
    assert_eq!(source.owner_session_id, SESSION);
    #[cfg(feature = "age360-fault-fixtures")]
    paired_faults::after_registration(&f);
    println!(
        "owner={} source={}",
        serde_json::to_string(&owner).unwrap(),
        serde_json::to_string(&source).unwrap()
    );
    if mode == "pause" {
        let output = f
            .command()
            .args(["mailbox", "pause", "--session-id", SESSION])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if mode == "driver_replacement" {
        assert!(current_identity_matches(&owner.driver_identity));
        assert_eq!(
            unsafe { libc::kill(owner.driver_identity.pid as i32, libc::SIGKILL) },
            0
        );
        let successor = wait(|| {
            let current = f.mailbox().completion_continuation_owner().ok()??;
            (current.owner_generation != owner.owner_generation).then_some(current)
        });
        assert_eq!(successor.guardian_identity, owner.guardian_identity);
        println!(
            "paired original guardian replaced driver={}",
            serde_json::to_string(&successor).unwrap()
        );
    }
    if mode != "sync" {
        // Original physical workload fencing is intact: the native launcher
        // cannot finish while this fixture deliberately holds its Bash tree.
        f.gate("release-workload");
    }
    #[cfg(feature = "age360-fault-fixtures")]
    paired_faults::before_acceptance(&f, &source);
    if !matches!(mode, "sync" | "acceptance_reply_loss") {
        f.wait_initial(&mut initial);
    }
    let acceptance = wait(|| {
        let value = f
            .mailbox_for_poll()?
            .completion_continuation_acceptance(&source.registration_id)
            .ok()??;
        (value["phase"] == "accepted").then_some(value)
    });
    println!("acceptance={acceptance}");
    #[cfg(feature = "age360-fault-fixtures")]
    paired_faults::after_acceptance(&f);
    let mut manual_child = None;
    if mode == "manual_overlap" {
        let child_pid: i64 = wait(|| {
            fs::read_to_string(
                f.root
                    .path()
                    .join("wake-child-before-claim-admission.reached"),
            )
            .ok()?
            .parse()
            .ok()
        });
        let child_identity = read_live_process_identity(child_pid).unwrap().unwrap();
        let (claim, attempt) = wait(|| {
            let mailbox = f.mailbox_for_poll()?;
            let claim = mailbox.wake_session_reader().wake_claim(SESSION).ok()??;
            let attempt = mailbox
                .continuation_activation(SESSION, &claim.claim_token)
                .ok()??;
            Some((claim, attempt))
        });
        let (source_id, phase): (Option<String>, String) = f.sidecar_connection()
            .query_row(
                "SELECT source_registration_id,phase FROM completion_continuation_attempt WHERE attempt_id=?1 AND claim_token=?2",
                rusqlite::params![attempt.attempt_id, claim.claim_token],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).unwrap();
        assert_eq!(source_id.as_deref(), Some(source.registration_id.as_str()));
        assert!(matches!(phase.as_str(), "starting" | "running"));
        assert!(
            f.mailbox()
                .completion_event_listeners(&source.handle)
                .unwrap()
                .iter()
                .all(|listener| listener.acknowledged_at.is_none())
        );
        assert!(!f.root.path().join("resume-prompts.jsonl").exists());
        // The original native turn can leave a committed tail for explicit
        // recovery. Settle it through the real CLI before testing manual
        // coordination; otherwise State refuses the resume before the sidecar
        // claim boundary and this case would only exercise that refusal.
        let duties = oulipoly_state::StateDb::open_historical_read_only(&f.data.join("state.db"))
            .unwrap()
            .completed_turn_identities()
            .unwrap();
        for duty in &duties {
            let settled = f
                .command()
                .args([
                    "completed-turn",
                    "--invocation",
                    &duty.invocation_uuid,
                    "--settle",
                ])
                .output()
                .unwrap();
            assert!(
                settled.status.success(),
                "completed turn settlement failed: {}",
                String::from_utf8_lossy(&settled.stderr)
            );
        }
        assert!(
            oulipoly_state::StateDb::open_historical_read_only(&f.data.join("state.db"))
                .unwrap()
                .completed_turn_identities()
                .unwrap()
                .is_empty()
        );
        let manual = f
            .command()
            .args([
                "resume",
                "-m",
                MODEL,
                "--session-id",
                SESSION,
                "--models-dir",
            ])
            .arg(&f.models)
            .stdout(fs::File::create(f.root.path().join("manual.stdout")).unwrap())
            .stderr(fs::File::create(f.root.path().join("manual.stderr")).unwrap())
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        manual_child = Some(manual);
        wait(|| {
            f.root
                .path()
                .join("manual-after-claim-coordination-NativeBusy.reached")
                .exists()
                .then_some(())
        });
        assert_eq!(
            read_live_process_identity(child_pid).unwrap(),
            Some(child_identity)
        );
        let retained = f
            .mailbox()
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .unwrap();
        assert_eq!(retained.claim_token, claim.claim_token);
        let retained_attempt = f
            .mailbox()
            .continuation_activation(SESSION, &claim.claim_token)
            .unwrap();
        assert_eq!(retained_attempt.unwrap().attempt_id, attempt.attempt_id);
        let retained_source: Option<String> = f.sidecar_connection()
            .query_row("SELECT source_registration_id FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt.attempt_id], |row| row.get(0)).unwrap();
        assert_eq!(
            retained_source.as_deref(),
            Some(source.registration_id.as_str())
        );
        assert!(
            f.mailbox()
                .completion_event_listeners(&source.handle)
                .unwrap()
                .iter()
                .all(|listener| listener.acknowledged_at.is_none())
        );
        assert!(!f.root.path().join("resume-prompts.jsonl").exists());
        println!(
            "manual coordination overlapped unadmitted child={child_pid} exact claim={} attempt={} source={}",
            claim.claim_token, attempt.attempt_id, source.registration_id
        );
        fs::remove_file(f.root.path().join("wake-child-before-claim-admission.hold")).unwrap();
        fs::remove_file(
            f.root
                .path()
                .join("manual-after-claim-coordination-NativeBusy.hold"),
        )
        .unwrap();
    }
    if mode == "acceptance_reply_loss" {
        f.wait_initial(&mut initial);
    }
    if mode != "sync" {
        let dispatch: serde_json::Value = wait(|| {
            serde_json::from_slice(&fs::read(f.root.path().join("bash-dispatch.stdout")).ok()?).ok()
        });
        let expected = if mode == "acceptance_reply_loss" {
            "effects-possible-no-replay"
        } else {
            "root-accepted"
        };
        assert_eq!(dispatch["dispatch_state"], expected, "{dispatch}");
        assert_eq!(dispatch["retry_safe"], false, "{dispatch}");
        assert_eq!(dispatch["effects_possible"], true, "{dispatch}");
        assert_eq!(dispatch["handle"], source.handle, "{dispatch}");
        assert!(
            PathBuf::from(&source.handle_dir)
                .join("root-work-accepted-v1.json")
                .exists(),
            "paired test must prove root-owned acceptance, not merely source registration"
        );
    }
    if mode == "pause" {
        let listeners = f
            .mailbox()
            .completion_event_listeners(&source.handle)
            .unwrap();
        assert!(!listeners.is_empty());
        assert!(listeners.iter().all(|l| l.acknowledged_at.is_none()));
        assert!(!f.root.path().join("recipient-byte-receipt.json").exists());
        let output = f
            .command()
            .args(["mailbox", "resume", "--session-id", SESSION])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        println!("actual paused listener retained then resumed");
    }
    if matches!(
        mode,
        "publication_race" | "publication_error" | "publication_io_error" | "hash_cancel"
    ) {
        let evidence =
            oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                &binding,
            )
            .unwrap();
        assert_eq!(
            evidence.outcome.kind,
            if mode == "publication_race" {
                "exit_root"
            } else {
                "ready"
            }
        );
        println!(
            "immutable original outcome after cancellation={}",
            serde_json::to_string(&evidence.outcome).unwrap()
        );
    }
    if matches!(mode, "missing_selection" | "missing_pin" | "missing_short") {
        let evidence =
            oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                &binding,
            )
            .unwrap();
        assert!(evidence.original_output_missing());
        assert_eq!(evidence.outcome.kind, "ready");
        assert_eq!(evidence.outcome.root_wait_status, None);
        assert_eq!(
            evidence.outcome.ready_sentinel.as_deref(),
            Some("paired-source-output")
        );
        assert!(!evidence.outcome.original_tree_drained);
        let receipt: serde_json::Value = wait(|| {
            serde_json::from_slice(
                &fs::read(f.root.path().join("recipient-missing-output-receipt.json")).ok()?,
            )
            .ok()
        });
        assert_eq!(
            receipt["snapshot"]["output"]["representation"],
            "missing-original-output-v1"
        );
        assert_eq!(
            receipt["outcome"],
            serde_json::to_value(&evidence.outcome).unwrap()
        );
        println!("paired missing-original-output receipt={receipt}");
    }
    if mode == "registration_reply_loss" {
        let evidence =
            oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                &binding,
            )
            .unwrap();
        assert_eq!(evidence.outcome.kind, "never_launched");
        assert!(
            !PathBuf::from(&source.handle_dir)
                .join("registration-receipt-v2.json")
                .exists()
        );
        println!("exact committed registration recovered without original workload launch");
    }
    if mode == "early_exit" {
        let evidence =
            oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                &binding,
            )
            .unwrap();
        assert_eq!(source.completion_kind, "ready");
        assert_eq!(evidence.outcome.kind, "exit_tree");
        assert_eq!(evidence.snapshot.rc, 37);
        assert_eq!(evidence.outcome.root_wait_status, Some(37 << 8));
        assert!(evidence.outcome.ready_sentinel.is_none());
    }
    if mode == "large_output" {
        let evidence =
            oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                &binding,
            )
            .unwrap();
        let oulipoly_state::completion_continuation::CompletionOutput::Artifact(artifact) =
            evidence.snapshot.output
        else {
            panic!("large output was not an explicit full artifact");
        };
        assert_eq!(artifact.byte_len, 16 * 1024 * 1024);
        let copy = f
            .data
            .join("completion-continuation")
            .join(&source.domain_id)
            .join("outputs")
            .join(&artifact.sha256);
        assert_eq!(fs::metadata(&copy).unwrap().len(), artifact.byte_len);
        assert_eq!(
            oulipoly_state::completion_continuation::sha256(&fs::read(&copy).unwrap()),
            artifact.sha256
        );
    }

    if mode == "sync" {
        wait(|| {
            f.root
                .path()
                .join("provider-dispatched")
                .exists()
                .then_some(())
        });
        let listeners = f
            .mailbox()
            .completion_event_listeners(&source.handle)
            .unwrap();
        assert_eq!(listeners.len(), 1);
        assert!(!listeners[0].active);
        assert!(listeners[0].mailbox_seq.is_none());
        assert!(
            listeners[0].acknowledged_at.is_none(),
            "local synchronous bytes are not mailbox ACK"
        );
        let output: serde_json::Value = wait(|| {
            serde_json::from_slice(&fs::read(f.root.path().join("sync-byte-response.json")).ok()?)
                .ok()
        });
        assert_eq!(output["output"], "paired-source-output");
        assert_eq!(output["receipt"]["remote_ack"], "unconfirmed");
        assert_eq!(output["receipt"]["physical_drain"], "unconfirmed");
        f.gate("release-initial-provider");
        f.wait_initial(&mut initial);
        let dispatch: serde_json::Value =
            serde_json::from_slice(&fs::read(f.root.path().join("bash-dispatch.stdout")).unwrap())
                .unwrap();
        assert_eq!(dispatch["dispatch_state"], "root-accepted", "{dispatch}");
        assert_eq!(dispatch["retry_safe"], false, "{dispatch}");
        assert_eq!(dispatch["effects_possible"], true, "{dispatch}");
        assert_eq!(dispatch["handle"], source.handle, "{dispatch}");
        wait(|| {
            f.mailbox()
                .pending_continuation_attempt_ids(&source.registration_id)
                .ok()?
                .is_empty()
                .then_some(())
        });
        let listeners = f
            .mailbox()
            .completion_event_listeners(&source.handle)
            .unwrap();
        assert!(!listeners[0].active);
        assert!(listeners[0].mailbox_seq.is_none());
        assert!(listeners[0].acknowledged_at.is_none());
        assert!(listeners[0].acknowledgement_reason.is_none());
        assert!(f.mailbox().list_pending(SESSION).unwrap().is_empty());
        assert!(!f.root.path().join("recipient-byte-receipt.json").exists());
        assert!(!f.root.path().join("resume-prompts.jsonl").exists());
        let evidence =
            oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                &binding,
            )
            .unwrap();
        assert!(evidence.outcome.original_tree_drained);
        assert_eq!(
            fs::read_to_string(f.root.path().join("source-launches"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        // The selected small body is independently recoverable after the
        // response and Bash source directory are unavailable. Response-only
        // policy still has no mailbox row and no ACK.
        let lost_source = f.root.path().join("removed-bash-source");
        fs::rename(&source.handle_dir, &lost_source).unwrap();
        let listed = f
            .command()
            .args([
                "notify",
                "agent-bash-recovery-list",
                "--session-id",
                SESSION,
            ])
            .output()
            .unwrap();
        assert!(
            listed.status.success(),
            "{}",
            String::from_utf8_lossy(&listed.stderr)
        );
        let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
        assert!(
            listed["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["event_id"] == source.handle)
        );
        let all = f
            .command()
            .args(["notify", "agent-bash-recovery-list"])
            .output()
            .unwrap();
        assert!(
            all.status.success(),
            "{}",
            String::from_utf8_lossy(&all.stderr)
        );
        let all: serde_json::Value = serde_json::from_slice(&all.stdout).unwrap();
        assert!(
            all["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["event_id"] == source.handle)
        );
        let recovered = f.root.path().join("manual-recovered.bin");
        let readback = f
            .command()
            .args([
                "notify",
                "agent-bash-recovery-read",
                "--event-id",
                &source.handle,
                "--output",
                recovered.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            readback.status.success(),
            "{}",
            String::from_utf8_lossy(&readback.stderr)
        );
        let readback: serde_json::Value = serde_json::from_slice(&readback.stdout).unwrap();
        assert_eq!(readback["selected_output"]["kind"], "raw_bytes");
        assert_eq!(readback["physical_drain"]["attempt_search_complete"], false);
        let attempt_cursor = readback["physical_drain"]["next_attempt_cursor"]
            .as_str()
            .unwrap();
        let attempt_page = f
            .command()
            .args([
                "notify",
                "agent-bash-recovery-attempts",
                "--event-id",
                &source.handle,
                "--cursor",
                attempt_cursor,
            ])
            .output()
            .unwrap();
        assert!(
            attempt_page.status.success(),
            "{}",
            String::from_utf8_lossy(&attempt_page.stderr)
        );
        let attempt_page: serde_json::Value = serde_json::from_slice(&attempt_page.stdout).unwrap();
        assert_eq!(
            attempt_page["physical_drain"]["attempt_search_complete"],
            true
        );
        assert!(
            attempt_page["physical_drain"]["attempts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|attempt| attempt["operation"] == "source_recovery"
                    && attempt["phase"] == "drained"
                    && attempt["drain_receipt"].is_string())
        );
        assert_eq!(fs::read(&recovered).unwrap(), b"paired-source-output");
        assert_eq!(
            readback["presentation_and_ack"][0]["policy"],
            "response_only"
        );
        assert!(f.mailbox().list_pending(SESSION).unwrap().is_empty());
        assert!(
            f.mailbox()
                .completion_event_listeners(&source.handle)
                .unwrap()[0]
                .acknowledged_at
                .is_none()
        );
        println!(
            "sync output returned; accepted source retained; no notification or ACK; original drain read from source evidence, not suppression"
        );
        let result: serde_json::Value = wait(|| {
            serde_json::from_slice(
                &fs::read(PathBuf::from(&source.handle_dir).join("root-work-result-v1.json"))
                    .ok()?,
            )
            .ok()
        });
        assert_eq!(result["physical_tree_drained"], true, "{result}");
        return;
    }
    wait(|| {
        let listeners = f
            .mailbox_for_poll()?
            .completion_event_listeners(&source.handle)
            .ok()?;
        listeners
            .iter()
            .all(|l| l.acknowledged_at.is_some())
            .then_some(())
    });
    // Listener ACK may race ahead through the normal native acceptance path;
    // require the adapter's independent byte readback, not ACK as a proxy.
    let byte_receipt: serde_json::Value = wait(|| {
        serde_json::from_slice(&fs::read(f.root.path().join("recipient-byte-receipt.json")).ok()?)
            .ok()
    });
    assert_eq!(byte_receipt["output_checked"], true);
    if matches!(mode, "async" | "pause" | "ack_before_drain") {
        assert_eq!(byte_receipt["artifact"], true);
    }
    println!("actual native adapter byte receipt={byte_receipt}");
    if matches!(mode, "ack_before_drain" | "manual_overlap") {
        let descendant: i64 = wait(|| {
            fs::read_to_string(f.root.path().join("descendant.pid"))
                .ok()?
                .parse()
                .ok()
        });
        let descendant_identity = read_live_process_identity(descendant)
            .unwrap()
            .expect("recipient's published descendant must still be live");
        let (claim, attempt) = wait(|| {
            let mailbox = f.mailbox_for_poll()?;
            let claim = mailbox.wake_session_reader().wake_claim(SESSION).ok()??;
            let attempt = mailbox
                .continuation_activation(SESSION, &claim.claim_token)
                .ok()??;
            Some((claim, attempt))
        });
        let phase: String = f
            .sidecar_connection()
            .query_row(
                "SELECT phase FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(matches!(phase.as_str(), "starting" | "running"));
        assert_eq!(
            read_live_process_identity(descendant).unwrap(),
            Some(descendant_identity),
            "listener ACK must precede physical descendant drain"
        );
        let mut writer = MailboxDb::open(&f.data.join("pid-identity.db")).unwrap();
        assert_eq!(
            writer
                .wake_sessions()
                .release_wake_claim_for_manual_resume(SESSION, &claim.claim_token),
            Ok(false),
            "manual release must retain the live v2 activation claim"
        );
        drop(writer);
        println!(
            "paired v2 ACK before drain attempt={} phase={phase} descendant={descendant}",
            attempt.attempt_id
        );
        f.gate("release-descendant");
    }
    // Both modes have completed their mode-specific initial observation above.
    assert!(initial.wait().unwrap().success());
    let prompt = fs::read_to_string(f.root.path().join("resume-prompts.jsonl")).unwrap();
    assert!(
        prompt.contains(&source.handle),
        "native resumed recipient did not receive exact source"
    );
    let launch_count = fs::read_to_string(f.root.path().join("source-launches"))
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        launch_count,
        if mode == "registration_reply_loss" {
            0
        } else {
            1
        },
        "completion recovery must not replay original workload"
    );
    println!(
        "actual workload launches={launch_count} native recipient invocations={}",
        prompt.lines().count()
    );
    if mode == "manual_overlap" {
        assert_eq!(
            prompt.lines().count(),
            1,
            "the overlapping manual and automatic entries must deliver to one recipient"
        );
    }
    let listeners = f
        .mailbox()
        .completion_event_listeners(&source.handle)
        .unwrap();
    if mode == "manual_overlap" {
        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].mailbox_seq, byte_receipt["seq"].as_i64());
        assert!(listeners[0].acknowledged_at.is_some());
    }
    assert!(
        listeners
            .iter()
            .all(|l| l.acknowledgement_reason.as_deref() != Some("consumed_in_call"))
    );
    println!(
        "listeners={} outstanding={:?}",
        serde_json::to_string(&listeners).unwrap(),
        f.mailbox()
            .pending_continuation_attempt_ids(&source.registration_id)
            .unwrap()
    );
    wait(|| {
        f.mailbox_for_poll()?
            .pending_continuation_attempt_ids(&source.registration_id)
            .ok()?
            .is_empty()
            .then_some(())
    });
    if let Some(mut manual) = manual_child {
        let status = wait(|| manual.try_wait().unwrap());
        println!(
            "overlapping manual resume status={status} stderr={}",
            fs::read_to_string(f.root.path().join("manual.stderr")).unwrap()
        );
        assert_eq!(
            fs::read_to_string(f.root.path().join("resume-prompts.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1,
            "manual completion must not create a second recipient"
        );
        wait(|| {
            f.mailbox_for_poll()?
                .wake_session_reader()
                .wake_claim(SESSION)
                .ok()?
                .is_none()
                .then_some(())
        });
        assert!(f.mailbox().list_pending(SESSION).unwrap().is_empty());
    }
    let (attempts, integrated): (i64, i64) = f.sidecar_connection().query_row(
        "SELECT COUNT(*),COALESCE(SUM(integrated),0) FROM completion_continuation_attempt WHERE source_registration_id=?1",
        [&source.registration_id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    if mode == "manual_overlap" {
        assert_eq!(
            (attempts, integrated),
            (2, 2),
            "source and exact activation both owe integrated physical outcomes"
        );
    }
    let census = live_census::observe(f.root.path()).expect("live resource traversal");
    println!(
        "paired live resource observations source_recovery_attempts={attempts} integrated={integrated} observed_files={} observed_bytes={} disappeared_entries={:?}; non-atomic traversal, unknown sizes for disappeared entries, not a complete snapshot; fixture teardown is not product release authority",
        census.observed_files, census.observed_bytes, census.disappeared
    );
    let result: serde_json::Value = wait(|| {
        serde_json::from_slice(
            &fs::read(PathBuf::from(&source.handle_dir).join("root-work-result-v1.json")).ok()?,
        )
        .ok()
    });
    assert_eq!(result["physical_tree_drained"], true, "{result}");
    assert!(
        result["outcome"]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    );
    if mode == "stripped_nested" {
        assert_ne!(
            fs::read_to_string(f.root.path().join("stripped-nested.rc"))
                .unwrap()
                .trim(),
            "0"
        );
        let stderr = fs::read_to_string(f.root.path().join("stripped-nested.stderr")).unwrap();
        assert!(
            stderr.contains("exact nested authority is required"),
            "stripped ambient context must be rejected by live process-tree state: {stderr}"
        );
    }
}
#[test]
fn normal_sleeping_recipient() {
    if private_case(true) {
        return;
    }
    paired_case("async");
}
#[test]
fn paired_two_native_sources_share_one_activation_and_drain() {
    if private_case(true) {
        return;
    }
    let f = Fixture::new("two_source");
    f.gate("release-resume");
    f.gate("test-descendant-enabled");
    let mut initial = f.start();
    let sources = wait(|| {
        let sources = oulipoly_state::StateDb::open_historical_read_only(&f.data.join("state.db"))
            .ok()?
            .admitted_completion_continuations()
            .ok()?;
        (sources.len() == 2).then_some(sources)
    });
    let registrations: Vec<_> = sources
        .iter()
        .map(|binding| binding.registration().unwrap())
        .collect();
    assert_ne!(
        registrations[0].registration_id,
        registrations[1].registration_id
    );
    let pause = f
        .command()
        .args(["mailbox", "pause", "--session-id", SESSION])
        .output()
        .unwrap();
    assert!(
        pause.status.success(),
        "{}",
        String::from_utf8_lossy(&pause.stderr)
    );
    f.gate("release-workload");
    f.wait_initial(&mut initial);
    wait(|| {
        let mailbox = f.mailbox_for_poll()?;
        registrations
            .iter()
            .all(|source| {
                mailbox
                    .completion_continuation_acceptance(&source.registration_id)
                    .ok()
                    .flatten()
                    .is_some_and(|row| row["phase"] == "accepted")
            })
            .then_some(())
    });
    assert_eq!(f.mailbox().list_pending(SESSION).unwrap().len(), 2);
    let resume = f
        .command()
        .args(["mailbox", "resume", "--session-id", SESSION])
        .output()
        .unwrap();
    assert!(
        resume.status.success(),
        "{}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let (claim, attempt) = wait(|| {
        let mailbox = f.mailbox_for_poll()?;
        let claim = mailbox.wake_session_reader().wake_claim(SESSION).ok()??;
        let attempt = mailbox
            .continuation_activation(SESSION, &claim.claim_token)
            .ok()??;
        Some((claim, attempt))
    });
    let links: i64 = f
        .sidecar_connection()
        .query_row(
            "SELECT COUNT(*) FROM completion_continuation_attempt_source WHERE attempt_id=?1",
            [&attempt.attempt_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(links, 2);
    for source in &registrations {
        let mailbox = f.mailbox();
        assert_eq!(
            mailbox
                .continuation_attempt_association_completeness(&source.registration_id)
                .unwrap(),
            "known"
        );
        assert_eq!(
            mailbox
                .pending_continuation_attempt_ids(&source.registration_id)
                .unwrap(),
            vec![attempt.attempt_id.clone()]
        );
        assert!(
            mailbox
                .completion_recovery_attempts(&source.registration_id, None)
                .unwrap()
                .0
                .iter()
                .any(|row| row["attempt_id"] == attempt.attempt_id
                    && row["association_completeness"] == "known")
        );
    }
    let first_receipt: serde_json::Value = wait(|| {
        serde_json::from_slice(&fs::read(f.root.path().join("recipient-byte-receipt.json")).ok()?)
            .ok()
    });
    let second_receipt: serde_json::Value = wait(|| {
        serde_json::from_slice(
            &fs::read(f.root.path().join("recipient-second-byte-receipt.json")).ok()?,
        )
        .ok()
    });
    assert_ne!(first_receipt["seq"], second_receipt["seq"]);
    assert_eq!(first_receipt["artifact"], true);
    assert_eq!(second_receipt["byte_len"], 20);
    wait(|| {
        let mailbox = f.mailbox_for_poll()?;
        registrations
            .iter()
            .all(|source| {
                mailbox
                    .completion_event_listeners(&source.handle)
                    .ok()
                    .is_some_and(|listeners| {
                        listeners.len() == 1 && listeners[0].acknowledged_at.is_some()
                    })
            })
            .then_some(())
    });
    let descendant: i64 = fs::read_to_string(f.root.path().join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(read_live_process_identity(descendant).unwrap().is_some());
    assert_eq!(
        f.mailbox()
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .unwrap()
            .claim_token,
        claim.claim_token
    );
    f.gate("release-descendant");
    wait(|| {
        let mailbox = f.mailbox_for_poll()?;
        (mailbox
            .wake_session_reader()
            .wake_claim(SESSION)
            .ok()?
            .is_none()
            && registrations.iter().all(|source| {
                mailbox
                    .pending_continuation_attempt_ids(&source.registration_id)
                    .ok()
                    .is_some_and(|ids| ids.is_empty())
            }))
        .then_some(())
    });
    for source in &registrations {
        assert!(
            f.mailbox()
                .completion_recovery_attempts(&source.registration_id, None)
                .unwrap()
                .0
                .iter()
                .any(|row| row["attempt_id"] == attempt.attempt_id
                    && row["phase"] == "drained"
                    && row["drain_receipt"].is_string())
        );
    }
    assert_eq!(
        fs::read_to_string(f.root.path().join("source-launches"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    assert_eq!(
        fs::read_to_string(f.root.path().join("resume-prompts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    println!(
        "two native accepted sources, one exact claim={}, one activation={}, two raw receipts and ACKs, retained physical drain",
        claim.claim_token, attempt.attempt_id
    );
}
#[test]
fn paired_v2_ack_precedes_physical_activation_drain() {
    if private_case(true) {
        return;
    }
    paired_case("ack_before_drain");
}
#[cfg(feature = "age360-fault-fixtures")]
#[test]
fn paired_manual_resume_overlaps_unadmitted_v2_receiver() {
    if private_case(true) {
        return;
    }
    paired_case("manual_overlap");
}
#[test]
fn sync_response_without_notification_or_ack() {
    if private_case(true) {
        return;
    }
    paired_case("sync");
}

#[test]
fn paired_stripped_nested_context_cannot_mint_a_fresh_root() {
    if private_case(true) {
        return;
    }
    paired_case("stripped_nested");
}

#[test]
fn native_guardian_restarts_exact_driver_without_provider_dependency() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    let mut initial = f.start_with_hold(true);
    let old = f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    assert!(current_identity_matches(&old.driver_identity));
    assert_eq!(
        unsafe { libc::kill(old.driver_identity.pid as i32, libc::SIGKILL) },
        0
    );
    let replacement = wait(|| {
        let owner = f.mailbox().completion_continuation_owner().ok()??;
        (owner.owner_generation != old.owner_generation).then_some(owner)
    });
    assert_eq!(old.guardian_identity, replacement.guardian_identity);
    assert_eq!(
        parent(replacement.driver_identity.pid),
        replacement.guardian_identity.pid
    );
    assert!(
        initial.try_wait().unwrap().is_none(),
        "native provider remains independent of CD loss"
    );
    println!(
        "replaced owner={} predecessor={}",
        serde_json::to_string(&replacement).unwrap(),
        serde_json::to_string(&old).unwrap()
    );
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    wait(|| {
        if !current_identity_matches(&replacement.guardian_identity) {
            return Some(());
        }
        let pid = unsafe {
            libc::waitpid(
                replacement.guardian_identity.pid as i32,
                std::ptr::null_mut(),
                libc::WNOHANG,
            )
        };
        (pid == replacement.guardian_identity.pid as i32).then_some(())
    });
}

/// This isolates native activation using a legitimate submitted input. It is
/// NOT paired Bash/source-evidence coverage and is reported separately.
#[test]
fn native_activation_receipt_does_not_release_live_descendant_custody() {
    if private_case(false) {
        return;
    }
    native_activation_custody(0);
}
#[test]
fn native_driver_replacement_retains_predecessor_activation_until_exact_drain() {
    if private_case(false) {
        return;
    }
    native_activation_custody(1);
}
#[test]
fn native_both_owner_loss_restart_preserves_original_custodian_debt() {
    if private_case(false) {
        return;
    }
    native_activation_custody(2);
}
#[test]
fn native_accepted_cancellation_drains_resistant_activation_without_source_signal() {
    if private_case(false) {
        return;
    }
    native_activation_custody(3);
}
#[test]
fn native_ac_loss_original_adopter_integrates_actual_drain() {
    if private_case(false) {
        return;
    }
    native_activation_custody(5);
}
#[test]
fn native_ac_loss_adopter_cancels_resistant_descendants() {
    if private_case(false) {
        return;
    }
    native_activation_custody(6);
}
#[test]
fn native_guardian_loss_before_first_source_requires_a_new_root() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    let mut initial = f.start_with_hold(true);
    let owner = f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    assert!(
        f.mailbox()
            .completion_contexts()
            .unwrap()
            .iter()
            .any(|v| v.pid == i64::from(initial.id()))
    );
    assert_eq!(
        unsafe { libc::kill(owner.guardian_identity.pid as i32, libc::SIGKILL) },
        0
    );
    // The driver no longer inherits the election or promotes itself. A fresh
    // independent entry mints a distinct root authority. Do not wait for the
    // dead driver's /proc identity to disappear: an unreaped zombie can retain
    // the same PID/starttime while holding no election lock or live authority.
    // The stale owner row is discovery only and cannot make that dead process
    // tree live again.
    let output = f
        .command()
        .args(["-m", "absent-pre-admission-model", "create successor root"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&output.stderr).contains("absent-pre-admission-model"));
    let replacement = wait(|| {
        let current = f.mailbox().completion_continuation_owner().ok()??;
        (current.owner_generation != owner.owner_generation).then_some(current)
    });
    assert_ne!(replacement.guardian_identity, owner.driver_identity);
    assert_ne!(replacement.guardian_identity, owner.guardian_identity);
    assert_ne!(
        replacement.supervisor_authority_id,
        owner.supervisor_authority_id
    );
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
}
#[cfg(feature = "age360-fault-fixtures")]
#[test]
fn native_adopter_retains_wait_until_delayed_launcher_identity_arrives() {
    if private_case(false) {
        return;
    }
    native_activation_custody(8);
}
fn native_activation_custody(owner_loss: u8) {
    native_activation_channel_custody(owner_loss, None)
}
fn native_activation_channel_custody(owner_loss: u8, channel: Option<&str>) {
    let f = Fixture::new("owner_only");
    if let Some(mode) = channel {
        fs::write(f.root.path().join("native-channel-mode"), mode).unwrap();
        fs::write(
            f.root.path().join("native-channel-helper"),
            std::env::current_exe()
                .unwrap()
                .as_os_str()
                .as_encoded_bytes(),
        )
        .unwrap();
    }
    let receipt_window = match owner_loss {
        11 => Some("native-before-custody-retention"),
        12 => Some("native-after-custody-retention"),
        _ => None,
    };
    if let Some(window) = receipt_window {
        f.gate(&format!("{window}.hold"));
    }
    if owner_loss == 8 {
        f.gate("activation-observation.hold");
        f.gate("adopted-terminal-wait.hold");
    }
    f.gate("test-descendant-enabled");
    if matches!(owner_loss, 3 | 6 | 8 | 9 | 10) {
        f.gate("cancel-probe-enabled");
    }
    f.gate("release-resume");
    let mut initial = f.start_with_hold(true);
    let owner = f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    let mut writer = MailboxDb::open(&f.data.join("pid-identity.db")).unwrap();
    writer
        .enqueue_submitted_input(&oulipoly_state::mailbox::SubmittedInputEnqueue {
            submission_token: "native-custody-input",
            target: oulipoly_state::mailbox::InboxTarget {
                kind: oulipoly_state::InboxTargetKind::Session,
                id: SESSION,
            },
            input: b"native-custody-input",
        })
        .unwrap();
    drop(writer);
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    wait(|| f.root.path().join("descendant.pid").exists().then_some(()));
    let descendant: i64 = fs::read_to_string(f.root.path().join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let descendant_identity = read_live_process_identity(descendant)
        .unwrap()
        .expect("published real descendant incarnation");
    wait(|| {
        let rows = f.mailbox().list_mailbox(SESSION, true).ok()?;
        (!rows.is_empty() && rows.iter().all(|r| r.delivered_at.is_some())).then_some(())
    });
    let mailbox = f.mailbox();
    let claim = mailbox
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .expect("ACK must not release live AC tree reservation");
    let attempt = mailbox
        .continuation_activation(SESSION, &claim.claim_token)
        .unwrap()
        .expect("actual retained activation");
    let (phase,custodian,launcher,generation):(String,String,String,String)=f.sidecar_connection().query_row("SELECT phase,custodian_identity,launcher_identity,runtime_generation_uuid FROM completion_continuation_attempt WHERE attempt_id=?1",[&attempt.attempt_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert!(matches!(phase.as_str(), "starting" | "running"));
    let binding_invocation = if owner_loss == 0 && channel.is_none() {
        // AGE123 binding witness: the driver-created generation identifies the
        // actual resumed invocation. A chain alias is not a provider session.
        let invocation: String = f
            .sidecar_connection()
            .query_row(
                "SELECT spawn_invocation_uuid FROM runtime_generation WHERE generation_uuid=?1",
                [&generation],
                |row| row.get(0),
            )
            .unwrap();
        let state = rusqlite::Connection::open_with_flags(
            f.data.join("state.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let binding: (String, String, String) = state.query_row(
            "SELECT provider_name, provider_session_id, resume_input_id FROM invocations WHERE invocation_uuid=?1",
            [&invocation], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert_eq!(binding, (PROVIDER.into(), SESSION.into(), SESSION.into()));
        assert_ne!(owner.driver_identity.pid, i64::from(std::process::id()));
        assert_ne!(owner.guardian_identity.pid, i64::from(std::process::id()));
        println!(
            "native binding invocation={invocation} provider={} concrete_session={} resume_input={}",
            binding.0, binding.1, binding.2
        );
        Some(invocation)
    } else {
        None
    };

    assert!(read_live_process_identity(descendant).unwrap().is_some());
    println!(
        "ACK before drain owner={} attempt={} custodian={custodian} launcher={launcher} generation={generation} descendant={descendant}",
        serde_json::to_string(&owner).unwrap(),
        serde_json::to_string(&attempt).unwrap()
    );
    let adopter: String = f
        .sidecar_connection()
        .query_row(
            "SELECT adopter_identity FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&attempt.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    for (role, encoded) in [
        ("AC", custodian.as_str()),
        ("original_adopter", adopter.as_str()),
    ] {
        let identity: oulipoly_state::completion_continuation::SourceProcessIdentity =
            serde_json::from_str(encoded).unwrap();
        let descriptors = fs::read_dir(format!("/proc/{}/fd", identity.pid))
            .unwrap()
            .count();
        let status = fs::read_to_string(format!("/proc/{}/status", identity.pid)).unwrap();
        let rss = status
            .lines()
            .find(|line| line.starts_with("VmRSS:"))
            .unwrap_or("VmRSS unavailable");
        println!(
            "resource sample role={role} pid={} fds={descriptors} {rss}",
            identity.pid
        );
    }
    if matches!(owner_loss, 1 | 2) {
        assert!(current_identity_matches(&owner.driver_identity));
        assert_eq!(
            unsafe { libc::kill(owner.driver_identity.pid as i32, libc::SIGKILL) },
            0
        );
        if owner_loss == 2 {
            assert!(current_identity_matches(&owner.guardian_identity));
            assert_eq!(
                unsafe { libc::kill(owner.guardian_identity.pid as i32, libc::SIGKILL) },
                0
            );
            wait(|| {
                let pid = unsafe {
                    libc::waitpid(
                        owner.guardian_identity.pid as i32,
                        std::ptr::null_mut(),
                        libc::WNOHANG,
                    )
                };
                (pid == owner.guardian_identity.pid as i32).then_some(())
            });
            // A new independent native root bootstraps ownership before model
            // resolution. Deliberately absent model prevents any provider work.
            let output = f
                .command()
                .args(["-m", "absent-recovery-model", "recover owner only"])
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("absent-recovery-model"));
        }
        let replacement = wait(|| {
            let current = f.mailbox().completion_continuation_owner().ok()??;
            // MailboxDb reads a copied SQLite snapshot; sidecar_connection reads
            // the live WAL view. Do not infer the live attempt transition from
            // a different snapshot's owner row. Observe both in one live query.
            let (published_generation, phase): (String, String) = f.sidecar_connection().query_row(
                "SELECT o.generation,a.phase FROM completion_continuation_owner o JOIN completion_continuation_attempt a ON a.domain_id=o.domain_id WHERE o.phase='running' AND a.attempt_id=?1",
                [&attempt.attempt_id], |r| Ok((r.get(0)?,r.get(1)?))).ok()?;
            let expected_phase = if owner_loss == 1 {
                matches!(phase.as_str(), "starting" | "running")
            } else {
                phase == "unknown_custody"
            };
            (current.owner_generation != owner.owner_generation
                && published_generation == current.owner_generation
                && expected_phase)
                .then_some(current)
        });
        if owner_loss == 1 {
            assert_eq!(replacement.guardian_identity, owner.guardian_identity);
        } else {
            assert_ne!(replacement.guardian_identity, owner.guardian_identity);
        }
        let phase: String = f
            .sidecar_connection()
            .query_row(
                "SELECT phase FROM completion_continuation_attempt WHERE attempt_id=?1",
                [&attempt.attempt_id],
                |r| r.get(0),
            )
            .unwrap();
        if owner_loss == 1 {
            assert!(matches!(phase.as_str(), "starting" | "running"));
        } else {
            assert_eq!(phase, "unknown_custody");
        }
        assert!(read_live_process_identity(descendant).unwrap().is_some());
        assert_eq!(
            fs::read_to_string(f.root.path().join("resume-prompts.jsonl"))
                .unwrap()
                .lines()
                .count(),
            1,
            "replacement must not replay recipient while old custody remains"
        );
        println!(
            "replacement retained predecessor debt={}",
            attempt.attempt_id
        );
    }
    if matches!(owner_loss, 5 | 6 | 8 | 10) {
        let custodian: oulipoly_state::completion_continuation::SourceProcessIdentity =
            serde_json::from_str(&custodian).unwrap();
        assert!(current_identity_matches(&custodian));
        assert_eq!(
            unsafe { libc::kill(custodian.pid as i32, libc::SIGKILL) },
            0
        );
        // Killing AC is not release: the real adopted descendant remains live.
        assert!(
            f.mailbox()
                .continuation_activation(SESSION, &claim.claim_token)
                .unwrap()
                .is_some()
        );
        assert!(read_live_process_identity(descendant).unwrap().is_some());
    }
    if channel == Some("commit_failure") {
        let state = rusqlite::Connection::open(f.data.join("state.db")).unwrap();
        state.execute_batch("CREATE TRIGGER fixture_fail_artifact_commit BEFORE INSERT ON invocation_returned_artifacts BEGIN SELECT RAISE(FAIL, 'fixture transient artifact persistence failure'); END;").unwrap();
    }
    let wait_obstruction = PathBuf::from(&attempt.result_path).with_file_name("owned-waits");
    if channel == Some("wait_storage_failure") {
        assert!(!wait_obstruction.exists());
        fs::write(&wait_obstruction, b"fixture obstruction, not a receipt").unwrap();
    }
    let mut writer = MailboxDb::open(&f.data.join("pid-identity.db")).unwrap();
    assert!(!matches!(
        writer
            .wake_sessions()
            .release_wake_claim_for_manual_resume(SESSION, &claim.claim_token),
        Ok(true)
    ));
    drop(writer);
    drop(mailbox);
    if let Some(window) = receipt_window {
        f.gate("release-descendant");
        wait(|| {
            f.root
                .path()
                .join(format!("{window}.reached"))
                .exists()
                .then_some(())
        });
        let producer_pid =
            fs::read_to_string(f.root.path().join(format!("{window}.reached"))).unwrap();
        let exact: oulipoly_state::completion_continuation::SourceProcessIdentity =
            serde_json::from_str(&launcher).unwrap();
        assert_eq!(producer_pid.trim().parse::<i64>().unwrap(), exact.pid);
        assert!(current_identity_matches(&exact));
        println!("actual original executor held at receipt boundary={window} pid={producer_pid}");
        request_linked_cancel(&f, &attempt);
        // Keep the producer held. Actual AC cancellation, not test release,
        // terminates it at this exact producer receipt boundary.
    } else if matches!(owner_loss, 3 | 6 | 8 | 9 | 10) {
        wait(|| {
            f.root
                .path()
                .join("cancel-descendant-ready")
                .exists()
                .then_some(())
        });
        let launcher: oulipoly_state::completion_continuation::SourceProcessIdentity =
            serde_json::from_str(&launcher).unwrap();
        assert!(current_identity_matches(&launcher));
        if matches!(owner_loss, 9 | 10) {
            if channel == Some("wait_storage_failure") {
                // Select original-AC adoption rather than let an inner reaper
                // consume this descendant first. Freeze its genuine ancestors
                // below the launcher; only product cancellation kills them.
                let mut child = descendant;
                let mut seen = std::collections::HashSet::new();
                loop {
                    let stat = fs::read_to_string(format!("/proc/{child}/stat")).unwrap();
                    let parent: i64 = stat
                        .rsplit_once(") ")
                        .unwrap()
                        .1
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .parse()
                        .unwrap();
                    if parent == launcher.pid {
                        break;
                    }
                    assert!(
                        parent > 1 && seen.insert(parent),
                        "must reach original launcher through real ancestry"
                    );
                    let ac: oulipoly_state::completion_continuation::SourceProcessIdentity =
                        serde_json::from_str(&custodian).unwrap();
                    assert_ne!(parent, ac.pid, "never freeze original AC");
                    let identity = read_live_process_identity(parent).unwrap().unwrap();
                    assert_eq!(unsafe { libc::kill(parent as i32, libc::SIGSTOP) }, 0);
                    wait(|| {
                        assert_eq!(
                            read_live_process_identity(parent).unwrap(),
                            Some(identity.clone())
                        );
                        let stat = fs::read_to_string(format!("/proc/{parent}/stat")).unwrap();
                        (stat.rsplit_once(") ").unwrap().1.split_whitespace().next() == Some("T"))
                            .then_some(())
                    });
                    println!("frozen exact inner ancestor, not a wait receipt: {identity:?}");
                    child = parent;
                }
            }
            request_linked_cancel(&f, &attempt);
            if channel == Some("wait_storage_failure") {
                let ac: oulipoly_state::completion_continuation::SourceProcessIdentity =
                    serde_json::from_str(&custodian).unwrap();
                // Cessation is NOT reaping: with storage obstructed the original
                // owner must retain this exact terminal, still-waitable incarnation.
                wait(|| {
                    let identity = read_live_process_identity(descendant).unwrap()?;
                    assert_eq!(identity, descendant_identity);
                    let stat = fs::read_to_string(format!("/proc/{descendant}/stat")).unwrap();
                    let fields: Vec<_> = stat
                        .rsplit_once(") ")
                        .unwrap()
                        .1
                        .split_whitespace()
                        .collect();
                    (fields[0] == "Z" && fields[1].parse::<i64>().unwrap() == ac.pid).then(|| {
                        assert_eq!(fields[49].parse::<i32>().unwrap(), libc::SIGKILL);
                        println!("ceased but unreaped exact descendant={identity:?} stat={stat}");
                    })
                });
                let adopter: oulipoly_state::completion_continuation::SourceProcessIdentity =
                    serde_json::from_str(&adopter).unwrap();
                assert!(current_identity_matches(&ac) && current_identity_matches(&adopter));
                assert!(
                    f.mailbox()
                        .continuation_activation(SESSION, &claim.claim_token)
                        .unwrap()
                        .is_some()
                );
                assert_eq!(
                    fs::read(&wait_obstruction).unwrap(),
                    b"fixture obstruction, not a receipt"
                );
                let pending: (i64, Option<String>, bool) = f.sidecar_connection().query_row(
                    "SELECT integrated,drain_receipt,EXISTS(SELECT 1 FROM session_wake_claim WHERE session_id=?2 AND claim_token=?3) FROM completion_continuation_attempt WHERE attempt_id=?1",
                    rusqlite::params![attempt.attempt_id, SESSION, claim.claim_token],
                    |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                assert_eq!(pending, (0, None, true));
                assert!(
                    !std::path::Path::new(&attempt.result_path).exists(),
                    "no false aggregate drain"
                );
                fs::remove_file(&wait_obstruction).unwrap();
                println!(
                    "physical cancellation remained effective with original wait owner retained through actual journal I/O denial"
                );
            }
        } else {
            assert_eq!(unsafe { libc::kill(launcher.pid as i32, libc::SIGTERM) }, 0);
            println!(
                "terminal cancellation exact launcher={}",
                serde_json::to_string(&launcher).unwrap()
            );
        }
        if owner_loss == 8 {
            wait(|| {
                f.root
                    .path()
                    .join("adopted-terminal-wait.reached")
                    .exists()
                    .then_some(())
            });
            fs::remove_file(f.root.path().join("adopted-terminal-wait.hold")).unwrap();
            fs::remove_file(f.root.path().join("activation-observation.hold")).unwrap();
        }
    } else {
        f.gate("release-descendant");
    }
    wait(|| {
        f.mailbox()
            .wake_session_reader()
            .wake_claim(SESSION)
            .ok()?
            .is_none()
            .then_some(())
    });
    // Wake claim release can precede the original adopter's durable drain
    // integration. Await that independent obligation, not just claim release.
    let receipt: (String, i64, String) = wait(|| {
        f.sidecar_connection().query_row(
            "SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1 AND phase='drained' AND integrated=1 AND drain_receipt IS NOT NULL",
            [&attempt.attempt_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).ok()
    });
    assert_eq!(receipt.0, "drained");
    assert_eq!(receipt.1, 1);
    assert!(receipt.2.contains("ECHILD"));
    if let Some(invocation) = binding_invocation {
        let state = rusqlite::Connection::open_with_flags(
            f.data.join("state.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let outcome: (String, bool, i64) = state
            .query_row(
                "SELECT status, success, exit_code FROM invocations WHERE invocation_uuid=?1",
                [&invocation],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            outcome,
            (
                oulipoly_state::InvocationStatus::Succeeded.as_str().into(),
                true,
                0
            )
        );
        println!("native bound invocation={invocation} final outcome={outcome:?}");
    }

    if channel == Some("wait_storage_failure") {
        let ac: oulipoly_state::completion_continuation::SourceProcessIdentity =
            serde_json::from_str(&custodian).unwrap();
        let journal = wait_obstruction.join(format!(
            "{}-{}-{}.json",
            descendant, descendant_identity.os_pid_starttime_ticks, ac.pid
        ));
        let value: serde_json::Value = serde_json::from_slice(&fs::read(journal).unwrap()).unwrap();
        assert_eq!(value["attempt_id"], attempt.attempt_id);
        assert_eq!(value["owner"], serde_json::to_value(&ac).unwrap());
        assert_eq!(value["process"]["pid"], descendant);
        assert_eq!(value["process"]["boot_id"], descendant_identity.os_boot_id);
        assert_eq!(
            value["process"]["starttime_ticks"],
            descendant_identity.os_pid_starttime_ticks
        );
        assert_eq!(value["status"], libc::SIGKILL);
        assert_eq!(value["observation"], "waitid_wnowait");
        let aggregate: serde_json::Value = serde_json::from_str(&receipt.2).unwrap();
        assert_eq!(aggregate["custodian"], serde_json::to_value(&ac).unwrap());
        assert_eq!(aggregate["attempt_id"], attempt.attempt_id);
        assert!(read_live_process_identity(descendant).unwrap().is_none());
        println!("restored original wait journal={value}; integrated original ECHILD={aggregate}");
    }

    if matches!(owner_loss, 5 | 6 | 8 | 10) {
        let value: serde_json::Value = serde_json::from_str(&receipt.2).unwrap();
        assert_eq!(
            value["classification"],
            "original_adopting_boundary_drained"
        );
        assert!(value["adopter"].is_object());
        assert_eq!(value["custodian_wait_status"], libc::SIGKILL);
    }
    if matches!(owner_loss, 3 | 6 | 8 | 9..=12) {
        let receipt: serde_json::Value = serde_json::from_str(&receipt.2).unwrap();
        assert!(receipt["accepted_cancellation"].is_string(), "{receipt}");
        if matches!(owner_loss, 9..=12) {
            let expected = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
            assert_eq!(receipt["accepted_cancellation"], expected);
            let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
            let launch = expected.split(':').next().unwrap();
            if channel == Some("wait_storage_failure") {
                wait(|| {
                    let status: String = state.connection().query_row("SELECT status FROM provider_logical_launches WHERE logical_launch_id=?1", [launch], |r| r.get(0)).unwrap();
                    (status == "cancelled").then_some(())
                });
            }
            let status: String = state
                .connection()
                .query_row(
                    "SELECT status FROM provider_logical_launches WHERE logical_launch_id=?1",
                    [launch],
                    |r| r.get(0),
                )
                .unwrap();
            let receipts: i64 = state.connection().query_row("SELECT COUNT(*) FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key LIKE '%/native-custody-receipts'", [launch], |r| r.get(0)).unwrap();
            println!(
                "logical cancellation after physical drain: status={status} retained_producer_receipts={receipts}; physical assertions do not certify logical settlement"
            );
        }
        if receipt_window.is_none() {
            assert!(!f.root.path().join("release-descendant").exists());
        }
        assert!(read_live_process_identity(descendant).unwrap().is_none());
    }
    if channel == Some("commit_failure") {
        let journal_root = f
            .data
            .join("state.db")
            .with_extension("native-producer-custody");
        let (journal, retained) = wait(|| {
            for entry in fs::read_dir(&journal_root).ok()? {
                let journal = entry.ok()?.path().join("channel");
                let bytes = fs::read(journal.join("settlement.json")).ok()?;
                let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
                if value["CleanupFailed"]["artifacts"]
                    .as_array()
                    .is_some_and(|v| !v.is_empty())
                {
                    return Some((journal, bytes));
                }
            }
            None
        });
        let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
        let count: i64 = state
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM invocation_returned_artifacts",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 0,
            "cached decoded refs are not an invocation artifact commit"
        );
        rusqlite::Connection::open(f.data.join("state.db"))
            .unwrap()
            .execute_batch("DROP TRIGGER fixture_fail_artifact_commit;")
            .unwrap();
        assert_eq!(fs::read(journal.join("settlement.json")).unwrap(), retained);
        assert!(journal.join("original-channel").exists());
        println!(
            "actual failed State commit retained exact channel refs; storage recovered under original driver"
        );
    }
    if matches!(owner_loss, 9..=12) {
        let expected = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
        let launch = expected.split(':').next().unwrap();
        wait(|| {
            let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).ok()?;
            let status: String = state
                .connection()
                .query_row(
                    "SELECT status FROM provider_logical_launches WHERE logical_launch_id=?1",
                    [launch],
                    |r| r.get(0),
                )
                .ok()?;
            (status == "cancelled").then_some(())
        });
        println!(
            "actual physical drain AND logical cancellation settled boundary={receipt_window:?}"
        );
    }
    if let Some(mode) = channel {
        let expected = fs::read_to_string(f.root.path().join("state-cancel-token")).unwrap();
        let launch = expected.split(':').next().unwrap();
        let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
        let row: i64 = state
            .connection()
            .query_row(
                "SELECT invocation_id FROM provider_launch_attempts WHERE logical_launch_id=?1",
                [launch],
                |r| r.get(0),
            )
            .unwrap();
        let refs = state.list_returned_artifacts(row).unwrap();
        assert_eq!(refs.len(), 1);
        let artifact = oulipoly_agent_messenger::show_returned(
            oulipoly_agent_messenger::ShowReturnedRequest::VersionId {
                db_path: f.root.path().join("native-artifact-store.db"),
                version_id: refs[0].version_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(artifact.content, b"actual-native-return");
        assert_eq!(
            refs[0].sha256,
            oulipoly_state::completion_continuation::sha256(&artifact.content)
        );
        let duties = state.pending_native_channel_duties().unwrap();
        if matches!(mode, "committed" | "wait_storage_failure") {
            assert!(duties.is_empty());
        } else {
            assert_eq!(duties.len(), 1);
            let oulipoly_state::ProviderLaunchChannelSettlement::ContinuingCustody {
                domain_id,
                original_owner,
                disposition,
                path,
                artifacts,
            } = &duties[0]
            else {
                panic!("not continuing custody");
            };
            assert_eq!(domain_id, &owner.domain_id);
            assert_eq!(original_owner.logical_launch_id.to_string(), launch);
            assert_eq!(
                disposition,
                if mode == "commit_failure" {
                    "cleanup_failed"
                } else {
                    mode
                }
            );
            assert_eq!(artifacts, &refs);
            if mode == "quarantined" {
                assert!(fs::read_to_string(path).unwrap().ends_with("malformed\n"));
            } else if mode == "commit_failure" {
                assert!(!fs::read(path).unwrap().is_empty());
            } else {
                assert_eq!(
                    fs::read(
                        PathBuf::from(path)
                            .parent()
                            .unwrap()
                            .join("retained-cleanup-obligation")
                    )
                    .unwrap(),
                    b"retain me"
                );
            }
            println!(
                "logical cancellation retains domain-owned continuing duty={:?}",
                duties[0]
            );
        }
    }
}

#[cfg(feature = "age360-fault-fixtures")]
#[test]
fn native_interrupted_fresh_creation_never_publishes_legacy17() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("fresh-schema17.hold");
    let mut initial = f.start_with_hold(true);
    wait(|| {
        f.root
            .path()
            .join("fresh-schema17.reached")
            .exists()
            .then_some(())
    });
    assert!(
        !f.data.join("pid-identity.db").exists(),
        "legacy17 must remain staging-only"
    );
    initial.kill().unwrap();
    initial.wait().unwrap();
    fs::remove_file(f.root.path().join("fresh-schema17.hold")).unwrap();
    let mut replacement = f.start_with_hold(true);
    let owner = f.owner();
    assert!(
        f.mailbox()
            .completion_continuation_domain()
            .unwrap()
            .is_some()
    );
    println!(
        "fresh crash recovered owner={}",
        serde_json::to_string(&owner).unwrap()
    );
    f.gate("release-initial-provider");
    f.wait_initial(&mut replacement);
}

#[cfg(feature = "age360-fault-fixtures")]
#[test]
fn native_root_reconciles_retained_result_across_driver_replacement() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("owner_only");
    f.gate("test-descendant-enabled");
    f.gate("release-resume");
    f.gate("root-result-retained.hold");
    let mut initial = f.start_with_hold(true);
    let owner = f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    MailboxDb::open(&f.data.join("pid-identity.db"))
        .unwrap()
        .enqueue_submitted_input(&oulipoly_state::mailbox::SubmittedInputEnqueue {
            submission_token: "native-result-replay",
            target: oulipoly_state::mailbox::InboxTarget {
                kind: oulipoly_state::mailbox::InboxTargetKind::Session,
                id: SESSION,
            },
            input: b"native-custody-input",
        })
        .unwrap();
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    wait(|| f.root.path().join("descendant.pid").exists().then_some(()));
    f.gate("release-descendant");
    wait(|| {
        f.root
            .path()
            .join("root-result-retained.reached")
            .exists()
            .then_some(())
    });
    let claim = f
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .unwrap();
    let attempt = f
        .mailbox()
        .continuation_activation(SESSION, &claim.claim_token)
        .unwrap()
        .unwrap();
    let original = fs::read(&attempt.result_path).unwrap();
    let retained: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(retained["authority"], "root_supervisor");
    assert_eq!(retained["result_retained"], true);
    assert_eq!(retained["owned_children"], "ECHILD");
    assert!(
        f.mailbox()
            .continuation_activation(SESSION, &claim.claim_token)
            .unwrap()
            .is_some()
    );
    assert!(current_identity_matches(&owner.driver_identity));
    assert_eq!(
        unsafe { libc::kill(owner.driver_identity.pid as i32, libc::SIGKILL) },
        0
    );
    fs::remove_file(f.root.path().join("root-result-retained.hold")).unwrap();
    let replacement = wait(|| {
        let current = f.mailbox().completion_continuation_owner().ok()??;
        (current.owner_generation != owner.owner_generation).then_some(current)
    });
    assert_eq!(replacement.guardian_identity, owner.guardian_identity);
    assert_eq!(
        replacement.supervisor_authority_id,
        owner.supervisor_authority_id
    );
    wait(|| {
        f.mailbox()
            .continuation_activation(SESSION, &claim.claim_token)
            .ok()?
            .is_none()
            .then_some(())
    });
    let integrated: String = f
        .sidecar_connection()
        .query_row(
            "SELECT drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",
            [attempt.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        integrated.as_bytes(),
        original,
        "root reconciliation must preserve the exact retained result"
    );
    assert_eq!(
        fs::read_to_string(f.root.path().join("resume-prompts.jsonl"))
            .unwrap()
            .lines()
            .count(),
        1,
        "driver replacement must not replay the recipient"
    );
}

#[test]
fn paired_ready_exit_before_sentinel_is_genuine_exit() {
    if private_case(true) {
        return;
    }
    paired_case("early_exit");
}
#[test]
fn paired_supported_output_artifact_is_complete_and_retained() {
    if private_case(true) {
        return;
    }
    paired_case("large_output");
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        for name in [
            "native.stdout",
            "native.stderr",
            "bash-dispatch.stdout",
            "bash-dispatch.stderr",
            "resume-prompts.jsonl",
            "provider-error.txt",
            "resume-preflight.jsonl",
            "native-describe.actor.json",
            "native-policy.evaluate.actor.json",
            "descendant.pid",
            "recipient-exact-ack.json",
        ] {
            if let Ok(bytes) = fs::read(self.root.path().join(name)) {
                eprintln!("fixture {name}: {}", String::from_utf8_lossy(&bytes));
            }
        }
        // Only this test's private PID namespace is visible here.
        for entry in fs::read_dir("/proc").unwrap().flatten() {
            if entry.file_name().to_string_lossy().parse::<i64>().is_err() {
                continue;
            }
            for file in ["stat", "wchan", "cmdline"] {
                if let Ok(bytes) = fs::read(entry.path().join(file)) {
                    eprintln!(
                        "fixture proc {} {file}: {}",
                        entry.file_name().to_string_lossy(),
                        String::from_utf8_lossy(&bytes)
                    );
                }
            }
        }
        fn logs(path: &std::path::Path) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    logs(&entry.path());
                } else if let Ok(bytes) = fs::read(entry.path()) {
                    eprintln!(
                        "fixture attempt {}: {}",
                        entry.path().display(),
                        String::from_utf8_lossy(&bytes)
                    );
                }
            }
        }
        logs(&self.data.join("completion-continuation"));
        logs(&self.data.join("state.native-producer-custody"));
        logs(&self.data.join("pid-identity.native-recovery-errors"));
        // Retain exact paired helper failure evidence, without dumping raw output,
        // pinned binaries or the private launch capability/environment.
        fn source_logs(path: &std::path::Path) {
            let Ok(entries) = fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.path().is_dir() {
                    source_logs(&entry.path());
                } else if !name.contains("environment")
                    && !name.contains("request")
                    && (name.ends_with(".json")
                        || name.ends_with(".txt")
                        || name.ends_with(".stderr")
                        || name.ends_with(".stdout"))
                    && fs::metadata(entry.path()).is_ok_and(|m| m.len() < 65536)
                    && let Ok(bytes) = fs::read(entry.path())
                {
                    eprintln!(
                        "paired source {}: {}",
                        entry.path().display(),
                        String::from_utf8_lossy(&bytes)
                    );
                }
            }
        }
        source_logs(&self.root.path().join("spool/agent-bash"));
        if let Ok(db) = rusqlite::Connection::open_with_flags(
            self.data.join("pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            for table in [
                "completion_continuation_owner",
                "completion_continuation_attempt",
                "session_wake_claim",
                "session_runtime",
                "session_metadata",
                "runtime_generation",
            ] {
                let Ok(mut statement) = db.prepare(&format!("SELECT * FROM {table}")) else {
                    continue;
                };
                let count = statement.column_count();
                let Ok(rows) = statement.query_map([], |r| {
                    (0..count)
                        .map(|i| r.get::<_, rusqlite::types::Value>(i))
                        .collect::<rusqlite::Result<Vec<_>>>()
                }) else {
                    continue;
                };
                for row in rows {
                    eprintln!("fixture {table}: {row:?}");
                }
            }
        }
    }
}

// Native token cancellation uses the same State API as runtime cancellation,
// joined to the actual activation invocation. No fixture-written launch rows.
fn request_linked_cancel(f: &Fixture, attempt: &oulipoly_state::mailbox::ContinuationAttempt) {
    let (generation, invocation) = wait(|| {
        f.mailbox()
            .continuation_runtime_identity(attempt)
            .ok()
            .flatten()
    });
    let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
    let count: i64 = state
        .connection()
        .query_row("SELECT COUNT(*) FROM provider_launch_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    println!(
        "State launch-attempt rows={count}; requiring actual join runtime={generation} invocation={invocation}"
    );
    let launch: String = state.connection().query_row(
        "SELECT logical_launch_id FROM provider_launch_attempts WHERE runtime_generation_uuid=?1 AND invocation_uuid=?2",
        rusqlite::params![generation, invocation], |r| r.get(0)).expect("native activation lacks actual State logical-launch cancellation link; synthetic rows are forbidden");
    state
        .request_cancel(uuid::Uuid::parse_str(&launch).unwrap())
        .unwrap();
    let token: String = state.connection().query_row(
        "SELECT logical_launch_id || ':' || cancel_requested_at FROM provider_logical_launches WHERE logical_launch_id=?1",
        [&launch], |r| r.get(0)).unwrap();
    fs::write(f.root.path().join("state-cancel-token"), &token).unwrap();
    println!(
        "actual State cancellation token={token} runtime={generation} invocation={invocation}"
    );
}

#[test]
fn native_state_linked_token_cancellation_drains_resistant_activation() {
    if !private_case(false) {
        native_activation_custody(9);
    }
}
#[test]
fn native_state_linked_token_cancellation_after_ac_loss_drains_original_adopter() {
    if !private_case(false) {
        native_activation_custody(10);
    }
}

#[test]
fn paired_pause_retains_event_until_real_resume_and_ack() {
    if !private_case(true) {
        paired_case("pause");
    }
}
#[test]
fn paired_driver_replacement_does_not_replay_original_source() {
    if !private_case(true) {
        paired_case("driver_replacement");
    }
}

#[cfg(feature = "age360-fault-fixtures")]
#[test]
fn native_state_cancellation_before_producer_custody_retention_settles() {
    if private_case(false) {
        return;
    }
    native_activation_custody(11);
}

#[cfg(feature = "age360-fault-fixtures")]
#[test]
fn native_state_cancellation_after_producer_custody_retention_settles() {
    if private_case(false) {
        return;
    }
    native_activation_custody(12);
}

#[test]
fn native_channel_producer_helper() {
    let Ok(channel) = std::env::var("AGE360_NATIVE_RETURN_HELPER_CHANNEL") else {
        return;
    };
    let db = PathBuf::from(std::env::var("AGE360_NATIVE_RETURN_HELPER_DB").unwrap());
    oulipoly_agent_store::Store::init(&db).unwrap();
    oulipoly_agent_messenger::return_artifact(oulipoly_agent_messenger::ReturnRequest {
        db_path: db,
        invocation_uuid: std::env::var("AGE360_NATIVE_RETURN_HELPER_PRODUCER")
            .unwrap()
            .parse()
            .unwrap(),
        name: oulipoly_agent_messenger::ReturnName::new("result").unwrap(),
        source: oulipoly_agent_messenger::ReturnSource::InlineBytes(
            b"actual-native-return".to_vec(),
        ),
        format_hint: None,
        verdict_line: None,
        return_channel: Some(PathBuf::from(channel)),
    })
    .unwrap();
}
#[test]
fn native_original_owner_recovers_committed_return_after_executor_cancellation() {
    if !private_case(false) {
        native_activation_channel_custody(9, Some("committed"));
    }
}
#[test]
fn native_original_adopter_recovers_quarantine_and_keeps_domain_cleanup_owner() {
    if !private_case(false) {
        native_activation_channel_custody(10, Some("quarantined"));
    }
}
#[test]
fn native_original_owner_recovers_failed_cleanup_and_preserves_owned_sidecar() {
    if !private_case(false) {
        native_activation_channel_custody(9, Some("cleanup_failed"));
    }
}

#[test]
fn native_cancellation_keeps_original_wait_owner_when_journal_storage_is_unavailable() {
    if !private_case(false) {
        native_activation_channel_custody(9, Some("wait_storage_failure"));
    }
}

#[test]
fn native_missing_output_rejects_transient_then_delivers_original_wait_without_capture_retries() {
    if private_case(false) {
        return;
    }
    let f = Fixture::new("native_missing");
    f.gate("release-resume");
    let mut initial = f.start();
    let binding = f.source();
    let source = binding.registration().unwrap();
    wait(|| {
        f.root
            .path()
            .join("native-source-invalid-ready")
            .exists()
            .then_some(())
    });
    let rejected: serde_json::Value = serde_json::from_slice(
        &fs::read(f.root.path().join("native-source-transient-rejection.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(rejected["status"], "unavailable");
    let before = f
        .mailbox()
        .completion_continuation_acceptance(&source.registration_id)
        .unwrap()
        .unwrap();
    assert_ne!(before["phase"], "accepted");
    assert!(
        f.mailbox()
            .completion_event_listeners(&source.handle)
            .unwrap()
            .iter()
            .all(|l| l.mailbox_seq.is_none() && l.acknowledged_at.is_none())
    );
    f.gate("release-source-proof");
    f.wait_initial(&mut initial);
    let receipt: serde_json::Value = wait(|| {
        serde_json::from_slice(
            &fs::read(f.root.path().join("recipient-missing-output-receipt.json")).ok()?,
        )
        .ok()
    });
    assert_eq!(receipt["outcome"]["root_wait_status"], 37 << 8);
    assert_eq!(receipt["snapshot"]["output"]["observed_byte_len"], 0);
    assert!(
        receipt["snapshot"]["output"]["selection"]["byte_len"]
            .as_u64()
            .unwrap()
            > 0
    );
    let observed: serde_json::Value = serde_json::from_slice(
        &fs::read(
            f.root
                .path()
                .join("native-source-original-observation.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["snapshot"]["output"], observed["output"]);
    assert_eq!(
        receipt["outcome"]["root_wait_status"],
        observed["root_wait_status"]
    );
    wait(|| {
        f.root
            .path()
            .join("recipient-exact-ack.json")
            .exists()
            .then_some(())
    });
    wait(|| {
        f.mailbox()
            .completion_event_listeners(&source.handle)
            .ok()?
            .iter()
            .all(|l| l.acknowledged_at.is_some())
            .then_some(())
    });
    wait(|| {
        f.mailbox()
            .pending_continuation_attempt_ids(&source.registration_id)
            .ok()?
            .is_empty()
            .then_some(())
    });
    let manual = f
        .command()
        .args([
            "notify",
            "agent-bash-recovery-read",
            "--event-id",
            &source.handle,
        ])
        .output()
        .unwrap();
    assert!(
        manual.status.success(),
        "{}",
        String::from_utf8_lossy(&manual.stderr)
    );
    let manual: serde_json::Value = serde_json::from_slice(&manual.stdout).unwrap();
    assert_eq!(manual["selected_output"]["kind"], "selected_missing_output");
    assert_eq!(
        manual["selected_output"]["evidence"],
        receipt["snapshot"]["output"]
    );
    let attempts = || {
        f.sidecar_connection().query_row("SELECT COUNT(*) FROM completion_continuation_attempt WHERE source_registration_id=?1", [&source.registration_id], |r| r.get::<_,i64>(0)).unwrap()
    };
    let before = attempts();
    // Experiment observation window only; product retains NoDeadline. Covers
    // multiple native retry intervals after exact acceptance and physical drain.
    std::thread::sleep(Duration::from_millis(2300));
    assert_eq!(attempts(), before);
    println!(
        "native recipient missing output={receipt}; original observation={observed}; settled source recovery attempts={before}"
    );
}

#[test]
fn native_transient_artifact_commit_recovers_after_cached_cleanup_failure() {
    if !private_case(false) {
        native_activation_channel_custody(9, Some("commit_failure"));
    }
}

#[test]
fn native_mailbox_resume_services_legacy_notification_without_owner() {
    if !private_case(false) {
        legacy_mailbox_service(false);
    }
}

#[test]
fn native_mailbox_resume_replaces_eligible_retained_legacy_claim() {
    if !private_case(false) {
        legacy_mailbox_service(true);
    }
}

fn legacy_mailbox_service(retained_claim: bool) {
    let f = Fixture::new("owner_only");
    let mut initial = f.start();
    let old_owner = f.owner();
    f.wait_initial(&mut initial);
    wait(|| {
        f.mailbox()
            .completion_continuation_owner()
            .unwrap()
            .is_none()
            .then_some(())
    });
    // Final-system legacy-shaped materialized notification. This is deliberately
    // not a source-recovery fixture or an inferred admission/accepted attempt.
    let mut db = MailboxDb::open(&f.data.join("pid-identity.db")).unwrap();
    db.set_notifications_paused(SESSION, true).unwrap();
    db.enqueue_agent_bash_complete(&oulipoly_state::mailbox::AgentBashCompleteEnqueue {
        session_id: SESSION,
        handle: "age365-legacy-mailbox",
        payload_json: r#"{"fixture":"legacy-materialized"}"#,
        owner_invocation_uuid: None,
        matched_os_pid: None,
        matched_os_boot_id: None,
        matched_os_pid_starttime_ticks: None,
        matched_chain_index: None,
        state_dir: "/synthetic",
        meta_path: "/synthetic/meta",
        log_path: "/synthetic/log",
        rc_path: "/synthetic/rc",
        rc: 0,
    })
    .unwrap();
    drop(db);
    if retained_claim {
        let c = rusqlite::Connection::open(f.data.join("pid-identity.db")).unwrap();
        c.execute("INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count,wake_pid) VALUES(?1,'legacy-retained','2026-09-13T00:00:00Z','fixture',1,999999999)", [SESSION]).unwrap();
    }
    // Read and ACK entrypoints are not owner election. A stale endpoint on a
    // reader must not turn inspection into failed service admission.
    let read = f
        .command()
        .env("OULIPOLY_COMPLETION_ENDPOINT", "/absent/owner.sock")
        .args(["mailbox", "list", "--session-id", SESSION, "--json"])
        .output()
        .unwrap();
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    assert!(
        f.mailbox()
            .completion_continuation_owner()
            .unwrap()
            .is_none()
    );
    assert_eq!(f.mailbox().list_pending(SESSION).unwrap().len(), 1);
    let output = f
        .command()
        .env("AGE365_LEGACY_WAKE", "1")
        .args(["mailbox", "resume", "--session-id", SESSION, "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let owner = f.owner();
    assert_ne!(owner.owner_generation, old_owner.owner_generation);
    wait(|| {
        f.root
            .path()
            .join("resume-prompts.jsonl")
            .exists()
            .then_some(())
    });
    let claim = f
        .mailbox()
        .wake_session_reader()
        .wake_claim(SESSION)
        .unwrap()
        .unwrap();
    assert_ne!(claim.claim_token, "legacy-retained");
    let attempt = f
        .mailbox()
        .continuation_activation(SESSION, &claim.claim_token)
        .unwrap()
        .unwrap();
    assert_eq!(attempt.owner_generation, owner.owner_generation);
    assert!(attempt.source_registration_id.is_none());
    f.gate("release-resume");
    wait(|| {
        f.root
            .path()
            .join("recipient-exact-ack.json")
            .exists()
            .then_some(())
    });
    wait(|| {
        f.mailbox()
            .wake_session_reader()
            .wake_claim(SESSION)
            .unwrap()
            .is_none()
            .then_some(())
    });
    assert!(f.mailbox().list_pending(SESSION).unwrap().is_empty());
    let c = f.sidecar_connection();
    let phase: String = c
        .query_row(
            "SELECT phase FROM completion_continuation_attempt WHERE attempt_id=?1",
            [&attempt.attempt_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(phase, "drained");
    let count: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM completion_continuation_attempt WHERE session_id=?1",
            [SESSION],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "no fabricated legacy attempt or duplicate activation"
    );
    println!(
        "legacy servicing: retained_claim={retained_claim} exact native attempt={} ACK and original physical drain",
        attempt.attempt_id
    );
}

/// Hold a real retiring driver's physical lifetime, rather than delaying the
/// caller or relying on a naturally occurring hello/close race.
#[test]
fn native_retiring_owner_admits_first_sequential_fresh_and_resume_after_drain() {
    if private_case(false) {
        return;
    }
    for resume in [false, true] {
        let f = Fixture::new("owner_only");
        // Instrument only this private generated provider, not product code.
        let provider = f.root.path().join("provider.py");
        let source = fs::read_to_string(&provider).unwrap();
        fs::write(&provider, source.replace("def launch(request):", "def launch(request):\n    with pathlib.Path(__file__).parent.joinpath('boundary-provider-effects').open('a') as effects:\n        effects.write(request['request_id'] + '\\n')")).unwrap();
        let effect_count = || {
            fs::read_to_string(f.root.path().join("boundary-provider-effects"))
                .unwrap()
                .lines()
                .count()
        };
        let mut initial = f.start_with_hold(true);
        let owner = f.owner();
        wait(|| {
            f.root
                .path()
                .join("provider-initial-ready")
                .exists()
                .then_some(())
        });
        assert!(current_identity_matches(&owner.driver_identity));
        assert_eq!(
            unsafe { libc::kill(owner.driver_identity.pid as i32, libc::SIGSTOP) },
            0
        );
        // Observe the actual stopped driver before releasing the original
        // context. It cannot disappear before we exercise closing admission.
        wait(|| {
            let stat =
                fs::read_to_string(format!("/proc/{}/stat", owner.driver_identity.pid)).ok()?;
            (stat.rsplit_once(')')?.1.split_whitespace().next()? == "T").then_some(())
        });
        f.gate("release-initial-provider");
        f.wait_initial(&mut initial);
        wait(|| {
            let phase: String = f
                .sidecar_connection()
                .query_row(
                    "SELECT phase FROM completion_continuation_owner WHERE generation=?1",
                    [&owner.owner_generation],
                    |r| r.get(0),
                )
                .ok()?;
            (phase == "closing").then_some(())
        });
        f.gate("release-resume");
        let mut command = f.command();
        if resume {
            command.args([
                "resume",
                "--session-id",
                SESSION,
                "--prompt",
                "first sequential launch across retirement",
            ]);
        } else {
            command.args(["-m", MODEL, "first sequential launch across retirement"]);
        }
        let mut incoming = command
            .arg("--models-dir")
            .arg(&f.models)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let gate_path = PathBuf::from(&owner.endpoint).with_file_name("admission.lock");
        wait(|| {
            assert!(
                incoming.try_wait().unwrap().is_none(),
                "first entry failed during retirement"
            );
            let syscall = fs::read_to_string(format!("/proc/{}/syscall", incoming.id())).ok()?;
            let mut fields = syscall.split_whitespace();
            if fields.next()?.parse::<i64>().ok()? != libc::SYS_flock {
                return None;
            }
            let fd = i32::from_str_radix(fields.next()?.trim_start_matches("0x"), 16).ok()?;
            (fs::read_link(format!("/proc/{}/fd/{fd}", incoming.id())).ok()? == gate_path)
                .then_some(())
        });
        assert!(current_identity_matches(&owner.driver_identity));
        // No new generation or provider effects while old physical custody is
        // held. The resumed provider records every execution in this fixture.
        assert!(
            f.mailbox()
                .completion_continuation_owner()
                .unwrap()
                .is_none()
        );
        assert!(!f.root.path().join("resume-prompts.jsonl").exists());
        assert_eq!(
            effect_count(),
            1,
            "no provider effect before physical drain"
        );
        println!(
            "resume={resume} closing generation={} entrant={} blocked_on=admission.lock driver=stopped",
            owner.owner_generation,
            incoming.id()
        );
        assert_eq!(
            unsafe { libc::kill(owner.driver_identity.pid as i32, libc::SIGCONT) },
            0
        );
        let output = incoming.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            output.stdout,
            if resume {
                b"native resumed\n".as_slice()
            } else {
                b"native initial\n".as_slice()
            }
        );
        assert_eq!(
            effect_count(),
            2,
            "accepted entry executes its provider exactly once"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.starts_with("OULIPOLY_RESULT="))
                .count(),
            1
        );
        if resume {
            assert_eq!(
                fs::read_to_string(f.root.path().join("resume-prompts.jsonl"))
                    .unwrap()
                    .lines()
                    .count(),
                1
            );
        }
        assert!(
            !current_identity_matches(&owner.driver_identity),
            "old driver must actually be reaped before admission"
        );
        println!(
            "resume={resume} first_entry=success old_driver=reaped output_bytes={}",
            output.stdout.len()
        );
    }
}
