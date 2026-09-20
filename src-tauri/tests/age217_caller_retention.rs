#![cfg(unix)]
//! Actual CLI resume/balanced boundaries, using only generated private providers/state.

use oulipoly_agent_messenger::{ReturnName, ReturnRequest, ReturnSource};
use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const REASON: &str = "live_session_authority_publication_failed";

// Called only by the private fake provider, using the exact allocated invocation
// identity. Ordinary harness discovery does nothing; no default path is allowed.
#[test]
fn produce_reference() {
    let Ok(uuid) = std::env::var("AGE217_PRODUCER_UUID") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("AGE217_FIXTURE_ROOT").unwrap());
    let channel = PathBuf::from(std::env::var_os("AGE217_RETURN_CHANNEL").unwrap());
    oulipoly_agent_store::Store::init(root.join("artifact-store.db")).unwrap();
    let reference = oulipoly_agent_messenger::return_artifact(ReturnRequest {
        db_path: root.join("artifact-store.db"),
        invocation_uuid: uuid.parse().unwrap(),
        name: ReturnName::new("fixture").unwrap(),
        source: ReturnSource::InlineBytes(b"authentic-produced-artifact".to_vec()),
        format_hint: None,
        verdict_line: None,
        return_channel: Some(channel),
    })
    .unwrap();
    fs::write(
        root.join("produced-ref.json"),
        serde_json::to_vec(&reference).unwrap(),
    )
    .unwrap();
}

struct Fixture {
    root: tempfile::TempDir,
    config: PathBuf,
    data: PathBuf,
    models: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let data = root.path().join("data");
        let models = config.join("oulipoly-agent-runner/models");
        fs::create_dir_all(&models).unwrap();
        let script = root.path().join("provider.py");
        fs::write(
            &script,
            include_str!("fixtures/age217/caller-retention-provider.py").replace(
                "AGE217_EXPLICIT_FIXTURE_ROOT",
                &serde_json::to_string(root.path().to_str().unwrap()).unwrap(),
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            root.path().join("helper-path"),
            std::env::current_exe().unwrap().to_str().unwrap(),
        )
        .unwrap();
        fs::write(
            models.join("fixture.toml"),
            "[[providers]]\nname = 'fixture'\nargs = []\n",
        )
        .unwrap();
        fs::write(
            config.join("oulipoly-agent-runner/providers.toml"),
            format!(
                r#"
[fixture]
command = "/nonexistent/age217-no-legacy-fallback"
prompt_mode = "arg"
settings_id = "fixture-settings"
[fixture.implementation]
family = "caller-fixture"
executable = '{}'
[fixture.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"
[fixture.resume]
kind = "flag"
flag = "--resume"
"#,
                script.display()
            ),
        )
        .unwrap();
        Self {
            root,
            config,
            data,
            models,
        }
    }
    fn state(&self) -> StateDb {
        StateDb::open(&self.data.join("state.db")).unwrap()
    }
    fn seed_resume(&self) {
        let state = self.state();
        let conn = rusqlite::Connection::open(state.path()).unwrap();
        conn.execute("INSERT INTO session_chains(chain_id,created_at,last_used_at,model_name) VALUES ('fixture-chain','2026-09-01T00:00:00Z','2026-09-01T00:00:00Z','fixture')", []).unwrap();
        conn.execute("INSERT INTO session_chain_segments(chain_id,provider_name,session_id,started_at,transition_reason) VALUES ('fixture-chain','fixture',?1,'2026-09-01T00:00:00Z','initial')", [SESSION]).unwrap();
        conn.execute("INSERT INTO session_chain_segment_provider_authority(segment_id,provider_instance_id,settings_id) SELECT id,'caller-fixture-instance','fixture-settings' FROM session_chain_segments", []).unwrap();
        conn.execute("INSERT INTO imported_session_display_metadata(provider_name,provider_session_id,cwd,first_seen_at,last_seen_at) VALUES ('fixture',?1,?2,'2026-09-01T00:00:00Z','2026-09-01T00:00:00Z')", rusqlite::params![SESSION,self.root.path().to_str().unwrap()]).unwrap();
    }
    fn command(&self, resume: bool) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_DATA_HOME", self.root.path().join("xdg-data"))
            .env("XDG_STATE_HOME", self.root.path().join("xdg-state"))
            .env("OULIPOLY_CONFIG_HOME", "")
            .env("OULIPOLY_DATA_DIR", &self.data)
            .env("TMPDIR", self.root.path())
            .current_dir(self.root.path());
        if resume {
            cmd.args([
                "resume",
                "--session-id",
                SESSION,
                "--prompt",
                "fixture prompt",
            ]);
        }
        cmd.arg("-p").arg(self.root.path());
        cmd.args(["-m", "fixture", "--models-dir"])
            .arg(&self.models);
        if !resume {
            cmd.arg("fixture prompt");
        }
        cmd
    }
}

