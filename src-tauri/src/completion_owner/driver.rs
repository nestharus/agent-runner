//! The legacy driver schedules committed State obligations even when no sidecar
//! source row exists. It proposes immutable reservations to the root supervisor;
//! it does not fork, reap, cancel, integrate terminal results, or succeed the
//! root. The selected v30 route currently refuses before scheduling because
//! bounded repair and wake still require broker-owned operations.
use oulipoly_kernel_broker::protocol::{self, StateRoute};
use oulipoly_state::StateDb;
use oulipoly_state::mailbox::{CompletionDomainOwner, ContinuationAttempt, MailboxDb};
use std::collections::{BTreeSet, HashMap};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) const DRIVER_ARG: &str = "__completion-driver-v1";
const STATE_REPAIR_SUFFIX_BATCH: usize = 64;
const SOURCE_RECOVERY_BATCH: usize = 16;
const SOURCE_RETRY_BACKOFF: Duration = Duration::from_secs(1);
const DRIVER_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub(super) fn entry() -> Result<(), String> {
    let path = std::env::args_os()
        .nth(2)
        .map(PathBuf::from)
        .ok_or("completion driver path missing")?;
    let fd: RawFd = std::env::args()
        .nth(3)
        .ok_or("completion driver channel missing")?
        .parse()
        .map_err(|_| "completion driver channel is invalid")?;
    if fd < 0 {
        return Err("completion driver channel is invalid".into());
    }
    // The guardian deliberately execs the replacement driver so no mutex,
    // allocator, recorder, control-thread, or SQLite state from its
    // multi-threaded process can survive the fork boundary.
    let mut channel = unsafe { UnixStream::from_raw_fd(fd) };
    let owner = super::linux::read_driver_owner(&mut channel)?;
    unsafe { std::env::set_var(super::ENDPOINT_ENV, &owner.endpoint) };
    // A held-J v30 guardian supplies the root selector. The broker still
    // authenticates this process and derives the source from I; argv is never
    // owner or storage authority. Legacy guardians omit this argument.
    let root_id = std::env::args().nth(4);
    run_with_root(&path, &owner, channel, root_id.as_deref())
}

#[cfg(test)]
pub(super) fn run(
    path: &Path,
    owner: &CompletionDomainOwner,
    launch_channel: UnixStream,
) -> Result<(), String> {
    run_with_root(path, owner, launch_channel, None)
}

