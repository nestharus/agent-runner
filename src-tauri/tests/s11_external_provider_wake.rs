#![cfg(unix)]

//! ## Declared roles
//!
//! Roles: orchestration, formatter, accessor, parser, validator.
//!
//! TEST: external-provider runtime fixtures for session ingestion, prompt
//! acceptance, and policy diagnostics.

#[path = "fixtures/bounded_runner_image.rs"]
mod bounded_runner_image;
mod provider_authority_fixture;

use oulipoly_state::mailbox::{MailboxDb, MailboxRow};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const MODEL: &str = "s11-external-wake-model";
const PROVIDER: &str = "opencode";
const OPENCODE_PROVIDERS: [&str; 3] = ["opencode", "opencode2", "opencode5"];
const SESSION: &str = "ses_s11externalwake";

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    home_dir: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    work_dir: PathBuf,
    provider: &'static str,
}

impl Fixture {
    fn new() -> Self {
        Self::with_provider(PROVIDER)
    }

    fn with_provider(provider: &'static str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("xdg-config");
        let data_home = dir.path().join("xdg-data");
        let home_dir = dir.path().join("home");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let work_dir = dir.path().join("work");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&work_dir).unwrap();
        Self {
            dir,
            config_home,
            data_home,
            home_dir,
            app_config_dir,
            models_dir,
            work_dir,
            provider,
        }
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.home_dir)
            .env("S11_WORK_DIR", &self.work_dir)
            .env(
                "OULIPOLY_DATA_DIR",
                self.data_home.join("oulipoly-agent-runner"),
            )
            .env_remove("OULIPOLY_AUTO_WAKE")
            .env_remove("OULIPOLY_AUTO_WAKE_SESSION_ID")
            .env_remove("OULIPOLY_AUTO_WAKE_TOKEN")
            .env_remove("OULIPOLY_AUTO_WAKE_COUNT")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .current_dir(self.dir.path());
        let command = format!("{cmd:?}");
        let output = cmd.output().unwrap();
        eprintln!(
            "S11_COMMAND case={:?} fixture={} command={} status={:?} stdout_hex={} stderr_hex={}\nstdout={}\nstderr={}",
            std::thread::current().name(),
            self.dir.path().display(),
            command,
            output.status,
            hex_bytes(&output.stdout),
            hex_bytes(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // Do not insert a diagnostic subprocess between seed return and enqueue:
        // the independent owner may legitimately retire while no work exists.
        if cmd.get_args().any(|arg| arg == "resume") {
            self.capture_diagnostics("command-return");
        }
        output
    }

    fn capture_diagnostics(&self, phase: &str) {
        let capture = Command::new("/usr/bin/python3")
            .arg("-c")
            .arg(include_str!("fixtures/s11_diagnostics.py"))
            .arg(self.dir.path())
            .output();
        eprintln!(
            "S11_SNAPSHOT case={:?} phase={phase} fixture={} capture_completed={}",
            std::thread::current().name(),
            self.dir.path().display(),
            capture.is_ok()
        );
        if let Ok(capture) = capture {
            eprintln!(
                "S11_SNAPSHOT_ROWS status={:?} stderr={}\n{}",
                capture.status,
                String::from_utf8_lossy(&capture.stderr),
                String::from_utf8_lossy(&capture.stdout)
            );
        }
    }

    fn run_agent_with_env(&self, prompt: &str, envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(runner_bin());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.arg("-m")
            .arg(MODEL)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(prompt);
        self.run(cmd)
    }

    fn run_resume_with_env(&self, prompt: &str, envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(runner_bin());
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.arg("resume")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg("--session-id")
            .arg(SESSION)
            .arg("--prompt")
            .arg(prompt);
        self.run(cmd)
    }

    fn write_external_provider(&self) {
        let provider = self.provider;
        let provider_path = self.write_script("external-provider.py", external_provider_script());
        self.write_script(
            "slow_control.py",
            include_str!("fixtures/s11_slow_control.py"),
        );
        self.write_script("s11_ingress.py", include_str!("fixtures/s11_ingress.py"));
        self.write_script(
            "s11_diagnostics.py",
            include_str!("fixtures/s11_diagnostics.py"),
        );
        self.write_script(
            "s11_recipient_control.py",
            include_str!("fixtures/s11_recipient_control.py"),
        );
        let turn_script_path = self.write_script("s11-turns.py", turn_script());
        fs::write(
            self.models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"provider = {{ path = {} }}
prompt_mode = "arg"

[[providers]]
name = "{provider}"
args = []
"#,
                toml_string(&path_string(&provider_path))
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("providers.toml"),
            provider_authority_fixture::with_explicit_provider_authority_at(
                &format!(
                    r#"[{provider}]
command = "fixture-opencode"
args = []
prompt_mode = "arg"
"#
                ),
                "s11-external-provider",
                &provider_path,
            ),
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("sessions.toml"),
            format!(
                r#"[{provider}]
turn_script = {}
"#,
                toml_string(&path_string(&turn_script_path))
            ),
        )
        .unwrap();
    }

    fn remove_turn_script_fallback(&self) {
        fs::remove_file(self.app_config_dir.join("sessions.toml")).unwrap();
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(&path, body).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn mailbox(&self) -> MailboxDb {
        MailboxDb::open(&self.sidecar_path()).unwrap()
    }

    fn finalized_invocation_count(&self) -> i64 {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM invocations WHERE finished_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn latest_resume_acceptance(&self) -> (Option<String>, Option<String>) {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT resume_acceptance_status, resume_acceptance_evidence
                 FROM invocations
                 WHERE resume_input_id IS NOT NULL
                 ORDER BY id DESC
                 LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
            .expect("expected a resumed invocation row")
    }

    fn latest_invocation_uuid(&self) -> String {
        Connection::open(self.state_path())
            .unwrap()
            .query_row(
                "SELECT invocation_uuid FROM invocations ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn mailbox_row(&self, seq: i64) -> MailboxRow {
        self.mailbox()
            .list_mailbox(SESSION, true)
            .unwrap()
            .into_iter()
            .find(|row| row.seq == seq)
            .unwrap()
    }

    fn recorded_resume_prompts(&self) -> Vec<String> {
        let path = self.work_dir.join("resume-prompts.jsonl");
        if !path.exists() {
            return Vec::new();
        }
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn assert_xdg_isolated(&self) {
        assert!(
            !self
                .home_dir
                .join(".local/share/oulipoly-agent-runner")
                .exists(),
            "state must stay in isolated XDG_DATA_HOME"
        );
        assert!(
            !self.home_dir.join(".config/oulipoly-agent-runner").exists(),
            "config must stay in isolated XDG_CONFIG_HOME"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.capture_diagnostics("assertion-failure");
        }
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn assert_unconfirmed_resume(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "{stdout}");
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let result: Value = serde_json::from_str(lines[0]).unwrap();
    let mut keys = result
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        [
            "agent_runner_chain_id",
            "agent_runner_invocation_id",
            "error_category",
            "exit_code",
            "finished_at",
            "id",
            "provider_name",
            "provider_session_id",
            "status",
            "success",
            "terminal_reason"
        ]
    );
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["error_category"], "resume_completion_unconfirmed");
    assert_eq!(result["terminal_reason"], "resume_completion_unconfirmed");
    assert_eq!(result["provider_name"], PROVIDER);
    assert_eq!(result["provider_session_id"], SESSION);
    assert_eq!(result["agent_runner_invocation_id"], result["id"]);
}

#[test]
fn nested_external_provider_cannot_inherit_parent_live_session_binding() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    let output = fixture.run_agent_with_env(
        "launch child with its own invocation authority",
        &[
            ("S11_CHECK_LIVE_BINDING_ISOLATION", "1"),
            ("OULIPOLY_LIVE_SESSION_BIND_SOCKET", "/parent-owner.sock"),
            ("OULIPOLY_LIVE_SESSION_BIND_TOKEN", "parent-owner-token"),
        ],
    );
    assert_success(&output);
    let child_invocation = fs::read_to_string(fixture.work_dir.join("child-invocation")).unwrap();
    assert_eq!(child_invocation, fixture.latest_invocation_uuid());
    fixture.assert_xdg_isolated();
}

#[test]
fn external_provider_runtime_uses_ingested_session_when_launch_capture_missing() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider without launch session capture",
        &[("S11_OMIT_EXIT_SESSION", "1")],
    );
    assert_success(&output);

    wait_until("initial invocation finalized", || {
        fixture.finalized_invocation_count() >= 1
    });
    let runtime = fixture
        .mailbox()
        .wake_session_reader()
        .session_metadata(SESSION)
        .unwrap()
        .unwrap();
    let expected_models_dir = path_string(&fixture.models_dir);
    assert_eq!(runtime.provider_name.as_deref(), Some(PROVIDER));
    assert_eq!(runtime.model_name.as_deref(), Some(MODEL));
    assert_eq!(
        runtime.models_dir.as_deref(),
        Some(expected_models_dir.as_str())
    );
    fixture.assert_xdg_isolated();
}

