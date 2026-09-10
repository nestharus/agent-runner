//! Real runner → native fake provider → agent-bash producer → immediate ACK.
use crate::{MODEL, SESSION, fake_provider, fixtures::Fixture, liveness::wait_until};
use rusqlite::Connection;
use std::process::Command;

pub(crate) fn immediate_ack(completed: bool) {
    run_ack_case(completed, "none");
}

pub(crate) fn unpause_ack(mode: &str) {
    run_ack_case(true, mode);
}

fn run_ack_case(completed: bool, unpause_mode: &str) {
    let _guard = crate::test_guard::integration_test_guard();
    let fixture = Fixture::new();
    let helper = std::env::var_os("AGENT_BASH_BIN")
        .map(std::path::PathBuf::from)
        .expect("explicit AGENT_BASH_BIN required for real dispatch fixture");
    let helper = fixture.install_agent_bash(&helper);
    let runner = helper
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("runner/oulipoly-agent-runner");
    let hook_source = format!(
        "UNPAUSE_MODE = {unpause_mode:?}\n{}",
        include_str!("early_ack_hook.py")
    );
    let hook = fixture.write_executable("early_ack_hook.py", &hook_source);
    let mut provider = fake_provider::provider_script(
        &format!("python3 '{}' initial", hook.display()),
        &format!("python3 '{}' resume", hook.display()),
        "early-ack-receipt-${WU_D_PROVIDER_RESUME_INDEX}.txt",
    )
    .replace(
        "env.update({\"work\":",
        "env[\"FIXTURE_PROVIDER_PID\"] = str(os.getpid())\n    env.update({\"work\":",
    );
    let completion = if completed {
        r#"    if code == 0:
        stdout += b"Deterministic assistant completed real receipt, log read and exact ACK.\n"
        event(request, seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1

    data_event_count = 0"#
    } else {
        // Genuine provider error after durable ACK must still fail the invocation.
        r#"    if resumed and code == 0:
        code = 23
    elif code == 0:
        event(request, seq, "marker", name="oulipoly.produced_assistant_response", value=True)
        seq += 1

    data_event_count = 0"#
    };
    provider = provider.replace("    data_event_count = 0", completion);
    fixture.write_provider(&provider);
    let mut cmd = Command::new(&runner);
    cmd.args(["-m", MODEL, "--models-dir"])
        .arg(&fixture.models_dir)
        .arg("Real immediate ACK regression");
    let output = fixture.run(cmd);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    if unpause_mode == "sleeping" {
        let mailbox = fixture.mailbox();
        assert!(mailbox.notifications_paused(SESSION).unwrap());
        let pending = mailbox.list_pending(SESSION).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].delivery_attempts, 0);
        assert!(!fixture.prompt_file("early-ack-verified.json").exists());
        let mut command = Command::new(&runner);
        command.args(["mailbox", "resume", "--session-id", SESSION, "--json"]);
        let unpause = fixture.run(command);
        assert_eq!(unpause.status.code(), Some(0), "{unpause:?}");
        let response: serde_json::Value = serde_json::from_slice(&unpause.stdout).unwrap();
        assert_eq!(response["paused"], false);
        assert_eq!(response["wake"]["status"], "spawned");
    }
    wait_until("immediate ACK and both terminal invocations", || {
        if !fixture.prompt_file("early-ack-verified.json").exists() {
            return false;
        }
        let db = Connection::open(fixture.state_path()).unwrap();
        db.query_row(
            "SELECT COUNT(*) FROM invocations WHERE status != 'running'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
            == 2
    });
    let db = Connection::open(fixture.state_path()).unwrap();
    let mut query = db
        .prepare("SELECT status,success,exit_code,terminal_reason FROM invocations ORDER BY id")
        .unwrap();
    let invocations = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(invocations.len(), 2);
    assert!(
        invocations
            .iter()
            .all(|row| row.3.as_deref() != Some("guard_drop")),
        "{invocations:?}"
    );
    if completed {
        assert!(
            invocations
                .iter()
                .all(|row| row.0 == "succeeded" && row.1 && row.2 == 0),
            "{invocations:?}"
        );
    } else {
        assert!(
            invocations
                .iter()
                .any(|row| row.0 == "failed" && !row.1 && row.2 == 23),
            "{invocations:?}"
        );
    }
    let mailbox = fixture.mailbox();
    let rows = mailbox.list_mailbox(SESSION, true).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].delivered_at.is_some());
    assert_eq!(
        rows[0].delivered_by_invocation_uuid.as_deref(),
        Some("real-fixture-consumer")
    );
    assert_eq!(rows[0].delivery_attempts, 1);
    assert!(rows[0].delivery_error.is_none());
    assert!(
        mailbox
            .unresolved_delivery_attempt_windows(SESSION)
            .unwrap()
            .is_empty()
    );
    assert!(mailbox.mailbox_observation_stop(SESSION).unwrap().is_none());
    let sidecar = Connection::open(fixture.sidecar_path()).unwrap();
    let reason: String = sidecar
        .query_row(
            "SELECT acknowledgement_reason FROM completion_event_listener",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "manual_ack");
    let confirmations: i64 = sidecar.query_row("SELECT COUNT(*) FROM mailbox_delivery_attempts WHERE observation_confirmed_at IS NOT NULL OR acknowledged_at IS NOT NULL", [], |row| row.get(0)).unwrap();
    assert_eq!(
        confirmations, 0,
        "manual ACK must not invent automatic observation or transport ACK"
    );
    let confirmations: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM session_delivery_acknowledgements",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        confirmations, 0,
        "assistant success cannot manufacture delivery confirmation after consumer ACK"
    );
    if unpause_mode != "none" {
        let mut command = Command::new(&runner);
        command.args(["mailbox", "resume", "--session-id", SESSION, "--json"]);
        let settled = fixture.run(command);
        assert_eq!(settled.status.code(), Some(0), "{settled:?}");
        let response: serde_json::Value = serde_json::from_slice(&settled.stdout).unwrap();
        assert_eq!(response["wake"]["status"], "no_pending");
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM invocations", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
