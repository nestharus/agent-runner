//! Central durable session admission queue and capacity policy.

use oulipoly_state::mailbox::{MailboxDb, SessionAdmissionAttempt, SessionAdmissionRow};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIN_AVAILABLE_MEMORY_ENV: &str = "OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES";
const DEFAULT_MEMORY_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MEMORY_RESERVE_PERCENT: u64 = 10;
const RESERVATION_STALE_AFTER: Duration = Duration::from_secs(60);
const WAIT_RETRY_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AdmissionCapacityConfig {
    minimum_available_memory_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MemoryObservation {
    available_bytes: u64,
    total_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrainOutcome {
    Admitted,
    Pressure,
    Empty,
    Contended,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueueStatus {
    reason: String,
    position: Option<i64>,
}

pub(crate) struct SessionAdmissionGuard {
    mailbox_path: PathBuf,
    registration_identity: String,
    claim_token: String,
}

impl Drop for SessionAdmissionGuard {
    fn drop(&mut self) {
        let settled = MailboxDb::open(&self.mailbox_path).and_then(|mut db| {
            db.session_admissions()
                .settle(&self.registration_identity, &self.claim_token)
        });
        if let Err(error) = settled {
            tracing::warn!(
                registration_identity = self.registration_identity,
                "Failed to settle session admission: {error}"
            );
            return;
        }
        if let Err(error) = drain_one_at(&self.mailbox_path) {
            tracing::warn!("Failed to drain session admission queue after settlement: {error}");
        }
        super::sweep::run_post_settlement_wake_reclaim_sweep();
    }
}

pub(super) fn enqueue_and_wait(
    registration_identity: &str,
    session_id: Option<&str>,
) -> Result<SessionAdmissionGuard, String> {
    let mailbox_path = MailboxDb::default_path()?;
    enqueue_and_wait_at(&mailbox_path, registration_identity, session_id)
}

fn enqueue_and_wait_at(
    mailbox_path: &Path,
    registration_identity: &str,
    session_id: Option<&str>,
) -> Result<SessionAdmissionGuard, String> {
    let admission_id = uuid::Uuid::new_v4().to_string();
    let now = unix_time_ms()?;
    let launcher =
        oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or_else(|| "Session admission launcher identity is not live".to_string())?;
    let mut db = MailboxDb::open(mailbox_path)?;
    db.session_admissions().enqueue(
        &admission_id,
        registration_identity,
        session_id,
        &launcher,
        now,
    )?;
    drop(db);

    let mut reported = None;
    loop {
        let mut db = MailboxDb::open(mailbox_path)?;
        let row = db
            .session_admissions()
            .row(registration_identity)?
            .ok_or_else(|| {
                format!("Session admission {registration_identity} disappeared while queued")
            })?;
        if let Some(claim_token) = admitted_claim_token(&row)
            && db.session_admissions().begin_launch(
                registration_identity,
                claim_token,
                unix_time_ms()?,
            )?
        {
            report_launching(registration_identity, session_id);
            return Ok(SessionAdmissionGuard {
                mailbox_path: mailbox_path.to_path_buf(),
                registration_identity: registration_identity.to_string(),
                claim_token: claim_token.to_string(),
            });
        }
        let position = db
            .session_admissions()
            .queued_position(registration_identity)?;
        report_queued_status(
            registration_identity,
            QueueStatus {
                reason: row.queue_reason,
                position,
            },
            &mut reported,
        );
        drop(db);
        if position == Some(1) {
            let outcome = match drain_one_at(mailbox_path) {
                Ok(DrainOutcome::Admitted) => continue,
                Ok(outcome) => outcome,
                Err(error) => {
                    report_queued_status(
                        registration_identity,
                        QueueStatus {
                            reason: "coordination_unavailable".to_string(),
                            position,
                        },
                        &mut reported,
                    );
                    tracing::warn!(
                        registration_identity,
                        "Session admission retry retained: {error}"
                    );
                    DrainOutcome::Contended
                }
            };
            let mut db = MailboxDb::open(mailbox_path)?;
            db.session_admissions().update_queued_reason(
                registration_identity,
                queued_reason(outcome),
                unix_time_ms()?,
            )?;
        }
        std::thread::sleep(WAIT_RETRY_INTERVAL);
    }
}

fn queued_reason(outcome: DrainOutcome) -> &'static str {
    match outcome {
        DrainOutcome::Admitted | DrainOutcome::Empty => "fifo_wait",
        DrainOutcome::Pressure => "memory_pressure",
        DrainOutcome::Contended => "drain_contended",
    }
}

fn report_queued_status(
    registration_identity: &str,
    status: QueueStatus,
    reported: &mut Option<QueueStatus>,
) {
    if reported.as_ref() == Some(&status) {
        return;
    }
    eprintln!(
        "OULIPOLY_SESSION_ADMISSION={}",
        serde_json::json!({
            "state": "queued",
            "registration_identity": registration_identity,
            "reason": &status.reason,
            "queue_position": status.position,
        })
    );
    *reported = Some(status);
}

fn report_launching(registration_identity: &str, session_id: Option<&str>) {
    eprintln!(
        "OULIPOLY_SESSION_ADMISSION={}",
        serde_json::json!({
            "state": "launching",
            "registration_identity": registration_identity,
            "session_id": session_id,
        })
    );
}

fn admitted_claim_token(row: &SessionAdmissionRow) -> Option<&str> {
    (row.state == "admitted")
        .then_some(row.claim_token.as_deref())
        .flatten()
}

fn drain_one_at(mailbox_path: &Path) -> Result<DrainOutcome, String> {
    let config = AdmissionCapacityConfig::from_env()?;
    let result = super::sweep::try_with_serialized_drain(mailbox_path, || {
        let mut db = MailboxDb::open(mailbox_path)?;
        drain_one_with(&mut db, config, observe_system_memory)
    })?;
    Ok(result.unwrap_or(DrainOutcome::Contended))
}

pub(super) fn drain_one_owned(db: &mut MailboxDb) -> Result<(), String> {
    let config = AdmissionCapacityConfig::from_env()?;
    let _ = drain_one_with(db, config, observe_system_memory)?;
    Ok(())
}

fn drain_one_with(
    db: &mut MailboxDb,
    config: AdmissionCapacityConfig,
    observe_memory: impl FnOnce() -> Result<Option<MemoryObservation>, String>,
) -> Result<DrainOutcome, String> {
    let observation = match observe_memory() {
        Ok(observation) => observation,
        Err(error) => {
            tracing::warn!("Session admission memory observation unavailable: {error}");
            None
        }
    };
    if observation.is_some_and(|observation| memory_pressure(config, observation)) {
        return Ok(DrainOutcome::Pressure);
    }
    let now = unix_time_ms()?;
    let stale_before =
        now.saturating_sub(i64::try_from(RESERVATION_STALE_AFTER.as_millis()).unwrap_or(i64::MAX));
    let claim_token = uuid::Uuid::new_v4().to_string();
    match db
        .session_admissions()
        .try_admit_next(&claim_token, now, stale_before)?
    {
        SessionAdmissionAttempt::Admitted(_) => Ok(DrainOutcome::Admitted),
        SessionAdmissionAttempt::Empty => Ok(DrainOutcome::Empty),
    }
}

impl AdmissionCapacityConfig {
    fn from_env() -> Result<Self, String> {
        let minimum_available_memory_bytes = match std::env::var(MIN_AVAILABLE_MEMORY_ENV) {
            Ok(value) => Some(parse_positive_u64(MIN_AVAILABLE_MEMORY_ENV, &value)?),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(format!("{MIN_AVAILABLE_MEMORY_ENV} must be UTF-8"));
            }
        };
        Ok(Self {
            minimum_available_memory_bytes,
        })
    }
}

fn parse_positive_u64(name: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("{name} must be a positive byte count"))
}

