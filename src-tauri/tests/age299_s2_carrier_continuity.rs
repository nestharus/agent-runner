#![cfg(target_os = "linux")]

//! Linux production-binary causal proof for AGE-299 S2 completion authority.
//!
//! ## Declared roles
//! `orchestration`, `validator`

use oulipoly_state::mailbox::MailboxDb;
use oulipoly_state::{InvocationStatus, ProviderSessionBinding, StateDb};
use rusqlite::{Connection, params};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const MODEL: &str = "age299-s2-carrier";
const PROVIDER: &str = "age299-s2-provider";
const SESSION_ID: &str = "age299-s2-live-session";
const CHAIN_ID: &str = "29929929-2992-4992-8992-299299299299";

#[derive(Clone, Copy, Debug)]
enum Carrier {
    Balancing,
    Repl,
    Resume,
}

struct Fixture {
    root: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    registered: PathBuf,
    prepared: PathBuf,
    bind: PathBuf,
    release: PathBuf,
    child_env: PathBuf,
    registration_stdout: PathBuf,
    registration_stderr: PathBuf,
}

struct MaterializationSummary {
    materialized_count: i64,
    authority_ordinal: i64,
    sidecar_generation: String,
    continuity_digest: String,
}

struct ProviderScriptPaths<'a> {
    registered: &'a Path,
    prepared: &'a Path,
    bind: &'a Path,
    release: &'a Path,
    child_env: &'a Path,
    registration_stdout: &'a Path,
    registration_stderr: &'a Path,
}

impl Fixture {
    fn new(carrier: Carrier) -> Self {
        let root = tempfile::tempdir().unwrap();
        let config_home = root.path().join("config");
        let data_home = root.path().join("data");
        let app_config = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        let registered = root.path().join("registered");
        let prepared = root.path().join("prepared");
        let bind = root.path().join("bind");
        let release = root.path().join("release");
        let child_env = root.path().join("child-env");
        let registration_stdout = root.path().join("registration-stdout");
        let registration_stderr = root.path().join("registration-stderr");
        let script = root.path().join("provider.sh");
        write_executable(
            &script,
            &provider_script(
                root.path(),
                ProviderScriptPaths {
                    registered: &registered,
                    prepared: &prepared,
                    bind: &bind,
                    release: &release,
                    child_env: &child_env,
                    registration_stdout: &registration_stdout,
                    registration_stderr: &registration_stderr,
                },
            ),
        );
        fs::write(
            models_dir.join(format!("{MODEL}.toml")),
            format!("[[providers]]\nname = {PROVIDER:?}\nargs = []\ninteractive_args = []\n"),
        )
        .unwrap();
        fs::write(
            app_config.join("providers.toml"),
            format!(
                "[{PROVIDER}]\ncommand = {:?}\nargs = []\ninteractive_args = []\nprompt_mode = \"arg\"\n\n[{PROVIDER}.resume]\nkind = \"flag\"\nflag = \"--resume\"\n",
                script.display().to_string()
            ),
        )
        .unwrap();
        let fixture = Self {
            root,
            config_home,
            data_home,
            models_dir,
            registered,
            prepared,
            bind,
            release,
            child_env,
            registration_stdout,
            registration_stderr,
        };
        if matches!(carrier, Carrier::Resume) {
            fixture.seed_resume_identity();
        }
        fixture
    }

    fn state_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn result_artifact(&self, invocation_uuid: &str) -> PathBuf {
        self.state_path()
            .parent()
            .unwrap()
            .join("invocations/raw-io")
            .join(format!("{invocation_uuid}.result"))
    }

