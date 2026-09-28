//! Source-only private user-namespace exercise of the actual broker launch and
//! opt-in Runner entry. It never exercises installed host-root sudo authority.
#![cfg(all(target_os = "linux", feature = "age319-private-broker-fixture"))]
use oulipoly_kernel_broker::RootRecord;
use oulipoly_kernel_broker::protocol::{
    self, AcceptedWorkSpec, JoinSpec, Operation, ProcessWitness, SourceScope, SourceSocketWitness,
};
use oulipoly_kernel_broker::source_acceptance::{assess_v2_candidate, read_captured_v2_evidence};
use oulipoly_kernel_broker::source_physical::{SourceObservation, SourcePhysicalRegistry};
use oulipoly_state::completion_continuation::AdmittedSourceBinding;
use oulipoly_state::mailbox::{
    AgentBashCompleteEnqueue, BrokerSidecar, EnqueueResult, FreshV30Lane, MailboxDb,
};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[track_caller]
fn eventually(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(
            Instant::now() < until,
            "private root join fixture timed out at {}",
            std::panic::Location::caller()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn bash_retention_check(bash: &str, registration: &str) -> String {
    let output = Command::new(bash)
        .args(["__age319-private-source-retention-check-v1", registration])
        .output()
        .unwrap();
    if !output.status.success() {
        return "error".into();
    }
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn restart_source_broker(
    socket: &Path,
    broker_state: &Path,
    runner: &str,
    gate: &Path,
    log: &Path,
) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"));
    command
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", runner)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", gate)
        .stderr(Stdio::from(File::create(log).unwrap()));
    if gate.join("v2-wake-enabled").exists() {
        command.env("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_V1", "1");
    }
    if gate.join("v2-wake-lost").exists() {
        command.env("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_DROP_ACK_V1", "1");
    }
    if gate.join("v2-wake-unknown").exists() {
        command.env("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_DROP_WRITE_V1", "1");
    }
    if let Ok(offset) = fs::read_to_string(gate.join("source-clock-offset")) {
        command.env(
            "OULIPOLY_KERNEL_BROKER_FIXTURE_SOURCE_CLOCK_OFFSET_SECONDS_V1",
            offset.trim(),
        );
    }
    let mut broker = command.spawn().unwrap();
    eventually(|| {
        protocol::request_at(socket, Operation::Classify).is_ok()
            || broker.try_wait().unwrap().is_some()
    });
    assert!(
        broker.try_wait().unwrap().is_none(),
        "source broker restart: {}",
        fs::read_to_string(log).unwrap()
    );
    broker
}

fn replace_source_decision_claims(journal_path: &Path, claims: &[u8]) {
    let journal = rusqlite::Connection::open(journal_path).unwrap();
    journal
        .execute_batch("DROP TRIGGER decisions_no_update")
        .unwrap();
    journal
        .execute("UPDATE decisions SET claims=?1", [claims])
        .unwrap();
    journal
        .execute_batch("CREATE TRIGGER decisions_no_update BEFORE UPDATE ON decisions BEGIN SELECT RAISE(ABORT, 'immutable source decision'); END")
        .unwrap();
}

fn assert_old_debt_and_no_f_ack(broker_state: &Path) {
    let old = rusqlite::Connection::open_with_flags(
        broker_state.join("sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let pending: i64 = old.query_row(
        "SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND handle='old-unacked' AND delivered_at IS NULL",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(pending, 1, "old v29 unsettled row changed");
    let fresh = rusqlite::Connection::open_with_flags(
        broker_state.join("v30/sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    for table in [
        "fresh_recipient_source",
        "fresh_recipient_row_source",
        "fresh_recipient_grant",
        "fresh_recipient_ack_delegation",
    ] {
        let count: i64 = fresh
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "source W minted {table}");
    }
}

fn indexed_physical_account(provider_dir: &Path, physical: &str) -> serde_json::Value {
    for entry in fs::read_dir(provider_dir.join("index-v1/accounts")).unwrap() {
        let path = entry.unwrap().path();
        if let Ok(bytes) = fs::read(path)
            && let Ok(sealed) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && sealed["data"]["physical_key"] == physical
        {
            return sealed["data"].clone();
        }
    }
    panic!("indexed physical account {physical} absent");
}

fn assert_old_pending_v29(broker_state: &Path) {
    let old = rusqlite::Connection::open_with_flags(
        broker_state.join("sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(old.query_row(
        "SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND handle='old-unacked' AND delivered_at IS NULL",
        [], |r| r.get::<_, i64>(0)).unwrap(), 1);
}

/// The owner inventory counts every deliverable row in the retained sidecar.
/// Identify the exact remaining row after both native ACKs instead of
/// attributing a global count to either native recipient.
fn read_owner_mailbox_diagnostic(
    broker_state: &Path,
    historical_sidecar: &Path,
    state_path: &Path,
    source_generation: &str,
    original_row_seq: i64,
) -> serde_json::Value {
    let side = rusqlite::Connection::open_with_flags(
        broker_state.join("sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let pending = side
        .prepare(
            "SELECT seq FROM mailbox WHERE delivered_at IS NULL AND
             (delivery_error IS NULL OR delivery_error NOT IN
              ('wake_sweep_abandoned','mailbox_payload_verification_failed',
               'mailbox_ingress_expired','completion_effect_uncertain')) ORDER BY seq",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(pending.len(), 1, "owner inventory row identity changed");
    let seq = pending[0];
    assert_ne!(
        seq, original_row_seq,
        "native ACK left original row pending"
    );
    let retained =
        BrokerSidecar::open_existing(&broker_state.join("sidecar/pid-identity.db"), broker_state)
            .unwrap();
    let row = retained
        .read_exact_mailbox_row(source_generation, "old-pending", seq)
        .unwrap()
        .unwrap()
        .row;
    assert_eq!(
        (row.kind.as_str(), row.handle.as_str()),
        ("fixture", "old-unacked")
    );
    assert_eq!(row.payload_json, "{}");
    assert_eq!(row.delivery_attempts, 0);
    assert!(row.delivered_at.is_none());
    assert!(row.owner_invocation_uuid.is_none());
    assert!(row.target_kind.is_none() && row.target_id.is_none());
    assert!(row.payload_file_path.is_none());
    assert!(row.payload_sha256.is_none() && row.payload_byte_len.is_none());
    assert!(
        retained
            .read_exact_mailbox_row(source_generation, "wrong-recipient", seq)
            .unwrap()
            .is_none()
    );
    assert!(
        retained
            .read_exact_mailbox_row("changed-generation", "old-pending", seq)
            .is_err()
    );
    let (listeners, projected_sources, recipient_grants): (i64, i64, i64) = side
        .query_row(
            "SELECT
             (SELECT COUNT(*) FROM completion_event_listener WHERE mailbox_seq=?1),
             (SELECT COUNT(*) FROM completion_continuation_source WHERE event_id=?2),
             (SELECT COUNT(*) FROM broker_native_recipient_grant WHERE row_seq=?1)
             +(SELECT COUNT(*) FROM broker_v2_recipient_grant WHERE row_seq=?1)",
            rusqlite::params![seq, row.handle],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!((listeners, projected_sources, recipient_grants), (0, 0, 0));
    let (session_runtime, wake_claim): (i64, i64) = side
        .query_row(
            "SELECT
             (SELECT COUNT(*) FROM session_runtime WHERE session_id=?1),
             (SELECT COUNT(*) FROM session_wake_claim WHERE session_id=?1)",
            [&row.session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!((session_runtime, wake_claim), (0, 0));
    let state = rusqlite::Connection::open_with_flags(
        state_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let state_sources: i64 = state
        .query_row(
            "SELECT COUNT(*) FROM invocation_completion_v2_identity WHERE handle=?1",
            [&row.handle],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state_sources, 0);
    let historical = rusqlite::Connection::open_with_flags(
        historical_sidecar,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let historical_rows: i64 = historical
        .query_row(
            "SELECT COUNT(*) FROM mailbox WHERE session_id=?1 AND handle=?2",
            rusqlite::params![row.session_id, row.handle],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(historical_rows, 0, "row came from historical copy");
    serde_json::json!({
        "seq": seq,
        "session_id": row.session_id,
        "kind": row.kind,
        "handle": row.handle,
        "phase": "pending_no_delivery_attempt",
        "delivery_attempts": row.delivery_attempts,
        "inline_payload_sha256": format!("{:x}", Sha256::digest(row.payload_json.as_bytes())),
        "retained_payload_sha256": row.payload_sha256,
        "owner_invocation_uuid": row.owner_invocation_uuid,
        "target_kind": row.target_kind,
        "target_id": row.target_id,
        "listener_count": listeners,
        "sidecar_source_count": projected_sources,
        "state_source_count": state_sources,
        "recipient_grant_count": recipient_grants,
        "session_runtime_count": session_runtime,
        "wake_claim_count": wake_claim,
        "historical_copy_row_count": historical_rows,
        "source": "private fixture INSERT after v29 copy",
    })
}

fn assert_exact_native_mailbox_acks(
    broker_state: &Path,
    gate: &Path,
    original_session: &str,
    original_seq: i64,
    fresh_session: &str,
    fresh_seq: i64,
) {
    let original_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(gate.join("native-turn-receipt.json")).unwrap()).unwrap();
    let f_receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(gate.join("native-f-turn-receipt.json")).unwrap())
            .unwrap();
    let retained = rusqlite::Connection::open_with_flags(
        broker_state.join("sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let original: (i64, String, String, String) = retained
        .query_row(
            "SELECT m.delivery_attempts,g.phase,g.turn_id,g.ack_response_sha256
             FROM mailbox m JOIN broker_native_recipient_grant g ON g.row_seq=m.seq
             JOIN completion_event_listener l ON l.mailbox_seq=m.seq
             WHERE m.session_id=?1 AND m.seq=?2 AND m.delivered_at IS NOT NULL
               AND g.acknowledged_at=m.delivered_at
               AND l.acknowledgement_reason='explicit_native_codex_assistant_ack'",
            rusqlite::params![original_session, original_seq],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(original.0, 1);
    assert_eq!(original.1, "acked");
    assert_eq!(original.2, original_receipt["turn_id"]);
    assert_eq!(original.3, original_receipt["ack_response_sha256"]);

    let fresh = rusqlite::Connection::open_with_flags(
        broker_state.join("v30/sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let f: (i64, String, String, String) = fresh
        .query_row(
            "SELECT m.delivery_attempts,g.phase,a.turn_id,a.assistant_response_sha256
             FROM mailbox m JOIN fresh_recipient_grant g
               ON g.session_id=m.session_id AND g.seq=m.seq
             JOIN fresh_headless_native_f_ack a ON a.grant_id=g.grant_id
             WHERE m.session_id=?1 AND m.seq=?2 AND m.delivered_at IS NOT NULL
               AND g.acknowledged_at=m.delivered_at
               AND a.acknowledged_at=m.delivered_at
               AND a.basis='native_codex_f_assistant_ack'",
            rusqlite::params![fresh_session, fresh_seq],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(f.0, 1);
    assert_eq!(f.1, "acked");
    assert_eq!(f.2, f_receipt["turn_id"]);
    assert_eq!(f.3, f_receipt["assistant_response_sha256"]);
    assert_ne!(original.2, f.2, "F must have its own native turn");
}

fn assert_pending_notify_without_delivery(broker_state: &Path) {
    let fresh = rusqlite::Connection::open_with_flags(
        broker_state.join("v30/sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    for (table, expected) in [
        ("fresh_recipient_source", 1),
        ("fresh_recipient_grant", 0),
        ("fresh_recipient_ack_delegation", 0),
    ] {
        let count: i64 = fresh
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, expected, "unexpected {table} count");
    }
    let pending: i64 = fresh
        .query_row(
            "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1);
    let old = rusqlite::Connection::open_with_flags(
        broker_state.join("sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let old_pending: i64 = old.query_row(
        "SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND handle='old-unacked' AND delivered_at IS NULL",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(old_pending, 1);
}

struct SnapshotRestore {
    path: std::path::PathBuf,
    bytes: Vec<u8>,
}

impl Drop for SnapshotRestore {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, &self.bytes);
    }
}

fn assert_postcommit_source_physical_after_owner(
    socket: &Path,
    broker_state: &Path,
    gate: &Path,
    bash: &str,
    completed_v2: bool,
    wake: bool,
    wake_lost: bool,
    wake_unknown: bool,
) {
    // J, guardian and driver have exited. Reopen only the Broker's
    // retained one-use physical record after a Broker restart.
    let physical_dir = broker_state.join("source-physical");
    let pending = SourcePhysicalRegistry::open(&physical_dir).unwrap();
    assert_eq!(pending.records().len(), 1);
    let grant = &pending.records()[0].grant;
    assert_eq!((grant.phase.as_str(), grant.revision), ("consumed", 2));
    let replay = protocol::StateWriteSpec {
        protocol: "broker-source-effect-launch-v30".into(),
        source_generation: grant.source_generation.clone(),
        root_id: grant.root_id.clone(),
        owner_generation: grant.owner_generation.clone(),
        action: protocol::StateWriteAction::LaunchSourceGrant,
    };
    assert!(protocol::launch_source_effect_grant_at(socket, &replay).is_err());
    assert_eq!(
        SourcePhysicalRegistry::open(&physical_dir)
            .unwrap()
            .records()
            .len(),
        1
    );
    let gone = |stamp: &oulipoly_kernel_broker::entry_registry::ProcessStamp| {
        oulipoly_kernel_broker::identity::observed_incarnation_gone(
            stamp.host_pid,
            &stamp.boot_id,
            stamp.starttime_ticks,
            (stamp.pidns_dev, stamp.pidns_ino),
        )
        .unwrap()
    };
    eventually(|| gone(&pending.records()[0].worker));
    assert!(gone(&pending.records()[0].guardian));
    assert!(gone(&pending.records()[0].driver));
    assert!(!gone(&pending.records()[0].pid1));
    assert!(matches!(
        pending
            .observe(&pending.records()[0].grant.grant_id)
            .unwrap(),
        SourceObservation::Live { .. } | SourceObservation::DrainPending { .. }
    ));
    assert!(
        !physical_dir
            .join(format!(
                "{}.terminal.json",
                pending.records()[0].grant.grant_id
            ))
            .exists()
    );
    fs::write(gate.join("source-adopted-started.release"), b"release").unwrap();
    eventually(|| {
        let physical = SourcePhysicalRegistry::open(&physical_dir).unwrap();
        physical.records().len() == 1
            && matches!(
                physical
                    .observe(&physical.records()[0].grant.grant_id)
                    .unwrap(),
                SourceObservation::Drained { .. }
            )
    });
    let physical = SourcePhysicalRegistry::open(&physical_dir).unwrap();
    let record = &physical.records()[0];
    let registration_path = fs::read_to_string(gate.join("source-ready")).unwrap();
    let registration_bytes = fs::read(registration_path.trim()).unwrap();
    let registration: serde_json::Value = serde_json::from_slice(&registration_bytes).unwrap();
    if completed_v2 {
        let source_dir = Path::new(registration_path.trim()).parent().unwrap();
        eventually(|| source_dir.join("completion-snapshot-v2.json").exists());
        let outcome_bytes = fs::read(source_dir.join("source-outcome-v2.json")).unwrap();
        let outcome: serde_json::Value = serde_json::from_slice(&outcome_bytes).unwrap();
        let snapshot: serde_json::Value = serde_json::from_slice(
            &fs::read(source_dir.join("completion-snapshot-v2.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fs::read(registration_path.trim()).unwrap(),
            registration_bytes
        );
        assert_eq!(
            outcome["registration_digest"],
            record.grant.candidate.registration_digest
        );
        assert_eq!(outcome["registration_id"], registration["registration_id"]);
        assert_eq!(outcome["observer"], registration["registering_caller"]);
        assert_eq!(outcome["kind"], "exit_tree");
        assert_eq!(outcome["original_tree_drained"], true);
        assert_eq!(outcome["output_closed"], true);
        assert_eq!(snapshot["status"], "completed");
        assert_eq!(snapshot["rc"], 0);
        assert_eq!(
            snapshot["outcome_sha256"],
            format!("{:x}", Sha256::digest(&outcome_bytes))
        );
        assert_eq!(snapshot["outcome_byte_len"], outcome_bytes.len());
        let selected = fs::read(source_dir.join("completion-output-v2.bin")).unwrap();
        assert_eq!(
            snapshot["output"]["sha256"],
            format!("{:x}", Sha256::digest(&selected))
        );
        assert_eq!(snapshot["output"]["byte_len"], selected.len());
        assert_eq!(selected, fs::read(source_dir.join("log")).unwrap());
    }
    assert_eq!(
        registration["recovery"]["sha256"],
        format!("{:x}", Sha256::digest(fs::read(bash).unwrap()))
    );
    assert_eq!(
        registration["registration_id"],
        record.grant.candidate.registration_id
    );
    assert_eq!(
        record.grant.candidate.registration_digest,
        format!("{:x}", Sha256::digest(&registration_bytes))
    );
    let id = &record.grant.grant_id;
    let observation = physical.observe(id).unwrap();
    let SourceObservation::Drained {
        worker_wait_status,
        stdout,
        stderr,
        cancel_requested: false,
    } = observation
    else {
        panic!("source Q not physically drained: {observation:?}");
    };
    assert_eq!(worker_wait_status, 0);
    let output = fs::read(physical_dir.join(format!("{id}.stdout"))).unwrap();
    let errors = fs::read(physical_dir.join(format!("{id}.stderr"))).unwrap();
    assert_eq!(stdout.byte_len, output.len() as u64);
    assert_eq!(stdout.sha256, format!("{:x}", Sha256::digest(&output)));
    assert_eq!(stderr.byte_len, errors.len() as u64);
    assert_eq!(stderr.sha256, format!("{:x}", Sha256::digest(&errors)));
    assert!(
        errors
            .windows(b"adopted-stderr\n".len())
            .any(|w| w == b"adopted-stderr\n")
    );
    let terminal: serde_json::Value = serde_json::from_slice(
        &physical
            .read_witness(id, "terminal.json", 16 * 1024)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(terminal["grant_id"], *id);
    assert_eq!(terminal["adopted_tree_drained"], true);
    assert_eq!(terminal["worker_local_pid"], record.worker_local_pid);
    // The restarted serving Broker must read its own retained W
    // record and persist an assessment, even though the caller that
    // launched W and the original owner are gone.
    eventually(|| {
        let _ = protocol::request_at(&socket, Operation::Classify);
        let sidecar = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            &broker_state,
        )
        .unwrap();
        sidecar
            .read_source_evidence(&record.grant)
            .unwrap()
            .is_some_and(|row| row.phase == if completed_v2 { "accepted" } else { "unknown" })
            && (!completed_v2
                || sidecar
                    .read_source_retention_release(&record.grant)
                    .unwrap()
                    .is_some())
    });
    let mut retained =
        BrokerSidecar::open_existing(&broker_state.join("sidecar/pid-identity.db"), &broker_state)
            .unwrap();
    let evidence = retained
        .read_source_evidence(&record.grant)
        .unwrap()
        .unwrap();
    assert!(
        matches!(evidence.phase.as_str(), "unknown" | "accepted"),
        "physical Q and fresh State provenance must agree: {evidence:?}"
    );
    let reply_status = serde_json::from_slice::<serde_json::Value>(&output)
        .ok()
        .and_then(|reply| reply["status"].as_str().map(str::to_owned));
    eprintln!(
        "age319 real H-W-Q: grant={} worker_wait={} stdout={} stderr={} reply_status={:?} evidence_phase={}",
        id, worker_wait_status, stdout.byte_len, stderr.byte_len, reply_status, evidence.phase
    );
    if completed_v2 {
        assert_eq!(reply_status.as_deref(), Some("source_ready"));
        assert_eq!(evidence.phase, "accepted");
        oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(&mut retained, &physical, id)
            .unwrap(); // exact retry after a lost acceptance response
        let accepted = oulipoly_kernel_broker::source_acceptance::accept_v2_completion_source(
            &mut retained,
            &physical,
            id,
        )
        .unwrap();
        let release =
            oulipoly_kernel_broker::source_acceptance::decide_v2_source_retention_release(
                &mut retained,
                &physical,
                id,
            )
            .unwrap();
        assert_eq!(release.protocol, "broker-source-retention-release-v1");
        assert_eq!(release.registration_id, accepted.registration_id);
        assert_eq!(accepted.evidence_seal, evidence.seal.clone().unwrap());
        assert_eq!(
            accepted.selected_output["sha256"],
            format!(
                "{:x}",
                Sha256::digest(
                    fs::read(
                        Path::new(registration_path.trim())
                            .parent()
                            .unwrap()
                            .join("completion-output-v2.bin")
                    )
                    .unwrap()
                )
            )
        );
        assert!(accepted.owned_artifact.is_some());
        assert_eq!(
            retained
                .read_source_retention_release(&record.grant)
                .unwrap()
                .unwrap()
                .0,
            release
        );
        let (acceptance_json, acceptance_sha, release_link): (String, String, String) =
            rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap()
            .query_row(
                "SELECT a.receipt_json,a.receipt_sha256,r.acceptance_sha256
                 FROM broker_completion_source_acceptance a
                 JOIN broker_source_retention_release r USING(registration_id)
                 WHERE a.registration_id=?1 AND a.grant_id=?2 AND a.source_generation=?3",
                rusqlite::params![
                    accepted.registration_id,
                    accepted.grant_id,
                    accepted.source_generation
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            acceptance_sha,
            format!("{:x}", Sha256::digest(acceptance_json.as_bytes()))
        );
        assert_eq!(release.acceptance_sha256, acceptance_sha);
        assert_eq!(release_link, acceptance_sha);
        let exact_manifest = physical
            .read_witness(id, "evidence.json", 32 * 1024 * 1024)
            .unwrap();
        let publication = oulipoly_kernel_broker::source_acceptance::trigger_v2_completion_source(
            &mut retained,
            &physical,
            id,
        )
        .unwrap();
        let listener = &registration["listeners"][0];
        assert_eq!(publication.event.state, "triggered");
        assert_eq!(publication.listeners.len(), 1);
        assert_eq!(
            publication.listeners[0].listener_id,
            listener["listener_id"]
        );
        assert_eq!(publication.listeners[0].session_id, listener["session_id"]);
        assert_eq!(publication.mailbox_rows.len(), 1);
        assert_eq!(
            publication.mailbox_rows[0].session_id,
            listener["session_id"]
        );
        assert_eq!(
            publication.mailbox_rows[0].owner_invocation_uuid.as_deref(),
            listener["owner_invocation_uuid"].as_str()
        );
        assert!(publication.mailbox_rows[0].delivered_at.is_none());
        assert!(publication.listeners[0].acknowledged_at.is_none());
        let payload: serde_json::Value = serde_json::from_slice(
            &fs::read(publication.event.payload_file_path.as_ref().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(payload["schema_version"], 2);
        assert_eq!(
            payload["completion_protocol"],
            oulipoly_state::completion_continuation::PROTOCOL
        );
        assert_eq!(payload["registration_id"], accepted.registration_id);
        assert_eq!(payload["registration_digest"], accepted.registration_digest);
        assert_eq!(payload["snapshot"]["output"], accepted.selected_output);
        let output_path = Path::new(payload["output_artifact"]["path"].as_str().unwrap());
        assert_eq!(
            fs::read(output_path).unwrap(),
            fs::read(
                Path::new(registration_path.trim())
                    .parent()
                    .unwrap()
                    .join("completion-output-v2.bin")
            )
            .unwrap()
        );
        let duplicate = oulipoly_kernel_broker::source_acceptance::trigger_v2_completion_source(
            &mut BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                broker_state,
            )
            .unwrap(),
            &physical,
            id,
        )
        .unwrap();
        assert!(!duplicate.triggered);
        assert_eq!(duplicate.mailbox_rows, publication.mailbox_rows);
        let saved_output = fs::read(output_path).unwrap();
        fs::write(output_path, b"changed retained Broker output").unwrap();
        assert!(
            oulipoly_kernel_broker::source_acceptance::trigger_v2_completion_source(
                &mut retained,
                &physical,
                id,
            )
            .is_err(),
            "changed retained Broker bytes passed trigger retry"
        );
        fs::write(output_path, saved_output).unwrap();
        let event_payload_path = Path::new(publication.event.payload_file_path.as_deref().unwrap());
        let saved_payload = fs::read(event_payload_path).unwrap();
        fs::write(event_payload_path, b"changed retained completion payload").unwrap();
        assert!(
            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                &mut retained,
                &physical,
                id,
            )
            .is_err(),
            "changed retained payload passed recipient readback"
        );
        fs::write(event_payload_path, saved_payload).unwrap();
        let evidence_artifact = physical_dir.join(format!("{id}.evidence-artifact"));
        let saved_evidence_artifact = fs::read(&evidence_artifact).unwrap();
        fs::write(&evidence_artifact, b"changed sealed Broker artifact").unwrap();
        assert!(
            oulipoly_kernel_broker::source_acceptance::trigger_v2_completion_source(
                &mut retained,
                &physical,
                id,
            )
            .is_err(),
            "changed sealed Broker artifact passed trigger retry"
        );
        fs::write(&evidence_artifact, saved_evidence_artifact).unwrap();
        let sidecar_path = broker_state.join("sidecar/pid-identity.db");
        let tamper = rusqlite::Connection::open(&sidecar_path).unwrap();
        assert!(
            tamper
                .execute(
                    "UPDATE completion_event_listener SET session_id='changed-session'
                 WHERE event_id=?1 AND listener_id=?2",
                    rusqlite::params![
                        registration["handle"].as_str().unwrap(),
                        listener["listener_id"].as_str().unwrap()
                    ],
                )
                .is_err(),
            "original listener/session identity must be immutable"
        );
        drop(tamper);
        let mut changed_seal = accepted.clone();
        changed_seal.evidence_seal.manifest_sha256 = "0".repeat(64);
        assert!(
            retained
                .accept_captured_completion_source(&record.grant, &changed_seal, &exact_manifest)
                .is_err()
        );
        let mut changed_selection = accepted.clone();
        changed_selection.selected_output["sha256"] = serde_json::json!("0".repeat(64));
        assert!(
            retained
                .accept_captured_completion_source(
                    &record.grant,
                    &changed_selection,
                    &exact_manifest
                )
                .is_err()
        );
        assert!(
            retained
                .decide_source_retention_release(&record.grant, &changed_selection)
                .is_err()
        );
        assert!(
            !Path::new(registration_path.trim())
                .parent()
                .unwrap()
                .join("source-retention-release-v1.json")
                .exists(),
            "Broker release decision is not Bash receipt delivery"
        );
        for changed in [
            {
                let mut g = record.grant.clone();
                g.owner_generation = "wrong-owner".into();
                g
            },
            {
                let mut g = record.grant.clone();
                g.root_id = "wrong-root".into();
                g
            },
            {
                let mut g = record.grant.clone();
                g.source_generation = "wrong-generation".into();
                g
            },
            {
                let mut g = record.grant.clone();
                g.candidate.registration_id = "wrong-registration".into();
                g
            },
        ] {
            assert!(
                retained
                    .decide_source_retention_release(&changed, &accepted)
                    .is_err()
            );
            assert!(
                retained
                    .trigger_accepted_completion_source(&changed, &accepted)
                    .is_err(),
                "wrong source/root/owner/generation passed Broker trigger"
            );
        }
        let state_source: serde_json::Value = serde_json::from_slice(
            &fs::read(broker_state.join("sidecar/state-source.json")).unwrap(),
        )
        .unwrap();
        let original_state = rusqlite::Connection::open_with_flags(
            state_source["path"].as_str().unwrap(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let original_count: i64 = original_state.query_row(
            "SELECT count(*) FROM invocation_completion_exact_source_decisions WHERE registration_id=?1",
            [&record.grant.candidate.registration_id], |r| r.get(0),
        ).unwrap();
        assert_eq!(original_count, 1);
        let original_row: (String, String, String, i64, i64) = original_state.query_row(
            "SELECT registration_sha256,root_id,owner_generation,original_state_device,original_state_inode
             FROM invocation_completion_exact_source_decisions WHERE registration_id=?1",
            [&record.grant.candidate.registration_id],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
        ).unwrap();
        let provenance: (String, String, String, String, String, String, i64, i64) =
            rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap()
            .query_row(
                "SELECT registration_digest,caller_admission_id,source_generation,root_id,
                 owner_generation,decision_id,state_device,state_inode
                 FROM broker_exact_fresh_source_admission WHERE registration_id=?1",
                [&record.grant.candidate.registration_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(provenance.0, record.grant.candidate.registration_digest);
        assert_eq!(
            provenance.1,
            retained
                .read_consumed_source_candidate(&record.grant)
                .unwrap()
                .caller_admission_id()
        );
        assert_eq!(provenance.2, record.grant.source_generation);
        assert_eq!(provenance.3, record.grant.root_id);
        assert_eq!(provenance.4, record.grant.owner_generation);
        assert!(!provenance.5.is_empty());
        assert_eq!(provenance.6, state_source["device"].as_i64().unwrap());
        assert_eq!(provenance.7, state_source["inode"].as_i64().unwrap());
        assert_eq!(
            original_row,
            (
                provenance.0.clone(),
                provenance.3.clone(),
                provenance.4.clone(),
                provenance.6,
                provenance.7,
            )
        );
        let sidecar_counts: (i64, i64, i64, i64) = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT (SELECT count(*) FROM broker_exact_source_projection),
                    (SELECT count(*) FROM broker_exact_fresh_source_admission),
                    (SELECT count(*) FROM broker_fresh_source_admission),
                    (SELECT count(*) FROM broker_source_evidence WHERE phase='accepted')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
        assert_eq!(sidecar_counts, (1, 1, 0, 1));
    } else {
        assert_eq!(reply_status.as_deref(), Some("pending"));
        assert_eq!(evidence.phase, "unknown");
        assert!(
            retained
                .read_source_retention_release(&record.grant)
                .unwrap()
                .is_none()
        );
        assert!(
            oulipoly_kernel_broker::source_acceptance::accept_v2_completion_source(
                &mut retained,
                &physical,
                id,
            )
            .is_err()
        );
    }
    let source_phase: String = rusqlite::Connection::open_with_flags(
        broker_state.join("sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap()
    .query_row(
        "SELECT phase FROM completion_continuation_source WHERE registration_id=?1",
        [&record.grant.candidate.registration_id],
        |row| row.get(0),
    )
    .unwrap();
    assert_eq!(
        source_phase,
        if completed_v2 {
            "accepted"
        } else {
            "registered"
        }
    );
    let publication_counts: (i64, i64, i64) = rusqlite::Connection::open_with_flags(
        broker_state.join("sidecar/pid-identity.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ).unwrap().query_row(
        "SELECT (SELECT count(*) FROM mailbox WHERE handle=?1),
                (SELECT count(*) FROM completion_event WHERE event_id=?1 AND state='triggered'),
                (SELECT count(*) FROM completion_event_listener WHERE event_id=?1 AND acknowledged_at IS NOT NULL)",
        [registration["handle"].as_str().unwrap()],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).unwrap();
    assert_eq!(
        publication_counts,
        if completed_v2 { (1, 1, 0) } else { (0, 0, 0) },
        "source trigger and notification custody differ from accepted source"
    );
    let recipient_custody = if completed_v2 {
        let custody = oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
            &mut retained,
            &physical,
            id,
        )
        .unwrap();
        assert_eq!(custody.source_grant_id, *id);
        assert_eq!(custody.source_id, registration["source_id"]);
        assert_eq!(custody.registration_id, registration["registration_id"]);
        assert_eq!(custody.source_generation, record.grant.source_generation);
        assert_eq!(custody.root_id, record.grant.root_id);
        assert_eq!(custody.owner_generation, record.grant.owner_generation);
        assert_eq!(
            custody.listener_id,
            registration["listeners"][0]["listener_id"]
        );
        assert_eq!(custody.session_id, registration["owner_session_id"]);
        assert_eq!(
            custody.owner_invocation_uuid,
            registration["owner_invocation_uuid"]
        );
        assert!(custody.row_seq > 0);
        let bytes = retained
            .mailbox()
            .completion_recovery_payload(registration["handle"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            custody.payload_sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        assert_eq!(custody.payload_byte_len, bytes.len() as i64);
        let reopened = oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
            &mut BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                broker_state,
            )
            .unwrap(),
            &physical,
            id,
        )
        .unwrap();
        assert_eq!(
            reopened, custody,
            "restart readback changed recipient provenance"
        );
        let counts: (i64, i64, i64) = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT (SELECT count(*) FROM mailbox WHERE seq=?1 AND delivered_at IS NULL),
                    (SELECT count(*) FROM completion_event_listener WHERE event_id=?2
                     AND mailbox_seq=?1 AND acknowledged_at IS NULL),
                    (SELECT count(*) FROM sqlite_master
                     WHERE type='table' AND name='fresh_recipient_grant')",
            rusqlite::params![custody.row_seq, registration["handle"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
        assert_eq!(counts, (1, 1, 0));
        assert!(
            retained
                .read_exact_mailbox_row(
                    &custody.source_generation,
                    "wrong-recipient-session",
                    custody.row_seq,
                )
                .unwrap()
                .is_none(),
            "a different session saw the original recipient row"
        );
        let copied_db = gate.join("recipient-copy.db");
        fs::copy(broker_state.join("sidecar/pid-identity.db"), &copied_db).unwrap();
        assert!(
            BrokerSidecar::open_existing(&copied_db, broker_state).is_err(),
            "copied Broker DB reopened as retained authority"
        );
        Some(custody)
    } else {
        assert!(
            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                &mut retained,
                &physical,
                id,
            )
            .is_err(),
            "pending source acquired recipient provenance"
        );
        None
    };
    let source_dir = Path::new(registration_path.trim()).parent().unwrap();
    let broker_receipt = source_dir.join("broker-source-retention-release-v1.json");
    let local_release = source_dir.join("source-retention-release-v1.json");
    if completed_v2 {
        eventually(|| broker_receipt.exists());
        assert!(
            !local_release.exists(),
            "Broker publication is not a Bash local release"
        );
        let mut reopened = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            broker_state,
        )
        .unwrap();
        oulipoly_kernel_broker::source_acceptance::deliver_v2_source_retention_release(
            &mut reopened,
            &physical,
            id,
        )
        .unwrap();
        assert_eq!(
            bash_retention_check(bash, registration_path.trim()),
            "released"
        );
        assert!(local_release.exists());
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(&broker_receipt).unwrap()).unwrap();
        let local: serde_json::Value =
            serde_json::from_slice(&fs::read(&local_release).unwrap()).unwrap();
        assert_eq!(
            local["broker_receipt_sha256"],
            format!("{:x}", Sha256::digest(fs::read(&broker_receipt).unwrap()))
        );
        assert_eq!(local["acceptance_sha256"], receipt["acceptance_sha256"]);
        assert_eq!(local["release_sha256"], receipt["release_sha256"]);
        let original_receipt = fs::read(&broker_receipt).unwrap();
        fs::write(&broker_receipt, b"changed root receipt").unwrap();
        assert_ne!(
            bash_retention_check(bash, registration_path.trim()),
            "released"
        );
        fs::write(&broker_receipt, &original_receipt).unwrap();
        for (field, changed) in [
            ("source_generation", serde_json::json!("wrong-generation")),
            ("owner_generation", serde_json::json!("wrong-owner")),
            ("root_id", serde_json::json!("wrong-root")),
            (
                "stable_output",
                serde_json::json!({"sha256":"0".repeat(64),"byte_len":0}),
            ),
            ("payload_sha256", serde_json::json!("0".repeat(64))),
        ] {
            let mut altered = receipt.clone();
            altered[field] = changed;
            fs::write(&broker_receipt, serde_json::to_vec(&altered).unwrap()).unwrap();
            assert_ne!(
                bash_retention_check(bash, registration_path.trim()),
                "released",
                "changed {field} receipt passed Bash"
            );
            fs::write(&broker_receipt, &original_receipt).unwrap();
        }
        let swapped = source_dir.join("broker-source-retention-release-v1.swapped");
        fs::rename(&broker_receipt, &swapped).unwrap();
        fs::copy(&swapped, &broker_receipt).unwrap();
        let status = Command::new("chown")
            .arg("1:1")
            .arg(&broker_receipt)
            .status()
            .unwrap();
        assert!(status.success(), "mapped nonroot receipt copy required");
        assert_ne!(
            bash_retention_check(bash, registration_path.trim()),
            "released"
        );
        fs::remove_file(&broker_receipt).unwrap();
        fs::rename(&swapped, &broker_receipt).unwrap();
        assert_eq!(
            bash_retention_check(bash, registration_path.trim()),
            "released"
        );
        let owner_change = Command::new("chown")
            .arg("1:1")
            .arg(registration_path.trim())
            .status()
            .unwrap();
        assert!(owner_change.success());
        assert_ne!(
            bash_retention_check(bash, registration_path.trim()),
            "released",
            "changed original source owner passed Bash"
        );
        assert!(
            Command::new("chown")
                .arg("0:0")
                .arg(registration_path.trim())
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            bash_retention_check(bash, registration_path.trim()),
            "released"
        );
    } else {
        assert!(!broker_receipt.exists());
        assert!(!local_release.exists());
        assert_eq!(
            bash_retention_check(bash, registration_path.trim()),
            "retained"
        );
        assert!(!local_release.exists());
    }
    if completed_v2 {
        let custody = recipient_custody.clone().unwrap();
        assert_eq!(
            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                &mut retained,
                &physical,
                id,
            )
            .unwrap(),
            custody,
            "exact readback minted or changed a recipient row"
        );
        let accepted = || {
            assess_v2_candidate(&retained, &physical, id)
                .expect("exact completed Bash source must remain assessable")
        };
        let candidate = accepted();
        assert_eq!(
            candidate.snapshot_sha256,
            evidence.seal.as_ref().unwrap().snapshot_sha256
        );
        assert_eq!(
            candidate.outcome_sha256,
            evidence.seal.as_ref().unwrap().outcome_sha256
        );
        read_captured_v2_evidence(&retained, &physical, id).unwrap();
        let reject_change = |name: &str, changed: Vec<u8>| {
            let path = source_dir.join(name);
            let original = fs::read(&path).unwrap();
            fs::write(&path, changed).unwrap();
            assert_ne!(
                bash_retention_check(bash, registration_path.trim()),
                "released",
                "{name} mutation retained Bash release"
            );
            assert!(
                assess_v2_candidate(&retained, &physical, id).is_err(),
                "{name} mutation was accepted"
            );
            assert!(
                read_captured_v2_evidence(&retained, &physical, id).is_err(),
                "{name} mutation kept captured custody"
            );
            let mut retry = BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                broker_state,
            )
            .unwrap();
            assert!(
                oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
                    &mut retry, &physical, id,
                )
                .is_err(),
                "{name} mutation passed accepted retry"
            );
            assert!(
                oulipoly_kernel_broker::source_acceptance::accept_v2_completion_source(
                    &mut retry, &physical, id,
                )
                .is_err(),
                "{name} mutation passed source acceptance retry"
            );
            assert!(
                oulipoly_kernel_broker::source_acceptance::decide_v2_source_retention_release(
                    &mut retry, &physical, id,
                )
                .is_err(),
                "{name} mutation passed release retry"
            );
            assert!(
                oulipoly_kernel_broker::source_acceptance::trigger_v2_completion_source(
                    &mut retry, &physical, id,
                )
                .is_err(),
                "{name} mutation passed trigger retry"
            );
            assert!(
                oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                    &mut retry, &physical, id,
                )
                .is_err(),
                "{name} mutation passed recipient custody retry"
            );
            fs::write(&path, original).unwrap();
            accepted();
        };
        let mut wrong_registration = registration.clone();
        wrong_registration["helper"]["sha256"] = serde_json::json!("0".repeat(64));
        reject_change(
            "source-registration-v2.json",
            serde_json::to_vec(&wrong_registration).unwrap(),
        );
        let mut wrong_worker = registration.clone();
        wrong_worker["registering_caller"]["pid"] = serde_json::json!(0);
        reject_change(
            "source-registration-v2.json",
            serde_json::to_vec(&wrong_worker).unwrap(),
        );
        let outcome_path = source_dir.join("source-outcome-v2.json");
        let missing_path = source_dir.join("source-outcome-v2.missing-test");
        fs::rename(&outcome_path, &missing_path).unwrap();
        assert!(
            assess_v2_candidate(&retained, &physical, id).is_err(),
            "missing outcome was accepted"
        );
        assert!(
            oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
                &mut BrokerSidecar::open_existing(
                    &broker_state.join("sidecar/pid-identity.db"),
                    broker_state
                )
                .unwrap(),
                &physical,
                id,
            )
            .is_err(),
            "missing outcome passed accepted retry"
        );
        fs::rename(&missing_path, &outcome_path).unwrap();
        let mut wrong_outcome: serde_json::Value =
            serde_json::from_slice(&fs::read(&outcome_path).unwrap()).unwrap();
        wrong_outcome["observer"]["pid"] = serde_json::json!(0);
        reject_change(
            "source-outcome-v2.json",
            serde_json::to_vec(&wrong_outcome).unwrap(),
        );
        let mut wrong_snapshot: serde_json::Value = serde_json::from_slice(
            &fs::read(source_dir.join("completion-snapshot-v2.json")).unwrap(),
        )
        .unwrap();
        wrong_snapshot["status"] = serde_json::json!("ready");
        reject_change(
            "completion-snapshot-v2.json",
            serde_json::to_vec(&wrong_snapshot).unwrap(),
        );
        reject_change(
            "completion-output-v2.bin",
            b"changed selected output".to_vec(),
        );
        let original_registration = source_dir.join("source-registration-v2.json");
        let saved_registration = source_dir.join("source-registration-v2.copy-test-original");
        fs::rename(&original_registration, &saved_registration).unwrap();
        fs::copy(&saved_registration, &original_registration).unwrap();
        assert_ne!(
            bash_retention_check(bash, registration_path.trim()),
            "released",
            "copied registration inode passed Bash"
        );
        assert!(
            read_captured_v2_evidence(&retained, &physical, id).is_err(),
            "copied registration inode kept captured custody"
        );
        assert!(
            oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
                &mut BrokerSidecar::open_existing(
                    &broker_state.join("sidecar/pid-identity.db"),
                    broker_state
                )
                .unwrap(),
                &physical,
                id,
            )
            .is_err(),
            "copied registration passed accepted retry"
        );
        fs::remove_file(&original_registration).unwrap();
        fs::rename(&saved_registration, &original_registration).unwrap();
        assert_eq!(accepted(), candidate);
        read_captured_v2_evidence(&retained, &physical, id).unwrap();
        let manifest_path = physical_dir.join(format!("{id}.evidence.json"));
        let manifest = fs::read(&manifest_path).unwrap();
        fs::write(&manifest_path, b"changed manifest").unwrap();
        assert!(
            oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
                &mut BrokerSidecar::open_existing(
                    &broker_state.join("sidecar/pid-identity.db"),
                    broker_state
                )
                .unwrap(),
                &physical,
                id,
            )
            .is_err(),
            "changed broker manifest passed accepted retry"
        );
        fs::write(&manifest_path, manifest).unwrap();
        oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
            &mut BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                broker_state,
            )
            .unwrap(),
            &physical,
            id,
        )
        .unwrap();
    }
    // Missing, partial and contradictory retained witnesses cannot
    // authorize Q after the original actors are gone.
    let terminal_path = physical_dir.join(format!("{id}.terminal.json"));
    let original = fs::read(&terminal_path).unwrap();
    fs::remove_file(&terminal_path).unwrap();
    assert!(matches!(
        SourcePhysicalRegistry::open(&physical_dir)
            .unwrap()
            .observe(id)
            .unwrap(),
        SourceObservation::Unknown {
            reason: "missing-terminal-receipt",
            ..
        }
    ));
    if completed_v2 {
        assert!(
            oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
                &mut BrokerSidecar::open_existing(
                    &broker_state.join("sidecar/pid-identity.db"),
                    broker_state
                )
                .unwrap(),
                &SourcePhysicalRegistry::open(&physical_dir).unwrap(),
                id,
            )
            .is_err(),
            "missing physical terminal passed accepted retry"
        );
    }
    fs::write(&terminal_path, &original).unwrap();
    let stderr_path = physical_dir.join(format!("{id}.stderr"));
    fs::write(&stderr_path, &errors[..errors.len() - 1]).unwrap();
    assert!(matches!(
        SourcePhysicalRegistry::open(&physical_dir)
            .unwrap()
            .observe(id)
            .unwrap(),
        SourceObservation::Unknown {
            reason: "incomplete-or-changed-output",
            ..
        }
    ));
    fs::write(&stderr_path, &errors).unwrap();
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["worker_local_pid"] = serde_json::json!(record.worker_local_pid + 1);
    fs::write(&terminal_path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(matches!(
        SourcePhysicalRegistry::open(&physical_dir)
            .unwrap()
            .observe(id)
            .unwrap(),
        SourceObservation::Unknown {
            reason: "conflicting-terminal-receipt",
            ..
        }
    ));
    fs::write(&terminal_path, &original).unwrap();
    assert!(matches!(
        SourcePhysicalRegistry::open(&physical_dir)
            .unwrap()
            .observe(id)
            .unwrap(),
        SourceObservation::Drained { .. }
    ));
    if wake {
        let original_payload = retained
            .mailbox()
            .completion_recovery_payload(registration["handle"].as_str().unwrap())
            .unwrap()
            .unwrap();
        let custody = recipient_custody.unwrap();
        let binding = oulipoly_state::mailbox::BrokerV2RecipientBinding {
            source_grant_id: custody.source_grant_id.clone(),
            source_id: custody.source_id.clone(),
            registration_id: custody.registration_id.clone(),
            source_generation: custody.source_generation.clone(),
            root_id: custody.root_id.clone(),
            owner_generation: custody.owner_generation.clone(),
            listener_id: custody.listener_id.clone(),
            session_id: custody.session_id.clone(),
            owner_invocation_uuid: custody.owner_invocation_uuid.clone(),
            row_seq: custody.row_seq,
            payload_sha256: custody.payload_sha256.clone(),
            payload_byte_len: custody.payload_byte_len,
        };
        let root_stamp = serde_json::to_string(&record.root_init).unwrap();
        let historical_child_stamp = serde_json::to_string(&record.joined_child).unwrap();
        for mutated in [
            oulipoly_state::mailbox::BrokerV2RecipientBinding {
                root_id: uuid::Uuid::new_v4().to_string(),
                ..binding.clone()
            },
            oulipoly_state::mailbox::BrokerV2RecipientBinding {
                session_id: uuid::Uuid::new_v4().to_string(),
                ..binding.clone()
            },
            oulipoly_state::mailbox::BrokerV2RecipientBinding {
                source_generation: uuid::Uuid::new_v4().to_string(),
                ..binding.clone()
            },
        ] {
            assert!(
                retained
                    .reserve_v2_recipient_grant(&mutated, &root_stamp, &historical_child_stamp)
                    .is_err(),
                "wrong source, root or session reserved a grant"
            );
        }
        assert!(retained.read_v2_recipient_grant(id).unwrap().is_none());
        fs::write(gate.join("v2-wake-activate"), b"activate").unwrap();
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let read = retained.read_v2_recipient_grant(id);
            if read.as_ref().is_ok_and(|g| {
                g.as_ref().is_some_and(|g| {
                    g.phase
                        == if wake_unknown {
                            "unknown"
                        } else if wake_lost {
                            "submitted"
                        } else {
                            "acked"
                        }
                })
            }) {
                break;
            }
            assert!(
                Instant::now() < until,
                "v2 wake did not settle: grant={read:?} broker={}",
                fs::read_to_string(
                    broker_state
                        .parent()
                        .unwrap()
                        .join("real-h-w-q-post-owner-restart.log")
                )
                .unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let grant = retained.read_v2_recipient_grant(id).unwrap().unwrap();
        assert_eq!(grant.binding, binding);
        assert_eq!(
            grant.phase,
            if wake_unknown {
                "unknown"
            } else if wake_lost {
                "submitted"
            } else {
                "acked"
            }
        );
        assert_eq!(
            grant.ack_response_sha256.is_some(),
            !wake_lost && !wake_unknown
        );
        assert_eq!(
            retained.v2_recipient_payload(&grant).unwrap(),
            original_payload
        );
        let actor: oulipoly_kernel_broker::entry_registry::ProcessStamp =
            serde_json::from_str(&grant.recipient_identity).unwrap();
        let root: oulipoly_kernel_broker::entry_registry::ProcessStamp =
            serde_json::from_str(&grant.root_init_identity).unwrap();
        assert_eq!(root, record.root_init);
        assert_eq!(
            (actor.pidns_dev, actor.pidns_ino),
            (root.pidns_dev, root.pidns_ino)
        );
        assert_ne!(actor.host_pid, record.joined_child.host_pid);
        assert!(retained.begin_v2_recipient_send(&grant).is_err());
        assert!(
            retained
                .mark_v2_recipient_submitted(id, "wrong-process")
                .is_err()
        );
        assert!(
            retained
                .acknowledge_v2_recipient(
                    id,
                    "wrong-process",
                    &grant.delivery_token,
                    &"0".repeat(64)
                )
                .is_err()
        );
        if wake_lost || wake_unknown {
            let mut stale: oulipoly_kernel_broker::entry_registry::ProcessStamp =
                serde_json::from_str(&grant.recipient_identity).unwrap();
            stale.starttime_ticks += 1;
            assert!(
                retained
                    .acknowledge_v2_recipient(
                        id,
                        &serde_json::to_string(&stale).unwrap(),
                        &grant.delivery_token,
                        &"0".repeat(64)
                    )
                    .is_err(),
                "stale incarnation ACKed a submitted grant"
            );
        }
        if !wake_lost && !wake_unknown {
            assert!(
                retained
                    .acknowledge_v2_recipient(
                        id,
                        &grant.recipient_identity,
                        &grant.delivery_token,
                        &"0".repeat(64)
                    )
                    .is_err(),
                "duplicate ACK changed exact settlement"
            );
        }
        let counts: (i64, i64, i64) = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap()
        .query_row(
            "SELECT (SELECT count(*) FROM broker_v2_recipient_grant),
                    (SELECT count(*) FROM mailbox WHERE seq=?1 AND delivered_at IS NOT NULL),
                    (SELECT count(*) FROM completion_event_listener
                     WHERE mailbox_seq=?1 AND acknowledged_at IS NOT NULL
                       AND acknowledgement_reason='explicit_v2_wake_ack')",
            [custody.row_seq],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
        assert_eq!(
            counts,
            if wake_lost || wake_unknown {
                (1, 0, 0)
            } else {
                (1, 1, 1)
            }
        );
        assert_eq!(
            retained.read_v2_recipient_grant(id).unwrap().unwrap(),
            grant
        );
    }
}

fn terminate_postcommit_original_owner(broker_state: &Path) {
    let physical = SourcePhysicalRegistry::open(broker_state.join("source-physical")).unwrap();
    assert_eq!(physical.records().len(), 1);
    let record = &physical.records()[0];
    for stamp in [&record.driver, &record.guardian] {
        if oulipoly_kernel_broker::identity::observed_incarnation_gone(
            stamp.host_pid,
            &stamp.boot_id,
            stamp.starttime_ticks,
            (stamp.pidns_dev, stamp.pidns_ino),
        )
        .unwrap()
        {
            continue;
        }
        let process =
            oulipoly_kernel_broker::identity::PinnedProcess::open(stamp.host_pid).unwrap();
        assert_eq!(
            oulipoly_kernel_broker::entry_registry::ProcessStamp::from(&process),
            *stamp
        );
        process.signal(libc::SIGKILL).unwrap();
    }
}

fn inner() {
    let mode = std::env::var("AGE319_PRIVATE_JOIN_MODE").unwrap_or_else(|_| "help".into());
    let old_pending_debt = mode == "normal_model_provider_native_codex_f_turn_old_pending_debt";
    let native_f_turn = mode == "normal_model_provider_native_codex_f_turn" || old_pending_debt;
    let closed_refusal_probe =
        native_f_turn && std::env::var_os("AGE319_PRIVATE_SECOND_ENTRY_REFUSAL_V1").is_some();
    let native_f_candidate =
        mode == "normal_model_provider_native_codex_f_candidate" || native_f_turn;
    let native_ack_mode =
        mode == "normal_model_provider_native_codex_recipient_ack" || native_f_candidate;
    let native_turn_mode =
        native_ack_mode || mode == "normal_model_provider_native_codex_turn_receipt";
    let native_codex_mode =
        native_turn_mode || mode == "normal_model_provider_native_codex_root_h_notify_row";
    let root_terminal_drain = matches!(
        mode.as_str(),
        "normal_model_provider_bash_causal_root_h_delegate_drain"
            | "normal_model_provider_bash_causal_root_h_notify_row_drain"
            | "normal_model_provider_bash_causal_root_h_notify_wake_drain"
    );
    let caller_mode = mode.starts_with("normal_model_provider_caller_")
        || mode == "normal_model_provider_bash_ordinary_sync_parent_output"
        || root_terminal_drain;
    let physical_mode = mode.starts_with("normal_model_provider_pty_physical");
    let resident_mode = mode.starts_with("normal_model_provider_pty_physical_resident_");
    let resident_bash = mode.starts_with("normal_model_provider_pty_physical_resident_bash_");
    let physical_f = mode.contains("_f_fenced_physical");
    let native_crash = mode
        .strip_prefix("normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_");
    let native_lost_reply = mode
        .strip_prefix("normal_model_provider_pty_physical_resident_bash_f_fenced_physical_lost_");
    let resident_notify = resident_bash
        && (mode.ends_with("_notify")
            || mode.contains("_f_fenced")
            || mode.ends_with("_restart")
            || mode.contains("_after_append"));
    let resident_failure = mode
        .strip_prefix("normal_model_provider_pty_physical_resident_")
        .filter(|case| *case != "tail" && !case.starts_with("bash_"));
    let physical_success = physical_mode
        && !matches!(
            mode.as_str(),
            "normal_model_provider_pty_physical_post_k_unknown"
                | "normal_model_provider_pty_physical_root_exit"
                | "normal_model_provider_pty_physical_restart_after_k"
        );
    let path_mode = matches!(
        mode.as_str(),
        "normal_model_provider_path" | "normal_model_provider_prefix"
    );
    let v3_quota = mode.starts_with("normal_model_provider_v3_quota");
    let manual_route = mode.starts_with("normal_model_provider_v3_quota_route_manual");
    let shared_mode = mode.starts_with("normal_model_provider_v3_quota_route_physical_shared");
    let shared_manual = mode == "normal_model_provider_v3_quota_route_physical_shared_manual";
    let manual_setup = manual_route || shared_manual;
    let manual_physical = mode.starts_with("normal_model_provider_v3_quota_route_manual_physical");
    let terminal_v3 = matches!(
        mode.as_str(),
        "normal_model_provider_v3_quota_route_manual_physical_terminal"
            | "normal_model_provider_v3_quota_route_manual_physical_capacity_terminal"
            | "normal_model_provider_v3_quota_route_manual_physical_account_quota_terminal"
    );
    let typed_terminal_v3 = mode.ends_with("physical_capacity_terminal")
        || mode.ends_with("physical_account_quota_terminal");
    let manual_denied = matches!(
        mode.as_str(),
        "normal_model_provider_v3_quota_route_manual_invalid"
            | "normal_model_provider_v3_quota_route_manual_full"
            | "normal_model_provider_v3_quota_route_manual_failed"
            | "normal_model_provider_v3_quota_route_manual_pending"
            | "normal_model_provider_v3_quota_route_manual_unknown"
    );
    // Keep the K switch enabled for the invalid-quota route refusal too.
    let v3_physical = mode.starts_with("normal_model_provider_v3_quota_route_physical")
        || manual_physical
        || manual_denied
        || mode == "normal_model_provider_v3_quota_route_invalid";
    let provider_option = if mode.ends_with("physical_capacity_terminal") {
        Some("--capacity-clean")
    } else if mode.ends_with("physical_account_quota_terminal") {
        Some("--quota-clean")
    } else if terminal_v3 || shared_mode {
        Some("--clean")
    } else if mode == "normal_model_provider_auth_after_healthy" {
        Some("--auth")
    } else if mode.ends_with("physical_nonzero") {
        Some("--fail")
    } else if mode.ends_with("physical_capacity") {
        Some("--capacity")
    } else if mode.ends_with("physical_account_quota") {
        Some("--quota")
    } else {
        None
    };
    let v3_mode = mode == "normal_model_provider_v3_closed" || v3_quota;
    let provider_mode = mode.starts_with("normal_model_provider");
    let provider_negative = (v3_quota && !v3_physical)
        || manual_denied
        || mode == "normal_model_provider_v3_quota_route_invalid"
        || matches!(
            mode.as_str(),
            "normal_model_provider_bad_config"
                | "normal_model_provider_unsupported"
                | "normal_model_provider_v3_closed"
                | "normal_model_provider_v3_quota"
                | "normal_model_provider_v3_quota_reply_loss"
                | "normal_model_provider_v3_quota_post_k"
                | "normal_model_provider_v3_quota_restart"
                | "normal_model_provider_v3_quota_invalid"
                | "normal_model_provider_v3_quota_full"
                | "normal_model_provider_v3_quota_stale"
                | "normal_model_provider_quota"
                | "normal_model_provider_auth"
        );
    let physical_normal =
        mode == "normal_model_held" && std::env::var_os("AGE319_TEST_NORMAL_PHYSICAL_V1").is_some();
    let featureless_normal =
        physical_normal && std::env::var_os("AGE319_TEST_FEATURELESS_NORMAL_V1").is_some();
    let physical_normal_bad_source =
        physical_normal && std::env::var_os("AGE319_TEST_NORMAL_BAD_SOURCE_V1").is_some();
    let model_mode = mode == "normal_model_held" || provider_mode;
    let native_mode = mode.starts_with("native_");
    let native_live = matches!(
        mode.as_str(),
        "native_cancel" | "native_drain" | "native_receipt_cancel" | "native_receipt_drain"
    );
    let release_mode = mode.starts_with("held_release") || native_mode;
    let wake_lost = mode == "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_lost";
    let wake_unknown = mode == "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_unknown";
    let wake_v2_source =
        mode == "normal_handoff_pre_k_h_source_postcommit_w_v2_wake" || wake_lost || wake_unknown;
    let completed_v2_source =
        mode == "normal_handoff_pre_k_h_source_postcommit_w_v2" || wake_v2_source;
    let postcommit_source =
        mode == "normal_handoff_pre_k_h_source_postcommit_w" || completed_v2_source;
    let production_source =
        mode == "normal_handoff_pre_k_h_source_registration_cli" || postcommit_source;
    let bad_disposition = mode == "normal_empty_bad_disposition";
    let source_witness = (mode == "held_release_source_witness").then(|| {
        (
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
            oulipoly_state::CompletionRegistrationAuthority::generate().unwrap(),
        )
    });
    let normal_mode = mode.starts_with("normal_");
    let handoff_mode = v3_mode
        || mode.starts_with("normal_model_provider")
        || matches!(
            mode.as_str(),
            "normal_handoff"
                | "normal_handoff_bash_child"
                | "normal_handoff_pre_k_h_source"
                | "normal_handoff_pre_k_h_source_registration_cli"
                | "normal_handoff_pre_k_h_source_postcommit_w"
                | "normal_handoff_pre_k_h_source_postcommit_w_v2"
                | "normal_handoff_pre_k_h_source_postcommit_w_v2_wake"
                | "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_lost"
                | "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_unknown"
                | "normal_handoff_fsync"
                | "normal_handoff_effect_reply_loss"
                | "normal_help"
                | "normal_model_held"
        );
    let recipient_mode = mode.starts_with("normal_recipient");
    let real_source = mode.starts_with("normal_bash_source");
    let nonzero_source = mode == "normal_bash_source_nonzero";
    let io_failure_source = mode == "normal_bash_source_capture_io_failure";
    let runner =
        std::env::var("OULIPOLY_AGE319_RUNNER_IMAGE").expect("built Runner image required");
    let bash = (mode == "normal_handoff_bash_child"
        || mode == "normal_handoff_pre_k_h_source"
        || production_source
        || mode.starts_with("normal_model_provider_bash_causal")
        || mode.starts_with("normal_model_provider_bash_ordinary")
        || native_codex_mode
        || mode.starts_with("normal_model_provider_pty_physical_resident_bash_"))
    .then(|| std::env::var("OULIPOLY_AGE319_BASH_IMAGE").expect("built Bash image required"));
    let bash_request = uuid::Uuid::new_v4().to_string();
    let provider_image = if native_codex_mode {
        std::env::var("AGE319_TEST_CODEX_ELF").expect("real Codex ELF required")
    } else {
        std::env::var("OULIPOLY_AGE319_PROVIDER_IMAGE").unwrap_or_default()
    };
    let temp = tempfile::tempdir().unwrap();
    let normal_effect = temp.path().join("normal-effect");
    let data = temp.path().join("data");
    // Keep the source for the historical broker copy outside the current
    // Runner's writable entry domain. v29 remains an independent island.
    let historical_data = temp.path().join("historical-data");
    fs::create_dir(&historical_data).unwrap();
    let historical_sidecar = historical_data.join("pid-identity.db");
    let broker_state = temp.path().join("broker-state");
    let physical_dir = broker_state.join("v30/fresh-provider");
    let gate = temp.path().join("gate");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&broker_state).unwrap();
    fs::set_permissions(&broker_state, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&gate).unwrap();
    fs::set_permissions(&gate, fs::Permissions::from_mode(0o700)).unwrap();
    if native_codex_mode {
        fs::create_dir(gate.join("native-codex-home")).unwrap();
        if native_turn_mode {
            let auth = std::env::var("AGE319_TEST_CODEX_AUTH")
                .expect("private native turn test requires an auth.json source");
            fs::copy(auth, gate.join("native-codex-home/auth.json")).unwrap();
        }
    }
    if root_terminal_drain {
        fs::write(gate.join("terminal-drain-mode"), b"held").unwrap();
    }
    if completed_v2_source {
        fs::write(gate.join("source-v2-workload"), b"selected").unwrap();
        if wake_v2_source {
            fs::write(gate.join("v2-wake-enabled"), b"enabled").unwrap();
            if wake_lost {
                fs::write(gate.join("v2-wake-lost"), b"lost").unwrap();
            }
            if wake_unknown {
                fs::write(gate.join("v2-wake-unknown"), b"unknown").unwrap();
            }
        }
    }
    let root_h_delegate = matches!(
        mode.as_str(),
        "normal_model_provider_bash_causal_root_h_delegate"
            | "normal_model_provider_bash_causal_root_h_delegate_drain"
            | "normal_model_provider_bash_causal_root_h_lost"
            | "normal_model_provider_bash_causal_root_h_notify_row"
            | "normal_model_provider_bash_causal_root_h_notify_row_drain"
            | "normal_model_provider_bash_causal_root_h_notify_wake"
            | "normal_model_provider_bash_causal_root_h_notify_wake_drain"
            | "normal_model_provider_bash_causal_root_h_notify_wake_lost"
            | "normal_model_provider_bash_causal_root_h_notify_wake_unknown"
            | "normal_model_provider_native_codex_root_h_notify_row"
            | "normal_model_provider_native_codex_turn_receipt"
            | "normal_model_provider_native_codex_recipient_ack"
            | "normal_model_provider_native_codex_f_candidate"
            | "normal_model_provider_native_codex_f_turn"
            | "normal_model_provider_native_codex_f_turn_old_pending_debt"
    );
    let root_h_lost = mode == "normal_model_provider_bash_causal_root_h_lost";
    let root_h_notify = root_h_delegate && (mode.contains("_notify_") || native_turn_mode);
    let root_h_wake = mode.contains("root_h_notify_wake");
    let root_h_wake_lost = mode.ends_with("root_h_notify_wake_lost");
    let root_h_wake_unknown = mode.ends_with("root_h_notify_wake_unknown");
    if root_h_delegate {
        fs::write(gate.join("selected-k-root-h"), b"intent").unwrap();
        if root_h_notify {
            fs::write(gate.join("source-production-registration-cli"), b"yes").unwrap();
            fs::write(gate.join("source-production-postcommit-h"), b"schedule").unwrap();
        }
        if root_h_wake {
            fs::write(gate.join("v2-wake-enabled"), b"enabled").unwrap();
            if root_h_wake_lost {
                fs::write(gate.join("v2-wake-lost"), b"lost").unwrap();
            }
            if root_h_wake_unknown {
                fs::write(gate.join("v2-wake-unknown"), b"unknown").unwrap();
            }
        }
        if root_h_lost {
            fs::write(gate.join("selected-k-root-h-lost"), b"lost").unwrap();
        }
    }
    if let Some((invocation, session, capability)) = &source_witness {
        fs::write(
            gate.join("source-witness-request"),
            serde_json::to_vec(&serde_json::json!({
                "invocation": invocation,
                "session": session,
                "capability": capability.process_environment_value(),
            }))
            .unwrap(),
        )
        .unwrap();
    }
    let native_store = gate.join("provider-native-session.json");
    let native_adapter = gate.join("resident-native-adapter.py");
    if resident_mode {
        fs::write(
            &native_adapter,
            include_str!("fixtures/age319-resident-native-adapter.py"),
        )
        .unwrap();
        fs::set_permissions(&native_adapter, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!native_store.exists());
    }
    let config_home = temp.path().join("config-home");
    if provider_mode {
        let config_dir = config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(config_dir.join("models")).unwrap();
        let selected_image = if mode.ends_with("physical_account_quota")
            || mode.ends_with("physical_account_quota_terminal")
        {
            let path = gate.join("opencode-fixture");
            fs::copy(&provider_image, &path).unwrap();
            path.to_string_lossy().into_owned()
        } else {
            provider_image.clone()
        };
        let (provider_command, provider_environment) = if path_mode {
            let bin = temp.path().join("provider-bin");
            fs::create_dir(&bin).unwrap();
            let image = bin.join("age319-provider");
            fs::copy(&provider_image, &image).unwrap();
            fs::set_permissions(&image, fs::Permissions::from_mode(0o755)).unwrap();
            let command = if mode == "normal_model_provider_prefix" {
                "env -u AGE319_PREFIX_REMOVED age319-provider"
            } else {
                "age319-provider"
            };
            (
                serde_json::to_string(command).unwrap(),
                format!(
                    "environment = {{ PATH = {} }}\n",
                    serde_json::to_string(&format!("{}:/usr/bin:/bin", bin.display())).unwrap()
                ),
            )
        } else {
            (
                serde_json::to_string(&selected_image).unwrap(),
                String::new(),
            )
        };
        let marker = serde_json::to_string(gate.join("provider-effect").to_str().unwrap()).unwrap();
        let unused_marker =
            serde_json::to_string(gate.join("provider-effect-unused").to_str().unwrap()).unwrap();
        let provider_name = if mode == "normal_model_provider_bad_config" {
            "wrong-account"
        } else {
            "local"
        };
        let prompt_mode = if mode == "normal_model_provider_unsupported" {
            "prompt_mode = \"arg\"\n"
        } else {
            ""
        };
        let quota = if v3_quota
            || matches!(
                mode.as_str(),
                "normal_model_provider_quota_available"
                    | "normal_model_provider_quota_reply_loss"
                    | "normal_model_provider_quota_restart"
            ) {
            let quota_bytes: &[u8] = match mode.as_str() {
                "normal_model_provider_v3_quota_route_manual_invalid"
                | "normal_model_provider_v3_quota_invalid"
                | "normal_model_provider_v3_quota_route_invalid"
                | "normal_model_provider_v3_quota_route_auth_failed" => b"invalid quota",
                "normal_model_provider_v3_quota_route_manual_full"
                | "normal_model_provider_v3_quota_full"
                | "normal_model_provider_v3_quota_route_full" => {
                    br#"{"used_percent":100,"resets_at":"2099-01-01T00:00:00Z"}"#
                }
                "normal_model_provider_v3_quota_stale"
                | "normal_model_provider_v3_quota_route_stale" => {
                    br#"{"used_percent":20,"resets_at":"2020-01-01T00:00:00Z"}"#
                }
                _ => br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            };
            fs::write(gate.join("quota.json"), quota_bytes).unwrap();
            if mode.starts_with("normal_model_provider_v3_quota_auth")
                || mode == "normal_model_provider_v3_quota_route_auth_failed"
            {
                if matches!(
                    mode.as_str(),
                    "normal_model_provider_v3_quota_auth_restart"
                        | "normal_model_provider_v3_quota_auth_shared"
                ) {
                    format!(
                        "quota_script = 'if test -e {0}/auth-ok; then cat {0}/quota.json; else printf invalid; fi'\nauth_refresh_command = 'printf x > {0}/started-auth; while ! test -e {0}/finish-auth; do sleep 0.05; done; printf x >> {0}/auth-ok'\n",
                        gate.display()
                    )
                } else if mode == "normal_model_provider_v3_quota_auth_failed"
                    || mode == "normal_model_provider_v3_quota_route_auth_failed"
                {
                    format!("quota_script = 'printf invalid'\nauth_refresh_command = 'exit 9'\n")
                } else {
                    format!(
                        "quota_script = 'if test -e {0}/auth-ok; then cat {0}/quota.json; else printf invalid; fi'\nauth_refresh_command = 'printf x >> {0}/auth-ok'\n",
                        gate.display()
                    )
                }
            } else if mode == "normal_model_provider_v3_quota_route_manual_failed" {
                "quota_script = 'exit 7'\n".into()
            } else if mode == "normal_model_provider_v3_quota_route_manual_pending" {
                format!(
                    "quota_script = 'while ! test -e {0}/finish-manual-quota; do sleep 0.05; done; cat {0}/quota.json'\n",
                    gate.display()
                )
            } else if mode == "normal_model_provider_v3_quota_restart"
                || mode.ends_with("shared_pending")
            {
                format!(
                    "quota_script = 'printf x > {0}/started-quota; while ! test -e {0}/finish-quota; do sleep 0.05; done; cat {0}/quota.json'\n",
                    gate.display()
                )
            } else {
                format!(
                    "quota_script = 'cat {}'\n",
                    gate.join("quota.json").display()
                )
            }
        } else if mode == "normal_model_provider_quota" {
            "quota_script = \"quota-must-not-run\"\n".into()
        } else if mode == "normal_model_provider_auth_after_healthy" {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            format!(
                "quota_script = 'cat {0}/quota.json'\nauth_refresh_command = 'printf x >> {0}/auth-ok'\n",
                gate.display()
            )
        } else if mode == "normal_model_provider_auth_recovery" {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            format!(
                "quota_script = 'test -f {} && cat {}'\nauth_refresh_command = 'printf x >> {}'\n",
                gate.join("auth-refreshed").display(),
                gate.join("quota.json").display(),
                gate.join("auth-refreshed").display(),
            )
        } else if mode == "normal_model_provider_auth" {
            "quota_script = \"quota-must-not-run\"\nauth_refresh_command = \"auth-must-not-run\"\n"
                .into()
        } else if matches!(
            mode.as_str(),
            "normal_model_provider_auth_success"
                | "normal_model_provider_auth_reply_loss"
                | "normal_model_provider_auth_restart"
        ) {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":20,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            format!(
                "quota_script = 'if test -e {0}/auth-ok; then cat {0}/quota.json; else exit 7; fi'\nauth_refresh_command = 'printf x >> {0}/auth-ok'\n",
                gate.display()
            )
        } else {
            String::new()
        };
        let local_args = if v3_physical && provider_option.is_some() {
            format!("{marker}, \"{}\"", provider_option.unwrap())
        } else if mode == "normal_model_provider_auth_after_healthy" {
            format!("{marker}, \"--auth\"")
        } else if mode == "normal_model_provider_caller_binary" {
            format!("{marker}, \"--binary\"")
        } else if mode == "normal_model_provider_caller_nonzero" {
            format!("{marker}, \"--fail-clean\"")
        } else if caller_mode
            && !root_terminal_drain
            && mode != "normal_model_provider_bash_ordinary_sync_parent_output"
        {
            format!("{marker}, \"--clean\"")
        } else {
            marker.clone()
        };
        let selected_env = if native_codex_mode {
            format!(
                "environment = {{ CODEX_HOME = {}, OULIPOLY_DATA_DIR = {}, PATH = '/usr/bin:/bin' }}\n",
                serde_json::to_string(gate.join("native-codex-home").to_str().unwrap()).unwrap(),
                serde_json::to_string(data.to_str().unwrap()).unwrap()
            )
        } else if v3_physical {
            "environment = { AGE319_SELECTED_ACCOUNT = 'physical-local' }\n".into()
        } else {
            String::new()
        };
        let authority = if manual_setup {
            format!(
                "settings_id = 'fixture'\nimplementation = {{ family = 'private-test', executable = {provider_command} }}\n"
            )
        } else {
            String::new()
        };
        let interactive_args = if resident_bash {
            let mut args = vec![
                "--interactive-only".to_string(),
                gate.join("interactive-effect").display().to_string(),
                bash.as_ref().unwrap().to_string(),
                temp.path().join("v30.sock").display().to_string(),
                bash_request.clone(),
                gate.join("bash-effect").display().to_string(),
                data.display().to_string(),
            ];
            if resident_notify {
                args.push("notify".into());
            }
            format!(
                "interactive_args = {}\n",
                serde_json::to_string(&args).unwrap()
            )
        } else if mode.starts_with("normal_model_provider_pty_") {
            format!(
                "interactive_args = [\"--interactive-only\", {}]\n",
                serde_json::to_string(gate.join("interactive-effect").to_str().unwrap()).unwrap()
            )
        } else {
            String::new()
        };
        let native_implementation = if resident_mode {
            format!(
                "settings_id = \"age319-resident-settings\"\nimplementation = {{ family = \"age319-resident\", executable = {} }}\n",
                serde_json::to_string(native_adapter.to_str().unwrap()).unwrap(),
            )
        } else {
            String::new()
        };
        let second_provider = if mode
            .starts_with("normal_model_provider_v3_quota_route_physical_shared")
            || mode == "normal_model_provider_v3_quota_auth_shared"
        {
            let second_marker =
                serde_json::to_string(gate.join("provider-effect-second").to_str().unwrap())
                    .unwrap();
            let second_quota = if mode.ends_with("shared_command_changed") {
                format!(
                    "quota_script = 'cat {}; true'\n",
                    gate.join("quota.json").display()
                )
            } else {
                quota.clone()
            };
            let second_identity = if mode.ends_with("shared_account_changed") {
                "physical-other"
            } else {
                "physical-local"
            };
            format!(
                "[local-two]\ncommand = {provider_command}\nargs = [{second_marker}]\nquota_account_id = '{second_identity}'\n{authority}{selected_env}{second_quota}"
            )
        } else {
            String::new()
        };
        fs::write(
            config_dir.join("providers.toml"),
            format!(
                "[unused]\ncommand = {provider_command}\nargs = [{unused_marker}]\nquota_account_id = 'physical-unused'\n{interactive_args}{provider_environment}{authority}{native_implementation}{unused_native_env}[{provider_name}]\ncommand = {provider_command}\nargs = [{local_args}]\nquota_account_id = 'physical-local'\n{interactive_args}{provider_environment}{authority}{selected_env}{prompt_mode}{quota}{native_implementation}{second_provider}",
                unused_native_env = if native_codex_mode { selected_env.as_str() } else { "" },
            ),
        )
        .unwrap();
        fs::write(
            config_dir.join("models/configured-model.toml"),
            "[[providers]]\nname = \"unused\"\n[[providers]]\nname = \"local\"\n",
        )
        .unwrap();
        if mode == "normal_model_provider_v3_quota_route_manual_cross_model"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_cross_model"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_capacity"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_account_quota"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_capacity_terminal"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_account_quota_terminal"
        {
            fs::write(
                config_dir.join("models/alias.toml"),
                "[[providers]]\nname = \"local\"\n",
            )
            .unwrap();
        }
        if mode.starts_with("normal_model_provider_v3_quota_route_physical_shared")
            || mode == "normal_model_provider_v3_quota_auth_shared"
        {
            fs::write(
                config_dir.join("models/configured-model-two.toml"),
                "[[providers]]\nname = \"unused\"\n[[providers]]\nname = \"local-two\"\n",
            )
            .unwrap();
        }
    }
    if mode == "normal_model_held" {
        let config_dir = config_home.join("oulipoly-agent-runner");
        fs::create_dir_all(config_dir.join("models")).unwrap();
        let physical_script = temp.path().join("normal-provider.sh");
        if physical_normal {
            fs::write(
                &physical_script,
                if std::env::var_os("AGE319_TEST_FEATURELESS_BINARY_V1").is_some() {
                    b"#!/bin/sh\nprintf 'one\\n' >> \"$AGE319_EFFECT_FILE\"\nprintf '\\377\\000normal-provider-out:'\ncat\nprintf '\\000\\377normal-provider-err\\n' >&2\n".as_slice()
                } else if std::env::var_os("AGE319_TEST_FEATURELESS_NONZERO_V1").is_some() {
                    b"#!/bin/sh\nprintf 'one\\n' >> \"$AGE319_EFFECT_FILE\"\nprintf 'normal-provider-out:'\ncat\nprintf 'normal-provider-err\\n' >&2\nexit 17\n".as_slice()
                } else {
                    b"#!/bin/sh\nprintf 'one\\n' >> \"$AGE319_EFFECT_FILE\"\nprintf 'normal-provider-out:'\ncat\nprintf 'normal-provider-err\\n' >&2\n".as_slice()
                },
            ).unwrap();
            fs::set_permissions(&physical_script, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            config_dir.join("providers.toml"),
            if physical_normal {
                format!(
                    "[local]\ncommand = '{}'\nquota_account_id = 'physical-local'\n",
                    physical_script.display()
                )
            } else {
                "[local]\ncommand = 'echo'\nquota_account_id = 'physical-local'\n".into()
            },
        )
        .unwrap();
        fs::write(
            config_dir.join("models/fixture-model.toml"),
            "[[providers]]\nname = 'local'\n",
        )
        .unwrap();
    }
    let mailbox =
        MailboxDb::open_completion_continuation_domain(&data.join("pid-identity.db")).unwrap();
    let pending_binding = (normal_mode
        && !recipient_mode
        && mode != "normal_empty"
        && !bad_disposition
        && !handoff_mode)
        .then(|| {
            if real_source {
                let registration =
                    fs::read(std::env::var("AGE319_PRIVATE_BASH_REGISTRATION").unwrap()).unwrap();
                let source: oulipoly_state::completion_continuation::SourceRegistration =
                    serde_json::from_slice(&registration).unwrap();
                let state_path = std::env::var("AGE319_PRIVATE_BASH_STATE_DB").unwrap();
                let committed = rusqlite::Connection::open_with_flags(
                    &state_path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let bytes: Vec<u8> = committed
                    .query_row(
                        "SELECT completion_v2_binding FROM invocation_completion_obligations
                     WHERE event_id=?1 AND completion_v2_binding IS NOT NULL",
                        [&source.handle],
                        |row| row.get(0),
                    )
                    .unwrap();
                let binding: AdmittedSourceBinding = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(binding.registration_bytes(), registration);
                assert_eq!(binding.registration().unwrap(), source);
                assert!(
                    !binding.caller_admission_id().is_empty(),
                    "Bash source must have a genuine committed State admission"
                );
                binding
            } else {
                let fixture: serde_json::Value = serde_json::from_str(include_str!(
                    "../../oulipoly-state/tests/fixtures/age360-paired-wire.json"
                ))
                .unwrap();
                AdmittedSourceBinding::new(
                    "fixture-admission",
                    fixture["registration_bytes_utf8"]
                        .as_str()
                        .unwrap()
                        .as_bytes(),
                )
                .unwrap()
            }
        });
    let domain = pending_binding
        .as_ref()
        .map(|binding| binding.registration().unwrap().domain_id)
        .unwrap_or_else(|| mailbox.completion_continuation_domain().unwrap().unwrap());
    let sidecar_generation = mailbox.sidecar_generation().unwrap();
    drop(mailbox);
    rusqlite::Connection::open(data.join("pid-identity.db"))
        .unwrap()
        .execute("VACUUM INTO ?1", [historical_sidecar.to_str().unwrap()])
        .unwrap();
    let recipient_mail = recipient_mode.then(|| {
        let mut old_mailbox = MailboxDb::open(&historical_sidecar).unwrap();
        match old_mailbox
            .enqueue_agent_bash_complete(&AgentBashCompleteEnqueue {
                session_id: "fixture-recipient",
                handle: "fixture-completion",
                payload_json: r#"{"protocol":"source-retention-release-v1","body":"pending"}"#,
                owner_invocation_uuid: Some("fixture-owner"),
                matched_os_pid: None,
                matched_os_boot_id: None,
                matched_os_pid_starttime_ticks: None,
                matched_chain_index: None,
                state_dir: "fixture-state",
                meta_path: "fixture-meta",
                log_path: "fixture-log",
                rc_path: "fixture-rc",
                rc: 0,
            })
            .unwrap()
        {
            EnqueueResult::Inserted(row) => row,
            other => panic!("recipient fixture enqueue: {other:?}"),
        }
    });
    let old = rusqlite::Connection::open(&historical_sidecar).unwrap();
    old.execute_batch(
        "DROP TRIGGER completion_uncertain_input_preserve;
         DROP TABLE completion_uncertain_input;
         DROP INDEX idx_mailbox_deliverable_session_live;
         DROP INDEX idx_mailbox_deliverable_target_live;
         DROP INDEX idx_mailbox_deliverable_global;
         PRAGMA user_version=29;",
    )
    .unwrap();
    old.execute_batch(include_str!(
        "../../oulipoly-state/src/mailbox/migrations/0022_live_history_barrier.sql"
    ))
    .unwrap();
    // VACUUM INTO creates a rollback-journal copy. Historical cutover source
    // validation requires the v29 island to retain the original WAL mode.
    old.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    drop(old);
    if native_mode {
        rusqlite::Connection::open(&historical_sidecar)
            .unwrap()
            .execute_batch(
                "INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('native-session','native-claim','2026-09-24T00:00:00Z','private-lineage',1);
                 INSERT INTO session_wake_claim(session_id,claim_token,claimed_at,reason,auto_wake_count) VALUES('native-sibling-session','native-sibling-claim','2026-09-24T00:00:00Z','private-lineage',1);",
            )
            .unwrap();
    }
    if normal_mode {
        rusqlite::Connection::open(&historical_sidecar)
            .unwrap()
            .execute(
                "UPDATE completion_continuation_domain SET domain_id=?1",
                [&domain],
            )
            .unwrap();
    }
    if !recipient_mode {
        let mut state = oulipoly_state::StateDb::open(&data.join("state.db")).unwrap();
        if let Some((invocation, session, capability)) = &source_witness {
            let started = state
                .start_invocation_with_prepared_completion_registration_authority(
                    &oulipoly_state::InvocationStart {
                        invocation_uuid: invocation.clone(),
                        model_name: "private-source-witness".into(),
                        provider_name: "fixture".into(),
                        provider_index: 0,
                        parent_invocation_id: None,
                    },
                    capability,
                )
                .unwrap();
            state
                .bind_invocation_provider_session_start(
                    oulipoly_state::InvocationMutationAuthority::Standalone,
                    started.invocation_row_id,
                    &oulipoly_state::ProviderSessionBinding {
                        provider_session_id: session.clone(),
                        capture_method: "private-fixture",
                        resume_input_id: None,
                        provider_session_resolved_account: None,
                    },
                )
                .unwrap();
        }
        if let Some(binding) = &pending_binding {
            state
                .seed_private_pending_completion_source(binding, &sidecar_generation)
                .unwrap();
        }
    }
    let broker_generation = if mode == "broker_state"
        || mode == "held_prepared"
        || mode == "held_guardian_death"
        || release_mode
        || normal_mode
    {
        let sidecar_dir = broker_state.join("sidecar");
        fs::create_dir(&sidecar_dir).unwrap();
        fs::set_permissions(&sidecar_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let proof = oulipoly_state::mailbox::QuiescedCutoverProof::private_fixture();
        let generation = if recipient_mode {
            fs::remove_dir(&sidecar_dir).unwrap();
            let source = historical_sidecar.clone();
            let stage =
                BrokerSidecar::stage_private_fixture_copy(&source, &broker_state, &proof).unwrap();
            let generation =
                BrokerSidecar::publish_private_fixture_copy(&source, &stage, &broker_state, &proof)
                    .unwrap();
            drop(oulipoly_state::StateDb::open(&data.join("state.db")).unwrap());
            BrokerSidecar::bind_private_fixture_state_source(
                &sidecar_dir.join("pid-identity.db"),
                &data.join("state.db"),
            )
            .unwrap();
            generation
        } else {
            let target = sidecar_dir.join("pid-identity.db");
            rusqlite::Connection::open(&historical_sidecar)
                .unwrap()
                .execute("VACUUM INTO ?1", [target.to_str().unwrap()])
                .unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
            BrokerSidecar::activate_private_fixture_copy(&target, &broker_state, &proof).unwrap()
        };
        if (normal_mode && !recipient_mode) || source_witness.is_some() {
            let target = sidecar_dir.join("pid-identity.db");
            BrokerSidecar::bind_private_fixture_state_source(&target, &data.join("state.db"))
                .unwrap();
        }
        Some(generation)
    } else {
        None
    };
    // The real F-turn starts with the historical copy as it is. The separate
    // debt mode adds a synthetic, undeliverable old row for the negative case.
    if handoff_mode && mode != "normal_model_provider_native_codex_f_turn" {
        let sidecar =
            rusqlite::Connection::open(broker_state.join("sidecar/pid-identity.db")).unwrap();
        sidecar
            .execute(
                "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
                 state_dir,meta_path,log_path,rc_path,rc)
                 VALUES('old-pending','fixture','old-unacked','{}','2026-09-24T00:00:00Z',
                 '/fixture','/fixture/meta','/fixture/log','/fixture/rc',0)",
                [],
            )
            .unwrap();
    }
    let socket = temp.path().join("broker.sock");
    if handoff_mode {
        FreshV30Lane::initialize_at(&broker_state).unwrap();
    }
    if v3_mode {
        let mut bootstrap = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        eventually(|| {
            broker_state
                .join("v30/fresh-provider/index-v1/admission-protocol.json")
                .exists()
                || bootstrap.try_wait().unwrap().is_some()
        });
        assert!(bootstrap.try_wait().unwrap().is_none());
        stop(&mut bootstrap);
        let rebuilt = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .arg("--offline-rebuild-fresh-index-v3")
            .arg(config_home.join("oulipoly-agent-runner"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .output()
            .unwrap();
        assert!(
            rebuilt.status.success(),
            "{}",
            String::from_utf8_lossy(&rebuilt.stderr)
        );
    }
    let broker_log = temp.path().join("broker.log");
    let mut broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .envs(
            bash.as_ref()
                .map(|path| ("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1", path)),
        )
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
        .envs(
            (featureless_normal
                && std::env::var_os("AGE319_TEST_NORMAL_CALLER_PARTIAL_V1").is_some())
            .then_some(("AGE319_TEST_NORMAL_CALLER_PARTIAL_V1", "1")),
        )
        .envs(
            (featureless_normal && std::env::var_os("AGE319_TEST_NORMAL_CALLER_LOST_V1").is_some())
                .then_some(("AGE319_TEST_NORMAL_CALLER_LOST_V1", "1")),
        )
        .envs(native_codex_mode.then(|| {
            (
                "OULIPOLY_KERNEL_BROKER_PRIVATE_NATIVE_CODEX_SHA256_V1",
                format!("{:x}", Sha256::digest(fs::read(&provider_image).unwrap())),
            )
        }))
        .envs(native_codex_mode.then_some(("AGE319_PRIVATE_NATIVE_BASH_DROP_REPLY_V1", "1")))
        .envs(native_turn_mode.then_some(("AGE319_PRIVATE_NATIVE_TURN_DROP_REPLY_V1", "1")))
        .envs(native_ack_mode.then_some(("AGE319_PRIVATE_NATIVE_RECIPIENT_ACK_V1", "1")))
        .envs(native_ack_mode.then_some(("AGE319_PRIVATE_NATIVE_ACK_DROP_REPLY_V1", "1")))
        .envs(native_f_turn.then_some(("AGE319_PRIVATE_NATIVE_F_TURN_DROP_REPLY_V1", "1")))
        .envs(native_f_turn.then_some(("AGE319_PRIVATE_NATIVE_F_ACK_DROP_REPLY_V1", "1")))
        .envs(native_lost_reply.map(|stage| ("AGE319_PRIVATE_NATIVE_F_DROP_REPLY_V1", stage)))
        .envs(
            (mode == "normal_model_provider_bash_causal_success" || root_h_delegate)
                .then_some(("AGE319_PRIVATE_BASH_SOURCE_SUCCESS_V1", "1")),
        )
        .envs(root_h_delegate.then_some(("AGE319_PRIVATE_SELECTED_K_ROOT_H_V1", "1")))
        .envs(root_h_notify.then_some(("AGE319_PRIVATE_BASH_ORIGINAL_NOTIFY_V1", "1")))
        .envs(root_h_wake.then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_V1", "1")))
        .envs(
            root_h_wake_lost.then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_DROP_ACK_V1", "1")),
        )
        .envs(
            root_h_wake_unknown
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_DROP_WRITE_V1", "1")),
        )
        .envs(root_h_lost.then_some(("AGE319_PRIVATE_DROP_ROOT_H_REPLY_V1", "1")))
        .envs(
            (mode == "normal_model_provider_bash_causal_w_debt"
                || mode == "normal_model_provider_bash_causal_notify_w_debt"
                || mode == "normal_model_provider_bash_ordinary_sync_w_debt")
                .then_some(("AGE319_PRIVATE_SOURCE_W_CAPTURE_ONLY_V1", "1")),
        )
        .envs(
            (mode == "normal_model_provider_bash_ordinary_sync_socket_partial")
                .then_some(("AGE319_PRIVATE_SYNC_PARTIAL_SOCKET_REPLY_V1", "1")),
        )
        .envs(
            (mode == "normal_model_provider_bash_causal_notify_debt")
                .then_some(("AGE319_PRIVATE_NOTIFY_STATE_ONLY_V1", "1")),
        )
        .envs(
            (mode == "normal_model_provider_bash_causal_notify_row_debt")
                .then_some(("AGE319_PRIVATE_NOTIFY_ROW_ONLY_V1", "1")),
        )
        .envs(v3_mode.then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
            "1",
        )))
        .envs(v3_mode.then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
            config_home.join("oulipoly-agent-runner").to_str().unwrap(),
        )))
        .envs(
            mode.starts_with("normal_model_provider_v3_quota_route")
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
        )
        .envs(v3_physical.then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")))
        .envs(mode.ends_with("shared_reply_loss").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_SHARED_Q_REPLY_V3_V1",
            "1",
        )))
        .envs(
            matches!(
                mode.as_str(),
                "normal_model_provider_v3_quota_route_reply_loss"
                    | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                    | "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss"
            )
            .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ROUTE_V3_REPLY_V1", "1")),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_route_manual_reply_loss"
                || mode
                    == "normal_model_provider_v3_quota_route_manual_physical_manual_reply_loss")
                .then_some((
                    "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_MANUAL_BEGIN_REPLY_V3_V1",
                    "1",
                )),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_route_manual_unknown").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_MANUAL_POST_K_CAS_V3_V1",
                "1",
            )),
        )
        .envs((mode == "normal_model_provider_reply_loss").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
            "1",
        )))
        .envs(mode.ends_with("physical_reply_loss").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
            "1",
        )))
        .envs(mode.ends_with("physical_post_k").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_PROVIDER_POST_K_CAS_V3_V1",
            "1",
        )))
        .envs(
            (mode == "normal_model_provider_pty_physical_reply_loss").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_INTERACTIVE_K_REPLY_V1",
                "1",
            )),
        )
        .envs(
            (mode == "normal_model_provider_pty_physical_post_k_unknown").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_INTERACTIVE_POST_K_UNKNOWN_V1",
                "1",
            )),
        )
        .envs(mode.contains("resident_bash_after_append").then_some((
            "OULIPOLY_KERNEL_BROKER_FIXTURE_INTERACTIVE_AFTER_APPEND_V1",
            "1",
        )))
        .envs(
            matches!(
                mode.as_str(),
                "normal_model_provider_quota_reply_loss"
                    | "normal_model_provider_auth_reply_loss"
                    | "normal_model_provider_v3_quota_reply_loss"
            )
            .then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ACCOUNT_EFFECT_REPLY_V1",
                "1",
            )),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_auth_reply_loss").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_AUTH_EFFECT_REPLY_V3_V1",
                "1",
            )),
        )
        .envs(native_mode.then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_NATIVE_GATE_V1", &gate)))
        .envs(
            (mode == "normal_model_provider_v3_quota_post_k").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_QUOTA_POST_K_CAS_V3_V1",
                "1",
            )),
        )
        .envs(
            (mode == "normal_model_provider_v3_quota_auth_post_k").then_some((
                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_AUTH_POST_K_CAS_V3_V1",
                "1",
            )),
        )
        .envs(
            (mode == "held_release_gate_fail")
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_GATE_WRITE_V1", "1")),
        )
        .envs(
            (mode == "normal_handoff_fsync")
                .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_HANDOFF_SYNC_V1", "1")),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&broker_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| UnixStream::connect(&socket).is_ok() || broker.try_wait().unwrap().is_some());
    assert!(
        broker.try_wait().unwrap().is_none() && UnixStream::connect(&socket).is_ok(),
        "broker startup: {}",
        fs::read_to_string(&broker_log).unwrap()
    );
    if manual_setup {
        let run_manual = || {
            Command::new(&runner)
                .arg("--usage")
                .arg("--models-dir")
                .arg(config_home.join("oulipoly-agent-runner/models"))
                .env("OULIPOLY_DATA_DIR", &data)
                .env("OULIPOLY_CONFIG_HOME", &config_home)
                .env(
                    "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                    socket.with_file_name("v30.sock"),
                )
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                .envs(terminal_v3.then_some(("AGE319_PRIVATE_ROOT_TERMINAL_V1", "1")))
                .envs(terminal_v3.then_some(("AGE319_PRIVATE_CALLER_OUTPUT_V1", "1")))
                .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                .env(
                    "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                    gate.join("provider-effect"),
                )
                .env_remove("LD_LIBRARY_PATH")
                .output()
                .unwrap()
        };
        let manual = run_manual();
        assert_eq!(
            manual.status.success(),
            !matches!(
                mode.as_str(),
                "normal_model_provider_v3_quota_route_manual_invalid"
                    | "normal_model_provider_v3_quota_route_manual_failed"
                    | "normal_model_provider_v3_quota_route_manual_pending"
                    | "normal_model_provider_v3_quota_route_manual_unknown"
            ),
            "manual stdout: {} stderr: {} broker: {}",
            String::from_utf8_lossy(&manual.stdout),
            String::from_utf8_lossy(&manual.stderr),
            fs::read_to_string(&broker_log).unwrap()
        );
        if mode == "normal_model_provider_v3_quota_route_manual_refresh"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_refresh"
        {
            fs::write(
                gate.join("quota.json"),
                br#"{"used_percent":24,"resets_at":"2099-01-01T00:00:00Z"}"#,
            )
            .unwrap();
            let refreshed = run_manual();
            assert!(
                refreshed.status.success(),
                "manual refresh: {}",
                String::from_utf8_lossy(&refreshed.stdout)
            );
            assert!(String::from_utf8_lossy(&refreshed.stdout).contains("24%"));
        }
        let operations = broker_state.join("v30/fresh-provider/manual-quota");
        if mode == "normal_model_provider_v3_quota_route_manual_cross_model"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_cross_model"
        {
            let source = fs::read_dir(&operations)
                .unwrap()
                .filter_map(Result::ok)
                .find(|entry| entry.path().join("k.json").exists())
                .unwrap();
            let intent: serde_json::Value =
                serde_json::from_slice(&fs::read(source.path().join("intent.json")).unwrap())
                    .unwrap();
            assert_eq!(intent["request"]["model"], "alias");
        }
        let physical_k_count = || match fs::read_dir(&operations) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("k.json").exists())
                .count(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("manual physical K directory read failed: {error}"),
        };
        let expected_k = if mode == "normal_model_provider_v3_quota_route_manual_refresh"
            || mode == "normal_model_provider_v3_quota_route_manual_physical_refresh"
        {
            2
        } else {
            1
        };
        if matches!(
            mode.as_str(),
            "normal_model_provider_v3_quota_route_manual_pending"
                | "normal_model_provider_v3_quota_route_manual_unknown"
        ) {
            // Pending quota work may persist physical K just after the CLI
            // returns. A bounded wait still refuses a missing or extra K.
            eventually(|| physical_k_count() == expected_k);
        }
        assert_eq!(
            physical_k_count(),
            expected_k,
            "manual call must spend one physical K per deliberate refresh"
        );
    }
    if normal_mode {
        let generation = broker_generation.unwrap();
        fs::write(data.join("pid-identity.db"), b"retired copied owner").unwrap();
        let out = temp.path().join("normal.out");
        let err = temp.path().join("normal.err");
        let mut entry = Command::new(&runner)
            .arg(if mode == "normal_help" {
                "--help"
            } else if model_mode {
                "--model"
            } else if handoff_mode {
                "__age319-private-root-handoff-v1"
            } else {
                "__age319-private-normal-v30"
            })
            .args(if provider_mode {
                if mode == "normal_model_provider_no_pin" {
                    vec!["configured-model", "hello fixture"]
                } else {
                    vec![
                        "configured-model",
                        "--pin-provider",
                        "local",
                        "hello fixture",
                    ]
                }
            } else if model_mode {
                vec!["fixture-model", "hello fixture"]
            } else {
                Vec::new()
            })
            .env("OULIPOLY_DATA_DIR", &data)
            .envs(
                (provider_mode || mode == "normal_owner_discovery" || mode == "normal_model_held")
                    .then_some(("OULIPOLY_CONFIG_HOME", &config_home)),
            )
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .envs(featureless_normal.then_some(("AGE319_TEST_FEATURELESS_DUP_PUBLISH_V1", "1")))
            .envs(
                (mode == "normal_model_held")
                    .then_some(("AGE319_PRIVATE_MODEL_SELECTION_PROBE_V1", "1")),
            )
            .envs(physical_normal.then_some(("AGE319_PRIVATE_NORMAL_PHYSICAL_V1", "1")))
            .envs(
                physical_normal_bad_source.then_some(("AGE319_PRIVATE_NORMAL_BAD_SOURCE_V1", "1")),
            )
            .envs(physical_normal.then_some(("AGE319_EFFECT_FILE", normal_effect.as_os_str())))
            .envs(native_codex_mode.then_some(("AGE319_PRIVATE_NATIVE_CODEX_V1", "1")))
            .envs(native_codex_mode.then_some(("AGE319_PRIVATE_NATIVE_BASH_NOTIFY_V1", "1")))
            .envs(native_turn_mode.then_some(("AGE319_PRIVATE_NATIVE_TURN_RECEIPT_V1", "1")))
            .envs(native_f_turn.then_some(("AGE319_PRIVATE_NATIVE_F_TURN_V1", "1")))
            .envs(
                native_f_candidate
                    .then_some(("AGE319_PRIVATE_BASH_RECIPIENT_MODE_V1", "native_k_pending")),
            )
            .envs(resident_mode.then_some(("AGE319_PRIVATE_NATIVE_STORE", &native_store)))
            .envs(resident_mode.then_some(("AGE319_PRIVATE_ROOT_PTY_RESIDENT_V1", "1")))
            .envs(
                mode.contains("_f_fenced")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_NATIVE_F_FENCE_V1", "1")),
            )
            .envs(physical_f.then_some(("AGE319_PRIVATE_ROOT_PTY_NATIVE_F_PHYSICAL_V1", "1")))
            .envs(native_crash.map(|stage| {
                (
                    "AGE319_PRIVATE_NATIVE_F_FAULT_V1",
                    stage.split("_changed_").next().unwrap(),
                )
            }))
            .envs(
                (mode.ends_with("_physical_partial") || native_crash == Some("after_partial"))
                    .then_some(("AGE319_PRIVATE_NATIVE_F_PARTIAL_WRITE_V1", "1")),
            )
            .envs(
                mode.ends_with("_adapter_unsupported")
                    .then_some(("AGE319_PRIVATE_NATIVE_ADAPTER_UNSUPPORTED_V1", "1")),
            )
            .envs(resident_failure.map(|case| ("AGE319_PRIVATE_RESIDENT_FAILURE_V1", case)))
            .envs(
                bash.as_ref()
                    .map(|path| ("AGE319_PRIVATE_BASH_IMAGE", path)),
            )
            .envs(
                (mode == "normal_handoff_bash_child")
                    .then_some(("AGE319_PRIVATE_BASH_CHILD_V1", "1")),
            )
            .envs(bash.as_ref().map(|_| {
                (
                    "AGE319_PRIVATE_BASH_EFFECT_MARKER",
                    gate.join("bash-effect"),
                )
            }))
            .envs(
                bash.as_ref()
                    .map(|_| ("AGE319_PRIVATE_BASH_REQUEST_KEY", &bash_request)),
            )
            .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
            .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
            .envs((mode == "normal_help").then_some(("AGE319_PRIVATE_OFFLINE_ROOT_V1", "1")))
            .envs(model_mode.then_some(("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")))
            .envs(provider_mode.then_some(("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")))
            .envs(
                mode.starts_with("normal_model_provider_pty_")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_CONTROL_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_pty_control")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_REPLAY_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_pty_control"
                    || mode == "normal_model_provider_pty_physical_resident_tail")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_NEGATIVE_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_pty_restart"
                    || mode == "normal_model_provider_pty_physical_restart")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_RESTART_V1", "1")),
            )
            .envs(physical_mode.then_some(("AGE319_PRIVATE_ROOT_PTY_PHYSICAL_V1", "1")))
            .envs(
                (mode == "normal_model_provider_pty_physical_wrong_plan")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_PHYSICAL_NEGATIVE_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_pty_physical_wrong_actor")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_PHYSICAL_ACTOR_GATE_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_pty_physical_finalizer_wrong_pair")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_FINALIZER_WRONG_PAIR_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_pty_physical_root_exit")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_EXIT_AFTER_K_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_pty_physical_restart_after_k")
                    .then_some(("AGE319_PRIVATE_ROOT_PTY_RESTART_AFTER_K_V1", "1")),
            )
            .envs(
                (root_terminal_drain
                    || terminal_v3
                    || caller_mode
                    || shared_mode
                    || matches!(
                        mode.as_str(),
                        "normal_model_provider"
                            | "normal_model_provider_bash_causal"
                            | "normal_model_provider_bash_causal_success"
                            | "normal_model_provider_bash_causal_root_h_delegate"
                            | "normal_model_provider_bash_causal_root_h_lost"
                            | "normal_model_provider_bash_causal_root_h_notify_row"
                            | "normal_model_provider_bash_causal_root_h_notify_wake"
                            | "normal_model_provider_bash_causal_root_h_notify_wake_lost"
                            | "normal_model_provider_bash_causal_root_h_notify_wake_unknown"
                            | "normal_model_provider_bash_causal_notify_ack"
                            | "normal_model_provider_bash_causal_notify_prepare_unavailable"
                            | "normal_model_provider_bash_causal_notify_lost_pending"
                    ))
                .then_some(("AGE319_PRIVATE_ROOT_TERMINAL_V1", "1")),
            )
            .envs(
                (caller_mode || terminal_v3 || shared_mode)
                    .then_some(("AGE319_PRIVATE_CALLER_OUTPUT_V1", "1")),
            )
            .envs(root_terminal_drain.then_some(("AGE319_PRIVATE_ROOT_DRAIN_V1", "1")))
            .envs(
                (mode == "normal_model_provider_caller_partial")
                    .then_some(("AGE319_PRIVATE_CALLER_PARTIAL_WRITE_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_caller_lost")
                    .then_some(("AGE319_PRIVATE_CALLER_LOST_WRITE_V1", "1")),
            )
            .envs(
                (mode.starts_with("normal_model_provider_bash_causal") || resident_bash)
                    .then_some(("AGE319_PRIVATE_PROVIDER_CAUSAL_BASH_V1", "1"))
                    .or_else(|| {
                        mode.starts_with("normal_model_provider_bash_ordinary")
                            .then_some(("AGE319_PRIVATE_PROVIDER_CAUSAL_BASH_V1", "1"))
                    }),
            )
            .envs(
                mode.strip_prefix("normal_model_provider_bash_ordinary_script")
                    .map(|suffix| {
                        (
                            "AGE319_PRIVATE_BASH_ORDINARY_MODE_V1",
                            format!("ordinary-script{}", suffix.replace('_', "-")),
                        )
                    }),
            )
            .envs(
                (mode == "normal_model_provider_bash_ordinary_refuse")
                    .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-refuse"))
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_parent_tamper").then_some((
                            "AGE319_PRIVATE_BASH_ORDINARY_MODE_V1",
                            "ordinary-parent-tamper",
                        ))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_failure")
                            .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-failure"))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_cancel")
                            .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-cancel"))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_elf")
                            .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-elf"))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_restart")
                            .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-restart"))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_copy")
                            .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-copy"))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_loss")
                            .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-loss"))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_sync_parent_output")
                            .then_some((
                                "AGE319_PRIVATE_BASH_ORDINARY_MODE_V1",
                                "ordinary-sync-parent-output",
                            ))
                    })
                    .or_else(|| {
                        (mode == "normal_model_provider_bash_ordinary_sync")
                            .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", "ordinary-sync"))
                            .or_else(|| {
                                (mode == "normal_model_provider_bash_ordinary_async").then_some((
                                    "AGE319_PRIVATE_BASH_ORDINARY_MODE_V1",
                                    "ordinary-async",
                                ))
                            })
                    })
                    .or_else(|| {
                        [
                            ("sync_reply_loss", "ordinary-sync-reply-loss"),
                            ("sync_partial", "ordinary-sync-partial"),
                            ("sync_repeat", "ordinary-sync-repeat"),
                            ("sync_large", "ordinary-sync-large"),
                            ("sync_signal", "ordinary-sync-signal"),
                            ("sync_tamper", "ordinary-sync-tamper"),
                            ("sync_w_debt", "ordinary-sync-w-debt"),
                            ("sync_socket_partial", "ordinary-sync-socket-partial"),
                            ("sync_post_tamper", "ordinary-sync-post-tamper"),
                            ("sync_encode_tamper", "ordinary-sync-encode-tamper"),
                        ]
                        .into_iter()
                        .find_map(|(suffix, value)| {
                            (mode == format!("normal_model_provider_bash_ordinary_{suffix}"))
                                .then_some(("AGE319_PRIVATE_BASH_ORDINARY_MODE_V1", value))
                        })
                    }),
            )
            .envs(
                (root_h_notify || mode.contains("_notify_") || resident_notify)
                    .then_some(("AGE319_PRIVATE_BASH_ORIGINAL_NOTIFY_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_bash_causal_success" || root_h_delegate)
                    .then_some(("AGE319_PRIVATE_BASH_SOURCE_SUCCESS_V1", "1")),
            )
            .envs(
                (mode == "normal_model_provider_bash_causal_notify_ack"
                    || mode == "normal_model_provider_bash_causal_notify_debt"
                    || mode == "normal_model_provider_bash_causal_notify_row_debt")
                    .then_some(("AGE319_PRIVATE_BASH_RECIPIENT_MODE_V1", "ack")),
            )
            .envs(
                (mode == "normal_model_provider_bash_causal_notify_lost_pending")
                    .then_some(("AGE319_PRIVATE_BASH_RECIPIENT_MODE_V1", "lost_pending")),
            )
            .envs(
                (mode == "normal_model_provider_bash_causal_notify_prepare_unavailable").then_some(
                    (
                        "AGE319_PRIVATE_BASH_RECIPIENT_MODE_V1",
                        "prepare_unavailable",
                    ),
                ),
            )
            .envs(
                provider_mode
                    .then_some(("AGE319_PRIVATE_PROVIDER_IMAGE_V1", provider_image.as_str())),
            )
            .envs(provider_mode.then_some((
                "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                gate.join("provider-effect").to_str().unwrap(),
            )))
            .envs(
                (mode == "normal_handoff").then_some(("AGE319_PRIVATE_HANDOFF_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_handoff_effect_reply_loss")
                    .then_some(("AGE319_PRIVATE_EFFECT_REPLY_LOSS_V1", "1")),
            )
            .envs(pending_binding.as_ref().map(|binding| {
                (
                    "AGE319_PRIVATE_EXPECT_SOURCE_DIGEST_V1",
                    binding.registration_digest(),
                )
            }))
            .envs(
                recipient_mode.then_some(("AGE319_PRIVATE_RECIPIENT_SELECTION_CHALLENGE_V1", "1")),
            )
            .envs(recipient_mail.as_ref().map(|row| {
                (
                    "AGE319_PRIVATE_EXPECT_RECIPIENT_SHA_V1",
                    row.payload_sha256.as_deref().unwrap(),
                )
            }))
            .envs(
                (mode == "normal_prepare_lost_reply")
                    .then_some(("AGE319_PRIVATE_NORMAL_PREPARE_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_release_lost_reply")
                    .then_some(("AGE319_PRIVATE_NORMAL_RELEASE_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_repair_lost_reply")
                    .then_some(("AGE319_PRIVATE_REPAIR_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (mode == "normal_source_grant_lost_reply")
                    .then_some(("AGE319_PRIVATE_SOURCE_GRANT_REPLY_LOSS_V1", "1")),
            )
            .envs(real_source.then_some(("AGE319_PRIVATE_SOURCE_LAUNCH_REPLAY_V1", "1")))
            .envs(
                (mode == "normal_owner_discovery")
                    .then_some(("AGE319_PRIVATE_OWNER_DISCOVERY_PROBE_V1", "1")),
            )
            .envs(
                (mode == "normal_handoff_pre_k_h_source" || production_source)
                    .then_some(("AGE319_PRIVATE_PRE_K_H_SOURCE_V1", "1")),
            )
            .envs(root_h_delegate.then_some(("AGE319_PRIVATE_SELECTED_K_ROOT_H_V1", "1")))
            .envs(
                (mode == "normal_empty" || bad_disposition || production_source || root_h_notify)
                    .then_some(("AGE319_PRIVATE_PRE_EFFECT_DISPOSITION_V1", "1")),
            )
            .envs(
                (postcommit_source || root_h_notify)
                    .then_some(("AGE319_PRIVATE_POSTCOMMIT_H_V1", "1")),
            )
            .envs((postcommit_source || root_h_notify).then_some((
                "AGE319_PRIVATE_SOURCE_Q_MARKER_V1",
                gate.join("source-adopted-started"),
            )))
            .envs(completed_v2_source.then_some(("AGE319_PRIVATE_V2_WAIT_SOURCE_V1", "1")))
            .envs(bad_disposition.then_some(("AGE319_PRIVATE_BAD_DISPOSITION_ROOT_V1", "1")))
            .envs(
                (mode == "normal_bash_source_lost_reply")
                    .then_some(("AGE319_PRIVATE_SOURCE_LAUNCH_REPLY_LOSS_V1", "1")),
            )
            .envs(
                (nonzero_source || io_failure_source)
                    .then_some(("AGE319_PRIVATE_SOURCE_PRELAUNCH_BARRIER_V1", "1")),
            )
            .env_remove("LD_LIBRARY_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).unwrap()))
            .stderr(Stdio::from(File::create(&err).unwrap()))
            .spawn()
            .unwrap();
        if featureless_normal {
            if std::env::var_os("AGE319_TEST_FEATURELESS_LOST_K_V1").is_some() {
                let until = Instant::now() + Duration::from_secs(30);
                while !gate.join("featureless-k-sent").exists()
                    && entry.try_wait().unwrap().is_none()
                {
                    assert!(
                        Instant::now() < until,
                        "featureless K was not sent: runner: {}; broker: {}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(gate.join("featureless-k-sent").exists());
                let until = Instant::now() + Duration::from_secs(30);
                loop {
                    let state =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    let count: i64 = state
                        .query_row("SELECT count(*) FROM fresh_normal_provider_k", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    if count == 1 {
                        break;
                    }
                    assert!(
                        Instant::now() < until,
                        "featureless K did not commit before restart"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                while !normal_effect.exists() {
                    assert!(
                        Instant::now() < until,
                        "featureless provider effect absent before restart"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                stop(&mut broker);
                let restart_log = temp.path().join("featureless-k-restart.log");
                broker =
                    restart_source_broker(&socket, &broker_state, &runner, &gate, &restart_log);
                fs::write(gate.join("featureless-k-resume"), b"restarted").unwrap();
            }
            let until = Instant::now() + Duration::from_secs(90);
            while entry.try_wait().unwrap().is_none() {
                assert!(
                    Instant::now() < until,
                    "featureless normal root stalled: runner: {}; broker: {}",
                    fs::read_to_string(&err).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default(),
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let status = entry.wait().unwrap();
            let partial = std::env::var_os("AGE319_TEST_NORMAL_CALLER_PARTIAL_V1").is_some();
            let lost = std::env::var_os("AGE319_TEST_NORMAL_CALLER_LOST_V1").is_some();
            let nonzero = std::env::var_os("AGE319_TEST_FEATURELESS_NONZERO_V1").is_some();
            let binary = std::env::var_os("AGE319_TEST_FEATURELESS_BINARY_V1").is_some();
            if partial || lost {
                assert!(!status.success());
                assert_eq!(
                    fs::read(&out).unwrap(),
                    if partial {
                        b"n".as_slice()
                    } else {
                        b"".as_slice()
                    }
                );
                assert!(
                    fs::read_to_string(&err)
                        .unwrap()
                        .contains("publication unknown")
                );
            } else {
                assert_eq!(
                    status.code(),
                    Some(if nonzero { 17 } else { 0 }),
                    "runner: {}; broker: {}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap_or_default()
                );
                assert_eq!(
                    fs::read(&out).unwrap(),
                    if binary {
                        b"\xff\0normal-provider-out:hello fixture".as_slice()
                    } else {
                        b"normal-provider-out:hello fixture".as_slice()
                    }
                );
                assert_eq!(
                    fs::read(&err).unwrap(),
                    if binary {
                        b"\0\xffnormal-provider-err\n".as_slice()
                    } else {
                        b"normal-provider-err\n".as_slice()
                    }
                );
            }
            assert_eq!(fs::read_to_string(&normal_effect).unwrap(), "one\n");
            let state = rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
            let admission_id: String = state
                .query_row(
                    "SELECT json_extract(k_json, '$.admission_id') FROM fresh_normal_provider_k",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let publication = broker_state.join("v30/normal-provider").join(&admission_id);
            assert!(publication.join("q.json").exists());
            assert!(publication.join("caller-intent.json").exists());
            let q: serde_json::Value =
                serde_json::from_slice(&fs::read(publication.join("q.json")).unwrap()).unwrap();
            let intent: serde_json::Value =
                serde_json::from_slice(&fs::read(publication.join("caller-intent.json")).unwrap())
                    .unwrap();
            assert_eq!(q["provider_wait_status"], if nonzero { 17 << 8 } else { 0 });
            assert_eq!(intent["q"], q);
            assert_eq!(intent["k"]["admission_id"], admission_id);
            assert_eq!(
                publication.join("caller-settled.json").exists(),
                !(partial || lost)
            );
            let release_path = fs::read_dir(broker_state.join("released-handoffs"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let released: oulipoly_state::mailbox::FreshReleasedHandoff =
                serde_json::from_slice(&fs::read(release_path).unwrap()).unwrap();
            assert!(
                protocol::observe_fresh_normal_publication_at(
                    &socket.with_file_name("v30.sock"),
                    &released.d_key
                )
                .is_err(),
                "a non-actor read the caller result"
            );
            stop(&mut broker);
            return;
        }
        eventually(|| gate.join("held").exists() || entry.try_wait().unwrap().is_some());
        assert!(
            gate.join("held").exists(),
            "normal held J: {} broker: {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        assert!(!gate.join("prepared").exists());
        let held_db = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let held_running: i64 = held_db
            .query_row(
                "SELECT count(*) FROM completion_continuation_owner",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(held_running, 0);
        let held_prepared: i64 = held_db
            .query_row("SELECT count(*) FROM broker_prepared_owner", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(held_prepared, 0, "held J must precede prepared W");
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        fs::write(gate.join("prepare"), b"yes").unwrap();
        eventually(|| gate.join("prepared").exists() || entry.try_wait().unwrap().is_some());
        assert!(
            gate.join("prepared").exists(),
            "normal entry: {} broker: {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        let prepared: oulipoly_state::mailbox::PreparedBrokerOwner =
            serde_json::from_slice(&fs::read(gate.join("prepared")).unwrap()).unwrap();
        assert_eq!(prepared.source_generation, generation);
        assert_eq!(prepared.entry.host_pid, entry.id() as i32);
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let db = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let running: i64 = db
            .query_row(
                "SELECT count(*) FROM completion_continuation_owner",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(running, 0, "prepared W must not mint running owner");
        let record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                broker_state
                    .join("entries")
                    .join(format!("{}.json", prepared.root_id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(record["join_consumed"], true);
        assert_eq!(
            record["joined_child"]["host_pid"],
            prepared.joined_child.host_pid
        );
        assert_eq!(
            fs::read(data.join("pid-identity.db")).unwrap(),
            b"retired copied owner"
        );
        let prepared_read = protocol::StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::read_prepared_owner_at(&socket, &prepared_read).is_err());
        if mode == "normal_guardian_death" {
            unsafe { libc::kill(prepared.guardian.host_pid, libc::SIGKILL) };
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            assert!(!entry.wait().unwrap().success());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        } else if mode == "normal_driver_death" {
            unsafe { libc::kill(prepared.driver.host_pid, libc::SIGKILL) };
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            assert!(!entry.wait().unwrap().success());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        } else if mode == "normal_broker_death" {
            stop(&mut broker);
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            assert!(!entry.wait().unwrap().success());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        } else {
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| gate.join("released").exists() || entry.try_wait().unwrap().is_some());
            assert!(
                gate.join("released").exists(),
                "{}",
                fs::read_to_string(&err).unwrap()
            );
            let released: oulipoly_state::mailbox::BrokerReleaseEvidence =
                serde_json::from_slice(&fs::read(gate.join("released")).unwrap()).unwrap();
            assert_eq!(released.prepared, prepared);
            if mode == "normal_handoff_fsync" {
                eventually(|| entry.try_wait().unwrap().is_some());
                assert!(!entry.wait().unwrap().success());
                assert!(!gate.join("child-handoff").exists());
                assert!(!gate.join("child-attested").exists());
                assert!(
                    fs::read_to_string(&err)
                        .unwrap()
                        .contains("private interrupted handoff before fsync")
                );
                assert_eq!(
                    fs::read_to_string(
                        broker_state
                            .join("released-handoffs")
                            .join(format!("{}.json", prepared.root_id))
                    )
                    .unwrap(),
                    "{"
                );
                let fresh = rusqlite::Connection::open_with_flags(
                    broker_state.join("v30/state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let started: i64 = fresh
                    .query_row("SELECT count(*) FROM invocations", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(started, 0, "uncertain old receipt cannot create fresh D");
                stop(&mut broker);
                broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .stderr(Stdio::from(
                        File::create(temp.path().join("handoff-fsync-restart.log")).unwrap(),
                    ))
                    .spawn()
                    .unwrap();
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                let old = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let pending: i64 = old.query_row(
                    "SELECT count(*) FROM mailbox WHERE handle='old-unacked' AND delivered_at IS NULL",
                    [], |row| row.get(0),
                ).unwrap();
                assert_eq!(pending, 1);
                assert!(fs::metadata(&out).unwrap().len() == 0);
                stop(&mut broker);
                return;
            }
            if mode == "normal_owner_discovery" {
                let deadline = Instant::now() + Duration::from_secs(20);
                while !gate.join("child-attested").exists() && entry.try_wait().unwrap().is_none() {
                    assert!(
                        Instant::now() < deadline,
                        "owner discovery stalled: entry={} broker={} gate={:?}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                        fs::read_dir(&gate)
                            .unwrap()
                            .filter_map(Result::ok)
                            .map(|e| e.file_name())
                            .collect::<Vec<_>>()
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            } else {
                eventually(|| {
                    gate.join("child-attested").exists() || entry.try_wait().unwrap().is_some()
                });
            }
            assert!(
                gate.join("child-attested").exists(),
                "{}",
                fs::read_to_string(&err).unwrap()
            );
            if bad_disposition {
                eventually(|| entry.try_wait().unwrap().is_some());
                assert!(!entry.wait().unwrap().success());
                assert!(
                    fs::read_to_string(&err)
                        .unwrap_or_default()
                        .contains("v30 child disposition changed released root/owner")
                );
                assert!(fs::read(&out).unwrap().is_empty());
                assert!(!gate.join("source-launched").exists());
                assert!(!gate.join("child-effect").exists());
                stop(&mut broker);
                unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                return;
            }
            if mode == "normal_owner_discovery" {
                let discovered: protocol::OwnerDiscoveryReadback =
                    serde_json::from_slice(&fs::read(gate.join("owner-discovery")).unwrap())
                        .unwrap();
                assert_eq!(discovered.root_id, prepared.root_id);
                assert_eq!(discovered.source_generation, generation);
                assert_eq!(discovered.release_id, released.release_id);
                assert_eq!(discovered.owner, released.owner);
                assert!(discovered.pid_binding.is_none());
                assert_eq!(
                    fs::read(data.join("pid-identity.db")).unwrap(),
                    b"retired copied owner"
                );
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                eventually(|| entry.try_wait().unwrap().is_some());
                let _ = entry.wait();
                stop(&mut broker);
                unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                return;
            }
            if mode == "normal_handoff_pre_k_h_source" || production_source {
                let until = Instant::now() + Duration::from_secs(90);
                while !gate.join("source-ready").exists()
                    && entry.try_wait().unwrap().is_none()
                    && Instant::now() < until
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let source_ready = gate.join("source-ready").exists();
                let h_ready = gate.join("h-source-ready").exists();
                let source_error =
                    fs::read_to_string(gate.join("private-source-stderr.log")).unwrap_or_default();
                let entry_error = fs::read_to_string(&err).unwrap_or_default();
                let broker_error = fs::read_to_string(&broker_log).unwrap_or_default();
                let restarted_error =
                    fs::read_to_string(temp.path().join("normal-restart.log")).unwrap_or_default();
                let guardian_control =
                    fs::read_to_string(gate.join("guardian-pre-k-control.log")).unwrap_or_default();
                let guardian_environment =
                    fs::read(format!("/proc/{}/environ", prepared.guardian.host_pid))
                        .unwrap_or_default();
                let guardian_environment = String::from_utf8_lossy(&guardian_environment)
                    .split('\0')
                    .filter(|entry| {
                        entry.starts_with("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1=")
                            || entry.starts_with("AGE319_PRIVATE_PRE_K_H_SOURCE_V1=")
                    })
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                let bash_state = fs::read_dir(gate.join("agent-bash"))
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|entry| {
                        let dir = entry.path();
                        (
                            dir.display().to_string(),
                            fs::read_to_string(dir.join("root-work-diagnostic-v1.jsonl"))
                                .unwrap_or_default(),
                            fs::read_to_string(dir.join("root-work-accepted-v1.json"))
                                .unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>();
                let grants = fs::read_dir(broker_state.join("grants"))
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
                    .collect::<Vec<_>>();
                let works = fs::read_dir(broker_state.join("works"))
                    .ok()
                    .into_iter()
                    .flatten()
                    .filter_map(Result::ok)
                    .map(|entry| fs::read_to_string(entry.path()).unwrap_or_default())
                    .collect::<Vec<_>>();
                let work_processes = works
                    .iter()
                    .filter_map(|record| {
                        let value: serde_json::Value = serde_json::from_str(record).ok()?;
                        let pid = value["init_host_pid"].as_i64()?;
                        let children =
                            fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
                                .unwrap_or_default();
                        let wchan =
                            fs::read_to_string(format!("/proc/{pid}/wchan")).unwrap_or_default();
                        let child_states = children
                            .split_whitespace()
                            .map(|child| {
                                (
                                    child.to_owned(),
                                    fs::read_to_string(format!("/proc/{child}/wchan"))
                                        .unwrap_or_default(),
                                    fs::read(format!("/proc/{child}/cmdline")).unwrap_or_default(),
                                )
                            })
                            .collect::<Vec<_>>();
                        Some((pid, wchan, children, child_states))
                    })
                    .collect::<Vec<_>>();
                let h_path = fs::read_to_string(gate.join("h-source-ready")).unwrap_or_default();
                if !source_ready {
                    let _ = fs::write(gate.join("source-release"), b"done");
                    let _ = fs::write(gate.join("h-source-release"), b"done");
                    stop(&mut entry);
                    stop(&mut broker);
                    unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                    panic!(
                        "paired pre-K source failed: H={h_ready} K={source_ready} h_path={h_path:?} entry={entry_error} source={source_error} broker={broker_error} restarted={restarted_error} guardian={guardian_control} guardian_env={guardian_environment:?} bash_state={bash_state:?} grants={grants:?} works={works:?} work_processes={work_processes:?}"
                    );
                }
                let verified = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    assert!(entry.try_wait().unwrap().is_none(), "J must retain held D");
                    assert_eq!(
                        fs::read(data.join("pid-identity.db")).unwrap(),
                        b"retired copied owner"
                    );
                    let grant: serde_json::Value = grants
                        .iter()
                        .filter_map(|g| serde_json::from_str(g).ok())
                        .find(|g: &serde_json::Value| g["consumed"] == true)
                        .expect("consumed H grant absent");
                    assert_eq!(grant["version"], 3);
                    assert_eq!(grant["root_id"], prepared.root_id);
                    assert_eq!(
                        grant["sealed_helper"]["sha256"],
                        format!("{:x}", Sha256::digest(fs::read(&runner).unwrap()))
                    );
                    let h_pid = grant["initiator"]["pid"].as_i64().unwrap();
                    let bash_image = fs::metadata(bash.as_ref().unwrap()).unwrap();
                    let live_h = fs::metadata(format!("/proc/{h_pid}/exe")).unwrap();
                    assert_eq!(
                        (live_h.dev(), live_h.ino()),
                        (bash_image.dev(), bash_image.ino()),
                        "H source image changed"
                    );
                    let h_dir = fs::read_dir(gate.join("agent-bash"))
                        .unwrap()
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .find(|path| path.join("root-work-accepted-v1.json").exists())
                        .expect("Bash H acceptance artifact absent");
                    let intent: serde_json::Value = serde_json::from_slice(
                        &fs::read(h_dir.join("root-work-intent-v1.json")).unwrap(),
                    )
                    .unwrap();
                    let h_meta: serde_json::Value =
                        serde_json::from_slice(&fs::read(h_dir.join("meta.json")).unwrap())
                            .unwrap();
                    assert_eq!(
                        h_meta["delivery_helper"]["sha256"],
                        grant["sealed_helper"]["sha256"]
                    );
                    let work: serde_json::Value = works
                        .iter()
                        .filter_map(|w| serde_json::from_str(w).ok())
                        .find(|w: &serde_json::Value| w["accepted_grant_id"] == grant["grant_id"])
                        .expect("consumed K work absent");
                    let k_pid = work["init_host_pid"].as_i64().unwrap();
                    let children =
                        fs::read_to_string(format!("/proc/{k_pid}/task/{k_pid}/children")).unwrap();
                    let child_pids: Vec<i64> = children
                        .split_whitespace()
                        .map(|pid| pid.parse().unwrap())
                        .collect();
                    assert_eq!(child_pids.len(), 1, "K must have one direct Bash worker");
                    let worker_pid = child_pids[0];
                    let live_worker = fs::metadata(format!("/proc/{worker_pid}/exe")).unwrap();
                    assert_eq!(
                        (live_worker.dev(), live_worker.ino()),
                        (bash_image.dev(), bash_image.ino()),
                        "K worker image changed"
                    );
                    let status = fs::read_to_string(format!("/proc/{worker_pid}/status")).unwrap();
                    assert!(status.lines().any(|line| line == format!("PPid:\t{k_pid}")));
                    let physical = |name: &str| -> u32 {
                        status
                            .lines()
                            .find_map(|line| line.strip_prefix(name))
                            .and_then(|fields| fields.split_whitespace().next())
                            .and_then(|field| field.parse().ok())
                            .unwrap()
                    };
                    assert_eq!(
                        physical("Uid:"),
                        unsafe { libc::geteuid() },
                        "C/K physical UID changed"
                    );
                    assert_eq!(
                        physical("Gid:"),
                        unsafe { libc::getegid() },
                        "C/K physical GID changed"
                    );
                    let actual_env = fs::read(format!("/proc/{worker_pid}/environ")).unwrap();
                    let actual_env: Vec<&[u8]> = actual_env.split(|b| *b == 0).collect();
                    let selected_env: std::collections::BTreeSet<Vec<u8>> = intent["environment"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|entry| {
                            let key = entry["key"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|n| n.as_u64().unwrap() as u8)
                                .collect::<Vec<_>>();
                            let value = entry["value"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|n| n.as_u64().unwrap() as u8)
                                .collect::<Vec<_>>();
                            [key, b"=".to_vec(), value].concat()
                        })
                        .collect();
                    let physical_env: std::collections::BTreeSet<Vec<u8>> = actual_env
                        .iter()
                        .filter(|entry| !entry.is_empty())
                        .map(|entry| entry.to_vec())
                        .collect();
                    assert_eq!(
                        physical_env, selected_env,
                        "H-selected and K-actual environment differ"
                    );
                    for selected in [
                        b"AGE319_SELECTED_ENV=held-d-pre-k".as_slice(),
                        format!(
                            "AGE319_SELECTED_ACCOUNT={}:{}",
                            unsafe { libc::geteuid() },
                            unsafe { libc::getegid() }
                        )
                        .as_bytes(),
                    ] {
                        assert!(
                            actual_env.contains(&selected),
                            "C environment/account changed at K"
                        );
                        assert!(
                            intent["environment"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|entry| {
                                    let key = entry["key"]
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .map(|n| n.as_u64().unwrap() as u8)
                                        .collect::<Vec<_>>();
                                    let value = entry["value"]
                                        .as_array()
                                        .unwrap()
                                        .iter()
                                        .map(|n| n.as_u64().unwrap() as u8)
                                        .collect::<Vec<_>>();
                                    [key, b"=".to_vec(), value].concat() == selected
                                }),
                            "H did not select C environment/account"
                        );
                    }
                    let registration_path = fs::read_to_string(gate.join("source-ready")).unwrap();
                    let registration_path = Path::new(registration_path.trim());
                    assert_eq!(
                        registration_path.file_name().unwrap(),
                        "source-registration-v2.json"
                    );
                    let registration_bytes = fs::read(registration_path).unwrap();
                    let registration: serde_json::Value =
                        serde_json::from_slice(&registration_bytes).unwrap();
                    let registration_stat = fs::metadata(registration_path).unwrap();
                    assert_eq!(registration["registering_caller"]["pid"], worker_pid);
                    assert_eq!(
                        registration["helper"]["sha256"],
                        grant["sealed_helper"]["sha256"]
                    );
                    let worker_dir = registration_path.parent().unwrap();
                    if completed_v2_source {
                        let k_meta: serde_json::Value = serde_json::from_slice(
                            &fs::read(worker_dir.join("meta.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(
                            intent["argv"],
                            serde_json::json!(["/bin/true", "selected workload"])
                        );
                        assert_eq!(k_meta["argv"], intent["argv"]);
                        for name in [
                            "source-outcome-v2.json",
                            "completion-snapshot-v2.json",
                            "completion-output-v2.bin",
                        ] {
                            assert!(
                                !worker_dir.join(name).exists(),
                                "{name} preceded exact H registration"
                            );
                        }
                    }
                    let h_env: std::collections::BTreeMap<String, String> = serde_json::from_slice(
                        &fs::read(h_dir.join("delivery-helper-environment.json")).unwrap(),
                    )
                    .unwrap();
                    let k_env: std::collections::BTreeMap<String, String> = serde_json::from_slice(
                        &fs::read(worker_dir.join("delivery-helper-environment.json")).unwrap(),
                    )
                    .unwrap();
                    let different_keys: Vec<_> = h_env
                        .keys()
                        .chain(k_env.keys())
                        .filter(|key| h_env.get(*key) != k_env.get(*key))
                        .collect();
                    assert_eq!(
                        registration["helper"]["environment_sha256"],
                        h_meta["delivery_helper"]["environment_sha256"],
                        "H/K helper environment differs at keys: {different_keys:?}"
                    );
                    assert_eq!(
                        registration["owner_session_id"],
                        grant["sealed_helper"]["owner_session_id"]
                    );
                    assert_eq!(
                        registration["owner_invocation_uuid"],
                        grant["sealed_helper"]["owner_invocation_uuid"]
                    );
                    assert_eq!(registration_stat.nlink(), 1);
                    assert_eq!(fs::read(registration_path).unwrap(), registration_bytes);
                    let original = rusqlite::Connection::open_with_flags(
                        data.join("state.db"),
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap();
                    let invocation = grant["sealed_helper"]["owner_invocation_uuid"]
                        .as_str()
                        .unwrap();
                    let (session, capability): (String, String) = original.query_row(
                        "SELECT provider_session_id,completion_registration_capability_digest FROM invocations WHERE invocation_uuid=?1",
                        [invocation], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
                    assert_eq!(session, grant["sealed_helper"]["owner_session_id"]);
                    assert_eq!(
                        capability,
                        grant["sealed_helper"]["state_capability_digest"]
                    );
                    let decisions: i64 = original
                        .query_row(
                            "SELECT count(*) FROM invocation_completion_exact_source_decisions",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        decisions, 0,
                        "private source must stop before State decision"
                    );
                    let sidecar = rusqlite::Connection::open_with_flags(
                        broker_state.join("sidecar/pid-identity.db"),
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap();
                    for table in [
                        "completion_continuation_source",
                        "broker_source_effect_grant",
                        "broker_exact_source_projection",
                        "broker_fresh_source_admission",
                    ] {
                        let count: i64 = sidecar
                            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                                row.get(0)
                            })
                            .unwrap();
                        assert_eq!(count, 0, "private pre-register source changed {table}");
                    }
                    let fresh = rusqlite::Connection::open_with_flags(
                        broker_state.join("v30/state.db"),
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap();
                    let fresh_sources: i64 = fresh
                        .query_row(
                            "SELECT count(*) FROM fresh_bash_source_registration",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        fresh_sources, 0,
                        "private pre-register source changed fresh State"
                    );
                    let challenged: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("challenged-owner-readback.json")).unwrap(),
                    )
                    .unwrap();
                    let readback = &challenged["readback"];
                    assert_eq!(challenged["query_pid"], k_pid);
                    assert_eq!(readback["root_id"], prepared.root_id);
                    assert_eq!(
                        readback["owner"],
                        serde_json::to_value(&released.owner).unwrap()
                    );
                    assert_eq!(readback["domain_id"], released.owner.domain_id);
                    assert_eq!(
                        readback["owner_generation"],
                        released.owner.owner_generation
                    );
                    assert_eq!(readback["source_generation"], generation);
                    assert_eq!(readback["release_id"], released.release_id);
                    assert_eq!(readback["session_id"], session);
                    assert_eq!(readback["invocation_uuid"], invocation);
                    let guardian = UnixStream::connect(&released.owner.endpoint).unwrap();
                    let mut request = protocol::OwnerDiscoveryRequest {
                        root_id: prepared.root_id.clone(),
                        query_pid: Some(k_pid as i32),
                        expected_owner_generation: Some(released.owner.owner_generation.clone()),
                    };
                    assert!(
                        protocol::discover_owner_at(&socket, &request, guardian.as_raw_fd())
                            .is_err(),
                        "outside source copied a live K binding"
                    );
                    request.expected_owner_generation = Some(uuid::Uuid::new_v4().to_string());
                    assert!(
                        protocol::discover_owner_at(&socket, &request, guardian.as_raw_fd())
                            .is_err()
                    );
                    request.expected_owner_generation =
                        Some(released.owner.owner_generation.clone());
                    request.root_id = uuid::Uuid::new_v4().to_string();
                    assert!(
                        protocol::discover_owner_at(&socket, &request, guardian.as_raw_fd())
                            .is_err()
                    );
                    request.root_id = prepared.root_id.clone();
                    let (wrong_guardian, _other) = UnixStream::pair().unwrap();
                    assert!(
                        protocol::discover_owner_at(&socket, &request, wrong_guardian.as_raw_fd())
                            .is_err()
                    );
                    request.query_pid = Some(h_pid as i32);
                    assert!(
                        protocol::discover_owner_at(&socket, &request, guardian.as_raw_fd())
                            .is_err(),
                        "outside H source copied K binding"
                    );
                    if production_source {
                        let marked: i64 = original.query_row(
                            "SELECT count(*) FROM invocation_completion_exact_required WHERE invocation_uuid=?1",
                            [invocation], |row| row.get(0),
                        ).unwrap();
                        assert_eq!(marked, 1, "fresh v30 original-State route marker absent");
                    }
                    if production_source {
                        fs::write(gate.join("source-production-registration-cli"), b"yes").unwrap();
                    }
                    if postcommit_source {
                        fs::write(gate.join("source-production-postcommit-h"), b"schedule")
                            .unwrap();
                    }
                    fs::write(gate.join("source-decision-request"), b"issue").unwrap();
                    if production_source && !postcommit_source {
                        let deadline = Instant::now() + Duration::from_secs(180);
                        while !gate.join("source-decision-readback.json").exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < deadline
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        let reply: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("source-decision-readback.json")).unwrap_or_else(
                                |_| {
                                    panic!(
                                        "production Bash-byte registration absent: {}",
                                        fs::read_to_string(gate.join("private-source-stderr.log"))
                                            .unwrap_or_default()
                                    )
                                },
                            ),
                        )
                        .unwrap();
                        assert_eq!(reply["status"], "registered");
                        assert_eq!(reply["registration_committed"], true);
                        assert_eq!(reply["exact_source_projection_available"], false);
                        assert!(reply["exact_source_decision_id"].is_string());
                        let probes: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("source-production-probes.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(probes["broker_exact_retry"], true);
                        assert_eq!(probes["state_exact_retry"], true);
                        assert_eq!(probes["public_api_bypass_refused"], true);
                        assert_eq!(probes["refusals"].as_array().unwrap().len(), 6);
                    } else if !production_source {
                        let state_deadline = Instant::now() + Duration::from_secs(180);
                        while !gate.join("source-state-ready").exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < state_deadline
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            gate.join("source-state-ready").exists(),
                            "Bash decision did not reach State: {}",
                            fs::read_to_string(gate.join("private-source-stderr.log"))
                                .unwrap_or_default()
                        );
                        let state_count: i64 = original
                            .query_row(
                                "SELECT count(*) FROM invocation_completion_exact_source_decisions",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap();
                        assert_eq!(state_count, 0, "Bash decision committed before State gate");
                        fs::write(gate.join("source-clock-offset"), b"240").unwrap();
                        stop(&mut broker);
                        broker = restart_source_broker(
                            &socket,
                            &broker_state,
                            &runner,
                            &gate,
                            &temp.path().join("consumed-h-expired-before-state.log"),
                        );
                        fs::write(gate.join("source-state-stale"), b"yes").unwrap();
                        while !gate.join("source-state-stale-refused").exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < state_deadline
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            gate.join("source-state-stale-refused").exists(),
                            "expired H decision was not refused by State: {}",
                            fs::read_to_string(gate.join("private-source-stderr.log"))
                                .unwrap_or_default()
                        );
                        let still_zero: i64 = original
                            .query_row(
                                "SELECT count(*) FROM invocation_completion_exact_source_decisions",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap();
                        assert_eq!(still_zero, 0, "expired H decision changed State");
                        stop(&mut broker);
                        fs::remove_file(gate.join("source-clock-offset")).unwrap();
                        broker = restart_source_broker(
                            &socket,
                            &broker_state,
                            &runner,
                            &gate,
                            &temp.path().join("consumed-h-unexpired-state.log"),
                        );
                        fs::write(gate.join("source-state-commit"), b"yes").unwrap();
                        while !gate.join("source-state-admitted").exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < state_deadline
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            gate.join("source-state-admitted").exists(),
                            "Bash decision State commit/retry failed: {}",
                            fs::read_to_string(gate.join("private-source-stderr.log"))
                                .unwrap_or_default()
                        );
                    }
                    if postcommit_source {
                        let until = Instant::now() + Duration::from_secs(90);
                        while !gate.join("source-postcommit-ready").exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            gate.join("source-postcommit-ready").exists(),
                            "postcommit helper did not reach committed gate: entry={} source={} broker={} stage={} postcommit_step={} guardian_stage={} state_decisions={} helper_logs={:?} gate={:?}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(gate.join("private-source-stderr.log")).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default(),
                            fs::read_to_string(gate.join("source-broker-stage")).unwrap_or_default(),
                            fs::read_to_string(gate.join("source-postcommit-step")).unwrap_or_default(),
                            fs::read_to_string(gate.join("source-guardian-postcommit-stage")).unwrap_or_default(),
                            original.query_row::<i64,_,_>("SELECT count(*) FROM invocation_completion_exact_source_decisions", [], |r| r.get(0)).unwrap_or(-1),
                            fs::read_dir(worker_dir).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().contains("private-exact-source-decision") && e.path().extension().is_some_and(|x| x == "stderr")).map(|e| fs::read_to_string(e.path()).unwrap_or_default()).collect::<Vec<_>>(),
                            fs::read_dir(&gate).unwrap().filter_map(Result::ok).map(|e| e.file_name()).collect::<Vec<_>>(),
                        );
                        assert!(
                            entry.try_wait().unwrap().is_none(),
                            "original J exited before postcommit H custody"
                        );
                        assert!(Path::new(&format!("/proc/{worker_pid}")).exists());
                    }
                    assert!(
                        entry.try_wait().unwrap().is_none(),
                        "J exited before H projection"
                    );
                    let attribution: (String, String, String, String, String, i64, i64, Vec<u8>) = original.query_row(
                        "SELECT request_id,decision_id,registration_id,registration_sha256,projection_state,original_state_device,original_state_inode,broker_readback_json FROM invocation_completion_exact_source_decisions",
                        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?))
                    ).unwrap();
                    let original_stat = fs::metadata(data.join("state.db")).unwrap();
                    assert_eq!(
                        (attribution.5, attribution.6),
                        (original_stat.dev() as i64, original_stat.ino() as i64)
                    );
                    assert_eq!(attribution.2, registration["registration_id"]);
                    assert_eq!(
                        attribution.3,
                        format!("{:x}", Sha256::digest(&registration_bytes))
                    );
                    assert_eq!(attribution.4, "unavailable");
                    let state_readback: serde_json::Value =
                        serde_json::from_slice(&attribution.7).unwrap();
                    assert_eq!(state_readback["issuer_kind"], "consumed_h_sealed_helper");
                    assert_eq!(
                        state_readback["registration_worker"]["host_pid"],
                        worker_pid
                    );
                    assert_ne!(
                        state_readback["issuer"]["host_pid"],
                        state_readback["registration_worker"]["host_pid"]
                    );
                    assert_eq!(state_readback["owner_invocation_uuid"], invocation);
                    assert_eq!(state_readback["owner_session_id"], session);
                    let retained_path = broker_state.join("sidecar/pid-identity.db");
                    let mut retained =
                        BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                    assert!(
                        retained
                            .read_bounded_source_selection(
                                &generation,
                                &prepared.root_id,
                                &released.owner
                            )
                            .is_err(),
                        "unprojected H source became selectable"
                    );
                    assert!(
                        retained
                            .reserve_source_effect_grant(
                                &generation,
                                &prepared.root_id,
                                &released.owner
                            )
                            .is_err(),
                        "W reserved before H projection"
                    );
                    let receipt_count: i64 = sidecar
                        .query_row(
                            "SELECT count(*) FROM broker_exact_source_projection",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(receipt_count, 0, "H receipt preceded projection");
                    drop(retained);
                    if postcommit_source {
                        fs::write(gate.join("source-postcommit-notify"), b"schedule").unwrap();
                        let until = Instant::now() + Duration::from_secs(90);
                        while !gate.join("source-launched").exists()
                            && entry.try_wait().unwrap().is_none()
                            && !fs::read_to_string(gate.join("source-driver-postcommit-stage"))
                                .is_ok_and(|stage| stage.starts_with("refused:"))
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            gate.join("source-launched").exists(),
                            "postcommit H did not wake W: entry={} broker={} helper={} guardian_stage={} driver_stage={} broker_stage={} helper_logs={:?} gate={:?}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default(),
                            fs::read_to_string(gate.join("private-source-stderr.log"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("source-guardian-postcommit-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("source-driver-postcommit-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("source-broker-stage"))
                                .unwrap_or_default(),
                            fs::read_dir(worker_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|e| e
                                    .file_name()
                                    .to_string_lossy()
                                    .contains("private-exact-source-decision")
                                    && e.path().extension().is_some_and(|x| x == "stderr"))
                                .map(|e| fs::read_to_string(e.path()).unwrap_or_default())
                                .collect::<Vec<_>>(),
                            fs::read_dir(&gate)
                                .unwrap()
                                .filter_map(Result::ok)
                                .map(|e| e.file_name())
                                .collect::<Vec<_>>(),
                        );
                        eventually(|| gate.join("source-postcommit-retry-acked").exists());
                        assert_eq!(
                            fs::read(gate.join("source-postcommit-refusals")).unwrap(),
                            b"root owner sibling decision copied"
                        );
                        let projected = rusqlite::Connection::open_with_flags(
                            &retained_path,
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .unwrap();
                        let receipt: (String, String, String, String, String) = projected.query_row(
                            "SELECT request_id,decision_id,registration_id,registration_sha256,root_id FROM broker_exact_source_projection",
                            [], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
                        ).unwrap();
                        assert_eq!(
                            receipt,
                            (
                                attribution.0.clone(),
                                attribution.1.clone(),
                                attribution.2.clone(),
                                attribution.3.clone(),
                                prepared.root_id.clone()
                            )
                        );
                        let grant: (String, i64) = projected
                            .query_row(
                                "SELECT phase,revision FROM broker_source_effect_grant",
                                [],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .unwrap();
                        assert_eq!(grant, ("consumed".into(), 2));
                        let physical =
                            SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                                .unwrap();
                        assert_eq!(
                            physical.records().len(),
                            1,
                            "one W must create one physical custody tree"
                        );
                        let record = &physical.records()[0];
                        assert_eq!(record.grant.root_id, prepared.root_id);
                        assert_eq!(record.grant.source_generation, generation);
                        assert_eq!(record.grant.owner_generation, prepared.owner_generation);
                        assert_eq!(record.grant.candidate.registration_id, attribution.2);
                        assert_eq!(record.grant.candidate.registration_digest, attribution.3);
                        assert_eq!(record.grant.phase, "consumed");
                        eventually(|| gate.join("source-adopted-started").exists());
                        assert_eq!(
                            fs::read(gate.join("source-adopted-started")).unwrap(),
                            b"adopted\n"
                        );
                        assert!(
                            matches!(
                                physical.observe(&record.grant.grant_id).unwrap(),
                                SourceObservation::Live { .. }
                                    | SourceObservation::DrainPending { .. }
                            ),
                            "delayed adopted child must keep Q pending"
                        );
                        assert!(
                            !broker_state
                                .join("source-physical")
                                .join(format!("{}.terminal.json", record.grant.grant_id))
                                .exists()
                        );
                    } else {
                        stop(&mut broker);
                        broker = restart_source_broker(
                            &socket,
                            &broker_state,
                            &runner,
                            &gate,
                            &temp.path().join("consumed-h-before-projection.log"),
                        );
                        stop(&mut broker);
                        let mut retained =
                            BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                        let projected = retained
                            .repair_bounded_suffix(
                                &generation,
                                &prepared.root_id,
                                &released.owner,
                                0,
                            )
                            .unwrap();
                        assert_eq!(projected.authority_ordinal, 1);
                        drop(retained); // Simulate loss of the sidecar commit response.
                        broker = restart_source_broker(
                            &socket,
                            &broker_state,
                            &runner,
                            &gate,
                            &temp.path().join("consumed-h-after-projection.log"),
                        );
                        let mut retained =
                            BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                        let replay = retained
                            .repair_bounded_suffix(
                                &generation,
                                &prepared.root_id,
                                &released.owner,
                                0,
                            )
                            .unwrap();
                        assert_eq!(replay, projected, "H projection retry changed receipt");
                        let selected = retained
                            .read_bounded_source_selection(
                                &generation,
                                &prepared.root_id,
                                &released.owner,
                            )
                            .unwrap();
                        assert_eq!(
                            selected.candidate.as_ref().unwrap().registration_id,
                            attribution.2
                        );
                        assert!(
                            retained
                                .read_bounded_source_selection(
                                    &uuid::Uuid::new_v4().to_string(),
                                    &prepared.root_id,
                                    &released.owner
                                )
                                .is_err()
                        );
                        assert!(
                            retained
                                .read_bounded_source_selection(
                                    &generation,
                                    &uuid::Uuid::new_v4().to_string(),
                                    &released.owner
                                )
                                .is_err()
                        );
                        let mut stale_owner = released.owner.clone();
                        stale_owner.owner_generation = uuid::Uuid::new_v4().to_string();
                        assert!(
                            retained
                                .read_bounded_source_selection(
                                    &generation,
                                    &prepared.root_id,
                                    &stale_owner
                                )
                                .is_err()
                        );
                        let receipt: (String, String, String, String, String, Vec<u8>) = sidecar.query_row(
                        "SELECT request_id,decision_id,registration_id,registration_sha256,root_id,broker_readback_json FROM broker_exact_source_projection",
                        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
                    ).unwrap();
                        assert_eq!(
                            (&receipt.0, &receipt.1, &receipt.2, &receipt.3, &receipt.4),
                            (
                                &attribution.0,
                                &attribution.1,
                                &attribution.2,
                                &attribution.3,
                                &prepared.root_id
                            )
                        );
                        assert_eq!(receipt.5, attribution.7);
                        drop(retained);
                        fs::write(gate.join("source-projection-done"), b"yes").unwrap();
                    }
                    let decision_deadline = Instant::now() + Duration::from_secs(180);
                    while !gate.join("source-decision-readback.json").exists()
                        && entry.try_wait().unwrap().is_none()
                        && Instant::now() < decision_deadline
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    let issued: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("source-decision-readback.json")).unwrap_or_else(
                            |_| {
                                panic!(
                                    "actual Bash helper issuance absent: {}",
                                    fs::read_to_string(gate.join("private-source-stderr.log"))
                                        .unwrap_or_default()
                                )
                            },
                        ),
                    )
                    .unwrap();
                    if production_source {
                        assert_eq!(issued["status"], "registered");
                        assert_eq!(issued["exact_source_decision_id"], attribution.1);
                        assert_eq!(issued["exact_source_request_id"], attribution.0);
                        assert_eq!(issued["exact_source_projection_available"], false);
                    } else {
                        assert_eq!(issued["decision"], issued["retry"]);
                        assert_eq!(
                            issued["decision"]["issuer_kind"],
                            "consumed_h_sealed_helper"
                        );
                        assert_eq!(
                            issued["decision"]["registration_worker"]["host_pid"],
                            worker_pid
                        );
                        assert_eq!(
                            issued["decision"]["registration_sha256"],
                            format!("{:x}", Sha256::digest(&registration_bytes))
                        );
                        assert_eq!(
                            issued["decision"]["registration_device"],
                            registration_stat.dev()
                        );
                        assert_eq!(
                            issued["decision"]["registration_inode"],
                            registration_stat.ino()
                        );
                        assert_eq!(issued["negatives"].as_array().unwrap().len(), 5);
                    }
                    let decisions = rusqlite::Connection::open_with_flags(
                        broker_state.join("source-decisions/decisions.db"),
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap();
                    let count: i64 = decisions
                        .query_row("SELECT count(*) FROM decisions", [], |row| row.get(0))
                        .unwrap();
                    assert_eq!(count, 1, "lost reply/retry wrote another decision");
                    let capability: Vec<u8> = intent["registration_authority"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|byte| byte.as_u64().unwrap() as u8)
                        .collect();
                    let capability = String::from_utf8(capability).unwrap();
                    let stamp = |identity: &oulipoly_state::completion_continuation::SourceProcessIdentity| {
                        ProcessWitness { host_pid: identity.pid as i32,
                            boot_id: identity.boot_id.clone(),
                            starttime_ticks: identity.starttime_ticks as u64 }
                    };
                    let outsider_request = protocol::ExactSourceDecisionRequest {
                        request_id: attribution.0.clone(),
                        witness: protocol::SourceWitnessProbe {
                            owner: protocol::OwnerWitness {
                                root_id: prepared.root_id.clone(),
                                domain_id: released.owner.domain_id.clone(),
                                supervisor_id: released.owner.supervisor_authority_id.clone(),
                                guardian: stamp(&released.owner.guardian_identity),
                                driver: stamp(&released.owner.driver_identity),
                                owner_generation: Some(released.owner.owner_generation.clone()),
                                work_id: Some(work["work_id"].as_str().unwrap().into()),
                                owner_session_id: Some(session.clone()),
                                owner_invocation_uuid: Some(invocation.into()),
                                registration_authority_sha256: Some(format!(
                                    "{:x}",
                                    Sha256::digest(capability.as_bytes())
                                )),
                            },
                            source_generation: generation.clone(),
                            owner_generation: released.owner.owner_generation.clone(),
                            registration_path: registration_path.to_path_buf(),
                            registration_len: registration_bytes.len() as u64,
                            registration_sha256: format!(
                                "{:x}",
                                Sha256::digest(&registration_bytes)
                            ),
                            owner_session_id: session.clone(),
                            owner_invocation_uuid: invocation.into(),
                            capability,
                            accepted_intent_path: Some(h_dir.join("root-work-intent-v1.json")),
                        },
                    };
                    let source_fd = fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                        .open(registration_path)
                        .unwrap();
                    assert!(
                        protocol::issue_exact_source_decision_at(
                            &socket,
                            &outsider_request,
                            guardian.as_raw_fd(),
                            source_fd.as_raw_fd()
                        )
                        .is_err(),
                        "outside caller replayed the Bash decision with its exact file and strings"
                    );
                    let still_zero: i64 = original
                        .query_row(
                            "SELECT count(*) FROM invocation_completion_exact_source_decisions",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        still_zero, 1,
                        "actual Bash-byte decision did not commit to original State"
                    );
                }));
                fs::write(gate.join("source-release"), b"done").unwrap();
                eventually(|| entry.try_wait().unwrap().is_some());
                let _ = entry.wait();
                if let Err(panic) = verified {
                    stop(&mut broker);
                    unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                    std::panic::resume_unwind(panic);
                }
                assert!(
                    fs::read_to_string(&err)
                        .unwrap_or_default()
                        .contains("original H acceptance uncertain; no replay"),
                    "pre-register H response unexpectedly claimed completion"
                );
                if postcommit_source {
                    stop(&mut broker);
                    terminate_postcommit_original_owner(&broker_state);
                    let mut restarted = restart_source_broker(
                        &socket,
                        &broker_state,
                        &runner,
                        &gate,
                        &temp.path().join("real-h-w-q-post-owner-restart.log"),
                    );
                    let observed = std::panic::catch_unwind(|| {
                        assert_postcommit_source_physical_after_owner(
                            &socket,
                            &broker_state,
                            &gate,
                            bash.as_ref().unwrap(),
                            completed_v2_source,
                            wake_v2_source,
                            wake_lost,
                            wake_unknown,
                        );
                    });
                    stop(&mut restarted);
                    if let Err(panic) = observed {
                        unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                        std::panic::resume_unwind(panic);
                    }
                    if wake_v2_source {
                        let physical =
                            SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                                .unwrap();
                        let id = &physical.records()[0].grant.grant_id;
                        let before = BrokerSidecar::open_existing(
                            &broker_state.join("sidecar/pid-identity.db"),
                            &broker_state,
                        )
                        .unwrap()
                        .read_v2_recipient_grant(id)
                        .unwrap()
                        .unwrap();
                        let mut second = restart_source_broker(
                            &socket,
                            &broker_state,
                            &runner,
                            &gate,
                            &temp.path().join("real-h-w-q-recipient-restart.log"),
                        );
                        std::thread::sleep(Duration::from_millis(400));
                        let after = BrokerSidecar::open_existing(
                            &broker_state.join("sidecar/pid-identity.db"),
                            &broker_state,
                        )
                        .unwrap()
                        .read_v2_recipient_grant(id)
                        .unwrap()
                        .unwrap();
                        assert_eq!(after, before, "restart regranted or resent v2 row");
                        stop(&mut second);
                    }
                    unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                } else {
                    stop(&mut broker);
                    unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                }
                return;
            }
            if mode == "normal_handoff_bash_child" {
                let refusals: serde_json::Value =
                    serde_json::from_slice(&fs::read(gate.join("bash-child-refused")).unwrap())
                        .unwrap();
                assert!(
                    refusals["direct"]
                        .as_str()
                        .unwrap()
                        .contains("consumed causal parent work grant absent")
                );
                assert!(
                    refusals["grandchild"]
                        .as_str()
                        .unwrap()
                        .contains("consumed causal parent work grant absent")
                );
                assert!(!gate.join("bash-effect").exists());
                let fresh = rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                let count: i64 = fresh
                    .query_row("SELECT count(*) FROM fresh_bash_child", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 0);
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                eventually(|| entry.try_wait().unwrap().is_some());
                assert!(
                    entry.wait().unwrap().success(),
                    "{}",
                    fs::read_to_string(&err).unwrap()
                );
                stop(&mut broker);
                return;
            }
            if mode.starts_with("normal_model_provider_bash_ordinary") {
                if mode.contains("_ordinary_script") {
                    use std::os::unix::fs::MetadataExt;
                    fs::write(gate.join("child-effect"), b"yes").unwrap();
                    let replacing = mode.ends_with("_replace");
                    let removing = mode.ends_with("_remove");
                    let script_path = gate.join("script-bin/scriptcmd");
                    let original_inode = if replacing || removing {
                        eventually(|| gate.join("ordinary-paused").exists());
                        let inode = fs::metadata(&script_path).unwrap().ino();
                        if replacing {
                            let replacement = gate.join("script-bin/replacement");
                            fs::write(&replacement, "#!/bin/sh\nprintf 'new|%s|%s|%s|%s|%s\\n' \"$0\" \"$1\" \"$2\" \"$PWD\" \"$AGE319_ORDINARY_EFFECTIVE_ENV\"\nprintf new > \"$3\"\n").unwrap();
                            fs::set_permissions(
                                &replacement,
                                fs::metadata(&script_path).unwrap().permissions(),
                            )
                            .unwrap();
                            fs::rename(replacement, &script_path).unwrap();
                        } else {
                            fs::remove_file(&script_path).unwrap();
                        }
                        fs::write(gate.join("ordinary-release"), b"yes").unwrap();
                        inode
                    } else {
                        0
                    };
                    eventually(|| gate.join("ordinary-bash-status").exists());
                    assert_eq!(
                        fs::read_to_string(gate.join("ordinary-bash-status")).unwrap(),
                        "0",
                        "{}",
                        fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default()
                    );
                    let report: serde_json::Value =
                        serde_json::from_slice(&fs::read(gate.join("bash-causal-output")).unwrap())
                            .unwrap();
                    assert_eq!(report["schema_version"], 31);
                    assert_eq!(report["dispatch_state"], "sync-child-result");
                    assert_eq!(report["publication"]["phase"], "unknown");
                    let request_id = report["publication"]["child"]["request_id"]
                        .as_str()
                        .unwrap();
                    let grant_id = report["publication"]["event"]["physical_grant_id"]
                        .as_str()
                        .unwrap();
                    let directory = broker_state.join("v30/fresh-provider");
                    let selected: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            directory.join(format!("{request_id}.child-work-selection.json")),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(directory.join(format!("{request_id}.fresh-grant.json")))
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        selected["broker_resolved_path"],
                        script_path.display().to_string()
                    );
                    assert_eq!(selected["configured_program"], "scriptcmd");
                    assert_eq!(selected["path_execution"], true);
                    assert_eq!(selected["selected_source"]["observed_shebang"], true);
                    assert_eq!(grant["preflight_image"], selected["selected_source"]);
                    assert_eq!(grant["path_execution"], true);
                    if replacing || removing {
                        assert_eq!(selected["image_descriptor"]["inode"], original_inode);
                    }
                    if replacing {
                        assert_ne!(
                            grant["path_at_k"]["inode"],
                            selected["image_descriptor"]["inode"]
                        );
                    } else if removing {
                        assert!(grant["path_at_k"].is_null());
                    }
                    eventually(|| {
                        directory
                            .join(format!("{grant_id}.source-event.json"))
                            .exists()
                    });
                    let event: oulipoly_state::mailbox::FreshBashSourceEvent =
                        serde_json::from_slice(
                            &fs::read(directory.join(format!("{grant_id}.source-event.json")))
                                .unwrap(),
                        )
                        .unwrap();
                    assert!(event.tree_drained && event.output_closed);
                    assert!(directory.join(format!("{grant_id}.drain.json")).exists());
                    assert!(directory.join(format!("{grant_id}.consumed.json")).exists());
                    if removing {
                        assert_eq!(event.wait_status, 127 << 8);
                        assert_eq!(report["publication"]["exit_code"], 127);
                        assert!(!gate.join("ordinary-effect").exists());
                    } else {
                        assert_eq!(event.wait_status, 0);
                        let label = if replacing { "new" } else { "old" };
                        let expected = format!(
                            "{label}|{}|alpha|beta|{}|original-value\n",
                            script_path.display(),
                            std::env::current_dir().unwrap().display()
                        );
                        assert_eq!(
                            fs::read(directory.join(format!("{grant_id}.stdout"))).unwrap(),
                            expected.as_bytes()
                        );
                        use base64::Engine as _;
                        assert_eq!(
                            base64::engine::general_purpose::STANDARD
                                .decode(report["stdout_base64"].as_str().unwrap())
                                .unwrap(),
                            expected.as_bytes()
                        );
                        assert_eq!(
                            fs::read(gate.join("ordinary-effect")).unwrap(),
                            label.as_bytes()
                        );
                    }
                    let consumed = fs::read_dir(&directory)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json")
                        })
                        .count();
                    assert_eq!(consumed, 2, "script result replayed a physical K");
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    stop(&mut broker);
                    return;
                }
                if mode.ends_with("_parent_tamper") {
                    fs::write(gate.join("child-effect"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("ordinary-paused").exists() || entry.try_wait().unwrap().is_some()
                    });
                    let request_id = fs::read_to_string(gate.join("ordinary-paused")).unwrap();
                    let directory = broker_state.join("v30/fresh-provider");
                    let selected_path =
                        directory.join(format!("{request_id}.child-work-selection.json"));
                    let mut selected: serde_json::Value =
                        serde_json::from_slice(&fs::read(&selected_path).unwrap()).unwrap();
                    assert_eq!(selected["role"], "bash-child-ordinary-tree-v1");
                    selected["binding"]["causal_parent"]["grant_id"] =
                        uuid::Uuid::new_v4().to_string().into();
                    fs::write(&selected_path, serde_json::to_vec(&selected).unwrap()).unwrap();
                    fs::write(gate.join("ordinary-release"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("ordinary-bash-status").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    assert_ne!(
                        fs::read_to_string(gate.join("ordinary-bash-status")).unwrap(),
                        "0",
                        "changed parent selection reached ordinary K"
                    );
                    assert!(!gate.join("ordinary-effect").exists());
                    assert!(
                        !directory
                            .join(format!("{request_id}.fresh-grant.json"))
                            .exists()
                    );
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    stop(&mut broker);
                    return;
                }
                if mode.ends_with("_refuse") {
                    fs::write(gate.join("child-effect"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("ordinary-refuse-statuses").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    let statuses: Vec<(String, i32)> = serde_json::from_slice(
                        &fs::read(gate.join("ordinary-refuse-statuses")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(statuses.len(), 12);
                    assert!(
                        statuses[..7].iter().all(|(_, status)| *status != 0),
                        "{statuses:?}"
                    );
                    assert!(
                        statuses[7..].iter().all(|(_, status)| *status == 0),
                        "{statuses:?}"
                    );
                    assert_eq!(
                        fs::read(gate.join("ordinary-shebang-effect")).unwrap(),
                        b"effect"
                    );
                    assert_eq!(
                        fs::read(gate.join("ordinary-plain-effect")).unwrap(),
                        b"effect"
                    );
                    assert!(!gate.join("ordinary-refused-effect").exists());
                    let physical = broker_state.join("v30/fresh-provider");
                    for (case, code) in [
                        ("malformed-elf", 126),
                        ("missing-interp", 127),
                        ("non-executable", 126),
                    ] {
                        let report: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join(format!("ordinary-{case}-output"))).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(report["schema_version"], 31, "{case}");
                        assert_eq!(report["dispatch_state"], "sync-child-result", "{case}");
                        assert_eq!(report["publication"]["phase"], "unknown", "{case}");
                        assert_eq!(report["publication"]["exit_code"], code, "{case}");
                        let grant = report["publication"]["event"]["physical_grant_id"]
                            .as_str()
                            .unwrap();
                        let event: oulipoly_state::mailbox::FreshBashSourceEvent =
                            serde_json::from_slice(
                                &fs::read(physical.join(format!("{grant}.source-event.json")))
                                    .unwrap(),
                            )
                            .unwrap();
                        assert_eq!(event.wait_status, code << 8, "{case}");
                        assert!(physical.join(format!("{grant}.drain.json")).exists());
                    }
                    let child_grants = fs::read_dir(&physical)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".fresh-grant.json")
                        })
                        .count();
                    assert_eq!(
                        child_grants, 6,
                        "ordinary format outcomes missed or duplicated physical K"
                    );
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    stop(&mut broker);
                    return;
                }
                let asynchronous = mode.ends_with("_async") || mode.ends_with("_restart");
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                if mode.ends_with("_sync_tamper")
                    || mode.ends_with("_sync_post_tamper")
                    || mode.ends_with("_sync_encode_tamper")
                {
                    let post = mode.ends_with("_sync_post_tamper");
                    let encode = mode.ends_with("_sync_encode_tamper");
                    let marker = if post {
                        "sync-begin-paused"
                    } else if encode {
                        "sync-verify-paused"
                    } else {
                        "sync-paused"
                    };
                    eventually(|| gate.join(marker).exists());
                    let request_id = fs::read_to_string(gate.join(marker)).unwrap();
                    let fresh =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    let event_json: String = fresh.query_row(
                        "SELECT receipt_json FROM fresh_bash_selected_event WHERE request_id=?1",
                        [&request_id], |r| r.get(0),
                    ).unwrap();
                    let event: oulipoly_state::mailbox::FreshBashSourceEvent =
                        serde_json::from_str(&event_json).unwrap();
                    fs::write(
                        broker_state
                            .join("v30/fresh-provider")
                            .join(format!("{}.stdout", event.physical_grant_id)),
                        if encode {
                            &b"changedbyts"[..]
                        } else {
                            &b"changedbytes"[..]
                        },
                    )
                    .unwrap();
                    fs::write(
                        gate.join(if post {
                            "sync-begin-release"
                        } else if encode {
                            "sync-verify-release"
                        } else {
                            "sync-release"
                        }),
                        b"yes",
                    )
                    .unwrap();
                }
                eventually(|| {
                    gate.join("ordinary-bash-status").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                if mode.ends_with("_sync_partial")
                    || mode.ends_with("_sync_tamper")
                    || mode.ends_with("_sync_post_tamper")
                    || mode.ends_with("_sync_encode_tamper")
                    || mode.ends_with("_sync_w_debt")
                {
                    assert_ne!(
                        fs::read_to_string(gate.join("ordinary-bash-status")).unwrap(),
                        "0"
                    );
                    let fresh =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    let reserved: i64 = fresh
                        .query_row(
                            "SELECT count(*) FROM fresh_bash_sync_publication",
                            [],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        reserved,
                        if mode.ends_with("_sync_partial")
                            || mode.ends_with("_sync_post_tamper")
                            || mode.ends_with("_sync_encode_tamper")
                        {
                            1
                        } else {
                            0
                        }
                    );
                    if mode.ends_with("_sync_partial") || mode.ends_with("_sync_encode_tamper") {
                        assert!(
                            fs::read(gate.join("bash-causal-output"))
                                .unwrap()
                                .starts_with(b"{\"schema_version\":31,")
                        );
                    } else {
                        assert!(
                            fs::read(gate.join("bash-causal-output"))
                                .unwrap()
                                .is_empty()
                        );
                    }
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    stop(&mut broker);
                    return;
                }
                assert_eq!(
                    fs::read_to_string(gate.join("ordinary-bash-status")).unwrap_or_default(),
                    "0",
                    "ordinary Bash: {}; broker: {}; entry: {}",
                    fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default(),
                    fs::read_to_string(&err).unwrap_or_default(),
                );
                let report: serde_json::Value =
                    serde_json::from_slice(&fs::read(gate.join("bash-causal-output")).unwrap())
                        .unwrap();
                let request_id = if asynchronous {
                    report["request_id"].as_str().unwrap()
                } else {
                    report["publication"]["child"]["request_id"]
                        .as_str()
                        .unwrap()
                };
                let grant = if asynchronous {
                    report["physical_grant_id"].as_str().unwrap()
                } else {
                    report["publication"]["event"]["physical_grant_id"]
                        .as_str()
                        .unwrap()
                };
                if asynchronous {
                    assert_eq!(report["delivery_mode"], "async");
                    assert_eq!(report["completion_policy"], "tree");
                    assert!(report["handle"].as_str().unwrap().starts_with("ab30_"));
                    assert_eq!(report["effects_possible"], true);
                } else {
                    assert_eq!(report["schema_version"], 31);
                    assert_eq!(
                        report["dispatch_state"],
                        if mode.ends_with("_sync_reply_loss")
                            || mode.ends_with("_sync_socket_partial")
                        {
                            "sync-publication-unknown"
                        } else {
                            "sync-child-result"
                        }
                    );
                    assert_eq!(report["publication"]["phase"], "unknown");
                    if mode.ends_with("_sync_reply_loss") || mode.ends_with("_sync_socket_partial")
                    {
                        assert!(report.get("stdout_base64").is_none());
                        assert!(report.get("stderr_base64").is_none());
                    } else {
                        assert_eq!(report["stdout_encoding"], "base64");
                        assert_eq!(report["stderr_encoding"], "base64");
                    }
                    if !mode.ends_with("_cancel")
                        && !mode.ends_with("_sync_reply_loss")
                        && !mode.ends_with("_sync_socket_partial")
                        && !mode.ends_with("_sync_large")
                    {
                        assert_eq!(report["stdout_base64"], "Af9vcmRpbmFyeQA=");
                        assert_eq!(report["stderr_base64"], "ZXJyAP4=");
                    }
                    if mode.ends_with("_sync_large") {
                        use base64::Engine as _;
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(report["stdout_base64"].as_str().unwrap())
                            .unwrap();
                        assert_eq!(decoded.len(), 200011);
                        assert_eq!(&decoded[..11], b"\x01\xffordinary\x00");
                        assert!(decoded[11..].iter().all(|byte| *byte == 0));
                    }
                    if report["dispatch_state"] == "sync-child-result" {
                        use base64::Engine as _;
                        for stream in ["stdout", "stderr"] {
                            let decoded = base64::engine::general_purpose::STANDARD
                                .decode(report[format!("{stream}_base64")].as_str().unwrap())
                                .unwrap();
                            assert_eq!(
                                decoded.len() as u64,
                                report["publication"]["event"][format!("{stream}_len")]
                                    .as_u64()
                                    .unwrap()
                            );
                            assert_eq!(
                                format!("{:x}", Sha256::digest(&decoded)),
                                report["publication"]["event"][format!("{stream}_sha256")]
                            );
                        }
                    }
                    if mode.ends_with("_failure") {
                        assert_eq!(report["publication"]["outcome"], "exited");
                        assert_eq!(report["publication"]["exit_code"], 37);
                    } else if mode.ends_with("_sync_signal") {
                        assert_eq!(report["publication"]["outcome"], "signaled");
                        assert_eq!(report["publication"]["signal"], libc::SIGTERM);
                    } else if mode.ends_with("_cancel") {
                        assert_eq!(report["publication"]["outcome"], "cancelled");
                    }
                }
                let directory = broker_state.join("v30/fresh-provider");
                if !asynchronous && !mode.ends_with("_sync_parent_output") {
                    let parent_grant = report["publication"]["child"]["parent_work_grant_id"]
                        .as_str()
                        .unwrap();
                    assert!(
                        !directory
                            .join(format!("{parent_grant}.drain.json"))
                            .exists(),
                        "sync child response waited for parent Q"
                    );
                }
                let selected: serde_json::Value = serde_json::from_slice(
                    &fs::read(directory.join(format!("{request_id}.child-work-selection.json")))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(selected["role"], "bash-child-ordinary-tree-v1");
                let configured = if mode.ends_with("_elf") {
                    gate.join("ordinary-elf-image").display().to_string()
                } else {
                    "sh".into()
                };
                assert_eq!(
                    selected["configured_program"], configured,
                    "original argv0 changed"
                );
                assert_eq!(selected["child_request_id"], request_id);
                if mode.ends_with("_elf") {
                    use std::os::unix::fs::MetadataExt;
                    let image = gate.join("ordinary-elf-image");
                    let metadata = fs::metadata(&image).unwrap();
                    assert_eq!(selected["image_descriptor"]["inode"], metadata.ino());
                    assert_eq!(selected["image_descriptor"]["device"], metadata.dev());
                    let grant_record: serde_json::Value = serde_json::from_slice(
                        &fs::read(directory.join(format!("{request_id}.fresh-grant.json")))
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(grant_record["preflight_image"]["observed_shebang"], false);
                    assert!(
                        grant_record["preflight_image"]["observed_xattrs_sha256"]
                            .as_str()
                            .is_some()
                    );
                }
                let intent: serde_json::Value = serde_json::from_slice(
                    &fs::read(directory.join(format!("{request_id}.ordinary-bash-intent.json")))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(
                    intent["command_sha256"],
                    selected["ordinary_command_sha256"]
                );
                assert!(
                    intent.get("command").is_none(),
                    "effective environment persisted in C intent"
                );
                assert!(
                    !fs::read_to_string(
                        directory.join(format!("{request_id}.ordinary-bash-intent.json"))
                    )
                    .unwrap()
                    .contains("original-value")
                );
                for entry in fs::read_dir(&directory).unwrap().filter_map(Result::ok) {
                    if entry.file_type().unwrap().is_file() {
                        let bytes = fs::read(entry.path()).unwrap();
                        assert!(
                            !bytes
                                .windows(b"age319-secret-must-stay-in-memfd-319".len())
                                .any(|window| window == b"age319-secret-must-stay-in-memfd-319"),
                            "secret persisted in broker artifact {}",
                            entry.path().display()
                        );
                    }
                }
                if mode.ends_with("_restart") {
                    assert!(
                        directory.join(format!("{grant}.consumed.json")).exists(),
                        "ordinary child K was not durably consumed before broker stop"
                    );
                    let consumed_before = fs::read_dir(&directory)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json")
                        })
                        .count();
                    assert_eq!(consumed_before, 2, "expected parent and one child K");
                    assert!(
                        !directory.join(format!("{grant}.drain.json")).exists(),
                        "ordinary Q completed before restart boundary"
                    );
                    stop(&mut broker);
                    assert!(
                        !directory
                            .join(format!("{grant}.source-event.json"))
                            .exists(),
                        "ordinary W completed before restart boundary"
                    );
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                            bash.as_ref().unwrap(),
                        )
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .stdout(Stdio::null())
                        .stderr(Stdio::from(
                            File::create(temp.path().join("ordinary-broker-restart.log")).unwrap(),
                        ))
                        .spawn()
                        .unwrap();
                    eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                    let consumed_after = fs::read_dir(&directory)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json")
                        })
                        .count();
                    assert_eq!(
                        consumed_after, consumed_before,
                        "restart created a second K"
                    );
                }
                let source_event_path = directory.join(format!("{grant}.source-event.json"));
                let source_deadline = Instant::now() + Duration::from_secs(20);
                while !source_event_path.exists() && Instant::now() < source_deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
                let artifact_links = fs::read_dir(&directory)
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter_map(|entry| {
                        use std::os::unix::fs::MetadataExt;
                        let meta = entry.metadata().ok()?;
                        Some((
                            entry.file_name().to_string_lossy().into_owned(),
                            meta.ino(),
                            meta.nlink(),
                        ))
                    })
                    .collect::<Vec<_>>();
                assert!(
                    source_event_path.exists(),
                    "ordinary W absent: entry={} bash_status={} bash_error={} broker_before={} broker_after={} artifact_links={artifact_links:?}",
                    fs::read_to_string(&err).unwrap_or_default(),
                    fs::read_to_string(gate.join("ordinary-bash-status")).unwrap_or_default(),
                    fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default(),
                    fs::read_to_string(temp.path().join("ordinary-broker-restart.log"))
                        .unwrap_or_default(),
                );
                let event: oulipoly_state::mailbox::FreshBashSourceEvent = serde_json::from_slice(
                    &fs::read(directory.join(format!("{grant}.source-event.json"))).unwrap(),
                )
                .unwrap();
                assert_eq!(event.request_id, request_id);
                assert_eq!(event.physical_grant_id, grant);
                assert!(event.tree_drained && event.output_closed);
                if mode.ends_with("_cancel") {
                    assert!(event.cancelled && event.selected_kind == "cancelled");
                    assert_eq!(event.cancel_grant_id.as_deref(), Some(grant));
                    assert!(!gate.join("ordinary-background").exists());
                } else {
                    assert!(!event.cancelled);
                    if mode.ends_with("_sync_large") {
                        assert_eq!(
                            fs::metadata(directory.join(format!("{grant}.stdout")))
                                .unwrap()
                                .len(),
                            200011
                        );
                    } else {
                        assert_eq!(
                            fs::read(directory.join(format!("{grant}.stdout"))).unwrap(),
                            b"\x01\xffordinary\x00"
                        );
                    }
                    assert_eq!(
                        fs::read(directory.join(format!("{grant}.stderr"))).unwrap(),
                        b"err\x00\xfe"
                    );
                    assert_eq!(fs::read(gate.join("ordinary-effect")).unwrap(), b"effect");
                    assert_eq!(
                        fs::read(gate.join("ordinary-background")).unwrap(),
                        b"background"
                    );
                }
                if mode.ends_with("_failure") {
                    assert_eq!(event.wait_status, 37 << 8);
                }
                let fresh = rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                eventually(|| {
                    fresh
                        .query_row("SELECT count(*) FROM fresh_bash_selected_event", [], |r| {
                            r.get::<_, i64>(0)
                        })
                        .is_ok_and(|count| count == 1)
                });
                if asynchronous {
                    eventually(|| {
                        fresh
                            .query_row("SELECT count(*) FROM fresh_bash_notify_request", [], |r| {
                                r.get::<_, i64>(0)
                            })
                            .is_ok_and(|count| count == 1)
                    });
                }
                let notify_count: i64 = fresh
                    .query_row("SELECT count(*) FROM fresh_bash_notify_request", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                assert_eq!(notify_count, if asynchronous { 1 } else { 0 });
                let sync_publications: i64 = fresh
                    .query_row(
                        "SELECT count(*) FROM fresh_bash_sync_publication",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(sync_publications, if asynchronous { 0 } else { 1 });
                if !asynchronous {
                    let stored: String = fresh.query_row(
                        "SELECT receipt_json FROM fresh_bash_sync_publication WHERE request_id=?1",
                        [request_id], |r| r.get(0),
                    ).unwrap();
                    let stored: serde_json::Value = serde_json::from_str(&stored).unwrap();
                    assert_eq!(stored, report["publication"]);
                    let consumed = fs::read_dir(&directory)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json")
                        })
                        .count();
                    assert_eq!(consumed, 2, "sync response created a second K");
                    let root_publications: i64 = fresh
                        .query_row("SELECT count(*) FROM fresh_root_publication", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    if !mode.ends_with("_sync_parent_output") {
                        assert_eq!(
                            root_publications, 0,
                            "child response created root publication"
                        );
                    }
                }
                if mode.ends_with("_copy") {
                    eventually(|| gate.join("ordinary-copy-status").exists());
                    assert_eq!(
                        fs::read_to_string(gate.join("ordinary-sibling-status")).unwrap(),
                        "0",
                        "{}",
                        fs::read_to_string(gate.join("ordinary-sibling-error")).unwrap_or_default()
                    );
                    assert_ne!(
                        fs::read_to_string(gate.join("ordinary-copy-status")).unwrap(),
                        "0"
                    );
                    let grant_count = fs::read_dir(&directory)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".fresh-grant.json")
                        })
                        .count();
                    assert_eq!(grant_count, 2, "copied request launched second child work");
                }
                if asynchronous {
                    let side = rusqlite::Connection::open(
                        broker_state.join("v30/sidecar/pid-identity.db"),
                    )
                    .unwrap();
                    eventually(|| {
                        side.query_row(
                            "SELECT count(*) FROM mailbox WHERE handle=?1 AND delivered_at IS NULL",
                            [report["handle"].as_str().unwrap()],
                            |r| r.get::<_, i64>(0),
                        )
                        .is_ok_and(|count| count == 1)
                    });
                } else {
                    assert_old_debt_and_no_f_ack(&broker_state);
                }
                fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                eventually(|| entry.try_wait().unwrap().is_some());
                if mode.ends_with("_sync") || mode.ends_with("_sync_parent_output") {
                    let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let (root, actor) = lane.released_handoff_for_root(&prepared.root_id).unwrap();
                    let session = lane.read_session(&root.d_key).unwrap().unwrap();
                    let terminal = lane
                        .settle_private_root_terminal(&root, &actor, &session)
                        .unwrap();
                    assert_eq!(
                        terminal.execution.as_ref().unwrap().child_event,
                        Some(event.clone())
                    );
                    assert_eq!(
                        terminal.publication_state,
                        if caller_mode {
                            "settled"
                        } else {
                            "not_started"
                        },
                        "parent caller error: {}; broker: {}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default()
                    );
                    let parent = &terminal.execution.as_ref().unwrap().parent;
                    if caller_mode {
                        assert!(gate.join("caller-control-stdout").exists());
                        assert!(gate.join("caller-control-stderr").exists());
                        assert_eq!(
                            fs::read(&out).unwrap(),
                            fs::read(directory.join(format!("{}.stdout", parent.grant_id)))
                                .unwrap()
                        );
                        assert_eq!(
                            fs::read(&err).unwrap(),
                            fs::read(directory.join(format!("{}.stderr", parent.grant_id)))
                                .unwrap()
                        );
                    }
                    let offered = oulipoly_state::mailbox::FreshRootCallerResult {
                        parent_grant_id: parent.grant_id.clone(),
                        wait_status: parent.wait_status,
                        stdout_sha256: parent.stdout_sha256.clone(),
                        stdout_len: parent.stdout_len,
                        stderr_sha256: parent.stderr_sha256.clone(),
                        stderr_len: parent.stderr_len,
                        native_f: None,
                    };
                    assert_ne!(offered.stdout_sha256, event.stdout_sha256);
                    if !caller_mode {
                        assert_eq!(
                            lane.begin_private_root_caller_result(
                                &root, &actor, &session, &offered
                            )
                            .unwrap_err(),
                            "caller result terminal was cancelled"
                        );
                        stop(&mut broker);
                        return;
                    }
                    let parent_publication = lane
                        .begin_private_root_caller_result(&root, &actor, &session, &offered)
                        .unwrap();
                    assert_eq!(parent_publication.publication_state, "settled");
                    assert_eq!(
                        parent_publication.execution.as_ref().unwrap().child_event,
                        Some(event.clone())
                    );
                    let fresh =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    let child_phase: String = fresh
                        .query_row(
                            "SELECT json_extract(receipt_json, '$.phase') FROM fresh_bash_sync_publication WHERE request_id=?1",
                            [request_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    let parent_phase: String = fresh
                        .query_row(
                            "SELECT phase FROM fresh_root_publication WHERE handoff_id=?1",
                            [&root.handoff_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        (child_phase.as_str(), parent_phase.as_str()),
                        ("unknown", "unknown")
                    );
                    let mut child_as_parent = offered.clone();
                    child_as_parent.stdout_sha256 = event.stdout_sha256.clone();
                    assert_eq!(
                        lane.begin_private_root_caller_result(
                            &root,
                            &actor,
                            &session,
                            &child_as_parent
                        )
                        .unwrap_err(),
                        "caller result differs from verified parent Q"
                    );
                    assert_eq!(
                        lane.read_private_root_terminal(&root, &actor, &session)
                            .unwrap()
                            .publication_state,
                        "settled"
                    );
                }
                stop(&mut broker);
                return;
            }
            if mode.starts_with("normal_model_provider_bash_causal") {
                let success_source = mode.ends_with("_success") || root_h_delegate;
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                if root_h_notify {
                    let until = Instant::now() + Duration::from_secs(90);
                    while !gate.join("source-postcommit-ready").exists()
                        && entry.try_wait().unwrap().is_none()
                        && Instant::now() < until
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    assert!(
                        gate.join("source-postcommit-ready").exists(),
                        "selected-K original W did not reach postcommit: entry={} broker={} bash={} guardian={} driver={}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                        fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default(),
                        fs::read_to_string(gate.join("source-guardian-postcommit-stage"))
                            .unwrap_or_default(),
                        fs::read_to_string(gate.join("source-driver-postcommit-stage"))
                            .unwrap_or_default()
                    );
                    assert!(
                        SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                            .unwrap()
                            .records()
                            .is_empty(),
                        "pending original W already spent source Q"
                    );
                    let pending_rows: i64 = rusqlite::Connection::open_with_flags(
                        broker_state.join("sidecar/pid-identity.db"),
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap()
                    .query_row(
                        "SELECT count(*) FROM mailbox WHERE session_id!='old-pending'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                    assert_eq!(pending_rows, 0, "pending original W created a row");
                    fs::write(gate.join("source-postcommit-notify"), b"schedule").unwrap();
                }
                if mode.ends_with("_w_debt") {
                    eventually(|| {
                        fs::read_to_string(gate.join("bash-causal-error"))
                            .is_ok_and(|error| error.contains("source W captured before State"))
                    });
                    let physical_dir = broker_state.join("v30/fresh-provider");
                    let captured = fs::read_dir(&physical_dir)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".source-event.json")
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(captured.len(), 1, "one immutable captured event debt");
                    let event: oulipoly_state::mailbox::FreshBashSourceEvent =
                        serde_json::from_slice(&fs::read(captured[0].path()).unwrap()).unwrap();
                    let fresh =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    assert_eq!(
                        fresh
                            .query_row("SELECT count(*) FROM fresh_lane_accepted_source", [], |r| {
                                r.get::<_, i64>(0)
                            })
                            .unwrap(),
                        0
                    );
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    stop(&mut broker);
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                            bash.as_ref().unwrap(),
                        )
                        .stderr(Stdio::from(
                            File::create(temp.path().join("source-w-repair.log")).unwrap(),
                        ))
                        .spawn()
                        .unwrap();
                    eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                    eventually(|| {
                        fresh
                            .query_row("SELECT count(*) FROM fresh_lane_accepted_source", [], |r| {
                                r.get::<_, i64>(0)
                            })
                            .unwrap()
                            == 1
                    });
                    let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    lane.accept_private_bash_source(&event).unwrap();
                    assert_eq!(
                        fs::read(physical_dir.join(format!("{}.stdout", event.physical_grant_id)))
                            .unwrap(),
                        b"broker-child-output\n"
                    );
                    assert_eq!(
                        fs::read(data.join("pid-identity.db")).unwrap(),
                        b"retired copied owner"
                    );
                    let notify_from_c = mode.contains("_notify_");
                    if notify_from_c {
                        eventually(|| {
                            fresh
                                .query_row(
                                    "SELECT count(*) FROM fresh_lane_recipient_attachment",
                                    [],
                                    |r| r.get::<_, i64>(0),
                                )
                                .is_ok_and(|count| count == 1)
                        });
                    }
                    assert_eq!(
                        fresh
                            .query_row(
                                "SELECT count(*) FROM fresh_lane_recipient_attachment",
                                [],
                                |r| r.get::<_, i64>(0)
                            )
                            .unwrap(),
                        if notify_from_c { 1 } else { 0 }
                    );
                    if notify_from_c {
                        let side = rusqlite::Connection::open(
                            broker_state.join("v30/sidecar/pid-identity.db"),
                        )
                        .unwrap();
                        eventually(|| {
                            side.query_row(
                            "SELECT count(*) FROM mailbox WHERE handle=?1 AND delivered_at IS NULL",
                            [&event.source_id], |r| r.get::<_, i64>(0))
                            .is_ok_and(|count| count == 1)
                        });
                        let pending: i64 = side.query_row(
                            "SELECT count(*) FROM mailbox WHERE handle=?1 AND delivered_at IS NULL",
                            [&event.source_id], |r| r.get(0)).unwrap();
                        assert_eq!(pending, 1, "repaired async W did not retain pending F");
                    } else {
                        assert_old_debt_and_no_f_ack(&broker_state);
                    }
                    stop(&mut broker);
                    return;
                }
                let until =
                    Instant::now() + Duration::from_secs(if root_h_delegate { 90 } else { 20 });
                while fs::read(gate.join("bash-causal-output"))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                    .is_none()
                    && fs::read(gate.join("bash-causal-terminal-status"))
                        .ok()
                        .as_deref()
                        != Some(b"70")
                    && entry.try_wait().unwrap().is_none()
                    && Instant::now() < until
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(
                    gate.join("bash-causal-output").exists(),
                    "causal Bash did not launch; entry={} broker={} bash={}",
                    fs::read_to_string(&err).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default(),
                    fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default()
                );
                if root_h_delegate && !root_h_lost {
                    // The accepted original H must carry the consumed ticket
                    // and selected-K child through W preparation.
                    let h_source_until = Instant::now() + Duration::from_secs(20);
                    while !gate.join("h-source-intent-dir").exists()
                        && Instant::now() < h_source_until
                    {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    assert!(
                        gate.join("h-source-intent-dir").exists(),
                        "delegated H source absent: gate={:?} bash={} entry={} broker={}",
                        fs::read_dir(&gate)
                            .unwrap()
                            .flatten()
                            .map(|e| e.file_name())
                            .collect::<Vec<_>>(),
                        fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default(),
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default()
                    );
                    let intent_dir = std::path::PathBuf::from(
                        fs::read_to_string(gate.join("h-source-intent-dir")).unwrap(),
                    );
                    eventually(|| intent_dir.join("root-work-accepted-v1.json").exists());
                    let accepted: serde_json::Value = serde_json::from_slice(
                        &fs::read(intent_dir.join("root-work-accepted-v1.json")).unwrap(),
                    )
                    .unwrap();
                    let root_id = accepted["root_id"].as_str().unwrap();
                    let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let (root, actor) = lane.released_handoff_for_root(root_id).unwrap();
                    let delegation = lane
                        .read_root_h_delegation(&root.handoff_id)
                        .unwrap()
                        .unwrap();
                    assert_eq!(
                        root.delegated_h_listener_policy.as_deref(),
                        Some(if root_h_notify {
                            "notify"
                        } else {
                            "response_only"
                        })
                    );
                    let mut child = lane
                        .read_bash_child(&delegation.child_request_id)
                        .unwrap()
                        .unwrap();
                    child.session = lane.read_session(&child.d_key).unwrap().unwrap();
                    assert_eq!(delegation.child_actor, child.actor);
                    assert_eq!(
                        delegation.listener_policy,
                        if root_h_notify {
                            "notify"
                        } else {
                            "response_only"
                        }
                    );
                    assert_eq!(delegation.root_invocation_uuid, root.invocation_uuid);
                    assert_eq!(accepted["owner_generation"], delegation.owner_generation);
                    assert_eq!(accepted["initiator"]["pid"], child.actor.host_pid);
                    assert_eq!(accepted["registration"]["kind"], "root");
                    assert_eq!(accepted["delegated_root_h"]["work_id"], accepted["work_id"]);
                    assert_eq!(
                        accepted["delegated_root_h"]["witness"]["delegated_root_h_request_id"],
                        child.request_id
                    );
                    lane.require_consumed_root_h_delegation(
                        &root,
                        &actor,
                        &child,
                        &delegation.selected_k,
                    )
                    .unwrap();
                    let mut wrong_child = child.clone();
                    wrong_child.actor.starttime_ticks += 1;
                    assert!(
                        lane.require_consumed_root_h_delegation(
                            &root,
                            &actor,
                            &wrong_child,
                            &delegation.selected_k,
                        )
                        .is_err()
                    );
                    let mut wrong_plan = delegation.selected_k.clone();
                    wrong_plan.plan_sha256 = "0".repeat(64);
                    assert!(
                        lane.require_consumed_root_h_delegation(&root, &actor, &child, &wrong_plan)
                            .is_err()
                    );
                    let mut wrong_session = root.clone();
                    wrong_session.d_key = uuid::Uuid::new_v4().to_string();
                    assert!(
                        lane.require_consumed_root_h_delegation(
                            &wrong_session,
                            &actor,
                            &child,
                            &delegation.selected_k,
                        )
                        .is_err()
                    );
                    let mut stale_owner = root.clone();
                    stale_owner.old_release.prepared.owner_generation =
                        uuid::Uuid::new_v4().to_string();
                    assert!(
                        lane.require_consumed_root_h_delegation(
                            &stale_owner,
                            &actor,
                            &child,
                            &delegation.selected_k,
                        )
                        .is_err()
                    );
                    let mut wrong_policy = root.clone();
                    wrong_policy.delegated_h_listener_policy = Some(
                        if root_h_notify {
                            "response_only"
                        } else {
                            "notify"
                        }
                        .into(),
                    );
                    assert!(
                        lane.require_consumed_root_h_delegation(
                            &wrong_policy,
                            &actor,
                            &child,
                            &delegation.selected_k,
                        )
                        .is_err(),
                        "late policy change acquired consumed H"
                    );
                    eventually(|| {
                        fs::read_dir(broker_state.join("grants"))
                            .unwrap()
                            .flatten()
                            .any(|entry| {
                                fs::read(entry.path())
                                    .ok()
                                    .and_then(|bytes| {
                                        serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                                    })
                                    .is_some_and(|grant| grant["work_id"] == accepted["work_id"])
                            })
                    });
                }
                let report: serde_json::Value = serde_json::from_slice(
                    &fs::read(gate.join("bash-causal-output")).unwrap(),
                )
                .unwrap_or_else(|error| {
                    if root_h_delegate {
                        let state_dir = std::path::PathBuf::from(fs::read_to_string(gate.join("h-source-intent-dir")).unwrap_or_default());
                        for entry in fs::read_dir(&state_dir).ok().into_iter().flatten().flatten() {
                            if entry.file_name().to_string_lossy().ends_with(".stderr") || entry.file_name().to_string_lossy().ends_with(".stdout") {
                                eprintln!("delegated H helper {}: {}", entry.path().display(), fs::read_to_string(entry.path()).unwrap_or_default());
                            }
                        }
                        eprintln!("delegated H worker meta={} rc={} grant={} files={:?}",
                            fs::read_to_string(state_dir.join("meta.json")).unwrap_or_default(),
                            fs::read_to_string(state_dir.join("rc")).unwrap_or_default(),
                            fs::read_to_string(state_dir.join("root-work-broker-grant-v1.json")).unwrap_or_default(),
                            fs::read_dir(&state_dir).ok().into_iter().flatten().filter_map(Result::ok).map(|e| e.file_name()).collect::<Vec<_>>());
                    }
                    let output = fs::read(gate.join("bash-causal-output")).unwrap_or_default();
                    let prefix = if output.starts_with(b"{") { "json-object" } else { "other-or-empty" };
                    panic!(
                        "causal Bash: {error}; stdout_bytes={} stdout_newline={} stdout_prefix={prefix} terminal_intent={} effect_marker={} h_ready={} h_client_stage={} h_broker_stage={} h_guardian_stage={} guardian_wchan={} guardian_env={} h_intent={} h_diagnostic={} h_accepted={} stderr: {}; entry: {}; broker: {}; helper: {}",
                        output.len(),
                        output.ends_with(b"\n"),
                        fs::read_to_string(gate.join("bash-causal-terminal-status")).unwrap_or_else(|_| "absent".into()),
                        gate.join("bash-effect").exists(),
                        gate.join("h-source-ready").exists(),
                        fs::read_to_string(gate.join("delegated-h-client-stage")).unwrap_or_default(),
                        fs::read_to_string(gate.join("delegated-h-broker-stage")).unwrap_or_default(),
                        fs::read_to_string(gate.join("delegated-h-guardian-stage")).unwrap_or_default(),
                        fs::read_to_string(format!("/proc/{}/wchan", prepared.guardian.host_pid)).unwrap_or_default(),
                        String::from_utf8_lossy(&fs::read(format!("/proc/{}/environ", prepared.guardian.host_pid)).unwrap_or_default())
                            .split('\0').filter(|item| item.contains("FIXTURE_GATE_DIR") || item.contains("SELECTED_K_ROOT_H"))
                            .collect::<Vec<_>>().join(","),
                        fs::read_to_string(gate.join("h-source-intent-dir")).unwrap_or_default(),
                        fs::read_to_string(
                            std::path::PathBuf::from(fs::read_to_string(gate.join("h-source-intent-dir")).unwrap_or_default())
                                .join("root-work-diagnostic-v1.jsonl")
                        ).unwrap_or_default(),
                        fs::read_to_string(
                            std::path::PathBuf::from(fs::read_to_string(gate.join("h-source-intent-dir")).unwrap_or_default())
                                .join("root-work-accepted-v1.json")
                        ).unwrap_or_default(),
                        fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default(),
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                        fs::read_to_string(gate.join("causal-helper-error")).unwrap_or_default()
                    )
                });
                let terminal_deadline = Instant::now() + Duration::from_secs(20);
                while !gate.join("bash-causal-terminal-status").exists()
                    && Instant::now() < terminal_deadline
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(
                    gate.join("bash-causal-terminal-status").exists(),
                    "causal Bash terminal status absent: bash={} helper={} entry={} broker={}",
                    fs::read_to_string(gate.join("bash-causal-error")).unwrap_or_default(),
                    fs::read_to_string(gate.join("causal-helper-error")).unwrap_or_default(),
                    fs::read_to_string(&err).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default(),
                );
                assert_eq!(
                    fs::read(gate.join("bash-causal-terminal-status")).unwrap(),
                    b"0"
                );
                let child: oulipoly_state::mailbox::FreshBashChild =
                    serde_json::from_value(report["child"].clone()).unwrap();
                if root_h_delegate {
                    let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let delegation: oulipoly_state::mailbox::FreshRootHDelegation = if root_h_lost {
                        assert!(report["root_h_consumption"].is_null());
                        let (root, _) = lane.released_handoff_for_root(&child.root_id).unwrap();
                        lane.read_root_h_delegation(&root.handoff_id)
                            .unwrap()
                            .unwrap()
                    } else {
                        serde_json::from_value(report["root_h_consumption"].clone()).unwrap()
                    };
                    assert_eq!(delegation.child_request_id, child.request_id);
                    assert_eq!(delegation.root_id, child.root_id);
                    assert_eq!(
                        delegation.root_invocation_uuid,
                        child.parent_invocation_uuid
                    );
                    assert_eq!(delegation.selected_k.grant_id, child.parent_work_grant_id);
                    assert_eq!(delegation.selected_k.work_id, child.parent_work_id);
                    assert_eq!(
                        delegation.listener_policy,
                        if root_h_notify {
                            "notify"
                        } else {
                            "response_only"
                        }
                    );
                    assert_eq!(
                        lane.read_root_h_delegation(&delegation.handoff_id).unwrap(),
                        Some(delegation)
                    );
                    let state =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    let (issued, consumed): (i64, i64) = (
                        state
                            .query_row("SELECT count(*) FROM fresh_root_h_delegation", [], |r| {
                                r.get(0)
                            })
                            .unwrap(),
                        state
                            .query_row("SELECT count(*) FROM fresh_root_h_consumption", [], |r| {
                                r.get(0)
                            })
                            .unwrap(),
                    );
                    assert_eq!((issued, consumed), (1, 1));
                    assert_eq!(
                        state
                            .query_row(
                                "SELECT count(*) FROM invocation_completion_v2_identity",
                                [],
                                |r| r.get::<_, i64>(0)
                            )
                            .unwrap(),
                        0
                    );
                }
                assert_eq!(
                    child.listener_policy,
                    if mode.contains("_notify_") {
                        "notify"
                    } else {
                        "response_only"
                    }
                );
                let result: oulipoly_state::mailbox::FreshBashPrivateResult =
                    serde_json::from_value(report["bash_reported_result"].clone()).unwrap();
                assert_eq!(report["result_provenance"], "bash-self-report-only");
                let source: oulipoly_state::mailbox::FreshBashSourceEvent =
                    serde_json::from_value(report["fresh_source_w"].clone()).unwrap();
                assert_eq!(source.request_id, child.request_id);
                assert_eq!(source.source_id, child.handle);
                assert_eq!(source.attempt_id, child.invocation_uuid);
                assert_eq!(source.completion_policy, "tree");
                assert_eq!(
                    source.selected_kind,
                    if success_source {
                        "tree_drained"
                    } else {
                        "cancelled"
                    }
                );
                assert_eq!(source.cancelled, !success_source);
                assert!(source.tree_drained && source.output_closed);
                assert_eq!(
                    source.stdout_sha256,
                    format!("{:x}", Sha256::digest(b"broker-child-output\n"))
                );
                assert_eq!(source.stdout_len, b"broker-child-output\n".len() as u64);
                assert_ne!(
                    result.stdout_sha256, source.stdout_sha256,
                    "Bash O is separate from physical Q"
                );
                {
                    use oulipoly_state::mailbox::FreshBashListenerPolicy;
                    let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let original = if child.listener_policy == "notify" {
                        FreshBashListenerPolicy::Notify
                    } else {
                        FreshBashListenerPolicy::ResponseOnly
                    };
                    lane.register_private_bash_listener(&child, original)
                        .unwrap();
                    assert!(
                        lane.register_private_bash_listener(
                            &child,
                            if original == FreshBashListenerPolicy::Notify {
                                FreshBashListenerPolicy::ResponseOnly
                            } else {
                                FreshBashListenerPolicy::Notify
                            }
                        )
                        .is_err(),
                        "duplicate C changed original listener policy"
                    );
                    let fresh =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    let notify_count: i64 = fresh
                        .query_row("SELECT count(*) FROM fresh_bash_notify_request", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    assert_eq!(
                        notify_count,
                        if mode.contains("_notify_") { 1 } else { 0 },
                        "later policy inference retracted or invented F debt"
                    );
                }
                if mode.ends_with("notify_debt") || mode.ends_with("notify_row_debt") {
                    let fresh =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    eventually(|| {
                        fresh
                            .query_row("SELECT count(*) FROM fresh_bash_notify_request", [], |r| {
                                r.get::<_, i64>(0)
                            })
                            .unwrap()
                            == 1
                    });
                    assert_eq!(
                        fresh
                            .query_row(
                                "SELECT count(*) FROM fresh_lane_recipient_attachment",
                                [],
                                |r| r.get::<_, i64>(0)
                            )
                            .unwrap(),
                        1
                    );
                    eventually(|| entry.try_wait().unwrap().is_some());
                    let side = rusqlite::Connection::open(
                        broker_state.join("v30/sidecar/pid-identity.db"),
                    )
                    .unwrap();
                    assert_eq!(
                        side.query_row(
                            "SELECT count(*) FROM fresh_recipient_row_source",
                            [],
                            |r| r.get::<_, i64>(0)
                        )
                        .unwrap(),
                        0
                    );
                    let rows: i64 = side
                        .query_row(
                            "SELECT count(*) FROM mailbox WHERE handle=?1",
                            [&source.source_id],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        rows,
                        if mode.ends_with("notify_row_debt") {
                            1
                        } else {
                            0
                        }
                    );
                    let partial_lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let (partial_root, partial_actor) = partial_lane
                        .released_handoff_for_root(&source.root_id)
                        .unwrap();
                    let partial_session = partial_lane
                        .read_session(&partial_root.d_key)
                        .unwrap()
                        .unwrap();
                    let partial_terminal = partial_lane
                        .read_private_root_terminal(&partial_root, &partial_actor, &partial_session)
                        .unwrap();
                    assert_eq!(partial_terminal.execution_state, "unknown");
                    assert_eq!(partial_terminal.notification_state, "repair_required");
                    assert_eq!(
                        partial_terminal.unknown_stage.as_deref(),
                        Some("notify_sidecar_row_absent_reconcile_required")
                    );
                    assert!(
                        partial_terminal
                            .unknown_stages
                            .iter()
                            .any(|stage| stage.starts_with("parent_k_q:"))
                    );
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    stop(&mut broker);
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                            bash.as_ref().unwrap(),
                        )
                        .stderr(Stdio::from(
                            File::create(temp.path().join("notify-repair.log")).unwrap(),
                        ))
                        .spawn()
                        .unwrap();
                    eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                    eventually(|| {
                        side.query_row("SELECT count(*) FROM fresh_recipient_row_source", [], |r| {
                            r.get::<_, i64>(0)
                        })
                        .unwrap()
                            == 1
                    });
                    let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let root = lane.released_handoff_for_root(&source.root_id).unwrap().0;
                    let root_session = lane.read_session(&root.d_key).unwrap().unwrap();
                    let repaired_terminal = lane
                        .read_private_root_terminal(&partial_root, &partial_actor, &partial_session)
                        .unwrap();
                    assert_eq!(repaired_terminal.notification_state, "pending_f");
                    assert!(repaired_terminal.delivery_grant_id.is_none());
                    let seq: i64 = side
                        .query_row("SELECT seq FROM fresh_recipient_row_source", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    let payload = lane
                        .lookup_payload(&lane.identity().lane_id, &root_session.session_id, seq)
                        .unwrap();
                    assert!(String::from_utf8_lossy(&payload).contains("fresh-bash-complete-v30"));
                    assert_eq!(
                        side.query_row("SELECT count(*) FROM fresh_recipient_grant", [], |r| r
                            .get::<_, i64>(0))
                            .unwrap(),
                        0
                    );
                    let old =
                        rusqlite::Connection::open(broker_state.join("sidecar/pid-identity.db"))
                            .unwrap();
                    assert_eq!(old.query_row("SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND delivered_at IS NULL",[],|r|r.get::<_,i64>(0)).unwrap(),1);
                    stop(&mut broker);
                    return;
                }
                assert!(
                    report["broker_physical_q"]
                        .as_str()
                        .is_some_and(|q| q.starts_with("fresh-bash-physical-drained ")),
                    "broker child physical Q absent from Bash probe: {report}"
                );
                let physical_fields: Vec<_> = report["broker_physical_q"]
                    .as_str()
                    .unwrap()
                    .split_ascii_whitespace()
                    .collect();
                assert_eq!(source.physical_grant_id, physical_fields[1]);
                assert_eq!(physical_fields[2], "0");
                assert_eq!(
                    physical_fields[5],
                    if success_source { "false" } else { "true" }
                );
                assert_eq!(
                    fs::read(gate.join("bash-physical-effect")).unwrap(),
                    b"broker-ran\n"
                );
                assert!(
                    physical_dir
                        .join(format!("{}.source-event.json", source.physical_grant_id))
                        .exists()
                );
                assert_eq!(
                    fs::read(physical_dir.join(format!("{}.stdout", source.physical_grant_id)))
                        .unwrap(),
                    b"broker-child-output\n"
                );
                let root: oulipoly_state::mailbox::FreshReleasedHandoff = serde_json::from_slice(
                    &fs::read(
                        broker_state
                            .join("released-handoffs")
                            .join(format!("{}.json", prepared.root_id)),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(child.request_id, bash_request);
                assert_eq!(child.parent_invocation_uuid, root.invocation_uuid);
                let parent_grant: serde_json::Value = serde_json::from_slice(
                    &fs::read(physical_dir.join(format!("{}.fresh-grant.json", root.handoff_id)))
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(child.parent_work_grant_id, parent_grant["id"]);
                assert_ne!(child.actor.pidns_ino, prepared.root_init.pidns_ino);
                let parent_attach: serde_json::Value = serde_json::from_slice(
                    &fs::read(
                        physical_dir.join(format!("{}.attach.json", child.parent_work_grant_id)),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(child.parent_work_id, parent_attach["work_id"]);
                let child_selection: serde_json::Value = serde_json::from_slice(
                    &fs::read(
                        physical_dir
                            .join(format!("{}.child-work-selection.json", child.request_id)),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(child_selection["role"], "bash-child-private-fixed-v1");
                assert_eq!(child_selection["child_d_key"], child.d_key);
                assert_eq!(child_selection["child_request_id"], child.request_id);
                assert_eq!(
                    child_selection["binding"]["causal_parent"]["grant_id"],
                    child.parent_work_grant_id
                );
                assert_eq!(
                    child_selection["binding"]["causal_parent"]["work_id"],
                    child.parent_work_id
                );
                assert_eq!(
                    child_selection["child_receipt_sha256"],
                    format!("{:x}", Sha256::digest(serde_json::to_vec(&child).unwrap()))
                );
                let root_selection: serde_json::Value = serde_json::from_slice(
                    &fs::read(
                        physical_dir.join(format!("{}.route-selection.json", root.handoff_id)),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_ne!(
                    child_selection["plan_sha256"],
                    root_selection["selection"]["plan_sha256"]
                );
                assert_eq!(fs::read(gate.join("causal-env-count")).unwrap(), b"0");
                assert_eq!(fs::read(gate.join("causal-fd-clear")).unwrap(), b"closed");
                let keyring_result =
                    fs::read_to_string(gate.join("causal-keyring-result")).unwrap();
                assert_eq!(keyring_result, "joined-empty-session-keyring");
                let intermediary_pid =
                    fs::read_to_string(gate.join("causal-intermediary-proc-pid")).unwrap();
                assert_ne!(
                    report["parent_pid_before_c"],
                    intermediary_pid.parse::<u32>().unwrap(),
                    "Bash retained the intermediary as direct parent at C"
                );
                assert_ne!(
                    child.actor.pidns_ino,
                    parent_attach["pidns_ino"].as_u64().unwrap(),
                    "Bash must run inside a nested PID namespace below its parent work"
                );
                assert!(
                    physical_dir
                        .join(format!("{}.consumed.json", child.parent_work_grant_id))
                        .exists()
                );
                assert_eq!(child.root_id, prepared.root_id);
                assert_ne!(child.invocation_uuid, root.invocation_uuid);
                assert_ne!(child.d_key, root.d_key);
                assert!(child.handle.starts_with("ab30_"));
                assert_eq!(report["stdout"], "v30-child-output\n");
                assert_eq!(result.exit_code, 0);
                assert_eq!(
                    result.stdout_sha256,
                    format!("{:x}", Sha256::digest(b"v30-child-output\n"))
                );
                assert_eq!(report["no_new_privs"], 0);
                assert_eq!(report["seccomp"], 0);
                assert_eq!(fs::read(gate.join("bash-effect")).unwrap(), b"ran\n");
                if root_h_delegate && !root_h_lost {
                    let original = rusqlite::Connection::open_with_flags(
                        data.join("state.db"),
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap();
                    let decisions: i64 = original
                        .query_row(
                            "SELECT count(*) FROM invocation_completion_exact_source_decisions",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(decisions, 1, "delegated original Bash W absent");
                    let registrations: i64 = original
                        .query_row(
                            "SELECT count(*) FROM invocation_completion_v2_identity",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(registrations, 1, "original W registration absent");
                    let h_state = std::path::PathBuf::from(
                        fs::read_to_string(gate.join("h-source-intent-dir")).unwrap(),
                    );
                    let original_meta: serde_json::Value =
                        serde_json::from_slice(&fs::read(h_state.join("meta.json")).unwrap())
                            .unwrap();
                    assert_eq!(
                        original_meta["delivery_mode"],
                        if root_h_notify { "async" } else { "sync" }
                    );
                    let w_replies = fs::read_dir(&h_state)
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter(|entry| {
                            let name = entry.file_name();
                            let name = name.to_string_lossy();
                            name.starts_with("continuation-register-") && name.ends_with(".stdout")
                        })
                        .filter_map(|entry| fs::read(entry.path()).ok())
                        .filter_map(|bytes| {
                            serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                        })
                        .collect::<Vec<_>>();
                    assert_eq!(w_replies.len(), 1, "original Bash W helper reply absent");
                    assert_eq!(w_replies[0]["status"], "registered");
                    assert_eq!(w_replies[0]["registration_committed"], true);
                    let h_grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(h_state.join("root-work-broker-grant-v1.json")).unwrap(),
                    )
                    .unwrap();
                    let durable_grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            broker_state
                                .join("grants")
                                .join(format!("{}.json", h_grant["grant_id"].as_str().unwrap())),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        durable_grant["consumed"], true,
                        "original H grant did not launch Q"
                    );
                    assert_eq!(
                        durable_grant["delegated_root_h"]["proof"]["work_id"],
                        h_grant["work_id"]
                    );
                    let q_records = fs::read_dir(broker_state.join("works"))
                        .unwrap()
                        .filter_map(Result::ok)
                        .filter_map(|entry| fs::read(entry.path()).ok())
                        .filter_map(|bytes| {
                            serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                        })
                        .filter(|work| work["accepted_grant_id"] == h_grant["grant_id"])
                        .collect::<Vec<_>>();
                    assert_eq!(
                        q_records.len(),
                        1,
                        "original H did not launch one physical Q"
                    );
                    assert_eq!(q_records[0]["work_id"], h_grant["work_id"]);
                    assert!(
                        q_records[0]["init_host_pid"]
                            .as_i64()
                            .is_some_and(|pid| pid > 0)
                    );
                    assert_eq!(fs::read(h_state.join("rc")).unwrap(), b"0\n");
                    if root_h_notify {
                        eventually(|| {
                            h_state.join("completion-snapshot-v2.json").exists()
                                && h_state.join("source-outcome-v2.json").exists()
                                && gate.join("source-adopted-started").exists()
                        });
                        fs::write(gate.join("source-adopted-started.release"), b"drain").unwrap();
                    }
                    let original_session = FreshV30Lane::open_at(&broker_state)
                        .unwrap()
                        .read_session(&root.d_key)
                        .unwrap()
                        .unwrap();
                    let original_sidecar = rusqlite::Connection::open_with_flags(
                        broker_state.join("sidecar/pid-identity.db"),
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap();
                    if root_h_notify {
                        let until = Instant::now() + Duration::from_secs(20);
                        loop {
                            if original_sidecar
                                .query_row(
                                    "SELECT count(*) FROM mailbox WHERE session_id=?1",
                                    [&original_session.session_id],
                                    |row| row.get::<_, i64>(0),
                                )
                                .is_ok_and(|count| count == 1)
                            {
                                break;
                            }
                            assert!(
                                Instant::now() < until,
                                "selected-K original notification absent: broker={} bash={} source_physical={:?}",
                                fs::read_to_string(&broker_log).unwrap_or_default(),
                                fs::read_to_string(gate.join("bash-causal-error"))
                                    .unwrap_or_default(),
                                SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                                    .map(|registry| registry.records().len())
                            );
                            std::thread::sleep(Duration::from_millis(20));
                        }
                    }
                    let rows: i64 = original_sidecar
                        .query_row(
                            "SELECT count(*) FROM mailbox WHERE session_id=?1",
                            [&original_session.session_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(
                        rows,
                        if root_h_notify { 1 } else { 0 },
                        "original H listener policy did not control row materialization"
                    );
                    let old_pending: i64 = original_sidecar
                        .query_row(
                            "SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND handle='old-unacked' AND delivered_at IS NULL",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(old_pending, 1, "selected-K source changed old v29 debt");
                    if root_h_notify {
                        let physical =
                            SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                                .unwrap();
                        assert_eq!(
                            physical.records().len(),
                            1,
                            "selected-K H did not retain one original source Q"
                        );
                        let source_grant = &physical.records()[0].grant;
                        let mut retained = BrokerSidecar::open_existing(
                            &broker_state.join("sidecar/pid-identity.db"),
                            &broker_state,
                        )
                        .unwrap();
                        let original_binding = retained
                            .read_consumed_source_candidate(source_grant)
                            .unwrap();
                        let original_source = original_binding.registration().unwrap();
                        assert_eq!(original_source.delivery_mode, "async");
                        assert_eq!(
                            original_source.owner_session_id,
                            original_session.session_id
                        );
                        assert_eq!(original_source.owner_invocation_uuid, root.invocation_uuid);
                        let custody =
                            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                                &mut retained,
                                &physical,
                                &source_grant.grant_id,
                            )
                            .unwrap();
                        assert_eq!(custody.root_id, prepared.root_id);
                        assert_eq!(custody.owner_generation, prepared.owner_generation);
                        assert_eq!(custody.session_id, original_session.session_id);
                        assert_eq!(custody.owner_invocation_uuid, root.invocation_uuid);
                        let payload = retained
                            .mailbox()
                            .completion_recovery_payload(&original_source.handle)
                            .unwrap()
                            .unwrap();
                        assert_eq!(
                            custody.payload_sha256,
                            format!("{:x}", Sha256::digest(&payload))
                        );
                        assert_eq!(custody.payload_byte_len, payload.len() as i64);
                        let snapshot_path = std::path::Path::new(&original_source.handle_dir)
                            .join(&original_source.snapshot_relative);
                        let saved_snapshot = fs::read(&snapshot_path).unwrap();
                        fs::write(&snapshot_path, b"changed original source").unwrap();
                        assert!(
                            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                                &mut retained,
                                &physical,
                                &source_grant.grant_id,
                            )
                            .is_err(),
                            "changed selected-K original source passed custody readback"
                        );
                        fs::write(&snapshot_path, saved_snapshot).unwrap();
                        let payload_path: String = original_sidecar
                            .query_row(
                                "SELECT payload_file_path FROM mailbox WHERE seq=?1",
                                [custody.row_seq],
                                |row| row.get(0),
                            )
                            .unwrap();
                        let payload_json: serde_json::Value =
                            serde_json::from_slice(&payload).unwrap();
                        let output_path = std::path::Path::new(
                            payload_json["output_artifact"]["path"].as_str().unwrap(),
                        );
                        let saved_output = fs::read(output_path).unwrap();
                        fs::write(output_path, b"changed retained output").unwrap();
                        assert!(
                            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                                &mut retained,
                                &physical,
                                &source_grant.grant_id,
                            )
                            .is_err(),
                            "changed selected-K retained output passed custody readback"
                        );
                        fs::write(output_path, saved_output).unwrap();
                        fs::write(&payload_path, b"changed retained payload").unwrap();
                        assert!(
                            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                                &mut retained,
                                &physical,
                                &source_grant.grant_id,
                            )
                            .is_err(),
                            "changed selected-K payload passed custody readback"
                        );
                        fs::write(&payload_path, &payload).unwrap();
                        assert_eq!(
                            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                                &mut retained,
                                &physical,
                                &source_grant.grant_id,
                            )
                            .unwrap(),
                            custody,
                        );
                        let again = oulipoly_kernel_broker::source_acceptance::trigger_v2_completion_source(
                            &mut retained, &physical, &source_grant.grant_id,
                        ).unwrap();
                        assert_eq!(
                            again.mailbox_rows.len(),
                            1,
                            "duplicate trigger created another original row"
                        );
                        assert_eq!(again.mailbox_rows[0].seq, custody.row_seq);
                        if root_h_wake {
                            fs::write(gate.join("v2-wake-activate"), b"activate").unwrap();
                            eventually(|| {
                                retained
                                    .read_v2_recipient_grant(&source_grant.grant_id)
                                    .is_ok_and(|grant| {
                                        grant.as_ref().is_some_and(|grant| {
                                            grant.phase
                                                == if root_h_wake_unknown {
                                                    "unknown"
                                                } else if root_h_wake_lost {
                                                    "submitted"
                                                } else {
                                                    "acked"
                                                }
                                        })
                                    })
                            });
                            let grant = retained
                                .read_v2_recipient_grant(&source_grant.grant_id)
                                .unwrap()
                                .unwrap();
                            assert_eq!(grant.binding.row_seq, custody.row_seq);
                            assert_eq!(grant.binding.payload_sha256, custody.payload_sha256);
                            assert_eq!(grant.binding.session_id, original_session.session_id);
                            assert_eq!(
                                grant.ack_response_sha256.is_some(),
                                !root_h_wake_lost && !root_h_wake_unknown,
                            );
                            assert!(retained.begin_v2_recipient_send(&grant).is_err());
                            assert!(
                                retained
                                    .acknowledge_v2_recipient(
                                        &source_grant.grant_id,
                                        "wrong-process",
                                        &grant.delivery_token,
                                        &"0".repeat(64),
                                    )
                                    .is_err(),
                                "wrong actor ACKed selected-K original wake"
                            );
                            if root_h_wake_lost || root_h_wake_unknown {
                                let mut stale: oulipoly_kernel_broker::entry_registry::ProcessStamp =
                                    serde_json::from_str(&grant.recipient_identity).unwrap();
                                stale.starttime_ticks += 1;
                                assert!(
                                    retained
                                        .acknowledge_v2_recipient(
                                            &source_grant.grant_id,
                                            &serde_json::to_string(&stale).unwrap(),
                                            &grant.delivery_token,
                                            &"0".repeat(64),
                                        )
                                        .is_err(),
                                    "stale recipient incarnation ACKed selected-K wake"
                                );
                            } else {
                                assert!(
                                    retained
                                        .acknowledge_v2_recipient(
                                            &source_grant.grant_id,
                                            &grant.recipient_identity,
                                            &grant.delivery_token,
                                            &"0".repeat(64),
                                        )
                                        .is_err(),
                                    "duplicate ACK changed selected-K wake settlement"
                                );
                            }
                            let grant_count: i64 = original_sidecar
                                .query_row(
                                    "SELECT count(*) FROM broker_v2_recipient_grant",
                                    [],
                                    |row| row.get(0),
                                )
                                .unwrap();
                            assert_eq!(grant_count, 1, "selected-K wake duplicated a grant");
                            let phase = grant.phase.clone();
                            assert!(
                                raw_fresh_id_request(
                                    &socket.with_file_name("v30.sock"),
                                    b'C',
                                    uuid::Uuid::new_v4(),
                                )
                                .contains("Bash child image changed"),
                                "wrong Bash image control lost before wake restart"
                            );
                            if !root_terminal_drain {
                                stop(&mut broker);
                                broker = restart_source_broker(
                                    &socket,
                                    &broker_state,
                                    &runner,
                                    &gate,
                                    &temp.path().join("selected-h-wake-restart.log"),
                                );
                            }
                            assert_eq!(
                                retained
                                    .read_v2_recipient_grant(&source_grant.grant_id)
                                    .unwrap()
                                    .unwrap(),
                                grant,
                                "restart changed original selected-K wake grant"
                            );
                            assert_eq!(
                                phase,
                                if root_h_wake_unknown {
                                    "unknown"
                                } else if root_h_wake_lost {
                                    "submitted"
                                } else {
                                    "acked"
                                }
                            );
                        } else {
                            assert!(
                                retained
                                    .read_v2_recipient_grant(&source_grant.grant_id)
                                    .unwrap()
                                    .is_none()
                            );
                        }
                    }
                    let release = BrokerSidecar::open_existing(
                        &broker_state.join("sidecar/pid-identity.db"),
                        &broker_state,
                    )
                    .unwrap()
                    .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
                    .unwrap();
                    assert_eq!(
                        release, root.old_release,
                        "original D/J release readback changed"
                    );
                }
                let fresh = rusqlite::Connection::open_with_flags(
                    broker_state.join("v30/state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let (parent, session, model): (i64,String,String) = fresh.query_row(
                    "SELECT parent_invocation_id,provider_session_id,model_name FROM invocations WHERE invocation_uuid=?1",
                    [&child.invocation_uuid], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
                let root_id: i64 = fresh
                    .query_row(
                        "SELECT id FROM invocations WHERE invocation_uuid=?1",
                        [&root.invocation_uuid],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(parent, root_id);
                assert_eq!(session, child.session.session_id);
                assert_eq!(model, "agent-bash-child");
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM invocations", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    2
                );
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM fresh_bash_private_work", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    1
                );
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM fresh_bash_child", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    1,
                    "lost first C reply must not admit a second child"
                );
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM fresh_bash_private_result", [], |r| {
                            r.get::<_, i64>(0)
                        })
                        .unwrap(),
                    1
                );
                let accepted: (String, String, String, String, String) = fresh.query_row(
                    "SELECT source_id,attempt_id,state_admission_id,root_id,owner_generation FROM fresh_lane_accepted_source",
                    [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                ).unwrap();
                assert_eq!(accepted.0, source.source_id);
                assert_eq!(accepted.1, source.attempt_id);
                assert_eq!(accepted.2, source.state_admission_id);
                assert_eq!(accepted.3, source.root_id);
                assert_eq!(accepted.4, source.owner_generation);
                assert_eq!(
                    fresh
                        .query_row("SELECT count(*) FROM fresh_bash_selected_event", [], |r| {
                            r.get::<_, i64>(0)
                        })
                        .unwrap(),
                    1
                );
                if !mode.contains("_notify_") {
                    assert_eq!(
                        fresh
                            .query_row(
                                "SELECT count(*) FROM fresh_lane_recipient_attachment",
                                [],
                                |r| r.get::<_, i64>(0)
                            )
                            .unwrap(),
                        0
                    );
                    assert_old_debt_and_no_f_ack(&broker_state);
                }
                assert_eq!(
                    oulipoly_kernel_broker::registry::RootRegistry::open(&broker_state)
                        .unwrap()
                        .live_roots()
                        .count(),
                    1,
                    "Bash child must not mint a supervisor/root"
                );
                let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                lane.repair_captured_private_bash_sources().unwrap();
                lane.accept_private_bash_source(&source).unwrap();
                lane.repair_captured_private_bash_source(&bash_request)
                    .unwrap();
                let (pre_q_root, pre_q_actor) =
                    lane.released_handoff_for_root(&prepared.root_id).unwrap();
                let pre_q_session = lane.read_session(&pre_q_root.d_key).unwrap().unwrap();
                let pre_q_terminal = lane
                    .read_private_root_terminal(&pre_q_root, &pre_q_actor, &pre_q_session)
                    .unwrap();
                assert_eq!(pre_q_terminal.execution_state, "unknown");
                assert!(
                    pre_q_terminal
                        .unknown_stage
                        .as_deref()
                        .unwrap()
                        .starts_with("parent_k_q:")
                );
                let mut wrong_source = source.clone();
                wrong_source.parent_work_id = uuid::Uuid::new_v4().to_string();
                assert!(lane.accept_private_bash_source(&wrong_source).is_err());
                wrong_source = source.clone();
                wrong_source.physical_grant_id = uuid::Uuid::new_v4().to_string();
                assert!(lane.accept_private_bash_source(&wrong_source).is_err());
                wrong_source = source.clone();
                wrong_source.stdout_sha256 = result.stdout_sha256.clone();
                assert!(
                    lane.accept_private_bash_source(&wrong_source).is_err(),
                    "Bash O substituted for broker output"
                );
                wrong_source = source.clone();
                wrong_source.completion_policy = "ready".into();
                assert!(lane.accept_private_bash_source(&wrong_source).is_err());
                assert_eq!(
                    lane.read_private_bash_result(&bash_request).unwrap(),
                    Some(result)
                );
                let root_actor = oulipoly_state::mailbox::FreshRecipientIdentity {
                    host_pid: prepared.joined_child.host_pid,
                    boot_id: prepared.joined_child.boot_id.clone(),
                    starttime_ticks: prepared.joined_child.starttime_ticks,
                    pidns_dev: prepared.joined_child.pidns_dev,
                    pidns_ino: prepared.joined_child.pidns_ino,
                };
                let mut wrong_actor = child.actor.clone();
                wrong_actor.starttime_ticks += 1;
                assert!(
                    lane.require_bash_child(&child, &root, &root_actor, &wrong_actor)
                        .is_err()
                );
                wrong_actor = child.actor.clone();
                wrong_actor.pidns_ino += 1;
                assert!(
                    lane.require_bash_child(&child, &root, &root_actor, &wrong_actor)
                        .is_err()
                );
                let mut wrong_parent = child.clone();
                wrong_parent.parent_invocation_uuid = uuid::Uuid::new_v4().to_string();
                assert!(
                    lane.require_bash_child(&wrong_parent, &root, &root_actor, &child.actor)
                        .is_err()
                );
                let mut wrong_d = child.clone();
                wrong_d.session.request_id = root.d_key.clone();
                assert!(
                    lane.require_bash_child(&wrong_d, &root, &root_actor, &child.actor)
                        .is_err()
                );
                if root_terminal_drain {
                    fs::write(gate.join("terminal-drain-release"), b"verified-w").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(
                        entry.wait().unwrap().success(),
                        "root output failed: {} / {}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default()
                    );
                    let terminal = lane
                        .read_private_root_terminal(&pre_q_root, &pre_q_actor, &pre_q_session)
                        .unwrap();
                    let original: oulipoly_state::mailbox::FreshRootTerminalReadback =
                        serde_json::from_slice(
                            &fs::read(gate.join("root-terminal-readback.json")).unwrap(),
                        )
                        .unwrap();
                    assert_eq!(terminal.execution, original.execution);
                    assert_eq!(terminal.execution_state, "success");
                    assert_eq!(terminal.publication_state, "settled");
                    let entry_record: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            broker_state
                                .join("entries")
                                .join(format!("{}.json", prepared.root_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        entry_record["terminal_settlement"]["d_key"],
                        pre_q_root.d_key
                    );
                    assert_eq!(
                        entry_record["terminal_settlement"]["publication_sha256"],
                        terminal.publication_sha256.as_deref().unwrap()
                    );
                    let parent = &terminal.execution.as_ref().unwrap().parent;
                    assert_eq!(
                        fs::read(&out).unwrap(),
                        fs::read(physical_dir.join(format!("{}.stdout", parent.grant_id))).unwrap()
                    );
                    assert_eq!(
                        fs::read(&err).unwrap(),
                        fs::read(physical_dir.join(format!("{}.stderr", parent.grant_id))).unwrap()
                    );
                    assert_eq!(terminal.native_receipt_state, "not_observed");
                    assert_eq!(terminal.unresolved_child_request_ids.len(), 0);
                    assert_eq!(
                        terminal.execution.as_ref().unwrap().child_event,
                        Some(source)
                    );
                    assert_eq!(
                        terminal.notification_state,
                        if root_h_notify {
                            "pending_f"
                        } else {
                            "response_only"
                        }
                    );
                    // The old original H wake ACK is separate from the fresh
                    // W-derived F row. It cannot settle that row or imply a
                    // provider-native input receipt.
                    assert_eq!(terminal.ack_basis, None);
                    let inventory: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("root-drain-terminal.json")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(inventory["root_id"], prepared.root_id);
                    assert_eq!(inventory["fenced"], true);
                    assert_eq!(inventory["close_eligible"], false);
                    assert!(
                        broker_state
                            .join("root-drains")
                            .join(format!("{}.json", prepared.root_id))
                            .exists()
                    );
                    assert!(
                        oulipoly_kernel_broker::registry::RootRegistry::open(&broker_state)
                            .unwrap()
                            .admission_fenced(&prepared.root_id)
                    );
                    if !root_h_notify {
                        assert_old_debt_and_no_f_ack(&broker_state);
                    } else {
                        let old = rusqlite::Connection::open(
                            broker_state.join("sidecar/pid-identity.db"),
                        )
                        .unwrap();
                        let pending: i64 = old.query_row(
                            "SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND handle='old-unacked' AND delivered_at IS NULL",
                            [], |row| row.get(0),
                        ).unwrap();
                        assert_eq!(pending, 1, "old v29 debt changed by fresh terminal fence");
                    }
                    stop(&mut broker);
                    assert!(
                        oulipoly_kernel_broker::registry::RootRegistry::open(&broker_state)
                            .unwrap()
                            .admission_fenced(&prepared.root_id),
                        "root drain fence was lost across Broker restart readback"
                    );
                    unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                    return;
                }
                let mut partial = child.clone();
                partial.request_id = uuid::Uuid::new_v4().to_string();
                partial.d_key = uuid::Uuid::new_v4().to_string();
                partial.invocation_uuid = uuid::Uuid::new_v4().to_string();
                partial.handle = format!("ab30_{}", uuid::Uuid::new_v4().simple());
                partial.actor.starttime_ticks += 1;
                let fresh_write =
                    rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                fresh_write
                    .execute(
                        "INSERT INTO fresh_bash_child
                     (request_id,d_key,invocation_uuid,handle,root_handoff_id,root_id,
                      parent_invocation_uuid,actor_identity,receipt_json,admitted_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'2026-09-24T00:00:00Z')",
                        rusqlite::params![
                            partial.request_id,
                            partial.d_key,
                            partial.invocation_uuid,
                            partial.handle,
                            partial.root_handoff_id,
                            partial.root_id,
                            partial.parent_invocation_uuid,
                            serde_json::to_string(&partial.actor).unwrap(),
                            serde_json::to_string(&partial).unwrap()
                        ],
                    )
                    .unwrap();
                assert!(
                    lane.admit_private_bash_work(&partial).is_err(),
                    "partial child row acquired a work grant"
                );
                assert!(
                    fresh
                        .query_row(
                            "SELECT count(*) FROM fresh_bash_private_work WHERE request_id=?1",
                            [&partial.request_id],
                            |r| r.get::<_, i64>(0),
                        )
                        .unwrap()
                        == 0
                );
                let image_probe = raw_fresh_id_request(
                    &socket.with_file_name("v30.sock"),
                    b'C',
                    uuid::Uuid::new_v4(),
                );
                assert!(
                    image_probe.contains(if root_h_wake {
                        "installed Bash image absent"
                    } else {
                        "Bash child image changed"
                    }),
                    "{image_probe}"
                );
                assert!(
                    !raw_fresh_id_request(&socket, b'C', uuid::Uuid::new_v4())
                        .starts_with("fresh-bash-child "),
                    "old socket admitted a fresh child"
                );
                // The same pinned Bash image with a copied root key remains
                // outside the root namespace when called by this sibling.
                let outside_marker = gate.join("outside-bash-effect");
                let sibling = Command::new(bash.as_ref().unwrap())
                    .arg("__age319-private-admit-child-v1")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("AGE319_PRIVATE_BASH_EFFECT_MARKER", &outside_marker)
                    .env("AGE319_PRIVATE_BASH_REQUEST_KEY", &bash_request)
                    .env("AGENT_BASH_OWNER_INVOCATION_UUID", &root.invocation_uuid)
                    .env("AGENT_BASH_OWNER_SESSION_ID", &child.session.session_id)
                    .env(
                        "OULIPOLY_PARENT_INVOCATION",
                        format!("{{\"id\":\"{}\"}}", root.invocation_uuid),
                    )
                    .output()
                    .unwrap();
                assert!(
                    !sibling.status.success(),
                    "copied root data admitted sibling Bash"
                );
                assert!(!outside_marker.exists());
                fs::write(gate.join("sibling-request"), b"yes").unwrap();
                eventually(|| {
                    fs::read(gate.join("sibling-result.json"))
                        .ok()
                        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                        .is_some()
                        || entry.try_wait().unwrap().is_some()
                });
                let sibling_result: serde_json::Value =
                    serde_json::from_slice(&fs::read(gate.join("sibling-result.json")).unwrap())
                        .unwrap();
                assert_eq!(sibling_result["success"], false);
                assert!(
                    sibling_result["stderr"]
                        .as_str()
                        .unwrap()
                        .contains("Bash is outside exact consumed parent work")
                        || (root_h_notify
                            && sibling_result["stderr"]
                                .as_str()
                                .unwrap()
                                .contains("Bash child scope uncertain"))
                        || (root_h_wake
                            && sibling_result["stderr"]
                                .as_str()
                                .unwrap()
                                .contains("installed Bash image absent")),
                    "same-root sibling namespace refusal: {sibling_result}"
                );
                assert!(!gate.join("sibling-effect").exists());
                if mode.contains("notify_") && !root_h_notify {
                    eventually(|| {
                        gate.join("bash-recipient-output").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    let observed: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("bash-recipient-output")).unwrap_or_else(|_| {
                            panic!(
                                "recipient did not observe F: entry={} broker={}",
                                fs::read_to_string(&err).unwrap_or_default(),
                                fs::read_to_string(&broker_log).unwrap_or_default()
                            )
                        }),
                    )
                    .unwrap();
                    assert_eq!(
                        fs::read_to_string(gate.join("bash-f-request-id")).unwrap(),
                        observed["delivery_request_id"].as_str().unwrap()
                    );
                    assert_eq!(observed["grant"]["source_id"], source.source_id);
                    assert_eq!(observed["grant"]["attempt_id"], source.attempt_id);
                    assert_eq!(child.listener_policy, "notify");
                    assert_eq!(observed["listener_policy"], "notify_at_admission");
                    let mut lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    lane.register_private_bash_source(&child).unwrap();
                    assert!(
                        lane.request_private_bash_notification(&bash_request, &child.actor)
                            .is_err(),
                        "Bash child substituted for original listener"
                    );
                    let session = lane.read_session(&root.d_key).unwrap().unwrap();
                    let seq = observed["grant"]["seq"].as_i64().unwrap();
                    let payload = lane
                        .lookup_payload(&lane.identity().lane_id, &session.session_id, seq)
                        .unwrap();
                    assert_eq!(
                        format!("{:x}", Sha256::digest(payload)),
                        observed["observed_payload_sha256"].as_str().unwrap()
                    );
                    let row = rusqlite::Connection::open(
                        broker_state.join("v30/sidecar/pid-identity.db"),
                    )
                    .unwrap();
                    let acked: bool = row
                        .query_row(
                            "SELECT delivered_at IS NOT NULL FROM mailbox WHERE seq=?1",
                            [seq],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert_eq!(acked, mode.ends_with("notify_ack"));
                    if mode.ends_with("notify_prepare_unavailable") {
                        assert_eq!(
                            observed["native_f_preparation_refusal"],
                            "native F original-root resident PTY generation and selected adapter page authority absent"
                        );
                        assert!(matches!(
                            observed["readback"]["grant"]["phase"].as_str(),
                            Some("unknown" | "submitted")
                        ));
                        let count: i64 = row
                            .query_row("SELECT count(*) FROM fresh_native_f_preparation", [], |r| {
                                r.get(0)
                            })
                            .unwrap();
                        assert_eq!(count, 0);
                        let generation_count: i64 = row
                            .query_row(
                                "SELECT count(*) FROM runtime_generation WHERE session_id=?1",
                                [&session.session_id],
                                |r| r.get(0),
                            )
                            .unwrap();
                        assert_eq!(generation_count, 0);
                    }
                }
                if root_h_delegate && !root_h_lost {
                    // Original W and Q have completed. The parent selected-K
                    // fixture deliberately remains an unresolved root result;
                    // it is outside this delegated H/W/Q admission assertion.
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    stop(&mut entry);
                    stop(&mut broker);
                    unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                    return;
                }
                fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                let until = Instant::now() + Duration::from_secs(20);
                while entry.try_wait().unwrap().is_none() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(
                    entry.try_wait().unwrap().is_some(),
                    "causal parent Q wait: entry={} broker={}",
                    fs::read_to_string(&err).unwrap_or_default(),
                    fs::read_to_string(&broker_log).unwrap_or_default()
                );
                assert!(
                    fs::read_to_string(&err)
                        .unwrap()
                        .contains("private provider runtime result mapped after Q; root terminal publication closed"),
                    "{}",
                    fs::read_to_string(&err).unwrap()
                );
                let mapped: serde_json::Value = serde_json::from_slice(
                    &fs::read(gate.join("provider-runtime-result")).unwrap(),
                )
                .unwrap();
                assert_eq!(mapped["mapped_after_q"], true);
                let parent_grant_id = child.parent_work_grant_id;
                for suffix in ["consumed", "exit", "drain", "pid1-wait"] {
                    assert!(
                        physical_dir
                            .join(format!("{parent_grant_id}.{suffix}.json"))
                            .exists(),
                        "causal parent physical {suffix} absent"
                    );
                }
                let (terminal_root, terminal_actor) =
                    lane.released_handoff_for_root(&prepared.root_id).unwrap();
                let terminal_session = lane.read_session(&terminal_root.d_key).unwrap().unwrap();
                let terminal = lane
                    .settle_private_root_terminal(
                        &terminal_root,
                        &terminal_actor,
                        &terminal_session,
                    )
                    .unwrap();
                let wire: oulipoly_state::mailbox::FreshRootTerminalReadback =
                    serde_json::from_slice(
                        &fs::read(gate.join("root-terminal-readback.json")).unwrap(),
                    )
                    .unwrap();
                assert_eq!(wire.execution, terminal.execution);
                assert_eq!(wire.notification_state, terminal.notification_state);
                assert_eq!(terminal.handoff_id, root.handoff_id);
                assert_eq!(terminal.invocation_uuid, root.invocation_uuid);
                assert_eq!(terminal.session_id, terminal_session.session_id);
                assert!(
                    terminal.execution.is_some(),
                    "terminal evidence: {terminal:#?}"
                );
                assert_eq!(
                    terminal.execution.as_ref().unwrap().parent.grant_id,
                    parent_grant_id
                );
                assert_eq!(
                    terminal
                        .execution
                        .as_ref()
                        .unwrap()
                        .child_event
                        .as_ref()
                        .unwrap(),
                    &source
                );
                assert_eq!(terminal.publication_state, "not_started");
                assert_eq!(
                    terminal.child_request_id.as_deref(),
                    Some(bash_request.as_str())
                );
                assert_eq!(
                    terminal.unresolved_child_request_ids,
                    vec![partial.request_id.clone()]
                );
                assert_eq!(
                    terminal.refusal.as_deref(),
                    Some("unresolved_child_admission")
                );
                assert_eq!(terminal.execution_state, "failure");
                assert_eq!(
                    terminal.terminal_state,
                    "execution_failed_child_admission_pending"
                );
                assert!(
                    terminal
                        .artifacts
                        .contains(&format!("unresolved-child-c:{}", partial.request_id))
                );
                assert_eq!(terminal.native_receipt_state, "not_observed");
                assert_eq!(
                    terminal.listener_policy.as_deref(),
                    Some(if mode.contains("notify_") {
                        "notify"
                    } else {
                        "response_only"
                    })
                );
                if mode.contains("notify_") {
                    assert!(matches!(
                        terminal.notification_state.as_str(),
                        "repair_required"
                            | "pending_f"
                            | "f_unknown"
                            | "f_submitted_native_pending"
                            | "acked"
                    ));
                    if mode.ends_with("notify_ack") {
                        assert_eq!(terminal.notification_state, "acked");
                        assert_eq!(terminal.ack_basis.as_deref(), Some("manual_ack"));
                        assert_eq!(terminal.native_receipt_state, "not_observed");
                    } else {
                        assert_ne!(terminal.notification_state, "acked");
                    }
                } else {
                    assert_eq!(terminal.notification_state, "response_only");
                    assert!(terminal.delivery_grant_id.is_none());
                    assert!(terminal.ack_basis.is_none());
                }
                let mut wrong_terminal_actor = terminal_actor.clone();
                wrong_terminal_actor.starttime_ticks += 1;
                assert!(
                    lane.read_private_root_terminal(
                        &terminal_root,
                        &wrong_terminal_actor,
                        &terminal_session
                    )
                    .is_err()
                );
                assert!(
                    lane.begin_private_root_publication(
                        &terminal_root,
                        &terminal_actor,
                        &terminal_session,
                        b"cannot claim all work while another C remains unresolved\n"
                    )
                    .is_err()
                );
                let parent = &terminal.execution.as_ref().unwrap().parent;
                let offered = oulipoly_state::mailbox::FreshRootCallerResult {
                    parent_grant_id: parent.grant_id.clone(),
                    wait_status: parent.wait_status,
                    stdout_sha256: parent.stdout_sha256.clone(),
                    stdout_len: parent.stdout_len,
                    stderr_sha256: parent.stderr_sha256.clone(),
                    stderr_len: parent.stderr_len,
                    native_f: None,
                };
                assert_eq!(
                    lane.begin_private_root_caller_result(
                        &terminal_root,
                        &terminal_actor,
                        &terminal_session,
                        &offered
                    )
                    .unwrap_err(),
                    "root terminal has unresolved child C"
                );
                assert!(
                    lane.begin_private_root_publication(
                        &terminal_root,
                        &terminal_actor,
                        &terminal_session,
                        b"changed caller output\n"
                    )
                    .is_err()
                );
                // Preserve the original child's durable-result readback
                // check after a broker restart, now on the causal route.
                stop(&mut broker);
                broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .env(
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                        bash.as_ref().unwrap(),
                    )
                    .stderr(Stdio::from(
                        File::create(temp.path().join("bash-child-restart.log")).unwrap(),
                    ))
                    .spawn()
                    .unwrap();
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                let reopened = FreshV30Lane::open_at(&broker_state).unwrap();
                if root_h_delegate {
                    let delegated = reopened
                        .read_root_h_delegation(&terminal_root.handoff_id)
                        .unwrap()
                        .unwrap();
                    assert_eq!(delegated.child_request_id, child.request_id);
                    assert_eq!(delegated.selected_k.grant_id, parent_grant_id);
                    let state =
                        rusqlite::Connection::open(broker_state.join("v30/state.db")).unwrap();
                    assert_eq!(
                        state
                            .query_row(
                                "SELECT count(*) FROM fresh_root_h_consumption WHERE handoff_id=?1",
                                [&terminal_root.handoff_id],
                                |r| r.get::<_, i64>(0),
                            )
                            .unwrap(),
                        1,
                        "restart replayed or lost one-use H consumption"
                    );
                }
                let replay = reopened
                    .read_private_root_terminal(&terminal_root, &terminal_actor, &terminal_session)
                    .unwrap();
                assert_eq!(replay.execution, terminal.execution);
                assert_eq!(replay.publication_state, "not_started");
                assert_eq!(
                    reopened
                        .settle_private_root_terminal(
                            &terminal_root,
                            &terminal_actor,
                            &terminal_session
                        )
                        .unwrap()
                        .execution,
                    terminal.execution
                );
                assert_eq!(
                    reopened.read_private_bash_result(&bash_request).unwrap(),
                    lane.read_private_bash_result(&bash_request).unwrap()
                );
                stop(&mut broker);
                return;
            }
            if v3_mode
                || mode.starts_with("normal_model_provider_pty_")
                || matches!(
                    mode.as_str(),
                    "normal_handoff"
                        | "normal_handoff_effect_reply_loss"
                        | "normal_help"
                        | "normal_model_held"
                        | "normal_model_provider"
                        | "normal_model_provider_caller_binary"
                        | "normal_model_provider_caller_nonzero"
                        | "normal_model_provider_caller_partial"
                        | "normal_model_provider_caller_lost"
                        | "normal_model_provider_reply_loss"
                        | "normal_model_provider_q_reply_loss"
                        | "normal_model_provider_restart"
                        | "normal_model_provider_bad_config"
                        | "normal_model_provider_unsupported"
                        | "normal_model_provider_quota"
                        | "normal_model_provider_auth"
                        | "normal_model_provider_auth_recovery"
                        | "normal_model_provider_auth_after_healthy"
                        | "normal_model_provider_auth_success"
                        | "normal_model_provider_auth_reply_loss"
                        | "normal_model_provider_auth_restart"
                        | "normal_model_provider_quota_available"
                        | "normal_model_provider_quota_reply_loss"
                        | "normal_model_provider_quota_restart"
                        | "normal_model_provider_no_pin"
                        | "normal_model_provider_path"
                        | "normal_model_provider_prefix"
                        | "normal_model_provider_native_codex_root_h_notify_row"
                        | "normal_model_provider_native_codex_turn_receipt"
                        | "normal_model_provider_native_codex_recipient_ack"
                        | "normal_model_provider_native_codex_f_candidate"
                        | "normal_model_provider_native_codex_f_turn"
                        | "normal_model_provider_native_codex_f_turn_old_pending_debt"
                )
            {
                let marker: serde_json::Value =
                    serde_json::from_slice(&fs::read(gate.join("child-handoff")).unwrap()).unwrap();
                let receipt: oulipoly_state::mailbox::FreshReleasedHandoff =
                    serde_json::from_slice(
                        &fs::read(
                            broker_state
                                .join("released-handoffs")
                                .join(format!("{}.json", prepared.root_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                assert_eq!(marker["handoff_id"], receipt.handoff_id);
                assert_eq!(marker["d_key"], receipt.d_key);
                assert_eq!(marker["invocation_uuid"], receipt.invocation_uuid);
                assert_eq!(
                    marker["root_work_intent"],
                    serde_json::to_value(&receipt.root_work_intent).unwrap()
                );
                assert_eq!(
                    marker["no_new_privs"], 0,
                    "root child final exec must permit setuid"
                );
                assert_eq!(
                    marker["seccomp"], 0,
                    "root child final exec has no Runner seccomp filter"
                );
                assert!(receipt.old_release == released);
                assert_eq!(
                    receipt.root_work_intent,
                    if mode == "normal_help" {
                        oulipoly_state::mailbox::FreshRootWorkIntent::CliHelp(vec!["--help".into()])
                    } else if provider_mode {
                        let mut args = vec!["--model".into(), "configured-model".into()];
                        if mode != "normal_model_provider_no_pin" {
                            args.extend(["--pin-provider".into(), "local".into()]);
                        }
                        args.push("hello fixture".into());
                        oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(args)
                    } else if model_mode {
                        oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(vec![
                            "--model".into(),
                            "fixture-model".into(),
                            "hello fixture".into(),
                        ])
                    } else {
                        oulipoly_state::mailbox::FreshRootWorkIntent::PrivateProbe(vec![
                            "__age319-private-root-handoff-v1".into(),
                        ])
                    }
                );
                let receipt_json = serde_json::to_string(&receipt).unwrap();
                assert!(!receipt_json.contains("ab30_"));
                assert!(!receipt_json.contains("bash_handle"));
                let fresh_state = rusqlite::Connection::open_with_flags(
                    broker_state.join("v30/state.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let (session, provider): (String, String) = fresh_state.query_row(
                    "SELECT provider_session_id,provider_name FROM invocations WHERE invocation_uuid=?1",
                    [&receipt.invocation_uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                ).unwrap();
                assert_eq!(session, marker["session_id"].as_str().unwrap());
                assert_eq!(provider, "agent-runner");
                let root_model: String = fresh_state
                    .query_row(
                        "SELECT model_name FROM invocations WHERE invocation_uuid=?1",
                        [&receipt.invocation_uuid],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(root_model, "agent-runner-root");
                let invocation_count: i64 = fresh_state
                    .query_row("SELECT count(*) FROM invocations", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(
                    invocation_count, 1,
                    "root with no Bash child has one root invocation"
                );
                let bash_column_count: i64 = fresh_state
                    .query_row("SELECT count(*) FROM pragma_table_info('fresh_released_handoff') WHERE name='bash_handle'", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(
                    bash_column_count, 0,
                    "root State must not carry a Bash placeholder"
                );
                let bound: i64 = fresh_state.query_row(
                    "SELECT count(*) FROM fresh_released_handoff WHERE handoff_id=?1 AND d_key=?2",
                    rusqlite::params![receipt.handoff_id, receipt.d_key],
                    |row| row.get(0),
                ).unwrap();
                assert_eq!(bound, 1);
                let intent_kind: String = fresh_state
                    .query_row(
                        "SELECT root_intent_kind FROM fresh_released_handoff WHERE handoff_id=?1",
                        [&receipt.handoff_id],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    intent_kind,
                    if mode == "normal_help" {
                        "cli_help"
                    } else if model_mode {
                        "normal_cli"
                    } else {
                        "private_probe"
                    }
                );
                assert_eq!(
                    fs::metadata(&out).unwrap().len(),
                    0,
                    "handoff/D must not start Bash work"
                );
                let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                let child = &prepared.joined_child;
                let actor = oulipoly_state::mailbox::FreshRecipientIdentity {
                    host_pid: child.host_pid,
                    boot_id: child.boot_id.clone(),
                    starttime_ticks: child.starttime_ticks,
                    pidns_dev: child.pidns_dev,
                    pidns_ino: child.pidns_ino,
                };
                let session = lane.read_session(&receipt.d_key).unwrap().unwrap();
                assert!(
                    lane.read_root_effect(&receipt, &actor, &session)
                        .unwrap()
                        .is_none()
                );
                if model_mode {
                    assert!(
                        lane.read_normal_work(&receipt, &actor, &session)
                            .unwrap()
                            .is_none()
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key
                        )
                        .is_err(),
                        "live same-image sibling prepared held normal work"
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &uuid::Uuid::new_v4().to_string()
                        )
                        .is_err(),
                        "wrong key prepared held normal work"
                    );
                }
                assert!(
                    protocol::begin_fresh_root_effect_at(
                        &socket.with_file_name("v30.sock"),
                        &receipt.d_key,
                    )
                    .is_err(),
                    "same-image sibling began root work"
                );
                assert!(
                    protocol::begin_fresh_root_effect_at(
                        &socket.with_file_name("v30.sock"),
                        &uuid::Uuid::new_v4().to_string(),
                    )
                    .is_err(),
                    "wrong D key began root work"
                );
                let mut reused_pid = actor.clone();
                reused_pid.starttime_ticks += 1;
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &receipt, &reused_pid)
                        .is_err()
                );
                if model_mode {
                    assert!(
                        lane.prepare_normal_work(&receipt, &reused_pid, &session)
                            .is_err()
                    );
                }
                let mut wrong_lane = receipt.clone();
                wrong_lane.fresh_lane.lane_id = uuid::Uuid::new_v4().to_string();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_lane, &actor)
                        .is_err()
                );
                let mut wrong_domain = receipt.clone();
                wrong_domain.fresh_lane.domain_id = receipt.old_release.prepared.domain_id.clone();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_domain, &actor)
                        .is_err()
                );
                let mut wrong_old_lane = receipt.clone();
                wrong_old_lane.old_release.prepared.source_generation =
                    receipt.fresh_lane.source_generation.clone();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_old_lane, &actor)
                        .is_err()
                );
                let mut wrong_handle = receipt.clone();
                wrong_handle.root_work_intent =
                    oulipoly_state::mailbox::FreshRootWorkIntent::CliHelp(vec![
                        if mode == "normal_help" {
                            "-h"
                        } else {
                            "--help"
                        }
                        .into(),
                    ]);
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_handle, &actor)
                        .is_err()
                );
                let mut malformed_intent = receipt.clone();
                malformed_intent.root_work_intent =
                    oulipoly_state::mailbox::FreshRootWorkIntent::CliHelp(vec!["--new".into()]);
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &malformed_intent, &actor)
                        .is_err()
                );
                let mut wrong_invocation = receipt.clone();
                wrong_invocation.invocation_uuid = uuid::Uuid::new_v4().to_string();
                assert!(
                    lane.require_released_handoff(&receipt.d_key, &wrong_invocation, &actor)
                        .is_err()
                );
                drop(lane);
                // A copied locator from this test process has the wrong image
                // and the wrong physical child. It cannot reserve a second U.
                assert!(
                    protocol::request_released_fresh_handoff_at(
                        &socket.with_file_name("v30.sock"),
                        protocol::StateReadSpec {
                            protocol: "broker-release-attest-v30".into(),
                            source_generation: generation.clone(),
                            root_id: prepared.root_id.clone(),
                            owner_generation: prepared.owner_generation.clone(),
                            attempt_id: None,
                        },
                    )
                    .is_err()
                );
                let release_spec = serde_json::to_string(&protocol::StateReadSpec {
                    protocol: "broker-release-attest-v30".into(),
                    source_generation: generation.clone(),
                    root_id: prepared.root_id.clone(),
                    owner_generation: prepared.owner_generation.clone(),
                    attempt_id: None,
                })
                .unwrap();
                assert!(
                    protocol::request_released_fresh_handoff_at(
                        &socket,
                        serde_json::from_str(&release_spec).unwrap(),
                    )
                    .is_err(),
                    "old v29 listener admitted a fresh U"
                );
                let sibling_u = Command::new(&runner)
                    .arg("__age319-private-handoff-probe-v1")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("AGE319_PRIVATE_HANDOFF_PROBE_SPEC", &release_spec)
                    .status()
                    .unwrap();
                assert!(!sibling_u.success(), "same-image sibling U was admitted");
                let sibling_d = Command::new(&runner)
                    .arg("__age319-private-handoff-probe-v1")
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("AGE319_PRIVATE_HANDOFF_PROBE_D_KEY", &receipt.d_key)
                    .status()
                    .unwrap();
                assert!(!sibling_d.success(), "same-image sibling D was admitted");
                if !native_codex_mode {
                    stop(&mut broker);
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .envs(bash.as_ref().map(|path| {
                        (
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                            if mode.ends_with("_wrong_image") {
                                Path::new("/bin/true")
                            } else {
                                Path::new(path)
                            },
                        )
                    }))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                    .envs(native_codex_mode.then(|| (
                        "OULIPOLY_KERNEL_BROKER_PRIVATE_NATIVE_CODEX_SHA256_V1",
                        format!("{:x}", Sha256::digest(fs::read(&provider_image).unwrap())),
                    )))
                    .envs(root_h_delegate.then_some(("AGE319_PRIVATE_SELECTED_K_ROOT_H_V1", "1")))
                    .envs(root_h_notify.then_some(("AGE319_PRIVATE_BASH_ORIGINAL_NOTIFY_V1", "1")))
                    .envs(root_h_delegate.then_some(("AGE319_PRIVATE_BASH_SOURCE_SUCCESS_V1", "1")))
                    .envs(v3_mode.then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                        "1",
                    )))
                    .envs(v3_mode.then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                        config_home.join("oulipoly-agent-runner").to_str().unwrap(),
                    )))
                    .envs(
                        mode.starts_with("normal_model_provider_v3_quota_route")
                            .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
                    )
                    .envs(
                        v3_physical
                            .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")),
                    )
                    .envs(mode.ends_with("shared_reply_loss").then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_SHARED_Q_REPLY_V3_V1",
                        "1",
                    )))
                    .envs(
                        matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_route_reply_loss"
                                | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                                | "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss"
                        )
                        .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ROUTE_V3_REPLY_V1", "1")),
                    )
                    .envs(
                        native_lost_reply
                            .map(|stage| ("AGE319_PRIVATE_NATIVE_F_DROP_REPLY_V1", stage)),
                    )
                    .envs(mode.contains("resident_bash_after_append").then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_INTERACTIVE_AFTER_APPEND_V1",
                        "1",
                    )))
                    .envs((mode == "normal_model_provider_reply_loss").then_some((
                        "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
                        "1",
                    )))
                    .envs(
                        mode.ends_with("physical_reply_loss")
                            .then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
                                "1",
                            )),
                    )
                    .envs(
                        mode.ends_with("physical_post_k").then_some(
                            (
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_PROVIDER_POST_K_CAS_V3_V1",
                                "1",
                            ),
                        ),
                    )
                    .envs(
                        matches!(
                            mode.as_str(),
                            "normal_model_provider_quota_reply_loss"
                                | "normal_model_provider_v3_quota_reply_loss"
                        )
                        .then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ACCOUNT_EFFECT_REPLY_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_pty_physical_reply_loss").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_INTERACTIVE_K_REPLY_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_pty_physical_post_k_unknown").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_INTERACTIVE_POST_K_UNKNOWN_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_v3_quota_auth_reply_loss").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_AUTH_EFFECT_REPLY_V3_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_v3_quota_post_k").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_QUOTA_POST_K_CAS_V3_V1",
                            "1",
                        )),
                    )
                    .envs(
                        (mode == "normal_model_provider_v3_quota_auth_post_k").then_some((
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_FAIL_AUTH_POST_K_CAS_V3_V1",
                            "1",
                        )),
                    )
                    .stderr(Stdio::from(
                        File::create(temp.path().join("handoff-restart.log")).unwrap(),
                    ))
                    .spawn()
                    .unwrap();
                }
                let fresh_socket = socket.with_file_name("v30.sock");
                eventually(|| {
                    broker.try_wait().unwrap().is_some()
                        || protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                });
                assert!(
                    broker.try_wait().unwrap().is_none(),
                    "fresh broker restart: {}",
                    fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                );
                fs::write(gate.join("child-retry"), b"yes").unwrap();
                eventually(|| {
                    gate.join("child-retried").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("child-retried").exists(),
                    "retry: {}",
                    fs::read_to_string(&err).unwrap()
                );
                assert_eq!(
                    fs::read_dir(broker_state.join("released-handoffs"))
                        .unwrap()
                        .count(),
                    1
                );
                let old = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let pending: i64 = old
                    .query_row(
                        "SELECT count(*) FROM mailbox WHERE session_id='old-pending'
                     AND handle='old-unacked' AND delivered_at IS NULL",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    pending,
                    if native_f_turn && !old_pending_debt {
                        0
                    } else {
                        1
                    },
                    "historical pending debt differs from fixture mode"
                );
                let old_state_path = data.join("state.db");
                let old_state_before = fs::read(&old_state_path).unwrap();
                let old_wal_path = data.join("state.db-wal");
                let old_wal_before = fs::read(&old_wal_path).ok();
                let v29_main_before = fs::read(&historical_sidecar).unwrap();
                let v29_wal = historical_data.join("pid-identity.db-wal");
                let v29_wal_before = fs::read(&v29_wal).ok();
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                if closed_refusal_probe && !old_pending_debt {
                    let concurrent = Command::new(&runner)
                        .arg("--help")
                        .env("OULIPOLY_DATA_DIR", &data)
                        .env("OULIPOLY_CONFIG_HOME", &config_home)
                        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env("AGE319_PRIVATE_OFFLINE_ROOT_V1", "1")
                        .env_remove("LD_LIBRARY_PATH")
                        .output()
                        .unwrap();
                    assert!(
                        !concurrent.status.success(),
                        "concurrent second E succeeded"
                    );
                    assert_eq!(
                        fs::read_dir(broker_state.join("entries"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|entry| entry
                                .path()
                                .extension()
                                .is_some_and(|ext| ext == "json"))
                            .count(),
                        1,
                        "concurrent second E reserved an entry: {}",
                        String::from_utf8_lossy(&concurrent.stderr)
                    );
                }
                if mode.ends_with("shared_pending") {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    eventually(|| {
                        gate.join("started-quota").exists() || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("started-quota").exists(),
                        "first quota K absent: {}",
                        fs::read_to_string(&err).unwrap()
                    );
                    let second_err = temp.path().join("second-pending.err");
                    let mut second = Command::new(&runner)
                        .args([
                            "--model",
                            "configured-model-two",
                            "--pin-provider",
                            "local-two",
                            "hello second",
                        ])
                        .env("OULIPOLY_DATA_DIR", &data)
                        .env("OULIPOLY_CONFIG_HOME", &config_home)
                        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                        .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                        .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                        .env("AGE319_PRIVATE_SECOND_RESULT_V1", "1")
                        .env(
                            "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                            gate.join("provider-effect"),
                        )
                        .env_remove("LD_LIBRARY_PATH")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::from(File::create(&second_err).unwrap()))
                        .spawn()
                        .unwrap();
                    eventually(|| second.try_wait().unwrap().is_some());
                    assert!(!second.wait().unwrap().success());
                    assert!(
                        fs::read_to_string(&second_err)
                            .unwrap()
                            .contains("OULIPOLY_KERNEL_ENTRY_GAP=v30 E did not reserve a root"),
                        "second pending actor: {}",
                        fs::read_to_string(&second_err).unwrap()
                    );
                    let effects = provider_dir.join("account-effects");
                    assert_eq!(
                        fs::read_dir(&effects)
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|item| item
                                .file_name()
                                .to_string_lossy()
                                .ends_with("-quota-first"))
                            .count(),
                        1
                    );
                    assert!(
                        !effects
                            .join(format!("{}-1-quota-first/result.json", receipt.handoff_id))
                            .exists()
                    );
                    fs::write(gate.join("finish-quota"), b"yes").unwrap();
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(entry.wait().unwrap().success());
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    return;
                }
                if mode == "normal_model_provider_v3_quota_auth_shared" {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    eventually(|| {
                        gate.join("shared-auth-first-ready").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("shared-auth-first-ready").exists(),
                        "first invalid Q absent: {}",
                        fs::read_to_string(&err).unwrap()
                    );
                    fs::write(gate.join("shared-auth-first-go"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("started-auth").exists() || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("started-auth").exists(),
                        "first auth K absent: {}",
                        fs::read_to_string(&err).unwrap()
                    );
                    let second_err = temp.path().join("second-auth.err");
                    let mut second = Command::new(&runner)
                        .args([
                            "--model",
                            "configured-model-two",
                            "--pin-provider",
                            "local-two",
                            "hello second",
                        ])
                        .env("OULIPOLY_DATA_DIR", &data)
                        .env("OULIPOLY_CONFIG_HOME", &config_home)
                        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                        .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                        .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                        .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                        .env("AGE319_PRIVATE_SECOND_RESULT_V1", "1")
                        .env(
                            "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                            gate.join("provider-effect"),
                        )
                        .env_remove("LD_LIBRARY_PATH")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::from(File::create(&second_err).unwrap()))
                        .spawn()
                        .unwrap();
                    eventually(|| second.try_wait().unwrap().is_some());
                    assert!(!second.wait().unwrap().success());
                    assert!(
                        fs::read_to_string(&second_err)
                            .unwrap()
                            .contains("OULIPOLY_KERNEL_ENTRY_GAP=v30 E did not reserve a root"),
                        "second auth actor: {}",
                        fs::read_to_string(&second_err).unwrap()
                    );
                    assert_eq!(
                        fs::read_dir(broker_state.join("released-handoffs"))
                            .unwrap()
                            .count(),
                        1
                    );
                    let first_auth = provider_dir
                        .join("account-effects")
                        .join(format!("{}-1-auth-refresh", receipt.handoff_id));
                    assert_eq!(
                        fs::read_dir(&first_auth)
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|item| item
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json"))
                            .count(),
                        1,
                        "invalid-Q auth spent another physical K"
                    );
                    fs::write(gate.join("finish-auth"), b"yes").unwrap();
                    eventually(|| {
                        gate.join("shared-auth-first-refreshed").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("shared-auth-first-refreshed").exists(),
                        "auth Q not refreshed: {}",
                        fs::read_to_string(&err).unwrap()
                    );
                    fs::write(gate.join("shared-auth-first-retry-go"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    assert!(
                        fs::read_to_string(&err)
                            .unwrap()
                            .contains("v3 auth already spent after rejection"),
                        "{}",
                        fs::read_to_string(&err).unwrap()
                    );
                    assert!(
                        !provider_dir
                            .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                            .exists()
                    );
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    return;
                }
                if matches!(
                    mode.as_str(),
                    "normal_model_provider_v3_quota_restart"
                        | "normal_model_provider_v3_quota_auth_restart"
                ) {
                    let auth_restart = mode == "normal_model_provider_v3_quota_auth_restart";
                    let effect_dir = broker_state
                        .join("v30/fresh-provider/account-effects")
                        .join(format!(
                            "{}-1-{}",
                            receipt.handoff_id,
                            if auth_restart {
                                "auth-refresh"
                            } else {
                                "quota-first"
                            }
                        ));
                    eventually(|| {
                        effect_dir.exists()
                            && fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .any(|entry| {
                                    entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".consumed.json")
                                })
                            && gate
                                .join(if auth_restart {
                                    "started-auth"
                                } else {
                                    "started-quota"
                                })
                                .exists()
                    });
                    assert!(!effect_dir.join("result.json").exists());
                    stop(&mut broker);
                    let restart_log = temp.path().join("v3-quota-inflight-restart.log");
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                            "1",
                        )
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                            config_home.join("oulipoly-agent-runner"),
                        )
                        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                        .spawn()
                        .unwrap();
                    let fresh_socket = socket.with_file_name("v30.sock");
                    eventually(|| {
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok(),
                        "v3 pending quota restart refused: {}",
                        fs::read_to_string(&restart_log).unwrap()
                    );
                    fs::write(
                        gate.join(if auth_restart {
                            "finish-auth"
                        } else {
                            "finish-quota"
                        }),
                        b"yes",
                    )
                    .unwrap();
                }
                if matches!(
                    mode.as_str(),
                    "normal_model_provider_v3_quota_route_reply_loss"
                        | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                        | "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss"
                ) {
                    let route = broker_state
                        .join("v30/fresh-provider")
                        .join(format!("{}.route-selection.json", receipt.handoff_id));
                    eventually(|| {
                        gate.join("route-reply-dropped").exists() && route.exists()
                            || entry.try_wait().unwrap().is_some()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("route-reply-dropped").exists() && route.exists(),
                        "route reply did not drop: runner: {}; broker: {}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(&broker_log).unwrap()
                    );
                    let before = fs::read(&route).unwrap();
                    stop(&mut broker);
                    let restart_log = temp.path().join("v3-route-inflight-restart.log");
                    broker =
                        Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner"),
                            )
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1",
                                "1",
                            )))
                            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                            .spawn()
                            .unwrap();
                    let fresh_socket = socket.with_file_name("v30.sock");
                    eventually(|| {
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok(),
                        "v3 route restart refused: {}",
                        fs::read_to_string(&restart_log).unwrap()
                    );
                    assert_eq!(fs::read(&route).unwrap(), before);
                    fs::write(gate.join("route-restarted"), b"yes").unwrap();
                }
                if mode.ends_with("physical_source_changed") {
                    eventually(|| gate.join("v3-provider-ready").exists());
                    fs::write(
                        config_home.join("oulipoly-agent-runner/models/configured-model.toml"),
                        "[[providers]]\nname = \"local\"\n",
                    )
                    .unwrap();
                    fs::write(gate.join("v3-provider-continue"), b"yes").unwrap();
                }
                if mode.ends_with("physical_bad_plan")
                    || mode.ends_with("physical_bad_actor")
                    || mode.ends_with("physical_source_changed")
                {
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    assert!(
                        !provider_dir
                            .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                            .exists()
                    );
                    assert!(!gate.join("provider-effect").exists());
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    return;
                }
                if provider_negative {
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    let stderr = fs::read_to_string(&err).unwrap();
                    if manual_route {
                        let provider_dir = broker_state.join("v30/fresh-provider");
                        let route_file = provider_dir
                            .join(format!("{}.route-selection.json", receipt.handoff_id));
                        let healthy = matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_route_manual_healthy"
                                | "normal_model_provider_v3_quota_route_manual_cross_model"
                                | "normal_model_provider_v3_quota_route_manual_refresh"
                                | "normal_model_provider_v3_quota_route_manual_reply_loss"
                                | "normal_model_provider_v3_quota_route_manual_route_reply_loss"
                        );
                        assert_eq!(route_file.exists(), healthy, "runner: {stderr}");
                        if healthy {
                            let route: serde_json::Value =
                                serde_json::from_slice(&fs::read(&route_file).unwrap()).unwrap();
                            assert_eq!(route["selection"]["account"], "local");
                            assert_eq!(route["selection"]["account_identity"], "physical-local");
                            assert_eq!(route["sequence"], 0);
                            if mode == "normal_model_provider_v3_quota_route_manual_refresh" {
                                assert_eq!(
                                    route["selection"]["quota_remaining_basis_points"],
                                    7600
                                );
                            }
                            assert!(
                                stderr.contains(
                                    "v3 route, cancellation and provider K writers are closed"
                                ),
                                "{stderr}"
                            );
                            let actor_selection: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("v3-route-selection.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(actor_selection, route["selection"]);
                        } else {
                            assert!(
                                stderr.contains(
                                    if matches!(
                                        mode.as_str(),
                                        "normal_model_provider_v3_quota_route_manual_pending"
                                            | "normal_model_provider_v3_quota_route_manual_unknown"
                                    ) {
                                        "v3 route has unknown physical-account debt"
                                    } else {
                                        "v3 route has no eligible account or pin"
                                    }
                                ),
                                "{stderr}"
                            );
                        }
                        let effect_parent = provider_dir.join("account-effects");
                        let physical_effects: Vec<_> = if effect_parent.exists() {
                            fs::read_dir(&effect_parent)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry.path().join("intent.json").exists())
                                .map(|entry| entry.file_name())
                                .collect()
                        } else {
                            Vec::new()
                        };
                        assert!(
                            physical_effects.is_empty(),
                            "route actor spent a duplicate physical Q K: {physical_effects:?}"
                        );
                        assert!(
                            !provider_dir
                                .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                                .exists()
                        );
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                        assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                        assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                        stop(&mut broker);
                        let restart_log = temp.path().join("manual-route-restart.log");
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner"),
                            )
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                            .spawn()
                            .unwrap();
                        let fresh_socket = socket.with_file_name("v30.sock");
                        eventually(|| {
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                                || broker.try_wait().unwrap().is_some()
                        });
                        assert!(
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate)
                                .is_ok(),
                            "manual-route restart refused: {}",
                            fs::read_to_string(&restart_log).unwrap()
                        );
                        stop(&mut broker);
                        return;
                    }
                    let reason = if matches!(
                        mode.as_str(),
                        "normal_model_provider_v3_quota_post_k"
                            | "normal_model_provider_v3_quota_auth_post_k"
                    ) {
                        "fresh account effect unknown"
                    } else if matches!(
                        mode.as_str(),
                        "normal_model_provider_v3_quota_route_invalid"
                            | "normal_model_provider_v3_quota_route_full"
                            | "normal_model_provider_v3_quota_route_stale"
                            | "normal_model_provider_v3_quota_route_auth_failed"
                    ) {
                        "v3 route has no eligible account or pin"
                    } else if v3_quota {
                        "v3 route, cancellation and provider K writers are closed"
                    } else {
                        match mode.as_str() {
                            "normal_model_provider_bad_config" => {
                                "fresh provider \"local\" absent before K"
                            }
                            "normal_model_provider_unsupported" => {
                                "fresh pool has incompatible prompt modes before K"
                            }
                            "normal_model_provider_v3_closed" => {
                                "v3 route, cancellation and provider K writers are closed"
                            }
                            "normal_model_provider_quota" => {
                                "fresh route has no eligible account or pin"
                            }
                            "normal_model_provider_auth" => {
                                "fresh route has no eligible account or pin"
                            }
                            _ => unreachable!(),
                        }
                    };
                    assert!(
                        stderr.contains(reason),
                        "runner: {stderr}; broker status: {:?}; broker: {}; restarted: {}",
                        broker.try_wait().unwrap(),
                        fs::read_to_string(&broker_log).unwrap(),
                        fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                    );
                    assert!(!gate.join("provider-effect").exists());
                    assert!(!gate.join("provider-runtime-result").exists());
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_v3_quota_route"
                            | "normal_model_provider_v3_quota_route_reply_loss"
                    ) {
                        let route: serde_json::Value = serde_json::from_slice(
                            &fs::read(
                                provider_dir
                                    .join(format!("{}.route-selection.json", receipt.handoff_id)),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(route["selection"]["account"], "local");
                        assert_eq!(route["selection"]["index"], 1);
                        assert_eq!(route["sequence"], 0, "pinned choice advanced RR");
                        let actor_selection: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("v3-route-selection.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(actor_selection, route["selection"]);
                    }
                    if v3_quota && !manual_setup {
                        let effect_dir = provider_dir
                            .join("account-effects")
                            .join(format!("{}-1-quota-first", receipt.handoff_id));
                        let intent: serde_json::Value = serde_json::from_slice(
                            &fs::read(effect_dir.join("intent.json")).unwrap(),
                        )
                        .unwrap();
                        if mode != "normal_model_provider_v3_quota_post_k" {
                            let result: serde_json::Value = serde_json::from_slice(
                                &fs::read(effect_dir.join("result.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(result["effect_id"], intent["id"]);
                            assert_eq!(
                                result["outcome"],
                                if mode == "normal_model_provider_v3_quota_invalid"
                                    || mode == "normal_model_provider_v3_quota_route_invalid"
                                    || mode == "normal_model_provider_v3_quota_route_auth_failed"
                                    || mode.starts_with("normal_model_provider_v3_quota_auth")
                                {
                                    "invalid"
                                } else {
                                    "valid_windows"
                                }
                            );
                            assert_eq!(result["state"], "drained");
                            if mode == "normal_model_provider_v3_quota_full"
                                || mode == "normal_model_provider_v3_quota_route_full"
                            {
                                assert_eq!(
                                    result["windows"][0]["used_percent"].as_f64(),
                                    Some(100.0)
                                );
                            }
                            if mode == "normal_model_provider_v3_quota_stale"
                                || mode == "normal_model_provider_v3_quota_route_stale"
                            {
                                assert_eq!(
                                    result["windows"][0]["resets_at"],
                                    "2020-01-01T00:00:00Z"
                                );
                            }
                        } else {
                            assert!(!effect_dir.join("result.json").exists());
                        }
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "v3 quota probe spent more than one K"
                        );
                        if mode.starts_with("normal_model_provider_v3_quota_auth") {
                            let auth_dir = provider_dir
                                .join("account-effects")
                                .join(format!("{}-1-auth-refresh", receipt.handoff_id));
                            if mode == "normal_model_provider_v3_quota_auth_post_k" {
                                assert!(!auth_dir.join("result.json").exists());
                                assert!(!gate.join("auth-ok").exists());
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".consumed.json"))
                                        .count(),
                                    1
                                );
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".drain.json"))
                                        .count(),
                                    0
                                );
                            } else {
                                let auth: serde_json::Value = serde_json::from_slice(
                                    &fs::read(auth_dir.join("result.json")).unwrap(),
                                )
                                .unwrap();
                                assert_eq!(auth["state"], "drained");
                                assert_eq!(
                                    auth["outcome"],
                                    if mode == "normal_model_provider_v3_quota_auth_failed" {
                                        "failed"
                                    } else {
                                        "refreshed"
                                    }
                                );
                                if mode != "normal_model_provider_v3_quota_auth_failed" {
                                    assert_eq!(fs::read(gate.join("auth-ok")).unwrap(), b"x");
                                }
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".consumed.json"))
                                        .count(),
                                    1
                                );
                                assert_eq!(
                                    fs::read_dir(&auth_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".drain.json"))
                                        .count(),
                                    1
                                );
                                let retry_dir = provider_dir
                                    .join("account-effects")
                                    .join(format!("{}-1-quota-retry", receipt.handoff_id));
                                if mode == "normal_model_provider_v3_quota_auth_failed" {
                                    assert!(!retry_dir.exists());
                                } else {
                                    let retry: serde_json::Value = serde_json::from_slice(
                                        &fs::read(retry_dir.join("result.json")).unwrap(),
                                    )
                                    .unwrap();
                                    assert_eq!(retry["outcome"], "valid_windows");
                                }
                            }
                        }
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".drain.json"))
                                .count(),
                            usize::from(mode != "normal_model_provider_v3_quota_post_k"),
                            "v3 quota Q state differs"
                        );
                        assert!(provider_dir.join("index-v1/manifest.json").exists());
                        if matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_reply_loss"
                                | "normal_model_provider_v3_quota_auth_reply_loss"
                        ) {
                            assert!(gate.join("account-effect-reply-dropped").exists());
                        }
                    }
                    assert!(
                        !provider_dir
                            .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                            .exists(),
                        "negative quota authorized provider K"
                    );
                    if !mode.starts_with("normal_model_provider_v3_quota_auth")
                        && !matches!(
                            mode.as_str(),
                            "normal_model_provider_quota"
                                | "normal_model_provider_auth"
                                | "normal_model_provider_v3_closed"
                                | "normal_model_provider_v3_quota"
                                | "normal_model_provider_v3_quota_reply_loss"
                                | "normal_model_provider_v3_quota_post_k"
                                | "normal_model_provider_v3_quota_restart"
                                | "normal_model_provider_v3_quota_invalid"
                                | "normal_model_provider_v3_quota_full"
                                | "normal_model_provider_v3_quota_stale"
                                | "normal_model_provider_v3_quota_route"
                                | "normal_model_provider_v3_quota_route_reply_loss"
                                | "normal_model_provider_v3_quota_route_invalid"
                                | "normal_model_provider_v3_quota_route_full"
                                | "normal_model_provider_v3_quota_route_stale"
                                | "normal_model_provider_v3_quota_route_auth_failed"
                                | "normal_model_provider_v3_quota_auth"
                        )
                    {
                        // The joined broker always holds its index admission
                        // lease, even when an ordinary request refuses before
                        // provider K. That directory carries no effect/grant.
                        assert!(
                            provider_dir
                                .join("index-v1/admission-protocol.json")
                                .exists()
                        );
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry.file_name() != "index-v1")
                                .count(),
                            0,
                            "pre-K refusal created provider grant/effect"
                        );
                    }
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                    assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                    if v3_quota {
                        stop(&mut broker);
                        let restart_log = temp.path().join("v3-quota-restart.log");
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )
                            .env(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner"),
                            )
                            .envs(
                                mode.starts_with("normal_model_provider_v3_quota_route")
                                    .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
                            )
                            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                            .spawn()
                            .unwrap();
                        let fresh_socket = socket.with_file_name("v30.sock");
                        eventually(|| {
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                                || broker.try_wait().unwrap().is_some()
                        });
                        assert!(
                            protocol::request_at(&fresh_socket, Operation::ObserveEntryGate)
                                .is_ok(),
                            "v3 quota restart refused: {}",
                            fs::read_to_string(&restart_log).unwrap()
                        );
                        let effect_dir = provider_dir
                            .join("account-effects")
                            .join(format!("{}-1-quota-first", receipt.handoff_id));
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "v3 quota restart replayed physical K"
                        );
                        if mode.starts_with("normal_model_provider_v3_quota_auth") {
                            let auth_dir = provider_dir
                                .join("account-effects")
                                .join(format!("{}-1-auth-refresh", receipt.handoff_id));
                            assert_eq!(
                                fs::read_dir(&auth_dir)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".consumed.json"))
                                    .count(),
                                1,
                                "v3 auth restart replayed physical K"
                            );
                        }
                    }
                    stop(&mut broker);
                    return;
                }
                if mode.ends_with("physical_post_k") {
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    let grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir.join(format!("{}.fresh-grant.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let id = grant["id"].as_str().unwrap();
                    assert!(provider_dir.join(format!("{id}.consumed.json")).exists());
                    assert!(!provider_dir.join(format!("{id}.attach.json")).exists());
                    assert!(!provider_dir.join(format!("{id}.drain.json")).exists());
                    assert!(!gate.join("provider-effect").exists());
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    stop(&mut broker);
                    let restart_log = temp.path().join("v3-provider-post-k-restart.log");
                    broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                            "1",
                        )
                        .env(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                            config_home.join("oulipoly-agent-runner"),
                        )
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")
                        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                        .spawn()
                        .unwrap();
                    let fresh_socket = socket.with_file_name("v30.sock");
                    eventually(|| {
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok()
                            || broker.try_wait().unwrap().is_some()
                    });
                    assert!(
                        protocol::request_at(&fresh_socket, Operation::ObserveEntryGate).is_ok(),
                        "post-K debt restart refused: {}",
                        fs::read_to_string(&restart_log).unwrap()
                    );
                    assert!(provider_dir.join(format!("{id}.consumed.json")).exists());
                    assert!(!provider_dir.join(format!("{id}.drain.json")).exists());
                    stop(&mut broker);
                    return;
                }
                if mode == "normal_model_provider_auth_after_healthy" {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    eventually(|| {
                        gate.join("provider-effect").exists() || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("provider-effect").exists(),
                        "runner: {}; broker: {}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                    );
                    let route: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir
                                .join(format!("{}.route-selection.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(route["selection"]["account"], "local");
                    let grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir.join(format!("{}.fresh-grant.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let grant_id = grant["id"].as_str().unwrap();
                    eventually(|| provider_dir.join(format!("{grant_id}.exit.json")).exists());
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    assert!(
                        provider_dir
                            .join(format!("{grant_id}.terminal.json"))
                            .exists(),
                        "runner stderr: {}; witness: {:?}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(gate.join("provider-runtime-result"))
                    );
                    let terminal: serde_json::Value = serde_json::from_slice(
                        &fs::read(provider_dir.join(format!("{grant_id}.terminal.json"))).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(terminal["outcome"], "auth_rejected");
                    assert!(provider_dir.join(format!("{grant_id}.drain.json")).exists());
                    let effects = provider_dir.join("account-effects");
                    for (kind, outcome) in [
                        ("quota-first", "valid_windows"),
                        ("auth-refresh", "refreshed"),
                        ("quota-retry", "valid_windows"),
                    ] {
                        let dir = effects.join(format!("{}-1-{kind}", receipt.handoff_id));
                        let result: serde_json::Value =
                            serde_json::from_slice(&fs::read(dir.join("result.json")).unwrap())
                                .unwrap();
                        assert_eq!(result["outcome"], outcome);
                        assert_eq!(
                            fs::read_dir(dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1
                        );
                    }
                    assert_eq!(fs::read(gate.join("auth-ok")).unwrap(), b"x");
                    assert_eq!(
                        fs::read_dir(&provider_dir)
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|entry| entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json"))
                            .count(),
                        1
                    );
                    let mapped: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("provider-runtime-result")).unwrap_or_else(|error| {
                            panic!(
                                "mapped result absent: {error}; runner={} broker={}",
                                fs::read_to_string(&err).unwrap_or_default(),
                                fs::read_to_string(&broker_log).unwrap_or_default()
                            )
                        }),
                    )
                    .unwrap();
                    assert_eq!(mapped["auth_after_provider_q"], true);
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                    assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                    stop(&mut broker);
                    return;
                }
                if mode == "normal_model_provider_auth_after_healthy" {
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    eventually(|| {
                        gate.join("provider-effect").exists() || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("provider-effect").exists(),
                        "runner: {}; broker: {}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                    );
                    let route: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir
                                .join(format!("{}.route-selection.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(route["selection"]["account"], "local");
                    let grant: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            provider_dir.join(format!("{}.fresh-grant.json", receipt.handoff_id)),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    let grant_id = grant["id"].as_str().unwrap();
                    eventually(|| provider_dir.join(format!("{grant_id}.exit.json")).exists());
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    assert!(!entry.wait().unwrap().success());
                    assert!(
                        provider_dir
                            .join(format!("{grant_id}.terminal.json"))
                            .exists(),
                        "runner stderr: {}; witness: {:?}",
                        fs::read_to_string(&err).unwrap(),
                        fs::read_to_string(gate.join("provider-runtime-result"))
                    );
                    let terminal: serde_json::Value = serde_json::from_slice(
                        &fs::read(provider_dir.join(format!("{grant_id}.terminal.json"))).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(terminal["outcome"], "auth_rejected");
                    assert!(provider_dir.join(format!("{grant_id}.drain.json")).exists());
                    let effects = provider_dir.join("account-effects");
                    for (kind, outcome) in [
                        ("quota-first", "valid_windows"),
                        ("auth-refresh", "refreshed"),
                        ("quota-retry", "valid_windows"),
                    ] {
                        let dir = effects.join(format!("{}-1-{kind}", receipt.handoff_id));
                        let result: serde_json::Value =
                            serde_json::from_slice(&fs::read(dir.join("result.json")).unwrap())
                                .unwrap();
                        assert_eq!(result["outcome"], outcome);
                        assert_eq!(
                            fs::read_dir(dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1
                        );
                    }
                    assert_eq!(fs::read(gate.join("auth-ok")).unwrap(), b"x");
                    assert_eq!(
                        fs::read_dir(&provider_dir)
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|entry| entry
                                .file_name()
                                .to_string_lossy()
                                .ends_with(".consumed.json"))
                            .count(),
                        1
                    );
                    let mapped: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("provider-runtime-result")).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(mapped["auth_after_provider_q"], true);
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                    assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                    stop(&mut broker);
                    return;
                }
                if provider_mode {
                    if native_codex_mode {
                        let until = Instant::now() + Duration::from_secs(90);
                        while !gate.join("source-postcommit-ready").exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            gate.join("source-postcommit-ready").exists(),
                            "real K Bash did not reach original H: entry={} broker={} h_client={} h_broker={} h_guardian={} h_state={} h_error={} h_diagnostic={}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default(),
                            fs::read_to_string(gate.join("delegated-h-client-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("delegated-h-broker-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("delegated-h-guardian-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("h-source-intent-dir"))
                                .unwrap_or_default(),
                            fs::read(
                                std::path::PathBuf::from(
                                    fs::read_to_string(gate.join("h-source-intent-dir"))
                                        .unwrap_or_default()
                                )
                                .join("meta.json")
                            )
                            .ok()
                            .and_then(
                                |bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            )
                            .and_then(|meta| meta["error"].as_str().map(ToOwned::to_owned))
                            .unwrap_or_default(),
                            fs::read_to_string(
                                std::path::PathBuf::from(
                                    fs::read_to_string(gate.join("h-source-intent-dir"))
                                        .unwrap_or_default()
                                )
                                .join("root-work-diagnostic-v1.jsonl")
                            )
                            .unwrap_or_default(),
                        );
                        let native: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("native-k-control-readback.json")).unwrap(),
                        )
                        .unwrap();
                        let mut lane = FreshV30Lane::open_at(&broker_state).unwrap();
                        let child = lane.read_bash_child(&bash_request).unwrap().unwrap();
                        let (root, _) = lane.released_handoff_for_root(&child.root_id).unwrap();
                        let k_pid = native["provider_pid"].as_i64().unwrap() as i32;
                        let mut ancestor = child.actor.host_pid;
                        let mut found_k = false;
                        for _ in 0..16 {
                            if ancestor == k_pid {
                                found_k = true;
                                break;
                            }
                            let current =
                                oulipoly_kernel_broker::identity::PinnedProcess::open(ancestor)
                                    .unwrap();
                            let status =
                                fs::read_to_string(format!("/proc/{ancestor}/status")).unwrap();
                            let parent: i32 = status
                                .lines()
                                .find_map(|line| line.strip_prefix("PPid:"))
                                .unwrap()
                                .trim()
                                .parse()
                                .unwrap();
                            assert!(parent > 0, "Bash ancestry escaped selected K");
                            let parent_pin =
                                oulipoly_kernel_broker::identity::PinnedProcess::open(parent)
                                    .unwrap();
                            assert!(current.direct_child_of(&parent_pin).unwrap());
                            ancestor = parent;
                        }
                        assert!(found_k, "real Bash is not descended from selected Codex K");
                        let selected =
                            oulipoly_kernel_broker::identity::PinnedProcess::open(k_pid).unwrap();
                        assert_eq!(
                            selected.starttime_ticks,
                            native["provider_starttime"].as_u64().unwrap()
                        );
                        assert_eq!(child.parent_work_grant_id, native["grant_id"]);
                        assert_eq!(child.parent_work_id, native["work_id"]);
                        let pending: i64 = rusqlite::Connection::open(&historical_sidecar)
                            .unwrap()
                            .query_row(
                                "SELECT count(*) FROM mailbox WHERE session_id!='old-pending'",
                                [],
                                |r| r.get(0),
                            )
                            .unwrap();
                        assert_eq!(pending, 0, "pending original W created notification row");
                        fs::write(gate.join("source-postcommit-notify"), b"schedule").unwrap();
                        let until = Instant::now() + Duration::from_secs(90);
                        while !gate.join("native-bash-result.json").exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            gate.join("native-bash-result.json").exists(),
                            "real K Bash result absent: entry={} broker={} h_client={} h_broker={} h_guardian={} postcommit={} driver={} source_launch={} h_diagnostic={}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default(),
                            fs::read_to_string(gate.join("delegated-h-client-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("delegated-h-broker-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("delegated-h-guardian-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("source-guardian-postcommit-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("source-driver-postcommit-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(gate.join("source-launch-stage"))
                                .unwrap_or_default(),
                            fs::read_to_string(
                                std::path::PathBuf::from(
                                    fs::read_to_string(gate.join("h-source-intent-dir"))
                                        .unwrap_or_default()
                                )
                                .join("root-work-diagnostic-v1.jsonl")
                            )
                            .unwrap_or_default()
                        );
                        let result: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("native-bash-result.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(result["exitCode"], 0);
                        let report: serde_json::Value =
                            serde_json::from_str(result["stdout"].as_str().unwrap().trim())
                                .unwrap();
                        assert_eq!(report["child"]["request_id"], bash_request);
                        assert_eq!(report["original_h_submitted"], true);
                        assert_eq!(
                            report["root_h_consumption"]["selected_k"]["grant_id"],
                            native["grant_id"]
                        );
                        assert_eq!(report["fresh_source_w"]["request_id"], bash_request);
                        assert!(
                            report["broker_physical_q"]
                                .as_str()
                                .unwrap()
                                .starts_with("fresh-bash-physical-drained ")
                        );
                        let attempt: serde_json::Value = serde_json::from_slice(
                            &fs::read(physical_dir.join(format!(
                                "{}.native-bash-attempt.json",
                                native["grant_id"].as_str().unwrap()
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(attempt["request_id"], bash_request);
                        assert_eq!(attempt["native_session_id"], native["native_session_id"]);
                        assert!(
                            gate.join("native-bash-reply-dropped").exists(),
                            "native command reply was not lost"
                        );
                        assert_ne!(
                            native["native_session_id"],
                            lane.read_session(&root.d_key).unwrap().unwrap().session_id
                        );
                        let original = rusqlite::Connection::open(&data.join("state.db")).unwrap();
                        let decisions: i64 = original
                            .query_row(
                                "SELECT count(*) FROM invocation_completion_exact_source_decisions",
                                [],
                                |r| r.get(0),
                            )
                            .unwrap();
                        assert_eq!(decisions, 1);
                        let registrations: i64 = original
                            .query_row(
                                "SELECT count(*) FROM invocation_completion_v2_identity",
                                [],
                                |r| r.get(0),
                            )
                            .unwrap();
                        assert_eq!(registrations, 1);
                        let h_state = std::path::PathBuf::from(
                            fs::read_to_string(gate.join("h-source-intent-dir")).unwrap(),
                        );
                        assert_eq!(fs::read(h_state.join("rc")).unwrap(), b"0\n");
                        let h_grant: serde_json::Value = serde_json::from_slice(
                            &fs::read(h_state.join("root-work-broker-grant-v1.json")).unwrap(),
                        )
                        .unwrap();
                        let durable_h: serde_json::Value =
                            serde_json::from_slice(
                                &fs::read(broker_state.join("grants").join(format!(
                                    "{}.json",
                                    h_grant["grant_id"].as_str().unwrap()
                                )))
                                .unwrap(),
                            )
                            .unwrap();
                        assert_eq!(durable_h["consumed"], true);
                        let original_q = fs::read_dir(broker_state.join("works"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter_map(|entry| fs::read(entry.path()).ok())
                            .filter_map(|bytes| {
                                serde_json::from_slice::<serde_json::Value>(&bytes).ok()
                            })
                            .filter(|work| work["accepted_grant_id"] == h_grant["grant_id"])
                            .collect::<Vec<_>>();
                        assert_eq!(original_q.len(), 1, "original H physical Q absent");
                        assert_eq!(original_q[0]["work_id"], h_grant["work_id"]);
                        assert!(
                            original_q[0]["init_host_pid"]
                                .as_i64()
                                .is_some_and(|pid| pid > 0)
                        );
                        eventually(|| gate.join("source-adopted-started").exists());
                        if native_f_turn {
                            let roots =
                                oulipoly_kernel_broker::registry::RootRegistry::open(&broker_state)
                                    .unwrap();
                            let source =
                                SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                                    .unwrap();
                            let grant_id = source.records()[0].grant.grant_id.clone();
                            assert!(matches!(
                                source.observe(&grant_id).unwrap(),
                                SourceObservation::Live { .. }
                            ));
                            assert!(source.retirement(&grant_id).unwrap().is_none());
                            let accepted: oulipoly_kernel_broker::accepted_grant::GrantRecord =
                                serde_json::from_value(durable_h.clone()).unwrap();
                            let works = oulipoly_kernel_broker::WorkRegistry::open(
                                broker_state.join("works"),
                                &roots,
                            )
                            .unwrap();
                            assert!(
                                !broker_state
                                    .join("works")
                                    .join(format!("{}.retired.json", accepted.grant_id))
                                    .exists()
                            );
                            let expected: RootRecord = serde_json::from_slice(
                                &fs::read(broker_state.join(format!("{}.json", prepared.root_id)))
                                    .unwrap(),
                            )
                            .unwrap();
                            let refusal = works
                                .physical_q(
                                    &expected,
                                    &roots,
                                    &accepted,
                                    &broker_state.join("terminals"),
                                )
                                .unwrap_err();
                            assert!(refusal.to_string().contains("still live"), "{refusal}");
                            let before =
                                protocol::root_drain_readback_at(&socket, &expected, false)
                                    .unwrap();
                            let before: serde_json::Value = serde_json::from_str(
                                before.strip_prefix("root-drain-v1 ").unwrap().trim_end(),
                            )
                            .unwrap();
                            assert_eq!(before["source_physical_outstanding"], 1);
                            assert_eq!(before["work_outstanding"], 1);
                            assert_eq!(before["close_eligible"], false);
                            assert!(protocol::root_pid1_drain_at(&socket, &expected).is_err());
                        }
                        fs::write(gate.join("source-adopted-started.release"), b"drain").unwrap();
                        let original_session = lane.read_session(&root.d_key).unwrap().unwrap();
                        let side = rusqlite::Connection::open(
                            broker_state.join("sidecar/pid-identity.db"),
                        )
                        .unwrap();
                        eventually(|| {
                            side.query_row(
                                "SELECT count(*) FROM mailbox WHERE session_id=?1",
                                [&original_session.session_id],
                                |r| r.get::<_, i64>(0),
                            )
                            .is_ok_and(|count| count == 1)
                        });
                        let physical =
                            SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                                .unwrap();
                        assert_eq!(physical.records().len(), 1);
                        let source_grant = &physical.records()[0].grant;
                        let mut retained = BrokerSidecar::open_existing(
                            &broker_state.join("sidecar/pid-identity.db"),
                            &broker_state,
                        )
                        .unwrap();
                        let original_binding = retained
                            .read_consumed_source_candidate(source_grant)
                            .unwrap();
                        let original_source = original_binding.registration().unwrap();
                        assert_eq!(original_source.delivery_mode, "async");
                        assert_eq!(
                            original_source.owner_session_id,
                            original_session.session_id
                        );
                        assert_eq!(original_source.owner_invocation_uuid, root.invocation_uuid);
                        assert_ne!(
                            report["fresh_source_w"]["source_id"],
                            original_source.handle
                        );
                        assert_ne!(report["fresh_source_w"]["attempt_id"], root.invocation_uuid);
                        let custody =
                            oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody(
                                &mut retained,
                                &physical,
                                &source_grant.grant_id,
                            )
                            .unwrap();
                        assert_eq!(custody.root_id, prepared.root_id);
                        assert_eq!(custody.owner_generation, prepared.owner_generation);
                        assert_eq!(custody.session_id, original_session.session_id);
                        assert_eq!(custody.owner_invocation_uuid, root.invocation_uuid);
                        let payload = retained
                            .mailbox()
                            .completion_recovery_payload(&original_source.handle)
                            .unwrap()
                            .unwrap();
                        let payload_json: serde_json::Value =
                            serde_json::from_slice(&payload).unwrap();
                        assert_eq!(payload_json["kind"], "agent_bash_complete");
                        assert_eq!(payload_json["handle"], original_source.handle);
                        assert_eq!(
                            custody.payload_sha256,
                            format!("{:x}", Sha256::digest(&payload))
                        );
                        assert_eq!(custody.payload_byte_len, payload.len() as i64);
                        if native_turn_mode {
                            fs::write(gate.join("native-turn-ready"), b"row retained").unwrap();
                            let until = Instant::now() + Duration::from_secs(150);
                            while !gate.join("native-turn-receipt.json").exists()
                                && entry.try_wait().unwrap().is_none()
                                && Instant::now() < until
                            {
                                std::thread::sleep(Duration::from_millis(50));
                            }
                            assert!(
                                gate.join("native-turn-receipt.json").exists(),
                                "native turn receipt absent: entry={} broker={}",
                                fs::read_to_string(&err).unwrap_or_default(),
                                fs::read_to_string(&broker_log).unwrap_or_default()
                            );
                            let receipt: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("native-turn-receipt.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(receipt["readback"], native);
                            assert_eq!(receipt["status"], "completed");
                            assert_eq!(receipt["original_row_acknowledged"], native_ack_mode);
                            let attempt: serde_json::Value = serde_json::from_slice(
                                &fs::read(physical_dir.join(format!(
                                    "{}.native-turn-attempt.json",
                                    native["grant_id"].as_str().unwrap()
                                )))
                                .unwrap(),
                            )
                            .unwrap();
                            assert_eq!(attempt["readback"], native);
                            assert_eq!(attempt["nonce"], receipt["nonce"]);
                            assert!(
                                gate.join("native-turn-reply-dropped").exists(),
                                "native turn submission reply was not dropped"
                            );
                            let pending: i64 = side.query_row(
                                "SELECT count(*) FROM mailbox WHERE seq=?1 AND delivered_at IS NULL",
                                [custody.row_seq], |row| row.get(0),
                            ).unwrap();
                            if native_ack_mode {
                                assert_eq!(pending, 0, "native recipient did not ACK original row");
                                let (reason, digest): (String, String) = side.query_row(
                                    "SELECT l.acknowledgement_reason,g.ack_response_sha256
                                     FROM completion_event_listener l JOIN broker_native_recipient_grant g
                                     ON g.row_seq=l.mailbox_seq WHERE l.mailbox_seq=?1",
                                    [custody.row_seq], |row| Ok((row.get(0)?,row.get(1)?)),
                                ).unwrap();
                                assert_eq!(reason, "explicit_native_codex_assistant_ack");
                                assert_eq!(digest, receipt["ack_response_sha256"]);
                                let grant = retained
                                    .read_native_recipient_grant(&custody.source_grant_id)
                                    .unwrap()
                                    .unwrap();
                                let selected = grant.selected_k_json.clone();
                                assert_eq!(
                                    serde_json::from_str::<serde_json::Value>(&selected).unwrap(),
                                    native
                                );
                                let turn = receipt["turn_id"].as_str().unwrap();
                                let turn_digest = receipt["turn_receipt_sha256"].as_str().unwrap();
                                let response_digest =
                                    receipt["ack_response_sha256"].as_str().unwrap();
                                assert_eq!(grant.phase, "acked");
                                assert!(
                                    retained
                                        .acknowledge_native_recipient(
                                            &custody.source_grant_id,
                                            &selected,
                                            "wrong-token",
                                            turn,
                                            turn_digest,
                                            response_digest
                                        )
                                        .is_err()
                                );
                                assert!(
                                    retained
                                        .acknowledge_native_recipient(
                                            &custody.source_grant_id,
                                            &selected,
                                            &grant.delivery_token,
                                            "wrong-turn",
                                            turn_digest,
                                            response_digest
                                        )
                                        .is_err()
                                );
                                assert!(
                                    retained
                                        .acknowledge_native_recipient(
                                            &custody.source_grant_id,
                                            "wrong-K",
                                            &grant.delivery_token,
                                            turn,
                                            turn_digest,
                                            response_digest
                                        )
                                        .is_err()
                                );
                                let same = retained
                                    .acknowledge_native_recipient(
                                        &custody.source_grant_id,
                                        &selected,
                                        &grant.delivery_token,
                                        turn,
                                        turn_digest,
                                        response_digest,
                                    )
                                    .unwrap();
                                assert_eq!(same, grant, "lost ACK reply must read one settlement");
                                let attempts: i64 = side
                                    .query_row(
                                        "SELECT delivery_attempts FROM mailbox WHERE seq=?1",
                                        [custody.row_seq],
                                        |row| row.get(0),
                                    )
                                    .unwrap();
                                assert_eq!(attempts, 1);
                                if native_f_candidate {
                                    if native_f_turn {
                                        // The original ACK alone cannot retire either Q.
                                        assert!(
                                            !broker_state
                                                .join("source-physical")
                                                .join(format!(
                                                    "{}.retired.json",
                                                    custody.source_grant_id
                                                ))
                                                .exists()
                                        );
                                        assert!(
                                            !broker_state
                                                .join("works")
                                                .join(format!(
                                                    "{}.retired.json",
                                                    h_grant["grant_id"].as_str().unwrap()
                                                ))
                                                .exists()
                                        );
                                    }
                                    eventually(|| gate.join("bash-recipient-output").exists());
                                    let f_report: serde_json::Value = serde_json::from_slice(
                                        &fs::read(gate.join("bash-recipient-output")).unwrap(),
                                    )
                                    .unwrap();
                                    assert_eq!(f_report["mode"], "native_k_pending");
                                    let f = &f_report["grant"];
                                    let recipient =
                                        oulipoly_state::mailbox::FreshRecipientIdentity {
                                            host_pid: prepared.joined_child.host_pid,
                                            boot_id: prepared.joined_child.boot_id.clone(),
                                            starttime_ticks: prepared.joined_child.starttime_ticks,
                                            pidns_dev: prepared.joined_child.pidns_dev,
                                            pidns_ino: prepared.joined_child.pidns_ino,
                                        };
                                    let delivery_request =
                                        f_report["delivery_request_id"].as_str().unwrap();
                                    let token = f["delivery_token"].as_str().unwrap();
                                    let witness = lane
                                        .attest_native_k_f_candidate(
                                            &bash_request,
                                            delivery_request,
                                            token,
                                            &recipient,
                                            &custody.source_grant_id,
                                            &selected,
                                        )
                                        .unwrap();
                                    assert_eq!(witness.fresh.grant_id, f["grant_id"]);
                                    assert_eq!(witness.fresh.phase, "submitted");
                                    assert_eq!(witness.original_row_seq, custody.row_seq);
                                    assert_ne!(witness.fresh.source_id, custody.source_id);
                                    assert_eq!(
                                        witness.native_session_id,
                                        native["native_session_id"]
                                    );
                                    assert_eq!(witness.original_turn_id, turn);
                                    assert_eq!(witness.original_turn_receipt_sha256, turn_digest);
                                    assert_eq!(
                                        witness.fresh_parent_work_grant_id,
                                        native["grant_id"]
                                    );
                                    assert_eq!(
                                        witness,
                                        lane.attest_native_k_f_candidate(
                                            &bash_request,
                                            delivery_request,
                                            token,
                                            &recipient,
                                            &custody.source_grant_id,
                                            &selected,
                                        )
                                        .unwrap()
                                    );
                                    assert!(
                                        lane.attest_native_k_f_candidate(
                                            &bash_request,
                                            delivery_request,
                                            "wrong-token",
                                            &recipient,
                                            &custody.source_grant_id,
                                            &selected,
                                        )
                                        .is_err()
                                    );
                                    assert!(
                                        lane.attest_native_k_f_candidate(
                                            &uuid::Uuid::new_v4().to_string(),
                                            delivery_request,
                                            token,
                                            &recipient,
                                            &custody.source_grant_id,
                                            &selected,
                                        )
                                        .is_err()
                                    );
                                    assert!(
                                        lane.attest_native_k_f_candidate(
                                            &bash_request,
                                            delivery_request,
                                            token,
                                            &recipient,
                                            &custody.source_grant_id,
                                            "wrong-K",
                                        )
                                        .is_err()
                                    );
                                    if native_f_turn {
                                        eventually(|| {
                                            gate.join("native-f-turn-receipt.json").exists()
                                                || entry.try_wait().unwrap().is_some()
                                        });
                                        assert!(
                                            gate.join("native-f-turn-receipt.json").exists(),
                                            "native F receipt absent: entry={} broker={}",
                                            fs::read_to_string(&err).unwrap_or_default(),
                                            fs::read_to_string(&broker_log).unwrap_or_default()
                                        );
                                        let f_receipt: serde_json::Value = serde_json::from_slice(
                                            &fs::read(gate.join("native-f-turn-receipt.json"))
                                                .unwrap(),
                                        )
                                        .unwrap();
                                        let f_attempt: serde_json::Value = serde_json::from_slice(
                                            &fs::read(physical_dir.join(format!(
                                                "{}.native-f-attempt.json",
                                                witness.fresh.grant_id
                                            )))
                                            .unwrap(),
                                        )
                                        .unwrap();
                                        assert_eq!(
                                            f_receipt["format"],
                                            "age319-native-f-turn-receipt/v1"
                                        );
                                        assert_eq!(f_receipt["selected_k"], native);
                                        assert_eq!(
                                            f_receipt["fresh_grant_id"],
                                            witness.fresh.grant_id
                                        );
                                        assert_eq!(
                                            f_receipt["session_id"],
                                            witness.fresh.session_id
                                        );
                                        assert_eq!(f_receipt["row_seq"], witness.fresh.seq);
                                        assert_eq!(f_receipt["source_id"], witness.fresh.source_id);
                                        assert_eq!(
                                            f_receipt["payload_sha256"],
                                            witness.fresh.payload_sha256
                                        );
                                        assert_eq!(f_receipt["first_turn_id"], receipt["turn_id"]);
                                        assert_ne!(f_receipt["turn_id"], receipt["turn_id"]);
                                        assert_eq!(f_receipt["status"], "completed");
                                        assert_eq!(f_receipt["fresh_row_acknowledged"], true);
                                        assert_eq!(f_receipt["ack_reply_dropped"], true);
                                        assert_eq!(
                                            f_receipt["fresh_ack"]["basis"],
                                            "native_codex_f_assistant_ack"
                                        );
                                        let settled = lane
                                            .read_headless_native_f_ack(
                                                delivery_request,
                                                &recipient,
                                            )
                                            .unwrap()
                                            .unwrap();
                                        assert_eq!(
                                            serde_json::to_value(&settled).unwrap(),
                                            f_receipt["fresh_ack"]
                                        );
                                        let reopened =
                                            oulipoly_state::mailbox::FreshV30Lane::open_at(
                                                &broker_state,
                                            )
                                            .unwrap();
                                        assert_eq!(
                                            reopened
                                                .read_headless_native_f_ack(
                                                    delivery_request,
                                                    &recipient
                                                )
                                                .unwrap(),
                                            Some(settled.clone())
                                        );
                                        let mut duplicate = settled.proof.clone();
                                        assert!(
                                            lane.acknowledge_headless_native_f(&duplicate).is_err()
                                        );
                                        duplicate.delivery_token = "wrong-token".into();
                                        assert!(
                                            lane.acknowledge_headless_native_f(&duplicate).is_err()
                                        );
                                        assert_eq!(
                                            f_attempt["fresh_grant_id"],
                                            witness.fresh.grant_id
                                        );
                                        assert_eq!(f_attempt["delivery_token"], token);
                                        assert_eq!(
                                            f_attempt["delivery_request_id"],
                                            delivery_request
                                        );
                                        assert_eq!(f_attempt["source_id"], witness.fresh.source_id);
                                        assert_eq!(f_attempt["row_seq"], witness.fresh.seq);
                                        assert_eq!(f_attempt["nonce"], f_receipt["nonce"]);
                                        assert_eq!(
                                            f_attempt["envelope_sha256"],
                                            f_receipt["envelope_sha256"]
                                        );
                                        use base64::Engine as _;
                                        let reserved_payload =
                                            base64::engine::general_purpose::STANDARD
                                                .decode(
                                                    f_attempt["payload_base64"].as_str().unwrap(),
                                                )
                                                .unwrap();
                                        assert_eq!(
                                            format!("{:x}", Sha256::digest(&reserved_payload)),
                                            witness.fresh.payload_sha256
                                        );
                                        assert_eq!(
                                            reserved_payload.len() as i64,
                                            witness.fresh.payload_byte_len
                                        );
                                        assert!(
                                            f_receipt["assistant_response_sha256"]
                                                .as_str()
                                                .is_some_and(|v| v.len() == 64)
                                        );
                                        assert!(gate.join("native-f-turn-reply-dropped").exists());
                                        eventually(|| {
                                            gate.join("native-f-duplicate-refused").exists()
                                        });
                                        assert!(
                                            fs::read_to_string(
                                                gate.join("native-f-duplicate-refused")
                                            )
                                            .unwrap()
                                            .contains("no replay")
                                        );
                                    }
                                    let fresh_side = rusqlite::Connection::open(
                                        broker_state.join("v30/sidecar/pid-identity.db"),
                                    )
                                    .unwrap();
                                    let fresh_row: (Option<String>, i64) = fresh_side
                                        .query_row(
                                            "SELECT delivered_at,delivery_attempts FROM mailbox
                                         WHERE session_id=?1 AND seq=?2",
                                            rusqlite::params![
                                                witness.fresh.session_id,
                                                witness.fresh.seq
                                            ],
                                            |row| Ok((row.get(0)?, row.get(1)?)),
                                        )
                                        .unwrap();
                                    if native_f_turn {
                                        assert!(fresh_row.0.is_some());
                                        assert_eq!(fresh_row.1, 1);
                                        assert_eq!(
                                            lane.read_recipient_delivery(
                                                &witness.fresh.grant_id,
                                                &recipient
                                            )
                                            .unwrap()
                                            .unwrap()
                                            .phase,
                                            "acked"
                                        );
                                    } else {
                                        assert_eq!(fresh_row, (None, 0));
                                    }
                                    if native_f_turn {
                                        eventually(|| {
                                            lane.settle_private_root_terminal(
                                                &root,
                                                &recipient,
                                                &original_session,
                                            )
                                            .is_ok_and(
                                                |read| {
                                                    matches!(
                                                        read.execution_state.as_str(),
                                                        "success" | "failure"
                                                    ) && read.unknown_stages.is_empty()
                                                },
                                            )
                                        });
                                    }
                                    let terminal = lane
                                        .read_private_root_terminal(
                                            &root,
                                            &recipient,
                                            &original_session,
                                        )
                                        .unwrap();
                                    assert_eq!(
                                        terminal.notification_state,
                                        if native_f_turn {
                                            "acked"
                                        } else {
                                            "f_submitted_native_pending"
                                        }
                                    );
                                    if native_f_turn {
                                        assert_eq!(
                                            terminal.ack_basis.as_deref(),
                                            Some("native_codex_f_assistant_ack")
                                        );
                                        let expected: RootRecord = serde_json::from_slice(
                                            &fs::read(
                                                broker_state
                                                    .join(format!("{}.json", prepared.root_id)),
                                            )
                                            .unwrap(),
                                        )
                                        .unwrap();
                                        let before = protocol::root_drain_readback_at(
                                            &socket, &expected, true,
                                        )
                                        .unwrap();
                                        let before: serde_json::Value = serde_json::from_str(
                                            before
                                                .strip_prefix("root-drain-v1 ")
                                                .unwrap()
                                                .trim_end(),
                                        )
                                        .unwrap();
                                        assert_eq!(before["source_physical_outstanding"], 1);
                                        assert_eq!(before["source_physical_retired"], 0);
                                        let roots =
                                            oulipoly_kernel_broker::registry::RootRegistry::open(
                                                &broker_state,
                                            )
                                            .unwrap();
                                        let source_dir = broker_state.join("source-physical");
                                        let accepted: oulipoly_kernel_broker::accepted_grant::GrantRecord =
                                            serde_json::from_value(durable_h.clone()).unwrap();
                                        let works_dir = broker_state.join("works");
                                        let terminal_dir = broker_state.join("terminals");
                                        // The serving Broker writes both seals after the real
                                        // terminal and dual ACKs; this test only observes them.
                                        eventually(|| {
                                            SourcePhysicalRegistry::open(&source_dir)
                                                .ok()
                                                .and_then(|physical| {
                                                    physical
                                                        .retirement(&custody.source_grant_id)
                                                        .ok()
                                                        .flatten()
                                                })
                                                .is_some()
                                                && works_dir
                                                    .join(format!(
                                                        "{}.retired.json",
                                                        accepted.grant_id
                                                    ))
                                                    .exists()
                                        });
                                        let reopened =
                                            SourcePhysicalRegistry::open(&source_dir).unwrap();
                                        let retired = reopened
                                            .retirement(&custody.source_grant_id)
                                            .unwrap()
                                            .unwrap();
                                        assert_eq!(
                                            retired.source_grant_id,
                                            custody.source_grant_id
                                        );
                                        assert_ne!(retired.original_turn_id, retired.fresh_turn_id);
                                        let retained = BrokerSidecar::open_existing(
                                            &broker_state.join("sidecar/pid-identity.db"),
                                            &broker_state,
                                        )
                                        .unwrap();
                                        let grants = oulipoly_kernel_broker::accepted_grant::GrantRegistry::open(
                                            broker_state.join("grants"),
                                        ).unwrap();
                                        let reopened_work =
                                            oulipoly_kernel_broker::WorkRegistry::open(
                                                &works_dir, &roots,
                                            )
                                            .unwrap();
                                        let work_retirement = reopened_work
                                            .retirement(
                                                &expected,
                                                &roots,
                                                &grants,
                                                &reopened,
                                                Some(&retained),
                                                Some(&lane),
                                                &accepted,
                                                &terminal_dir,
                                            )
                                            .unwrap()
                                            .unwrap();
                                        let work_q = reopened_work
                                            .physical_q(&expected, &roots, &accepted, &terminal_dir)
                                            .unwrap();
                                        assert_eq!(work_retirement.physical_q, work_q);
                                        assert_eq!(
                                            work_retirement.source_grant_id,
                                            custody.source_grant_id
                                        );
                                        assert_ne!(
                                            work_retirement.original_turn_id,
                                            work_retirement.fresh_turn_id
                                        );
                                        let after_work = protocol::root_drain_readback_at(
                                            &socket, &expected, false,
                                        )
                                        .unwrap();
                                        let after_work: serde_json::Value = serde_json::from_str(
                                            after_work
                                                .strip_prefix("root-drain-v1 ")
                                                .unwrap()
                                                .trim_end(),
                                        )
                                        .unwrap();
                                        assert_eq!(after_work["source_physical_records"], 1);
                                        assert_eq!(after_work["source_physical_retired"], 1);
                                        assert_eq!(after_work["source_physical_outstanding"], 0);
                                        assert_eq!(after_work["work_records"], 1);
                                        assert_eq!(after_work["work_retired"], 1);
                                        assert_eq!(after_work["work_outstanding"], 0);
                                        assert_eq!(after_work["live_works"], 0);
                                        assert_eq!(after_work["work_debt"], 0);
                                        assert_eq!(
                                            after_work["entry_physical_settled"], true,
                                            "{after_work}"
                                        );
                                        assert_eq!(after_work["entry_unsettled"], true);
                                        assert_eq!(after_work["close_eligible"], false);
                                        assert_eq!(after_work["owner_close_preflight"], false);
                                        assert_eq!(after_work["pid1"], "live");
                                        assert_eq!(
                                            after_work["owner_close_inventory"]["owner_generation"],
                                            prepared.owner_generation
                                        );
                                        assert_eq!(
                                            after_work["owner_close_inventory"]["state_projection_pending"],
                                            false
                                        );
                                        let mut stale_root = expected.clone();
                                        stale_root.init_starttime_ticks += 1;
                                        assert!(
                                            protocol::root_pid1_drain_at(&socket, &stale_root)
                                                .is_err()
                                        );
                                        let work_receipt_path = terminal_dir
                                            .join(format!("{}.json", work_q.work_incarnation,));
                                        let work_receipt = fs::read(&work_receipt_path).unwrap();
                                        fs::write(&work_receipt_path, b"changed child Q").unwrap();
                                        assert!(
                                            protocol::root_pid1_drain_at(&socket, &expected)
                                                .is_err()
                                        );
                                        assert!(
                                            reopened_work
                                                .physical_q(
                                                    &expected,
                                                    &roots,
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .is_err()
                                        );
                                        assert!(
                                            reopened_work
                                                .retirement(
                                                    &expected,
                                                    &roots,
                                                    &grants,
                                                    &reopened,
                                                    Some(&retained),
                                                    Some(&lane),
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .is_err()
                                        );
                                        let changed = protocol::root_drain_readback_at(
                                            &socket, &expected, false,
                                        )
                                        .unwrap();
                                        let changed: serde_json::Value = serde_json::from_str(
                                            changed
                                                .strip_prefix("root-drain-v1 ")
                                                .unwrap()
                                                .trim_end(),
                                        )
                                        .unwrap();
                                        assert_eq!(changed["work_outstanding"], 1);
                                        assert_eq!(changed["work_retired"], 0);
                                        assert_eq!(changed["live_works"], 1);
                                        assert_eq!(
                                            changed["uncertain_registry_or_incarnation"],
                                            true
                                        );
                                        fs::write(&work_receipt_path, work_receipt).unwrap();
                                        assert!(
                                            reopened_work
                                                .physical_q(
                                                    &expected,
                                                    &roots,
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .is_ok()
                                        );
                                        assert_eq!(
                                            reopened_work
                                                .retirement(
                                                    &expected,
                                                    &roots,
                                                    &grants,
                                                    &reopened,
                                                    Some(&retained),
                                                    Some(&lane),
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .unwrap(),
                                            Some(work_retirement.clone())
                                        );
                                        let grant_path = broker_state
                                            .join("grants")
                                            .join(format!("{}.json", accepted.grant_id));
                                        let grant_bytes = fs::read(&grant_path).unwrap();
                                        fs::write(&grant_path, b"changed consumed grant").unwrap();
                                        assert!(
                                            reopened_work
                                                .retirement(
                                                    &expected,
                                                    &roots,
                                                    &grants,
                                                    &reopened,
                                                    Some(&retained),
                                                    Some(&lane),
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .is_err()
                                        );
                                        fs::write(&grant_path, grant_bytes).unwrap();
                                        let seal_path = works_dir
                                            .join(format!("{}.retired.json", accepted.grant_id));
                                        let seal_bytes = fs::read(&seal_path).unwrap();
                                        let mut conflict: serde_json::Value =
                                            serde_json::from_slice(&seal_bytes).unwrap();
                                        conflict["fresh_turn_id"] = "conflicting-turn".into();
                                        fs::write(
                                            &seal_path,
                                            serde_json::to_vec(&conflict).unwrap(),
                                        )
                                        .unwrap();
                                        assert!(
                                            reopened_work
                                                .retirement(
                                                    &expected,
                                                    &roots,
                                                    &grants,
                                                    &reopened,
                                                    Some(&retained),
                                                    Some(&lane),
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .is_err()
                                        );
                                        fs::write(&seal_path, seal_bytes).unwrap();
                                        let receipt_path = source_dir.join(format!(
                                            "{}.terminal.json",
                                            custody.source_grant_id
                                        ));
                                        let original_receipt = fs::read(&receipt_path).unwrap();
                                        fs::write(&receipt_path, b"changed Q").unwrap();
                                        assert!(
                                            protocol::root_pid1_drain_at(&socket, &expected)
                                                .is_err()
                                        );
                                        assert!(
                                            reopened.retirement(&custody.source_grant_id).is_err()
                                        );
                                        assert!(
                                            reopened_work
                                                .retirement(
                                                    &expected,
                                                    &roots,
                                                    &grants,
                                                    &reopened,
                                                    Some(&retained),
                                                    Some(&lane),
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .is_err()
                                        );
                                        fs::write(&receipt_path, original_receipt).unwrap();
                                        assert!(
                                            reopened
                                                .retirement(&custody.source_grant_id)
                                                .unwrap()
                                                .is_some()
                                        );
                                        let output_path = source_dir
                                            .join(format!("{}.stdout", custody.source_grant_id));
                                        let output_bytes = fs::read(&output_path).unwrap();
                                        fs::write(&output_path, b"changed parent output").unwrap();
                                        assert!(
                                            reopened_work
                                                .retirement(
                                                    &expected,
                                                    &roots,
                                                    &grants,
                                                    &reopened,
                                                    Some(&retained),
                                                    Some(&lane),
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .is_err()
                                        );
                                        fs::write(&output_path, output_bytes).unwrap();
                                        assert_eq!(
                                            reopened_work
                                                .retirement(
                                                    &expected,
                                                    &roots,
                                                    &grants,
                                                    &reopened,
                                                    Some(&retained),
                                                    Some(&lane),
                                                    &accepted,
                                                    &terminal_dir,
                                                )
                                                .unwrap(),
                                            Some(work_retirement)
                                        );
                                        // The seals can finish while the original Runner still
                                        // owns PID1's direct-child slot. Wait for that exact
                                        // process to exit before issuing a physical drain.
                                        eventually(|| entry.try_wait().unwrap().is_some());
                                        if native_f_turn {
                                            assert_eq!(
                                                entry.wait().unwrap().code(),
                                                Some(0),
                                                "F-based original caller did not exit successfully: {}",
                                                fs::read_to_string(&err).unwrap_or_default()
                                            );
                                            assert_eq!(
                                                fs::read(&out).unwrap(),
                                                format!("AGE319_F_ACK {token}\n").as_bytes(),
                                                "original caller bytes differ from exact F ACK"
                                            );
                                            // The pinned source driver also writes its
                                            // preexisting launch diagnostic to the entry's
                                            // stderr. It is separate from the F caller bytes
                                            // committed by the publication boundary.
                                            assert_eq!(
                                                fs::read(&err).unwrap(),
                                                b"v30 source physically launched; v2 acceptance remains closed\n"
                                            );
                                        }
                                        let settled_entry = protocol::root_drain_readback_at(
                                            &socket, &expected, false,
                                        )
                                        .unwrap();
                                        let settled_entry: serde_json::Value =
                                            serde_json::from_str(
                                                settled_entry
                                                    .strip_prefix("root-drain-v1 ")
                                                    .unwrap()
                                                    .trim_end(),
                                            )
                                            .unwrap();
                                        assert_eq!(
                                            settled_entry["entry_original_exited"], true,
                                            "{settled_entry}"
                                        );
                                        if closed_refusal_probe {
                                            let published = lane
                                                .read_private_root_terminal(
                                                    &root,
                                                    &recipient,
                                                    &original_session,
                                                )
                                                .unwrap();
                                            assert_eq!(published.publication_state, "settled");
                                            let parent =
                                                &published.execution.as_ref().unwrap().parent;
                                            assert!(parent.cancelled);
                                            assert_eq!(parent.outcome, "cancelled");
                                            assert_eq!(
                                                published.ack_basis.as_deref(),
                                                Some("native_codex_f_assistant_ack")
                                            );
                                            let entry_record: serde_json::Value =
                                                serde_json::from_slice(
                                                    &fs::read(broker_state.join("entries").join(
                                                        format!("{}.json", prepared.root_id),
                                                    ))
                                                    .unwrap(),
                                                )
                                                .unwrap();
                                            assert_eq!(
                                                entry_record["terminal_settlement"]["root_id"],
                                                serde_json::Value::Null
                                            );
                                            assert_eq!(
                                                entry_record["terminal_settlement"]["d_key"],
                                                root.d_key
                                            );
                                            assert_eq!(
                                                entry_record["terminal_settlement"]["publication_sha256"],
                                                published.publication_sha256.unwrap()
                                            );
                                            assert_eq!(settled_entry["entry_unsettled"], false);
                                        }
                                        // A new serving incarnation must read the retained
                                        // evidence again, including a changed child Q.
                                        let work_receipt = fs::read(&work_receipt_path).unwrap();
                                        stop(&mut broker);
                                        fs::write(&work_receipt_path, b"changed after restart")
                                            .unwrap();
                                        broker = Command::new(env!(
                                            "CARGO_BIN_EXE_oulipoly-kernel-broker"
                                        ))
                                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                        .env(
                                            "OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1",
                                            &broker_state,
                                        )
                                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                                        .env(
                                            "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                                            bash.as_ref().unwrap(),
                                        )
                                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                                        .env(
                                            "OULIPOLY_KERNEL_BROKER_PRIVATE_NATIVE_CODEX_SHA256_V1",
                                            format!(
                                                "{:x}",
                                                Sha256::digest(fs::read(&provider_image).unwrap())
                                            ),
                                        )
                                        .stderr(Stdio::from(
                                            File::create(
                                                temp.path().join("retirement-restart.log"),
                                            )
                                            .unwrap(),
                                        ))
                                        .spawn()
                                        .unwrap();
                                        eventually(|| {
                                            protocol::root_drain_readback_at(
                                                &socket, &expected, false,
                                            )
                                            .is_ok()
                                        });
                                        let changed = protocol::root_drain_readback_at(
                                            &socket, &expected, false,
                                        )
                                        .unwrap();
                                        let changed: serde_json::Value = serde_json::from_str(
                                            changed
                                                .strip_prefix("root-drain-v1 ")
                                                .unwrap()
                                                .trim_end(),
                                        )
                                        .unwrap();
                                        assert_eq!(changed["source_physical_retired"], 1);
                                        assert_eq!(changed["work_retired"], 0);
                                        assert_eq!(changed["work_outstanding"], 1);
                                        assert_eq!(
                                            changed["uncertain_registry_or_incarnation"],
                                            true
                                        );
                                        assert_eq!(changed["close_eligible"], false);
                                        assert_eq!(changed["owner_close_preflight"], false);
                                        assert_eq!(
                                            changed["owner_close_inventory"]["owner_generation"],
                                            prepared.owner_generation
                                        );
                                        assert!(
                                            protocol::root_pid1_drain_at(&socket, &expected)
                                                .is_err()
                                        );
                                        fs::write(&work_receipt_path, work_receipt).unwrap();
                                        eventually(|| {
                                            protocol::root_drain_readback_at(
                                                &socket, &expected, false,
                                            )
                                            .ok()
                                            .and_then(|reply| {
                                                serde_json::from_str::<serde_json::Value>(
                                                    reply
                                                        .strip_prefix("root-drain-v1 ")?
                                                        .trim_end(),
                                                )
                                                .ok()
                                            })
                                            .is_some_and(|inventory| {
                                                inventory["work_retired"] == 1
                                                    && inventory["work_outstanding"] == 0
                                            })
                                        });
                                        assert!(
                                            !broker_state
                                                .join(format!(
                                                    "root-pid1/{}.terminal.json",
                                                    expected.root_id
                                                ))
                                                .exists()
                                        );
                                        assert_eq!(
                                            protocol::root_pid1_drain_at(&socket, &expected)
                                                .unwrap_or_else(|error| panic!(
                                                    "PID1 request: {error}; inventory: {}",
                                                    protocol::root_drain_readback_at(
                                                        &socket, &expected, false
                                                    )
                                                    .unwrap()
                                                )),
                                            "root-pid1-drain-v1 requested\n"
                                        );
                                        let terminal_path = broker_state.join(format!(
                                            "root-pid1/{}.terminal.json",
                                            expected.root_id
                                        ));
                                        eventually(|| terminal_path.exists());
                                        if std::env::var_os("AGE319_DIAG_PIDNS_V1").is_some() {
                                            for sample in 0..3 {
                                                eprintln!(
                                                    "PIDNS diagnostic sample {sample} root={expected:?}"
                                                );
                                                let mut pids = fs::read_dir("/proc")
                                                    .unwrap()
                                                    .flatten()
                                                    .filter_map(|entry| {
                                                        entry
                                                            .file_name()
                                                            .to_string_lossy()
                                                            .parse::<i32>()
                                                            .ok()
                                                    })
                                                    .collect::<Vec<_>>();
                                                pids.sort_unstable();
                                                for pid in pids {
                                                    let path = format!("/proc/{pid}");
                                                    let Ok(status) = fs::read_to_string(format!(
                                                        "{path}/status"
                                                    )) else {
                                                        continue;
                                                    };
                                                    let nspid = status
                                                        .lines()
                                                        .find(|line| line.starts_with("NSpid:"))
                                                        .unwrap_or("");
                                                    if pid != expected.init_host_pid
                                                        && nspid.split_whitespace().count() < 3
                                                    {
                                                        continue;
                                                    }
                                                    let stat =
                                                        fs::read_to_string(format!("{path}/stat"))
                                                            .unwrap_or_default();
                                                    let start = stat
                                                        .rsplit_once(") ")
                                                        .and_then(|(_, rest)| {
                                                            rest.split_whitespace().nth(19)
                                                        })
                                                        .unwrap_or("?");
                                                    let state = status
                                                        .lines()
                                                        .find(|line| line.starts_with("State:"))
                                                        .unwrap_or("");
                                                    let ppid = status
                                                        .lines()
                                                        .find(|line| line.starts_with("PPid:"))
                                                        .unwrap_or("");
                                                    let ns =
                                                        fs::read_link(format!("{path}/ns/pid"))
                                                            .map(|p| p.display().to_string())
                                                            .unwrap_or_default();
                                                    let children = fs::read_to_string(format!(
                                                        "{path}/task/{pid}/children"
                                                    ))
                                                    .unwrap_or_default();
                                                    let wchan =
                                                        fs::read_to_string(format!("{path}/wchan"))
                                                            .unwrap_or_default();
                                                    let cmd = fs::read(format!("{path}/cmdline"))
                                                        .unwrap_or_default();
                                                    eprintln!(
                                                        "PIDNS pid={pid} start={start} {state} {ppid} {nspid} ns={ns} children={children:?} wchan={wchan:?} cmd={:?}",
                                                        String::from_utf8_lossy(&cmd)
                                                    );
                                                }
                                                std::thread::sleep(Duration::from_secs(1));
                                            }
                                        }
                                        eventually(|| {
                                            protocol::root_drain_readback_at(
                                                &socket, &expected, false,
                                            )
                                            .ok()
                                            .and_then(|reply| {
                                                serde_json::from_str::<serde_json::Value>(
                                                    reply
                                                        .strip_prefix("root-drain-v1 ")?
                                                        .trim_end(),
                                                )
                                                .ok()
                                            })
                                            .is_some_and(|inventory| {
                                                inventory["pid1_terminal_proof"] == true
                                                    && inventory["pid1_parent_wait_proof"] == true
                                                    && inventory["pid1_exact_live"] == false
                                            })
                                        });
                                        let proof = protocol::root_drain_readback_at(
                                            &socket, &expected, false,
                                        )
                                        .unwrap();
                                        let proof: serde_json::Value = serde_json::from_str(
                                            proof
                                                .strip_prefix("root-drain-v1 ")
                                                .unwrap()
                                                .trim_end(),
                                        )
                                        .unwrap();
                                        assert_eq!(proof["pid1_echild_receipt"], true);
                                        assert_eq!(proof["pid1_terminal_proof"], true);
                                        assert_eq!(proof["pid1_exact_live"], false);
                                        assert_eq!(proof["source_physical_retired"], 1);
                                        assert_eq!(proof["work_retired"], 1);
                                        assert_eq!(proof["entry_physical_settled"], true);
                                        assert_eq!(proof["entry_unsettled"], false);
                                        assert_eq!(proof["pid1_parent_wait_proof"], true);
                                        assert_eq!(proof["close_eligible"], false);
                                        assert_eq!(
                                            proof["owner_close_preflight"], !old_pending_debt,
                                            "post-Q inventory: {proof}"
                                        );
                                        assert!(proof["owner_close_inventory"]["state_cursor"]["authority_ordinal"]
                                            .as_i64().is_some_and(|ordinal| ordinal > 0));
                                        assert_eq!(
                                            proof["owner_close_inventory"]["owner_generation"],
                                            prepared.owner_generation
                                        );
                                        assert_eq!(
                                            proof["owner_close_inventory"]["deliverable_mailbox_rows"],
                                            if old_pending_debt { 1 } else { 0 }
                                        );
                                        let close_intent = if old_pending_debt {
                                            assert!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    true,
                                                )
                                                .is_err()
                                            );
                                            assert!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .is_err()
                                            );
                                            None
                                        } else {
                                            let intent = protocol::owner_close_intent_at(
                                                &socket,
                                                &expected,
                                                &prepared.owner_generation,
                                                true,
                                            )
                                            .unwrap();
                                            assert_eq!(intent.root, expected);
                                            assert_eq!(
                                                intent.owner_generation,
                                                prepared.owner_generation
                                            );
                                            assert_eq!(
                                                serde_json::to_value(&intent.state_cursor).unwrap(),
                                                proof["owner_close_inventory"]["state_cursor"]
                                            );
                                            assert_eq!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .unwrap(),
                                                intent
                                            );
                                            let mut stale = expected.clone();
                                            stale.init_starttime_ticks += 1;
                                            assert!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &stale,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .is_err()
                                            );
                                            assert!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &expected,
                                                    "wrong-owner",
                                                    false,
                                                )
                                                .is_err()
                                            );
                                            let placeholder = File::open("/dev/null").unwrap();
                                            let late = protocol::prepare_accepted_work_at(
                                                &socket,
                                                &AcceptedWorkSpec {
                                                    root_id: expected.root_id.clone(),
                                                    work_id: uuid::Uuid::new_v4().to_string(),
                                                    request_sha256: "0".repeat(64),
                                                    accepted_sha256: "0".repeat(64),
                                                    owner_generation: prepared
                                                        .owner_generation
                                                        .clone(),
                                                },
                                                [placeholder.as_raw_fd(); 5],
                                            )
                                            .unwrap();
                                            assert!(
                                                late.contains("exact root admission fenced"),
                                                "{late}"
                                            );
                                            Some(intent)
                                        };
                                        assert_exact_native_mailbox_acks(
                                            &broker_state,
                                            &gate,
                                            &original_session.session_id,
                                            custody.row_seq,
                                            &witness.fresh.session_id,
                                            witness.fresh.seq,
                                        );
                                        let mailbox_diagnostic = old_pending_debt.then(|| {
                                            read_owner_mailbox_diagnostic(
                                                &broker_state,
                                                &historical_sidecar,
                                                &data.join("state.db"),
                                                &custody.source_generation,
                                                custody.row_seq,
                                            )
                                        });
                                        if let Some(diagnostic) = &mailbox_diagnostic {
                                            eprintln!(
                                                "owner mailbox historical debt: {diagnostic}"
                                            );
                                        } else {
                                            let retained = rusqlite::Connection::open_with_flags(
                                                broker_state.join("sidecar/pid-identity.db"),
                                                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                                            )
                                            .unwrap();
                                            assert_eq!(retained.query_row(
                                                "SELECT count(*) FROM mailbox WHERE session_id='old-pending'",
                                                [], |row| row.get::<_, i64>(0),
                                            ).unwrap(), 0);
                                        }
                                        assert_eq!(
                                            proof["owner_close_inventory"]["source_effect"]["accepted"],
                                            1
                                        );
                                        let terminal: serde_json::Value = serde_json::from_slice(
                                            &fs::read(&terminal_path).unwrap(),
                                        )
                                        .unwrap();
                                        assert_eq!(
                                            terminal["root"],
                                            serde_json::to_value(&expected).unwrap()
                                        );
                                        assert_eq!(terminal["owned_children"], "ECHILD");
                                        let terminal_backup =
                                            temp.path().join("root-pid1-terminal-held.json");
                                        fs::rename(&terminal_path, &terminal_backup).unwrap();
                                        let missing = protocol::root_drain_readback_at(
                                            &socket, &expected, false,
                                        )
                                        .unwrap();
                                        let missing: serde_json::Value = serde_json::from_str(
                                            missing
                                                .strip_prefix("root-drain-v1 ")
                                                .unwrap()
                                                .trim_end(),
                                        )
                                        .unwrap();
                                        assert_eq!(missing["pid1_terminal_proof"], false);
                                        assert_eq!(missing["pid1_echild_receipt"], false);
                                        assert_eq!(missing["close_eligible"], false);
                                        assert_eq!(missing["owner_close_preflight"], false);
                                        fs::rename(&terminal_backup, &terminal_path).unwrap();
                                        let closed_owner = if old_pending_debt {
                                            assert!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    true,
                                                )
                                                .is_err()
                                            );
                                            assert!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .is_err()
                                            );
                                            None
                                        } else {
                                            // Send the commit and drop its reply socket. The
                                            // following readback must recover that exact commit.
                                            let mut lost_reply =
                                                UnixStream::connect(&socket).unwrap();
                                            let mut challenge = [0u8; 16];
                                            lost_reply.read_exact(&mut challenge).unwrap();
                                            let mut frame = vec![0x82];
                                            frame.extend_from_slice(&challenge);
                                            frame.extend_from_slice(
                                                &serde_json::to_vec(
                                                    &protocol::OwnerCloseIntentRequest {
                                                        expected: expected.clone(),
                                                        owner_generation: prepared
                                                            .owner_generation
                                                            .clone(),
                                                    },
                                                )
                                                .unwrap(),
                                            );
                                            lost_reply.write_all(&frame).unwrap();
                                            drop(lost_reply);
                                            eventually(|| {
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .is_ok()
                                            });
                                            let closed = protocol::owner_close_at(
                                                &socket,
                                                &expected,
                                                &prepared.owner_generation,
                                                false,
                                            )
                                            .unwrap();
                                            assert_eq!(closed.root_id, expected.root_id);
                                            assert_eq!(
                                                closed.owner_generation,
                                                prepared.owner_generation
                                            );
                                            assert_eq!(
                                                closed.state_cursor,
                                                close_intent.as_ref().unwrap().state_cursor
                                            );
                                            // Duplicate read and commit are exact and idempotent.
                                            assert_eq!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .unwrap(),
                                                closed
                                            );
                                            assert_eq!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    true,
                                                )
                                                .unwrap(),
                                                closed
                                            );
                                            assert!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    "stale-owner",
                                                    false,
                                                )
                                                .is_err()
                                            );
                                            let mut stale = expected.clone();
                                            stale.init_starttime_ticks += 1;
                                            assert!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &stale,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .is_err()
                                            );
                                            fs::rename(&terminal_path, &terminal_backup).unwrap();
                                            assert!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .is_err()
                                            );
                                            fs::rename(&terminal_backup, &terminal_path).unwrap();
                                            Some(closed)
                                        };
                                        stop(&mut broker);
                                        broker = Command::new(env!(
                                            "CARGO_BIN_EXE_oulipoly-kernel-broker"
                                        ))
                                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                        .env(
                                            "OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1",
                                            &broker_state,
                                        )
                                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                                        .env(
                                            "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                                            bash.as_ref().unwrap(),
                                        )
                                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                                        .env(
                                            "OULIPOLY_KERNEL_BROKER_PRIVATE_NATIVE_CODEX_SHA256_V1",
                                            format!(
                                                "{:x}",
                                                Sha256::digest(fs::read(&provider_image).unwrap())
                                            ),
                                        )
                                        .stderr(Stdio::from(
                                            File::create(temp.path().join("pid1-restart.log"))
                                                .unwrap(),
                                        ))
                                        .spawn()
                                        .unwrap();
                                        eventually(|| {
                                            protocol::root_drain_readback_at(
                                                &socket, &expected, false,
                                            )
                                            .is_ok()
                                        });
                                        let restored = protocol::root_drain_readback_at(
                                            &socket, &expected, false,
                                        )
                                        .unwrap();
                                        let restored: serde_json::Value = serde_json::from_str(
                                            restored
                                                .strip_prefix("root-drain-v1 ")
                                                .unwrap()
                                                .trim_end(),
                                        )
                                        .unwrap();
                                        assert_eq!(restored["pid1_echild_receipt"], true);
                                        assert_eq!(restored["pid1_terminal_proof"], true);
                                        assert_eq!(restored["pid1_parent_wait_proof"], true);
                                        assert_eq!(restored["pid1_exact_live"], false);
                                        assert_eq!(restored["work_retired"], 1);
                                        assert_eq!(restored["close_eligible"], false);
                                        assert_eq!(restored["owner_close_preflight"], false);
                                        assert_eq!(
                                            restored["state_sidecar_outstanding_unknown"],
                                            old_pending_debt
                                        );
                                        if old_pending_debt {
                                            assert_eq!(
                                                restored["owner_close_inventory"]["deliverable_mailbox_rows"],
                                                1
                                            );
                                            assert_eq!(
                                                restored["owner_close_inventory"],
                                                proof["owner_close_inventory"]
                                            );
                                            assert_eq!(
                                                restored["owner_close_proof"],
                                                serde_json::Value::Null
                                            );
                                        } else {
                                            assert_eq!(
                                                restored["owner_close_inventory"],
                                                serde_json::Value::Null
                                            );
                                            assert_eq!(
                                                restored["owner_close_proof"],
                                                serde_json::to_value(
                                                    closed_owner.as_ref().unwrap()
                                                )
                                                .unwrap()
                                            );
                                            assert_eq!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .unwrap(),
                                                closed_owner.clone().unwrap()
                                            );
                                            assert_eq!(
                                                protocol::owner_close_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    true,
                                                )
                                                .unwrap(),
                                                closed_owner.clone().unwrap()
                                            );
                                        }
                                        if let Some(intent) = close_intent {
                                            assert_eq!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    false,
                                                )
                                                .unwrap(),
                                                intent
                                            );
                                            assert!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    true,
                                                )
                                                .is_err()
                                            );
                                            assert_eq!(
                                                restored["owner_close_intent"],
                                                serde_json::to_value(&intent).unwrap()
                                            );
                                        } else {
                                            assert_eq!(
                                                restored["owner_close_intent"],
                                                serde_json::Value::Null
                                            );
                                            assert!(
                                                protocol::owner_close_intent_at(
                                                    &socket,
                                                    &expected,
                                                    &prepared.owner_generation,
                                                    true,
                                                )
                                                .is_err()
                                            );
                                        }
                                        assert_exact_native_mailbox_acks(
                                            &broker_state,
                                            &gate,
                                            &original_session.session_id,
                                            custody.row_seq,
                                            &witness.fresh.session_id,
                                            witness.fresh.seq,
                                        );
                                        if let Some(mailbox_diagnostic) = mailbox_diagnostic {
                                            assert_eq!(
                                                read_owner_mailbox_diagnostic(
                                                    &broker_state,
                                                    &historical_sidecar,
                                                    &data.join("state.db"),
                                                    &custody.source_generation,
                                                    custody.row_seq,
                                                ),
                                                mailbox_diagnostic,
                                                "Broker restart changed exact pending row"
                                            );
                                        }
                                        if closed_refusal_probe {
                                            let second_err =
                                                gate.join("second-sequential-entry.err");
                                            let mut second = Command::new(&runner)
                                                .arg("--help")
                                                .env("OULIPOLY_DATA_DIR", &data)
                                                .env("OULIPOLY_CONFIG_HOME", &config_home)
                                                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                                                .env(
                                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                                                    &socket,
                                                )
                                                .env(
                                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1",
                                                    &gate,
                                                )
                                                .env("AGE319_PRIVATE_OFFLINE_ROOT_V1", "1")
                                                .env_remove("LD_LIBRARY_PATH")
                                                .stdout(Stdio::null())
                                                .stderr(Stdio::from(
                                                    File::create(&second_err).unwrap(),
                                                ))
                                                .spawn()
                                                .unwrap();
                                            eventually(|| second.try_wait().unwrap().is_some());
                                            let second_status = second.wait().unwrap();
                                            let entries: Vec<_> =
                                                fs::read_dir(broker_state.join("entries"))
                                                    .unwrap()
                                                    .filter_map(Result::ok)
                                                    .filter(|entry| {
                                                        entry
                                                            .path()
                                                            .extension()
                                                            .is_some_and(|ext| ext == "json")
                                                    })
                                                    .collect();
                                            if old_pending_debt {
                                                assert!(!second_status.success());
                                                assert_eq!(
                                                    entries.len(),
                                                    1,
                                                    "old debt allowed second E: {}",
                                                    fs::read_to_string(&second_err).unwrap()
                                                );
                                            } else {
                                                assert_eq!(
                                                    entries.len(),
                                                    2,
                                                    "second E did not reserve a new root: {}",
                                                    fs::read_to_string(&second_err).unwrap()
                                                );
                                                assert!(entries.iter().any(|entry| {
                                                    entry.file_name().to_string_lossy()
                                                        != format!("{}.json", prepared.root_id)
                                                }));
                                                let second_entry = entries
                                                    .iter()
                                                    .find(|entry| {
                                                        entry.file_name().to_string_lossy()
                                                            != format!("{}.json", prepared.root_id)
                                                    })
                                                    .unwrap();
                                                let second_record: serde_json::Value =
                                                    serde_json::from_slice(
                                                        &fs::read(second_entry.path()).unwrap(),
                                                    )
                                                    .unwrap();
                                                let second_id =
                                                    second_record["root_id"].as_str().unwrap();
                                                assert_ne!(second_id, prepared.root_id);
                                                let retained =
                                                    rusqlite::Connection::open_with_flags(
                                                        broker_state
                                                            .join("sidecar/pid-identity.db"),
                                                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                                                    )
                                                    .unwrap();
                                                let second_owner: (String, String) = retained.query_row(
                                                    "SELECT generation,phase FROM completion_continuation_owner WHERE kernel_root_id=?1",
                                                    [second_id],
                                                    |row| Ok((row.get(0)?, row.get(1)?)),
                                                ).unwrap();
                                                assert_ne!(
                                                    second_owner.0,
                                                    prepared.owner_generation
                                                );
                                                assert!(matches!(
                                                    second_owner.1.as_str(),
                                                    "running" | "closing" | "lost"
                                                ));
                                                assert_eq!(
                                                    protocol::owner_close_at(
                                                        &socket,
                                                        &expected,
                                                        &prepared.owner_generation,
                                                        false,
                                                    )
                                                    .unwrap(),
                                                    closed_owner.clone().unwrap(),
                                                    "first owner close changed after second E",
                                                );
                                                let third = Command::new(&runner)
                                                    .arg("--help")
                                                    .env("OULIPOLY_DATA_DIR", &data)
                                                    .env("OULIPOLY_CONFIG_HOME", &config_home)
                                                    .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                                                    .env(
                                                        "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                                                        &socket,
                                                    )
                                                    .env(
                                                        "OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1",
                                                        &gate,
                                                    )
                                                    .env("AGE319_PRIVATE_OFFLINE_ROOT_V1", "1")
                                                    .env_remove("LD_LIBRARY_PATH")
                                                    .output()
                                                    .unwrap();
                                                assert!(
                                                    !third.status.success(),
                                                    "third E entered before the second owner closed",
                                                );
                                                assert_eq!(
                                                    fs::read_dir(broker_state.join("entries"))
                                                        .unwrap()
                                                        .filter_map(Result::ok)
                                                        .filter(|entry| entry
                                                            .path()
                                                            .extension()
                                                            .is_some_and(|ext| ext == "json"))
                                                        .count(),
                                                    2,
                                                );
                                            }
                                        }
                                        eprintln!("root PID1 drain restart readback: {}", restored);
                                    }
                                }
                            } else {
                                assert_eq!(pending, 1, "native turn must not ACK original row");
                            }
                        }
                        stop(&mut broker);
                        return;
                    }
                    let pty_restart = mode == "normal_model_provider_pty_restart"
                        || mode == "normal_model_provider_pty_physical_restart";
                    let pty_record_before = pty_restart.then(|| {
                        eventually(|| {
                            gate.join("root-pty-ready").exists()
                                || entry.try_wait().unwrap().is_some()
                        });
                        assert!(
                            gate.join("root-pty-ready").exists(),
                            "{}",
                            fs::read_to_string(&err).unwrap()
                        );
                        fs::read(
                            broker_state
                                .join("v30/fresh-provider")
                                .join(format!("{}.interactive-pty-pre-k.json", receipt.handoff_id)),
                        )
                        .unwrap()
                    });
                    let preparation_before = pty_restart.then(|| {
                        fs::read(broker_state.join("v30/fresh-provider").join(format!(
                            "{}.interactive-k-preparation.json",
                            receipt.handoff_id
                        )))
                        .unwrap()
                    });
                    if pty_restart {
                        let fresh_socket = socket.with_file_name("v30.sock");
                        stop(&mut broker);
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .stdout(Stdio::null())
                            .stderr(Stdio::from(
                                File::create(temp.path().join("pty-broker-restart.log")).unwrap(),
                            ))
                            .spawn()
                            .unwrap();
                        eventually(|| {
                            protocol::request_at(
                                &fresh_socket,
                                protocol::Operation::ObserveEntryGate,
                            )
                            .is_ok()
                        });
                        fs::write(gate.join("root-pty-rechallenge"), b"go").unwrap();
                    }
                    let provider_dir = broker_state.join("v30/fresh-provider");
                    if mode == "normal_model_provider_no_pin"
                        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_READER_PROBE_V1")
                            .is_some()
                    {
                        eventually(|| entry.try_wait().unwrap().is_some());
                        assert!(!entry.wait().unwrap().success());
                        let stderr = fs::read_to_string(&err).unwrap();
                        assert!(stderr.contains("source census"), "{stderr}");
                        assert!(
                            !provider_dir
                                .join(format!("{}.route-selection.json", receipt.handoff_id))
                                .exists()
                        );
                        let broker_reads = format!(
                            "{}{}",
                            fs::read_to_string(&broker_log).unwrap(),
                            fs::read_to_string(temp.path().join("handoff-restart.log")).unwrap()
                        );
                        assert!(
                            broker_reads.contains("age319 indexed route-choice read:"),
                            "{broker_reads}"
                        );
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                        stop(&mut broker);
                        return;
                    }
                    if mode == "normal_model_provider_pty_physical_restart_after_k" {
                        eventually(|| gate.join("physical-k-ready").exists());
                        let k_path =
                            provider_dir.join(format!("{}.interactive-k.json", receipt.handoff_id));
                        let original_k = fs::read(&k_path).unwrap();
                        let k: serde_json::Value = serde_json::from_slice(&original_k).unwrap();
                        let id = k["grant"]["id"].as_str().unwrap();
                        assert!(provider_dir.join(format!("{id}.consumed.json")).exists());
                        assert!(provider_dir.join(format!("{id}.attach.json")).exists());
                        let control_path = k["preparation"]["handoff"]["control_path"]
                            .as_str()
                            .unwrap();
                        assert!(
                            Path::new(control_path).exists(),
                            "original root control lost before broker restart"
                        );
                        stop(&mut broker);
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .stderr(Stdio::from(
                                File::create(temp.path().join("physical-post-k-restart.log"))
                                    .unwrap(),
                            ))
                            .spawn()
                            .unwrap();
                        eventually(|| {
                            protocol::request_at(
                                &socket.with_file_name("v30.sock"),
                                Operation::ObserveEntryGate,
                            )
                            .is_ok()
                        });
                        fs::write(gate.join("physical-restarted"), b"go").unwrap();
                        eventually(|| entry.try_wait().unwrap().is_some());
                        assert_eq!(fs::read(&k_path).unwrap(), original_k);
                        assert!(fs::read_to_string(&err).unwrap().contains(id));
                        assert!(
                            !provider_dir
                                .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                .exists()
                        );
                        assert!(
                            !provider_dir
                                .join(format!("{}.fresh-grant.json", receipt.handoff_id))
                                .exists()
                        );
                        assert!(!gate.join("provider-effect").exists());
                        assert!(!gate.join("interactive-effect").exists());
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".interactive-k.json"))
                                .count(),
                            1
                        );
                        assert_old_debt_and_no_f_ack(&broker_state);
                        stop(&mut broker);
                        return;
                    }
                    if mode == "normal_model_provider_pty_physical_wrong_actor" {
                        eventually(|| gate.join("physical-before-k").exists());
                        let handoff: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!(
                                "{}.interactive-pty-pre-k.json",
                                receipt.handoff_id
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        let selected: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!(
                                "{}.interactive-route-selection.json",
                                receipt.handoff_id
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        let request = protocol::PrivateFreshPtyHandoff {
                            d_key: receipt.d_key.clone(),
                            session_id: handoff["binding"]["session_id"].as_str().unwrap().into(),
                            role: protocol::FreshPlanRole::Interactive,
                            account: "local".into(),
                            plan_sha256: selected["selection"]["plan_sha256"]
                                .as_str()
                                .unwrap()
                                .into(),
                            control_path: handoff["control_path"].as_str().unwrap().into(),
                        };
                        let dummy = File::open("/dev/null").unwrap();
                        assert!(
                            protocol::private_fresh_interactive_k_at(
                                &socket.with_file_name("v30.sock"),
                                &request,
                                [dummy.as_raw_fd(); 8],
                            )
                            .is_err(),
                            "sibling actor consumed interactive K"
                        );
                        assert!(
                            !provider_dir
                                .join(format!("{}.interactive-k.json", receipt.handoff_id))
                                .exists()
                        );
                        fs::write(gate.join("physical-continue"), b"go").unwrap();
                    }
                    if mode == "normal_model_provider_pty_physical_post_k_unknown" {
                        eventually(|| entry.try_wait().unwrap().is_some());
                        let k: serde_json::Value = serde_json::from_slice(
                            &fs::read(
                                provider_dir
                                    .join(format!("{}.interactive-k.json", receipt.handoff_id)),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        let id = k["grant"]["id"].as_str().unwrap();
                        assert_eq!(k["state"], "consumed-before-child-release");
                        assert!(!provider_dir.join(format!("{id}.consumed.json")).exists());
                        assert!(!provider_dir.join(format!("{id}.attach.json")).exists());
                        assert!(
                            !provider_dir
                                .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                .exists()
                        );
                        assert!(!gate.join("interactive-effect").exists());
                        assert!(!gate.join("provider-effect").exists());
                        assert!(fs::read_to_string(&err).unwrap().contains(id));
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".interactive-k.json"))
                                .count(),
                            1
                        );
                        assert_old_debt_and_no_f_ack(&broker_state);
                        stop(&mut broker);
                        return;
                    }
                    if mode == "normal_model_provider_pty_physical_root_exit" {
                        eventually(|| entry.try_wait().unwrap().is_some());
                        let k: serde_json::Value = serde_json::from_slice(
                            &fs::read(
                                provider_dir
                                    .join(format!("{}.interactive-k.json", receipt.handoff_id)),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        let id = k["grant"]["id"].as_str().unwrap();
                        eventually(|| provider_dir.join(format!("{id}.pid1-wait.json")).exists());
                        assert!(provider_dir.join(format!("{id}.consumed.json")).exists());
                        assert!(provider_dir.join(format!("{id}.attach.json")).exists());
                        assert!(provider_dir.join(format!("{id}.exit.json")).exists());
                        assert!(provider_dir.join(format!("{id}.drain.json")).exists());
                        assert!(
                            !provider_dir
                                .join(format!("{id}.interactive-output.json"))
                                .exists(),
                            "a dead original root cannot attest PTY EOF or transcript"
                        );
                        assert!(
                            !provider_dir
                                .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                .exists()
                        );
                        assert!(!gate.join("provider-effect").exists());
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".interactive-k.json"))
                                .count(),
                            1
                        );
                        assert_old_debt_and_no_f_ack(&broker_state);
                        stop(&mut broker);
                        return;
                    }
                    if let Some(failure) = resident_failure {
                        eventually(|| {
                            gate.join("interactive-resident-refusal").exists()
                                || entry.try_wait().unwrap().is_some()
                        });
                        assert!(
                            gate.join("interactive-resident-refusal").exists(),
                            "resident {failure}: {}",
                            fs::read_to_string(&err).unwrap_or_default()
                        );
                        let refusal =
                            fs::read_to_string(gate.join("interactive-resident-refusal")).unwrap();
                        assert!(!refusal.is_empty(), "resident refusal was empty");
                        eventually(|| entry.try_wait().unwrap().is_some());
                        let lost_control = matches!(failure, "absent_socket" | "replaced_socket");
                        assert_eq!(
                            entry.wait().unwrap().success(),
                            !lost_control,
                            "resident {failure} root result: {}",
                            fs::read_to_string(&err).unwrap_or_default()
                        );
                        let k: serde_json::Value = serde_json::from_slice(
                            &fs::read(
                                provider_dir
                                    .join(format!("{}.interactive-k.json", receipt.handoff_id)),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        let q_path =
                            provider_dir.join(format!("{}.interactive-q.json", receipt.handoff_id));
                        if lost_control {
                            assert!(!q_path.exists(), "lost original control published Q");
                            assert!(
                                fs::read_to_string(&err)
                                    .unwrap_or_default()
                                    .contains("interactive original control unknown after K")
                            );
                        } else {
                            let q: serde_json::Value =
                                serde_json::from_slice(&fs::read(q_path).unwrap()).unwrap();
                            assert_eq!(q["k"], k);
                            assert_eq!(q["provider_exit"]["wait_status"], 0);
                        }
                        let sidecar = rusqlite::Connection::open_with_flags(
                            broker_state.join("v30/sidecar/pid-identity.db"),
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .unwrap();
                        let generations: i64 = sidecar
                            .query_row(
                                "SELECT count(*) FROM runtime_generation WHERE session_id=?1",
                                [&session.session_id],
                                |row| row.get(0),
                            )
                            .unwrap();
                        let preparations: i64 = sidecar
                            .query_row(
                                "SELECT count(*) FROM fresh_native_f_preparation",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap();
                        assert_eq!(generations, 0, "refused resident created a generation");
                        assert_eq!(preparations, 0, "refused resident prepared F");
                        assert!(!gate.join("interactive-resident-readback.json").exists());
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".interactive-k.json"))
                                .count(),
                            1
                        );
                        eprintln!(
                            "resident refusal evidence: case={failure} grant={} reason={refusal}",
                            k["grant"]["id"]
                        );
                        stop(&mut broker);
                        return;
                    }
                    if mode.ends_with("_adapter_unsupported") {
                        let until = Instant::now() + Duration::from_secs(20);
                        while entry.try_wait().unwrap().is_none() && Instant::now() < until {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(!entry.wait().unwrap().success());
                        assert!(
                            fs::read_to_string(&err)
                                .unwrap_or_default()
                                .contains("selected resident adapter lacks native pages before K")
                        );
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".interactive-k.json"))
                                .count(),
                            0
                        );
                        let side = rusqlite::Connection::open(
                            broker_state.join("v30/sidecar/pid-identity.db"),
                        )
                        .unwrap();
                        for table in [
                            "fresh_native_f_submission",
                            "fresh_native_f_transport",
                            "fresh_native_f_receipt",
                            "fresh_native_f_auto_ack",
                        ] {
                            assert_eq!(
                                side.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                                    .get::<_, i64>(0))
                                    .unwrap(),
                                0
                            );
                        }
                        stop(&mut broker);
                        return;
                    }
                    if resident_mode {
                        let readback_path = gate.join("interactive-resident-readback.json");
                        eventually(|| {
                            readback_path.exists() || entry.try_wait().unwrap().is_some()
                        });
                        assert!(
                            readback_path.exists(),
                            "resident root: {}; broker: {}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default()
                        );
                        assert!(
                            entry.try_wait().unwrap().is_none(),
                            "original root exited before live inspection"
                        );
                        let readback: serde_json::Value =
                            serde_json::from_slice(&fs::read(&readback_path).unwrap()).unwrap();
                        let resident = &readback["broker"]["resident"];
                        let registration = &resident["registration"];
                        let generation = &readback["broker"]["generation"];
                        let grant_id = registration["grant_id"].as_str().unwrap();
                        let selected_plan: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!(
                                "{}.interactive-route-selection.json",
                                receipt.handoff_id
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(registration["session_id"], session.session_id);
                        assert_eq!(registration["invocation_uuid"], receipt.invocation_uuid);
                        assert_eq!(registration["account"], "local");
                        assert_eq!(
                            registration["plan_sha256"],
                            selected_plan["selection"]["plan_sha256"]
                        );
                        assert_eq!(readback["broker"]["lane_id"], session.lane_id);
                        assert_eq!(generation["generation_id"], grant_id);
                        assert_eq!(generation["lifecycle_state"], "running");
                        assert_eq!(generation["runtime_mode"], "pty_interactive");
                        assert_eq!(generation["session_id"], session.session_id);
                        assert_eq!(generation["provider_name"], registration["account"]);
                        assert_eq!(generation["model_name"], registration["model"]);
                        assert_eq!(generation["pty_control_path"], registration["control_path"]);
                        assert_eq!(generation["effective_cwd"], registration["effective_cwd"]);
                        assert_eq!(
                            generation["creator_process_evidence"]["Recorded"],
                            registration["creator"]
                        );
                        assert_eq!(
                            generation["exact_process_evidence"]["Recorded"],
                            registration["provider"]
                        );
                        let sidecar = rusqlite::Connection::open_with_flags(
                            broker_state.join("v30/sidecar/pid-identity.db"),
                            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                        )
                        .unwrap();
                        let stored: (String, String, String, String, i64, i64) = sidecar.query_row(
                            "SELECT generation_uuid,lifecycle_state,session_id,provider_name,creator_identity_os_pid,identity_os_pid FROM runtime_generation WHERE generation_uuid=?1",
                            [grant_id],
                            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?)),
                        ).unwrap();
                        assert_eq!(stored.0, grant_id);
                        assert_eq!(stored.1, "running");
                        assert_eq!(stored.2, session.session_id);
                        assert_eq!(stored.3, "local");
                        assert_eq!(
                            stored.4,
                            registration["creator"]["os_pid"].as_i64().unwrap()
                        );
                        assert_eq!(
                            stored.5,
                            registration["provider"]["os_pid"].as_i64().unwrap()
                        );
                        let provider_pid = stored.5;
                        assert_eq!(
                            oulipoly_state::pid_identity::read_live_process_identity(provider_pid)
                                .unwrap()
                                .unwrap()
                                .os_pid,
                            provider_pid
                        );
                        assert_eq!(
                            registration["observer_domain"],
                            oulipoly_state::pid_identity::procfs_observer_domain().unwrap()
                        );
                        let attested: oulipoly_state::mailbox::FreshInteractiveResidentRegistration =
                            serde_json::from_value(registration.clone()).unwrap();
                        let mut lane = FreshV30Lane::open_at(&broker_state).unwrap();
                        let replay = lane.register_interactive_resident(&attested).unwrap();
                        assert_eq!(serde_json::to_value(replay).unwrap(), *generation);
                        let mut wrong_observer = attested.clone();
                        wrong_observer.observer_domain.push_str(":other");
                        assert!(lane.register_interactive_resident(&wrong_observer).is_err());
                        let mut stale_identity = attested.clone();
                        stale_identity.provider.os_pid_starttime_ticks += 1;
                        assert!(lane.register_interactive_resident(&stale_identity).is_err());
                        assert_eq!(
                            readback["socket"]["provider_process"],
                            registration["provider"]
                        );
                        assert_eq!(
                            readback["socket"]["creator_process"],
                            registration["creator"]
                        );
                        assert_eq!(readback["socket"]["generation_id"], grant_id);
                        let path = registration["control_path"].as_str().unwrap();
                        let identity = oulipoly_runtime::executor::cli::pty_broker::query_pty_generation_identity(
                            std::path::Path::new(path),
                            resident["control_device"].as_u64().unwrap(),
                            resident["control_inode"].as_u64().unwrap(),
                        ).unwrap();
                        assert_eq!(identity.provider_process.os_pid, provider_pid);
                        let native: serde_json::Value =
                            serde_json::from_slice(&fs::read(&native_store).unwrap()).unwrap();
                        assert_eq!(native["format"], "age319-interactive-native-session/v1");
                        assert_eq!(native["session_id"], session.session_id);
                        assert_eq!(native["provider_local_pid"], resident["provider_local_pid"]);
                        assert_eq!(native["controlling_tty"], true);
                        assert_eq!(native["turns"], serde_json::json!([]));
                        assert_eq!(readback["native_tail"]["session_id"], session.session_id);
                        assert_eq!(
                            readback["native_tail"]["provider_instance_id"],
                            readback["socket"]["provider_instance_id"]
                        );
                        assert_eq!(
                            readback["native_tail"]["provider_instance_id"],
                            "age319-resident-native-fixture-instance"
                        );
                        assert_eq!(
                            readback["native_tail"]["settings_id"],
                            "age319-resident-settings"
                        );
                        assert_eq!(readback["native_tail"]["snapshot_complete"], true);
                        assert_eq!(readback["native_tail"]["turn_count"], 0);
                        assert!(
                            readback["native_tail"]["resume_token"]
                                .as_str()
                                .unwrap()
                                .ends_with(&format!(
                                    "{}:0",
                                    native["store_nonce"].as_str().unwrap()
                                ))
                        );
                        let preparation_count: i64 = sidecar
                            .query_row(
                                "SELECT count(*) FROM fresh_native_f_preparation",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap();
                        assert_eq!(preparation_count, 0);
                        assert!(
                            !provider_dir
                                .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                .exists()
                        );
                        if resident_bash {
                            eventually(|| gate.join("causal-before-c").exists());
                            let c_socket = socket.with_file_name("v30.sock");
                            assert!(
                                fs::symlink_metadata(&c_socket)
                                    .unwrap()
                                    .file_type()
                                    .is_socket()
                            );
                            assert!(
                                broker.try_wait().unwrap().is_none(),
                                "C broker exited before request"
                            );
                            eprintln!(
                                "resident Bash C broker_pid={} socket={}",
                                broker.id(),
                                c_socket.display()
                            );
                            let parent_path = provider_dir
                                .join(format!("{}.interactive-k.json", receipt.handoff_id));
                            let original_parent = fs::read(&parent_path).unwrap();
                            let consumed_parent_path =
                                provider_dir.join(format!("{grant_id}.consumed.json"));
                            let original_consumed_parent = fs::read(&consumed_parent_path).unwrap();
                            let ambiguous_path = provider_dir
                                .join(format!("{}.fresh-grant.json", receipt.handoff_id));
                            if mode.ends_with("_absent_parent") {
                                fs::remove_file(&parent_path).unwrap();
                            } else if mode.ends_with("_unconsumed_parent") {
                                fs::remove_file(&consumed_parent_path).unwrap();
                            } else if mode.ends_with("_ambiguous_parent") {
                                let k: serde_json::Value =
                                    serde_json::from_slice(&original_parent).unwrap();
                                fs::write(
                                    &ambiguous_path,
                                    serde_json::to_vec(&k["grant"]).unwrap(),
                                )
                                .unwrap();
                            } else if mode.ends_with("_wrong_plan")
                                || mode.ends_with("_wrong_actor")
                            {
                                let mut changed: serde_json::Value =
                                    serde_json::from_slice(&original_parent).unwrap();
                                if mode.ends_with("_wrong_plan") {
                                    changed["grant"]["plan_sha256"] =
                                        serde_json::json!("0".repeat(64));
                                } else {
                                    changed["grant"]["binding"]["actor_starttime"] =
                                        serde_json::json!(0);
                                }
                                fs::write(&parent_path, serde_json::to_vec(&changed).unwrap())
                                    .unwrap();
                            }
                            fs::write(gate.join("causal-release-c"), b"go").unwrap();
                            eventually(|| {
                                gate.join("bash-causal-terminal-status").exists()
                                    || entry.try_wait().unwrap().is_some()
                            });
                            let expected_refusal = if mode.ends_with("_wrong_image") {
                                Some("Bash child image changed")
                            } else if mode.ends_with("_absent_parent") {
                                Some("consumed causal parent work grant absent")
                            } else if mode.ends_with("_unconsumed_parent") {
                                Some("causal parent K not consumed")
                            } else if mode.ends_with("_ambiguous_parent") {
                                Some("ambiguous causal parent K")
                            } else if mode.ends_with("_wrong_plan")
                                || mode.ends_with("_wrong_actor")
                            {
                                Some("interactive causal parent K or plan changed")
                            } else {
                                None
                            };
                            if let Some(reason) = expected_refusal {
                                assert_eq!(
                                    fs::read(gate.join("bash-causal-terminal-status"))
                                        .unwrap_or_default(),
                                    b"70"
                                );
                                assert!(
                                    fs::read_to_string(gate.join("bash-causal-error"))
                                        .unwrap_or_default()
                                        .contains(reason),
                                    "{}",
                                    fs::read_to_string(gate.join("bash-causal-error"))
                                        .unwrap_or_default()
                                );
                                let fresh =
                                    rusqlite::Connection::open(broker_state.join("v30/state.db"))
                                        .unwrap();
                                assert_eq!(
                                    fresh
                                        .query_row(
                                            "SELECT count(*) FROM fresh_bash_child",
                                            [],
                                            |r| r.get::<_, i64>(0)
                                        )
                                        .unwrap(),
                                    0
                                );
                                assert_eq!(
                                    fresh
                                        .query_row(
                                            "SELECT count(*) FROM fresh_lane_accepted_source",
                                            [],
                                            |r| r.get::<_, i64>(0)
                                        )
                                        .unwrap(),
                                    0
                                );
                                assert!(!gate.join("bash-effect").exists());
                                let side = rusqlite::Connection::open(
                                    broker_state.join("v30/sidecar/pid-identity.db"),
                                )
                                .unwrap();
                                assert_eq!(
                                    side.query_row("SELECT count(*) FROM mailbox", [], |r| r
                                        .get::<_, i64>(0))
                                        .unwrap(),
                                    0
                                );
                                let old = rusqlite::Connection::open(
                                    broker_state.join("sidecar/pid-identity.db"),
                                )
                                .unwrap();
                                assert_eq!(old.query_row("SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND delivered_at IS NULL", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
                                if mode.ends_with("_absent_parent")
                                    || mode.ends_with("_wrong_plan")
                                    || mode.ends_with("_wrong_actor")
                                {
                                    fs::write(&parent_path, &original_parent).unwrap();
                                }
                                if mode.ends_with("_ambiguous_parent") {
                                    fs::remove_file(&ambiguous_path).unwrap();
                                }
                                if mode.ends_with("_unconsumed_parent") {
                                    fs::write(&consumed_parent_path, &original_consumed_parent)
                                        .unwrap();
                                }
                            } else {
                                assert_eq!(
                                    fs::read(gate.join("bash-causal-terminal-status"))
                                        .unwrap_or_default(),
                                    b"0",
                                    "bash: {}; root: {}; broker: {}",
                                    fs::read_to_string(gate.join("bash-causal-error"))
                                        .unwrap_or_default(),
                                    fs::read_to_string(&err).unwrap_or_default(),
                                    fs::read_to_string(&broker_log).unwrap_or_default(),
                                );
                                assert!(
                                    entry.try_wait().unwrap().is_none(),
                                    "root exited before Bash W"
                                );
                                assert!(
                                    oulipoly_state::pid_identity::read_live_process_identity(
                                        provider_pid
                                    )
                                    .unwrap()
                                    .is_some()
                                );
                                let report: serde_json::Value = serde_json::from_slice(
                                    &fs::read(gate.join("bash-causal-output")).unwrap(),
                                )
                                .unwrap();
                                let child = &report["child"];
                                let source = &report["fresh_source_w"];
                                let child_grant = source["physical_grant_id"].as_str().unwrap();
                                assert_eq!(child["root_id"], prepared.root_id);
                                assert_eq!(child["root_handoff_id"], receipt.handoff_id);
                                assert_eq!(
                                    child["parent_invocation_uuid"],
                                    receipt.invocation_uuid
                                );
                                assert_eq!(child["parent_work_grant_id"], grant_id);
                                let parent_attach: serde_json::Value = serde_json::from_slice(
                                    &fs::read(provider_dir.join(format!("{grant_id}.attach.json")))
                                        .unwrap(),
                                )
                                .unwrap();
                                assert_eq!(child["parent_work_id"], parent_attach["work_id"]);
                                assert_ne!(child_grant, grant_id);
                                assert_ne!(child["session"]["session_id"], session.session_id);
                                assert_eq!(source["root_id"], prepared.root_id);
                                assert_eq!(source["session_id"], child["session"]["session_id"]);
                                assert_eq!(source["parent_work_grant_id"], grant_id);
                                let selected_child: serde_json::Value =
                                    serde_json::from_slice(
                                        &fs::read(provider_dir.join(format!(
                                            "{bash_request}.child-work-selection.json"
                                        )))
                                        .unwrap(),
                                    )
                                    .unwrap();
                                assert_eq!(selected_child["role"], "bash-child-private-fixed-v1");
                                assert_eq!(selected_child["child_request_id"], bash_request);
                                assert_eq!(selected_child["child_d_key"], child["d_key"]);
                                assert_eq!(
                                    selected_child["binding"]["causal_parent"]["grant_id"],
                                    grant_id
                                );
                                assert_eq!(
                                    selected_child["binding"]["causal_parent"]["work_id"],
                                    parent_attach["work_id"]
                                );
                                let child_k: serde_json::Value = serde_json::from_slice(
                                    &fs::read(
                                        provider_dir
                                            .join(format!("{bash_request}.fresh-grant.json")),
                                    )
                                    .unwrap(),
                                )
                                .unwrap();
                                assert_eq!(child_k["id"], child_grant);
                                assert_eq!(child_k["plan_sha256"], selected_child["plan_sha256"]);
                                assert_eq!(child_k["binding"], selected_child["binding"]);
                                let fresh =
                                    rusqlite::Connection::open(broker_state.join("v30/state.db"))
                                        .unwrap();
                                assert_eq!(
                                    fresh
                                        .query_row(
                                            "SELECT count(*) FROM fresh_lane_accepted_source",
                                            [],
                                            |r| r.get::<_, i64>(0)
                                        )
                                        .unwrap(),
                                    1
                                );
                                let side = rusqlite::Connection::open(
                                    broker_state.join("v30/sidecar/pid-identity.db"),
                                )
                                .unwrap();
                                let expected_pending = i64::from(resident_notify);
                                eventually(|| {
                                    side.query_row("SELECT count(*) FROM mailbox WHERE handle=?1 AND delivered_at IS NULL", [source["source_id"].as_str().unwrap()], |r| r.get::<_, i64>(0)).is_ok_and(|n| n == expected_pending)
                                });
                                assert_eq!(
                                    side.query_row(
                                        "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
                                        [],
                                        |r| r.get::<_, i64>(0)
                                    )
                                    .unwrap(),
                                    expected_pending
                                );
                                assert_eq!(
                                    side.query_row(
                                        "SELECT count(*) FROM fresh_recipient_grant",
                                        [],
                                        |r| r.get::<_, i64>(0)
                                    )
                                    .unwrap(),
                                    0
                                );
                                let old = rusqlite::Connection::open(
                                    broker_state.join("sidecar/pid-identity.db"),
                                )
                                .unwrap();
                                assert_eq!(old.query_row("SELECT count(*) FROM mailbox WHERE session_id='old-pending' AND handle='old-unacked' AND delivered_at IS NULL", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
                                if mode.ends_with("_restart") {
                                    let accepted: oulipoly_state::mailbox::FreshBashSourceEvent =
                                        serde_json::from_value(source.clone()).unwrap();
                                    stop(&mut broker);
                                    broker =
                                        Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                                            .env(
                                                "OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1",
                                                &socket,
                                            )
                                            .env(
                                                "OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1",
                                                &broker_state,
                                            )
                                            .env(
                                                "OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1",
                                                &runner,
                                            )
                                            .env(
                                                "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                                                bash.as_ref().unwrap(),
                                            )
                                            .env(
                                                "OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1",
                                                &gate,
                                            )
                                            .stderr(Stdio::from(
                                                File::create(
                                                    temp.path().join("resident-bash-restart.log"),
                                                )
                                                .unwrap(),
                                            ))
                                            .spawn()
                                            .unwrap();
                                    eventually(|| {
                                        protocol::request_at(&socket, Operation::Classify).is_ok()
                                    });
                                    let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                                    lane.accept_private_bash_source(&accepted).unwrap();
                                    assert_eq!(
                                        fresh
                                            .query_row(
                                                "SELECT count(*) FROM fresh_bash_child",
                                                [],
                                                |r| r.get::<_, i64>(0)
                                            )
                                            .unwrap(),
                                        1
                                    );
                                    assert_eq!(
                                        fresh
                                            .query_row(
                                                "SELECT count(*) FROM fresh_lane_accepted_source",
                                                [],
                                                |r| r.get::<_, i64>(0)
                                            )
                                            .unwrap(),
                                        1
                                    );
                                    assert_eq!(
                                        fs::read_dir(&provider_dir)
                                            .unwrap()
                                            .filter_map(Result::ok)
                                            .filter(|entry| entry
                                                .file_name()
                                                .to_string_lossy()
                                                .ends_with(".consumed.json"))
                                            .count(),
                                        2
                                    );
                                    assert!(
                                        entry.try_wait().unwrap().is_none(),
                                        "root exited during W restart"
                                    );
                                }
                            }
                        }
                        fs::write(gate.join("interactive-resident-continue"), b"continue").unwrap();
                        if native_crash.is_some() || native_lost_reply.is_some() {
                            if let Some(stage) = native_crash {
                                eventually(|| {
                                    gate.join("interactive-f-crash-ready").exists()
                                        || entry.try_wait().unwrap().is_some()
                                });
                                assert_eq!(
                                    fs::read_to_string(gate.join("interactive-f-crash-ready"))
                                        .unwrap(),
                                    stage.split("_changed_").next().unwrap()
                                );
                                assert!(
                                    entry.try_wait().unwrap().is_none(),
                                    "original root died before F crash"
                                );
                                stop(&mut broker);
                                if stage.ends_with("_changed_native") {
                                    let mut native: serde_json::Value =
                                        serde_json::from_slice(&fs::read(&native_store).unwrap())
                                            .unwrap();
                                    native["turns"][0]["body"] =
                                        serde_json::json!("changed-native-body");
                                    fs::write(&native_store, serde_json::to_vec(&native).unwrap())
                                        .unwrap();
                                }
                                if stage.ends_with("_changed_session") {
                                    let mut native: serde_json::Value =
                                        serde_json::from_slice(&fs::read(&native_store).unwrap())
                                            .unwrap();
                                    native["session_id"] =
                                        serde_json::json!(uuid::Uuid::new_v4().to_string());
                                    fs::write(&native_store, serde_json::to_vec(&native).unwrap())
                                        .unwrap();
                                }
                                if stage.ends_with("_changed_control") {
                                    let fenced: serde_json::Value = serde_json::from_slice(
                                        &fs::read(gate.join("interactive-f-fenced.json")).unwrap(),
                                    )
                                    .unwrap();
                                    let path = std::path::PathBuf::from(
                                        fenced["preparation"]["pty_control_path"].as_str().unwrap(),
                                    );
                                    fs::rename(&path, path.with_extension("removed-sock")).unwrap();
                                }
                            } else {
                                eventually(|| {
                                    gate.join("interactive-f-reply-dropped").exists()
                                        && broker.try_wait().unwrap().is_some()
                                });
                                assert_eq!(
                                    fs::read_to_string(gate.join("interactive-f-reply-dropped"))
                                        .unwrap(),
                                    native_lost_reply.unwrap()
                                );
                                assert!(
                                    entry.try_wait().unwrap().is_none(),
                                    "original root died before F readback"
                                );
                            }
                            let side = rusqlite::Connection::open(
                                broker_state.join("v30/sidecar/pid-identity.db"),
                            )
                            .unwrap();
                            let counts = [
                                "fresh_native_f_submission",
                                "fresh_native_f_transport",
                                "fresh_native_f_receipt",
                                "fresh_native_f_auto_ack",
                            ]
                            .map(|table| {
                                side.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| {
                                    r.get::<_, i64>(0)
                                })
                                .unwrap()
                            });
                            assert_eq!(counts[0], 1, "one-use fence missing at crash");
                            if let Some(stage) = native_crash {
                                assert_eq!(
                                    counts[1],
                                    i64::from(
                                        !(stage == "after_fence"
                                            || stage == "after_partial"
                                            || stage.starts_with("after_write"))
                                    )
                                );
                                assert_eq!(
                                    counts[2],
                                    i64::from(stage.starts_with("after_receipt"))
                                );
                                assert_eq!(counts[3], 0);
                            } else {
                                let stage = native_lost_reply.unwrap();
                                assert_eq!(counts[1], 1);
                                assert_eq!(counts[2], i64::from(stage != "transport"));
                                assert_eq!(counts[3], i64::from(stage == "ack"));
                            }
                            broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                                .env(
                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                                    bash.as_ref().unwrap(),
                                )
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                                .stderr(Stdio::from(
                                    File::create(temp.path().join("native-f-restart.log")).unwrap(),
                                ))
                                .spawn()
                                .unwrap();
                            eventually(|| {
                                protocol::request_at(&socket, Operation::Classify).is_ok()
                            });
                            if native_crash.is_some() {
                                fs::write(gate.join("interactive-f-crash-continue"), b"continue")
                                    .unwrap();
                            }
                        }
                        if matches!(
                            native_crash,
                            Some(
                                "after_fence"
                                    | "after_partial"
                                    | "after_turn_changed_native"
                                    | "after_turn_changed_session"
                                    | "after_receipt_changed_native"
                                    | "after_write_changed_control"
                            )
                        ) {
                            let until = Instant::now() + Duration::from_secs(25);
                            while entry.try_wait().unwrap().is_none() && Instant::now() < until {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            assert!(
                                entry.try_wait().unwrap().is_some(),
                                "F refusal did not settle in bound"
                            );
                            assert!(!entry.wait().unwrap().success());
                            let failure = fs::read_to_string(&err).unwrap_or_default();
                            assert!(
                                failure.contains("pending") || failure.contains("no replay"),
                                "{failure}"
                            );
                            let side = rusqlite::Connection::open(
                                broker_state.join("v30/sidecar/pid-identity.db"),
                            )
                            .unwrap();
                            assert_eq!(
                                side.query_row(
                                    "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
                                    [],
                                    |r| r.get::<_, i64>(0)
                                )
                                .unwrap(),
                                1
                            );
                            assert_eq!(
                                side.query_row(
                                    "SELECT count(*) FROM fresh_native_f_auto_ack",
                                    [],
                                    |r| r.get::<_, i64>(0)
                                )
                                .unwrap(),
                                0
                            );
                            assert!(
                                !provider_dir
                                    .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                    .exists()
                            );
                            assert_old_pending_v29(&broker_state);
                            stop(&mut broker);
                            return;
                        }
                        if mode.ends_with("_physical_partial") {
                            let until = Instant::now() + Duration::from_secs(20);
                            while entry.try_wait().unwrap().is_none() && Instant::now() < until {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            assert!(!entry.wait().unwrap().success());
                            assert!(
                                fs::read_to_string(&err)
                                    .unwrap_or_default()
                                    .contains("native F partial PTY write unknown; no replay")
                            );
                            assert!(gate.join("interactive-f-fenced.json").exists());
                            let side = rusqlite::Connection::open(
                                broker_state.join("v30/sidecar/pid-identity.db"),
                            )
                            .unwrap();
                            for table in ["fresh_native_f_submission", "fresh_recipient_grant"] {
                                assert_eq!(
                                    side.query_row(
                                        &format!("SELECT count(*) FROM {table}"),
                                        [],
                                        |r| r.get::<_, i64>(0)
                                    )
                                    .unwrap(),
                                    1
                                );
                            }
                            for table in [
                                "fresh_native_f_transport",
                                "fresh_native_f_receipt",
                                "fresh_native_f_auto_ack",
                            ] {
                                assert_eq!(
                                    side.query_row(
                                        &format!("SELECT count(*) FROM {table}"),
                                        [],
                                        |r| r.get::<_, i64>(0)
                                    )
                                    .unwrap(),
                                    0
                                );
                            }
                            assert_eq!(
                                side.query_row(
                                    "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
                                    [],
                                    |r| r.get::<_, i64>(0)
                                )
                                .unwrap(),
                                1
                            );
                            assert!(
                                !provider_dir
                                    .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                    .exists()
                            );
                            let native: serde_json::Value =
                                serde_json::from_slice(&fs::read(&native_store).unwrap()).unwrap();
                            assert_eq!(native["turns"], serde_json::json!([]));
                            stop(&mut broker);
                            return;
                        }
                        if mode.contains("_after_append") {
                            let raw = provider_dir.join(format!("{grant_id}.interactive-output"));
                            let output_receipt =
                                provider_dir.join(format!("{grant_id}.interactive-output.json"));
                            let until = Instant::now() + Duration::from_secs(10);
                            while !gate.join("interactive-after-append-ready").exists()
                                && Instant::now() < until
                            {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            assert!(
                                gate.join("interactive-after-append-ready").exists()
                                    && fs::metadata(&raw).is_ok_and(|meta| meta.len() == 92)
                                    && !output_receipt.exists(),
                                "after append gap: root={} broker={} artifacts={:?}",
                                fs::read_to_string(&err).unwrap_or_default(),
                                fs::read_to_string(&broker_log).unwrap_or_default(),
                                fs::read_dir(&provider_dir)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .map(|item| item.file_name().to_string_lossy().into_owned())
                                    .collect::<Vec<_>>()
                            );
                            assert!(entry.try_wait().unwrap().is_none());
                            assert!(
                                !provider_dir
                                    .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                    .exists()
                            );
                            if mode.ends_with("_corrupt") {
                                use std::os::unix::fs::FileExt;
                                File::options()
                                    .write(true)
                                    .open(&raw)
                                    .unwrap()
                                    .write_all_at(b"X", 0)
                                    .unwrap();
                            }
                            stop(&mut broker);
                            broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                                .env(
                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1",
                                    bash.as_ref().unwrap(),
                                )
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                                .stderr(Stdio::from(
                                    File::create(temp.path().join("after-append-restart.log"))
                                        .unwrap(),
                                ))
                                .spawn()
                                .unwrap();
                            eventually(|| {
                                protocol::request_at(&socket, Operation::Classify).is_ok()
                            });
                            if mode.ends_with("_corrupt") {
                                let until = Instant::now() + Duration::from_secs(25);
                                while entry.try_wait().unwrap().is_none() && Instant::now() < until
                                {
                                    std::thread::sleep(Duration::from_millis(20));
                                }
                                assert!(
                                    !entry.wait().unwrap().success(),
                                    "corrupt transcript received Q"
                                );
                                assert!(
                                    fs::read_to_string(&err)
                                        .unwrap_or_default()
                                        .contains("interactive finalizer prior transcript corrupt"),
                                    "corruption did not return a bounded explicit unknown: {}",
                                    fs::read_to_string(&err).unwrap_or_default()
                                );
                                assert!(!output_receipt.exists());
                                assert!(
                                    !provider_dir
                                        .join(format!("{}.interactive-q.json", receipt.handoff_id))
                                        .exists()
                                );
                                assert_eq!(
                                    fs::read_dir(&provider_dir)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .filter(|item| item
                                            .file_name()
                                            .to_string_lossy()
                                            .ends_with(".interactive-k.json"))
                                        .count(),
                                    1
                                );
                                assert_pending_notify_without_delivery(&broker_state);
                                stop(&mut broker);
                                return;
                            }
                        }
                        let deadline = Instant::now() + Duration::from_secs(20);
                        while entry.try_wait().unwrap().is_none() && Instant::now() < deadline {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            entry.try_wait().unwrap().is_some(),
                            "resident root Q wait: root={} broker={} restart={} artifacts={:?}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default(),
                            fs::read_to_string(temp.path().join("resident-bash-restart.log"))
                                .unwrap_or_default(),
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                                .collect::<Vec<_>>()
                        );
                        assert!(
                            entry.wait().unwrap().success(),
                            "resident root Q: {}; W-restart broker: {}; after-append broker: {}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(temp.path().join("resident-bash-restart.log"))
                                .unwrap_or_default(),
                            fs::read_to_string(temp.path().join("after-append-restart.log"))
                                .unwrap_or_default()
                        );
                        let q: serde_json::Value = serde_json::from_slice(
                            &fs::read(
                                provider_dir
                                    .join(format!("{}.interactive-q.json", receipt.handoff_id)),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(q["k"]["grant"]["id"], grant_id);
                        assert_eq!(q["identity"]["provider_host_pid"], provider_pid);
                        assert_eq!(q["provider_exit"]["wait_status"], 0);
                        assert_eq!(q["tree_drain"]["zero_remaining"], true);
                        assert_eq!(q["pid1_wait"]["reaped"], true);
                        let transcript =
                            fs::read(provider_dir.join(format!("{grant_id}.interactive-output")))
                                .unwrap();
                        assert_eq!(
                            fs::read(gate.join("interactive-output-readback")).unwrap(),
                            transcript
                        );
                        assert_eq!(q["pty_output"]["bytes"], transcript.len());
                        assert_eq!(
                            q["pty_output"]["sha256"],
                            format!("{:x}", Sha256::digest(&transcript))
                        );
                        assert!(
                            transcript
                                .windows(b"interactive-ready".len())
                                .any(|part| part == b"interactive-ready")
                        );
                        assert!(transcript.windows(b"interactive-output:fixture-input-through-pty".len()).any(|part| part == b"interactive-output:fixture-input-through-pty"));
                        if resident_bash
                            && (mode.ends_with("_notify")
                                || mode.ends_with("_response")
                                || mode.ends_with("_restart")
                                || mode.ends_with("_after_append"))
                        {
                            let report: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("bash-causal-output")).unwrap(),
                            )
                            .unwrap();
                            let event: oulipoly_state::mailbox::FreshBashSourceEvent =
                                serde_json::from_value(report["fresh_source_w"].clone()).unwrap();
                            let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                            lane.accept_private_bash_source(&event).unwrap();
                        }
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".interactive-k.json"))
                                .count(),
                            1
                        );
                        let pending: i64 = rusqlite::Connection::open(
                            broker_state.join("v30/sidecar/pid-identity.db"),
                        )
                        .unwrap()
                        .query_row(
                            "SELECT count(*) FROM mailbox WHERE delivered_at IS NULL",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                        assert_eq!(pending, i64::from(resident_notify && !physical_f));
                        if mode.contains("_f_fenced") {
                            let fenced: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("interactive-f-fenced.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(fenced["grant"]["session_id"], session.session_id);
                            assert_eq!(fenced["preparation"]["session_id"], session.session_id);
                            assert_eq!(
                                fenced["preparation"]["grant_id"],
                                fenced["grant"]["grant_id"]
                            );
                            assert_eq!(fenced["fence"]["grant_id"], fenced["grant"]["grant_id"]);
                            assert_eq!(fenced["fence"]["runtime_generation_id"], grant_id);
                            assert_eq!(
                                fenced["preparation"]["tail_resume_token"],
                                readback["native_tail"]["resume_token"]
                            );
                            let bash_report: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("bash-causal-output")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(
                                fenced["preparation"]["source_id"],
                                bash_report["fresh_source_w"]["source_id"]
                            );
                            assert_eq!(
                                fenced["preparation"]["attempt_id"],
                                bash_report["fresh_source_w"]["attempt_id"]
                            );
                            let envelope = fenced["preparation"]["envelope_text"].as_str().unwrap();
                            let encoded = envelope
                                .lines()
                                .find_map(|line| line.strip_prefix("payload-base64: "))
                                .unwrap();
                            use base64::Engine as _;
                            let selected_bytes = base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .unwrap();
                            assert_eq!(
                                fenced["preparation"]["payload_sha256"],
                                format!("{:x}", Sha256::digest(&selected_bytes))
                            );
                            assert_eq!(
                                fenced["preparation"]["payload_byte_len"],
                                selected_bytes.len()
                            );
                            assert!(
                                fenced["duplicate_error"]
                                    .as_str()
                                    .unwrap()
                                    .contains("no replay")
                            );
                            let side = rusqlite::Connection::open(
                                broker_state.join("v30/sidecar/pid-identity.db"),
                            )
                            .unwrap();
                            for table in [
                                "fresh_recipient_grant",
                                "fresh_native_f_preparation",
                                "fresh_native_f_submission",
                            ] {
                                assert_eq!(
                                    side.query_row(
                                        &format!("SELECT count(*) FROM {table}"),
                                        [],
                                        |r| r.get::<_, i64>(0)
                                    )
                                    .unwrap(),
                                    1
                                );
                            }
                            assert_eq!(
                                side.query_row(
                                    "SELECT count(*) FROM fresh_recipient_ack_evidence",
                                    [],
                                    |r| r.get::<_, i64>(0)
                                )
                                .unwrap(),
                                0
                            );
                            let effect: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("interactive-effect")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(effect["input"], "fixture-input-through-pty\n");
                            let native_after_q: serde_json::Value =
                                serde_json::from_slice(&fs::read(&native_store).unwrap()).unwrap();
                            if physical_f {
                                let physical: serde_json::Value = serde_json::from_slice(
                                    &fs::read(gate.join("interactive-f-physical.json")).unwrap(),
                                )
                                .unwrap();
                                assert_eq!(native_after_q["turns"].as_array().unwrap().len(), 1);
                                assert_eq!(native_after_q["turns"][0]["body"], envelope);
                                assert_eq!(
                                    native_after_q["turns"][0]["nonce"],
                                    fenced["preparation"]["envelope_nonce"]
                                );
                                assert_eq!(
                                    physical["receipt"]["turn_id"],
                                    native_after_q["turns"][0]["turn_id"]
                                );
                                assert_eq!(
                                    physical["receipt"]["payload_sha256"],
                                    fenced["preparation"]["payload_sha256"]
                                );
                                assert_eq!(
                                    physical["transport"]["input_sha256"],
                                    fenced["fence"]["input_sha256"]
                                );
                                assert_eq!(physical["ack"]["phase"], "acked");
                                for table in [
                                    "fresh_native_f_transport",
                                    "fresh_native_f_receipt",
                                    "fresh_native_f_auto_ack",
                                ] {
                                    assert_eq!(
                                        side.query_row(
                                            &format!("SELECT count(*) FROM {table}"),
                                            [],
                                            |r| r.get::<_, i64>(0)
                                        )
                                        .unwrap(),
                                        1
                                    );
                                }
                                let lane = FreshV30Lane::open_at(&broker_state).unwrap();
                                let (root, actor) =
                                    lane.released_handoff_for_root(&prepared.root_id).unwrap();
                                let root_session = lane.read_session(&root.d_key).unwrap().unwrap();
                                let terminal = lane
                                    .settle_private_root_terminal(&root, &actor, &root_session)
                                    .unwrap();
                                assert_eq!(terminal.notification_state, "acked");
                                assert_eq!(terminal.ack_basis.as_deref(), Some("native_f_receipt"));
                                let exact = format!(
                                    "interactive-ready\r\n{}\r\nfixture-input-through-pty\r\ninteractive-output:fixture-input-through-pty\r\n",
                                    envelope.replace('\n', "\r\n")
                                );
                                assert_eq!(transcript, exact.as_bytes());
                                assert_old_pending_v29(&broker_state);
                            } else {
                                assert_eq!(native_after_q["turns"], serde_json::json!([]));
                            }
                        } else if resident_notify {
                            assert_pending_notify_without_delivery(&broker_state);
                        } else {
                            assert_old_debt_and_no_f_ack(&broker_state);
                        }
                        eprintln!(
                            "resident physical evidence: grant={grant_id} provider_host_pid={provider_pid} provider_local_pid={} pidns_ino={} native_session={} tail_token={} q_wait={} output_bytes={} output_sha256={} pending_f={pending}",
                            resident["provider_local_pid"],
                            resident["provider_pidns_ino"],
                            native["session_id"],
                            readback["native_tail"]["resume_token"],
                            q["provider_exit"]["wait_status"],
                            q["pty_output"]["bytes"],
                            q["pty_output"]["sha256"]
                        );
                        stop(&mut broker);
                        return;
                    }
                    let selected_marker = if mode == "normal_model_provider_no_pin" {
                        gate.join("provider-effect-unused")
                    } else {
                        gate.join("provider-effect")
                    };
                    if physical_success {
                        let until = Instant::now() + Duration::from_secs(30);
                        while !selected_marker.exists()
                            && entry.try_wait().unwrap().is_none()
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        if !selected_marker.exists() {
                            let artifacts = fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                                .collect::<Vec<_>>();
                            panic!(
                                "physical root stderr: {}; broker status: {:?}; artifacts: {artifacts:?}; broker log: {}",
                                fs::read_to_string(&err).unwrap_or_default(),
                                broker.try_wait().unwrap(),
                                fs::read_to_string(temp.path().join("handoff-restart.log"))
                                    .unwrap_or_default()
                            );
                        }
                    } else {
                        eventually(|| {
                            selected_marker.exists() || entry.try_wait().unwrap().is_some()
                        });
                    }
                    assert!(
                        selected_marker.exists(),
                        "{}",
                        fs::read_to_string(&err).unwrap()
                    );
                    let grant_file =
                        provider_dir.join(format!("{}.fresh-grant.json", receipt.handoff_id));
                    eventually(|| grant_file.exists());
                    let grant: serde_json::Value =
                        serde_json::from_slice(&fs::read(&grant_file).unwrap()).unwrap();
                    if mode == "normal_model_provider_pty_physical" {
                        // The provider has produced its marker and has not
                        // received the release/cancel signal. Its root PID1
                        // must remain a blocker even though this inventory
                        // does not enumerate fresh-provider sidecar work.
                        let exact: RootRecord = serde_json::from_slice(
                            &fs::read(broker_state.join(format!("{}.json", prepared.root_id)))
                                .unwrap(),
                        )
                        .unwrap();
                        let observed =
                            protocol::root_drain_readback_at(&socket, &exact, false).unwrap();
                        let observed: serde_json::Value = serde_json::from_str(
                            observed.strip_prefix("root-drain-v1 ").unwrap().trim_end(),
                        )
                        .unwrap();
                        assert_eq!(observed["root_id"], prepared.root_id);
                        oulipoly_kernel_broker::identity::PinnedProcess::open(exact.init_host_pid)
                            .unwrap()
                            .verify()
                            .unwrap();
                        // This mode restarts Broker before provider K, so
                        // parent-child wait authority is correctly unknown
                        // even while the exact root and provider remain live.
                        assert_eq!(observed["pid1"], "unknown");
                        assert_eq!(observed["state"], "blocked");
                        assert_eq!(observed["source_effect_readback_uncertain"], false);
                        assert_eq!(observed["source_effect"]["consumed_without_evidence"], 0);
                        assert_eq!(observed["state_sidecar_outstanding_unknown"], true);
                        assert_eq!(observed["close_eligible"], false);
                    }
                    if path_mode {
                        assert_eq!(
                            grant["configured_program"],
                            if mode == "normal_model_provider_prefix" {
                                "env"
                            } else {
                                "age319-provider"
                            }
                        );
                        assert!(grant["broker_resolved_path"].as_str().unwrap().contains(
                            if mode == "normal_model_provider_prefix" {
                                "/env"
                            } else {
                                "/age319-provider"
                            }
                        ));
                        assert!(
                            grant["preflight_image"]["metadata"]["inode"]
                                .as_u64()
                                .unwrap()
                                > 0
                        );
                    }
                    let route_file =
                        provider_dir.join(format!("{}.route-selection.json", receipt.handoff_id));
                    let route: serde_json::Value =
                        serde_json::from_slice(&fs::read(route_file).unwrap()).unwrap();
                    let selected_account = if mode == "normal_model_provider_no_pin" {
                        "unused"
                    } else {
                        "local"
                    };
                    assert_eq!(route["selection"]["account"], selected_account);
                    assert_eq!(
                        route["selection"]["index"],
                        if mode == "normal_model_provider_no_pin" {
                            0
                        } else {
                            1
                        }
                    );
                    assert_eq!(route["selection"]["plan_sha256"], grant["plan_sha256"]);
                    if mode.starts_with("normal_model_provider_pty_") {
                        let record_bytes = fs::read(
                            provider_dir
                                .join(format!("{}.interactive-pty-pre-k.json", receipt.handoff_id)),
                        )
                        .unwrap();
                        if let Some(before) = &pty_record_before {
                            assert_eq!(&record_bytes, before, "broker restart changed ^ record");
                        }
                        let record: serde_json::Value =
                            serde_json::from_slice(&record_bytes).unwrap();
                        assert_eq!(record["state"], "pre-k-nonactivating");
                        assert_eq!(record["binding"]["actor_pid"], actor.host_pid);
                        assert_eq!(
                            record["binding"]["session_id"],
                            route["binding"]["session_id"]
                        );
                        assert_eq!(record["selection"]["account"], "local");
                        let interactive: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!(
                                "{}.interactive-route-selection.json",
                                receipt.handoff_id
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(interactive["role"], "interactive");
                        assert_eq!(interactive["selection"]["role"], "interactive");
                        assert_eq!(
                            interactive["selection"]["account"],
                            route["selection"]["account"]
                        );
                        assert_ne!(
                            interactive["selection"]["plan_sha256"],
                            route["selection"]["plan_sha256"]
                        );
                        assert_eq!(
                            record["selection"]["plan_sha256"],
                            interactive["selection"]["plan_sha256"]
                        );
                        let candidate: serde_json::Value =
                            serde_json::from_slice(
                                &fs::read(provider_dir.join(format!(
                                    "{}.interactive-route-1.json",
                                    receipt.handoff_id
                                )))
                                .unwrap(),
                            )
                            .unwrap();
                        assert_eq!(candidate["role"], "interactive");
                        assert_eq!(candidate["account"], "local");
                        assert_eq!(
                            candidate["plan_sha256"],
                            interactive["selection"]["plan_sha256"]
                        );
                        assert_eq!(
                            candidate["broker_resolved_path"].as_str(),
                            Some(provider_image.as_str())
                        );
                        assert_eq!(
                            candidate["image_descriptor"]["inode"].as_u64(),
                            Some(fs::metadata(&provider_image).unwrap().ino())
                        );
                        assert!(candidate["cwd_inode"].as_u64().unwrap() > 0);
                        let preparation_bytes = fs::read(provider_dir.join(format!(
                            "{}.interactive-k-preparation.json",
                            receipt.handoff_id
                        )))
                        .unwrap();
                        if let Some(before) = &preparation_before {
                            assert_eq!(
                                &preparation_bytes, before,
                                "broker restart changed interactive pre-K preparation"
                            );
                        }
                        let preparation: serde_json::Value =
                            serde_json::from_slice(&preparation_bytes).unwrap();
                        assert_eq!(preparation["state"], "pre-k-nonactivating");
                        assert_eq!(preparation["handoff"], record);
                        assert_eq!(
                            preparation["image_descriptor"],
                            candidate["image_descriptor"]
                        );
                        assert_eq!(preparation["cwd_inode"], candidate["cwd_inode"]);
                        assert_eq!(
                            preparation["broker_resolved_path"],
                            candidate["broker_resolved_path"]
                        );
                        if physical_success {
                            let k: serde_json::Value = serde_json::from_slice(
                                &fs::read(
                                    provider_dir
                                        .join(format!("{}.interactive-k.json", receipt.handoff_id)),
                                )
                                .unwrap(),
                            )
                            .unwrap();
                            let q: serde_json::Value = serde_json::from_slice(
                                &fs::read(
                                    provider_dir
                                        .join(format!("{}.interactive-q.json", receipt.handoff_id)),
                                )
                                .unwrap(),
                            )
                            .unwrap();
                            let interactive_id = k["grant"]["id"].as_str().unwrap();
                            assert_eq!(k["state"], "consumed-before-child-release");
                            assert_eq!(k["preparation"], preparation);
                            assert_eq!(q["k"], k);
                            assert_eq!(q["attach"]["grant_id"], interactive_id);
                            assert_eq!(q["provider_exit"]["grant_id"], interactive_id);
                            assert_eq!(
                                q["provider_exit"]["provider_local_pid"],
                                q["attach"]["provider_local_pid"]
                            );
                            assert_eq!(q["provider_exit"]["wait_status"], 0);
                            assert_eq!(q["tree_drain"]["zero_remaining"], true);
                            assert_eq!(q["pid1_wait"]["reaped"], true);
                            assert_eq!(q["pid1_wait"]["wait_status"], 0);
                            assert!(q["attach"]["provider_pid"].as_i64().unwrap() > 0);
                            assert!(q["attach"]["provider_starttime"].as_u64().unwrap() > 0);
                            assert!(q["attach"]["pidns_ino"].as_u64().unwrap() > 0);
                            assert_eq!(
                                q["identity"]["provider_host_pid"],
                                q["attach"]["provider_pid"]
                            );
                            assert_eq!(
                                q["identity"]["provider_local_pid"],
                                q["attach"]["provider_local_pid"]
                            );
                            assert_eq!(
                                q["identity"]["provider_starttime_ticks"],
                                q["attach"]["provider_starttime"]
                            );
                            assert_eq!(
                                q["identity"]["provider_pidns_ino"],
                                q["attach"]["pidns_ino"]
                            );
                            assert_eq!(
                                q["identity"]["provider_boot_id"],
                                k["grant"]["binding"]["actor_boot_id"]
                            );
                            assert_eq!(
                                q["identity"]["pid1_boot_id"],
                                k["grant"]["binding"]["actor_boot_id"]
                            );
                            assert_eq!(
                                q["identity"],
                                serde_json::from_slice::<serde_json::Value>(
                                    &fs::read(provider_dir.join(format!(
                                        "{interactive_id}.interactive-identity.json"
                                    )))
                                    .unwrap()
                                )
                                .unwrap()
                            );
                            let fixture: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("interactive-effect")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(fixture["controlling_tty"], true);
                            assert_eq!(fixture["input"], "fixture-input-through-pty\n");
                            assert_eq!(fixture["pid"], q["attach"]["provider_local_pid"]);
                            let output = fs::read(
                                provider_dir.join(format!("{interactive_id}.interactive-output")),
                            )
                            .unwrap();
                            assert_eq!(
                                fs::read(gate.join("interactive-output-readback")).unwrap(),
                                output
                            );
                            assert!(
                                output
                                    .windows(b"interactive-ready".len())
                                    .any(|part| part == b"interactive-ready")
                            );
                            assert!(
                                output
                                    .windows(b"interactive-output:fixture-input-through-pty".len())
                                    .any(|part| part
                                        == b"interactive-output:fixture-input-through-pty")
                            );
                            assert_eq!(q["pty_output"]["bytes"], output.len());
                            assert_eq!(
                                q["pty_output"]["sha256"],
                                format!("{:x}", Sha256::digest(&output))
                            );
                            let q_readback =
                                fs::read_to_string(gate.join("interactive-q-readback")).unwrap();
                            assert!(q_readback.starts_with(&format!(
                                "fresh-interactive-drained {interactive_id} 0 "
                            )));
                            if mode == "normal_model_provider_pty_physical" {
                                eprintln!(
                                    "interactive physical evidence: grant={interactive_id} provider_host_pid={} provider_local_pid={} provider_starttime_ticks={} provider_pidns_ino={} wait_status={} output_bytes={} output_sha256={} controlling_tty={} input={:?}",
                                    q["identity"]["provider_host_pid"],
                                    q["identity"]["provider_local_pid"],
                                    q["identity"]["provider_starttime_ticks"],
                                    q["identity"]["provider_pidns_ino"],
                                    q["provider_exit"]["wait_status"],
                                    q["pty_output"]["bytes"],
                                    q["pty_output"]["sha256"],
                                    fixture["controlling_tty"],
                                    fixture["input"]
                                );
                            }
                            if mode == "normal_model_provider_pty_physical_reply_loss" {
                                assert!(
                                    gate.join("interactive-k-reply-dropped").exists(),
                                    "gate files: {:?}; broker log: {}",
                                    fs::read_dir(&gate)
                                        .unwrap()
                                        .filter_map(Result::ok)
                                        .map(|entry| entry
                                            .file_name()
                                            .to_string_lossy()
                                            .into_owned())
                                        .collect::<Vec<_>>(),
                                    fs::read_to_string(&broker_log).unwrap_or_default()
                                );
                            }
                            assert_ne!(interactive_id, grant["id"].as_str().unwrap());
                            assert_eq!(
                                fs::read_dir(&provider_dir)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".interactive-k.json"))
                                    .count(),
                                1
                            );
                        }
                        let path = Path::new(record["control_path"].as_str().unwrap());
                        assert!(path.exists(), "root control closed before provider Q");
                        assert!(
                            std::fs::read_dir(format!("/proc/{}/fd", actor.host_pid))
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter_map(|fd| fs::read_link(fd.path()).ok())
                                .any(|target| target.to_string_lossy().starts_with("/dev/ptmx")),
                            "original root actor dropped PTY master after first challenge"
                        );
                        assert_old_debt_and_no_f_ack(&broker_state);
                        if mode == "normal_model_provider_pty_replaced" {
                            let held_path = gate.join("held-root-pty.sock");
                            fs::rename(path, &held_path).unwrap();
                            let sibling = std::os::unix::net::UnixListener::bind(path).unwrap();
                            let mut sibling_master = -1;
                            let mut sibling_slave = -1;
                            assert_eq!(
                                unsafe {
                                    libc::openpty(
                                        &mut sibling_master,
                                        &mut sibling_slave,
                                        std::ptr::null_mut(),
                                        std::ptr::null(),
                                        std::ptr::null(),
                                    )
                                },
                                0
                            );
                            let sibling_master = unsafe { File::from_raw_fd(sibling_master) };
                            let sibling_slave = unsafe { File::from_raw_fd(sibling_slave) };
                            let request = protocol::PrivateFreshPtyHandoff {
                                d_key: receipt.d_key.clone(),
                                session_id: route["binding"]["session_id"].as_str().unwrap().into(),
                                role: protocol::FreshPlanRole::Interactive,
                                account: "local".into(),
                                plan_sha256: interactive["selection"]["plan_sha256"]
                                    .as_str()
                                    .unwrap()
                                    .into(),
                                control_path: path.to_path_buf(),
                            };
                            assert!(
                                protocol::private_fresh_pty_handoff_at(
                                    &socket.with_file_name("v30.sock"),
                                    &request,
                                    sibling_master.as_raw_fd(),
                                    sibling_slave.as_raw_fd()
                                )
                                .is_err(),
                                "sibling/copied endpoint replaced original root custody"
                            );
                            drop(sibling);
                        }
                        if mode == "normal_model_provider_pty_root_exit" {
                            let path = path.to_path_buf();
                            assert_eq!(unsafe { libc::kill(actor.host_pid, libc::SIGKILL) }, 0);
                            eventually(|| std::os::unix::net::UnixStream::connect(&path).is_err());
                            stop(&mut entry);
                            assert!(
                                path.exists(),
                                "abrupt root death unexpectedly unlinked endpoint"
                            );
                            assert!(grant_file.exists());
                            assert!(!gate.join("provider-runtime-result").exists());
                            assert_old_debt_and_no_f_ack(&broker_state);
                            fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                            stop(&mut broker);
                            return;
                        }
                    }
                    assert_eq!(route["binding"]["handoff_id"], receipt.handoff_id);
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_quota_available"
                            | "normal_model_provider_quota_reply_loss"
                            | "normal_model_provider_quota_restart"
                    ) {
                        let effect_dir = provider_dir
                            .join("account-effects")
                            .join(format!("{}-1-quota-first", receipt.handoff_id));
                        let effect: serde_json::Value = serde_json::from_slice(
                            &fs::read(effect_dir.join("result.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(effect["state"], "drained");
                        assert_eq!(effect["outcome"], "valid_windows");
                        if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_INDEX_V1")
                            .is_some()
                        {
                            let account = indexed_physical_account(&provider_dir, "physical-local");
                            let intent: serde_json::Value = serde_json::from_slice(
                                &fs::read(effect_dir.join("intent.json")).unwrap(),
                            )
                            .unwrap();
                            let indexed = &account["effects"][intent["id"].as_str().unwrap()];
                            assert_eq!(indexed["decision_handoff"], receipt.handoff_id);
                            assert!(indexed["route_source"].is_object());
                            assert!(indexed["candidate"].is_object());
                            assert!(indexed["consumed_k"].is_object());
                            assert!(indexed["certified_q"].is_object());
                            assert!(indexed["result"].is_object());
                        }
                        if mode == "normal_model_provider_quota_reply_loss" {
                            assert!(
                                gate.join("account-effect-reply-dropped").exists(),
                                "quota effect reply was not actually dropped"
                            );
                        }
                        assert_eq!(route["selection"]["quota_remaining_basis_points"], 8000);
                        assert_eq!(
                            route["selection"]["eligible_accounts"],
                            serde_json::json!(["unused", "local"])
                        );
                        assert_eq!(
                            fs::read_dir(&effect_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "quota reply loss caused a second effect K"
                        );
                    }
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_auth_recovery"
                            | "normal_model_provider_auth_success"
                            | "normal_model_provider_auth_reply_loss"
                            | "normal_model_provider_auth_restart"
                    ) {
                        let effects = provider_dir.join("account-effects");
                        for (kind, expected) in [
                            ("quota-first", "failed"),
                            ("auth-refresh", "refreshed"),
                            ("quota-retry", "valid_windows"),
                        ] {
                            let effect_dir =
                                effects.join(format!("{}-1-{kind}", receipt.handoff_id));
                            let effect: serde_json::Value = serde_json::from_slice(
                                &fs::read(effect_dir.join("result.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(effect["state"], "drained");
                            assert_eq!(effect["outcome"], expected);
                            if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_INDEX_V1")
                                .is_some()
                            {
                                let account =
                                    indexed_physical_account(&provider_dir, "physical-local");
                                let intent: serde_json::Value = serde_json::from_slice(
                                    &fs::read(effect_dir.join("intent.json")).unwrap(),
                                )
                                .unwrap();
                                let indexed = &account["effects"][intent["id"].as_str().unwrap()];
                                assert!(indexed["consumed_k"].is_object());
                                assert!(indexed["certified_q"].is_object());
                                assert!(indexed["result"].is_object());
                            }
                            assert_eq!(
                                fs::read_dir(&effect_dir)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".consumed.json"))
                                    .count(),
                                1,
                                "auth reply loss caused a duplicate {kind} K"
                            );
                        }
                        let auth_marker = if mode == "normal_model_provider_auth_recovery" {
                            "auth-refreshed"
                        } else {
                            "auth-ok"
                        };
                        assert_eq!(fs::read(gate.join(auth_marker)).unwrap(), b"x");
                        assert_eq!(route["selection"]["quota_remaining_basis_points"], 8000);
                    }
                    let grant_id = grant["id"].as_str().unwrap();
                    eventually(|| provider_dir.join(format!("{grant_id}.exit.json")).exists());
                    // The fixture's clean/typed exits can settle Q as soon as
                    // the exit is observed. Only a provider with an adopted
                    // child must remain undrained at this point.
                    if !caller_mode && !terminal_v3 && !typed_terminal_v3 && !shared_mode {
                        assert!(
                            !provider_dir.join(format!("{grant_id}.drain.json")).exists(),
                            "provider exit with adopted child falsely settled Q"
                        );
                    }
                    assert!(
                        protocol::private_fresh_provider_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key,
                            b'6',
                            None
                        )
                        .is_err(),
                        "sibling observed provider K"
                    );
                    assert!(
                        protocol::private_fresh_route_at(
                            &socket.with_file_name("v30.sock"),
                            &protocol::FreshRouteRequest {
                                protocol_version: 4,
                                d_key: receipt.d_key.clone(),
                                model: "configured-model".into(),
                                config_sha256: route["selection"]["config_sha256"]
                                    .as_str()
                                    .unwrap()
                                    .into(),
                                account: None,
                                account_identity: None,
                                index: None,
                                total: 2,
                                pin: (mode != "normal_model_provider_no_pin")
                                    .then(|| "local".into()),
                                quota_script: None,
                                auth_refresh_command: None,
                                environment_sha256: None,
                            },
                            b'f',
                            &[std::fs::File::open("/").unwrap().as_raw_fd()],
                        )
                        .is_err(),
                        "sibling read back fresh route"
                    );
                    assert!(
                        protocol::private_fresh_provider_at(
                            &socket.with_file_name("v30.sock"),
                            &uuid::Uuid::new_v4().to_string(),
                            b'6',
                            None
                        )
                        .is_err(),
                        "wrong D key observed provider K"
                    );
                    assert_eq!(
                        fs::read(&selected_marker).unwrap(),
                        b"one-provider-effect\n"
                    );
                    if !caller_mode && !terminal_v3 && !shared_mode {
                        assert!(
                            !gate.join("provider-runtime-result").exists(),
                            "runtime mapped a provider result before physical Q"
                        );
                    } else if gate.join("provider-runtime-result").exists() {
                        assert!(provider_dir.join(format!("{grant_id}.drain.json")).exists());
                    }
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_restart"
                            | "normal_model_provider_quota_restart"
                            | "normal_model_provider_auth_restart"
                            | "normal_model_provider_v3_quota_route_physical_restart"
                            | "normal_model_provider_v3_quota_route_manual_physical_restart"
                    ) {
                        let fresh_socket = socket.with_file_name("v30.sock");
                        stop(&mut broker);
                        assert!(
                            !provider_dir.join(format!("{grant_id}.drain.json")).exists(),
                            "broker death falsely settled adopted work"
                        );
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                "1",
                            )))
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                config_home.join("oulipoly-agent-runner").to_str().unwrap(),
                            )))
                            .envs(
                                v3_physical
                                    .then_some(("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")),
                            )
                            .envs(v3_physical.then_some((
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1",
                                "1",
                            )))
                            .stdout(Stdio::null())
                            .stderr(Stdio::from(
                                File::create(temp.path().join("broker-restart.log")).unwrap(),
                            ))
                            .spawn()
                            .unwrap();
                        let until = Instant::now() + Duration::from_secs(15);
                        while protocol::request_at(
                            &fresh_socket,
                            protocol::Operation::ObserveEntryGate,
                        )
                        .ok()
                        .as_deref()
                            != Some("entry-gate-v1 fresh-v30-closed\n")
                            && broker.try_wait().unwrap().is_none()
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        assert!(
                            protocol::request_at(
                                &fresh_socket,
                                protocol::Operation::ObserveEntryGate
                            )
                            .ok()
                            .as_deref()
                                == Some("entry-gate-v1 fresh-v30-closed\n"),
                            "fresh socket did not rebind; broker status {:?}; log {}",
                            broker.try_wait().unwrap(),
                            fs::read_to_string(temp.path().join("broker-restart.log")).unwrap()
                        );
                        assert!(
                            broker.try_wait().unwrap().is_none(),
                            "restarted broker exited: {}",
                            fs::read_to_string(temp.path().join("broker-restart.log")).unwrap()
                        );
                    }
                    if mode == "normal_model_provider_q_reply_loss"
                        || mode.ends_with("physical_q_reply_loss")
                    {
                        fs::write(gate.join("provider-drop-q-reply"), b"yes").unwrap();
                    }
                    fs::write(gate.join("provider-cancel"), b"yes").unwrap();
                    eventually(|| entry.try_wait().unwrap().is_some());
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_pty_control" | "normal_model_provider_pty_restart"
                    ) {
                        assert!(
                            !gate
                                .join(format!("root-pty-{}.sock", receipt.d_key))
                                .exists(),
                            "root control socket survived root exit"
                        );
                    }
                    if physical_success {
                        assert!(
                            !gate
                                .join(format!("root-pty-{}.sock", receipt.d_key))
                                .exists()
                        );
                    }
                    if mode == "normal_model_provider_pty_replaced" {
                        assert!(
                            gate.join(format!("root-pty-{}.sock", receipt.d_key))
                                .exists(),
                            "root removed a replacement endpoint it did not own"
                        );
                        let stderr = fs::read_to_string(&err).unwrap();
                        assert!(
                            stderr.contains("root PTY control endpoint replaced"),
                            "{stderr}"
                        );
                        assert!(!gate.join("provider-runtime-result").exists());
                        assert_old_debt_and_no_f_ack(&broker_state);
                        stop(&mut broker);
                        return;
                    }
                    let entry_status = entry.wait().unwrap();
                    if caller_mode || terminal_v3 || shared_mode {
                        let expected = if mode.ends_with("nonzero") {
                            9
                        } else if mode.ends_with("partial")
                            || mode.ends_with("lost")
                            || typed_terminal_v3
                        {
                            1
                        } else {
                            0
                        };
                        assert_eq!(
                            entry_status.code(),
                            Some(expected),
                            "runner={} broker={}",
                            String::from_utf8_lossy(&fs::read(&err).unwrap()),
                            fs::read_to_string(&broker_log).unwrap_or_default()
                        );
                    } else {
                        assert!(
                            !entry_status.success(),
                            "private provider fixture became ordinary CLI success"
                        );
                    }
                    if mode == "normal_model_provider_q_reply_loss"
                        || mode.ends_with("physical_q_reply_loss")
                    {
                        let stderr = fs::read_to_string(&err).unwrap();
                        assert!(stderr.contains("fresh provider unknown:"), "{stderr}");
                        assert!(gate.join("provider-q-reply-dropped").exists());
                        assert!(stderr.contains("\"automatic_replay\":false"), "{stderr}");
                        assert!(stderr.contains(grant_id), "{stderr}");
                        assert!(!gate.join("provider-runtime-result").exists());
                        assert!(provider_dir.join(format!("{grant_id}.drain.json")).exists());
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "lost Q reply caused a second K"
                        );
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                        assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                        assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                        stop(&mut broker);
                        return;
                    }
                    if !caller_mode && !terminal_v3 && !shared_mode {
                        assert!(
                            fs::read_to_string(&err)
                                .unwrap()
                                .contains("private provider runtime result mapped after Q; root terminal publication closed"),
                            "{}",
                            fs::read_to_string(&err).unwrap()
                        );
                    }
                    let mapped: serde_json::Value = serde_json::from_slice(
                        &fs::read(gate.join("provider-runtime-result")).unwrap_or_else(|error| {
                            panic!(
                                "mapped result absent: {error}; runner={} broker={}",
                                fs::read_to_string(&err).unwrap_or_default(),
                                fs::read_to_string(&broker_log).unwrap_or_default()
                            )
                        }),
                    )
                    .unwrap();
                    if terminal_v3 {
                        let terminal: oulipoly_state::mailbox::FreshRootTerminalReadback =
                            serde_json::from_slice(
                                &fs::read(gate.join("root-terminal-readback.json")).unwrap(),
                            )
                            .unwrap();
                        assert_eq!(
                            terminal.execution_state,
                            if typed_terminal_v3 {
                                "failure"
                            } else {
                                "success"
                            }
                        );
                        assert_eq!(
                            terminal.terminal_state,
                            if typed_terminal_v3 {
                                "execution_failed"
                            } else {
                                "execution_completed"
                            }
                        );
                        assert_eq!(terminal.publication_state, "not_started");
                        let settled = FreshV30Lane::open_at(&broker_state)
                            .unwrap()
                            .read_private_root_terminal(&receipt, &actor, &session)
                            .unwrap();
                        assert_eq!(settled.publication_state, "settled");
                        assert!(settled.publication_sha256.is_some());
                        let record: serde_json::Value = serde_json::from_slice(
                            &fs::read(
                                broker_state
                                    .join("entries")
                                    .join(format!("{}.json", receipt.old_release.prepared.root_id)),
                            )
                            .unwrap(),
                        )
                        .unwrap();
                        assert_eq!(record["terminal_settlement"]["d_key"], receipt.d_key);
                        if typed_terminal_v3 {
                            fs::write(gate.join("v3-stop-after-route"), b"yes").unwrap();
                            for channel in ["stdout", "stderr"] {
                                fs::rename(
                                    gate.join(format!("caller-control-{channel}")),
                                    gate.join(format!("first-caller-control-{channel}")),
                                )
                                .unwrap();
                            }
                        }
                        let mut second = Command::new(&runner)
                            .arg(if typed_terminal_v3 {
                                "--model"
                            } else {
                                "--help"
                            })
                            .args(if typed_terminal_v3 {
                                vec!["alias", "--pin-provider", "local", "second fixture"]
                            } else {
                                Vec::new()
                            })
                            .env("OULIPOLY_DATA_DIR", &data)
                            .env("OULIPOLY_CONFIG_HOME", &config_home)
                            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .envs(
                                (!typed_terminal_v3)
                                    .then_some(("AGE319_PRIVATE_OFFLINE_ROOT_V1", "1")),
                            )
                            .envs(
                                typed_terminal_v3.then_some(("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")),
                            )
                            .envs(
                                typed_terminal_v3
                                    .then_some(("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")),
                            )
                            .envs(
                                typed_terminal_v3
                                    .then_some(("AGE319_PRIVATE_ROOT_TERMINAL_V1", "1")),
                            )
                            .envs(
                                typed_terminal_v3
                                    .then_some(("AGE319_PRIVATE_CALLER_OUTPUT_V1", "1")),
                            )
                            .envs(
                                typed_terminal_v3.then_some((
                                    "AGE319_PRIVATE_PROVIDER_IMAGE_V1",
                                    &provider_image,
                                )),
                            )
                            .envs(typed_terminal_v3.then_some((
                                "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                                gate.join("provider-effect"),
                            )))
                            .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                            .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                            .env_remove("LD_LIBRARY_PATH")
                            .stderr(Stdio::from(
                                File::create(gate.join("second-entry.err")).unwrap(),
                            ))
                            .spawn()
                            .unwrap();
                        eventually(|| second.try_wait().unwrap().is_some());
                        let entry_count = fs::read_dir(broker_state.join("entries"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .filter(|entry| {
                                entry.path().extension().is_some_and(|ext| ext == "json")
                            })
                            .count();
                        assert_eq!(
                            entry_count,
                            1,
                            "second Runner E reserved before exact owner close: {}",
                            fs::read_to_string(gate.join("second-entry.err")).unwrap()
                        );
                        assert!(!second.wait().unwrap().success());
                        if typed_terminal_v3 {
                            for channel in ["stdout", "stderr"] {
                                let _ =
                                    fs::remove_file(gate.join(format!("caller-control-{channel}")));
                                fs::rename(
                                    gate.join(format!("first-caller-control-{channel}")),
                                    gate.join(format!("caller-control-{channel}")),
                                )
                                .unwrap();
                            }
                        }
                        assert!(!gate.join("v3-second-route-selection.json").exists());
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1,
                            "second root admission replayed provider K"
                        );
                    }
                    assert_eq!(mapped["mapped_after_q"], true);
                    if matches!(
                        mode.as_str(),
                        "normal_model_provider_quota_restart"
                            | "normal_model_provider_auth_restart"
                    ) {
                        assert_eq!(
                            mapped["quota_restart_readback"], true,
                            "quota result was not read back through restarted broker"
                        );
                    }
                    assert_eq!(
                        mapped["exit_code"],
                        if mode.ends_with("nonzero") {
                            9
                        } else if mode.ends_with("physical_capacity")
                            || mode.ends_with("physical_account_quota")
                            || typed_terminal_v3
                        {
                            1
                        } else {
                            0
                        }
                    );
                    assert_eq!(
                        mapped["provider_index"],
                        if mode == "normal_model_provider_no_pin" {
                            0
                        } else {
                            1
                        }
                    );
                    assert_eq!(mapped["model"], "configured-model");
                    assert_eq!(
                        mapped["provider"],
                        if mode == "normal_model_provider_no_pin" {
                            "unused"
                        } else {
                            "local"
                        }
                    );
                    assert_eq!(mapped["route_observed_live"], 0);
                    assert_eq!(mapped["route_observed_invocations"], 0);
                    if mode == "normal_model_provider_reply_loss"
                        || mode.ends_with("physical_reply_loss")
                    {
                        assert!(gate.join("provider-k-reply-dropped").exists());
                        let consumed: std::collections::HashSet<_> = fs::read_dir(&provider_dir)
                            .unwrap()
                            .filter_map(Result::ok)
                            .map(|entry| entry.file_name().to_string_lossy().into_owned())
                            .filter(|name| name.ends_with(".consumed.json"))
                            .collect();
                        let mut expected =
                            std::collections::HashSet::from([format!("{grant_id}.consumed.json")]);
                        if mode == "normal_model_provider_pty_physical_reply_loss" {
                            let interactive: serde_json::Value = serde_json::from_slice(
                                &fs::read(
                                    provider_dir
                                        .join(format!("{}.interactive-k.json", receipt.handoff_id)),
                                )
                                .unwrap(),
                            )
                            .unwrap();
                            let interactive_id = interactive["grant"]["id"].as_str().unwrap();
                            assert_ne!(interactive_id, grant_id);
                            expected.insert(format!("{interactive_id}.consumed.json"));
                        }
                        assert_eq!(consumed, expected, "lost K reply changed provider grants");
                    }
                    let short_output = mode.ends_with("physical_capacity")
                        || mode.ends_with("physical_account_quota")
                        || typed_terminal_v3;
                    if !short_output && !mode.ends_with("binary") {
                        assert_eq!(mapped["stdout"], "provider-stdout:hello fixture");
                    }
                    if (!v3_physical || !short_output) && !mode.ends_with("binary") {
                        assert_eq!(mapped["stderr"], "provider-stderr\n");
                    }
                    if !v3_physical {
                        if !mode.ends_with("binary") {
                            assert_eq!(mapped["stdout"], "provider-stdout:hello fixture");
                            assert_eq!(mapped["stderr"], "provider-stderr\n");
                        }
                        assert_eq!(
                            fs::read(provider_dir.join(format!("{grant_id}.stdout"))).unwrap(),
                            if mode.ends_with("binary") {
                                b"\0\xffstdout\n".as_slice()
                            } else {
                                b"provider-stdout:hello fixture".as_slice()
                            }
                        );
                        assert_eq!(
                            fs::read(provider_dir.join(format!("{grant_id}.stderr"))).unwrap(),
                            if mode.ends_with("binary") {
                                b"err\0\xfestderr".as_slice()
                            } else {
                                b"provider-stderr\n".as_slice()
                            }
                        );
                    }
                    if v3_physical {
                        if manual_route {
                            let account_effects = provider_dir.join("account-effects");
                            let physical_effects: Vec<_> = if account_effects.exists() {
                                fs::read_dir(&account_effects)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry.path().join("intent.json").exists())
                                    .map(|entry| entry.file_name())
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            assert!(
                                physical_effects.is_empty(),
                                "manual Q route spent a duplicate QuotaFirst K: {physical_effects:?}"
                            );
                            let selected: serde_json::Value = serde_json::from_slice(
                                &fs::read(gate.join("v3-route-selection.json")).unwrap(),
                            )
                            .unwrap();
                            assert_eq!(selected["account_identity"], "physical-local");
                            if mode.ends_with("physical_refresh") {
                                assert_eq!(selected["quota_remaining_basis_points"], 7600);
                            }
                        }
                        let process: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("provider-effect.process.json")).unwrap(),
                        )
                        .unwrap();
                        let expected: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("v3-provider-expected-plan.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(expected["account_identity"], "physical-local");
                        assert_eq!(process["argv"], expected["argv"]);
                        assert_eq!(process["cwd"], expected["cwd"]);
                        assert_eq!(process["env"], expected["env"]);
                        let mut expected_argv =
                            vec![gate.join("provider-effect").to_string_lossy().into_owned()];
                        if let Some(option) = provider_option {
                            expected_argv.push(option.into());
                        }
                        assert_eq!(process["argv"], serde_json::json!(expected_argv));
                        assert_eq!(
                            process["cwd"],
                            serde_json::json!(std::env::current_dir().unwrap())
                        );
                        assert_eq!(process["selected_account"], "physical-local");
                        assert_eq!(process["uid"], serde_json::json!(unsafe { libc::getuid() }));
                        assert_eq!(process["no_new_privs"], 0);
                        assert_eq!(process["seccomp"], 0);
                        assert_eq!(process["forbidden_environment"], false);
                        let terminal: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!("{grant_id}.terminal.json")))
                                .unwrap(),
                        )
                        .unwrap();
                        let expected_outcome = if mode.ends_with("physical_capacity_terminal") {
                            "model_at_capacity"
                        } else if mode.ends_with("physical_account_quota_terminal") {
                            "quota_rejected"
                        } else if terminal_v3 || shared_mode {
                            "clean"
                        } else if mode.ends_with("physical_nonzero") {
                            "generic_failure"
                        } else if mode.ends_with("physical_capacity") {
                            "model_at_capacity"
                        } else if mode.ends_with("physical_account_quota") {
                            "quota_rejected"
                        } else {
                            "cancelled"
                        };
                        assert_eq!(terminal["outcome"], expected_outcome);
                        assert_eq!(terminal["selection"]["account_identity"], "physical-local");
                        assert_eq!(terminal["grant_id"], grant_id);
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            1
                        );
                    }
                    if v3_physical && !short_output {
                        assert_eq!(
                            fs::read(provider_dir.join(format!("{grant_id}.stdout"))).unwrap(),
                            b"provider-stdout:hello fixture"
                        );
                        assert_eq!(
                            fs::read(provider_dir.join(format!("{grant_id}.stderr"))).unwrap(),
                            b"provider-stderr\n"
                        );
                    }
                    for suffix in [
                        "consumed.json",
                        "attach.json",
                        "exit.json",
                        "drain.json",
                        "pid1-wait.json",
                    ] {
                        assert!(
                            provider_dir.join(format!("{grant_id}.{suffix}")).exists(),
                            "missing {suffix}"
                        );
                    }
                    let terminal_lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let terminal = terminal_lane
                        .settle_private_root_terminal(&receipt, &actor, &session)
                        .unwrap();
                    if mode == "normal_model_provider" {
                        let wire: oulipoly_state::mailbox::FreshRootTerminalReadback =
                            serde_json::from_slice(
                                &fs::read(gate.join("root-terminal-readback.json")).unwrap(),
                            )
                            .unwrap();
                        assert_eq!(wire, terminal);
                    }
                    assert_eq!(
                        terminal.execution.as_ref().unwrap().parent.grant_id,
                        grant_id
                    );
                    assert!(terminal.execution.as_ref().unwrap().child_event.is_none());
                    assert_eq!(terminal.notification_state, "not_applicable");
                    assert!(terminal.delivery_grant_id.is_none());
                    assert_eq!(
                        terminal.publication_state,
                        if caller_mode && (mode.ends_with("lost") || mode.ends_with("partial")) {
                            "unknown"
                        } else if caller_mode || terminal_v3 || shared_mode {
                            "settled"
                        } else {
                            "not_started"
                        }
                    );
                    if caller_mode || terminal_v3 || shared_mode {
                        assert!(gate.join("caller-control-stdout").exists());
                        assert!(gate.join("caller-control-stderr").exists());
                        let expected_out = if typed_terminal_v3 {
                            b"".as_slice()
                        } else if mode.ends_with("binary") {
                            b"\0\xffstdout\n".as_slice()
                        } else {
                            b"provider-stdout:hello fixture".as_slice()
                        };
                        let expected_err = if mode.ends_with("physical_capacity_terminal") {
                            br#"{"type":"error","error":{"data":{"code":"model_at_capacity","message":"model busy"}}}"#.as_slice()
                        } else if mode.ends_with("physical_account_quota_terminal") {
                            br#"{"type":"error","error":{"data":{"message":"quota exhausted for account"}}}"#.as_slice()
                        } else if mode.ends_with("binary") {
                            b"err\0\xfestderr".as_slice()
                        } else {
                            b"provider-stderr\n".as_slice()
                        };
                        if mode.ends_with("lost") {
                            assert!(fs::read(&out).unwrap().is_empty());
                            assert!(
                                fs::read(&err)
                                    .unwrap()
                                    .windows(b"caller result write lost before bytes".len())
                                    .any(|w| w == b"caller result write lost before bytes")
                            );
                        } else if mode.ends_with("partial") {
                            assert_eq!(fs::read(&out).unwrap(), &expected_out[..1]);
                            assert!(fs::read(&err).unwrap().starts_with(expected_err));
                            assert!(fs::read(&err).unwrap().windows(b"publication remains unknown: private caller disconnected".len()).any(|w| w == b"publication remains unknown: private caller disconnected"));
                        } else {
                            assert_eq!(fs::read(&out).unwrap(), expected_out);
                            assert_eq!(fs::read(&err).unwrap(), expected_err);
                        }
                        assert!(terminal.publication_sha256.is_some());
                        let parent = &terminal.execution.as_ref().unwrap().parent;
                        let offered = oulipoly_state::mailbox::FreshRootCallerResult {
                            parent_grant_id: parent.grant_id.clone(),
                            wait_status: parent.wait_status,
                            stdout_sha256: parent.stdout_sha256.clone(),
                            stdout_len: parent.stdout_len,
                            stderr_sha256: parent.stderr_sha256.clone(),
                            stderr_len: parent.stderr_len,
                            native_f: None,
                        };
                        assert_eq!(
                            terminal_lane
                                .begin_private_root_caller_result(
                                    &receipt, &actor, &session, &offered
                                )
                                .unwrap()
                                .publication_sha256,
                            terminal.publication_sha256
                        );
                        let mut wrong = offered.clone();
                        wrong.stderr_len += 1;
                        assert!(
                            terminal_lane
                                .begin_private_root_caller_result(
                                    &receipt, &actor, &session, &wrong
                                )
                                .is_err()
                        );
                        let mut wrong_actor = actor.clone();
                        wrong_actor.starttime_ticks += 1;
                        assert!(
                            terminal_lane
                                .begin_private_root_caller_result(
                                    &receipt,
                                    &wrong_actor,
                                    &session,
                                    &offered
                                )
                                .is_err()
                        );
                    }
                    if mode == "normal_model_provider" {
                        let mut wrong_actor = actor.clone();
                        wrong_actor.starttime_ticks += 1;
                        assert!(
                            terminal_lane
                                .read_private_root_terminal(&receipt, &wrong_actor, &session)
                                .is_err()
                        );
                        let mut wrong_session = session.clone();
                        wrong_session.session_id.push_str("-wrong");
                        assert!(
                            terminal_lane
                                .read_private_root_terminal(&receipt, &actor, &wrong_session)
                                .is_err()
                        );
                        let q = provider_dir.join(format!("{grant_id}.drain.json"));
                        let held = provider_dir.join(format!("{grant_id}.drain.held"));
                        fs::rename(&q, &held).unwrap();
                        let unknown = terminal_lane
                            .read_private_root_terminal(&receipt, &actor, &session)
                            .unwrap();
                        assert_eq!(unknown.execution_state, "unknown");
                        assert_eq!(unknown.terminal_state, "execution_unknown");
                        assert!(
                            unknown
                                .unknown_stages
                                .iter()
                                .any(|stage| stage.starts_with("parent_k_q:"))
                        );
                        assert!(
                            terminal_lane
                                .begin_private_root_publication(
                                    &receipt,
                                    &actor,
                                    &session,
                                    b"cannot publish unverified Q"
                                )
                                .is_err()
                        );
                        fs::rename(&held, &q).unwrap();
                        assert_eq!(
                            terminal_lane
                                .read_private_root_terminal(&receipt, &actor, &session)
                                .unwrap()
                                .execution,
                            terminal.execution
                        );
                        let artifact = b"caller-output\nOULIPOLY_RESULT fixture\n";
                        let publication = terminal_lane
                            .begin_private_root_publication(&receipt, &actor, &session, artifact)
                            .unwrap();
                        assert_eq!(publication.publication_state, "unknown");
                        assert_eq!(publication.execution_state, terminal.execution_state);
                        assert_eq!(
                            terminal_lane
                                .begin_private_root_publication(
                                    &receipt, &actor, &session, artifact
                                )
                                .unwrap()
                                .publication_sha256,
                            publication.publication_sha256
                        );
                        assert!(
                            terminal_lane
                                .begin_private_root_publication(
                                    &receipt,
                                    &actor,
                                    &session,
                                    b"different caller bytes"
                                )
                                .is_err()
                        );
                    }
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_root_effect",
                                [],
                                |r| r.get(0)
                            )
                            .unwrap(),
                        0
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_work_preparation",
                                [],
                                |r| r.get(0)
                            )
                            .unwrap(),
                        if typed_terminal_v3 { 2 } else { 1 }
                    );
                    assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                    assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    assert_eq!(fs::read(&historical_sidecar).unwrap(), v29_main_before);
                    assert_eq!(fs::read(&v29_wal).ok(), v29_wal_before);
                    if mode == "normal_model_provider" || caller_mode {
                        stop(&mut broker);
                        broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                            .stderr(Stdio::from(
                                File::create(temp.path().join("root-terminal-restart.log"))
                                    .unwrap(),
                            ))
                            .spawn()
                            .unwrap();
                        eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                        let reopened = FreshV30Lane::open_at(&broker_state).unwrap();
                        let replay = reopened
                            .read_private_root_terminal(&receipt, &actor, &session)
                            .unwrap();
                        assert_eq!(replay.execution, terminal.execution);
                        if replay.publication_state != terminal.publication_state {
                            // The original Runner may begin its caller publication
                            // while this fixture restarts the broker. That is a
                            // forward State transition, not a changed Q/result.
                            assert_eq!(terminal.publication_state, "not_started");
                            assert!(matches!(
                                replay.publication_state.as_str(),
                                "unknown" | "settled"
                            ));
                            assert!(replay.publication_sha256.is_some());
                        }
                        assert_eq!(
                            reopened
                                .settle_private_root_terminal(&receipt, &actor, &session)
                                .unwrap()
                                .execution,
                            terminal.execution
                        );
                    }
                    if shared_mode {
                        if mode.ends_with("shared_config_changed") {
                            fs::write(
                                config_home
                                    .join("oulipoly-agent-runner/models/configured-model-two.toml"),
                                "[[providers]]\nname = \"unused\"\n",
                            )
                            .unwrap();
                        }
                        if mode.ends_with("shared_restart") {
                            stop(&mut broker);
                            let restart_log = temp.path().join("shared-broker-restart.log");
                            broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                                .env(
                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1",
                                    "1",
                                )
                                .env(
                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                                    config_home.join("oulipoly-agent-runner"),
                                )
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1", "1")
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1", "1")
                                .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                                .spawn()
                                .unwrap();
                            eventually(|| {
                                protocol::request_at(&socket, Operation::Classify).is_ok()
                                    || broker.try_wait().unwrap().is_some()
                            });
                            assert!(
                                broker.try_wait().unwrap().is_none(),
                                "shared broker restart: {}",
                                fs::read_to_string(&restart_log).unwrap()
                            );
                        }
                        let second_err = temp.path().join("second.err");
                        let mut second = Command::new(&runner)
                            .args([
                                "--model",
                                "configured-model-two",
                                "--pin-provider",
                                "local-two",
                                "hello second",
                            ])
                            .env("OULIPOLY_DATA_DIR", &data)
                            .env("OULIPOLY_CONFIG_HOME", &config_home)
                            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                            .env("AGE319_PRIVATE_REPAIR_CHALLENGE_V1", "1")
                            .env("AGE319_PRIVATE_SOURCE_SELECTION_CHALLENGE_V1", "1")
                            .env("AGE319_PRIVATE_NORMAL_ROOT_V1", "1")
                            .env("AGE319_PRIVATE_FRESH_PROVIDER_V1", "1")
                            .env("AGE319_PRIVATE_PROVIDER_IMAGE_V1", &provider_image)
                            .env("AGE319_PRIVATE_SECOND_RESULT_V1", "1")
                            .envs(
                                mode.ends_with("shared_env_changed")
                                    .then_some(("AGE319_SHARED_ENV_CHANGED_V1", "1")),
                            )
                            .env(
                                "AGE319_PRIVATE_PROVIDER_MARKER_V1",
                                gate.join("provider-effect"),
                            )
                            .env_remove("LD_LIBRARY_PATH")
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::from(File::create(&second_err).unwrap()))
                            .spawn()
                            .unwrap();
                        eventually(|| second.try_wait().unwrap().is_some());
                        let second_status = second.wait().unwrap();
                        let second_error = fs::read_to_string(&second_err).unwrap();
                        if !matches!(
                            mode.as_str(),
                            "normal_model_provider_v3_quota_route_physical_shared"
                                | "normal_model_provider_v3_quota_route_physical_shared_manual"
                                | "normal_model_provider_v3_quota_route_physical_shared_reply_loss"
                                | "normal_model_provider_v3_quota_route_physical_shared_restart"
                        ) && !mode.ends_with("shared_account_changed")
                        {
                            assert!(!second_status.success(), "changed source was admitted");
                            assert!(
                                second_error.contains("shared physical quota readback refused")
                                    || second_error.contains("config source changed")
                                    || second_error.contains("pin")
                                    || second_error.contains("candidate"),
                                "second actor: {second_error}"
                            );
                            let effects = provider_dir.join("account-effects");
                            assert_eq!(
                                fs::read_dir(&effects)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with("-quota-first"))
                                    .count(),
                                1,
                                "incompatible source announced a second quota K"
                            );
                            assert!(!gate.join("provider-effect-second").exists());
                            assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                            assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                            stop(&mut broker);
                            return;
                        }
                        assert!(
                            !second_status.success()
                                && second_error
                                    .contains("private provider runtime result mapped after Q"),
                            "second actor: {second_error}; broker status: {:?}; broker: {}",
                            broker.try_wait().unwrap(),
                            fs::read_to_string(&broker_log).unwrap_or_default()
                        );
                        let second_mapped: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("provider-runtime-result-second")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(second_mapped["model"], "configured-model-two");
                        if mode.ends_with("shared_reply_loss") {
                            assert!(gate.join("account-effect-reply-dropped").exists());
                        }
                        if mode.ends_with("shared_account_changed") {
                            assert_eq!(
                                second_mapped["shared_quota_receipts"]
                                    .as_array()
                                    .unwrap()
                                    .len(),
                                0,
                                "a different physical account reused Q"
                            );
                            assert_eq!(second_mapped["provider"], "local-two");
                            let effects = provider_dir.join("account-effects");
                            assert_eq!(
                                fs::read_dir(&effects)
                                    .unwrap()
                                    .filter_map(Result::ok)
                                    .filter(|entry| entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with("-quota-first"))
                                    .count(),
                                2,
                                "different physical account needs its own quota K"
                            );
                            assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                            assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                            stop(&mut broker);
                            return;
                        }
                        assert_eq!(
                            second_mapped["shared_quota_receipts"]
                                .as_array()
                                .unwrap()
                                .len(),
                            1
                        );
                        assert_eq!(
                            second_mapped["route_config_sha256"].as_str().unwrap().len(),
                            64
                        );
                        let effects = provider_dir.join("account-effects");
                        assert_eq!(
                            fs::read_dir(&effects)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with("-quota-first"))
                                .count(),
                            if shared_manual { 0 } else { 1 },
                            "second actor announced another quota effect"
                        );
                        assert_eq!(
                            fs::read_dir(&provider_dir)
                                .unwrap()
                                .filter_map(Result::ok)
                                .filter(|entry| entry
                                    .file_name()
                                    .to_string_lossy()
                                    .ends_with(".consumed.json"))
                                .count(),
                            2,
                            "second actor did not consume its own provider K"
                        );
                        let receipts: Vec<_> = fs::read_dir(broker_state.join("released-handoffs"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .collect();
                        assert_eq!(
                            receipts.len(),
                            2,
                            "two original roots need separate receipts"
                        );
                        let released: Vec<oulipoly_state::mailbox::FreshReleasedHandoff> = receipts
                            .iter()
                            .map(|entry| {
                                serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap()
                            })
                            .collect();
                        assert_ne!(released[0].d_key, released[1].d_key);
                        let second_receipt = released
                            .iter()
                            .find(|value| value.d_key != receipt.d_key)
                            .unwrap();
                        assert_ne!(
                            second_receipt.old_release.prepared.root_id,
                            receipt.old_release.prepared.root_id
                        );
                        let second_route: serde_json::Value = serde_json::from_slice(
                            &fs::read(provider_dir.join(format!(
                                "{}.route-selection.json",
                                second_receipt.handoff_id
                            )))
                            .unwrap(),
                        )
                        .unwrap();
                        let second_grant: serde_json::Value =
                            serde_json::from_slice(
                                &fs::read(provider_dir.join(format!(
                                    "{}.fresh-grant.json",
                                    second_receipt.handoff_id
                                )))
                                .unwrap(),
                            )
                            .unwrap();
                        assert_eq!(
                            second_route["binding"]["handoff_id"],
                            second_receipt.handoff_id
                        );
                        assert_eq!(second_route["selection"]["account"], "local-two");
                        assert_eq!(
                            second_route["selection"]["plan_sha256"],
                            second_grant["plan_sha256"]
                        );
                        assert_eq!(
                            second_route["selection"]["config_sha256"],
                            second_mapped["route_config_sha256"]
                        );
                        let second_process: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("provider-effect-second.process.json")).unwrap(),
                        )
                        .unwrap();
                        let expected_plan: serde_json::Value = serde_json::from_slice(
                            &fs::read(gate.join("v3-provider-expected-plan.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(second_process["argv"], expected_plan["argv"]);
                        assert_eq!(second_process["cwd"], expected_plan["cwd"]);
                        assert_eq!(second_process["env"], expected_plan["env"]);
                        assert_eq!(second_process["selected_account"], "physical-local");
                        assert_eq!(second_process["no_new_privs"], 0);
                        assert_eq!(second_process["seccomp"], 0);
                        assert_eq!(second_process["forbidden_environment"], false);
                        assert_eq!(fs::read(&old_state_path).unwrap(), old_state_before);
                        assert_eq!(fs::read(&old_wal_path).ok(), old_wal_before);
                    }
                    stop(&mut broker);
                    return;
                }
                if physical_normal_bad_source {
                    let until = Instant::now() + Duration::from_secs(20);
                    while !gate.join("normal-pre-k-ready").exists()
                        && entry.try_wait().unwrap().is_none()
                    {
                        assert!(Instant::now() < until, "normal pre-K gate timed out");
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    assert!(gate.join("normal-pre-k-ready").exists());
                    let script = temp.path().join("normal-provider.sh");
                    let original = temp.path().join("normal-provider.saved");
                    fs::rename(&script, &original).unwrap();
                    fs::write(&script, b"#!/bin/sh\nexit 87\n").unwrap();
                    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
                    fs::write(gate.join("normal-pre-k-mutated"), b"changed source").unwrap();
                    let until = Instant::now() + Duration::from_secs(20);
                    while !gate.join("normal-pre-k-refused").exists()
                        && entry.try_wait().unwrap().is_none()
                    {
                        assert!(Instant::now() < until, "changed source K refusal timed out");
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    assert!(gate.join("normal-pre-k-refused").exists());
                    let count: i64 = rusqlite::Connection::open(broker_state.join("v30/state.db"))
                        .unwrap()
                        .query_row("SELECT count(*) FROM fresh_normal_provider_k", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(count, 0, "changed source consumed K");
                    assert!(!normal_effect.exists(), "changed source launched provider");
                    assert!(!entry.wait().unwrap().success());
                    assert!(
                        fs::read_to_string(&err)
                            .unwrap()
                            .contains("changed executable refused before K")
                    );
                    stop(&mut broker);
                    return;
                }
                if physical_normal {
                    let until = Instant::now() + Duration::from_secs(20);
                    while !gate.join("normal-k-sent").exists()
                        && entry.try_wait().unwrap().is_none()
                    {
                        assert!(
                            Instant::now() < until,
                            "normal K gate timed out: runner: {}; broker: {}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default()
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    assert!(
                        gate.join("normal-k-sent").exists(),
                        "normal K refused before reply: {}; broker: {}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                    );
                    stop(&mut broker);
                    let restart_log = temp.path().join("normal-k-restarted-broker.log");
                    broker =
                        restart_source_broker(&socket, &broker_state, &runner, &gate, &restart_log);
                    fs::write(gate.join("normal-k-resume"), b"restart-observed").unwrap();
                }
                if physical_normal {
                    let until = Instant::now() + Duration::from_secs(35);
                    while entry.try_wait().unwrap().is_none() {
                        assert!(
                            Instant::now() < until,
                            "normal physical root still live: runner: {}; broker: {}",
                            fs::read_to_string(&err).unwrap_or_default(),
                            fs::read_to_string(&broker_log).unwrap_or_default()
                        );
                        std::thread::sleep(Duration::from_millis(20));
                    }
                } else {
                    eventually(|| entry.try_wait().unwrap().is_some());
                }
                let completed = entry.wait().unwrap().success();
                if mode == "normal_model_held" {
                    assert!(!completed, "normal provider route must refuse before spawn");
                    assert!(
                        fs::read_to_string(&err)
                            .unwrap()
                            .contains("normal provider route held"),
                        "{}; physical store: {:?}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_dir(broker_state.join("v30/normal-provider")).map(|entries| {
                            entries
                                .filter_map(Result::ok)
                                .map(|entry| {
                                    let files = fs::read_dir(entry.path())
                                        .ok()
                                        .into_iter()
                                        .flatten()
                                        .filter_map(Result::ok)
                                        .map(|file| {
                                            (file.file_name(), fs::read_to_string(file.path()).ok())
                                        })
                                        .collect::<Vec<_>>();
                                    (entry.file_name(), files)
                                })
                                .collect::<Vec<_>>()
                        }),
                    );
                    let settled_lane = FreshV30Lane::open_at(&broker_state).unwrap();
                    let preparation = settled_lane
                        .read_normal_work(&receipt, &actor, &session)
                        .unwrap()
                        .unwrap();
                    let invocation = preparation
                        .headless_model_invocation(&receipt, &session)
                        .unwrap();
                    assert_eq!(invocation.root_id, receipt.old_release.prepared.root_id);
                    assert_eq!(
                        invocation.owner_generation,
                        receipt.old_release.prepared.owner_generation
                    );
                    assert_eq!(invocation.actor, actor);
                    assert_eq!(invocation.model, "fixture-model");
                    assert_eq!(invocation.provider_pin, None);
                    assert_eq!(invocation.prompt, "hello fixture");
                    let selected = settled_lane
                        .read_normal_model_selection(&receipt, &actor, &session)
                        .unwrap()
                        .unwrap();
                    assert_eq!(selected.invocation, invocation);
                    assert_eq!(selected.account, "local");
                    assert_eq!(selected.account_identity, "physical-local");
                    assert_eq!(selected.index, 0);
                    assert_eq!(selected.total, 1);
                    assert_eq!(selected.state, "selected_no_effect");
                    let plan = settled_lane
                        .read_normal_executable_plan(&receipt, &actor, &session)
                        .unwrap()
                        .unwrap();
                    assert_eq!(plan.selection, selected);
                    assert_eq!(plan.state, "planned_no_effect");
                    assert_eq!(plan.plan_sha256.len(), 64);
                    assert_eq!(plan.stdin_len, "hello fixture".len() as u64);
                    assert_eq!(plan.argv.first().map(String::as_str), None);
                    let admission = settled_lane
                        .read_normal_provider_admission(&receipt, &actor, &session)
                        .unwrap()
                        .unwrap();
                    assert_eq!(admission.handoff_id, receipt.handoff_id);
                    assert_eq!(admission.plan_sha256, plan.plan_sha256);
                    assert_eq!(admission.state, "admitted_no_effect");
                    if physical_normal {
                        let k = settled_lane
                            .read_normal_provider_k(&receipt, &actor, &session)
                            .unwrap()
                            .unwrap();
                        assert_eq!(k.admission_id, admission.admission_id);
                        assert_eq!(k.plan_sha256, plan.plan_sha256);
                        assert_eq!(k.state, "consumed");
                        let physical_dir = broker_state
                            .join("v30/normal-provider")
                            .join(&k.admission_id);
                        let q: serde_json::Value =
                            serde_json::from_slice(&fs::read(physical_dir.join("q.json")).unwrap())
                                .unwrap();
                        let parent: serde_json::Value = serde_json::from_slice(
                            &fs::read(physical_dir.join("parent-wait.json")).unwrap(),
                        )
                        .unwrap();
                        assert_eq!(q["admission_id"], k.admission_id);
                        assert_eq!(q["plan_sha256"], k.plan_sha256);
                        assert_eq!(q["provider_wait_status"], 0);
                        assert_eq!(q["tree_drained"], true);
                        assert_eq!(parent["admission_id"], k.admission_id);
                        assert_eq!(fs::read_to_string(&normal_effect).unwrap(), "one\n");
                        let stdout = fs::read(physical_dir.join("stdout")).unwrap();
                        let stderr = fs::read(physical_dir.join("stderr")).unwrap();
                        assert_eq!(stdout, b"normal-provider-out:hello fixture");
                        assert_eq!(stderr, b"normal-provider-err\n");
                        assert_eq!(
                            q["stdout"]["sha256"],
                            format!("{:x}", Sha256::digest(&stdout))
                        );
                        assert_eq!(
                            q["stderr"]["sha256"],
                            format!("{:x}", Sha256::digest(&stderr))
                        );
                    }
                    assert_eq!(
                        settled_lane
                            .admit_normal_provider_plan(&receipt, &actor, &session, &plan)
                            .unwrap(),
                        admission
                    );
                    let mut changed_plan = plan.clone();
                    changed_plan.argv.push("unexpected".into());
                    assert!(
                        settled_lane
                            .admit_normal_provider_plan(&receipt, &actor, &session, &changed_plan,)
                            .is_err()
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_executable_plan",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        1
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_provider_k",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap(),
                        if physical_normal { 1 } else { 0 },
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_provider_admission",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        1
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_model_selection",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap(),
                        1
                    );
                    let mut swapped_actor = preparation.clone();
                    swapped_actor.actor.starttime_ticks += 1;
                    assert!(
                        swapped_actor
                            .headless_model_invocation(&receipt, &session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_model_selection(&receipt, &swapped_actor.actor, &session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_executable_plan(&receipt, &swapped_actor.actor, &session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_provider_admission(
                                &receipt,
                                &swapped_actor.actor,
                                &session,
                            )
                            .is_err()
                    );
                    let mut swapped_session = session.clone();
                    swapped_session.session_id = uuid::Uuid::new_v4().to_string();
                    assert!(
                        preparation
                            .headless_model_invocation(&receipt, &swapped_session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_model_selection(&receipt, &actor, &swapped_session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_executable_plan(&receipt, &actor, &swapped_session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_provider_admission(&receipt, &actor, &swapped_session)
                            .is_err()
                    );
                    let mut swapped_root = receipt.clone();
                    swapped_root.old_release.prepared.root_id = uuid::Uuid::new_v4().to_string();
                    assert!(
                        preparation
                            .headless_model_invocation(&swapped_root, &session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_model_selection(&swapped_root, &actor, &session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_executable_plan(&swapped_root, &actor, &session)
                            .is_err()
                    );
                    assert!(
                        settled_lane
                            .read_normal_provider_admission(&swapped_root, &actor, &session)
                            .is_err()
                    );
                    let mut malformed = preparation.clone();
                    malformed.intent =
                        oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(vec![
                            "--model".into(),
                            "fixture-model".into(),
                            "--pin-provider".into(),
                            "".into(),
                            "hello fixture".into(),
                        ]);
                    let mut matching_receipt = receipt.clone();
                    matching_receipt.root_work_intent = malformed.intent.clone();
                    assert!(
                        malformed
                            .headless_model_invocation(&matching_receipt, &session)
                            .is_err()
                    );
                    let held: (String, String) = fresh_state.query_row(
                        "SELECT state,intent_json FROM fresh_normal_work_preparation WHERE handoff_id=?1",
                        [&receipt.handoff_id],
                        |row| Ok((row.get(0)?,row.get(1)?)),
                    ).unwrap();
                    assert_eq!(held.0, "held");
                    assert_eq!(
                        serde_json::from_str::<oulipoly_state::mailbox::FreshRootWorkIntent>(
                            &held.1
                        )
                        .unwrap(),
                        receipt.root_work_intent
                    );
                    assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_root_effect",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        0
                    );
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_normal_work_preparation",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        1
                    );
                    let source = File::open(config_home.join("oulipoly-agent-runner")).unwrap();
                    assert!(
                        protocol::launch_fresh_normal_provider_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key,
                            [source.as_raw_fd(), source.as_raw_fd(), source.as_raw_fd()],
                        )
                        .is_err(),
                        "a later sibling launched K after the original actor exited",
                    );
                    assert_eq!(
                        oulipoly_kernel_broker::normal_model_selection::observe(
                            &settled_lane,
                            &receipt,
                            &actor,
                            &session,
                            &source,
                        )
                        .unwrap(),
                        Some(selected)
                    );
                    fs::write(
                        config_home.join("oulipoly-agent-runner/providers.toml"),
                        "[local]\ncommand = 'echo changed'\nquota_account_id = 'physical-local'\n",
                    )
                    .unwrap();
                    assert!(
                        oulipoly_kernel_broker::normal_model_selection::observe(
                            &settled_lane,
                            &receipt,
                            &actor,
                            &session,
                            &source,
                        )
                        .is_err()
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key
                        )
                        .is_err(),
                        "same-image sibling prepared normal work"
                    );
                    assert!(
                        protocol::observe_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &receipt.d_key
                        )
                        .is_err(),
                        "same-image sibling observed normal work"
                    );
                    assert!(
                        protocol::prepare_fresh_normal_work_at(
                            &socket.with_file_name("v30.sock"),
                            &uuid::Uuid::new_v4().to_string()
                        )
                        .is_err(),
                        "wrong D key prepared normal work"
                    );
                } else {
                    let effect: String = fresh_state
                        .query_row(
                            "SELECT state FROM fresh_root_effect WHERE handoff_id=?1",
                            [&receipt.handoff_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    if mode == "normal_handoff_effect_reply_loss" {
                        assert!(!completed, "lost start reply executed root work");
                        assert_eq!(effect, "started", "lost start is durable unknown");
                        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                        assert!(
                            fs::read_to_string(&err)
                                .unwrap()
                                .contains("root effect start reply lost; execution refused")
                        );
                    } else {
                        assert!(
                            completed,
                            "handoff child: {}",
                            fs::read_to_string(&err).unwrap()
                        );
                        assert_eq!(effect, "returned_success");
                        if mode == "normal_help" {
                            let help = fs::read_to_string(&out).unwrap();
                            assert!(help.contains("Usage:"), "real CLI help absent: {help}");
                            assert!(!help.contains("OULIPOLY_KERNEL_V30_CHILD_EFFECT"));
                        } else {
                            assert_eq!(
                                fs::read_to_string(&out).unwrap(),
                                format!(
                                    "OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n",
                                    released.release_id
                                )
                            );
                        }
                    }
                    assert_eq!(
                        fresh_state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM fresh_root_effect",
                                [],
                                |row| row.get(0),
                            )
                            .unwrap(),
                        1
                    );
                }
                stop(&mut broker);
                if mode == "normal_model_held" {
                    let reopened = FreshV30Lane::open_at(&broker_state).unwrap();
                    assert_eq!(
                        reopened
                            .read_normal_provider_admission(&receipt, &actor, &session)
                            .unwrap()
                            .unwrap()
                            .state,
                        "admitted_no_effect"
                    );
                    if physical_normal {
                        let k = reopened
                            .read_normal_provider_k(&receipt, &actor, &session)
                            .unwrap()
                            .unwrap();
                        let admission = reopened
                            .read_normal_provider_admission(&receipt, &actor, &session)
                            .unwrap()
                            .unwrap();
                        assert_eq!(k.admission_id, admission.admission_id);
                        assert_eq!(fs::read_to_string(&normal_effect).unwrap(), "one\n");
                    }
                }
                return;
            }
            let repair_read = protocol::StateReadSpec {
                protocol: "broker-repair-read-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                attempt_id: None,
            };
            assert!(protocol::read_bounded_repair_at(&socket, &repair_read).is_err());
            let source_read = protocol::StateReadSpec {
                protocol: "broker-source-selection-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                attempt_id: None,
            };
            assert!(protocol::read_bounded_source_selection_at(&socket, &source_read).is_err());
            let grant_read = protocol::StateReadSpec {
                protocol: "broker-source-grant-read-v30".into(),
                ..source_read
            };
            assert!(protocol::read_source_effect_grant_at(&socket, &grant_read).is_err());
            let recipient_read = protocol::StateReadSpec {
                protocol: "broker-recipient-selection-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                attempt_id: None,
            };
            assert!(
                protocol::read_bounded_recipient_selection_at(&socket, &recipient_read).is_err()
            );
            let repair_write = protocol::StateWriteSpec {
                protocol: "broker-repair-write-v30".into(),
                source_generation: generation.clone(),
                root_id: prepared.root_id.clone(),
                owner_generation: prepared.owner_generation.clone(),
                action: protocol::StateWriteAction::Repair {
                    expected_ordinal: 0,
                },
            };
            assert!(protocol::write_bounded_repair_at(&socket, &repair_write).is_err());
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            let post_death = matches!(
                mode.as_str(),
                "normal_guardian_post"
                    | "normal_driver_post"
                    | "normal_broker_post"
                    | "normal_recipient_broker_post"
                    | "normal_recipient_driver_post"
            );
            let mut damaged_snapshot = None;
            if mode == "normal_guardian_post" {
                unsafe { libc::kill(prepared.guardian.host_pid, libc::SIGKILL) };
            } else if mode == "normal_driver_post" || mode == "normal_recipient_driver_post" {
                unsafe { libc::kill(prepared.driver.host_pid, libc::SIGKILL) };
            } else if mode == "normal_broker_post" || mode == "normal_recipient_broker_post" {
                stop(&mut broker);
                let restart_log = temp.path().join("normal-post-restart.log");
                broker = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                    .spawn()
                    .unwrap();
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
            }
            if !post_death && pending_binding.is_some() {
                eventually(|| {
                    gate.join("source-grant-ready").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-grant-ready").exists(),
                    "source grant boundary: {}",
                    fs::read_to_string(&err).unwrap_or_default()
                );
                // A sibling caller cannot turn the driver's reserved grant
                // into a physical source effect by naming its root/owner.
                let launch = protocol::StateWriteSpec {
                    protocol: "broker-source-effect-launch-v30".into(),
                    source_generation: generation.clone(),
                    root_id: prepared.root_id.clone(),
                    owner_generation: prepared.owner_generation.clone(),
                    action: protocol::StateWriteAction::LaunchSourceGrant,
                };
                assert!(protocol::launch_source_effect_grant_at(&socket, &launch).is_err());
                let mut stale = launch;
                stale.owner_generation = uuid::Uuid::new_v4().to_string();
                assert!(protocol::launch_source_effect_grant_at(&socket, &stale).is_err());
                if nonzero_source {
                    let source = pending_binding.as_ref().unwrap().registration().unwrap();
                    let path = Path::new(&source.handle_dir).join(&source.snapshot_relative);
                    let original = fs::read(&path).unwrap();
                    fs::write(&path, b"invalid v2 snapshot").unwrap();
                    damaged_snapshot = Some(SnapshotRestore {
                        path,
                        bytes: original,
                    });
                    fs::write(gate.join("source-allow-launch"), b"yes").unwrap();
                    // The original guardian must stay pinned until W has
                    // consumed the grant and acknowledged the held worker.
                    eventually(|| gate.join("source-launched").exists());
                } else if io_failure_source {
                    // Active capture races the test after terminal. Occupy
                    // the create-new name before W can release the worker.
                    use std::os::unix::fs::OpenOptionsExt;
                    let grant_id = fs::read_to_string(gate.join("source-grant-ready")).unwrap();
                    let evidence_path = broker_state
                        .join("source-physical")
                        .join(format!("{grant_id}.evidence.json"));
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(evidence_path)
                        .unwrap();
                    fs::write(gate.join("source-allow-launch"), b"yes").unwrap();
                    eventually(|| gate.join("source-launched").exists());
                } else if real_source {
                    eventually(|| gate.join("source-grant-consumed-readback").exists());
                }
            }
            if (recipient_mode || mode == "normal_empty") && !post_death {
                // Keep the exact joined owner live until the driver's
                // broker read has completed. Releasing the child first can
                // legitimately turn that read into dead-peer refusal.
                eventually(|| {
                    fs::read_to_string(&err).is_ok_and(|message| message.contains("v30 "))
                });
            }
            fs::write(gate.join("child-effect"), b"yes").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            if (real_source || recipient_mode || mode == "normal_empty") && !post_death {
                // The joined child can exit before the outside driver finishes
                // W, especially when the private prelaunch gate is held.
                let deadline = Instant::now() + Duration::from_secs(20);
                while !fs::read_to_string(&err).is_ok_and(|message| message.contains("v30 ")) {
                    assert!(
                        Instant::now() < deadline,
                        "driver did not report boundary: mode={mode} entry={} broker={} driver={:?}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                        prepared.driver,
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            if post_death {
                assert!(!entry.wait().unwrap().success());
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            } else {
                let entry_error = fs::read_to_string(&err).unwrap_or_default();
                let expected_stop = if recipient_mode {
                    Some(
                        "v30 recipient effect closed: no broker-authenticated live recipient or exact wake successor, durable one-use work grant, or pinned provider K/physical child tree",
                    )
                } else if mode == "normal_empty" {
                    Some("v30 no pending broker recipient; wake effect refused")
                } else if real_source && mode != "normal_bash_source_lost_reply" {
                    Some("v30 source physically launched; v2 acceptance remains closed")
                } else if real_source {
                    Some("v30 source launch uncertain or refused")
                } else {
                    None
                };
                if let Some(expected_stop) = expected_stop {
                    assert!(
                        entry_error.contains(expected_stop),
                        "normal repair boundary: entry={entry_error} broker={} status={:?} launched={} physical={:?}",
                        fs::read_to_string(&broker_log).unwrap_or_default(),
                        entry.try_wait().unwrap(),
                        gate.join("source-launched").exists(),
                        fs::read_dir(broker_state.join("source-physical"))
                            .unwrap()
                            .filter_map(Result::ok)
                            .map(|entry| entry.file_name())
                            .collect::<Vec<_>>(),
                    );
                } else {
                    assert!(!gate.join("source-launched").exists());
                }
                if recipient_mode || mode == "normal_empty" {
                    assert!(
                        fs::read(&out).unwrap().is_empty(),
                        "a refused recipient/wake path cannot create child effect"
                    );
                } else {
                    assert_eq!(
                        fs::read_to_string(&out).unwrap(),
                        format!("OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n", released.release_id),
                        "root: {}; broker: {}",
                        fs::read_to_string(&err).unwrap_or_default(),
                        fs::read_to_string(&broker_log).unwrap_or_default()
                    );
                }
                let projected = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let ordinal: i64 = projected
                    .query_row(
                        "SELECT COALESCE((SELECT authority_ordinal FROM completion_authority_continuity),0)",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    ordinal,
                    if recipient_mode || mode == "normal_empty" {
                        0
                    } else {
                        1
                    }
                );
                if let Some(binding) = &pending_binding {
                    let source_id = &binding.registration().unwrap().registration_id;
                    let phase: String = projected
                        .query_row(
                            "SELECT phase FROM completion_continuation_source WHERE registration_id=?1",
                            [source_id],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(phase, "registered");
                }
                let attempts: i64 = projected
                    .query_row(
                        "SELECT count(*) FROM completion_continuation_attempt WHERE operation='source_recovery'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(attempts, 0, "source preview cannot reserve or launch");
                if let Some(binding) = &pending_binding {
                    let source_id = &binding.registration().unwrap().registration_id;
                    let grant: (String, String, String, Vec<u8>, String, i64) = projected
                        .query_row(
                            "SELECT phase,registration_id,registration_digest,registration_bytes,
                         owner_generation,revision
                         FROM broker_source_effect_grant",
                            [],
                            |row| {
                                Ok((
                                    row.get(0)?,
                                    row.get(1)?,
                                    row.get(2)?,
                                    row.get(3)?,
                                    row.get(4)?,
                                    row.get(5)?,
                                ))
                            },
                        )
                        .unwrap();
                    assert_eq!(grant.0, if real_source { "consumed" } else { "reserved" });
                    assert_eq!(grant.1, *source_id);
                    assert_eq!(grant.2, binding.registration_digest());
                    assert_eq!(grant.3, binding.registration_bytes());
                    assert_eq!(grant.4, prepared.owner_generation);
                    assert_eq!(grant.5, if real_source { 2 } else { 1 });
                    if real_source {
                        assert_eq!(
                            gate.join("source-launched").exists(),
                            mode != "normal_bash_source_lost_reply"
                        );
                        let physical = broker_state.join("source-physical");
                        let grant_id = fs::read_to_string(gate.join("source-grant-ready")).unwrap();
                        eventually(|| {
                            fs::read_dir(&physical)
                                .unwrap()
                                .filter_map(Result::ok)
                                .any(|entry| {
                                    entry
                                        .file_name()
                                        .to_string_lossy()
                                        .ends_with(".terminal.json")
                                })
                        });
                        eventually(|| {
                            matches!(
                                SourcePhysicalRegistry::open(&physical)
                                    .unwrap()
                                    .observe(&grant_id)
                                    .unwrap(),
                                SourceObservation::Drained { .. }
                            )
                        });
                        let observation = SourcePhysicalRegistry::open(&physical)
                            .unwrap()
                            .observe(&grant_id)
                            .unwrap();
                        assert!(matches!(observation, SourceObservation::Drained {
                        worker_wait_status, cancel_requested: false, ..
                    } if (worker_wait_status == 0) != nonzero_source));
                        let mut retained = BrokerSidecar::open_existing(
                            &broker_state.join("sidecar/pid-identity.db"),
                            &broker_state,
                        )
                        .unwrap();
                        let custody = SourcePhysicalRegistry::open(&physical).unwrap();
                        eventually(|| {
                            retained
                                .read_source_evidence(&custody.records()[0].grant)
                                .unwrap()
                                .is_some()
                        });
                        if nonzero_source {
                            assert!(
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id
                                )
                                .is_err()
                            );
                            assert!(oulipoly_kernel_broker::source_acceptance::capture_and_stage_v2_evidence(
                            &mut retained, &custody, &grant_id
                        ).is_err());
                            assert_eq!(
                                retained
                                    .read_source_evidence(&custody.records()[0].grant)
                                    .unwrap()
                                    .unwrap()
                                    .phase,
                                "unknown"
                            );
                            drop(damaged_snapshot.take().unwrap());
                        } else if io_failure_source {
                            assert!(oulipoly_kernel_broker::source_acceptance::capture_and_stage_v2_evidence(
                            &mut retained, &custody, &grant_id
                        ).is_err(), "create-new evidence write must fail closed");
                            assert_eq!(
                                retained
                                    .read_source_evidence(&custody.records()[0].grant)
                                    .unwrap()
                                    .unwrap()
                                    .phase,
                                "unknown"
                            );
                        } else {
                            let assessed =
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id,
                                )
                                .unwrap();
                            assert_eq!(
                                assessed.registration_id,
                                pending_binding
                                    .as_ref()
                                    .unwrap()
                                    .registration()
                                    .unwrap()
                                    .registration_id
                            );
                            let captured =
                                oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                    &retained, &custody, &grant_id
                                ).unwrap();
                            assert_eq!(captured.candidate, assessed);
                            assert_eq!(
                                retained
                                    .read_source_evidence(&custody.records()[0].grant)
                                    .unwrap()
                                    .unwrap()
                                    .phase,
                                "captured"
                            );
                            assert!(oulipoly_kernel_broker::source_acceptance::capture_and_stage_v2_evidence(
                            &mut retained, &custody, &grant_id
                        ).is_err(), "duplicate capture cannot replace broker-owned bytes");
                            assert!(
                                oulipoly_kernel_broker::source_acceptance::commit_v2_evidence(
                                    &mut retained,
                                    &custody,
                                    &grant_id
                                )
                                .is_err(),
                                "re-admitted old v29 source has no exact H authority"
                            );
                            assert!(oulipoly_kernel_broker::source_acceptance::accept_v2_completion_source(
                                &mut retained, &custody, &grant_id,
                            ).is_err(), "old v29 row acquired Broker source acceptance");
                            assert!(oulipoly_kernel_broker::source_acceptance::decide_v2_source_retention_release(
                                &mut retained, &custody, &grant_id,
                            ).is_err(), "old v29 row acquired Broker release");
                            assert!(oulipoly_kernel_broker::source_acceptance::trigger_v2_completion_source(
                                &mut retained, &custody, &grant_id,
                            ).is_err(), "old v29 row triggered a Broker completion");
                            assert!(
                                retained
                                    .read_source_retention_release(&custody.records()[0].grant)
                                    .unwrap()
                                    .is_none()
                            );
                            let reply: serde_json::Value = serde_json::from_slice(
                                &fs::read(physical.join(format!("{grant_id}.stdout"))).unwrap(),
                            )
                            .unwrap();
                            let verified = oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                        pending_binding.as_ref().unwrap()).unwrap();
                            verified.validate_source_reply(&reply).unwrap();
                            let source = pending_binding.as_ref().unwrap().registration().unwrap();
                            let snapshot =
                                Path::new(&source.handle_dir).join(&source.snapshot_relative);
                            let original = fs::read(&snapshot).unwrap();
                            let mut changed = original.clone();
                            changed.push(b' ');
                            fs::write(&snapshot, &changed).unwrap();
                            let changed_evidence = oulipoly_state::completion_continuation::VerifiedCompletion::from_source_files(
                        pending_binding.as_ref().unwrap()).unwrap();
                            assert!(
                                changed_evidence.validate_source_reply(&reply).is_err(),
                                "changed original snapshot must not match physical Bash reply"
                            );
                            assert!(
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id
                                )
                                .is_err(),
                                "changed original snapshot must not pass broker candidate assessment"
                            );
                            assert!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .is_err(),
                            "changed original snapshot must fail captured readback"
                        );
                            fs::write(&snapshot, original).unwrap();
                            assert_eq!(
                                oulipoly_kernel_broker::source_acceptance::assess_v2_candidate(
                                    &retained, &custody, &grant_id
                                )
                                .unwrap(),
                                assessed
                            );
                            assert_eq!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .unwrap(),
                            captured
                        );
                            let evidence_path = physical.join(format!("{grant_id}.evidence.json"));
                            let owned_bytes = fs::read(&evidence_path).unwrap();
                            let mut damaged = owned_bytes.clone();
                            damaged.push(b' ');
                            fs::write(&evidence_path, &damaged).unwrap();
                            assert!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .is_err(),
                            "same-inode mutation of broker evidence must fail"
                        );
                            fs::write(&evidence_path, owned_bytes).unwrap();
                            assert_eq!(
                            oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                                &retained, &custody, &grant_id
                            )
                            .unwrap(),
                            captured
                        );
                        }
                    }
                } else {
                    let grants: i64 = projected
                        .query_row(
                            "SELECT count(*) FROM broker_source_effect_grant",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(grants, 0, "recipient preview cannot reserve a source grant");
                }
                if let Some(mail) = &recipient_mail {
                    // A human CLI's arbitrary audit label and exact row
                    // selectors cannot turn the retired copy into ACK
                    // authority while the retained broker route is active.
                    for session in ["fixture-recipient", "sibling-recipient"] {
                        for _ in 0..2 {
                            let ack = Command::new(&runner)
                                .args([
                                    "mailbox",
                                    "ack",
                                    "--session-id",
                                    session,
                                    "--from-seq",
                                    &mail.seq.to_string(),
                                    "--to-seq",
                                    &mail.seq.to_string(),
                                    "--delivered-by",
                                    "forged-recipient",
                                ])
                                .env("OULIPOLY_DATA_DIR", &data)
                                .env("OULIPOLY_CONFIG_HOME", &data)
                                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                                .output()
                                .unwrap();
                            assert!(!ack.status.success());
                            assert!(
                                String::from_utf8_lossy(&ack.stderr).contains(
                                    "v30 recipient write requires broker-authenticated recipient grant; retired sidecar refused"
                                ),
                                "{}",
                                String::from_utf8_lossy(&ack.stderr)
                            );
                        }
                    }
                    let pause = Command::new(&runner)
                        .args(["mailbox", "pause", "--session-id", "fixture-recipient"])
                        .env("OULIPOLY_DATA_DIR", &data)
                        .env("OULIPOLY_CONFIG_HOME", &data)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .output()
                        .unwrap();
                    assert!(!pause.status.success());
                    assert!(
                        String::from_utf8_lossy(&pause.stderr).contains(
                            "v30 recipient write requires broker-authenticated recipient grant"
                        ),
                        "{}",
                        String::from_utf8_lossy(&pause.stderr)
                    );
                    let (session, digest, delivered): (String, String, Option<String>) = projected
                        .query_row(
                            "SELECT session_id,payload_sha256,delivered_at FROM mailbox WHERE seq=?1",
                            [mail.seq],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .unwrap();
                    assert_eq!(session, "fixture-recipient");
                    assert_eq!(digest, mail.payload_sha256.as_deref().unwrap());
                    assert!(delivered.is_none(), "selection must not autoACK");
                    let paused: i64 = projected
                        .query_row(
                            "SELECT count(*) FROM mailbox_notification_control WHERE session_id='fixture-recipient' AND paused=1",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    assert_eq!(paused, 0, "retired pause cannot change retained control");
                    let claims: i64 = projected
                        .query_row("SELECT count(*) FROM session_wake_claim", [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(claims, 0, "selection must not claim or launch");
                }
            }
            assert_eq!(
                fs::read(data.join("pid-identity.db")).unwrap(),
                b"retired copied owner"
            );
        }
        if matches!(mode.as_str(), "normal_release" | "normal_recipient") {
            // Restart the actual old sidecar/source or pending recipient
            // authority after an independent empty fresh publication.
            FreshV30Lane::initialize_at(&broker_state).unwrap();
        }
        if mode != "normal_broker_death" {
            stop(&mut broker);
        }
        let restart_log = temp.path().join("normal-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
        if mode == "normal_recipient" {
            let mail = recipient_mail.as_ref().unwrap();
            let retained = rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let (digest, delivered): (String, Option<String>) = retained
                .query_row(
                    "SELECT payload_sha256,delivered_at FROM mailbox WHERE seq=?1",
                    [mail.seq],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(digest, mail.payload_sha256.as_deref().unwrap());
            assert!(
                delivered.is_none(),
                "restart must retain pending old ACK debt"
            );
        }
        let second = Command::new(&runner)
            .arg("__age319-private-normal-v30")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .output()
            .unwrap();
        assert!(!second.status.success());
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        if matches!(
            mode.as_str(),
            "normal_release"
                | "normal_prepare_lost_reply"
                | "normal_release_lost_reply"
                | "normal_repair_lost_reply"
                | "normal_source_grant_lost_reply"
        ) {
            let retained = rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let ordinal: i64 = retained
                .query_row(
                    "SELECT authority_ordinal FROM completion_authority_continuity",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(ordinal, 1, "restart must retain exact repair cursor");
            let grant: (String, i64, i64) = retained
                .query_row(
                    "SELECT phase,revision,count(*) FROM broker_source_effect_grant",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                grant,
                ("unknown".into(), 2, 1),
                "lost custody must retain one unknown grant"
            );
        }
        if real_source {
            let retained = rusqlite::Connection::open_with_flags(
                broker_state.join("sidecar/pid-identity.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            let grant: (String, i64, i64) = retained
                .query_row(
                    "SELECT phase,revision,count(*) FROM broker_source_effect_grant",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            assert_eq!(grant, ("consumed".into(), 2, 1));
            let grant_id = fs::read_to_string(gate.join("source-grant-ready")).unwrap();
            let registration_id = pending_binding
                .as_ref()
                .unwrap()
                .registration()
                .unwrap()
                .registration_id;
            let source_phase: String = retained
                .query_row(
                    "SELECT phase FROM completion_continuation_source WHERE registration_id=?1",
                    [&registration_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                source_phase, "registered",
                "evidence debt must not release the source"
            );
            assert!(matches!(
                SourcePhysicalRegistry::open(broker_state.join("source-physical"))
                    .unwrap()
                    .observe(&grant_id)
                    .unwrap(),
                SourceObservation::Drained { .. }
            ));
            let reopened = BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap();
            let physical =
                SourcePhysicalRegistry::open(broker_state.join("source-physical")).unwrap();
            let debt = reopened
                .read_source_evidence(&physical.records()[0].grant)
                .unwrap()
                .unwrap();
            assert_eq!(
                debt.phase,
                if nonzero_source || io_failure_source {
                    "unknown"
                } else {
                    "captured"
                }
            );
            if !nonzero_source && !io_failure_source {
                assert!(
                    oulipoly_kernel_broker::source_acceptance::read_captured_v2_evidence(
                        &reopened, &physical, &grant_id
                    )
                    .is_ok()
                );
            }
        }
        stop(&mut restarted);
        unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
        return;
    }
    if mode == "held_prepared" || mode == "held_guardian_death" || release_mode {
        let generation = broker_generation.unwrap();
        // A copied v29 path is unusable before E and throughout W/R.
        fs::write(data.join("pid-identity.db"), b"retired copied owner").unwrap();
        let out = temp.path().join("held.out");
        let err = temp.path().join("held.err");
        let mut entry = Command::new(&runner)
            .arg("__age319-private-held-prepared-v30")
            .env("OULIPOLY_DATA_DIR", &data)
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .envs(
                (mode == "held_guardian_death")
                    .then_some(("AGE319_PRIVATE_EXPECT_GUARDIAN_DEATH_V1", "1")),
            )
            .envs(release_mode.then_some(("AGE319_PRIVATE_RELEASE_V30", "1")))
            .envs(
                (mode == "held_release_prepare_lost_reply")
                    .then_some(("AGE319_PRIVATE_PREPARE_LOST_REPLY_V1", "1")),
            )
            .envs(
                (mode == "held_release_driver_route" || native_mode)
                    .then_some(("AGE319_PRIVATE_DRIVER_ROUTE_V30", "1")),
            )
            .envs(native_mode.then_some(("AGE319_PRIVATE_NATIVE_LINEAGE_V30", "1")))
            .envs(
                mode.starts_with("native_receipt_")
                    .then_some(("AGE319_PRIVATE_RECEIPT_HELPER_PROBE_V1", "1")),
            )
            .envs(
                (mode == "held_release_exec_driver")
                    .then_some(("AGE319_PRIVATE_EXEC_DRIVER_ROUTE_V30", "1")),
            )
            .envs(
                (mode == "held_release_lost_reply")
                    .then_some(("AGE319_PRIVATE_RELEASE_LOST_REPLY_V1", "1")),
            )
            .env_remove("LD_LIBRARY_PATH")
            .stdin(Stdio::null())
            .stdout(Stdio::from(File::create(&out).unwrap()))
            .stderr(Stdio::from(File::create(&err).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| gate.join("prepared").exists() || entry.try_wait().unwrap().is_some());
        assert!(
            gate.join("prepared").exists(),
            "entry: {} broker: {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        let prepared: oulipoly_state::mailbox::PreparedBrokerOwner =
            serde_json::from_slice(&fs::read(gate.join("prepared")).unwrap()).unwrap();
        assert_eq!(prepared.source_generation, generation);
        assert_eq!(prepared.entry.host_pid, entry.id() as i32);
        assert_eq!(prepared.domain_id, domain);
        assert_ne!(prepared.entry.pidns_ino, prepared.joined_child.pidns_ino);
        let entry_record: serde_json::Value = serde_json::from_slice(
            &fs::read(
                broker_state
                    .join("entries")
                    .join(format!("{}.json", prepared.root_id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            entry_record["prepared_driver"]["host_pid"],
            prepared.driver.host_pid
        );
        assert_eq!(
            entry_record["prepared_driver"]["starttime_ticks"],
            prepared.driver.starttime_ticks
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        assert!(entry.try_wait().unwrap().is_none());
        let persisted = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            &broker_state,
        )
        .unwrap()
        .read_exact_prepared_owner(&generation, &prepared.root_id, &prepared.owner_generation)
        .unwrap();
        assert_eq!(persisted, prepared);
        assert!(
            BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .is_err()
        );
        let db = rusqlite::Connection::open_with_flags(
            broker_state.join("sidecar/pid-identity.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let running: i64 = db
            .query_row(
                "SELECT count(*) FROM completion_continuation_owner",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(running, 0);
        let read = protocol::StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::read_prepared_owner_at(&socket, &read).is_err()); // wrong actor
        let child_attest = protocol::StateReadSpec {
            protocol: "broker-release-attest-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            attempt_id: None,
        };
        assert!(protocol::attest_released_child_at(&socket, &child_attest).is_err());
        let forged = protocol::StateWriteSpec {
            protocol: "broker-prepared-write-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            action: protocol::StateWriteAction::Prepare {
                driver_pid: prepared.driver.host_pid,
                endpoint: prepared.endpoint.clone(),
            },
        };
        assert!(protocol::prepare_owner_at(&socket, &forged).is_err());
        let mut wrong = read;
        wrong.source_generation = uuid::Uuid::new_v4().to_string();
        assert!(protocol::read_prepared_owner_at(&socket, &wrong).is_err());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let wrong_release = protocol::StateWriteSpec {
            protocol: "broker-held-release-v30".into(),
            source_generation: generation.clone(),
            root_id: prepared.root_id.clone(),
            owner_generation: prepared.owner_generation.clone(),
            action: protocol::StateWriteAction::Release,
        };
        assert!(protocol::release_prepared_owner_at(&socket, &wrong_release).is_err());
        if mode == "held_guardian_death" {
            unsafe {
                libc::kill(prepared.guardian.host_pid, libc::SIGKILL);
            }
            fs::write(gate.join("guardian-dead"), b"yes").unwrap();
            eventually(|| {
                gate.join("death-refused").exists() || entry.try_wait().unwrap().is_some()
            });
            assert!(
                gate.join("death-refused").exists(),
                "{}",
                fs::read_to_string(&err).unwrap()
            );
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        }
        if release_mode {
            if mode == "held_release_commit_fail" {
                rusqlite::Connection::open(broker_state.join("sidecar/pid-identity.db"))
                    .unwrap()
                    .execute_batch("CREATE TRIGGER age319_release_fault BEFORE INSERT ON broker_owner_release BEGIN SELECT RAISE(ABORT, 'fixture commit fault'); END;")
                    .unwrap();
            }
            if mode == "held_release_child_predeath" {
                unsafe {
                    libc::kill(prepared.joined_child.host_pid, libc::SIGKILL);
                }
            }
            fs::write(gate.join("release"), b"yes").unwrap();
            eventually(|| gate.join("released").exists() || entry.try_wait().unwrap().is_some());
            if mode == "held_release_commit_fail"
                || mode == "held_release_child_predeath"
                || mode == "held_release_gate_fail"
            {
                assert!(!gate.join("released").exists());
                let errors = fs::read_to_string(&err).unwrap();
                if mode == "held_release_commit_fail" {
                    assert!(errors.contains("fixture commit fault"), "{errors}");
                }
                if mode == "held_release_gate_fail" {
                    assert!(
                        errors.contains("Broken pipe") || errors.contains("os error 32"),
                        "{errors}"
                    );
                }
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                assert!(
                    BrokerSidecar::open_existing(
                        &broker_state.join("sidecar/pid-identity.db"),
                        &broker_state
                    )
                    .unwrap()
                    .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
                    .is_err()
                );
                let still_zero: i64 = db
                    .query_row(
                        "SELECT count(*) FROM completion_continuation_owner",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(still_zero, 0);
                stop(&mut broker);
                unsafe {
                    libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
                }
                return;
            }
            assert!(
                gate.join("released").exists(),
                "entry: {} broker: {}",
                fs::read_to_string(&err).unwrap(),
                fs::read_to_string(&broker_log).unwrap()
            );
            let evidence: oulipoly_state::mailbox::BrokerReleaseEvidence =
                serde_json::from_slice(&fs::read(gate.join("released")).unwrap()).unwrap();
            assert_eq!(evidence.prepared, prepared);
            assert_eq!(evidence.owner.owner_generation, prepared.owner_generation);
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            assert_eq!(
                db.query_row::<i64, _, _>(
                    "SELECT count(*) FROM completion_continuation_owner",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
                1
            );
            if let Some((invocation, session, capability)) = &source_witness {
                eventually(|| {
                    gate.join("source-auth-negative").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-auth-negative").exists(),
                    "source child: {} broker: {}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                let journal_path = broker_state.join("source-decisions/decisions.db");
                assert!(
                    !journal_path.exists(),
                    "authentication failure created a decision journal"
                );
                fs::write(gate.join("source-auth-checked"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-verification-ready").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-verification-ready").exists(),
                    "source decision issuance: {} broker: {}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                let original_state_meta = fs::metadata(data.join("state.db")).unwrap();
                let issued: protocol::ExactSourceDecisionReadback = serde_json::from_slice(
                    &fs::read(gate.join("source-verification-ready")).unwrap(),
                )
                .unwrap();
                assert_eq!(issued.original_state_device, original_state_meta.dev());
                assert_eq!(issued.original_state_inode, original_state_meta.ino());
                let writer = rusqlite::Connection::open(data.join("state.db")).unwrap();
                writer
                    .execute_batch("PRAGMA locking_mode=EXCLUSIVE; BEGIN EXCLUSIVE")
                    .unwrap();
                fs::write(gate.join("source-state-writer-held"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-state-writer-verified").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-state-writer-verified").exists(),
                    "verification waited on State writer: {} broker: {}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                assert_eq!(
                    issued.original_state_inode,
                    fs::metadata(data.join("state.db")).unwrap().ino(),
                    "read-only verification changed the original State identity under BEGIN EXCLUSIVE"
                );
                writer
                    .execute_batch("ROLLBACK; PRAGMA locking_mode=NORMAL")
                    .unwrap();
                drop(writer);
                fs::write(gate.join("source-state-writer-released"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-witness-positive").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-witness-positive").exists(),
                    "source child: {} broker: {}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                let marker: serde_json::Value = serde_json::from_slice(
                    &fs::read(gate.join("source-witness-positive")).unwrap(),
                )
                .unwrap();
                assert_eq!(marker["root_id"], prepared.root_id);
                assert_eq!(marker["source_generation"], generation);
                assert_eq!(marker["owner_generation"], prepared.owner_generation);
                assert_eq!(marker["invocation_uuid"], *invocation);
                assert_eq!(marker["session_id"], *session);
                assert_eq!(marker["decision"], serde_json::to_value(&issued).unwrap());
                let read_decisions = || {
                    rusqlite::Connection::open_with_flags(
                        &journal_path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap()
                };
                let journal = read_decisions();
                let count: i64 = journal
                    .query_row("SELECT count(*) FROM decisions", [], |row| row.get(0))
                    .unwrap();
                assert_eq!(count, 1, "lost reply or retry created another decision");
                let (request_id, decision_id, claims_bytes): (String, String, Vec<u8>) = journal
                    .query_row(
                        "SELECT request_id,decision_id,claims FROM decisions",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .unwrap();
                assert_eq!(request_id, marker["request_id"]);
                assert_eq!(decision_id, marker["decision"]["decision_id"]);
                let claims: serde_json::Value = serde_json::from_slice(&claims_bytes).unwrap();
                assert_eq!(claims["root_id"], prepared.root_id);
                assert_eq!(claims["root_init"]["host_pid"], prepared.root_init.host_pid);
                assert_eq!(claims["owner_generation"], prepared.owner_generation);
                assert_eq!(claims["owner_invocation_uuid"], *invocation);
                assert_eq!(claims["owner_session_id"], *session);
                assert_eq!(claims["original_state_device"], original_state_meta.dev());
                assert_eq!(claims["original_state_inode"], original_state_meta.ino());
                drop(journal);
                stop(&mut broker);
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-restart.log"),
                );
                fs::write(gate.join("source-broker-restarted"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-restart-readback").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-restart-readback").exists(),
                    "source restart: {} broker: {}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(temp.path().join("source-broker-restart.log")).unwrap()
                );
                assert_eq!(
                    read_decisions()
                        .query_row::<i64, _, _>("SELECT count(*) FROM decisions", [], |row| row
                            .get(0))
                        .unwrap(),
                    1
                );
                let mut stale: serde_json::Value = serde_json::from_slice(&claims_bytes).unwrap();
                stale["original_state_inode"] = (original_state_meta.ino() + 1).into();
                stop(&mut broker);
                replace_source_decision_claims(&journal_path, &serde_json::to_vec(&stale).unwrap());
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-stale.log"),
                );
                fs::write(gate.join("source-journal-stale"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-journal-stale-refused").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-journal-stale-refused").exists(),
                    "stale journal row: {}",
                    fs::read_to_string(&err).unwrap()
                );
                stop(&mut broker);
                replace_source_decision_claims(&journal_path, b"{");
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-malformed.log"),
                );
                fs::write(gate.join("source-journal-malformed"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-journal-malformed-refused").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-journal-malformed-refused").exists(),
                    "malformed journal row: {}",
                    fs::read_to_string(&err).unwrap()
                );
                stop(&mut broker);
                replace_source_decision_claims(&journal_path, &claims_bytes);
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-restored.log"),
                );
                fs::write(gate.join("source-journal-restored"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-journal-restored-readback").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-journal-restored-readback").exists(),
                    "restored journal row: {}",
                    fs::read_to_string(&err).unwrap()
                );
                let registration_path =
                    std::path::PathBuf::from(marker["registration_path"].as_str().unwrap());
                let registration = fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&registration_path)
                    .unwrap();
                assert!(registration.metadata().unwrap().is_file());
                assert_eq!(registration.metadata().unwrap().nlink(), 1);
                let bytes = fs::read(&registration_path).unwrap();
                assert_eq!(
                    bytes.len() as u64,
                    marker["registration_len"].as_u64().unwrap()
                );
                assert_eq!(
                    format!("{:x}", Sha256::digest(&bytes)),
                    marker["registration_sha256"]
                );
                let source: oulipoly_state::completion_continuation::SourceRegistration =
                    serde_json::from_slice(&bytes).unwrap();
                assert_eq!(source.owner_invocation_uuid, *invocation);
                assert_eq!(source.owner_session_id, *session);
                let mut outsider_socket = UnixStream::connect(&evidence.owner.endpoint).unwrap();
                outsider_socket.write_all(b"hello\n").unwrap();
                let mut hello = Vec::new();
                outsider_socket.read_to_end(&mut hello).unwrap();
                let outsider_owner: oulipoly_state::mailbox::CompletionDomainOwner =
                    serde_json::from_slice(&hello).unwrap();
                assert_eq!(outsider_owner, evidence.owner);
                let stamp = |stamp: &oulipoly_state::mailbox::PreparedProcessStamp| {
                    protocol::ProcessWitness {
                        host_pid: stamp.host_pid,
                        boot_id: stamp.boot_id.clone(),
                        starttime_ticks: stamp.starttime_ticks,
                    }
                };
                let outsider = protocol::PrivateSourceWitnessProbe {
                    owner: protocol::OwnerWitness {
                        root_id: prepared.root_id.clone(),
                        domain_id: prepared.domain_id.clone(),
                        supervisor_id: prepared.supervisor_authority_id.clone(),
                        guardian: stamp(&prepared.guardian),
                        driver: stamp(&prepared.driver),
                        owner_generation: Some(prepared.owner_generation.clone()),
                        work_id: None,
                        owner_session_id: Some(session.clone()),
                        owner_invocation_uuid: Some(invocation.clone()),
                        registration_authority_sha256: Some(format!(
                            "{:x}",
                            Sha256::digest(capability.process_environment_value().as_bytes())
                        )),
                    },
                    source_generation: generation.clone(),
                    owner_generation: prepared.owner_generation.clone(),
                    registration_path: registration_path.clone(),
                    registration_len: bytes.len() as u64,
                    registration_sha256: format!("{:x}", Sha256::digest(&bytes)),
                    owner_session_id: session.clone(),
                    owner_invocation_uuid: invocation.clone(),
                    capability: capability.process_environment_value().into(),
                    accepted_intent_path: None,
                };
                assert!(
                    protocol::private_source_witness_probe_at(
                        &socket,
                        &outsider,
                        outsider_socket.as_raw_fd(),
                        registration.as_raw_fd(),
                    )
                    .is_err_and(|error| error
                        .to_string()
                        .contains("owner witness is outside consumed work"))
                );
                let original_state = data.join("state.db");
                let displaced_state = data.join("state-original.db");
                let inspect = rusqlite::Connection::open_with_flags(
                    &original_state,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let original_count: i64 = inspect.query_row(
                    "SELECT count(*) FROM invocations WHERE invocation_uuid=?1 AND provider_session_id=?2 AND completion_registration_capability_digest IS NOT NULL",
                    rusqlite::params![invocation, session], |row| row.get(0)).unwrap();
                assert_eq!(original_count, 1);
                let effects: i64 = inspect
                    .query_row(
                        "SELECT count(*) FROM invocation_completion_obligations",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(effects, 0);
                let sidecar_effects: i64 = db
                    .query_row(
                        "SELECT count(*) FROM broker_source_effect_grant",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(sidecar_effects, 0);
                let retained = BrokerSidecar::open_existing(
                    &broker_state.join("sidecar/pid-identity.db"),
                    &broker_state,
                )
                .unwrap()
                .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
                .unwrap();
                assert_eq!(retained, evidence);
                let decisions: i64 = db.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE '%source_decision%'",
                    [], |row| row.get(0)).unwrap();
                assert_eq!(decisions, 0);
                drop(inspect);
                fs::rename(&original_state, &displaced_state).unwrap();
                fs::copy(&displaced_state, &original_state).unwrap();
                assert_ne!(
                    fs::metadata(&original_state).unwrap().ino(),
                    fs::metadata(&displaced_state).unwrap().ino()
                );
                fs::write(gate.join("source-state-replaced"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-witness-negatives-done").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-witness-negatives-done").exists(),
                    "source child: {} broker: {}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                assert_eq!(
                    db.query_row::<i64, _, _>(
                        "SELECT count(*) FROM broker_source_effect_grant",
                        [],
                        |row| row.get(0)
                    )
                    .unwrap(),
                    0
                );
                fs::remove_file(&original_state).unwrap();
                fs::rename(&displaced_state, &original_state).unwrap();
                let state_after = rusqlite::Connection::open_with_flags(
                    &original_state,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                assert_eq!(
                    state_after
                        .query_row::<i64, _, _>(
                            "SELECT count(*) FROM invocation_completion_obligations",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap(),
                    0
                );
                assert_eq!(db.query_row::<i64, _, _>(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE '%source_decision%'",
                    [], |row| row.get(0),
                ).unwrap(), 0);
                drop(state_after);
                assert_eq!(
                    BrokerSidecar::open_existing(
                        &broker_state.join("sidecar/pid-identity.db"),
                        &broker_state,
                    )
                    .unwrap()
                    .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation,)
                    .unwrap(),
                    evidence,
                );
                stop(&mut broker);
                fs::write(gate.join("source-clock-offset"), b"240").unwrap();
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-expired-first.log"),
                );
                fs::write(gate.join("source-expired-before-first"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-expired-first-refused").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-expired-first-refused").exists(),
                    "expired first consumption: {}",
                    fs::read_to_string(&err).unwrap()
                );
                stop(&mut broker);
                fs::remove_file(gate.join("source-clock-offset")).unwrap();
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-unexpired-first.log"),
                );
                fs::write(gate.join("source-state-restored"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-admission-first").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-admission-first").exists(),
                    "first State admission: {}",
                    fs::read_to_string(&err).unwrap()
                );
                stop(&mut broker);
                fs::write(gate.join("source-clock-offset"), b"240").unwrap();
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-expired-retry.log"),
                );
                fs::write(gate.join("source-expired-after-commit"), b"yes").unwrap();
                eventually(|| {
                    gate.join("source-state-admission-done").exists()
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("source-state-admission-done").exists(),
                    "State decision admission: entry={} broker={}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                let admitted = rusqlite::Connection::open_with_flags(
                    &original_state,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                for table in [
                    "invocation_completion_obligations",
                    "invocation_completion_continuity",
                    "invocation_completion_exact_source_decisions",
                ] {
                    let count: i64 = admitted
                        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                            row.get(0)
                        })
                        .unwrap();
                    assert_eq!(count, 1, "{table} first insert/retry");
                }
                let attributed: (String, String, String, String, String) = admitted
                    .query_row(
                        "SELECT request_id,decision_id,root_id,source_generation,owner_generation
                     FROM invocation_completion_exact_source_decisions",
                        [],
                        |row| {
                            Ok((
                                row.get(0)?,
                                row.get(1)?,
                                row.get(2)?,
                                row.get(3)?,
                                row.get(4)?,
                            ))
                        },
                    )
                    .unwrap();
                assert_eq!(
                    attributed,
                    (
                        request_id.clone(),
                        decision_id.clone(),
                        prepared.root_id.clone(),
                        generation.clone(),
                        prepared.owner_generation.clone(),
                    )
                );
                assert_eq!(
                    db.query_row::<i64, _, _>(
                        "SELECT count(*) FROM broker_source_effect_grant",
                        [],
                        |row| row.get(0)
                    )
                    .unwrap(),
                    0
                );
                let unavailable = BrokerSidecar::open_existing(
                    &broker_state.join("sidecar/pid-identity.db"),
                    &broker_state,
                )
                .unwrap()
                .read_bounded_source_selection(
                    &generation,
                    &prepared.root_id,
                    &evidence.owner,
                );
                assert!(
                    unavailable.is_err_and(|error| error.contains("complete State projection")),
                    "new decision source became selectable"
                );
                // Crash after the State writer committed, before any retained
                // sidecar source/receipt was written. The restarted broker must
                // recover solely from the immutable State suffix.
                stop(&mut broker);
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-before-projection.log"),
                );
                let retained_path = broker_state.join("sidecar/pid-identity.db");
                let mut retained =
                    BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                assert!(
                    retained
                        .reserve_source_effect_grant(
                            &generation,
                            &prepared.root_id,
                            &evidence.owner,
                        )
                        .is_err(),
                    "W reserved before exact sidecar projection"
                );
                let projected = retained
                    .repair_bounded_suffix(&generation, &prepared.root_id, &evidence.owner, 0)
                    .unwrap();
                assert_eq!(projected.authority_ordinal, 1);
                drop(retained);
                stop(&mut broker);
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-after-projection.log"),
                );
                let mut retained =
                    BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                let replay = retained
                    .repair_bounded_suffix(&generation, &prepared.root_id, &evidence.owner, 0)
                    .unwrap();
                assert_eq!(replay, projected, "lost reply replay changed projection");
                drop(retained);
                let mut retained =
                    BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                let selected = retained
                    .read_bounded_source_selection(&generation, &prepared.root_id, &evidence.owner)
                    .unwrap();
                assert_eq!(
                    selected.candidate.as_ref().unwrap().registration_id,
                    source.registration_id
                );
                assert!(
                    retained
                        .read_bounded_source_selection(
                            &uuid::Uuid::new_v4().to_string(),
                            &prepared.root_id,
                            &evidence.owner,
                        )
                        .is_err(),
                    "wrong source generation selected"
                );
                assert!(
                    retained
                        .read_bounded_source_selection(
                            &generation,
                            &uuid::Uuid::new_v4().to_string(),
                            &evidence.owner,
                        )
                        .is_err(),
                    "wrong root selected"
                );
                let mut wrong_owner = evidence.owner.clone();
                wrong_owner.owner_generation = uuid::Uuid::new_v4().to_string();
                assert!(retained.read_bounded_source_selection(
                    &generation, &prepared.root_id, &wrong_owner,
                ).is_err(), "wrong owner generation selected");
                let reserved = retained
                    .reserve_source_effect_grant(&generation, &prepared.root_id, &evidence.owner)
                    .unwrap();
                assert_eq!(reserved.phase, "reserved");
                let before_mismatch: i64 = db
                    .query_row(
                        "SELECT count(*) FROM broker_source_effect_grant",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(before_mismatch, 1);
                let tamper = rusqlite::Connection::open(&retained_path).unwrap();
                let immutable_trigger: String = tamper.query_row(
                    "SELECT sql FROM sqlite_master WHERE name='broker_exact_source_projection_immutable'",
                    [], |row| row.get(0),
                ).unwrap();
                tamper
                    .execute_batch("DROP TRIGGER broker_exact_source_projection_immutable;")
                    .unwrap();
                for (column, wrong) in [
                    ("root_id", "wrong-root"),
                    ("source_generation", "wrong-source"),
                    ("owner_generation", "wrong-owner"),
                    ("decision_id", "wrong-decision"),
                    ("issuer_stamp_json", "wrong-issuer"),
                    ("registration_sha256", "wrong-registration-digest"),
                ] {
                    let original: String = tamper
                        .query_row(
                            &format!("SELECT {column} FROM broker_exact_source_projection"),
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    tamper
                        .execute(
                            &format!("UPDATE broker_exact_source_projection SET {column}=?1"),
                            [wrong],
                        )
                        .unwrap();
                    assert!(
                        retained
                            .read_source_effect_grant(
                                &generation,
                                &prepared.root_id,
                                &evidence.owner,
                            )
                            .is_err(),
                        "{column} mismatch still returned W"
                    );
                    tamper
                        .execute(
                            &format!("UPDATE broker_exact_source_projection SET {column}=?1"),
                            [&original],
                        )
                        .unwrap();
                }
                for column in [
                    "registration_bytes",
                    "binding_bytes",
                    "broker_readback_json",
                ] {
                    let original: Vec<u8> = tamper
                        .query_row(
                            &format!("SELECT {column} FROM broker_exact_source_projection"),
                            [],
                            |row| row.get(0),
                        )
                        .unwrap();
                    tamper
                        .execute(
                            &format!("UPDATE broker_exact_source_projection SET {column}=?1"),
                            [b"changed bytes".as_slice()],
                        )
                        .unwrap();
                    assert!(
                        retained
                            .read_source_effect_grant(
                                &generation,
                                &prepared.root_id,
                                &evidence.owner,
                            )
                            .is_err(),
                        "{column} mismatch still returned W"
                    );
                    tamper
                        .execute(
                            &format!("UPDATE broker_exact_source_projection SET {column}=?1"),
                            [&original],
                        )
                        .unwrap();
                }
                tamper.execute_batch(&immutable_trigger).unwrap();
                drop(tamper);
                assert!(
                    retained
                        .read_source_effect_grant(&generation, &prepared.root_id, &evidence.owner,)
                        .unwrap()
                        .is_some(),
                    "restored exact receipt lost W"
                );
                drop(retained);
                fs::write(gate.join("source-projection-done"), b"yes").unwrap();
                let immutable = rusqlite::Connection::open(&original_state).unwrap();
                assert!(immutable.execute(
                    "UPDATE invocation_completion_exact_source_decisions SET root_id='changed'",
                    [],
                ).is_err_and(|error| error.to_string().contains("attribution immutable")));
                assert!(
                    immutable
                        .execute(
                            "DELETE FROM invocation_completion_exact_source_decisions",
                            [],
                        )
                        .is_err_and(|error| error.to_string().contains("attribution immutable"))
                );
                assert_eq!(
                    fs::read(data.join("pid-identity.db")).unwrap(),
                    b"retired copied owner"
                );
                // Simulate an externally corrupted State attribution while the
                // sidecar receipt and source remain. The selector must reject
                // this sidecar-only source even though its continuity matches.
                let decision_trigger: String = immutable.query_row(
                    "SELECT sql FROM sqlite_master WHERE name='invocation_completion_exact_source_decisions_no_delete'",
                    [], |row| row.get(0),
                ).unwrap();
                immutable
                    .execute_batch(
                        "DROP TRIGGER invocation_completion_exact_source_decisions_no_delete;",
                    )
                    .unwrap();
                immutable
                    .execute(
                        "DELETE FROM invocation_completion_exact_source_decisions",
                        [],
                    )
                    .unwrap();
                immutable.execute_batch(&decision_trigger).unwrap();
                drop(immutable);
                let retained = BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                assert!(
                    retained
                        .read_source_effect_grant(&generation, &prepared.root_id, &evidence.owner,)
                        .is_err(),
                    "orphan sidecar receipt still returned W"
                );
                drop(retained);
                let orphan_state = rusqlite::Connection::open(&original_state).unwrap();
                orphan_state
                    .execute_batch("PRAGMA foreign_keys=OFF;")
                    .unwrap();
                let obligation_trigger: String = orphan_state.query_row(
                    "SELECT sql FROM sqlite_master WHERE name='trg_invocation_completion_obligations_append_only_delete'",
                    [], |row| row.get(0),
                ).unwrap();
                orphan_state
                    .execute_batch(
                        "DROP TRIGGER trg_invocation_completion_obligations_append_only_delete;",
                    )
                    .unwrap();
                orphan_state
                    .execute("DELETE FROM invocation_completion_obligations", [])
                    .unwrap();
                orphan_state.execute_batch(&obligation_trigger).unwrap();
                drop(orphan_state);
                let retained = BrokerSidecar::open_existing(&retained_path, &broker_state).unwrap();
                assert!(
                    retained
                        .read_source_effect_grant(&generation, &prepared.root_id, &evidence.owner,)
                        .is_err(),
                    "sidecar-only source still returned W"
                );
                stop(&mut broker);
                fs::remove_file(gate.join("source-clock-offset")).unwrap();
                broker = restart_source_broker(
                    &socket,
                    &broker_state,
                    &runner,
                    &gate,
                    &temp.path().join("source-broker-after-retry.log"),
                );
            }
            eventually(|| {
                gate.join("child-attested").exists() || entry.try_wait().unwrap().is_some()
            });
            assert!(
                gate.join("child-attested").exists(),
                "child: {} broker: {} source-stage: {:?} guardian-stage: {:?} broker-stage: {:?}",
                fs::read_to_string(&err).unwrap(),
                fs::read_to_string(&broker_log).unwrap(),
                fs::read_to_string(gate.join("source-stage")),
                fs::read_to_string(gate.join("source-guardian-stage")),
                fs::read_to_string(gate.join("source-broker-stage")),
            );
            assert_eq!(
                fs::read_to_string(gate.join("child-attested")).unwrap(),
                evidence.release_id
            );
            assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            let durable = BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .unwrap();
            assert_eq!(durable, evidence);
            if native_mode {
                eventually(|| {
                    gate.join("native-prepared").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("native-prepared").exists(),
                    "native N: entry={} broker={}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                let grant = fs::read_to_string(gate.join("native-prepared")).unwrap();
                let native_db = broker_state.join("sidecar/pid-identity.db");
                let read_native = || {
                    rusqlite::Connection::open_with_flags(
                        &native_db,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .unwrap()
                };
                let state = read_native();
                let (attempt, phase, protocol, bound): (String, String, String, String) = state
                    .query_row(
                        "SELECT a.attempt_id,a.phase,b.protocol,b.grant_id FROM completion_continuation_attempt a JOIN completion_native_grant_binding b ON b.attempt_id=a.attempt_id WHERE a.session_id='native-session'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .unwrap();
                assert_eq!(phase, "accepted");
                // The State binding schema uses its original v1 field; the
                // broker's exact v5 grant and owner provenance select v30.
                assert_eq!(protocol, "native-continuation-v1");
                assert_eq!(bound, grant);
                let sibling_grant = fs::read_to_string(gate.join("native-sibling-grant")).unwrap();
                assert_ne!(sibling_grant, grant);
                assert_eq!(
                    state
                        .query_row::<i64, _, _>(
                            "SELECT count(*) FROM completion_native_grant_binding",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap(),
                    2
                );
                assert_eq!(
                    state
                        .query_row::<i64, _, _>(
                            "SELECT count(*) FROM completion_native_worker_attach",
                            [],
                            |row| row.get(0)
                        )
                        .unwrap(),
                    0
                );
                assert!(!gate.join("native-effect").exists(), "effect before K");
                fs::write(gate.join("native-k"), b"yes").unwrap();
                eventually(|| {
                    gate.join("native-attached").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("native-attached").exists(),
                    "native attach: entry={} broker={}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap()
                );
                assert_eq!(
                    fs::read_to_string(gate.join("native-attached")).unwrap(),
                    grant
                );
                assert!(
                    !gate.join("native-effect").exists(),
                    "effect before State-attached gate release"
                );
                let state = read_native();
                let pid1_json: String = state.query_row(
                    "SELECT pid1_identity FROM completion_native_worker_attach WHERE attempt_id=?1 AND grant_id=?2",
                    [&attempt, &grant],
                    |row| row.get(0),
                ).unwrap();
                let pid1: serde_json::Value = serde_json::from_str(&pid1_json).unwrap();
                let pid1 = pid1["pid"].as_i64().unwrap() as i32;
                assert!(pid1 > 0);
                if matches!(
                    mode.as_str(),
                    "native_pid1_loss" | "native_receipt_pid1_loss"
                ) {
                    assert_eq!(unsafe { libc::kill(pid1, libc::SIGKILL) }, 0);
                    fs::write(gate.join("native-pid1-loss"), b"yes").unwrap();
                }
                fs::write(gate.join("native-release"), b"yes").unwrap();
                eventually(|| {
                    gate.join("native-t-sent").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(gate.join("native-t-sent").exists());
                let mut provider_descendant = None;
                if native_live {
                    // The provider creates the marker before writing its
                    // physical-custody line; observe content, not just the
                    // directory entry.
                    eventually(|| {
                        fs::metadata(gate.join("native-effect"))
                            .is_ok_and(|metadata| metadata.len() > 0)
                    });
                    if mode.starts_with("native_receipt_") {
                        assert_eq!(
                            fs::read(gate.join("native-effect").with_extension("helper-entry"))
                                .unwrap(),
                            b"receipt-helper nnp=0 seccomp=0"
                        );
                    }
                    let provider = fs::read_to_string(gate.join("native-effect")).unwrap();
                    assert!(
                        provider.starts_with(
                            "nnp=0 seccomp=0 setsid=ok unshare=ok adopted=1 host_pid="
                        ),
                        "{provider}"
                    );
                    let pid: i32 = provider.trim().rsplit_once('=').unwrap().1.parse().unwrap();
                    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
                    assert!(
                        fd >= 0,
                        "provider descendant disappeared before cancellation"
                    );
                    provider_descendant = Some(unsafe { File::from_raw_fd(fd) });
                    assert_eq!(
                        fs::read_to_string(gate.join("native-effect"))
                            .unwrap()
                            .lines()
                            .count(),
                        1
                    );
                } else {
                    assert!(!gate.join("native-effect").exists());
                }
                // A completed t handler is required before killing its socket;
                // the lost reply has no authority to trigger a second K.
                eventually(|| protocol::request_at(&socket, Operation::Classify).is_ok());
                if mode == "native_cancel" {
                    // The same old broker must recover pending native/State Q
                    // and physical debt with both published fresh storage and
                    // an abandoned initializer stage in its root directory.
                    FreshV30Lane::initialize_at(&broker_state).unwrap();
                    let stage =
                        broker_state.join(format!(".v30-fresh-{}", uuid::Uuid::new_v4().simple()));
                    fs::create_dir(&stage).unwrap();
                    fs::set_permissions(&stage, fs::Permissions::from_mode(0o700)).unwrap();
                }
                stop(&mut broker);
                assert!(
                    !socket.exists() || protocol::request_at(&socket, Operation::Classify).is_err()
                );
                let restart_log = temp.path().join("native-restart.log");
                let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                    .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                    .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                    .spawn()
                    .unwrap();
                eventually(|| {
                    protocol::request_at(&socket, Operation::Classify).is_ok()
                        || restarted.try_wait().unwrap().is_some()
                });
                assert!(
                    restarted.try_wait().unwrap().is_none(),
                    "restart: {}",
                    fs::read_to_string(&restart_log).unwrap()
                );
                fs::write(gate.join("native-restarted"), b"yes").unwrap();
                if native_live {
                    eventually(|| {
                        gate.join("native-q-pending").exists()
                            || entry.try_wait().unwrap().is_some()
                    });
                    assert!(
                        gate.join("native-q-pending").exists(),
                        "{}",
                        fs::read_to_string(&err).unwrap()
                    );
                    std::thread::sleep(Duration::from_secs(6));
                    assert_eq!(
                        fs::read_to_string(gate.join("native-effect"))
                            .unwrap()
                            .lines()
                            .count(),
                        1
                    );
                    assert_eq!(
                        state
                            .query_row::<i64, _, _>(
                                "SELECT count(*) FROM completion_native_kernel_q",
                                [],
                                |row| row.get(0)
                            )
                            .unwrap(),
                        0
                    );
                    if matches!(mode.as_str(), "native_drain" | "native_receipt_drain") {
                        fs::write(gate.join("native-drain"), b"yes").unwrap();
                        fs::write(gate.join("native-effect.release"), b"yes").unwrap();
                    }
                    fs::write(gate.join("native-cancel"), b"yes").unwrap();
                }
                eventually(|| {
                    gate.join("native-q-readback").exists() || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    gate.join("native-q-readback").exists(),
                    "q: entry={} broker={} restart={}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(&broker_log).unwrap(),
                    fs::read_to_string(&restart_log).unwrap()
                );
                let q = fs::read_to_string(gate.join("native-q-readback")).unwrap();
                let state = read_native();
                let settled: i64 = state.query_row("SELECT count(*) FROM completion_native_kernel_q WHERE attempt_id=?1 AND grant_id=?2", [&attempt, &grant], |row| row.get(0)).unwrap();
                let incarnation: String = state.query_row(
                    "SELECT work_incarnation_id FROM completion_native_worker_attach WHERE attempt_id=?1 AND grant_id=?2",
                    [&attempt, &grant],
                    |row| row.get(0),
                ).unwrap();
                let spent: serde_json::Value = serde_json::from_slice(
                    &fs::read(broker_state.join("grants").join(format!("{grant}.json"))).unwrap(),
                )
                .unwrap();
                assert_eq!(spent["state"], "consumed");
                let sibling: serde_json::Value = serde_json::from_slice(
                    &fs::read(
                        broker_state
                            .join("grants")
                            .join(format!("{sibling_grant}.json")),
                    )
                    .unwrap(),
                )
                .unwrap();
                assert_eq!(sibling["state"], "prepared");
                assert!(!gate.join("native-sibling-effect").exists());
                assert_eq!(fs::read_dir(broker_state.join("works")).unwrap().count(), 1);
                if native_live {
                    assert!(q.starts_with("native-q-settled "), "{q}");
                    let mut exit = libc::pollfd {
                        fd: provider_descendant.as_ref().unwrap().as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    assert_eq!(unsafe { libc::poll(&mut exit, 1, 0) }, 1);
                    assert_ne!(exit.revents & libc::POLLIN, 0, "Q preceded descendant exit");
                    assert_eq!(settled, 1);
                    assert_eq!(
                        fs::read_to_string(gate.join("native-effect"))
                            .unwrap()
                            .lines()
                            .count(),
                        1
                    );
                    let terminal = fs::read(
                        broker_state
                            .join("terminals")
                            .join(format!("{incarnation}.native.json")),
                    )
                    .unwrap();
                    let physical: serde_json::Value = serde_json::from_slice(&terminal).unwrap();
                    assert_eq!(physical["physical_tree_drained"], true);
                    assert_eq!(
                        physical["cancellation_observed"],
                        matches!(mode.as_str(), "native_cancel" | "native_receipt_cancel")
                    );
                    if matches!(mode.as_str(), "native_drain" | "native_receipt_drain") {
                        assert_eq!(physical["worker_wait_status"], 0);
                    }
                    assert!(
                        physical["output"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|item| item["name"] == "launcher.stdout")
                    );
                    let digest: String = state.query_row("SELECT terminal_receipt_sha256 FROM completion_native_kernel_q WHERE attempt_id=?1", [&attempt], |row| row.get(0)).unwrap();
                    assert_eq!(digest, format!("{:x}", Sha256::digest(&terminal)));
                    let wait: serde_json::Value = serde_json::from_slice(
                        &fs::read(
                            broker_state
                                .join("terminals")
                                .join(format!("{grant}.native-pid1-wait.json")),
                        )
                        .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(wait["reaped"], true);
                } else {
                    assert!(
                        q.starts_with("native-unknown terminal-absent-after-PID1-loss "),
                        "{q}"
                    );
                    assert_eq!(settled, 0);
                    assert!(
                        !broker_state
                            .join("terminals")
                            .join(format!("{incarnation}.native.json"))
                            .exists()
                    );
                }
                let (phase, integrated): (String, i64) = state.query_row("SELECT phase,integrated FROM completion_continuation_attempt WHERE attempt_id=?1", [&attempt], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
                assert_eq!(
                    (phase.as_str(), integrated),
                    ("accepted", 0),
                    "Q must not stand in for Runner result"
                );
                assert_eq!(state.query_row::<i64, _, _>("SELECT count(*) FROM session_wake_claim WHERE session_id='native-session' AND claim_token='native-claim'", [], |row| row.get(0)).unwrap(), 1);
                assert_eq!(
                    fs::read(data.join("pid-identity.db")).unwrap(),
                    b"retired copied owner"
                );
                stop(&mut restarted);
                fs::write(gate.join("finish"), b"done").unwrap();
                eventually(|| entry.try_wait().unwrap().is_some());
                let _ = entry.wait();
                unsafe { libc::kill(prepared.root_init.host_pid, libc::SIGKILL) };
                return;
            }
            if mode == "held_release_driver_route" {
                eventually(|| {
                    gate.join("driver-routed").exists() || entry.try_wait().unwrap().is_some()
                });
                let attempt_id =
                    fs::read_to_string(gate.join("driver-routed")).unwrap_or_else(|_| {
                        panic!(
                            "driver route: {} broker: {}",
                            fs::read_to_string(&err).unwrap(),
                            fs::read_to_string(&broker_log).unwrap()
                        )
                    });
                let sidecar = rusqlite::Connection::open_with_flags(
                    broker_state.join("sidecar/pid-identity.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .unwrap();
                let (count, phase, revision): (i64, String, i64) = sidecar.query_row(
                    "SELECT (SELECT count(*) FROM completion_continuation_attempt),phase,revision FROM completion_continuation_attempt WHERE attempt_id=?1",
                    [&attempt_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                ).unwrap();
                assert_eq!((count, phase.as_str(), revision), (1, "accepted", 2));
                let read = protocol::StateReadSpec {
                    protocol: "broker-state-read-v1".into(),
                    source_generation: generation.clone(),
                    root_id: prepared.root_id.clone(),
                    owner_generation: prepared.owner_generation.clone(),
                    attempt_id: Some(attempt_id),
                };
                assert!(protocol::read_state_at(&socket, &read).is_err()); // entry is not guardian/driver
            }
            if mode == "held_release_exec_driver" {
                eventually(|| {
                    fs::read_to_string(&err)
                        .unwrap_or_default()
                        .contains("v30 driver bounded State repair and wake route is not available")
                        || entry.try_wait().unwrap().is_some()
                });
                assert!(
                    fs::read_to_string(&err).unwrap().contains(
                        "v30 driver bounded State repair and wake route is not available"
                    ),
                    "execed driver did not reach broker running-owner readback: {}",
                    fs::read_to_string(&err).unwrap()
                );
                assert_eq!(
                    fs::read(data.join("pid-identity.db")).unwrap(),
                    b"retired copied owner"
                );
                assert_eq!(
                    db.query_row::<i64, _, _>(
                        "SELECT count(*) FROM completion_continuation_attempt",
                        [],
                        |row| row.get(0)
                    )
                    .unwrap(),
                    0
                );
            }
            if mode == "held_release_guardian_death" {
                unsafe {
                    libc::kill(prepared.guardian.host_pid, libc::SIGKILL);
                }
            }
            if mode == "held_release_child_death" {
                unsafe {
                    libc::kill(prepared.joined_child.host_pid, libc::SIGKILL);
                }
            }
            if mode == "held_release_broker_death" {
                stop(&mut broker);
                let premature_restart_log = temp.path().join("released-child-restart.log");
                let mut premature_restart =
                    Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                        .stderr(Stdio::from(File::create(&premature_restart_log).unwrap()))
                        .spawn()
                        .unwrap();
                eventually(|| {
                    protocol::request_at(&socket, Operation::Classify).is_ok()
                        || premature_restart.try_wait().unwrap().is_some()
                });
                assert!(
                    premature_restart.try_wait().unwrap().is_none(),
                    "{}",
                    fs::read_to_string(&premature_restart_log).unwrap()
                );
                fs::write(gate.join("child-effect"), b"yes").unwrap();
                std::thread::sleep(Duration::from_millis(200));
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
                stop(&mut premature_restart);
            }
            if mode != "held_release_broker_death" && source_witness.is_none() {
                fs::write(gate.join("child-effect"), b"yes").unwrap();
            }
            if mode == "held_release"
                || mode == "held_release_lost_reply"
                || mode == "held_release_prepare_lost_reply"
                || mode == "held_release_driver_route"
            {
                eventually(|| {
                    fs::metadata(&out).unwrap().len() > 0 || entry.try_wait().unwrap().is_some()
                });
                assert_eq!(
                    fs::read_to_string(&out).unwrap(),
                    format!("OULIPOLY_KERNEL_V30_CHILD_EFFECT={}\n", evidence.release_id),
                    "child: {} broker: {} source-stage: {:?}",
                    fs::read_to_string(&err).unwrap(),
                    fs::read_to_string(temp.path().join("source-broker-restart.log"))
                        .unwrap_or_default(),
                    fs::read_to_string(gate.join("source-stage")),
                );
            } else {
                std::thread::sleep(Duration::from_millis(200));
                assert_eq!(fs::metadata(&out).unwrap().len(), 0);
            }
            if mode != "held_release_broker_death" {
                stop(&mut broker);
            }
            fs::write(gate.join("finish"), b"done").unwrap();
            eventually(|| entry.try_wait().unwrap().is_some());
            let _ = entry.wait();
            let restart_log = temp.path().join("released-restart.log");
            let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
                .stderr(Stdio::from(File::create(&restart_log).unwrap()))
                .spawn()
                .unwrap();
            eventually(|| {
                protocol::request_at(&socket, Operation::Classify).is_ok()
                    || restarted.try_wait().unwrap().is_some()
            });
            assert!(restarted.try_wait().unwrap().is_none());
            let second = Command::new(&runner)
                .arg("__age319-private-held-prepared-v30")
                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
                .output()
                .unwrap();
            assert!(!second.status.success());
            assert_eq!(
                fs::read_dir(broker_state.join("entries")).unwrap().count(),
                1
            );
            stop(&mut restarted);
            unsafe {
                libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
            }
            return;
        }
        stop(&mut broker);
        fs::write(gate.join("finish"), b"done").unwrap();
        eventually(|| entry.try_wait().unwrap().is_some());
        assert!(!entry.wait().unwrap().success());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let records: Vec<_> = fs::read_dir(broker_state.join("entries"))
            .unwrap()
            .collect();
        assert_eq!(records.len(), 1);
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap())
                .unwrap();
        assert_eq!(record["join_consumed"], true);
        assert_eq!(
            record["joined_child"]["host_pid"],
            prepared.joined_child.host_pid
        );
        let restart_log = temp.path().join("prepared-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| {
            protocol::request_at(&socket, Operation::Classify).is_ok()
                || restarted.try_wait().unwrap().is_some()
        });
        assert!(
            restarted.try_wait().unwrap().is_none(),
            "restart: {}",
            fs::read_to_string(&restart_log).unwrap()
        );
        let retained = BrokerSidecar::open_existing(
            &broker_state.join("sidecar/pid-identity.db"),
            &broker_state,
        )
        .unwrap()
        .read_exact_prepared_owner(&generation, &prepared.root_id, &prepared.owner_generation)
        .unwrap();
        assert_eq!(retained, prepared);
        assert!(
            BrokerSidecar::open_existing(
                &broker_state.join("sidecar/pid-identity.db"),
                &broker_state,
            )
            .unwrap()
            .read_exact_release(&generation, &prepared.root_id, &prepared.owner_generation)
            .is_err()
        );
        let second = Command::new(&runner)
            .arg("__age319-private-held-prepared-v30")
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1", &gate)
            .output()
            .unwrap();
        assert!(!second.status.success());
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        stop(&mut restarted);
        unsafe {
            libc::kill(prepared.root_init.host_pid, libc::SIGKILL);
        }
        return;
    }
    if let Some(_generation) = broker_generation {
        // The test executable shares UID and broker access but is not the
        // fixed Runner image. It cannot inspect the route or select a path.
        assert!(protocol::state_route_at(&socket).is_err());
        // The user-owned source is now stale and even invalid. The Runner's
        // first production preflight must still learn v30 from the live
        // broker, before parsing any copied row or reserving an entry.
        fs::write(data.join("pid-identity.db"), b"retired copied sidecar").unwrap();
        let invoke = || {
            Command::new(&runner)
                .arg("--help")
                .env("OULIPOLY_DATA_DIR", &data)
                .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
                .env_remove("LD_LIBRARY_PATH")
                .output()
                .unwrap()
        };
        let first = invoke();
        assert!(!first.status.success());
        assert!(
            String::from_utf8_lossy(&first.stderr).contains("installed Runner has no v30 route"),
            "{}",
            String::from_utf8_lossy(&first.stderr)
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        stop(&mut broker);
        let unavailable = invoke();
        assert!(!unavailable.status.success());
        assert!(
            String::from_utf8_lossy(&unavailable.stderr)
                .contains("installed broker entry gate unavailable")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        let restart_log = temp.path().join("state-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| {
            restarted.try_wait().unwrap().is_some()
                || protocol::request_at(&socket, Operation::Classify).is_ok()
        });
        assert!(restarted.try_wait().unwrap().is_none());
        let second = invoke();
        assert!(!second.status.success());
        assert!(
            String::from_utf8_lossy(&second.stderr).contains("installed Runner has no v30 route")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
        stop(&mut restarted);
        return;
    }
    // The old source is a live, exact v29 island. A current Runner must refuse
    // it at read-only domain preflight, including for syntactically admitted
    // NormalCli requests, before E/J or a workload effect.
    drop(oulipoly_state::StateDb::open(&historical_data.join("state.db")).unwrap());
    let old_writer = rusqlite::Connection::open(&historical_sidecar).unwrap();
    let current_version: i64 = rusqlite::Connection::open(data.join("pid-identity.db"))
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    let historical_version: i64 = old_writer
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!((current_version, historical_version), (31, 29));
    old_writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA wal_autocheckpoint=0;
             INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
               state_dir,meta_path,log_path,rc_path,rc)
             VALUES('old-session','fixture','old-pending','{}','2026-09-24T00:00:00Z',
               '/old','/old/meta','/old/log','/old/rc',0);",
        )
        .unwrap();
    let old_wal = historical_data.join("pid-identity.db-wal");
    let old_main_before = fs::read(&historical_sidecar).unwrap();
    let old_wal_before = fs::read(&old_wal).unwrap();
    assert!(!old_wal_before.is_empty());
    let old_pending = || -> i64 {
        old_writer
            .query_row(
                "SELECT count(*) FROM mailbox WHERE handle='old-pending'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(old_pending(), 1);
    let normal_syntax = [
        vec!["--model", "age319-missing-model", "hello"],
        vec!["--new", "local provider fixture"],
        vec!["resume", "local-fixture-session"],
    ];
    for args in normal_syntax {
        assert!(protocol::supported_entry_args(
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>()
        ));
        let refused = Command::new(&runner)
            .args(&args)
            .env("OULIPOLY_DATA_DIR", &historical_data)
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env_remove("LD_LIBRARY_PATH")
            .output()
            .unwrap();
        assert!(!refused.status.success());
        assert!(
            String::from_utf8_lossy(&refused.stderr)
                .contains("completion domain schema lineage differs"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            0
        );
    }
    let old_help = Command::new(&runner)
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", &historical_data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!old_help.status.success());
    assert!(
        String::from_utf8_lossy(&old_help.stderr)
            .contains("completion domain schema lineage differs")
    );
    assert_eq!(
        fs::read_dir(broker_state.join("entries")).unwrap().count(),
        0
    );
    assert_eq!(old_pending(), 1);
    assert_eq!(fs::read(&historical_sidecar).unwrap(), old_main_before);
    assert_eq!(fs::read(&old_wal).unwrap(), old_wal_before);
    assert_eq!(
        fs::read_dir(broker_state.join("entries")).unwrap().count(),
        0
    );
    assert_eq!(
        fs::read_dir(broker_state.join("grants")).unwrap().count(),
        0
    );
    assert!(!gate.join("provider-effect").exists());
    assert!(!gate.join("bash-effect").exists());
    old_writer
        .execute(
            "INSERT INTO mailbox(session_id,kind,handle,payload_json,enqueued_at,
               state_dir,meta_path,log_path,rc_path,rc)
             VALUES('old-session','fixture','old-after-refusal','{}','2026-09-24T00:00:01Z',
               '/old','/old/meta','/old/log','/old/rc',0)",
            [],
        )
        .unwrap();
    assert_eq!(old_pending(), 1);
    assert!(!protocol::supported_entry_args(&[
        "--age319-unsupported".into()
    ]));
    let unsupported = Command::new(&runner)
        .arg("--age319-unsupported")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr).contains("unsupported kernel CLI mode"));
    assert_eq!(
        fs::read_dir(broker_state.join("entries")).unwrap().count(),
        0
    );
    let out = temp.path().join("runner.out");
    let err = temp.path().join("runner.err");
    let args: &[&str] = if mode == "diagnostics" {
        &["diagnostics", "metrics", "--minutes", "1", "--json"]
    } else if mode == "join_only" || mode == "root_drain_fence" {
        &["__age319-private-join-only-v1"]
    } else {
        &["--help"]
    };
    let mut entry = Command::new(&runner)
        .args(args)
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::from(File::create(&out).unwrap()))
        .stderr(Stdio::from(File::create(&err).unwrap()))
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(20);
    while !gate.join("ready").exists() && entry.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < until,
            "entry stalled: {} / {}",
            fs::read_to_string(&err).unwrap(),
            fs::read_to_string(&broker_log).unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        gate.join("ready").exists(),
        "entry failed: {} / {}",
        fs::read_to_string(&err).unwrap(),
        fs::read_to_string(&broker_log).unwrap()
    );
    // The exact Runner is still blocked before exec, although the namespace
    // and durable one-use entry transition already exist.
    assert!(entry.try_wait().unwrap().is_none());
    assert_eq!(fs::metadata(&out).unwrap().len(), 0);
    let records: Vec<_> = fs::read_dir(broker_state.join("entries"))
        .unwrap()
        .collect();
    assert_eq!(records.len(), 1);
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap()).unwrap();
    assert_eq!(record["domain_id"], domain);
    assert_eq!(record["join_consumed"], true);
    let root = record["root_id"].as_str().unwrap();
    let root_record: serde_json::Value =
        serde_json::from_slice(&fs::read(broker_state.join(format!("{root}.json"))).unwrap())
            .unwrap();
    let init_pid = root_record["init_host_pid"].as_i64().unwrap() as i32;
    let guardian_pid = record["guardian"]["host_pid"].as_i64().unwrap() as i32;
    let mailbox = MailboxDb::open_historical_read_only(&data.join("pid-identity.db")).unwrap();
    let owner = mailbox.completion_continuation_owner().unwrap().unwrap();
    assert_eq!(owner.guardian_identity.pid, i64::from(guardian_pid));
    assert_eq!(owner.domain_id, domain);
    assert_eq!(
        owner.supervisor_authority_id,
        record["supervisor_authority_id"].as_str().unwrap()
    );
    assert_eq!(
        mailbox
            .completion_owner_kernel_root_id(&owner.owner_generation)
            .unwrap()
            .as_deref(),
        Some(root)
    );
    assert_eq!(
        fs::read_link(format!("/proc/{guardian_pid}/ns/pid")).unwrap(),
        fs::read_link("/proc/self/ns/pid").unwrap()
    );
    let child_pid: i32 = fs::read_to_string(gate.join("ready"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(record["joined_child"]["host_pid"], child_pid);
    assert!(record["joined_child"]["starttime_ticks"].as_u64().is_some());
    assert_ne!(init_pid, child_pid);
    assert_eq!(
        fs::read_link(format!("/proc/{init_pid}/ns/pid")).unwrap(),
        fs::read_link(format!("/proc/{child_pid}/ns/pid")).unwrap()
    );
    assert_ne!(
        fs::read_link(format!("/proc/{init_pid}/ns/pid")).unwrap(),
        fs::read_link("/proc/self/ns/pid").unwrap()
    );
    if mode == "held_death" {
        // J has consumed its one use and fsynced the exact child, but the
        // broker still owns the pre-exec gate. Losing the broker must deliver
        // EOF, not an executable release or a second child.
        stop(&mut broker);
        eventually(|| entry.try_wait().unwrap().is_some());
        assert!(!entry.wait().unwrap().success());
        assert_eq!(fs::metadata(&out).unwrap().len(), 0);
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(records[0].as_ref().unwrap().path()).unwrap())
                .unwrap();
        assert_eq!(persisted["join_consumed"], true);
        assert_eq!(persisted["joined_child"]["host_pid"], child_pid);
        let restart_log = temp.path().join("held-restart.log");
        let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
            .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
            .stdout(Stdio::null())
            .stderr(Stdio::from(File::create(&restart_log).unwrap()))
            .spawn()
            .unwrap();
        eventually(|| {
            protocol::request_at(&socket, Operation::Classify).is_ok()
                || restarted.try_wait().unwrap().is_some()
        });
        assert!(
            restarted.try_wait().unwrap().is_none(),
            "held restart: {}",
            fs::read_to_string(&restart_log).unwrap()
        );
        assert!(
            protocol::request_at(&socket, Operation::ReserveEntry)
                .unwrap()
                .starts_with("error ")
        );
        assert_eq!(
            fs::read_dir(broker_state.join("entries")).unwrap().count(),
            1
        );
        stop(&mut restarted);
        unsafe {
            libc::kill(init_pid, libc::SIGKILL);
        }
        return;
    }
    fs::write(gate.join("release"), b"yes").unwrap();
    eventually(|| entry.try_wait().unwrap().is_some());
    let exit = entry.wait().unwrap();
    let entry_error = fs::read_to_string(&err).unwrap();
    assert!(
        exit.success(),
        "entry: {entry_error} broker: {}",
        fs::read_to_string(&broker_log).unwrap()
    );
    let output = fs::read_to_string(&out).unwrap();
    if mode == "diagnostics" {
        assert!(output.trim_start().starts_with('{'), "{output}");
    } else if mode == "join_only" || mode == "root_drain_fence" {
        assert!(output.is_empty(), "{output}");
    } else {
        assert!(output.contains("Usage:"));
    }
    if mode == "root_drain_fence" {
        let exact: RootRecord = serde_json::from_value(root_record.clone()).unwrap();
        let before = protocol::root_drain_readback_at(&socket, &exact, false).unwrap();
        let before: serde_json::Value =
            serde_json::from_str(before.strip_prefix("root-drain-v1 ").unwrap().trim_end())
                .unwrap();
        assert_eq!(before["root_id"], root);
        assert_eq!(before["fenced"], false);
        assert_eq!(before["state"], "blocked");
        assert_eq!(before["source_effect_readback_uncertain"], true);
        assert!(before["source_effect"].is_null());
        assert_eq!(before["state_sidecar_outstanding_unknown"], true);
        assert_eq!(before["close_eligible"], false);
        let mut stale = exact.clone();
        stale.init_starttime_ticks += 1;
        assert!(protocol::root_drain_readback_at(&socket, &stale, true).is_err());
        let mut other = exact.clone();
        other.root_id = uuid::Uuid::new_v4().to_string();
        assert!(protocol::root_drain_readback_at(&socket, &other, true).is_err());
        let fenced = protocol::root_drain_readback_at(&socket, &exact, true).unwrap();
        let fenced: serde_json::Value =
            serde_json::from_str(fenced.strip_prefix("root-drain-v1 ").unwrap().trim_end())
                .unwrap();
        assert_eq!(fenced["fenced"], true);
        assert_eq!(fenced["state"], "blocked");
        assert_eq!(fenced["close_eligible"], false);
        assert!(protocol::root_pid1_drain_at(&socket, &exact).is_err());
        assert!(
            !broker_state
                .join(format!("root-pid1/{root}.request.json"))
                .exists()
        );
        assert!(
            broker_state
                .join(format!("root-drains/{root}.json"))
                .exists()
        );
    }
    let sibling = Command::new(&runner)
        .arg("--help")
        .env("OULIPOLY_DATA_DIR", &data)
        .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(!sibling.status.success());
    assert!(String::from_utf8_lossy(&sibling.stderr).contains("reservation refused"));
    // Possession of the root/owner IDs and writable acceptance-shaped files
    // does not let a sibling manufacture a positive guardian grant.
    let forged_state = temp.path().join("forged-work");
    fs::create_dir(&forged_state).unwrap();
    let forged_intent = forged_state.join("root-work-intent-v1.json");
    let forged_acceptance = forged_state.join("root-work-accepted-v1.json");
    fs::write(&forged_intent, b"{}").unwrap();
    fs::write(&forged_acceptance, b"{}").unwrap();
    let image = File::open(&runner).unwrap();
    let intent = File::open(&forged_intent).unwrap();
    let accepted = File::open(&forged_acceptance).unwrap();
    let state = File::open(&forged_state).unwrap();
    let cwd = File::open(".").unwrap();
    let false_grant = protocol::prepare_accepted_work_at(
        &socket,
        &AcceptedWorkSpec {
            root_id: root.into(),
            work_id: "forged-work".into(),
            request_sha256: "0".repeat(64),
            accepted_sha256: "0".repeat(64),
            owner_generation: owner.owner_generation.clone(),
        },
        [
            image.as_raw_fd(),
            intent.as_raw_fd(),
            cwd.as_raw_fd(),
            state.as_raw_fd(),
            accepted.as_raw_fd(),
        ],
    )
    .unwrap();
    assert!(false_grant.starts_with("error "), "{false_grant}");
    if mode == "root_drain_fence" {
        assert!(
            false_grant.contains("exact root admission fenced"),
            "{false_grant}"
        );
    }
    assert_eq!(
        fs::read_dir(broker_state.join("grants")).unwrap().count(),
        0
    );
    // The caller is a sibling executable, not the pinned entry. The root ID
    // and complete alleged binding do not turn it into launch authority.
    let cwd = File::open(".").unwrap();
    let (_receipt, completion) = UnixStream::pair().unwrap();
    let spec = JoinSpec {
        root_id: root.into(),
        domain_id: domain,
        supervisor_id: record["supervisor_authority_id"].as_str().unwrap().into(),
        guardian_pid: record["guardian"]["host_pid"].as_i64().unwrap() as i32,
        root_authority: "{}".into(),
        args: vec!["--help".into()],
        environment: vec![],
    };
    let replay = protocol::join_at(
        &socket,
        &spec,
        [0, 1, 2, cwd.as_raw_fd(), completion.as_raw_fd()],
    )
    .unwrap();
    assert!(replay.starts_with("error "), "{replay}");
    if mode == "root_drain_fence" {
        assert!(replay.contains("exact root admission fenced"), "{replay}");
    }
    assert!(
        protocol::request_at(&socket, Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    stop(&mut broker);
    if mode == "root_drain_fence" {
        // The consumed bit is the durable boundary before a namespace fork.
        // A restart must surface a spent record with no WorkRegistry child.
        let grant_id = uuid::Uuid::new_v4().to_string();
        let stamp = serde_json::json!({"device": 1, "inode": 1});
        let grant = serde_json::json!({
            "version": 2,
            "grant_id": grant_id,
            "root_id": root,
            "root_init": {
                "host_pid": root_record["init_host_pid"],
                "boot_id": root_record["boot_id"],
                "starttime_ticks": root_record["init_starttime_ticks"],
                "pidns_dev": root_record["pidns_dev"],
                "pidns_ino": root_record["pidns_ino"]
            },
            "owner_uid": 0,
            "supervisor_authority_id": owner.supervisor_authority_id,
            "owner_generation": owner.owner_generation,
            "guardian": record["guardian"],
            "joined_child": record["joined_child"],
            "work_id": "accepted-before-fork",
            "parent_grant_id": null,
            "parent_work_incarnation": null,
            "accepted_sha256": "a".repeat(64),
            "request_sha256": "b".repeat(64),
            "initiator": {"pid": child_pid, "boot_id": root_record["boot_id"], "starttime_ticks": 1},
            "artifacts": {"executable": stamp, "intent": stamp, "cwd": stamp, "state_dir": stamp, "accepted": stamp},
            "sealed_helper": null,
            "consumed": true
        });
        fs::write(
            broker_state.join(format!("grants/{grant_id}.json")),
            serde_json::to_vec(&grant).unwrap(),
        )
        .unwrap();
    }
    // Restart reads the same root PID1 and spent join record. An uncertain
    // response cannot fork another original entry.
    assert!(Path::new(&format!("/proc/{init_pid}")).exists());
    let restart_log = temp.path().join("restart.log");
    let mut restarted = Command::new(env!("CARGO_BIN_EXE_oulipoly-kernel-broker"))
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", &socket)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1", &broker_state)
        .env("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1", &runner)
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::create(&restart_log).unwrap()))
        .spawn()
        .unwrap();
    eventually(|| {
        protocol::request_at(&socket, Operation::Classify).is_ok()
            || restarted.try_wait().unwrap().is_some()
    });
    assert!(
        restarted.try_wait().unwrap().is_none(),
        "restart: {}",
        fs::read_to_string(&restart_log).unwrap()
    );
    if mode == "root_drain_fence" {
        let exact: RootRecord = serde_json::from_value(root_record.clone()).unwrap();
        let after = protocol::root_drain_readback_at(&socket, &exact, false).unwrap();
        let after: serde_json::Value =
            serde_json::from_str(after.strip_prefix("root-drain-v1 ").unwrap().trim_end()).unwrap();
        assert_eq!(after["fenced"], true);
        assert_eq!(after["pid1"], "live");
        assert_eq!(after["spent_without_work"], 1);
        assert_eq!(after["state"], "blocked");
        assert_eq!(after["source_effect_readback_uncertain"], true);
        assert!(after["source_effect"].is_null());
        assert_eq!(after["state_sidecar_outstanding_unknown"], true);
        assert_eq!(after["close_eligible"], false);
    }
    assert!(
        protocol::request_at(&socket, Operation::ReserveEntry)
            .unwrap()
            .starts_with("error ")
    );
    let (wrong_socket, _other_end) = UnixStream::pair().unwrap();
    let replayed = SourceSocketWitness {
        root_id: root.into(),
        domain_id: owner.domain_id.clone(),
        supervisor_id: owner.supervisor_authority_id.clone(),
        guardian: ProcessWitness {
            host_pid: guardian_pid,
            boot_id: owner.guardian_identity.boot_id.clone(),
            starttime_ticks: u64::try_from(owner.guardian_identity.starttime_ticks).unwrap(),
        },
        source: ProcessWitness {
            host_pid: child_pid,
            boot_id: record["joined_child"]["boot_id"].as_str().unwrap().into(),
            starttime_ticks: record["joined_child"]["starttime_ticks"].as_u64().unwrap(),
        },
        scope: SourceScope::Root,
        delegated_root_h_request_id: None,
    };
    // Restart cannot turn the dead joined child's old host identity into the
    // current caller, even while its root PID1 and grant record remain live.
    assert!(
        protocol::verify_source_socket_at(&socket, &replayed, wrong_socket.as_raw_fd()).is_err()
    );
    stop(&mut restarted);
    assert!(
        protocol::verify_source_socket_at(&socket, &replayed, wrong_socket.as_raw_fd()).is_err()
    );
    // Explicit fixture teardown does not claim a production drain or safe
    // registry retirement protocol.
    unsafe {
        libc::kill(init_pid, libc::SIGKILL);
    }
}

fn raw_fresh_id_request(socket: &Path, opcode: u8, id: uuid::Uuid) -> String {
    use std::io::{Read, Write};
    let mut stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge).unwrap();
    let mut frame = vec![opcode];
    frame.extend_from_slice(&challenge);
    frame.extend_from_slice(id.as_bytes());
    stream.write_all(&frame).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

#[test]
fn original_runner_joins_once_behind_persistent_root_pid1() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    if std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE").is_none() {
        return;
    }
    let start_mode = std::env::var("AGE319_PRIVATE_JOIN_START_MODE").ok();
    let mut reached_start = start_mode.is_none();
    for mode in [
        "help",
        "diagnostics",
        "join_only",
        "root_drain_fence",
        "held_death",
        "broker_state",
        "held_prepared",
        "held_guardian_death",
        "held_release",
        "held_release_broker_death",
        "held_release_guardian_death",
        "held_release_child_death",
        "held_release_lost_reply",
        "held_release_prepare_lost_reply",
        "held_release_driver_route",
        "held_release_exec_driver",
        "held_release_commit_fail",
        "held_release_child_predeath",
        "held_release_gate_fail",
        "native_cancel",
        "native_drain",
        "native_receipt_cancel",
        "native_receipt_drain",
        "native_pid1_loss",
        "native_receipt_pid1_loss",
        "normal_release",
        "normal_owner_discovery",
        "normal_handoff",
        "normal_handoff_bash_child",
        "normal_handoff_pre_k_h_source",
        "normal_handoff_pre_k_h_source_registration_cli",
        "normal_handoff_pre_k_h_source_postcommit_w",
        "normal_handoff_pre_k_h_source_postcommit_w_v2",
        "normal_handoff_pre_k_h_source_postcommit_w_v2_wake",
        "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_lost",
        "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_unknown",
        "normal_handoff_fsync",
        "normal_handoff_effect_reply_loss",
        "normal_help",
        "normal_model_held",
        "normal_model_provider",
        "normal_model_provider_caller_binary",
        "normal_model_provider_caller_nonzero",
        "normal_model_provider_caller_partial",
        "normal_model_provider_caller_lost",
        "normal_model_provider_pty_control",
        "normal_model_provider_pty_physical",
        "normal_model_provider_pty_physical_resident_tail",
        "normal_model_provider_pty_physical_resident_bash_notify",
        "normal_model_provider_pty_physical_resident_bash_f_fenced",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_restart",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_restart",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_partial",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_fence",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_partial",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_write",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_turn",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_receipt",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_turn_changed_native",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_turn_changed_session",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_receipt_changed_native",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_crash_after_write_changed_control",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_lost_transport",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_lost_receipt",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_physical_lost_ack",
        "normal_model_provider_pty_physical_resident_bash_f_fenced_adapter_unsupported",
        "normal_model_provider_pty_physical_resident_bash_response",
        "normal_model_provider_pty_physical_resident_bash_wrong_image",
        "normal_model_provider_pty_physical_resident_bash_absent_parent",
        "normal_model_provider_pty_physical_resident_bash_ambiguous_parent",
        "normal_model_provider_pty_physical_resident_bash_wrong_plan",
        "normal_model_provider_pty_physical_resident_bash_wrong_actor",
        "normal_model_provider_pty_physical_resident_bash_unconsumed_parent",
        "normal_model_provider_pty_physical_finalizer_wrong_pair",
        "normal_model_provider_pty_physical_resident_absent_socket",
        "normal_model_provider_pty_physical_resident_replaced_socket",
        "normal_model_provider_pty_physical_resident_wrong_account",
        "normal_model_provider_pty_physical_resident_wrong_session",
        "normal_model_provider_pty_physical_resident_stale_provider",
        "normal_model_provider_pty_physical_wrong_plan",
        "normal_model_provider_pty_physical_wrong_actor",
        "normal_model_provider_pty_physical_reply_loss",
        "normal_model_provider_pty_physical_post_k_unknown",
        "normal_model_provider_pty_physical_restart",
        "normal_model_provider_pty_physical_restart_after_k",
        "normal_model_provider_pty_physical_root_exit",
        "normal_model_provider_pty_restart",
        "normal_model_provider_pty_replaced",
        "normal_model_provider_pty_root_exit",
        "normal_model_provider_bash_causal",
        "normal_model_provider_bash_causal_success",
        "normal_model_provider_bash_causal_root_h_delegate",
        "normal_model_provider_bash_causal_root_h_delegate_drain",
        "normal_model_provider_bash_causal_root_h_lost",
        "normal_model_provider_bash_causal_root_h_notify_row",
        "normal_model_provider_native_codex_root_h_notify_row",
        "normal_model_provider_native_codex_turn_receipt",
        "normal_model_provider_native_codex_recipient_ack",
        "normal_model_provider_native_codex_f_candidate",
        "normal_model_provider_native_codex_f_turn",
        "normal_model_provider_native_codex_f_turn_old_pending_debt",
        "normal_model_provider_bash_causal_root_h_notify_row_drain",
        "normal_model_provider_bash_causal_root_h_notify_wake",
        "normal_model_provider_bash_causal_root_h_notify_wake_drain",
        "normal_model_provider_bash_causal_root_h_notify_wake_lost",
        "normal_model_provider_bash_causal_root_h_notify_wake_unknown",
        "normal_model_provider_bash_causal_w_debt",
        "normal_model_provider_bash_causal_notify_w_debt",
        "normal_model_provider_bash_causal_notify_ack",
        "normal_model_provider_bash_causal_notify_prepare_unavailable",
        "normal_model_provider_bash_causal_notify_lost_pending",
        "normal_model_provider_bash_causal_notify_debt",
        "normal_model_provider_bash_causal_notify_row_debt",
        "normal_model_provider_bash_ordinary_sync",
        "normal_model_provider_bash_ordinary_sync_parent_output",
        "normal_model_provider_bash_ordinary_async",
        "normal_model_provider_bash_ordinary_refuse",
        "normal_model_provider_bash_ordinary_loss",
        "normal_model_provider_bash_ordinary_copy",
        "normal_model_provider_bash_ordinary_restart",
        "normal_model_provider_bash_ordinary_elf",
        "normal_model_provider_bash_ordinary_failure",
        "normal_model_provider_bash_ordinary_cancel",
        "normal_model_provider_bash_ordinary_parent_tamper",
        "normal_model_provider_bash_ordinary_sync_reply_loss",
        "normal_model_provider_bash_ordinary_sync_partial",
        "normal_model_provider_bash_ordinary_sync_repeat",
        "normal_model_provider_bash_ordinary_sync_large",
        "normal_model_provider_bash_ordinary_sync_signal",
        "normal_model_provider_bash_ordinary_sync_tamper",
        "normal_model_provider_bash_ordinary_sync_w_debt",
        "normal_model_provider_bash_ordinary_sync_socket_partial",
        "normal_model_provider_bash_ordinary_sync_post_tamper",
        "normal_model_provider_bash_ordinary_sync_encode_tamper",
        "normal_model_provider_bash_ordinary_script",
        "normal_model_provider_bash_ordinary_script_replace",
        "normal_model_provider_bash_ordinary_script_remove",
        "normal_model_provider_bash_ordinary_script_loss",
        "normal_model_provider_reply_loss",
        "normal_model_provider_q_reply_loss",
        "normal_model_provider_restart",
        "normal_model_provider_bad_config",
        "normal_model_provider_unsupported",
        "normal_model_provider_v3_closed",
        "normal_model_provider_v3_quota",
        "normal_model_provider_v3_quota_reply_loss",
        "normal_model_provider_v3_quota_post_k",
        "normal_model_provider_v3_quota_restart",
        "normal_model_provider_v3_quota_invalid",
        "normal_model_provider_v3_quota_full",
        "normal_model_provider_v3_quota_stale",
        "normal_model_provider_v3_quota_auth",
        "normal_model_provider_v3_quota_auth_reply_loss",
        "normal_model_provider_v3_quota_auth_post_k",
        "normal_model_provider_v3_quota_auth_restart",
        "normal_model_provider_v3_quota_auth_failed",
        "normal_model_provider_v3_quota_auth_shared",
        "normal_model_provider_v3_quota_route",
        "normal_model_provider_v3_quota_route_reply_loss",
        "normal_model_provider_v3_quota_route_invalid",
        "normal_model_provider_v3_quota_route_full",
        "normal_model_provider_v3_quota_route_stale",
        "normal_model_provider_v3_quota_route_auth_failed",
        "normal_model_provider_v3_quota_route_manual_healthy",
        "normal_model_provider_v3_quota_route_manual_invalid",
        "normal_model_provider_v3_quota_route_manual_full",
        "normal_model_provider_v3_quota_route_manual_failed",
        "normal_model_provider_v3_quota_route_manual_cross_model",
        "normal_model_provider_v3_quota_route_manual_refresh",
        "normal_model_provider_v3_quota_route_manual_reply_loss",
        "normal_model_provider_v3_quota_route_manual_route_reply_loss",
        "normal_model_provider_v3_quota_route_manual_pending",
        "normal_model_provider_v3_quota_route_manual_unknown",
        "normal_model_provider_v3_quota_route_manual_physical",
        "normal_model_provider_v3_quota_route_manual_physical_terminal",
        "normal_model_provider_v3_quota_route_manual_physical_capacity_terminal",
        "normal_model_provider_v3_quota_route_manual_physical_account_quota_terminal",
        "normal_model_provider_v3_quota_route_manual_physical_cross_model",
        "normal_model_provider_v3_quota_route_manual_physical_refresh",
        "normal_model_provider_v3_quota_route_manual_physical_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_q_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_restart",
        "normal_model_provider_v3_quota_route_manual_physical_manual_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_route_reply_loss",
        "normal_model_provider_v3_quota_route_manual_physical_capacity",
        "normal_model_provider_v3_quota_route_manual_physical_account_quota",
        "normal_model_provider_v3_quota_route_manual_physical_bad_plan",
        "normal_model_provider_v3_quota_route_manual_physical_bad_actor",
        "normal_model_provider_v3_quota_route_manual_physical_source_changed",
        "normal_model_provider_v3_quota_route_physical",
        "normal_model_provider_v3_quota_route_physical_reply_loss",
        "normal_model_provider_v3_quota_route_physical_post_k",
        "normal_model_provider_v3_quota_route_physical_restart",
        "normal_model_provider_v3_quota_route_physical_q_reply_loss",
        "normal_model_provider_v3_quota_route_physical_nonzero",
        "normal_model_provider_v3_quota_route_physical_capacity",
        "normal_model_provider_v3_quota_route_physical_account_quota",
        "normal_model_provider_v3_quota_route_physical_bad_plan",
        "normal_model_provider_v3_quota_route_physical_bad_actor",
        "normal_model_provider_v3_quota_route_physical_source_changed",
        "normal_model_provider_v3_quota_route_physical_shared",
        "normal_model_provider_v3_quota_route_physical_shared_manual",
        "normal_model_provider_v3_quota_route_physical_shared_command_changed",
        "normal_model_provider_v3_quota_route_physical_shared_env_changed",
        "normal_model_provider_v3_quota_route_physical_shared_config_changed",
        "normal_model_provider_v3_quota_route_physical_shared_account_changed",
        "normal_model_provider_v3_quota_route_physical_shared_reply_loss",
        "normal_model_provider_v3_quota_route_physical_shared_restart",
        "normal_model_provider_v3_quota_route_physical_shared_pending",
        "normal_model_provider_quota",
        "normal_model_provider_auth",
        "normal_model_provider_auth_recovery",
        "normal_model_provider_auth_after_healthy",
        "normal_model_provider_auth_success",
        "normal_model_provider_auth_reply_loss",
        "normal_model_provider_auth_restart",
        "normal_model_provider_quota_available",
        "normal_model_provider_quota_reply_loss",
        "normal_model_provider_quota_restart",
        "normal_model_provider_no_pin",
        "normal_model_provider_path",
        "normal_model_provider_prefix",
        "normal_guardian_death",
        "normal_driver_death",
        "normal_broker_death",
        "normal_prepare_lost_reply",
        "normal_release_lost_reply",
        "normal_repair_lost_reply",
        "normal_source_grant_lost_reply",
        "normal_recipient",
        "normal_recipient_driver_post",
        "normal_recipient_broker_post",
        "normal_empty",
        "normal_empty_bad_disposition",
        "normal_guardian_post",
        "normal_driver_post",
        "normal_broker_post",
    ] {
        if !reached_start {
            reached_start = start_mode.as_deref() == Some(mode);
            if !reached_start {
                continue;
            }
        }
        if std::env::var("AGE319_PRIVATE_JOIN_ONLY_MODE")
            .ok()
            .is_some_and(|only| only != mode)
        {
            continue;
        }
        if std::env::var("AGE319_PRIVATE_JOIN_ONLY_PREFIX")
            .ok()
            .is_some_and(|prefix| !mode.starts_with(&prefix))
        {
            continue;
        }
        let mut command = Command::new("unshare");
        if mode == "normal_handoff_pre_k_h_source_postcommit_w_v2"
            || mode == "normal_handoff_pre_k_h_source_postcommit_w_v2_wake"
            || mode == "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_lost"
            || mode == "normal_handoff_pre_k_h_source_postcommit_w_v2_wake_unknown"
            || mode.starts_with("normal_model_provider_bash_causal_root_h_notify_wake")
        {
            command.args([
                "-Upfm",
                "--map-root-user",
                "--map-users=1:100000:65535",
                "--map-groups=1:100000:65535",
                "--mount-proc",
            ]);
        } else {
            command.args(["-Urpfm", "--mount-proc"]);
        }
        let output = command
            .arg(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "original_runner_joins_once_behind_persistent_root_pid1",
                "--nocapture",
            ])
            .env("AGE319_PRIVATE_JOIN_INNER", "1")
            .env("AGE319_PRIVATE_JOIN_MODE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        if mode.starts_with("normal_handoff_pre_k_h_source_postcommit_w")
            || mode == "normal_model_provider_pty_physical"
            || mode == "normal_model_provider_pty_physical_resident_tail"
            || mode == "normal_model_provider_pty_physical_resident_bash_notify"
        {
            eprint!("{}", String::from_utf8_lossy(&output.stderr));
        }
        eprintln!("private root mode passed: {mode}");
        if std::env::var("AGE319_PRIVATE_JOIN_THROUGH_MODE")
            .ok()
            .as_deref()
            == Some(mode)
        {
            break;
        }
    }
}

#[test]
fn combined_v30_source_witness_uses_held_release_and_original_state() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "combined_v30_source_witness_uses_held_release_and_original_state",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env("AGE319_PRIVATE_JOIN_MODE", "held_release_source_witness")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn consumed_h_bash_bytes_reach_production_registration_cli() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "consumed_h_bash_bytes_reach_production_registration_cli",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env(
            "AGE319_PRIVATE_JOIN_MODE",
            "normal_handoff_pre_k_h_source_registration_cli",
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn consumed_h_postcommit_wakes_exact_source_once() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "consumed_h_postcommit_wakes_exact_source_once",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env(
            "AGE319_PRIVATE_JOIN_MODE",
            "normal_handoff_pre_k_h_source_postcommit_w",
        )
        .output()
        .unwrap();
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn resident_bash_restart_after_w_retains_failing_signal() {
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "original_runner_joins_once_behind_persistent_root_pid1",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env(
            "AGE319_PRIVATE_JOIN_MODE",
            "normal_model_provider_pty_physical_resident_bash_restart",
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn resident_bash_restart_after_transcript_append_recovers_exact_q() {
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "original_runner_joins_once_behind_persistent_root_pid1",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env(
            "AGE319_PRIVATE_JOIN_MODE",
            "normal_model_provider_pty_physical_resident_bash_after_append",
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn resident_bash_corrupt_prior_transcript_refuses_q() {
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "original_runner_joins_once_behind_persistent_root_pid1",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env(
            "AGE319_PRIVATE_JOIN_MODE",
            "normal_model_provider_pty_physical_resident_bash_after_append_corrupt",
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn genuine_bash_source_v30_is_one_use_and_not_accepted_without_commit_custody() {
    if std::env::var_os("AGE319_PRIVATE_JOIN_INNER").is_some() {
        inner();
        return;
    }
    let Some(mode) = std::env::var("AGE319_PRIVATE_BASH_MODE").ok() else {
        return;
    };
    assert!(matches!(
        mode.as_str(),
        "normal_bash_source"
            | "normal_bash_source_lost_reply"
            | "normal_bash_source_nonzero"
            | "normal_bash_source_capture_io_failure"
    ));
    assert!(std::env::var_os("OULIPOLY_AGE319_RUNNER_IMAGE").is_some());
    assert!(std::env::var_os("AGE319_PRIVATE_BASH_REGISTRATION").is_some());
    assert!(std::env::var_os("AGE319_PRIVATE_BASH_STATE_DB").is_some());
    let output = Command::new("unshare")
        .args(["-Urpfm", "--mount-proc"])
        .arg(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "genuine_bash_source_v30_is_one_use_and_not_accepted_without_commit_custody",
            "--nocapture",
        ])
        .env("AGE319_PRIVATE_JOIN_INNER", "1")
        .env("AGE319_PRIVATE_JOIN_MODE", &mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{mode}: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
