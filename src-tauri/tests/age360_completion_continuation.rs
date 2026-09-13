//! Native runner + explicitly supplied Bash candidate, never a fake enqueue helper.
//! Each case re-execs in private user/network/PID/mount namespaces. PID namespace
//! teardown contains failure descendants; product NoDeadline is not modified.
#![cfg(target_os = "linux")]
mod provider_authority_fixture;
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
        .env("AGE360_PARENT_NET", &net);
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
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
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
            .env(
                "AGENT_BASH_AGENT_RUNNER_BIN",
                env!("CARGO_BIN_EXE_oulipoly-agent-runner"),
            )
            .current_dir(self.root.path());
        if self.case != "owner_only" {
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
        MailboxDb::open_read_only(&self.data.join("pid-identity.db")).unwrap()
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
            MailboxDb::open_read_only(&self.data.join("pid-identity.db"))
                .ok()?
                .completion_continuation_owner()
                .ok()?
        })
    }
    fn source(&self) -> oulipoly_state::completion_continuation::AdmittedSourceBinding {
        wait(|| {
            oulipoly_state::StateDb::open_read_only(&self.data.join("state.db"))
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
    f.gate("release-resume");
    let mut initial = f.start();
    let owner = f.owner();
    let binding = f.source();
    let source = binding.registration().unwrap();
    assert_eq!(source.domain_id, owner.domain_id);
    assert_eq!(source.owner_session_id, SESSION);
    println!(
        "owner={} source={}",
        serde_json::to_string(&owner).unwrap(),
        serde_json::to_string(&source).unwrap()
    );
    if mode == "async" {
        f.wait_initial(&mut initial);
        f.gate("release-workload");
    }
    let acceptance = wait(|| {
        let value = f
            .mailbox()
            .completion_continuation_acceptance(&source.registration_id)
            .ok()??;
        (value["phase"] == "accepted").then_some(value)
    });
    println!("acceptance={acceptance}");
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
        assert!(listeners[0].mailbox_seq.is_some());
        assert!(
            listeners[0].acknowledged_at.is_none(),
            "local synchronous bytes are not mailbox ACK"
        );
        f.gate("release-initial-provider");
        f.wait_initial(&mut initial);
    }
    wait(|| {
        let listeners = f
            .mailbox()
            .completion_event_listeners(&source.handle)
            .ok()?;
        listeners
            .iter()
            .all(|l| l.acknowledged_at.is_some())
            .then_some(())
    });
    // Both modes have completed their mode-specific initial observation above.
    assert!(initial.wait().unwrap().success());
    let prompt = fs::read_to_string(f.root.path().join("resume-prompts.jsonl")).unwrap();
    assert!(
        prompt.contains(&source.handle),
        "native resumed recipient did not receive exact source"
    );
    let listeners = f
        .mailbox()
        .completion_event_listeners(&source.handle)
        .unwrap();
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
        f.mailbox()
            .pending_continuation_attempt_ids(&source.registration_id)
            .ok()?
            .is_empty()
            .then_some(())
    });
}
#[test]
fn normal_sleeping_recipient() {
    if private_case(true) {
        return;
    }
    paired_case("async");
}
#[test]
fn sync_receipt_without_ack() {
    if private_case(true) {
        return;
    }
    paired_case("sync");
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
fn native_activation_custody(owner_loss: u8) {
    let f = Fixture::new("owner_only");
    f.gate("test-descendant-enabled");
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
    assert!(read_live_process_identity(descendant).unwrap().is_some());
    println!(
        "ACK before drain owner={} attempt={} custodian={custodian} launcher={launcher} generation={generation} descendant={descendant}",
        serde_json::to_string(&owner).unwrap(),
        serde_json::to_string(&attempt).unwrap()
    );
    if owner_loss != 0 {
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
            (current.owner_generation != owner.owner_generation).then_some(current)
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
        assert_eq!(phase, "unknown_custody");
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
    let mut writer = MailboxDb::open(&f.data.join("pid-identity.db")).unwrap();
    assert!(!matches!(
        writer
            .wake_sessions()
            .release_wake_claim_for_manual_resume(SESSION, &claim.claim_token),
        Ok(true)
    ));
    drop(writer);
    drop(mailbox);
    f.gate("release-descendant");
    wait(|| {
        f.mailbox()
            .wake_session_reader()
            .wake_claim(SESSION)
            .ok()?
            .is_none()
            .then_some(())
    });
    let receipt:(String,i64,String)=f.sidecar_connection().query_row("SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE attempt_id=?1",[attempt.attempt_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(receipt.0, "drained");
    assert_eq!(receipt.1, 1);
    assert!(receipt.2.contains("ECHILD"));
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