    fn seed_resume_identity(&self) {
        drop(StateDb::open(&self.state_path()).unwrap());
        let connection = Connection::open(self.state_path()).unwrap();
        connection
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-08-15T00:00:00Z', '2026-08-15T00:00:00Z', ?2)",
                params![CHAIN_ID, MODEL],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-08-15T00:00:00Z', 'initial')",
                params![CHAIN_ID, PROVIDER, SESSION_ID],
            )
            .unwrap();
    }

    fn spawn(&self, carrier: Carrier) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        match carrier {
            Carrier::Balancing => {
                command
                    .arg("--models-dir")
                    .arg(&self.models_dir)
                    .arg("--model")
                    .arg(MODEL)
                    .arg("retained-provider-outcome");
            }
            Carrier::Repl => {
                command
                    .arg("repl")
                    .arg("--models-dir")
                    .arg(&self.models_dir)
                    .arg(MODEL);
            }
            Carrier::Resume => {
                command
                    .arg("resume")
                    .arg("--model")
                    .arg(MODEL)
                    .arg("--session-id")
                    .arg(SESSION_ID)
                    .arg("--models-dir")
                    .arg(&self.models_dir);
            }
        }
        command
            .current_dir(self.root.path())
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_DATA_HOME", &self.data_home)
            .env("HOME", &self.data_home)
            .env_remove("OULIPOLY_DATA_DIR")
            .env_remove("OULIPOLY_PARENT_INVOCATION")
            .env_remove("AGENT_BASH_OWNER_INVOCATION_UUID")
            .env_remove("AGENT_BASH_OWNER_SESSION_ID")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }
}

