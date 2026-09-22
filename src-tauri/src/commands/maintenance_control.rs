//! Local, exact-job maintenance inspection and cancellation.
//!
//! This command opens only the filesystem maintenance ledger. It intentionally
//! runs before completion-owner bootstrap, provider construction, and State DB
//! access.

use oulipoly_state::maintenance::{
    MaintenanceCheckpoint, MaintenanceEvidenceRecord, MaintenanceJobKey, MaintenanceJobKind,
    MaintenanceJobStore, MaintenanceRequestEvidenceGap, now_unix_micros,
};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub(crate) struct MaintenanceInspection {
    command: String,
    key: MaintenanceJobKey,
    checkpoint: Option<MaintenanceCheckpoint>,
    evidence: Option<MaintenanceEvidenceRecord>,
    evidence_read_error: Option<String>,
    request_evidence_gap: Option<MaintenanceRequestEvidenceGap>,
    request_evidence_gap_read_error: Option<String>,
    cancellation_requested_for_epoch: Option<i64>,
}

pub(crate) fn run_status(
    command: &str,
    kind: &str,
    partition: &str,
    json: bool,
) -> Result<i32, String> {
    let data_root = oulipoly_state::paths::data_dir()?;
    let inspection = inspect_at(&data_root, command, kind, partition)?;
    render(&inspection, json)?;
    Ok(0)
}

pub(crate) fn run_cancel(
    kind: &str,
    partition: &str,
    epoch: i64,
    json: bool,
) -> Result<i32, String> {
    let data_root = oulipoly_state::paths::data_dir()?;
    let inspection = cancel_at(&data_root, kind, partition, epoch)?;
    render(&inspection, json)?;
    Ok(0)
}

