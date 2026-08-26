//! ## Declared roles
//! orchestration, accessor, mapper, formatter, validator
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/age123_resume_provider_identity.rs
//!     role: intrinsic-surface
//!     Domain: tauri-resume-provider-identity-integration-test-domain
//!     Owns:
//!       - ProviderFixture provider-script fixture surface
//!       - Fixture impl integration harness surface
//!       - InvocationIdentityRow captured invocation identity DTO
//!       - map_identity_row rusqlite row mapper
//!       - toml_string TOML literal formatter
//!       - identity_for provider resolved-account formatter/accessor
//!       - resume_session_mismatch_records_expected_segment_and_resolved_provider_identity validation body
//!       - successful_resume_records_resolved_provider_identity validation body
//!       - quota_retry_records_resolved_provider_identity_per_attempt validation body
//!       - manual_migration_records_target_resolved_provider_identity validation body
//!       - oulipoly_state InvocationStatus and StateDb harness APIs
//!       - rusqlite Connection, params, Row, and Result helper surface
//!       - oulipoly_config and oulipoly_runtime contracts exercised through the configured runner binary

#![cfg(unix)]

use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, EnqueueResult, MailboxDb, WakeClaimAcquireResult, WakeClaimRequest,
};
use oulipoly_state::{InvocationStatus, StateDb};
use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const CHAIN_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "6169694d-de0f-40d1-890c-6e28e55bab28";
const FORCE_KIND: &str = "OULIPOLY_AGE153_FORCE_TERMINAL_SIGNAL_KIND";

struct ProviderFixture<'a> {
    name: &'a str,
    body: &'a str,
}

struct Fixture {
    dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

#[derive(Debug)]
struct InvocationIdentityRow {
    provider_name: String,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
    provider_session_capture_method: Option<String>,
    provider_session_resolved_account: Option<String>,
    resume_acceptance_status: Option<String>,
    resume_acceptance_evidence: Option<String>,
    status: String,
    success: Option<bool>,
    exit_code: Option<i64>,
    error_category: Option<String>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();

        Self {
            dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn sidecar_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("pid-identity.db")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    fn conn(&self) -> Connection {
        let _ = self.open_db();
        Connection::open(self.db_path()).unwrap()
    }

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn write_resume_pool(&self, model_name: &str, providers: &[ProviderFixture<'_>]) {
        let mut model = String::new();
        for provider in providers {
            model.push_str(&format!(
                r#"[[providers]]
name = "{}"
args = ["exec-{}"]

"#,
                provider.name, provider.name
            ));
        }
        fs::write(self.models_dir.join(format!("{model_name}.toml")), model).unwrap();

        fs::write(
            self.models_dir.join("diagnostic.toml"),
            r#"[[providers]]
name = "diagnostic-provider"
"#,
        )
        .unwrap();
        fs::write(
            self.app_config_dir.join("config.toml"),
            r#"diagnostics_model = "diagnostic"
"#,
        )
        .unwrap();

        let mut providers_toml = String::new();
        for provider in providers {
            let command = self.write_script(&format!("{}-resume.sh", provider.name), provider.body);
            providers_toml.push_str(&format!(
                r#"[{}]
command = {}
args = []
interactive_args = ["launch-{}"]
prompt_mode = "arg"

[{}.resume]
kind = "flag"
flag = "--resume"

[{}.session_storage]
kind = "claude_code"
projects_dir = {}

"#,
                provider.name,
                toml_string(&command.display().to_string()),
                provider.name,
                provider.name,
                provider.name,
                toml_string(
                    &self
                        .provider_projects_dir(provider.name)
                        .display()
                        .to_string()
                )
            ));
        }

        let diagnostic_command = self.write_script(
            "diagnostic-provider.sh",
            "cat >/dev/null\nprintf '%s\\n' 'quota_exhausted' 'diagnosed quota exhaustion'",
        );
        providers_toml.push_str(&format!(
            r#"[diagnostic-provider]
command = {}
args = []
prompt_mode = "stdin"
"#,
            toml_string(&diagnostic_command.display().to_string())
        ));
        fs::write(self.app_config_dir.join("providers.toml"), providers_toml).unwrap();
    }

    fn provider_projects_dir(&self, provider: &str) -> PathBuf {
        self.dir.path().join(format!("{provider}-projects"))
    }

    fn stage_claude_jsonl(&self, provider: &str, session_id: &str) {
        let source_dir = self.provider_projects_dir(provider).join("source-project");
        fs::create_dir_all(&source_dir).unwrap();
        fs::write(
            source_dir.join(format!("{session_id}.jsonl")),
            format!(
                r#"{{"sessionId":"{session_id}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
            ),
        )
        .unwrap();
    }

    fn seed_rotated_chain(&self, source_provider: &str, target_provider: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'age123-resume')",
            params![CHAIN_ID],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, ended_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', '2026-04-17T09:00:00Z', 'initial')",
            params![CHAIN_ID, source_provider, SESSION_A],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T09:00:00Z', 'quota_threshold')",
            params![CHAIN_ID, target_provider, SESSION_B],
        )
        .unwrap();
    }

