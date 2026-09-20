//! Real allocated Completed -> later caller recommit fault, not MissingObservation.
use super::*;

#[test]
fn native_allocated_recommit_rejection_settles_failed_logical_launch() {
    if private_case(false) {
        return;
    }
    allocated_recommit_rejection(false);
}

#[test]
fn native_allocated_recommit_rejection_reports_settlement_storage_failure() {
    if private_case(false) {
        return;
    }
    allocated_recommit_rejection(true);
}

fn allocated_recommit_rejection(block_settlement: bool) {
    let f = Fixture::new("owner_only");
    f.gate("release-resume");
    let mut initial = f.start_with_hold(true);
    f.owner();
    wait(|| {
        f.root
            .path()
            .join("provider-initial-ready")
            .exists()
            .then_some(())
    });
    let state = oulipoly_state::StateDb::open(&f.data.join("state.db")).unwrap();
    let faults = rusqlite::Connection::open(state.path()).unwrap();
    // This trigger cannot reject initial publication, prompt acceptance or Exit
    // observation. Only the later binding write after real retained native custody
    // is faulted. Other State writes remain available; no product hook or ACK.
    faults.execute_batch("CREATE TRIGGER private_later_recommit BEFORE UPDATE OF provider_session_id ON invocations WHEN EXISTS (
        SELECT 1 FROM provider_launch_attempts a JOIN provider_launch_transition_replays r ON r.logical_launch_id=a.logical_launch_id
        WHERE a.invocation_id=OLD.id AND r.operation_key=a.attempt_id || '/native-custody-receipts'
    ) BEGIN SELECT RAISE(ABORT, 'private later caller recommit storage fault'); END;").unwrap();
    if block_settlement {
        faults.execute_batch("CREATE TRIGGER private_settlement_fault BEFORE UPDATE OF status ON provider_logical_launches WHEN NEW.status='failed' BEGIN SELECT RAISE(ABORT, 'private logical settlement storage fault'); END;").unwrap();
    }
    MailboxDb::open(&f.data.join("pid-identity.db"))
        .unwrap()
        .enqueue_submitted_input(&oulipoly_state::mailbox::SubmittedInputEnqueue {
            submission_token: "native-recommit-input",
            target: oulipoly_state::mailbox::InboxTarget {
                kind: oulipoly_state::mailbox::InboxTargetKind::Session,
                id: SESSION,
            },
            input: b"native-recommit-input",
        })
        .unwrap();
    f.gate("release-initial-provider");
    f.wait_initial(&mut initial);
    let (row, invocation, launch, attempt, reason, code): (
        i64,
        String,
        String,
        String,
        String,
        i32,
    ) = wait(|| {
        state.connection().query_row("SELECT i.id,i.invocation_uuid,l.logical_launch_id,a.attempt_id,i.terminal_reason,i.exit_code
            FROM invocations i JOIN provider_launch_attempts a ON a.invocation_id=i.id
            JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id
            WHERE i.status='failed'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).ok()
    });
    assert_eq!(reason, "session_authority_rejected");
    assert_eq!(
        code, -1,
        "a physically clean result cannot turn rejection into success"
    );
    let bound: String = state
        .connection()
        .query_row(
            "SELECT provider_session_id FROM invocations WHERE id=?1",
            [row],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        bound, SESSION,
        "allocated Completed retained the actually verified session"
    );
    assert!(
        state
            .invocation_provider_session_authority(row)
            .unwrap()
            .is_some()
    );
    let receipts: i64 = state.connection().query_row("SELECT COUNT(*) FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key=?2", rusqlite::params![launch,format!("{attempt}/native-custody-receipts")], |r|r.get(0)).unwrap();
    assert_eq!(
        receipts, 1,
        "original producer custody precedes caller fault"
    );
    let paths = state
        .invocation_output_artifact_paths(&invocation)
        .unwrap()
        .unwrap();
    assert_eq!(fs::read(paths.stdout).unwrap(), b"native resumed\n");
    // Wait for independent physical integration, not merely the logical status.
    let drain: String = wait(|| {
        f.sidecar_connection().query_row("SELECT drain_receipt FROM completion_continuation_attempt WHERE integrated=1 AND drain_receipt IS NOT NULL", [], |r|r.get(0)).ok()
    });
    let receipt: serde_json::Value = serde_json::from_str(&drain).unwrap();
    println!(
        "allocated invocation={invocation} logical={launch} attempt={attempt} reason={reason}; original custody={receipts}; independent drain={receipt}"
    );
    assert_eq!(receipt["owned_children"], "ECHILD");
    assert_eq!(receipt["root_exit_code"], 1);
    faults
        .execute_batch("DROP TRIGGER private_later_recommit")
        .unwrap();
    assert_settlement(&state, &attempt, &reason, block_settlement);
    // The enclosing custodian stores the real launcher stderr. Require the actual
    // caller rejection, not only equivalent State fixture rows.
    let mut stderr = String::new();
    diagnostics(&f.data.join("completion-continuation"), &mut stderr);
    println!("allocated caller stderr: {stderr}");
    assert!(
        stderr.contains("private later caller recommit storage fault"),
        "{stderr}"
    );
    let expected_settlement = if block_settlement {
        "private logical settlement storage fault"
    } else {
        "logical_settlement=failed logical launch recorded"
    };
    assert!(stderr.contains(expected_settlement), "{stderr}");
    assert!(
        stderr.contains("finalization=failed invocation recorded"),
        "{stderr}"
    );
    if block_settlement {
        assert!(!stderr.contains("logical_settlement=failed logical launch recorded"));
        faults
            .execute_batch("DROP TRIGGER private_settlement_fault")
            .unwrap();
    }
    assert!(
        !stderr.contains("authoritative session observation missing"),
        "real allocated path, not ordinary fixture"
    );
    assert!(
        !f.root.path().join("recipient-exact-ack.json").exists(),
        "this fixture did not explicitly ACK"
    );
}

fn assert_settlement(state: &oulipoly_state::StateDb, attempt: &str, reason: &str, blocked: bool) {
    let settled: (String,Option<String>,bool,bool,String) = state.connection().query_row("SELECT a.status,a.terminal_code,a.finished_at IS NOT NULL,l.finished_at IS NOT NULL,l.status
        FROM provider_launch_attempts a JOIN provider_logical_launches l USING(logical_launch_id) WHERE a.attempt_id=?1", [attempt], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    let expected = if blocked {
        ("active".into(), None, false, false, "active".into())
    } else {
        (
            "failed".into(),
            Some(reason.into()),
            true,
            true,
            "failed".into(),
        )
    };
    assert_eq!(
        settled, expected,
        "no fabricated logical settlement on persistence error"
    );
}

fn diagnostics(path: &std::path::Path, out: &mut String) {
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        diagnostic_entry(&entry.path(), out);
    }
}

fn diagnostic_entry(path: &std::path::Path, out: &mut String) {
    if path.is_dir() {
        return diagnostics(path, out);
    }
    if !path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("stderr")
    {
        return;
    }
    if let Ok(text) = fs::read_to_string(path) {
        out.push_str(&text);
    }
}