#[test]
fn all_success_carriers_refuse_damaged_sidecar_then_finalize_retained_outcome_after_restoration() {
    for carrier in [Carrier::Balancing, Carrier::Repl, Carrier::Resume] {
        let (fixture, child, invocation_uuid) = prepare_registered_carrier(carrier);

        let sidecar = MailboxDb::path_for_state_db(&fixture.state_path());
        let sidecar_connection = Connection::open(&sidecar).unwrap();
        let retained_summary = sidecar_connection
            .query_row(
                "SELECT materialized_count, authority_ordinal, sidecar_generation, continuity_digest
                 FROM completion_authority_materialization_summary
                 WHERE invocation_uuid = ?1",
                [&invocation_uuid],
                |row| {
                    Ok(MaterializationSummary {
                        materialized_count: row.get(0)?,
                        authority_ordinal: row.get(1)?,
                        sidecar_generation: row.get(2)?,
                        continuity_digest: row.get(3)?,
                    })
                },
            )
            .unwrap();
        sidecar_connection
            .execute(
                "UPDATE completion_authority_materialization_summary
                 SET materialized_count = materialized_count + 1
                 WHERE invocation_uuid = ?1",
                [&invocation_uuid],
            )
            .unwrap();
        drop(sidecar_connection);
        fs::write(&fixture.release, b"release\n").unwrap();
        let output = child.wait_with_output().unwrap();

        assert_refusal_oracle(carrier, &fixture, &invocation_uuid, &output);

        Connection::open(&sidecar)
            .unwrap()
            .execute(
                "UPDATE completion_authority_materialization_summary
                 SET materialized_count = ?2, authority_ordinal = ?3,
                     sidecar_generation = ?4, continuity_digest = ?5
                 WHERE invocation_uuid = ?1",
                params![
                    invocation_uuid,
                    retained_summary.materialized_count,
                    retained_summary.authority_ordinal,
                    retained_summary.sidecar_generation,
                    retained_summary.continuity_digest,
                ],
            )
            .unwrap();
        let restored_state = StateDb::open(&fixture.state_path()).unwrap();
        let row = restored_state
            .get_invocation_by_uuid(&invocation_uuid)
            .unwrap()
            .unwrap();
        restored_state
            .finalize_invocation(row.id, true, 0, None, None)
            .unwrap();
        let finalized = restored_state
            .get_invocation_by_uuid(&invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(finalized.status, InvocationStatus::Succeeded, "{carrier:?}");
    }
}

#[test]
fn all_success_carriers_retry_the_same_outcome_when_contention_releases() {
    for carrier in [Carrier::Balancing, Carrier::Repl, Carrier::Resume] {
        let (fixture, mut child, invocation_uuid) = prepare_registered_carrier(carrier);
        let sidecar_authority = stage_finalization_only_contention(&fixture, &mut child);
        std::thread::sleep(Duration::from_millis(5_250));
        <fs::File as fs4::FileExt>::unlock(&sidecar_authority).unwrap();

        let output = child.wait_with_output().unwrap();

        assert_eq!(output.status.code(), Some(0), "{carrier:?}: {output:?}");
        let row = StateDb::open(&fixture.state_path())
            .unwrap()
            .get_invocation_by_uuid(&invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Succeeded, "{carrier:?}");
        assert_eq!(row.success, Some(true), "{carrier:?}");
        assert_eq!(row.exit_code, Some(0), "{carrier:?}");
    }
}

#[test]
fn all_success_carriers_exhaust_contention_without_a_terminal_result() {
    for carrier in [Carrier::Balancing, Carrier::Repl, Carrier::Resume] {
        let (fixture, mut child, invocation_uuid) = prepare_registered_carrier(carrier);
        let sidecar_authority = stage_finalization_only_contention(&fixture, &mut child);
        std::thread::sleep(Duration::from_secs(17));
        <fs::File as fs4::FileExt>::unlock(&sidecar_authority).unwrap();

        let output = child.wait_with_output().unwrap();

        assert_eq!(output.status.code(), Some(1), "{carrier:?}: {output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stdout
                .lines()
                .any(|line| line.starts_with("OULIPOLY_RESULT=")),
            "{carrier:?}: {stdout}"
        );
        assert!(
            stderr.contains("completion_authority_contention"),
            "{carrier:?}: {stderr}"
        );
        assert!(
            !fixture.result_artifact(&invocation_uuid).exists(),
            "{carrier:?}"
        );
        let row = StateDb::open(&fixture.state_path())
            .unwrap()
            .get_invocation_by_uuid(&invocation_uuid)
            .unwrap()
            .unwrap();
        assert_eq!(row.status, InvocationStatus::Running, "{carrier:?}");
        assert_eq!(row.success, None, "{carrier:?}");
        assert_eq!(row.exit_code, None, "{carrier:?}");
    }
}

fn prepare_registered_carrier(carrier: Carrier) -> (Fixture, Child, String) {
    let fixture = Fixture::new(carrier);
    let mut child = fixture.spawn(carrier);
    if !wait_for_path_or_exit(&fixture.prepared, &mut child) {
        let output = child.wait_with_output().unwrap();
        panic!(
            "{carrier:?} exited before preparing registration: {output:?}; child env: {:?}",
            fs::read_to_string(&fixture.child_env),
        );
    }
    let child_env = fs::read_to_string(&fixture.child_env).unwrap();
    let mut env_lines = child_env.lines();
    let invocation_uuid = env_lines.next().unwrap().to_string();
    assert_eq!(env_lines.next(), Some("64"), "{carrier:?}: {child_env}");
    assert_eq!(env_lines.next(), Some("false"), "{carrier:?}: {child_env}");

    let state = StateDb::open(&fixture.state_path()).unwrap();
    let running = state
        .get_invocation_by_uuid(&invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(running.status, InvocationStatus::Running, "{carrier:?}");
    if running.provider_session_id.as_deref() != Some(SESSION_ID) {
        state
            .bind_invocation_provider_session_start(
                running.id,
                &ProviderSessionBinding {
                    provider_session_id: SESSION_ID.to_string(),
                    capture_method: "age299_s2_production_binary_fixture",
                    resume_input_id: None,
                    provider_session_resolved_account: Some(PROVIDER.to_string()),
                },
            )
            .unwrap();
    }
    drop(state);
    fs::write(&fixture.bind, b"bind\n").unwrap();
    if !wait_for_path_or_exit(&fixture.registered, &mut child) {
        let output = child.wait_with_output().unwrap();
        panic!(
            "{carrier:?} exited before registration: {output:?}; nested stdout: {}; nested stderr: {}",
            fs::read_to_string(&fixture.registration_stdout).unwrap_or_default(),
            fs::read_to_string(&fixture.registration_stderr).unwrap_or_default(),
        );
    }
    let registration = fs::read_to_string(&fixture.registration_stdout).unwrap();
    assert!(
        registration.contains("\"status\": \"registered\""),
        "{carrier:?}: {registration}"
    );
    let state = StateDb::open(&fixture.state_path()).unwrap();
    let obligation_count: i64 = state
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM invocation_completion_obligations
             WHERE invocation_uuid = ?1",
            [&invocation_uuid],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(obligation_count, 1, "{carrier:?}");
    drop(state);
    (fixture, child, invocation_uuid)
}

fn stage_finalization_only_contention(fixture: &Fixture, child: &mut Child) -> fs::File {
    let state_writer = Connection::open(fixture.state_path()).unwrap();
    state_writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    fs::write(&fixture.release, b"release\n").unwrap();
    wait_for_runtime_exit(fixture, child);

    let sidecar = MailboxDb::path_for_state_db(&fixture.state_path());
    let mut authority_path = sidecar.as_os_str().to_os_string();
    authority_path.push(".authority.lock");
    let sidecar_authority = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PathBuf::from(authority_path))
        .unwrap();
    <fs::File as fs4::FileExt>::lock(&sidecar_authority).unwrap();
    state_writer.execute_batch("COMMIT").unwrap();
    sidecar_authority
}

fn wait_for_runtime_exit(fixture: &Fixture, child: &mut Child) {
    let sidecar = MailboxDb::path_for_state_db(&fixture.state_path());
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        let exited = Connection::open(&sidecar)
            .and_then(|connection| {
                connection.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM runtime_generation
                         WHERE provider_name = ?1 AND lifecycle_state = 'exited'
                     )",
                    [PROVIDER],
                    |row| row.get::<_, bool>(0),
                )
            })
            .unwrap_or(false);
        if exited {
            return;
        }
        if child.try_wait().unwrap().is_some() {
            panic!("production carrier exited before runtime teardown completed");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for production runtime teardown");
}