#[test]
fn prompt_acceptance_hash_accepts_exact_and_rejects_mismatch_without_delivery_nonce() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    assert_success(&fixture.run_agent_with_env("seed manual resume", &[]));

    let output = fixture.run_resume_with_env(
        "manual exact payload",
        &[("S11_EMIT_PROMPT_ACCEPTANCE_MARKER", "1")],
    );
    assert_unconfirmed_resume(&output);
    assert_eq!(
        fixture.latest_resume_acceptance(),
        (None, None),
        "exact-prompt acceptance must not adopt the session-resume acceptance identity"
    );

    let fixture = Fixture::new();
    fixture.write_external_provider();
    assert_success(&fixture.run_agent_with_env("seed hash mismatch", &[]));
    let output = fixture.run_resume_with_env(
        "manual hash mismatch",
        &[
            ("S11_EMIT_PROMPT_ACCEPTANCE_MARKER", "1"),
            ("S11_MARKER_PROMPT_SHA_MISMATCH", "1"),
        ],
    );
    assert_unconfirmed_resume(&output);
    assert_eq!(
        fixture.latest_resume_acceptance(),
        (None, None),
        "prompt hash mismatch without a nonce"
    );
}

#[test]
fn prompt_acceptance_marker_requires_declared_capability() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    assert_success(&fixture.run_agent_with_env(
        "seed undeclared attestation",
        &[("S11_OMIT_PROMPT_ACCEPTANCE_CAPABILITY", "1")],
    ));

    let output = fixture.run_resume_with_env(
        "manual undeclared attestation",
        &[
            ("S11_EMIT_PROMPT_ACCEPTANCE_MARKER", "1"),
            ("S11_OMIT_PROMPT_ACCEPTANCE_CAPABILITY", "1"),
        ],
    );

    assert_unconfirmed_resume(&output);
    assert_eq!(fixture.latest_resume_acceptance(), (None, None));
}