    fn seed_active_chain(&self, provider: &str, session_id: &str) {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'age123-resume')",
            params![CHAIN_ID],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, '2026-04-17T08:00:00Z', 'initial')",
            params![CHAIN_ID, provider, session_id],
        )
        .unwrap();
    }

    fn run_resume(&self, resume_input: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg("age123-resume")
            .arg("--resume")
            .arg(resume_input)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continue after rotation");
        self.run(cmd)
    }

    fn run_resume_with_env(&self, resume_input: &str, env: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg("age123-resume")
            .arg("--resume")
            .arg(resume_input)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continue after rotation");
        for (key, value) in env {
            cmd.env(key, value);
        }
        self.run(cmd)
    }

    fn run_resume_with_migration(&self, resume_input: &str, target_provider: &str) -> Output {
        self.run_resume_with_migration_env(resume_input, target_provider, &[])
    }

    fn run_resume_with_migration_env(
        &self,
        resume_input: &str,
        target_provider: &str,
        env: &[(&str, &str)],
    ) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("-m")
            .arg("age123-resume")
            .arg("--resume")
            .arg(resume_input)
            .arg("--rotate-provider")
            .arg(target_provider)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("continue after manual migration");
        for (key, value) in env {
            cmd.env(key, value);
        }
        self.run(cmd)
    }

    fn seed_pending_auto_wake_claim(&self, session_id: &str, claim_token: &str) -> i64 {
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        let row = match db
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id,
                handle: "notification-boundary",
                payload_json: "{}",
                owner_invocation_uuid: None,
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: "/tmp/notification-boundary",
                meta_path: "/tmp/notification-boundary/meta.json",
                log_path: "/tmp/notification-boundary/output.log",
                rc_path: "/tmp/notification-boundary/rc",
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) | EnqueueResult::AlreadyEnqueued(row) => row,
            EnqueueResult::Conflict { existing } => existing,
        };
        let result = db
            .wake_sessions()
            .try_acquire_wake_claim(WakeClaimRequest {
                session_id,
                claim_token,
                reason: "notify_idle",
                auto_wake_count: 1,
                wake_invocation_uuid: None,
                stale_after_seconds: 600,
            })
            .unwrap();
        assert!(matches!(result, WakeClaimAcquireResult::Acquired(_)));
        row.seq
    }

    fn seed_auto_wake_claim(&self, session_id: &str, claim_token: &str) {
        let seq = self.seed_pending_auto_wake_claim(session_id, claim_token);
        let mut db = MailboxDb::open(&self.sidecar_path()).unwrap();
        db.mark_delivered(session_id, None, &[seq], "notification-boundary-test")
            .unwrap();
    }

    fn invocation_count(&self) -> i64 {
        self.conn()
            .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
            .unwrap()
    }

    fn run(&self, mut cmd: Command) -> Output {
        cmd.current_dir(self.dir.path());
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd.output().unwrap()
    }

    fn latest_invocation(&self) -> InvocationIdentityRow {
        self.conn()
            .query_row(
                "SELECT provider_name, provider_session_id, resume_input_id,
                        provider_session_capture_method, provider_session_resolved_account,
                        resume_acceptance_status, resume_acceptance_evidence, status, success,
                        exit_code, error_category
                 FROM invocations
                 ORDER BY id DESC
                 LIMIT 1",
                [],
                map_identity_row,
            )
            .unwrap()
    }

    fn invocation_for_provider(&self, provider: &str) -> InvocationIdentityRow {
        self.conn()
            .query_row(
                "SELECT provider_name, provider_session_id, resume_input_id,
                        provider_session_capture_method, provider_session_resolved_account,
                        resume_acceptance_status, resume_acceptance_evidence, status, success,
                        exit_code, error_category
                 FROM invocations
                 WHERE provider_name = ?1
                 ORDER BY id DESC
                 LIMIT 1",
                params![provider],
                map_identity_row,
            )
            .unwrap()
    }
}