pub(crate) fn inspect_at(
    data_root: &Path,
    command: &str,
    kind: &str,
    partition: &str,
) -> Result<MaintenanceInspection, String> {
    let key = parse_key(kind, partition)?;
    let store = MaintenanceJobStore::open(data_root).map_err(|error| error.to_string())?;
    let checkpoint = store.read_status(&key).map_err(|error| error.to_string())?;
    let (evidence, evidence_read_error) = match store.read_evidence(&key) {
        Ok(evidence) => (evidence, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let (request_evidence_gap, request_evidence_gap_read_error) =
        match store.read_request_evidence_gap(&key) {
            Ok(gap) => (gap, None),
            Err(error) => (None, Some(error.to_string())),
        };
    let cancellation_requested_for_epoch = store
        .read_cancellation_epoch(&key)
        .map_err(|error| error.to_string())?;
    Ok(MaintenanceInspection {
        command: command.to_string(),
        key,
        checkpoint,
        evidence,
        evidence_read_error,
        request_evidence_gap,
        request_evidence_gap_read_error,
        cancellation_requested_for_epoch,
    })
}

fn cancel_at(
    data_root: &Path,
    kind: &str,
    partition: &str,
    epoch: i64,
) -> Result<MaintenanceInspection, String> {
    let key = parse_key(kind, partition)?;
    let store = MaintenanceJobStore::open(data_root).map_err(|error| error.to_string())?;
    store
        .request_admitted_cancellation(&key, epoch, now_unix_micros())
        .map_err(|error| error.to_string())?;
    inspect_at(data_root, "maintenance cancel", kind, partition)
}

fn parse_key(kind: &str, partition: &str) -> Result<MaintenanceJobKey, String> {
    let kind = MaintenanceJobKind::parse(kind)
        .ok_or_else(|| format!("unknown maintenance job kind {kind:?}"))?;
    MaintenanceJobKey::new(kind, partition).map_err(|error| error.to_string())
}

fn render(output: &MaintenanceInspection, json: bool) -> Result<(), String> {
    let rendered = serde_json::to_string_pretty(output)
        .map_err(|error| format!("failed to render maintenance inspection: {error}"))?;
    if !json {
        println!("{}", output.command);
    }
    println!("{rendered}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::maintenance::{
        AcquireMaintenanceJob, MaintenanceRunState, MaintenanceWorkerIdentity,
    };

    #[test]
    fn offline_reader_observes_durable_job_evidence_and_cancel_requires_exact_epoch() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key =
            MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "event-store-v1").unwrap();
        let AcquireMaintenanceJob::Acquired(mut job) = store
            .acquire(
                key.clone(),
                44,
                MaintenanceWorkerIdentity::current().unwrap(),
                10,
            )
            .unwrap()
        else {
            panic!("job must be acquired");
        };
        job.record_progress("walking", Some("node:17".into()), 7, 11, 11)
            .unwrap();

        let observed = inspect_at(
            root.path(),
            "diagnostics maintenance",
            "event_discovery",
            "event-store-v1",
        )
        .unwrap();
        assert_eq!(observed.evidence.as_ref().unwrap().phase, "walking");
        assert_eq!(
            observed.evidence.as_ref().unwrap().state,
            MaintenanceRunState::Running
        );
        assert!(
            !root
                .path()
                .join("diagnostics/event-store-v1/writers")
                .exists()
        );

        assert!(cancel_at(root.path(), "event_discovery", "event-store-v1", 43).is_err());
        cancel_at(root.path(), "event_discovery", "event-store-v1", 44).unwrap();
        assert!(job.is_cancelled().unwrap());
        assert_eq!(
            inspect_at(
                root.path(),
                "maintenance status",
                "event_discovery",
                "event-store-v1"
            )
            .unwrap()
            .cancellation_requested_for_epoch,
            Some(44)
        );

        let directory = root
            .path()
            .join(oulipoly_state::maintenance::MAINTENANCE_DIRECTORY)
            .join(key.storage_id().unwrap());
        for slot in 0..=1 {
            let path = directory.join(format!("evidence.{slot}"));
            if path.is_file() {
                std::fs::remove_file(path).unwrap();
            }
        }
        std::fs::create_dir(directory.join("evidence.0")).unwrap();
        let gap = inspect_at(
            root.path(),
            "diagnostics maintenance",
            "event_discovery",
            "event-store-v1",
        )
        .unwrap();
        assert!(gap.checkpoint.is_some());
        assert!(gap.evidence.is_none());
        assert!(gap.evidence_read_error.is_some());
    }

    #[test]
    fn offline_reader_exposes_request_evidence_gap_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let store = MaintenanceJobStore::open(root.path()).unwrap();
        let key = MaintenanceJobKey::new(MaintenanceJobKind::EventDiscovery, "gap-test").unwrap();
        let AcquireMaintenanceJob::Acquired(mut job) = store
            .acquire(
                key.clone(),
                45,
                MaintenanceWorkerIdentity::current().unwrap(),
                20,
            )
            .unwrap()
        else {
            panic!("job must be acquired");
        };
        job.record_progress("walking", None, 0, 0, 21).unwrap();
        let directory = store.root().join(key.storage_id().unwrap());
        for slot in 0..=1 {
            let path = directory.join(format!("evidence.{slot}"));
            if path.is_file() {
                std::fs::remove_file(&path).unwrap();
            }
            std::fs::create_dir(&path).unwrap();
        }
        assert!(matches!(
            store
                .acquire(key, 45, MaintenanceWorkerIdentity::current().unwrap(), 22)
                .unwrap(),
            AcquireMaintenanceJob::DuplicateLive(_)
        ));
        let observed = inspect_at(
            root.path(),
            "diagnostics maintenance",
            "event_discovery",
            "gap-test",
        )
        .unwrap();
        let gap = observed.request_evidence_gap.unwrap();
        assert_eq!(gap.opportunity_epoch, 45);
        assert!(!gap.reason.is_empty());
        assert_eq!(gap.checksum_sha256.len(), 64);
        assert!(observed.request_evidence_gap_read_error.is_none());
        drop(job);
    }
}
