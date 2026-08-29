//! Central durable session admission queue and capacity policy.

use oulipoly_state::mailbox::{MailboxDb, SessionAdmissionAttempt, SessionAdmissionRow};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MIN_AVAILABLE_MEMORY_ENV: &str = "OULIPOLY_SESSION_ADMISSION_MIN_AVAILABLE_MEMORY_BYTES";
const DEFAULT_MEMORY_RESERVE_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MEMORY_RESERVE_PERCENT: u64 = 10;
const RESERVATION_STALE_AFTER: Duration = Duration::from_secs(60);
const WAIT_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const MEMORY_OBSERVATION_UNAVAILABLE: &str = "memory_observation_unavailable";

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
    LaunchMaterializing,
    Pressure,
    ObservationUnavailable,
    Waiting,
    Empty,
    Contended,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueueStatus {
    reason: String,
    sequence: i64,
}

pub(crate) struct SessionAdmissionGuard {
    mailbox_path: PathBuf,
    registration_identity: String,
    claim_token: String,
    run_post_settlement_sweep: bool,
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
        if self.run_post_settlement_sweep {
            super::sweep::run_post_settlement_wake_reclaim_sweep();
        }
    }
}

pub(super) fn enqueue_and_wait(
    registration_identity: &str,
    session_id: Option<&str>,
) -> Result<SessionAdmissionGuard, String> {
    let mailbox_path = MailboxDb::default_path()?;
    enqueue_and_wait_at_with_memory_observer(
        &mailbox_path,
        registration_identity,
        session_id,
        observe_system_memory,
        true,
    )
}

#[cfg(test)]
fn enqueue_and_wait_at(
    mailbox_path: &Path,
    registration_identity: &str,
    session_id: Option<&str>,
) -> Result<SessionAdmissionGuard, String> {
    enqueue_and_wait_at_with_memory_observer(
        mailbox_path,
        registration_identity,
        session_id,
        observe_system_memory,
        false,
    )
}

fn enqueue_and_wait_at_with_memory_observer(
    mailbox_path: &Path,
    registration_identity: &str,
    session_id: Option<&str>,
    mut observe_memory: impl FnMut() -> Result<Option<MemoryObservation>, String>,
    run_post_settlement_sweep: bool,
) -> Result<SessionAdmissionGuard, String> {
    let config = AdmissionCapacityConfig::from_env()?;
    let admission_id = uuid::Uuid::new_v4().to_string();
    let now = unix_time_ms()?;
    let launcher =
        oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))?
            .ok_or_else(|| "Session admission launcher identity is not live".to_string())?;
    let mut db = MailboxDb::open(mailbox_path)?;
    let admission = db.session_admissions().enqueue(
        &admission_id,
        registration_identity,
        session_id,
        &launcher,
        now,
    )?;
    let admission_id = admission.admission_id;
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
                run_post_settlement_sweep,
            });
        }
        let queued = row.state == "queued";
        if queued {
            report_queued_status(
                registration_identity,
                QueueStatus {
                    reason: row.queue_reason,
                    sequence: row.queue_sequence,
                },
                &mut reported,
            );
        }
        drop(db);
        if queued {
            match drain_one_at_with_config_and_observer(
                mailbox_path,
                config,
                Some(registration_identity),
                &mut observe_memory,
            ) {
                Ok(DrainOutcome::Admitted) => continue,
                Ok(DrainOutcome::ObservationUnavailable) => {
                    let mut db = MailboxDb::open(mailbox_path)?;
                    if db.session_admissions().cancel_queued(
                        registration_identity,
                        &admission_id,
                        MEMORY_OBSERVATION_UNAVAILABLE,
                        unix_time_ms()?,
                    )? {
                        return Err(format!(
                            "Session admission {MEMORY_OBSERVATION_UNAVAILABLE}: provider launch \
                             requires supported host-memory telemetry"
                        ));
                    }
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    report_queued_status(
                        registration_identity,
                        QueueStatus {
                            reason: "coordination_unavailable".to_string(),
                            sequence: row.queue_sequence,
                        },
                        &mut reported,
                    );
                    tracing::warn!(
                        registration_identity,
                        "Session admission retry retained: {error}"
                    );
                }
            }
        }
        std::thread::sleep(WAIT_RETRY_INTERVAL);
    }
}