fn map_identity_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationIdentityRow> {
    Ok(InvocationIdentityRow {
        provider_name: row.get(0)?,
        provider_session_id: row.get(1)?,
        resume_input_id: row.get(2)?,
        provider_session_capture_method: row.get(3)?,
        provider_session_resolved_account: row.get(4)?,
        resume_acceptance_status: row.get(5)?,
        resume_acceptance_evidence: row.get(6)?,
        status: row.get(7)?,
        success: row.get(8)?,
        exit_code: row.get(9)?,
        error_category: row.get(10)?,
    })
}

fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

fn identity_for(fixture: &Fixture, provider: &str) -> String {
    fixture
        .provider_projects_dir(provider)
        .display()
        .to_string()
}

#[test]
fn resume_session_mismatch_records_expected_segment_and_resolved_provider_identity() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'unexpected provider a launch' >&2\nexit 99",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'No conversation found with session ID: 6169694d-de0f-40d1-890c-6e28e55bab28' >&2\nexit 7",
            },
        ],
    );
    fixture.seed_rotated_chain("claude-a", "claude-b");

    let output = fixture.run_resume(CHAIN_ID);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let row = fixture.latest_invocation();
    assert_eq!(row.provider_name, "claude-b");
    assert_eq!(row.provider_session_id.as_deref(), Some(SESSION_B));
    assert_eq!(row.resume_input_id.as_deref(), Some(CHAIN_ID));
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("resumed")
    );
    assert_eq!(
        row.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("rejected"));
    assert!(
        row.resume_acceptance_evidence
            .as_deref()
            .is_some_and(|evidence| evidence.contains("resume_session_mismatch")),
        "{row:?}"
    );
    assert_eq!(
        row.error_category.as_deref(),
        Some("resume_session_mismatch")
    );
}

#[test]
fn successful_resume_records_resolved_provider_identity() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'unexpected provider a launch' >&2\nexit 99",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'resume accepted'\nexit 0",
            },
        ],
    );
    fixture.seed_rotated_chain("claude-a", "claude-b");

    let output = fixture.run_resume(CHAIN_ID);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let row = fixture.latest_invocation();
    assert_eq!(row.provider_name, "claude-b");
    assert_eq!(row.provider_session_id.as_deref(), Some(SESSION_B));
    assert_eq!(
        row.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
    assert_eq!(row.status, InvocationStatus::Succeeded.as_str());
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("unconfirmed"));
}

#[test]
fn resumed_provider_observes_exact_durable_admission_before_launch() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "provider-a",
                body: "printf '%s\\n' 'unexpected provider a launch' >&2\nexit 99",
            },
            ProviderFixture {
                name: "provider-b",
                body: r#"python3 - <<'PY'
import os
import sqlite3

path = os.path.join(os.environ["XDG_DATA_HOME"], "oulipoly-agent-runner", "pid-identity.db")
connection = sqlite3.connect(path)
rows = connection.execute(
    "SELECT session_id, state FROM session_admission_queue WHERE state = 'launching'"
).fetchall()
assert rows == [("6169694d-de0f-40d1-890c-6e28e55bab28", "launching")], rows
PY
printf '%s\n' 'resume admission observed'
exit 0"#,
            },
        ],
    );
    fixture.seed_rotated_chain("provider-a", "provider-b");

    let output = fixture.run_resume(CHAIN_ID);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let connection = Connection::open(fixture.sidecar_path()).unwrap();
    let (session_id, state, generation): (Option<String>, String, Option<String>) = connection
        .query_row(
            "SELECT session_id, state, runtime_generation_uuid
             FROM session_admission_queue",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(session_id.as_deref(), Some(SESSION_B));
    assert_eq!(state, "settled");
    assert!(generation.is_some());
}

