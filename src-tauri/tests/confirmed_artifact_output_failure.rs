#![cfg(unix)]
//! Returned-reference custody is independent of successful raw-output persistence.
//! Intent: README headless resume returned_artifacts contract and root's bounded
//! localized-output-failure disposition; no full raw-output recovery guarantee.
//! Roles: orchestration, validator, accessor.

#[path = "fixtures/bounded_runner_image.rs"]
mod bounded_runner_image;
mod provider_authority_fixture;

use oulipoly_agent_messenger::{ReturnName, ReturnRequest, ReturnSource, ShowReturnedRequest};
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output};

const SESSION: &str = "e6b7a4c2-97bf-4d4a-8ab3-136dc0c121c1";
const PAYLOAD: &[u8] = b"independently retained artifact";

#[test]
#[ignore = "private fake provider invokes this exact node with its actual launch environment"]
fn messenger_return_child() {
    let root = std::path::PathBuf::from(std::env::var_os("ARTIFACT_FIXTURE_ROOT").unwrap());
    let identity: Value =
        serde_json::from_str(&std::env::var("OULIPOLY_PARENT_INVOCATION").unwrap()).unwrap();
    let receipt = oulipoly_agent_messenger::return_artifact(ReturnRequest {
        db_path: root.join("store.db"),
        invocation_uuid: identity["id"].as_str().unwrap().parse().unwrap(),
        name: ReturnName::new("result-note").unwrap(),
        source: ReturnSource::InlineBytes(PAYLOAD.to_vec()),
        format_hint: None,
        verdict_line: None,
        return_channel: Some(std::env::var_os("OULIPOLY_RETURN_CHANNEL").unwrap().into()),
    })
    .unwrap();
    fs::write(
        root.join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}

struct Fixture(tempfile::TempDir);
impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        let root = fixture.0.path();
        for name in [
            "home",
            "config/oulipoly-agent-runner/models",
            "data",
            "state",
            "cache",
            "runtime",
            "provider-home",
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        drop(oulipoly_agent_store::Store::init(root.join("store.db")).unwrap());
        let script = root.join("provider.py");
        fs::write(
            &script,
            include_str!("fixtures/confirmed_artifact_provider.py"),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let config = root.join("config/oulipoly-agent-runner");
        fs::write(
            config.join("models/artifact-model.toml"),
            "[[providers]]\nname = \"fixture-account\"\nargs = []\n",
        )
        .unwrap();
        fs::write(config.join("providers.toml"), provider_authority_fixture::with_explicit_provider_authority_at(
            "[fixture-account]\ncommand = \"fixture-engine\"\nargs = []\nprompt_mode = \"arg\"\n",
            "artifact-fixture", &script)).unwrap();
        fixture
    }

    fn run(&self, resume: bool, obstruct: bool, assistant: bool) -> Output {
        let root = self.0.path();
        let mut cmd = Command::new(bounded_runner_image::runner_bin());
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("CODEX_HOME", root.join("provider-home"))
            .env("OULIPOLY_DATA_DIR", root.join("data/oulipoly-agent-runner"))
            .env("ARTIFACT_FIXTURE_ROOT", root)
            .env("ARTIFACT_FIXTURE_TEST", std::env::current_exe().unwrap())
            .env(
                "ARTIFACT_FIXTURE_OBSTRUCT",
                if obstruct { "1" } else { "0" },
            )
            .env(
                "ARTIFACT_FIXTURE_NO_ASSISTANT",
                if assistant { "0" } else { "1" },
            )
            .current_dir(root);
        if resume {
            cmd.args([
                "resume",
                "--session-id",
                SESSION,
                "--submission-token",
                "private-input",
                "--prompt",
                "consume accepted input",
            ]);
        } else {
            cmd.arg("seed fixture session");
        }
        cmd.args(["--model", "artifact-model"])
            .arg("--models-dir")
            .arg(root.join("config/oulipoly-agent-runner/models"));
        let output = cmd.output().unwrap();
        eprintln!(
            "command={cmd:?}\nstatus={:?}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn check(&self, obstruct: bool, assistant: bool) {
        assert!(self.run(false, false, true).status.success());
        let output = self.run(true, obstruct, assistant);
        let root = self.0.path();
        let receipt: Value =
            serde_json::from_slice(&fs::read(root.join("receipt.json")).unwrap()).unwrap();
        let id = receipt["producer_invocation_uuid"].as_str().unwrap();
        let payload = oulipoly_agent_messenger::show_returned(ShowReturnedRequest::VersionId {
            db_path: root.join("store.db"),
            version_id: receipt["version_id"].as_str().unwrap().into(),
        })
        .unwrap();
        assert_eq!(payload.content, PAYLOAD);
        let db = Connection::open(root.join("data/oulipoly-agent-runner/state.db")).unwrap();
        let outcome: (String, i32, Option<String>, i64) = db.query_row(
            "SELECT status,exit_code,error_category,(SELECT COUNT(*) FROM invocation_returned_artifacts a WHERE a.invocation_id=i.id) FROM invocations i WHERE invocation_uuid=?1", [id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
        eprintln!("outcome={outcome:?}; receipt={receipt}");
        assert_eq!(
            db.query_row(
                "SELECT COUNT(*) FROM provider_launch_attempts WHERE invocation_uuid=?1",
                [id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "ordinary, not native-precommitted"
        );
        let refs: (String, String) = db.query_row("SELECT version_id,sha256 FROM invocation_returned_artifacts a JOIN invocations i ON i.id=a.invocation_id WHERE i.invocation_uuid=?1", [id], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(refs.0, receipt["version_id"].as_str().unwrap());
        assert_eq!(refs.1, receipt["sha256"].as_str().unwrap());
        assert_eq!(outcome.3, 1);
        let events: Vec<Value> = fs::read_to_string(root.join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let accepted = events
            .iter()
            .find(|v| v["name"] == "oulipoly.prompt_accepted/v1")
            .unwrap();
        let launches: Vec<Value> = fs::read_to_string(root.join("launches.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let requested = &launches.last().unwrap()["params"]["prompt_acceptance"];
        assert_eq!(
            accepted["value"]["delivery_nonce"],
            requested["delivery_nonce"]
        );
        assert_eq!(
            accepted["value"]["prompt_sha256"],
            requested["prompt_sha256"]
        );
        assert!(requested["delivery_nonce"].is_string());
        if obstruct || !assistant {
            assert_eq!(output.status.code(), Some(1));
            assert_eq!(outcome.0, "failed");
            // README: failed/non-spooled results retain a stdout result marker.
            // A truthful failure envelope is not raw provider payload delivery.
            let stdout = std::str::from_utf8(&output.stdout).unwrap();
            assert_eq!(stdout.lines().count(), 1);
            let envelope: Value =
                serde_json::from_str(stdout.trim_end().strip_prefix("OULIPOLY_RESULT=").unwrap())
                    .unwrap();
            assert_eq!(envelope["id"], id);
            assert_eq!(envelope["success"], false);
            assert_eq!(envelope["status"], "failed");
            assert_eq!(envelope["error_category"], outcome.2.as_deref().unwrap());
            assert!(!stdout.contains("completed fixture response"));
            assert_eq!(db.query_row("SELECT COUNT(*) FROM invocation_output_deliveries WHERE invocation_uuid=?1 AND delivery_state='delivered'", [id], |r| r.get::<_,i64>(0)).unwrap(), 0);
            if assistant {
                assert_eq!(outcome.1, 1);
                assert_eq!(db.query_row("SELECT COUNT(*) FROM session_delivery_acknowledgements WHERE delivery_id=?1", [requested["delivery_nonce"].as_str().unwrap()], |r| r.get::<_,i64>(0)).unwrap(), 0);

                assert_eq!(outcome.2.as_deref(), Some("terminal_persistence"));
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("failed to persist provider output")
                );
            } else {
                assert_eq!(outcome.2.as_deref(), Some("resume_completion_unconfirmed"));
            }
        } else {
            assert_eq!(output.status.code(), Some(0));
            assert_eq!(outcome.0, "succeeded");
            assert_eq!(output.stdout, b"completed fixture response\n");
        }
    }
}

#[test]
fn confirmed_output_failure_keeps_actual_returned_reference() {
    Fixture::new().check(true, true);
}

#[test]
fn confirmed_output_success_keeps_actual_returned_reference() {
    Fixture::new().check(false, true);
}

#[test]
fn prompt_acceptance_does_not_invent_assistant_completion() {
    Fixture::new().check(true, false);
}