fn memory_pressure(config: AdmissionCapacityConfig, observation: MemoryObservation) -> bool {
    let reserve = config.minimum_available_memory_bytes.unwrap_or_else(|| {
        DEFAULT_MEMORY_RESERVE_BYTES.max(
            observation
                .total_bytes
                .saturating_mul(DEFAULT_MEMORY_RESERVE_PERCENT)
                / 100,
        )
    });
    observation.available_bytes < reserve
}

#[cfg(target_os = "linux")]
fn observe_system_memory() -> Result<Option<MemoryObservation>, String> {
    let contents = std::fs::read_to_string("/proc/meminfo")
        .map_err(|error| format!("Failed to observe system memory pressure: {error}"))?;
    let available_bytes = meminfo_bytes(&contents, "MemAvailable")?;
    let total_bytes = meminfo_bytes(&contents, "MemTotal")?;
    Ok(Some(MemoryObservation {
        available_bytes,
        total_bytes,
    }))
}

#[cfg(not(target_os = "linux"))]
fn observe_system_memory() -> Result<Option<MemoryObservation>, String> {
    Ok(None)
}

#[cfg(target_os = "linux")]
fn meminfo_bytes(contents: &str, key: &str) -> Result<u64, String> {
    let value_kib = contents
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name == key).then(|| value.split_whitespace().next())?
        })
        .ok_or_else(|| format!("System memory observation omitted {key}"))?
        .parse::<u64>()
        .map_err(|error| format!("Invalid {key} in system memory observation: {error}"))?;
    value_kib
        .checked_mul(1024)
        .ok_or_else(|| format!("System memory observation overflowed for {key}"))
}

