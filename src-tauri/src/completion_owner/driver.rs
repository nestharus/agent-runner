//! The driver scans committed State obligations even when no sidecar source row
//! exists. Blocking recovery executes in separately retained/reaped custodians.
use oulipoly_state::mailbox::{CompletionDomainOwner, ContinuationAttempt, MailboxDb};
use oulipoly_state::{InvocationMutationAuthority, StateDb};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::{Duration, Instant};

pub(super) fn run(path: &Path, owner: &CompletionDomainOwner, election: i32) -> Result<(), String> {
    // Establish adoption before any attempt can fork. CG loss can promote this
    // exact driver while it retains its already-owned descendants.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut live: HashMap<i64, String> = HashMap::new();
    let mut next_retry: HashMap<String, Instant> = HashMap::new();
    loop {
        if i64::from(unsafe { libc::getppid() }) != owner.guardian_identity.pid {
            return super::linux::succeed_guardian(path, owner, election);
        }
        reap(&mut live);
        let current = MailboxDb::open(path)?
            .completion_continuation_owner()?
            .ok_or("driver authority disappeared")?;
        if current.owner_generation != owner.owner_generation {
            return Err("driver generation was replaced".into());
        }
        #[cfg(feature = "age360-fault-fixtures")]
        oulipoly_state::completion_continuation::age360_fault_barrier("driver-replay");
        for attempt in MailboxDb::open(path)?.pending_continuation_attempts()? {
            let _ = super::custody::replay_result(path, &attempt);
            // No concurrent source spawn runs in this driver. A still-reserved
            // source request therefore failed before acceptance; retry its exact
            // revocation if the first State write also failed. SQL rejects every
            // accepted/possibly launched or predecessor-owned attempt.
            if attempt.operation == "source_recovery" {
                let _ = MailboxDb::open(path)?.revoke_unaccepted_continuation_attempt(&attempt);
            }
        }
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
        let obligations = state.admitted_completion_continuations()?;
        for binding in obligations {
            let source = binding.registration()?;
            if source.domain_id != owner.domain_id {
                continue;
            }
            // Exact-authority repair is deliberately before any source file read:
            // losing the registration worker after State commit cannot hide it.
            if state
                .repair_admitted_completion_continuation(
                    InvocationMutationAuthority::Standalone,
                    &binding,
                )
                .is_err()
            {
                continue;
            }
            let already_accepted = MailboxDb::open(path)?
                .completion_continuation_acceptance(&source.registration_id)?
                .is_some_and(|v| v["phase"] == "accepted");
            if already_accepted {
                continue;
            }
            let snapshot = Path::new(&source.handle_dir).join(&source.snapshot_relative);
            let accepted =
                crate::commands::notify_continuation::accept(&binding, &snapshot).is_ok();
            if accepted {
                continue;
            }
            if live
                .values()
                .any(|registration| registration == &source.registration_id)
                || live.len() >= 4
                || next_retry
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
                Ok(pid) => {
                    live.insert(pid, source.registration_id.clone());
                }
                Err(_) => {
                    let _ = MailboxDb::open(path)?.revoke_unaccepted_continuation_attempt(&attempt);
                }
            }
            next_retry.insert(
                source.registration_id,
                Instant::now() + Duration::from_secs(1),
            );
        }
        drop(state);
        // Existing mailbox transports and admission resources remain in charge;
        // there is no second resume API and no synchronous wait for recipient ACK.
        let mut mailbox = MailboxDb::open(path)?;
        let sessions: BTreeSet<_> = mailbox
            .wake_sessions()
            .pending_delivery_session_ids(i64::MAX as usize)?
            .into_iter()
            .collect();
        for session in sessions {
            let delivery = crate::mailbox_delivery::attempt_pty_mailbox_delivery_with_trigger(
                &mut mailbox,
                &session,
                "completion-independent-owner",
            );
            if !delivery.submitted && delivery.status != "paused" {
                let _ = crate::wake_coordinator::trigger_notify_wake(&session);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn reap(live: &mut HashMap<i64, String>) {
    loop {
        let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
        if pid <= 0 {
            #[cfg(feature = "age360-fault-fixtures")]
            if pid < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                // Test observation only, never an attempt-discharge certificate.
                oulipoly_state::completion_continuation::age360_fault_barrier(
                    "driver-reaped-echild",
                );
            }
            break;
        }
        // Custodian exit is not converted to drain. Only its exact durable
        // ECHILD/integration receipt can discharge its own attempt.
        live.remove(&i64::from(pid));
    }
}