#[test]
fn quota_retry_records_resolved_provider_identity_per_attempt() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'Claude usage limit reached for active resume provider' >&2\nexit 42",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'retried resume accepted'\nexit 0",
            },
        ],
    );
    fixture.stage_claude_jsonl("claude-a", SESSION_A);
    fixture.seed_active_chain("claude-a", SESSION_A);

    let output =
        fixture.run_resume_with_env(CHAIN_ID, &[(FORCE_KIND, "QuotaExhaustedInband,None")]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let first = fixture.invocation_for_provider("claude-a");
    let second = fixture.invocation_for_provider("claude-b");
    assert_eq!(
        first.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-a"))
    );
    assert_eq!(
        second.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
    assert_eq!(first.error_category.as_deref(), Some("quota_exhausted"));
    assert_eq!(first.status, InvocationStatus::Failed.as_str());
    assert_eq!(second.status, InvocationStatus::Succeeded.as_str());
    assert_eq!(second.success, Some(true));
}

#[test]
fn manual_migration_records_target_resolved_provider_identity() {
    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: "claude-a",
                body: "printf '%s\\n' 'unexpected source launch' >&2\nexit 99",
            },
            ProviderFixture {
                name: "claude-b",
                body: "printf '%s\\n' 'manual migration accepted'\nexit 0",
            },
        ],
    );
    fixture.stage_claude_jsonl("claude-a", SESSION_A);
    fixture.seed_active_chain("claude-a", SESSION_A);

    let output = fixture.run_resume_with_migration(CHAIN_ID, "claude-b");

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let row = fixture.latest_invocation();
    assert_eq!(row.provider_name, "claude-b");
    assert_eq!(row.resume_input_id.as_deref(), Some(CHAIN_ID));
    assert_eq!(row.provider_session_id.as_deref(), Some(SESSION_A));
    assert_eq!(
        row.provider_session_resolved_account,
        Some(identity_for(&fixture, "claude-b"))
    );
}

