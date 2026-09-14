//! Requires the caller's masked namespace wrapper and explicit source-built CLI.
//! Fixture deadlines contain tests only; no product timeout or fake drain exists.
#![cfg(target_os = "linux")]
mod provider_authority_fixture;
use oulipoly_state::mailbox::{InboxTarget, MailboxDb, SubmittedInputEnqueue};
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
const SESSION: &str = "ses_age360_manual_arbitration";
const PROMPT: &str = "MANUAL-EXACT-INPUT\nsecond line with spaces  ";

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
struct Fixture {
    root: tempfile::TempDir,
    runner: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        assert!(
            std::env::var_os("AGE360_RUNNER_BIN").is_some(),
            "explicit staged CLI required"
        );
        let runner = fs::canonicalize(std::env::var_os("AGE360_RUNNER_BIN").unwrap()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config/oulipoly-agent-runner");
        fs::create_dir_all(config.join("models")).unwrap();
        fs::create_dir(root.path().join("home")).unwrap();
        fs::write(
            root.path().join("provider.py"),
            include_str!("fixtures/age360-manual/provider.py"),
        )
        .unwrap();
        let wrapper = root.path().join("provider.sh");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nexec /usr/bin/python3 '{}' \"$@\"\n",
                root.path().join("provider.py").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            config.join("models/manual.toml"),
            "prompt_mode='arg'\n[[providers]]\nname='manual-fixture'\nargs=[]\n",
        )
        .unwrap();
        fs::write(config.join("providers.toml"),provider_authority_fixture::with_explicit_provider_authority_at(
            "[manual-fixture]\ncommand='manual-fixture'\nargs=[]\nprompt_mode='arg'\nsettings_id='manual-fixture'\n", "manual-arbitration", &wrapper)).unwrap();
        Self { root, runner }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(&self.runner);
        c.env_clear()
            .env("AGE360_MANUAL_RUNNER", &self.runner)
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("OULIPOLY_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("OULIPOLY_DATA_DIR", self.root.path().join("data"))
            .current_dir(self.root.path());
        c
    }
    fn spawn(&self, c: &mut Command, label: &str) -> OwnedChild {
        OwnedChild(
            c.stdout(fs::File::create(self.root.path().join(format!("{label}.stdout"))).unwrap())
                .stderr(fs::File::create(self.root.path().join(format!("{label}.stderr"))).unwrap())
                .spawn()
                .unwrap(),
        )
    }
    fn gate(&self, name: &str) {
        fs::write(self.root.path().join(name), b"release").unwrap();
    }
    fn exists(&self, name: &str) -> bool {
        self.root.path().join(name).exists()
    }
    fn sql(&self) -> rusqlite::Connection {
        rusqlite::Connection::open_with_flags(
            self.root.path().join("data/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
    }
    fn launches(&self) -> Vec<Value> {
        fs::read_to_string(self.root.path().join("launches.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for file in [
            "initial.stdout",
            "initial.stderr",
            "manual.stdout",
            "manual.stderr",
            "launches.jsonl",
        ] {
            if let Ok(text) = fs::read_to_string(self.root.path().join(file)) {
                println!("EVIDENCE {file}: {text}");
            }
        }
        if self.root.path().join("data/pid-identity.db").exists() {
            let sql = self.sql();
            for query in [
                "SELECT json_object('attempt',attempt_id,'phase',phase,'integrated',integrated,'receipt',drain_receipt,'launcher',launcher_identity,'custodian',custodian_identity,'generation',runtime_generation_uuid) FROM completion_continuation_attempt",
                "SELECT json_object('invocation',registration_identity,'state',state,'session',session_id,'pid',launcher_os_pid) FROM session_admission_queue",
            ] {
                if let Ok(mut statement) = sql.prepare(query)
                    && let Ok(rows) = statement.query_map([], |r| r.get::<_, String>(0))
                {
                    for row in rows {
                        println!("EVIDENCE SQL: {row:?}");
                    }
                }
            }
        }
        // Gates release private descendants even when an assertion unwinds.
        for gate in ["release-initial", "release-automatic", "release-descendant"] {
            self.gate(gate);
        }
    }
}
#[track_caller]
fn wait(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(45);
    while !predicate() {
        assert!(Instant::now() < end, "fixture observation expired");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn progress(mode: &str) {
    let f = Fixture::new();
    if mode == "policy" {
        f.gate("policy-transform-manual");
    }
    let mut initial = f.spawn(f.command().args(["-m", "manual", "initial"]), "initial");
    wait(|| f.exists("initial-ready"));
    let mut db = MailboxDb::open(&f.root.path().join("data/pid-identity.db")).unwrap();
    db.enqueue_submitted_input(&SubmittedInputEnqueue {
        submission_token: "automatic-token",
        target: InboxTarget {
            kind: oulipoly_state::InboxTargetKind::Session,
            id: SESSION,
        },
        input: b"AUTO-CUSTODY-INPUT",
    })
    .unwrap();
    drop(db);
    f.gate("release-initial");
    wait(|| initial.0.try_wait().unwrap().is_some());
    assert!(initial.0.wait().unwrap().success());
    wait(|| f.exists("automatic-ready"));
    let mut command = f.command();
    command.args(["resume", "--session-id", SESSION]);
    match mode {
        "file" => {
            let file = f.root.path().join("answer.txt");
            fs::write(&file, PROMPT).unwrap();
            command.arg("--file").arg(file);
        }
        "stdin" => {
            command.stdin(Stdio::piped());
        }
        "token" | "policy" => {
            command.args(["--submission-token", "manual-token", "--prompt", PROMPT]);
        }
        _ => {
            command.args(["--prompt", PROMPT]);
        }
    }
    let mut manual = f.spawn(&mut command, "manual");
    if mode == "stdin" {
        use std::io::Write;
        manual
            .0
            .stdin
            .take()
            .unwrap()
            .write_all(PROMPT.as_bytes())
            .unwrap();
    }
    wait(|| {
        f.sql().query_row("SELECT EXISTS(SELECT 1 FROM session_admission_queue WHERE launcher_os_pid=?1 AND state='queued')",[i64::from(manual.0.id())],|r|r.get::<_,bool>(0)).unwrap()
    });
    assert_eq!(
        f.launches().len(),
        2,
        "manual provider must not run behind automatic provider"
    );
    if matches!(mode, "token" | "policy") {
        let count: i64 = f
            .sql()
            .query_row(
                "SELECT count(*) FROM mailbox WHERE submission_token='manual-token'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "waiting input must remain private");
    }
    f.gate("release-automatic");
    wait(|| f.exists("descendant.pid"));
    // Exact input acceptance is deliberately earlier than native physical
    // custody drain. Runtime may remain draining while the descendant lives.
    wait(|| {
        f.sql().query_row("SELECT EXISTS(SELECT 1 FROM mailbox WHERE submission_token='automatic-token' AND delivered_at IS NOT NULL)", [], |r| r.get::<_,bool>(0)).unwrap()
    });
    assert!(manual.0.try_wait().unwrap().is_none());
    assert_eq!(f.launches().len(), 2);
    let descendant: i64 = fs::read_to_string(f.root.path().join("descendant.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        oulipoly_state::pid_identity::read_live_process_identity(descendant)
            .unwrap()
            .is_some()
    );
    let debt:i64=f.sql().query_row("SELECT count(*) FROM completion_continuation_attempt WHERE operation='activation' AND integrated=0",[],|r|r.get(0)).unwrap();
    assert_eq!(debt, 1);
    if mode == "owner-replacement" {
        let db = MailboxDb::open_read_only(&f.root.path().join("data/pid-identity.db")).unwrap();
        let owner = db.completion_continuation_owner().unwrap().unwrap();
        let exact =
            oulipoly_state::pid_identity::read_live_process_identity(owner.guardian_identity.pid)
                .unwrap()
                .unwrap();
        assert_eq!(exact.os_boot_id, owner.guardian_identity.boot_id);
        assert_eq!(
            exact.os_pid_starttime_ticks,
            owner.guardian_identity.starttime_ticks
        );
        assert_eq!(unsafe { libc::kill(exact.os_pid as i32, libc::SIGKILL) }, 0);
        wait(|| {
            MailboxDb::open_read_only(&f.root.path().join("data/pid-identity.db"))
                .unwrap()
                .completion_continuation_owner()
                .unwrap()
                .is_some_and(|next| next.owner_generation != owner.owner_generation)
        });
        assert!(
            manual.0.try_wait().unwrap().is_none(),
            "owner replacement is not release authority"
        );
        assert_eq!(f.launches().len(), 2);
    }
    f.gate("release-descendant");
    wait(|| manual.0.try_wait().unwrap().is_some());
    let status = manual.0.wait().unwrap();
    if mode != "policy" {
        assert!(status.success());
    }
    println!("manual wait status mode={mode}: {status}");
    let launches = f.launches();
    assert_eq!(launches.len(), 3);
    let prompt = launches[2]["prompt"].as_str().unwrap();
    if mode == "policy" {
        assert_eq!(prompt, "POLICY-TRANSFORMED-MANUAL");
        assert!(
            launches[2]["prompt_acceptance"].is_null(),
            "policy transformation must clear provider correlation"
        );
        let marker: (String, Option<String>) = f.sql().query_row(
            "SELECT headless_submission_state,submission_started_at FROM mailbox_delivery_attempts WHERE delivery_invocation_uuid=?1",
            [launches[2]["invocation"].as_str().unwrap()], |r| Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        assert_eq!(marker.0, "possible");
        assert!(
            marker.1.is_some(),
            "host submission authority must survive policy transform"
        );
    } else if mode == "token" {
        assert!(prompt.contains(PROMPT.trim_end()));
    } else {
        assert_eq!(prompt, PROMPT);
    }
    assert!(
        !prompt.contains("AUTO-CUSTODY-INPUT"),
        "settled automatic input must not be replayed"
    );
    assert!(
        fs::read_to_string(f.root.path().join("manual.stdout"))
            .unwrap()
            .starts_with("manual-result\n")
    );
    let drain:(String,i64,Option<String>)=f.sql().query_row("SELECT phase,integrated,drain_receipt FROM completion_continuation_attempt WHERE operation='activation'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(drain.0, "drained");
    assert_eq!(drain.1, 1);
    assert!(drain.2.is_some());
    println!(
        "mode={mode} exact launches={} integrated_drain={drain:?}",
        serde_json::to_string(&launches).unwrap()
    );
}
#[test]
fn manual_inline_waits_for_actual_native_drain() {
    progress("inline");
}
#[test]
fn manual_file_waits_for_actual_native_drain() {
    progress("file");
}
#[test]
fn manual_stdin_waits_for_actual_native_drain() {
    progress("stdin");
}
#[test]
fn manual_token_is_not_published_until_native_drain() {
    progress("token");
}

#[test]
fn manual_wait_survives_native_owner_replacement_with_original_drain() {
    progress("owner-replacement");
}

#[test]
fn transformed_policy_keeps_host_owned_possible_submission() {
    progress("policy");
}