fn rejects_with_retention(resume: bool, blocked_output: bool, persistence_cause: bool) {
    let f = Fixture::new();
    if resume {
        f.seed_resume();
    }
    let state = f.state();
    if persistence_cause {
        fs::write(f.root.path().join("publication-storage-fault"), b"private").unwrap();
    }
    if blocked_output {
        let paths = state
            .invocation_output_artifact_paths("probe")
            .unwrap()
            .unwrap();
        let parent = paths.stdout.parent().unwrap();
        if parent.exists() {
            fs::remove_dir(parent).unwrap();
        }
        fs::write(parent, b"fixture blocks output storage").unwrap();
    }
    let output = f.command(resume).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!(
        "caller={} blocked_output={blocked_output} exit={:?}\n{stderr}",
        if resume { "resume" } else { "balanced" },
        output.status.code()
    );
    assert!(!output.status.success(), "{output:?}");
    let raw = stderr
        .lines()
        .find_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .expect("actual caller invocation");
    let invocation = CompositeInvocationId::parse_env_value(raw).unwrap();
    assert!(
        f.root.path().join("launch.json").exists(),
        "must reach fake launch"
    );
    assert!(
        stderr.contains("authoritative session observation missing"),
        "{stderr}"
    );
    assert!(
        stderr.contains(REASON),
        "original publication reason: {stderr}"
    );
    let markers: Vec<serde_json::Value> = stderr
        .lines()
        .filter_map(|line| {
            line.strip_prefix("OULIPOLY_TERMINAL_SIGNAL=")
                .map(|json| serde_json::from_str(json).unwrap())
        })
        .collect();
    assert_eq!(
        markers.len(),
        1,
        "original marker, not secondary disposition: {stderr}"
    );
    let marker = &markers[0];
    assert_eq!(marker["invocation_id"], invocation.id);
    assert_eq!(marker["kind"], "SpawnError");
    assert!(
        marker["session_id"].is_null(),
        "no invented validated session"
    );
    let evidence = marker["evidence"]["excerpt"].as_str().unwrap();
    let cause = if persistence_cause {
        "session_identity_commit_failed"
    } else {
        "session_identity_mismatch"
    };
    assert!(evidence.contains(cause), "original host cause: {evidence}");
    assert!(evidence.contains("cleanup="));
    assert!(!evidence.contains("authoritative session observation missing"));
    assert!(
        !evidence.contains("PRIVATE_STORAGE_SECRET"),
        "host errors stay bounded"
    );
    let row = state
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    let conn = rusqlite::Connection::open(state.path()).unwrap();
    let (reason, session): (String, Option<String>) = conn
        .query_row(
            "SELECT terminal_reason,provider_session_id FROM invocations WHERE id=?1",
            [row.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        reason, REASON,
        "guard_drop must not replace produced failure"
    );
    assert_eq!(session, None, "expected session is not an observation");
    let reference: oulipoly_runtime::executor::ReturnedArtifactRef =
        serde_json::from_slice(&fs::read(f.root.path().join("produced-ref.json")).unwrap())
            .unwrap();
    assert_eq!(
        state.list_returned_artifacts(row.id).unwrap(),
        vec![reference.clone()]
    );
    assert!(
        state
            .invocation_provider_session_authority(row.id)
            .unwrap()
            .is_none()
    );
    let produced = oulipoly_agent_messenger::show_returned(
        oulipoly_agent_messenger::ShowReturnedRequest::VersionId {
            db_path: f.root.path().join("artifact-store.db"),
            version_id: reference.version_id.clone(),
        },
    )
    .unwrap();
    assert_eq!(produced.content, b"authentic-produced-artifact");
    assert_eq!(produced.meta.sha256, reference.sha256);
    let sidecar = rusqlite::Connection::open(f.data.join("pid-identity.db")).unwrap();
    let (generation_session, lifecycle): (Option<String>, String) = sidecar.query_row(
        "SELECT session_id,lifecycle_state FROM runtime_generation WHERE spawn_invocation_uuid=?1",
        [&invocation.id], |row| Ok((row.get(0)?,row.get(1)?)),
    ).unwrap();
    let launch: serde_json::Value =
        serde_json::from_slice(&fs::read(f.root.path().join("launch.json")).unwrap()).unwrap();
    let expected_session = launch["params"]["session"]["known_provider_session_id"]
        .as_str()
        .unwrap();
    assert_ne!(expected_session, "22222222-2222-4222-8222-222222222222");
    // Spawn custody records the start-known expectation before publication; that
    // association is not a State-authoritative observation of the rejected marker.
    assert_eq!(generation_session.as_deref(), Some(expected_session));
    assert_eq!(lifecycle, "exited");
    let deliveries: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM invocation_output_deliveries WHERE invocation_id=?1",
            [row.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        deliveries, 0,
        "partial prefix must not claim complete delivery"
    );
    if blocked_output {
        assert!(
            stderr.contains("artifacts=retained;output=storage_failure"),
            "{stderr}"
        );
    } else {
        let paths = state
            .invocation_output_artifact_paths(&format!("{}.partial", invocation.id))
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(paths.stdout).unwrap(), [0, 1, 255]);
        assert_eq!(fs::read(paths.stderr).unwrap(), [101, 114, 114, 255, 254]);
    }
    assert!(!stderr.contains("guard_drop"));
}

#[test]
fn resume_authority_rejection_retains_produced_failure() {
    rejects_with_retention(true, false, false);
}
#[test]
fn balanced_expected_session_rejection_retains_produced_failure() {
    rejects_with_retention(false, false, false);
}
#[test]
fn resume_authority_rejection_reports_storage_failure_and_finalizes_original_reason() {
    rejects_with_retention(true, true, false);
}
#[test]
fn balanced_authority_rejection_reports_storage_failure_and_finalizes_original_reason() {
    rejects_with_retention(false, true, false);
}

#[test]
fn resume_rejection_carries_host_persistence_cause_not_missing_observation() {
    rejects_with_retention(true, false, true);
}
#[test]
fn balanced_rejection_carries_host_persistence_cause_not_missing_observation() {
    rejects_with_retention(false, false, true);
}