// Linux /proc evidence identifies the real contained terminal helper, not the
// CLI chosen by the test. Automatic ownership and supersession stay enabled.
#[cfg(target_os = "linux")]
#[test]
fn hidden_nonterminal_page_after_append_preserves_terminal_checkpoint() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    fs::write(
        fixture.dir.path().join("slow-control.json"),
        serde_json::json!({
            "work_dir": fixture.work_dir, "wrong_digest": false
        })
        .to_string(),
    )
    .unwrap();
    let mut command = Command::new("/usr/bin/python3");
    command
        .arg("-c")
        .arg(include_str!("fixtures/s11_visibility_regression.py"))
        .arg(fixture.dir.path());
    assert_success(&fixture.run(command));
}

#[cfg(target_os = "linux")]
#[test]
fn bounded_post_anchor_user_observation_confirms_mailbox_without_attestation_or_turn_script() {
    slow_page_control(false);
}

#[cfg(target_os = "linux")]
#[test]
fn slow_post_anchor_wrong_digest_cannot_confirm_receipt() {
    slow_page_control(true);
}

#[cfg(target_os = "linux")]
fn slow_page_control(wrong_digest: bool) {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    fixture.remove_turn_script_fallback();
    // First observe normal retirement with no admitted source. The next real
    // producer enters through the supported CLI, never a forced owner restart.
    assert_success(&fixture.run_agent_with_env("idle predecessor before ingress", &[]));
    let db = Connection::open(fixture.sidecar_path()).unwrap();
    wait_until("idle predecessor closing before producer ingress", || {
        db.query_row(
            "SELECT phase='closing' FROM completion_continuation_owner",
            [],
            |r| r.get::<_, bool>(0),
        )
        .unwrap_or(false)
    });
    let predecessor: String = db
        .query_row(
            "SELECT generation FROM completion_continuation_owner WHERE phase='closing'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    eprintln!("SLOW_INGRESS observed_predecessor_closing={predecessor}");
    let agent_bash = std::env::var("AGE360_AGENT_BASH_BIN")
        .expect("explicit source-built agent-bash required; no installed fallback");
    fs::write(
        fixture.dir.path().join("slow-control.json"),
        serde_json::json!({
            "work_dir": fixture.work_dir, "sidecar": fixture.sidecar_path(),
            "runner": runner_bin(), "agent_bash": agent_bash, "admit_source": true,
            "wrong_digest": wrong_digest
        })
        .to_string(),
    )
    .unwrap();
    let seed = fixture.run_agent_with_env("owner admits actual detached completion", &[]);
    assert_success(&seed);
    let owner = result_envelope(&seed)["id"].as_str().unwrap().to_owned();
    let ingress: Value =
        serde_json::from_slice(&fs::read(fixture.work_dir.join("ingress-result.json")).unwrap())
            .unwrap();
    let registration: Value = serde_json::from_slice(
        &fs::read(fixture.work_dir.join("ingress-registration.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(ingress["rc"], 0);
    assert_eq!(ingress["owner"], owner);
    assert_eq!(registration["owner_invocation_uuid"], owner);
    let successor = fixture
        .mailbox()
        .completion_continuation_owner()
        .unwrap()
        .unwrap();
    assert_ne!(successor.owner_generation, predecessor);
    assert_eq!(registration["domain_id"], successor.domain_id);
    let handle = registration["handle"].as_str().unwrap();
    assert!(
        fixture
            .mailbox()
            .completion_event(handle)
            .unwrap()
            .is_some(),
        "actual producer registration must remain discoverable"
    );
    eprintln!("SLOW_INGRESS actual={ingress} registration={registration} successor={successor:?}");
    // The source producer, not a low-level post-seed insertion, creates this
    // notification. Seed/provider custody may legitimately await source drain.
    let mut notification_seq = None;
    wait_until("admitted source notification materialized", || {
        notification_seq = db
            .query_row("SELECT seq FROM mailbox WHERE handle=?1", [handle], |r| {
                r.get::<_, i64>(0)
            })
            .optional()
            .unwrap();
        notification_seq.is_some()
    });
    let notification = fixture.mailbox_row(notification_seq.unwrap());
    assert_eq!(
        notification.owner_invocation_uuid.as_deref(),
        Some(owner.as_str())
    );
    assert_eq!(
        fs::read_to_string(fixture.work_dir.join("source-launches")).unwrap(),
        "launch\n"
    );
    // No explicit resume competitor: the actual automatic owner submits.
    wait_until("automatic submitted invocation finalized", || {
        let db = Connection::open(fixture.sidecar_path()).unwrap();
        let invocation: Option<String> = db
            .query_row(
                "SELECT a.delivery_invocation_uuid FROM mailbox_delivery_attempts a
             JOIN mailbox_delivery_attempt_items i USING(attempt_id)
             WHERE i.mailbox_seq=?1 AND a.submission_started_at IS NOT NULL",
                [notification.seq],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        invocation.is_some_and(|id| {
            Connection::open(fixture.state_path())
                .unwrap()
                .query_row(
                    "SELECT finished_at IS NOT NULL FROM invocations WHERE invocation_uuid=?1",
                    [id],
                    |r| r.get::<_, bool>(0),
                )
                .unwrap_or(false)
        })
    });
    let db = Connection::open(fixture.sidecar_path()).unwrap();
    let attempts: Vec<Value> = db
        .prepare(
            "SELECT json_object('attempt_id',a.attempt_id,'invocation',delivery_invocation_uuid,
         'anchor',observation_anchor_token,'digest',observation_expected_sha256,
         'turn',observation_confirmed_turn_id,'confirmed',observation_confirmed_at,
         'ack',acknowledged_at) FROM mailbox_delivery_attempts a
         JOIN mailbox_delivery_attempt_items i USING(attempt_id)
         WHERE i.mailbox_seq=?1 AND submission_started_at IS NOT NULL",
        )
        .unwrap()
        .query_map([notification.seq], |r| r.get::<_, String>(0))
        .unwrap()
        .map(|r| serde_json::from_str(&r.unwrap()).unwrap())
        .collect();
    let events: Vec<Value> = fs::read_to_string(fixture.work_dir.join("slow-events.jsonl"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    eprintln!("SLOW_CONTROL attempts={attempts:?} events={events:?}");
    assert_eq!(attempts.len(), 1, "one actual submission per notification");
    let a = &attempts[0];
    let invocation = a["invocation"].as_str().unwrap();
    wait_until("submitted runtime physically exited", || {
        db.query_row("SELECT COUNT(*) FROM runtime_generation WHERE spawn_invocation_uuid=?1 AND lifecycle_state='exited'",
            [invocation], |r| r.get::<_, i64>(0)).unwrap() == 1
    });
    let generation: String = db.query_row("SELECT json_object('id',generation_uuid,'exit_code',exit_code,'reason',terminal_reason) FROM runtime_generation WHERE spawn_invocation_uuid=?1",
        [invocation], |r| r.get(0)).unwrap();
    eprintln!("SLOW_CONTROL generation={generation}");
    let launches: Vec<_> = events
        .iter()
        .filter(|e| e["kind"] == "launch_exit")
        .collect();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0]["invocation"], invocation);
    assert_eq!(launches[0]["assistant_result"], false);
    assert_eq!(launches[0]["provider_exit"], 0);
    let mut native_result = None;
    wait_until("automatic launcher wait receipt", || {
        let paths: Vec<String> = db.prepare("SELECT result_path FROM completion_continuation_attempt WHERE session_id=?1 AND operation='activation'")
            .unwrap().query_map([SESSION], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for path in paths {
            let path = Path::new(&path);
            let stderr =
                fs::read_to_string(path.with_file_name("launcher.stderr")).unwrap_or_default();
            let stdout =
                fs::read_to_string(path.with_file_name("launcher.stdout")).unwrap_or_default();
            let matched = stdout
                .lines()
                .chain(stderr.lines())
                .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .any(|result| {
                    result["id"] == invocation
                        && result["terminal_reason"] == "resume_completion_unconfirmed"
                });
            if matched && let Ok(bytes) = fs::read(path) {
                let receipt: Value = serde_json::from_slice(&bytes).unwrap();
                native_result = Some((receipt, stdout, stderr));
                return true;
            }
        }
        false
    });
    let (receipt, stdout, stderr) = native_result.unwrap();
    eprintln!(
        "SLOW_CONTROL actual_wait={receipt} launcher_stdout={stdout} launcher_stderr={stderr}"
    );
    let activation_json: String = db
        .query_row(
            "SELECT json_object('attempt_id',attempt_id,'owner_generation',owner_generation,
         'custodian',json(custodian_identity),'result_path',result_path)
         FROM completion_continuation_attempt WHERE attempt_id=?1 AND operation='activation'",
            [receipt["attempt_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    let activation: Value = serde_json::from_str(&activation_json).unwrap();
    assert_eq!(activation["owner_generation"], successor.owner_generation);
    assert_eq!(activation["custodian"], receipt["custodian"]);
    eprintln!("SLOW_CONTROL activation={activation}");
    assert_eq!(receipt["root_exit_code"], 1);
    assert_eq!(receipt["root_wait_status"], 256);
    assert_eq!(receipt["owned_children"], "ECHILD");
    assert_eq!(receipt["spawn_failed"], false);
    let outcome: (String, i32, String) = Connection::open(fixture.state_path())
        .unwrap()
        .query_row(
            "SELECT status,exit_code,terminal_reason FROM invocations WHERE invocation_uuid=?1",
            [invocation],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    eprintln!("SLOW_CONTROL notification={notification:?} outcome={outcome:?}");
    assert_eq!(
        fixture.recorded_resume_prompts().len(),
        1,
        "no duplicate submission"
    );
    assert_eq!(a["anchor"], "s11-anchor:0");
    assert!(a["ack"].is_null(), "native receipt must not fabricate ACK");
    let pages: Vec<_> = events
        .iter()
        .filter(|e| e["kind"] == "terminal_page")
        .collect();
    assert!(!pages.is_empty(), "must reach actual terminal reader");
    for page in pages {
        assert_eq!(page["target"]["attempt_id"], a["attempt_id"]);
        assert_eq!(page["target"]["admission_purpose"], "TerminalBounded");
        assert_eq!(page["before"]["invocation"], invocation);
        assert!(page["before"]["confirmed"].is_null());
        assert!(page["before"]["ack"].is_null());
        assert!(page["before"]["submitted"].is_string());
        assert_eq!(page["request"]["params"]["after_token"], a["anchor"]);
        assert_eq!(
            page["request"]["params"]["expected_delivery_nonce"],
            a["attempt_id"]
        );
        assert_eq!(page["after"]["confirmed"], Value::Null);
        assert_eq!(page["after"]["ack"], Value::Null);
        assert!(page["elapsed_ms"].as_u64().unwrap() >= 2500);
        assert_eq!(page["turn"], "s11-observed-user-1");
        if !wrong_digest {
            assert_eq!(page["digest"], a["digest"]);
        }
    }
    let delivered = fixture.mailbox_row(notification.seq);
    eprintln!("SLOW_CONTROL final_notification={delivered:?}");
    assert_eq!(
        fs::read(&notification.log_path).unwrap(),
        b"s11-admitted-source-output"
    );
    assert_eq!(
        fs::read_to_string(&notification.rc_path).unwrap().trim(),
        "0"
    );
    for field in ["snapshot_relative", "outcome_relative"] {
        let path = Path::new(registration["handle_dir"].as_str().unwrap())
            .join(registration[field].as_str().unwrap());
        let evidence: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        eprintln!("SLOW_INGRESS {field}={evidence}");
    }
    assert_eq!(delivered.delivery_attempts, 1);
    assert_eq!(outcome.0, "failed");
    if wrong_digest {
        assert!(delivered.delivered_at.is_none());
        assert!(a["confirmed"].is_null());
        assert_eq!(outcome.2, "resume_completion_unconfirmed");
        assert_eq!(
            delivered.delivery_error.as_deref(),
            Some("mailbox_delivery_unconfirmed")
        );
    } else {
        assert!(delivered.delivered_at.is_some());
        assert_eq!(
            delivered.delivered_by_invocation_uuid.as_deref(),
            Some(invocation)
        );
        assert_eq!(a["turn"], "s11-observed-user-1");
        assert_eq!(outcome.2, "resume_completion_unconfirmed");
        assert_eq!(outcome.1, 0, "provider clean exit, assistant result absent");
    }
    fixture.assert_xdg_isolated();
}

#[test]
fn accepted_owner_session_consumes_detached_child_completion_without_history_scan() {
    for provider in OPENCODE_PROVIDERS {
        assert_owner_session_consumes_detached_child_completion(provider);
    }
}

#[test]
fn accepted_manual_prompt_nonzero_has_one_typed_terminal_outcome() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    assert_success(&fixture.run_agent_with_env("seed manual failure", &[]));

    let output = fixture.run_resume_with_env(
        "accepted manual payload",
        &[
            ("S11_EMIT_PROMPT_ACCEPTANCE_MARKER", "1"),
            ("S11_NO_ASSISTANT_RESULT", "1"),
            ("S11_EXIT_NONZERO", "1"),
        ],
    );

    assert_eq!(output.status.code(), Some(29), "{output:?}");
    let result = result_envelope(&output);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["exit_code"], 29);
    assert_eq!(
        result["terminal_reason"],
        "resume_prompt_accepted_provider_failed"
    );
    assert_eq!(fixture.latest_resume_acceptance(), (None, None));
    fixture.assert_xdg_isolated();
}

#[test]
fn recipient_controls_preserve_hidden_prefix_and_select_nonce_bearing_launch() {
    let fixture = Fixture::new();
    fixture.write_external_provider();
    fixture.write_script(
        "s11_recipient_case.py",
        include_str!("fixtures/s11_recipient_case.py"),
    );
    let script = fixture.write_script(
        "s11_recipient_regression.py",
        include_str!("fixtures/s11_recipient_regression.py"),
    );
    let mut command = Command::new("/usr/bin/python3");
    command.arg(script).arg(fixture.dir.path());
    assert_success(&fixture.run(command));
}

// These cases select the actual automatic recipient, not the later manual turn.
// The Python driver retains exact source/attempt/provider/native-result joins.
fn recipient_case(provider: &'static str, marker: &str, exit: &str, projection: bool) {
    let fixture = Fixture::with_provider(provider);
    fixture.write_external_provider();
    fixture.remove_turn_script_fallback();
    let script = fixture.write_script(
        "s11_recipient_case.py",
        include_str!("fixtures/s11_recipient_case.py"),
    );
    let mut command = Command::new("/usr/bin/python3");
    command
        .arg(script)
        .arg(runner_bin())
        .arg(&fixture.models_dir)
        .arg(provider)
        .arg(marker)
        .arg(exit)
        .arg(if projection { "1" } else { "0" });
    let output = fixture.run(command);
    assert_success(&output);
    fixture.assert_xdg_isolated();
}

#[test]
fn missing_final_exit_does_not_settle_mismatched_prompt_acceptance() {
    recipient_case(PROVIDER, "hash", "missing", false);
}

#[test]
fn trusted_prompt_acceptance_settles_mailbox_delivery_after_provider_nonzero() {
    recipient_case(PROVIDER, "trusted", "nonzero", false);
}

#[test]
fn trusted_prompt_acceptance_settles_mailbox_delivery_when_final_exit_is_missing() {
    recipient_case(PROVIDER, "trusted", "missing", false);
}

#[test]
fn absent_prompt_acceptance_is_reconciled_before_nonzero_and_missing_exit_replay() {
    for exit in ["nonzero", "missing"] {
        recipient_case(PROVIDER, "absent", exit, false);
    }
}

#[test]
fn wrong_prompt_acceptance_session_and_nonce_are_reconciled_before_replay() {
    let mut failures = Vec::new();
    for marker in ["session", "nonce"] {
        for exit in ["nonzero", "missing"] {
            if std::panic::catch_unwind(|| recipient_case(PROVIDER, marker, exit, false)).is_err() {
                failures.push((marker, exit));
            }
        }
    }
    assert!(failures.is_empty(), "failed matrix cells: {failures:?}");
}

#[test]
fn trusted_prompt_acceptance_survives_mailbox_projection_failure_without_replay() {
    recipient_case(PROVIDER, "trusted", "nonzero", true);
}

#[test]
fn ordinary_completion_survives_mailbox_projection_failure_without_replay() {
    recipient_case(PROVIDER, "absent", "success", true);
}

#[test]
fn delivery_nonce_does_not_override_a_mismatched_prompt_hash() {
    recipient_case(PROVIDER, "hash", "nonzero", false);
}

fn assert_owner_session_consumes_detached_child_completion(provider: &'static str) {
    recipient_case(provider, "trusted", "success", false);
    recipient_case(provider, "trusted", "no-assistant", false);
}

#[test]
fn external_provider_policy_rejection_terminal_signal_excerpt_includes_diagnostics() {
    let fixture = Fixture::new();
    fixture.write_external_provider();

    let output = fixture.run_agent_with_env(
        "dispatch external provider reject",
        &[("S11_POLICY_REJECT", "1")],
    );

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let marker = terminal_signal_marker(&stderr);
    let excerpt = marker
        .get("evidence")
        .and_then(|evidence| evidence.get("excerpt"))
        .and_then(Value::as_str)
        .expect("terminal-signal marker should include evidence excerpt");
    assert!(excerpt.contains("policy rejected"), "{excerpt}");
    assert!(excerpt.contains("s11_policy_reject"), "{excerpt}");
    assert!(excerpt.contains("params.launch.argv"), "{excerpt}");
    assert!(excerpt.contains("s11 fixture rejected policy"), "{excerpt}");
    fixture.assert_xdg_isolated();
}

fn external_provider_script() -> &'static str {
    r#"#!/usr/bin/env python3
import base64
import hashlib
import json
import os
import pathlib
import sys
import time
import slow_control
import s11_recipient_control as recipient_control
slow_control.configure()

CONTRACT = "oulipoly.provider/v1"
PROMPT_ACCEPTANCE = "oulipoly.prompt_acceptance/v1"
PROMPT_ACCEPTED_MARKER = "oulipoly.prompt_accepted/v1"
SESSION = "ses_s11externalwake"

def request_id(request):
    return request.get("request_id", "s11-request")

def envelope(request, result):
    return {
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(request),
        "ok": True,
        "result": result,
    }

def describe(request):
    capabilities = {
        "launch": True,
        "launch_output_v1": True,
        "policy": True,
        "quota": False,
        "session": True,
        "session_turn_pages_v1": True,
        "terminal": False,
        "rotation": False,
        "discovery": False,
        "settings": False,
        "setup_brain": False,
        "setup": False,
        "migration": False,
    }
    if os.environ.get("S11_OMIT_PROMPT_ACCEPTANCE_CAPABILITY") != "1":
        capabilities["prompt_acceptance_v1"] = True
    return envelope(request, {
        "provider_id": "s11-external-provider-runtime-fixture",
        "display_name": "S11 External Provider Runtime Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": capabilities,
    })

def policy_evaluate(request):
    if os.environ.get("S11_POLICY_REJECT") == "1":
        return envelope(request, {
            "accepted": False,
            "env": {},
            "stdin": None,
            "prompt": None,
            "diagnostics": [{
                "severity": "error",
                "code": "s11_policy_reject",
                "path": "params.launch.argv",
                "message": "s11 fixture rejected policy",
            }],
            "markers": [],
        })
    return envelope(request, {
        "accepted": True,
        "env": {
            "OULIPOLY_LIVE_SESSION_BIND_SOCKET": "/policy-parent-owner.sock",
            "OULIPOLY_LIVE_SESSION_BIND_TOKEN": "policy-parent-owner-token",
        } if os.environ.get("S11_CHECK_LIVE_BINDING_ISOLATION") == "1" else {},
        "stdin": None,
        "prompt": None,
        "diagnostics": [],
        "markers": [],
    })

def emit(event):
    recipient_control.emitted(event)
    print(json.dumps(event, separators=(",", ":")), flush=True)

def stdout_event(request, seq, payload):
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "stdout",
        "data_base64": base64.b64encode(payload.encode("utf-8")).decode("ascii"),
    }

def provider_session_marker_event(request, seq, session_id):
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.provider_session",
        "value": {"provider_session_id": session_id},
    }

def prompt_acceptance_marker_event(request, seq, session_id, prompt):
    prompt_sha = hashlib.sha256(prompt.encode("utf-8")).hexdigest()
    acceptance = request.get("params", {}).get("prompt_acceptance", {})
    if os.environ.get("S11_OMIT_PROMPT_ACCEPTANCE_CAPABILITY") != "1":
        assert acceptance.get("protocol") == PROMPT_ACCEPTANCE
        assert acceptance.get("prompt_sha256") == prompt_sha
    if os.environ.get("S11_MARKER_PROMPT_SHA_MISMATCH") == "1":
        prompt_sha = hashlib.sha256(b"different payload").hexdigest()
    value = {
        "protocol": PROMPT_ACCEPTANCE,
        "provider_session_id": session_id,
        "prompt_sha256": prompt_sha,
        "source": "s11.fixture",
        "message_id": "msg-s11-prompt-accepted",
    }
    if os.environ.get("S11_MARKER_SESSION_MISMATCH") == "1":
        value["provider_session_id"] = "ses_s11-wrong-session"
    if acceptance.get("delivery_nonce"):
        value["delivery_nonce"] = acceptance["delivery_nonce"]
    if os.environ.get("S11_MARKER_DELIVERY_NONCE_MISMATCH") == "1":
        value["delivery_nonce"] = "s11-wrong-delivery-nonce"
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": PROMPT_ACCEPTED_MARKER,
        "value": value,
    }

def produced_assistant_response_marker_event(request, seq):
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.produced_assistant_response",
        "value": True,
    }

def launch_output_complete_event(request, seq, stdout_payloads):
    stdout = "".join(stdout_payloads).encode("utf-8")
    return {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.launch_output_complete/v1",
        "value": {
            "protocol": "oulipoly.launch_output/v1",
            "stdout": {
                "bytes": len(stdout),
                "sha256": hashlib.sha256(stdout).hexdigest(),
            },
            "stderr": {
                "bytes": 0,
                "sha256": hashlib.sha256(b"").hexdigest(),
            },
            "data_event_count": len(stdout_payloads),
        },
    }

def exit_event(request, seq, session_id):
    code = 29 if os.environ.get("S11_EXIT_NONZERO") == "1" else 0
    event = {
        "contract": CONTRACT,
        "request_id": request_id(request),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {"kind": "exited", "code": code},
        "terminal_signal": {
            "kind": "nonzero_exit" if code else "clean_exit",
            "evidence": "fixture nonzero exit" if code else "fixture clean exit",
            "observed_at_unix_ms": 1000 + seq,
        },
    }
    if session_id:
        event["session"] = {
            "provider_session_id": session_id,
            "state": {"cursor": "after-launch"},
        }
    return event

def launch(request):
    # Actual provider entry, not a command intent or an inferred latest row.
    evidence = pathlib.Path(os.environ["S11_WORK_DIR"]) / "provider-launches.jsonl"
    with evidence.open("a") as stream:
        stream.write(json.dumps(request) + "\n")
    params = request.get("params", {})
    known = params.get("session", {}).get("known_provider_session_id")
    prompt = params.get("model", {}).get("inputs", {}).get("prompt", "")
    if known:
        prompt_log = pathlib.Path(os.environ["S11_WORK_DIR"]).joinpath("resume-prompts.jsonl")
        with prompt_log.open("a") as stream:
            stream.write(json.dumps(prompt, separators=(",", ":")) + "\n")
        seq = 1
        emit(provider_session_marker_event(request, seq, known))
        seq += 1
        stdout_payloads = []
        produced_assistant_response = False
        if os.environ.get("S11_NO_ASSISTANT_RESULT") != "1":
            text = "resumed\n"
            if os.environ.get("S11_EMIT_AFFIRMATIVE_ASSISTANT_RESULT") == "1":
                text = "owner consumed detached child result and continued\n"
                pathlib.Path(os.environ["S11_WORK_DIR"]).joinpath("affirmative-result").write_text(text)
                produced_assistant_response = True
            emit(stdout_event(request, seq, text))
            stdout_payloads.append(text)
            seq += 1
        if os.environ.get("S11_EMIT_PROMPT_ACCEPTANCE_MARKER") == "1":
            emit(prompt_acceptance_marker_event(request, seq, known, prompt))
            seq += 1
        if produced_assistant_response:
            emit(produced_assistant_response_marker_event(request, seq))
            seq += 1
        emit(launch_output_complete_event(request, seq, stdout_payloads))
        seq += 1
        if os.environ.get("S11_OMIT_EXIT_EVENT") != "1":
            slow_control.launch_exit(request)
            emit(exit_event(request, seq, known))
        return
    session_id = None if os.environ.get("S11_OMIT_EXIT_SESSION") == "1" else SESSION
    seq = 1
    if session_id:
        emit(provider_session_marker_event(request, seq, session_id))
        seq += 1
        slow_control.admit_source(request, session_id)
        recipient_control.admit_source(request, session_id)
    initial = "initial\n"
    emit(stdout_event(request, seq, initial))
    seq += 1
    emit(launch_output_complete_event(request, seq, [initial]))
    emit(exit_event(request, seq + 1, session_id))

def session_id_from_request(request):
    params = request.get("params", {})
    extra = params.get("extra", {})
    return params.get("session_id") or extra.get("start_bound_provider_session_id") or SESSION

def capture(request):
    return envelope(request, {
        "provider_session_id": session_id_from_request(request),
        "state": {"captured": True},
        "artifacts": [],
    })

def session_turn_page(request):
    params = request.get("params", {})
    projection = params.get("turn_projection")
    start_mode = params.get("start_mode")
    prompt_log = pathlib.Path(os.environ["S11_WORK_DIR"]).joinpath("resume-prompts.jsonl")
    prompts = []
    if prompt_log.exists():
        prompts = [json.loads(line) for line in prompt_log.read_text().splitlines() if line]
    provider_instance_id = request.get("provider_instance_id")
    settings_id = params.get("settings_id")
    if start_mode == "tail":
        turns = []
        resume_token = "s11-anchor:" + str(len(prompts))
        snapshot_id = "s11-tail:" + str(len(prompts))
    elif projection == "user_observation":
        after_token = params.get("after_token") or "s11-anchor:0"
        start = int(after_token.rsplit(":", 1)[1])
        selected = prompts[start:start + params.get("max_turns", 1)]
        turns = []
        for offset, prompt in enumerate(selected):
            normalized = prompt.replace("\r\n", "\n").replace("\r", "\n").strip()
            turns.append({
                "session_id": SESSION,
                "turn_id": "s11-observed-user-" + str(start + offset + 1),
                "snapshot_sequence": offset,
                "timestamp": "2026-08-30T12:00:00Z",
                "role": "user",
                "parent_turn_id": None,
                "is_sidechain": False,
                "is_compaction_boundary": False,
                "body_state": "omitted_oversize",
                "body": None,
                "body_bytes": len(normalized.encode("utf-8")),
                "body_sha256": None,
                "canonical_text_sha256": hashlib.sha256(normalized.encode("utf-8")).hexdigest(),
            })
        resume_token = "s11-anchor:" + str(len(prompts))
        snapshot_id = "s11-observation:" + str(len(prompts))
    else:
        turns = []
        resume_token = "s11-canonical:" + str(len(prompts))
        snapshot_id = "s11-canonical-snapshot:" + str(len(prompts))
    return envelope(request, {
        "read_protocol": "oulipoly.session_turn_pages/v1",
        "provider_instance_id": provider_instance_id,
        "settings_id": settings_id,
        "session_id": SESSION,
        "turn_projection": projection,
        "snapshot_id": snapshot_id,
        "page_index": 0,
        "page_start_sequence": 0,
        "turns": turns,
        "page_turn_count": len(turns),
        "source_bytes_examined": sum(len(json.dumps(turn)) for turn in turns),
        "scan_progress": False,
        "snapshot_complete": True,
        "next_page_token": None,
        "resume_token": resume_token,
        "source_final": False,
        "warnings": [],
    })

def main():
    subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
    request = json.loads(sys.stdin.read() or "{}")
    recipient_control.configure(request, subcommand)
    if os.environ.get("S11_CHECK_LIVE_BINDING_ISOLATION") == "1":
        params = request.get("params", {})
        environments = [os.environ, request.get("host", {}).get("env", {}),
                        params.get("env", {}), params.get("launch", {}).get("env", {})]
        for environment in environments:
            for key in ("OULIPOLY_LIVE_SESSION_BIND_SOCKET", "OULIPOLY_LIVE_SESSION_BIND_TOKEN"):
                assert key not in environment, f"{subcommand} inherited {key}"
        if subcommand == "launch":
            identity = json.loads(params["env"]["OULIPOLY_PARENT_INVOCATION"])
            pathlib.Path(os.environ["S11_WORK_DIR"]).joinpath("child-invocation").write_text(identity["id"])
    if subcommand == "describe":
        print(json.dumps(describe(request)))
        return 0
    if subcommand == "policy.evaluate":
        print(json.dumps(policy_evaluate(request)))
        return 0
    if subcommand == "launch":
        launch(request)
        return 0
    if subcommand == "session.capture":
        print(json.dumps(capture(request)))
        return 0
    if subcommand == "session.read_turns":
        time.sleep(int(os.environ.get("S11_READ_TURNS_DELAY_MS", "0")) / 1000)
        print(json.dumps(recipient_control.read_page(request, slow_control.read_page(request, session_turn_page))))
        return 0
    print(json.dumps({
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(request),
        "ok": False,
        "error": {
            "category": "failed",
            "code": "unsupported_subcommand",
            "message": subcommand,
            "retryable": False,
        },
    }))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
"#
}

fn turn_script() -> &'static str {
    r#"#!/usr/bin/env python3
import json

print(json.dumps({
    "session_id": "ses_s11externalwake",
    "turn_id": "turn-s11-initial",
    "timestamp": "2026-06-06T00:00:00Z",
    "role": "assistant",
    "completion_outcome": "stop",
    "body": [{"type": "text", "text": "initial fixture turn"}],
}, separators=(",", ":")))
"#
}

fn assert_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn result_envelope(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines = stdout
        .lines()
        .chain(stderr.lines())
        .filter_map(|line| line.strip_prefix("OULIPOLY_RESULT="))
        .collect::<Vec<_>>();
    assert_eq!(
        lines.len(),
        1,
        "expected one result envelope: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout,
        stderr
    );
    serde_json::from_str(lines[0]).unwrap()
}

fn terminal_signal_marker(stderr: &str) -> Value {
    let marker = stderr
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_TERMINAL_SIGNAL="))
        .unwrap_or_else(|| panic!("missing terminal-signal marker in stderr:\n{stderr}"));
    serde_json::from_str(marker).unwrap()
}

fn wait_until(label: &str, mut predicate: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {label}");
}

fn runner_bin() -> &'static str {
    bounded_runner_image::runner_bin()
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap()
}