fn assert_refusal_oracle(
    carrier: Carrier,
    fixture: &Fixture,
    invocation_uuid: &str,
    output: &Output,
) {
    assert_eq!(output.status.code(), Some(1), "{carrier:?}: {output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.lines().any(|line| {
            line.starts_with("OULIPOLY_RESULT=") && line.contains("\"success\":true")
        }),
        "{carrier:?}: {stdout}"
    );
    assert!(
        stderr.contains("process_integrity") && stderr.contains("sidecar materialization summary"),
        "{carrier:?}: {stderr}"
    );
    assert!(
        !fixture.result_artifact(invocation_uuid).exists(),
        "{carrier:?}"
    );
    let row = StateDb::open(&fixture.state_path())
        .unwrap()
        .get_invocation_by_uuid(invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Running, "{carrier:?}");
    assert_eq!(row.success, None, "{carrier:?}");
    assert_eq!(row.exit_code, None, "{carrier:?}");
}

fn provider_script(root: &Path, paths: ProviderScriptPaths<'_>) -> String {
    let ProviderScriptPaths {
        registered,
        prepared,
        bind,
        release,
        child_env,
        registration_stdout,
        registration_stderr,
    } = paths;
    let runner = env!("CARGO_BIN_EXE_oulipoly-agent-runner");
    let state_dir = root.join("agent-bash-artifacts");
    let meta = state_dir.join("meta.json");
    let log = state_dir.join("log");
    let rc = state_dir.join("rc");
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
mkdir -p {state_dir:?}
invocation_uuid="$(python3 -c 'import json,os; print(json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])["id"])')"
authority_len="${{#OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY}}"
identity_has_authority="$(python3 -c 'import json,os; print(str("completion_registration_authority" in json.loads(os.environ["OULIPOLY_PARENT_INVOCATION"])).lower())')"
printf '%s\n%s\n%s\n' "$invocation_uuid" "$authority_len" "$identity_has_authority" > {child_env:?}
python3 - "$invocation_uuid" {meta:?} <<'PY'
import json, sys
with open(sys.argv[2], "w") as target:
    json.dump({{"owner_session_id": "age299-s2-live-session", "owner_invocation_uuid": sys.argv[1], "caller_chain": []}}, target)
PY
printf '%s\n' 'registered completion' > {log:?}
printf '%s\n' '0' > {rc:?}
touch {prepared:?}
while [ ! -f {bind:?} ]; do sleep 0.01; done
{runner:?} notify agent-bash-register --handle age299-s2-carrier-handle --delivery-mode async --state-dir {state_dir:?} --meta {meta:?} --log {log:?} --rc {rc:?} --json > {registration_stdout:?} 2> {registration_stderr:?}
touch {registered:?}
while [ ! -f {release:?} ]; do sleep 0.01; done
printf '%s\n' 'retained provider stdout'
exit 0
"#,
    )
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn wait_for_path_or_exit(path: &Path, child: &mut Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        if child.try_wait().unwrap().is_some() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {}", path.display());
}