fn unix_time_ms() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("System time precedes Unix epoch: {error}"))?
        .as_millis()
        .try_into()
        .map_err(|_| "System time exceeds session admission range".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::{
        AttachRuntimeGenerationSession, CreateRuntimeGeneration, ExitRuntimeGenerationNonOrderly,
        RuntimeGenerationFence, RuntimeGenerationId, RuntimeTerminalReason,
    };
    use std::sync::{Arc, Barrier};

    fn config() -> AdmissionCapacityConfig {
        AdmissionCapacityConfig {
            minimum_available_memory_bytes: Some(100),
        }
    }

    fn roomy_memory() -> Result<Option<MemoryObservation>, String> {
        Ok(Some(MemoryObservation {
            available_bytes: 1_000,
            total_bytes: 2_000,
        }))
    }

    fn enqueue(db: &mut MailboxDb, identity: &str, session_id: Option<&str>, now: i64) {
        let launcher =
            oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
                .unwrap()
                .unwrap();
        db.session_admissions()
            .enqueue(
                &format!("admission-{identity}"),
                identity,
                session_id,
                &launcher,
                now,
            )
            .unwrap();
    }

    #[test]
    fn initial_and_resume_launches_are_durably_queued_before_admission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "initial-invocation", None, 1);
        enqueue(&mut db, "resume-invocation", Some("provider-session"), 2);

        let initial = db
            .session_admissions()
            .row("initial-invocation")
            .unwrap()
            .unwrap();
        let resumed = db
            .session_admissions()
            .row("resume-invocation")
            .unwrap()
            .unwrap();
        assert_eq!(initial.state, "queued");
        assert_eq!(resumed.state, "queued");
        assert!(initial.queue_sequence < resumed.queue_sequence);
        assert_eq!(
            db.session_admissions()
                .queued_position("initial-invocation")
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            db.session_admissions()
                .queued_position("resume-invocation")
                .unwrap(),
            Some(2)
        );
    }

    #[test]
    fn drain_is_fifo_and_admits_only_one_per_capacity_observation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "first", None, 1);
        enqueue(&mut db, "second", None, 2);

        assert_eq!(
            drain_one_with(&mut db, config(), roomy_memory).unwrap(),
            DrainOutcome::Admitted
        );
        assert_eq!(
            db.session_admissions().row("first").unwrap().unwrap().state,
            "admitted"
        );
        assert_eq!(
            db.session_admissions()
                .row("second")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
    }

    #[test]
    fn pressure_retains_fifo_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "pressure", None, 1);
        let low = || {
            Ok(Some(MemoryObservation {
                available_bytes: 99,
                total_bytes: 2_000,
            }))
        };
        assert_eq!(
            drain_one_with(&mut db, config(), low).unwrap(),
            DrainOutcome::Pressure
        );
        assert_eq!(
            db.session_admissions()
                .row("pressure")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );

        assert_eq!(
            db.session_admissions()
                .row("pressure")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
    }

    #[test]
    fn unavailable_memory_observation_preserves_fifo_admission() {
        fn assert_admitted(
            observe_memory: impl FnOnce() -> Result<Option<MemoryObservation>, String>,
        ) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("pid-identity.db");
            let mut db = MailboxDb::open(&path).unwrap();
            enqueue(&mut db, "first", None, 1);
            enqueue(&mut db, "second", None, 2);

            assert_eq!(
                drain_one_with(&mut db, config(), observe_memory).unwrap(),
                DrainOutcome::Admitted
            );
            assert_eq!(
                db.session_admissions().row("first").unwrap().unwrap().state,
                "admitted"
            );
            assert_eq!(
                db.session_admissions()
                    .row("second")
                    .unwrap()
                    .unwrap()
                    .state,
                "queued"
            );
        }

        assert_admitted(|| Ok(None));
        assert_admitted(|| Err("injected observation failure".to_string()));
    }

    #[test]
    fn settlement_immediately_admits_the_next_fifo_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "first", None, 1);
        enqueue(&mut db, "second", None, 2);
        drain_one_with(&mut db, config(), roomy_memory).unwrap();
        let first = db.session_admissions().row("first").unwrap().unwrap();
        db.session_admissions()
            .settle("first", first.claim_token.as_deref().unwrap())
            .unwrap();
        assert_eq!(
            drain_one_with(&mut db, config(), roomy_memory).unwrap(),
            DrainOutcome::Admitted
        );
        assert_eq!(
            db.session_admissions()
                .row("second")
                .unwrap()
                .unwrap()
                .state,
            "admitted"
        );
    }

    #[test]
    fn concurrent_drainers_share_one_cross_process_admission_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "first", None, 1);
        enqueue(&mut db, "second", None, 2);
        drop(db);
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                super::super::sweep::try_with_serialized_drain(&path, || {
                    std::thread::sleep(Duration::from_millis(50));
                    let mut db = MailboxDb::open(&path)?;
                    drain_one_with(&mut db, config(), roomy_memory)
                })
                .unwrap()
            }));
        }
        barrier.wait();
        let outcomes = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_some()).count(),
            1
        );
        let mut db = MailboxDb::open(&path).unwrap();
        let admitted = ["first", "second"]
            .into_iter()
            .filter(|identity| {
                db.session_admissions()
                    .row(identity)
                    .unwrap()
                    .unwrap()
                    .state
                    == "admitted"
            })
            .count();
        assert_eq!(admitted, 1);
    }

    #[test]
    fn stale_reservation_recovers_without_losing_fifo_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "first", None, 1);
        enqueue(&mut db, "second", None, 2);
        drain_one_with(&mut db, config(), roomy_memory).unwrap();
        let attempt = db
            .session_admissions()
            .try_admit_next("replacement-token", i64::MAX, i64::MAX)
            .unwrap();
        let SessionAdmissionAttempt::Admitted(row) = attempt else {
            panic!("stale row must recover")
        };
        assert_eq!(row.registration_identity, "first");
        assert_eq!(
            db.session_admissions()
                .row("second")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
    }

    #[test]
    fn live_launching_reservation_does_not_expire_or_block_its_successor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "first", None, 1);
        enqueue(&mut db, "second", None, 2);
        drain_one_with(&mut db, config(), roomy_memory).unwrap();
        let first = db.session_admissions().row("first").unwrap().unwrap();
        assert!(
            db.session_admissions()
                .begin_launch("first", first.claim_token.as_deref().unwrap(), 3)
                .unwrap()
        );

        let SessionAdmissionAttempt::Admitted(second) = db
            .session_admissions()
            .try_admit_next("replacement-token", i64::MAX, i64::MAX)
            .unwrap()
        else {
            panic!("roomy memory must admit a successor while its parent is live");
        };
        assert_eq!(second.registration_identity, "second");
        assert_eq!(
            db.session_admissions().row("first").unwrap().unwrap().state,
            "launching"
        );
    }

    #[test]
    fn dead_queued_launcher_is_cancelled_before_fifo_admission() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let dead = oulipoly_state::pid_identity::ProcessIdentity {
            os_pid: i64::MAX,
            os_boot_id: "dead-boot".to_string(),
            os_pid_starttime_ticks: 1,
        };
        db.session_admissions()
            .enqueue("dead-admission", "dead", None, &dead, 1)
            .unwrap();
        enqueue(&mut db, "live", None, 2);

        assert_eq!(
            drain_one_with(&mut db, config(), roomy_memory).unwrap(),
            DrainOutcome::Admitted
        );
        assert_eq!(
            db.session_admissions().row("dead").unwrap().unwrap().state,
            "cancelled"
        );
        assert_eq!(
            db.session_admissions().row("live").unwrap().unwrap().state,
            "admitted"
        );
    }

    #[test]
    fn active_generation_does_not_block_a_child_and_binds_initial_session_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "initial-invocation", None, 1);
        enqueue(&mut db, "next-invocation", None, 2);
        drain_one_with(&mut db, config(), roomy_memory).unwrap();
        let initial = db
            .session_admissions()
            .row("initial-invocation")
            .unwrap()
            .unwrap();
        assert!(
            db.session_admissions()
                .begin_launch(
                    "initial-invocation",
                    initial.claim_token.as_deref().unwrap(),
                    3,
                )
                .unwrap()
        );

        let generation_id = RuntimeGenerationId::new();
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "initial-invocation",
                session_id: None,
                runtime_mode: "headless",
                provider_name: "test-provider",
                model_name: Some("test-model"),
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        assert_eq!(
            drain_one_with(&mut db, config(), roomy_memory).unwrap(),
            DrainOutcome::Admitted
        );

        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "initial-invocation",
        };
        db.runtime_lifecycle()
            .attach_runtime_generation_session(AttachRuntimeGenerationSession {
                fence,
                session_id: "provider-session",
            })
            .unwrap();
        let bound = db
            .session_admissions()
            .row("initial-invocation")
            .unwrap()
            .unwrap();
        assert_eq!(bound.session_id.as_deref(), Some("provider-session"));
        assert_eq!(
            bound.runtime_generation_uuid.as_deref(),
            Some(generation_id.to_string().as_str())
        );

        db.runtime_lifecycle()
            .exit_runtime_generation_non_orderly(ExitRuntimeGenerationNonOrderly {
                fence,
                reason: RuntimeTerminalReason::StartupFailed,
                exit_code: None,
            })
            .unwrap();
        assert_eq!(
            db.session_admissions()
                .row("initial-invocation")
                .unwrap()
                .unwrap()
                .state,
            "settled"
        );
        assert_eq!(
            db.session_admissions()
                .row("next-invocation")
                .unwrap()
                .unwrap()
                .state,
            "admitted"
        );
    }
}
