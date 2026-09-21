#![cfg(target_os = "linux")]

use oulipoly_state::maintenance::MAINTENANCE_DIRECTORY;
use oulipoly_state::pid_identity::{ProcessIdentity, read_live_process_identity};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::{Command, Stdio};
use uuid::Uuid;

const WORKER_ARG: &str = "--oulipoly-detached-maintenance-worker-v1";

#[derive(Serialize)]
struct LaunchProcessIdentity<'a> {
    os_pid: i64,
    os_boot_id: &'a str,
    os_pid_starttime_ticks: i64,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum LaunchAdmissionState {
    Admitted,
}

#[derive(Serialize)]
struct LaunchBody<'a> {
    format_version: u32,
    opportunity_epoch: i64,
    launch_id: Uuid,
    schedule_basis: &'a str,
    state: LaunchAdmissionState,
    child: LaunchProcessIdentity<'a>,
    outcome: Option<String>,
    parent_error: Option<String>,
    child_error: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    parent_evidence_gap: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    child_evidence_gap: bool,
    fallback_armed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_launch: Option<String>,
}

#[derive(Serialize)]
struct LaunchRecord<'a> {
    #[serde(flatten)]
    body: LaunchBody<'a>,
    checksum_sha256: String,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum FallbackState {
    Armed,
}

#[derive(Serialize)]
struct FallbackBody<'a> {
    format_version: u32,
    opportunity_epoch: i64,
    launch_id: Uuid,
    schedule_basis: &'a str,
    state: FallbackState,
    error: Option<String>,
}

#[derive(Serialize)]
struct FallbackRecord<'a> {
    #[serde(flatten)]
    body: FallbackBody<'a>,
    checksum_sha256: String,
}

#[derive(Deserialize)]
struct ObservedFallback {
    opportunity_epoch: i64,
    launch_id: Uuid,
    schedule_basis: String,
    state: String,
    error: Option<String>,
    checksum_sha256: String,
}

fn sha256(domain: &[u8], value: &impl Serialize) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(serde_json::to_vec(value).unwrap());
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn launch_record<'a>(
    epoch: i64,
    launch_id: Uuid,
    basis: &'a str,
    child: &'a ProcessIdentity,
) -> LaunchRecord<'a> {
    let body = LaunchBody {
        format_version: 2,
        opportunity_epoch: epoch,
        launch_id,
        schedule_basis: basis,
        state: LaunchAdmissionState::Admitted,
        child: LaunchProcessIdentity {
            os_pid: child.os_pid,
            os_boot_id: &child.os_boot_id,
            os_pid_starttime_ticks: child.os_pid_starttime_ticks,
        },
        outcome: None,
        parent_error: None,
        child_error: None,
        parent_evidence_gap: false,
        child_evidence_gap: false,
        fallback_armed: true,
        previous_launch: None,
    };
    let checksum_sha256 = sha256(b"oulipoly.maintenance-launch.v2\0", &body);
    LaunchRecord {
        body,
        checksum_sha256,
    }
}

fn armed_fallback(epoch: i64, launch_id: Uuid, basis: &str) -> FallbackRecord<'_> {
    let body = FallbackBody {
        format_version: 1,
        opportunity_epoch: epoch,
        launch_id,
        schedule_basis: basis,
        state: FallbackState::Armed,
        error: None,
    };
    let checksum_sha256 = sha256(b"oulipoly.maintenance-launch-fallback.v1\0", &body);
    FallbackRecord {
        body,
        checksum_sha256,
    }
}

fn write_json(path: &std::path::Path, value: &impl Serialize) {
    let mut bytes = serde_json::to_vec(value).unwrap();
    bytes.push(b'\n');
    fs::write(path, bytes).unwrap();
}

#[test]
fn private_child_persists_exact_fallback_when_launch_ledger_update_fails() {
    let root = tempfile::tempdir().unwrap();
    let launch_dir = root.path().join(MAINTENANCE_DIRECTORY).join("launch");
    fs::create_dir_all(&launch_dir).unwrap();
    let admission = launch_dir.join("admission.lock");
    fs::write(&admission, []).unwrap();

    let epoch = chrono::Utc::now().timestamp() / (24 * 60 * 60);
    let epoch_dir = launch_dir.join(format!("opportunity-{epoch}"));
    fs::create_dir(&epoch_dir).unwrap();
    let launch_id = Uuid::new_v4();
    let basis = "fallback_integration";
    let fallback = epoch_dir.join(format!("{launch_id}.fallback.json"));
    let stderr = epoch_dir.join(format!("{launch_id}.stderr"));
    write_json(&fallback, &armed_fallback(epoch, launch_id, basis));

    let mut child = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"))
        .args([
            WORKER_ARG,
            &epoch.to_string(),
            &launch_id.to_string(),
            basis,
        ])
        .env("OULIPOLY_DATA_DIR", root.path())
        .env("OULIPOLY_MAINTENANCE_ERROR_CARRIER", &fallback)
        .env("OULIPOLY_MAINTENANCE_ERROR_EPOCH", epoch.to_string())
        .env(
            "OULIPOLY_MAINTENANCE_ERROR_LAUNCH_ID",
            launch_id.to_string(),
        )
        .env("OULIPOLY_MAINTENANCE_ERROR_BASIS", basis)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&stderr)
                .unwrap(),
        ))
        .spawn()
        .unwrap();
    let identity = read_live_process_identity(i64::from(child.id()))
        .unwrap()
        .unwrap();
    write_json(
        &epoch_dir.join("index.json"),
        &launch_record(epoch, launch_id, basis, &identity),
    );

    // Admission is already represented by the exact launch record. Turning the
    // lock path into a directory makes the later result-ledger open fail without
    // blocking the child from validating its admission record.
    fs::remove_file(&admission).unwrap();
    fs::create_dir(&admission).unwrap();
    writeln!(child.stdin.take().unwrap(), "{launch_id}").unwrap();
    let status = child.wait().unwrap();
    assert!(!status.success());

    let observed: ObservedFallback = serde_json::from_slice(&fs::read(&fallback).unwrap()).unwrap();
    assert_eq!(observed.opportunity_epoch, epoch);
    assert_eq!(observed.launch_id, launch_id);
    assert_eq!(observed.schedule_basis, basis);
    assert_eq!(observed.state, "failed");
    assert!(
        observed
            .error
            .as_deref()
            .is_some_and(|error| error.contains("directory"))
    );
    assert_eq!(observed.checksum_sha256.len(), 64);
    assert!(
        fs::read_to_string(stderr)
            .unwrap()
            .contains("OULIPOLY_MAINTENANCE_GAP=worker_failed")
    );
}