fn queued_reason(outcome: DrainOutcome) -> &'static str {
    match outcome {
        DrainOutcome::Admitted | DrainOutcome::Waiting | DrainOutcome::Empty => "fifo_wait",
        DrainOutcome::LaunchMaterializing => "launch_materializing",
        DrainOutcome::Pressure => "memory_pressure",
        DrainOutcome::ObservationUnavailable => MEMORY_OBSERVATION_UNAVAILABLE,
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
            "queue_sequence": status.sequence,
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
    drain_one_at_with_config(mailbox_path, config, None)
}

fn drain_one_at_with_config(
    mailbox_path: &Path,
    config: AdmissionCapacityConfig,
    requested_registration_identity: Option<&str>,
) -> Result<DrainOutcome, String> {
    drain_one_at_with_config_and_observer(
        mailbox_path,
        config,
        requested_registration_identity,
        observe_system_memory,
    )
}

fn drain_one_at_with_config_and_observer(
    mailbox_path: &Path,
    config: AdmissionCapacityConfig,
    requested_registration_identity: Option<&str>,
    observe_memory: impl FnOnce() -> Result<Option<MemoryObservation>, String>,
) -> Result<DrainOutcome, String> {
    let result = super::sweep::try_with_serialized_drain(mailbox_path, || {
        let mut db = MailboxDb::open(mailbox_path)?;
        drain_with(
            &mut db,
            config,
            observe_memory,
            requested_registration_identity,
        )
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
    drain_with(db, config, observe_memory, None)
}

fn drain_with(
    db: &mut MailboxDb,
    config: AdmissionCapacityConfig,
    observe_memory: impl FnOnce() -> Result<Option<MemoryObservation>, String>,
    requested_registration_identity: Option<&str>,
) -> Result<DrainOutcome, String> {
    let observation = match observe_memory() {
        Ok(Some(observation)) => observation,
        Ok(None) => {
            return retain_queued_for_outcome(
                db,
                requested_registration_identity,
                DrainOutcome::ObservationUnavailable,
            );
        }
        Err(error) => {
            tracing::warn!("Session admission memory observation unavailable: {error}");
            return retain_queued_for_outcome(
                db,
                requested_registration_identity,
                DrainOutcome::ObservationUnavailable,
            );
        }
    };
    if memory_pressure(config, observation) {
        return retain_queued_for_outcome(
            db,
            requested_registration_identity,
            DrainOutcome::Pressure,
        );
    }
    let now = unix_time_ms()?;
    let stale_before =
        now.saturating_sub(i64::try_from(RESERVATION_STALE_AFTER.as_millis()).unwrap_or(i64::MAX));
    let claim_token = uuid::Uuid::new_v4().to_string();
    let attempt = match requested_registration_identity {
        Some(registration_identity) => db.session_admissions().try_admit_registration(
            registration_identity,
            &claim_token,
            now,
            stale_before,
        )?,
        None => db
            .session_admissions()
            .try_admit_next(&claim_token, now, stale_before)?,
    };
    let outcome = match attempt {
        SessionAdmissionAttempt::Admitted(_) => DrainOutcome::Admitted,
        SessionAdmissionAttempt::LaunchMaterializing => DrainOutcome::LaunchMaterializing,
        SessionAdmissionAttempt::Waiting => DrainOutcome::Waiting,
        SessionAdmissionAttempt::Empty => DrainOutcome::Empty,
    };
    retain_queued_for_outcome(db, requested_registration_identity, outcome)
}

fn retain_queued_for_outcome(
    db: &mut MailboxDb,
    requested_registration_identity: Option<&str>,
    outcome: DrainOutcome,
) -> Result<DrainOutcome, String> {
    let Some(registration_identity) = requested_registration_identity else {
        return Ok(outcome);
    };
    let Some(row) = db.session_admissions().row(registration_identity)? else {
        return Ok(outcome);
    };
    let reason = queued_reason(outcome);
    if row.state == "queued" && row.queue_reason != reason {
        db.session_admissions().update_queued_reason(
            registration_identity,
            reason,
            unix_time_ms()?,
        )?;
    }
    Ok(outcome)
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
    let proc_observation = std::fs::read_to_string("/proc/meminfo")
        .map_err(|error| format!("Failed to read /proc/meminfo: {error}"))
        .and_then(|contents| {
            memory_observation(
                meminfo_bytes(&contents, "MemAvailable")?,
                meminfo_bytes(&contents, "MemTotal")?,
            )
        });
    Ok(linux_memavailable_or_unavailable(proc_observation))
}

#[cfg(target_os = "linux")]
fn linux_memavailable_or_unavailable(
    proc_observation: Result<MemoryObservation, String>,
) -> Option<MemoryObservation> {
    match proc_observation {
        Ok(observation) => Some(observation),
        Err(error) => {
            tracing::warn!("Linux MemAvailable observation unavailable: {error}");
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn observe_system_memory() -> Result<Option<MemoryObservation>, String> {
    let mut total_bytes = 0_u64;
    let mut total_size = std::mem::size_of::<u64>();
    let mut total_mib = [libc::CTL_HW, libc::HW_MEMSIZE];
    if unsafe {
        libc::sysctl(
            total_mib.as_mut_ptr(),
            total_mib.len() as _,
            (&mut total_bytes as *mut u64).cast(),
            &mut total_size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(format!(
            "Failed to observe macOS total memory: {}",
            std::io::Error::last_os_error()
        ));
    }

    static HOST_PORT: std::sync::OnceLock<libc::mach_port_t> = std::sync::OnceLock::new();
    #[allow(deprecated)]
    let host = *HOST_PORT.get_or_init(|| unsafe { libc::mach_host_self() });
    let mut statistics = unsafe { std::mem::zeroed::<libc::vm_statistics64>() };
    let mut statistics_count = libc::HOST_VM_INFO64_COUNT;
    if unsafe {
        libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            (&mut statistics as *mut libc::vm_statistics64).cast(),
            &mut statistics_count,
        )
    } != libc::KERN_SUCCESS
    {
        return Err("Failed to observe macOS available memory".to_string());
    }
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(format!(
            "Failed to observe macOS memory page size: {}",
            std::io::Error::last_os_error()
        ));
    }
    let available_bytes = macos_available_memory_bytes(
        u64::from(statistics.active_count),
        u64::from(statistics.inactive_count),
        u64::from(statistics.free_count),
        page_size as u64,
    );
    memory_observation(available_bytes, total_bytes).map(Some)
}

#[cfg(any(target_os = "macos", test))]
fn macos_available_memory_bytes(
    active_pages: u64,
    inactive_pages: u64,
    free_pages: u64,
    page_size: u64,
) -> u64 {
    // XNU's doc/vm/memorystatus_notify.md defines this as active + inactive +
    // free + speculative; osfmk/kern/host.c reports free_count as free + speculative.
    active_pages
        .saturating_add(inactive_pages)
        .saturating_add(free_pages)
        .saturating_mul(page_size)
}

#[cfg(target_os = "windows")]
fn observe_system_memory() -> Result<Option<MemoryObservation>, String> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..MEMORYSTATUSEX::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(format!(
            "Failed to observe Windows system memory: {}",
            std::io::Error::last_os_error()
        ));
    }
    memory_observation(status.ullAvailPhys, status.ullTotalPhys).map(Some)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn observe_system_memory() -> Result<Option<MemoryObservation>, String> {
    Ok(None)
}

fn memory_observation(available_bytes: u64, total_bytes: u64) -> Result<MemoryObservation, String> {
    if total_bytes == 0 {
        return Err("System memory observation reported zero total memory".to_string());
    }
    if available_bytes > total_bytes {
        return Err(format!(
            "System memory observation reported {available_bytes} available bytes but only \
             {total_bytes} total bytes"
        ));
    }
    Ok(MemoryObservation {
        available_bytes,
        total_bytes,
    })
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
        AttachRuntimeGenerationSession, BindRuntimeGenerationRunning, CreateRuntimeGeneration,
        ExitRuntimeGenerationNonOrderly, RuntimeGenerationFence, RuntimeGenerationId,
        RuntimeTerminalReason,
    };
    use std::sync::{Arc, Barrier, mpsc};

    fn config() -> AdmissionCapacityConfig {
        AdmissionCapacityConfig {
            minimum_available_memory_bytes: Some(100),
        }
    }

    fn default_config() -> AdmissionCapacityConfig {
        AdmissionCapacityConfig {
            minimum_available_memory_bytes: None,
        }
    }

    fn roomy_memory() -> Result<Option<MemoryObservation>, String> {
        Ok(Some(MemoryObservation {
            available_bytes: 1_000,
            total_bytes: 2_000,
        }))
    }

    fn roomy_default_memory() -> Result<Option<MemoryObservation>, String> {
        Ok(Some(MemoryObservation {
            available_bytes: 7 * 1024 * 1024 * 1024,
            total_bytes: 8 * 1024 * 1024 * 1024,
        }))
    }

    #[test]
    fn supported_platform_memory_observer_returns_usable_values() {
        let observation = observe_system_memory()
            .unwrap()
            .expect("supported platforms must provide memory observation");
        assert!(observation.total_bytes > 0);
        assert!(observation.available_bytes <= observation.total_bytes);
    }

    #[test]
    fn macos_available_memory_follows_xnu_non_compressed_capacity() {
        assert_eq!(macos_available_memory_bytes(70, 20, 10, 4096), 409_600);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn invalid_linux_memavailable_is_reported_as_unavailable() {
        assert_eq!(
            linux_memavailable_or_unavailable(Err("injected procfs failure".to_string())),
            None
        );
    }

    fn enqueue(
        db: &mut MailboxDb,
        identity: &str,
        session_id: Option<&str>,
        now: i64,
    ) -> SessionAdmissionRow {
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
            .unwrap()
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
    }

    #[test]
    fn unavailable_memory_observation_returns_visible_error_without_stranding_queue() {
        fn assert_rejected(observe_memory: fn() -> Result<Option<MemoryObservation>, String>) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("pid-identity.db");

            let error = enqueue_and_wait_at_with_memory_observer(
                &path,
                "unobservable",
                None,
                observe_memory,
                false,
            )
            .err()
            .expect("missing telemetry must reject the launch");
            assert!(error.contains(MEMORY_OBSERVATION_UNAVAILABLE));
            assert!(error.contains("requires supported host-memory telemetry"));

            let mut db = MailboxDb::open(&path).unwrap();
            let row = db
                .session_admissions()
                .row("unobservable")
                .unwrap()
                .unwrap();
            assert_eq!(row.state, "cancelled");
            assert_eq!(row.queue_reason, MEMORY_OBSERVATION_UNAVAILABLE);
            assert_eq!(
                db.session_admissions()
                    .try_admit_next("claim", i64::MAX, i64::MIN)
                    .unwrap(),
                SessionAdmissionAttempt::Empty
            );
        }

        assert_rejected(|| Ok(None));
        assert_rejected(|| Err("injected observation failure".to_string()));
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
            drain_with(&mut db, config(), low, Some("pressure")).unwrap(),
            DrainOutcome::Pressure
        );
        let retained = db.session_admissions().row("pressure").unwrap().unwrap();
        assert_eq!(retained.state, "queued");
        assert_eq!(retained.queue_reason, "memory_pressure");
        assert_eq!(
            drain_with(&mut db, config(), roomy_memory, Some("pressure")).unwrap(),
            DrainOutcome::Admitted
        );
    }

    #[test]
    fn unavailable_memory_observation_retains_fifo_until_recovery() {
        fn assert_recovery(observe_memory: fn() -> Result<Option<MemoryObservation>, String>) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("pid-identity.db");
            let mut db = MailboxDb::open(&path).unwrap();
            let identities = (0..8)
                .map(|index| format!("invocation-{index}"))
                .collect::<Vec<_>>();
            for (index, identity) in identities.iter().enumerate() {
                enqueue(&mut db, identity, None, index as i64 + 1);
            }

            for identity in &identities {
                assert_eq!(
                    drain_with(&mut db, default_config(), observe_memory, Some(identity),).unwrap(),
                    DrainOutcome::ObservationUnavailable
                );
                let row = db.session_admissions().row(identity).unwrap().unwrap();
                assert_eq!(row.state, "queued");
                assert_eq!(row.queue_reason, "memory_observation_unavailable");
            }

            for (index, identity) in identities.iter().enumerate() {
                assert_eq!(
                    drain_with(
                        &mut db,
                        default_config(),
                        roomy_default_memory,
                        Some(identity),
                    )
                    .unwrap(),
                    DrainOutcome::Admitted
                );
                let row = db.session_admissions().row(identity).unwrap().unwrap();
                assert!(
                    db.session_admissions()
                        .begin_launch(
                            identity,
                            row.claim_token.as_deref().unwrap(),
                            index as i64 + 20,
                        )
                        .unwrap()
                );
                assert!(
                    db.session_admissions()
                        .settle(identity, row.claim_token.as_deref().unwrap())
                        .unwrap()
                );
            }
        }

        assert_recovery(|| Ok(None));
        assert_recovery(|| Err("injected observation failure".to_string()));
    }

    #[test]
    fn unavailable_memory_observation_cancels_only_the_exact_queued_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        let first = enqueue(&mut db, "first", None, 1);
        let second = enqueue(&mut db, "second", None, 2);

        assert_eq!(
            drain_with(&mut db, default_config(), || Ok(None), Some("first")).unwrap(),
            DrainOutcome::ObservationUnavailable
        );
        assert!(
            db.session_admissions()
                .cancel_queued(
                    "first",
                    &first.admission_id,
                    MEMORY_OBSERVATION_UNAVAILABLE,
                    3,
                )
                .unwrap()
        );
        let rejected = db.session_admissions().row("first").unwrap().unwrap();
        assert_eq!(rejected.state, "cancelled");
        assert_eq!(rejected.queue_reason, MEMORY_OBSERVATION_UNAVAILABLE);
        let remaining = db.session_admissions().row("second").unwrap().unwrap();
        assert_eq!(remaining.state, "queued");
        assert_eq!(remaining.queue_sequence, second.queue_sequence);

        assert!(
            !db.session_admissions()
                .cancel_queued(
                    "second",
                    &first.admission_id,
                    MEMORY_OBSERVATION_UNAVAILABLE,
                    4,
                )
                .unwrap()
        );
        assert_eq!(
            drain_with(
                &mut db,
                default_config(),
                roomy_default_memory,
                Some("second"),
            )
            .unwrap(),
            DrainOutcome::Admitted
        );
        assert!(
            !db.session_admissions()
                .cancel_queued(
                    "second",
                    &second.admission_id,
                    MEMORY_OBSERVATION_UNAVAILABLE,
                    5,
                )
                .unwrap()
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
    fn roomy_memory_serializes_startup_without_a_lifetime_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        for index in 0..8 {
            let identity = format!("invocation-{index}");
            enqueue(&mut db, &identity, None, i64::from(index) + 1);
        }

        for index in 0..8 {
            assert_eq!(
                drain_one_with(&mut db, config(), roomy_memory).unwrap(),
                DrainOutcome::Admitted
            );
            let identity = format!("invocation-{index}");
            let row = db.session_admissions().row(&identity).unwrap().unwrap();
            assert!(
                db.session_admissions()
                    .begin_launch(&identity, row.claim_token.as_deref().unwrap(), 20 + index)
                    .unwrap()
            );
            if index < 7 {
                assert_eq!(
                    drain_one_with(&mut db, config(), roomy_memory).unwrap(),
                    DrainOutcome::LaunchMaterializing
                );
            }
            assert!(
                db.session_admissions()
                    .settle(&identity, row.claim_token.as_deref().unwrap())
                    .unwrap()
            );
        }
    }

    #[test]
    fn non_head_waiter_cannot_admit_another_launch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "first", None, 1);
        enqueue(&mut db, "second", None, 2);

        assert_eq!(
            drain_with(&mut db, config(), roomy_memory, Some("second")).unwrap(),
            DrainOutcome::Waiting
        );
        assert_eq!(
            db.session_admissions().row("first").unwrap().unwrap().state,
            "queued"
        );
        assert_eq!(
            db.session_admissions()
                .row("second")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
        assert_eq!(
            drain_with(&mut db, config(), roomy_memory, Some("first")).unwrap(),
            DrainOutcome::Admitted
        );
        assert_eq!(
            drain_with(&mut db, config(), roomy_memory, Some("second")).unwrap(),
            DrainOutcome::LaunchMaterializing
        );
        let first = db.session_admissions().row("first").unwrap().unwrap();
        assert!(
            db.session_admissions()
                .settle("first", first.claim_token.as_deref().unwrap())
                .unwrap()
        );
        assert_eq!(
            drain_with(&mut db, config(), roomy_memory, Some("second")).unwrap(),
            DrainOutcome::Admitted
        );
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
    fn launching_reservation_blocks_only_until_its_runtime_is_running() {
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

        assert_eq!(
            db.session_admissions()
                .try_admit_next("blocked-token", i64::MAX, i64::MIN)
                .unwrap(),
            SessionAdmissionAttempt::LaunchMaterializing
        );

        let generation_id = RuntimeGenerationId::new();
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "first",
                session_id: None,
                runtime_mode: "headless",
                provider_name: "test-provider",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        assert_eq!(
            db.session_admissions()
                .try_admit_next("starting-token", i64::MAX, i64::MIN)
                .unwrap(),
            SessionAdmissionAttempt::LaunchMaterializing
        );
        let process_identity =
            oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
                .unwrap()
                .unwrap();
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "first",
        };
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence,
                spawned_os_pid: process_identity.os_pid,
                exact_process_identity: &process_identity,
                os_pgid: None,
            })
            .unwrap();

        let SessionAdmissionAttempt::Admitted(second) = db
            .session_admissions()
            .try_admit_next("replacement-token", i64::MAX, i64::MAX)
            .unwrap()
        else {
            panic!("a running parent must not impose an active-turn budget");
        };
        assert_eq!(second.registration_identity, "second");
        assert_eq!(
            db.session_admissions().row("first").unwrap().unwrap().state,
            "launching"
        );
    }

    #[test]
    fn running_generation_blocks_same_session_without_blocking_other_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pid-identity.db");
        let mut db = MailboxDb::open(&path).unwrap();
        enqueue(&mut db, "first", Some("session-a"), 1);
        enqueue(&mut db, "same-session", Some("session-a"), 2);
        enqueue(&mut db, "other-session", Some("session-b"), 3);
        drain_one_with(&mut db, config(), roomy_memory).unwrap();
        let first = db.session_admissions().row("first").unwrap().unwrap();
        assert!(
            db.session_admissions()
                .begin_launch("first", first.claim_token.as_deref().unwrap(), 4)
                .unwrap()
        );

        let generation_id = RuntimeGenerationId::new();
        db.runtime_lifecycle()
            .create_runtime_generation(CreateRuntimeGeneration {
                generation_id: &generation_id,
                spawn_invocation_uuid: "first",
                session_id: Some("session-a"),
                runtime_mode: "headless",
                provider_name: "test-provider",
                model_name: None,
                pty_control_path: None,
                models_dir: None,
                effective_cwd: None,
            })
            .unwrap();
        let process_identity =
            oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
                .unwrap()
                .unwrap();
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence: RuntimeGenerationFence {
                    generation_id: &generation_id,
                    spawn_invocation_uuid: "first",
                },
                spawned_os_pid: process_identity.os_pid,
                exact_process_identity: &process_identity,
                os_pgid: None,
            })
            .unwrap();

        let SessionAdmissionAttempt::Admitted(next) = db
            .session_admissions()
            .try_admit_next("next-token", i64::MAX, i64::MAX)
            .unwrap()
        else {
            panic!("a different session must remain launchable");
        };
        assert_eq!(next.registration_identity, "other-session");
        assert_eq!(
            db.session_admissions()
                .row("same-session")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
    }

    #[test]
    fn drain_cancels_one_dead_fifo_head_per_attempt() {
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
        db.session_admissions()
            .enqueue("dead-tail-admission", "dead-tail", None, &dead, 3)
            .unwrap();

        assert_eq!(
            drain_one_with(&mut db, config(), roomy_memory).unwrap(),
            DrainOutcome::Waiting
        );
        assert_eq!(
            db.session_admissions().row("dead").unwrap().unwrap().state,
            "cancelled"
        );
        assert_eq!(
            db.session_admissions().row("live").unwrap().unwrap().state,
            "queued"
        );
        assert_eq!(
            db.session_admissions()
                .row("dead-tail")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );

        assert_eq!(
            drain_one_with(&mut db, config(), roomy_memory).unwrap(),
            DrainOutcome::Admitted
        );
        assert_eq!(
            db.session_admissions().row("live").unwrap().unwrap().state,
            "admitted"
        );
        assert_eq!(
            db.session_admissions()
                .row("dead-tail")
                .unwrap()
                .unwrap()
                .state,
            "queued"
        );
    }

    #[test]
    fn queued_successor_reconciles_dead_fifo_head_without_independent_drainer() {
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
        drop(db);

        let (sender, receiver) = mpsc::channel();
        let waiter_path = path.clone();
        std::thread::spawn(move || {
            sender
                .send(enqueue_and_wait_at(&waiter_path, "live", None))
                .unwrap();
        });
        let guard = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("live successor did not reconcile the dead FIFO head")
            .unwrap();

        let mut db = MailboxDb::open(&path).unwrap();
        assert_eq!(
            db.session_admissions().row("dead").unwrap().unwrap().state,
            "cancelled"
        );
        assert_eq!(
            db.session_admissions().row("live").unwrap().unwrap().state,
            "launching"
        );
        drop(guard);
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
            DrainOutcome::LaunchMaterializing
        );
        let process_identity =
            oulipoly_state::pid_identity::read_live_process_identity(i64::from(std::process::id()))
                .unwrap()
                .unwrap();
        let fence = RuntimeGenerationFence {
            generation_id: &generation_id,
            spawn_invocation_uuid: "initial-invocation",
        };
        db.runtime_lifecycle()
            .bind_runtime_generation_running(BindRuntimeGenerationRunning {
                fence,
                spawned_os_pid: process_identity.os_pid,
                exact_process_identity: &process_identity,
                os_pgid: None,
            })
            .unwrap();
        assert_eq!(
            drain_one_with(&mut db, config(), roomy_memory).unwrap(),
            DrainOutcome::Admitted
        );

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