fn run_with_root(
    path: &Path,
    owner: &CompletionDomainOwner,
    launch_channel: UnixStream,
    root_id: Option<&str>,
) -> Result<(), String> {
    super::root_supervisor::install_driver_channel(launch_channel);
    let result = (|| match root_id {
        Some(root_id) => match protocol::state_route_at(&super::linux::owner_broker_socket())
            .map_err(|error| format!("completion driver broker route unavailable: {error}"))?
        {
            StateRoute::Legacy => Err("v30 driver root selector has no broker-owned source".into()),
            StateRoute::BrokerOwned { .. } => {
                let route = super::broker_route::V30OwnerRoute::driver(
                    &super::linux::owner_broker_socket(),
                    root_id,
                    owner,
                )?;
                route.read_running(owner, None)?;
                #[cfg(feature = "age319-private-broker-fixture")]
                if std::env::var_os("AGE319_PRIVATE_EXEC_DRIVER_ROUTE_V30").is_some() {
                    let gate_dir = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                        .ok_or("private exec driver gate directory missing")?;
                    let attested = Path::new(&gate_dir).join("child-attested");
                    let deadline = Instant::now() + Duration::from_secs(20);
                    while !attested.exists() {
                        if Instant::now() >= deadline {
                            return Err("private exec driver child attestation timed out".into());
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                Err("v30 driver bounded State repair and wake route is not available".into())
            }
        },
        None => run_owned(path, owner),
    })();
    // Preserve the original failure, but not at the price of discarding its
    // uniquely capable witness. Only evidence integration continues on this cut.
    #[cfg(test)]
    super::custody::finish_original_testimony();
    #[cfg(test)]
    super::root_supervisor::clear_driver_channel();
    result
}

fn run_owned(path: &Path, owner: &CompletionDomainOwner) -> Result<(), String> {
    let mut next_retry: HashMap<String, Instant> = HashMap::new();
    let mut pending_proposal_withdrawals: Vec<ContinuationAttempt> = Vec::new();
    loop {
        if i64::from(unsafe { libc::getppid() }) != owner.guardian_identity.pid {
            return Err("completion_root_supervisor_lost: driver parent changed".into());
        }
        let current = MailboxDb::open(path)?
            .completion_continuation_owner()?
            .ok_or("driver authority disappeared")?;
        if current.owner_generation != owner.owner_generation {
            return Err("driver generation was replaced".into());
        }
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("driver-replay");
        if !pending_proposal_withdrawals.is_empty() {
            // Retry only proposals this exact driver observed failing before a
            // launch response. Never rescan every unresolved root obligation:
            // accepted/unknown custody belongs exclusively to the root
            // supervisor's bounded reconciler.
            let mut mailbox = MailboxDb::open(path)?;
            pending_proposal_withdrawals.retain(|attempt| {
                mailbox
                    .try_revoke_unaccepted_continuation_attempt(attempt)
                    .is_err()
            });
        }
        let now = Instant::now();
        next_retry.retain(|_, retry_at| *retry_at > now);
        let mut state = StateDb::open_default()?;
        // Runtime/channel receipts come from the original allocated executor;
        // physical activation drain alone cannot settle a logical cancellation.
        for (generation, invocation) in state.cancelling_native_attempts()? {
            if MailboxDb::open(path)?.continuation_domain_drained_runtime(
                &owner.domain_id,
                &generation.to_string(),
                &invocation.to_string(),
            )? && let Err(error) =
                oulipoly_runtime::executor::settle_retained_native_cancellation(
                    &state, generation, invocation,
                )
            {
                let error_path = path
                    .with_extension("native-recovery-errors")
                    .join(format!("{generation}.txt"));
                if std::fs::read_to_string(&error_path).ok().as_deref() != Some(&error) {
                    let _ = super::custody::durable_write(&error_path, error.as_bytes());
                }
            }
        }
        // The sidecar continuity head is the durable cursor. Repair only its
        // bounded append-only State suffix, then schedule only source rows that
        // remain unaccepted. Accepted historical bindings never enter this hot
        // pass; explicit audit/retirement retains the full-ledger validation.
        let obligations = state.repair_pending_domain_completion_continuations(
            &owner.domain_id,
            &owner.supervisor_authority_id,
            STATE_REPAIR_SUFFIX_BATCH,
            SOURCE_RECOVERY_BATCH,
        )?;
        for binding in obligations {
            let source = binding.registration()?;
            let already_accepted = MailboxDb::open(path)?
                .completion_continuation_acceptance(&source.registration_id)?
                .is_some_and(|v| v["phase"] == "accepted");
            if already_accepted {
                continue;
            }
            let snapshot = Path::new(&source.handle_dir).join(&source.snapshot_relative);
            // This exact unaccepted source was selected from the current root's
            // bounded durable scope. An absent snapshot cannot be accepted.
            // Presence is a hint, never acceptance or workload replay authority.
            let accepted = snapshot.is_file()
                && crate::commands::notify_continuation::accept(&binding, &snapshot).is_ok();
            if accepted {
                continue;
            }
            if next_retry
                .get(&source.registration_id)
                .is_some_and(|when| *when > Instant::now())
            {
                continue;
            }
            let attempt_id = uuid::Uuid::new_v4().to_string();
            let result = oulipoly_state::paths::data_dir()?
                .join("completion-continuation")
                .join(&owner.domain_id)
                .join("attempts")
                .join(&attempt_id)
                .join("result.json");
            let attempt = ContinuationAttempt {
                attempt_id,
                owner_generation: owner.owner_generation.clone(),
                operation: "source_recovery".into(),
                request_sha256: binding.registration_digest().into(),
                source_registration_id: Some(source.registration_id.clone()),
                source_listener_revision: Some(source.listener_revision as i64),
                session_id: None,
                claim_token: None,
                result_path: result.to_string_lossy().into_owned(),
            };
            if MailboxDb::open(path)?
                .reserve_continuation_attempt(&attempt)
                .is_err()
            {
                continue; // continuing authoritative attempts, including predecessors, own the bound
            }
            match super::custody::spawn_source(path, &attempt, &binding) {
                Ok(_) => {}
                Err(_) => {
                    match MailboxDb::open(path).and_then(|mut mailbox| {
                        mailbox.try_revoke_unaccepted_continuation_attempt(&attempt)
                    }) {
                        Ok(_) => {}
                        Err(_) => pending_proposal_withdrawals.push(attempt.clone()),
                    }
                }
            }
            next_retry.insert(
                source.registration_id,
                Instant::now() + SOURCE_RETRY_BACKOFF,
            );
        }
        drop(state);
        // Existing mailbox transports and admission resources remain in charge;
        // there is no second resume API and no synchronous wait for recipient ACK.
        let mut mailbox = MailboxDb::open(path)?;
        let sessions: BTreeSet<_> = mailbox
            .wake_sessions()
            .pending_delivery_session_ids(
                crate::wake_coordinator::constants::WAKE_RECLAIM_SWEEP_SCAN_LIMIT,
            )?
            .into_iter()
            .collect();
        for session in sessions {
            let delivery = crate::mailbox_delivery::attempt_pty_mailbox_delivery_with_trigger(
                &mut mailbox,
                &session,
                "completion-independent-owner",
            );
            if !delivery.submitted && delivery.status != "paused" {
                let diagnostic = crate::wake_coordinator::trigger_notify_wake(&session);
                tracing::debug!(
                    session,
                    status = diagnostic.status,
                    "independent wake selection"
                );
                #[cfg(feature = "age360-fault-fixtures")]
                if diagnostic.status == "runtime_unavailable" {
                    oulipoly_state::completion_continuation::age360_fault_barrier(
                        "native-runtime-unavailable",
                    );
                }
            }
        }
        std::thread::sleep(DRIVER_POLL_INTERVAL);
    }
}