#[test]
fn notification_auto_wake_stays_bound_despite_explicit_rotation_target() {
    const SOURCE: &str = "provider-a";
    const TARGET: &str = "provider-b";
    const CLAIM_TOKEN: &str = "notification-boundary-claim";

    let fixture = Fixture::new();
    fixture.write_resume_pool(
        "age123-resume",
        &[
            ProviderFixture {
                name: SOURCE,
                body: "printf '%s\\n' 'bound notification resume accepted'\nexit 0",
            },
            ProviderFixture {
                name: TARGET,
                body: "printf '%s\\n' 'notification must not rotate' >&2\nexit 99",
            },
        ],
    );
    let source_dir = fixture.provider_projects_dir(SOURCE).join("source-project");
    fs::create_dir_all(&source_dir).unwrap();
    fs::write(
        source_dir.join(format!("{SESSION_A}.jsonl")),
        format!(
            r#"{{"sessionId":"{SESSION_A}","turnId":"turn-1","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
        ),
    )
    .unwrap();
    fixture.seed_active_chain(SOURCE, SESSION_A);
    fixture.seed_auto_wake_claim(CHAIN_ID, CLAIM_TOKEN);

    let output = fixture.run_resume_with_migration_env(
        CHAIN_ID,
        TARGET,
        &[
            ("OULIPOLY_AUTO_WAKE", "1"),
            ("OULIPOLY_AUTO_WAKE_SESSION_ID", CHAIN_ID),
            ("OULIPOLY_AUTO_WAKE_TOKEN", CLAIM_TOKEN),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let row = fixture.latest_invocation();
    assert_eq!(row.provider_name, SOURCE);
    assert_eq!(row.provider_session_id.as_deref(), Some(SESSION_A));
    assert_eq!(row.status, InvocationStatus::Succeeded.as_str());
}

#[test]
fn notification_auto_wake_validation_rejects_invalid_child_markers_before_provider_execution() {
    const PROVIDER: &str = "provider-poison";
    const CLAIM_TOKEN: &str = "notification-boundary-claim";
    const PROVIDER_CANARY_ENV: &str = "ACR329_PROVIDER_CANARY";

    let invalid_markers = [
        ("wrong-session", "wrong-session", CLAIM_TOKEN),
        ("empty-token", CHAIN_ID, ""),
        ("wrong-token", CHAIN_ID, "wrong-token"),
    ];
    for (case, expected_session, child_token) in invalid_markers {
        let fixture = Fixture::new();
        let canary = fixture.dir.path().join(format!("{case}-provider-ran"));
        fixture.write_resume_pool(
            "age123-resume",
            &[ProviderFixture {
                name: PROVIDER,
                body: ": > \"${ACR329_PROVIDER_CANARY:?}\"\nprintf '%s\\n' 'provider must not run' >&2\nexit 99",
            }],
        );
        fixture.seed_active_chain(PROVIDER, SESSION_A);
        let seq = fixture.seed_pending_auto_wake_claim(CHAIN_ID, CLAIM_TOKEN);

        let output = fixture.run_resume_with_env(
            CHAIN_ID,
            &[
                ("OULIPOLY_AUTO_WAKE", "1"),
                ("OULIPOLY_AUTO_WAKE_SESSION_ID", expected_session),
                ("OULIPOLY_AUTO_WAKE_TOKEN", child_token),
                (PROVIDER_CANARY_ENV, canary.to_str().unwrap()),
            ],
        );

        assert_eq!(output.status.code(), Some(0), "{case}: {output:?}");
        assert!(!canary.exists(), "{case}: provider executed");
        assert_eq!(fixture.invocation_count(), 0, "{case}");
        let db = MailboxDb::open(&fixture.sidecar_path()).unwrap();
        let pending = db.list_pending(CHAIN_ID).unwrap();
        assert_eq!(pending.len(), 1, "{case}");
        assert_eq!(pending[0].seq, seq, "{case}");
        assert_eq!(pending[0].delivery_attempts, 0, "{case}");
        assert!(pending[0].delivered_at.is_none(), "{case}");
        let claim = db
            .wake_session_reader()
            .wake_claim(CHAIN_ID)
            .unwrap()
            .unwrap();
        assert_eq!(claim.claim_token, CLAIM_TOKEN, "{case}");
        assert_eq!(claim.auto_wake_count, 1, "{case}");
    }

    let fixture = Fixture::new();
    let canary = fixture.dir.path().join("missing-sidecar-provider-ran");
    fixture.write_resume_pool(
        "age123-resume",
        &[ProviderFixture {
            name: PROVIDER,
            body: ": > \"${ACR329_PROVIDER_CANARY:?}\"\nprintf '%s\\n' 'provider must not run' >&2\nexit 99",
        }],
    );
    fixture.seed_active_chain(PROVIDER, SESSION_A);
    assert!(!fixture.sidecar_path().exists());

    let output = fixture.run_resume_with_env(
        CHAIN_ID,
        &[
            ("OULIPOLY_AUTO_WAKE", "1"),
            ("OULIPOLY_AUTO_WAKE_SESSION_ID", CHAIN_ID),
            ("OULIPOLY_AUTO_WAKE_TOKEN", CLAIM_TOKEN),
            (PROVIDER_CANARY_ENV, canary.to_str().unwrap()),
        ],
    );

    assert_eq!(output.status.code(), Some(0), "missing-sidecar: {output:?}");
    assert!(!canary.exists(), "missing-sidecar: provider executed");
    assert_eq!(fixture.invocation_count(), 0);
    assert!(!fixture.sidecar_path().exists());
}
