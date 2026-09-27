//! Consumed-H registration through the sealed Runner helper. Both original
//! descriptors stay open until the Broker decision is consumed in State.

use oulipoly_kernel_broker::protocol::{
    self, ExactSourceDecisionRequest, ExactSourceDecisionVerification, ExactSourceIssuerKind,
    OwnerDiscoveryReadback, OwnerWitness, ProcessWitness, SourceWitnessProbe,
};
use oulipoly_state::completion_continuation::{AdmittedSourceBinding, SourceProcessIdentity};
use oulipoly_state::{
    CompletionRegistrationAuthority, ExactSourceAdmissionResult, ExactSourceDecisionReference,
    InvocationMutationAuthority, SourceDecisionVerification, StateDb,
};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

fn stamp(identity: &SourceProcessIdentity) -> Result<ProcessWitness, String> {
    Ok(ProcessWitness {
        host_pid: i32::try_from(identity.pid).map_err(|e| e.to_string())?,
        boot_id: identity.boot_id.clone(),
        starttime_ticks: u64::try_from(identity.starttime_ticks).map_err(|e| e.to_string())?,
    })
}

pub(super) fn register(
    binding: &AdmittedSourceBinding,
    registration_fd: &File,
    registration_path: &Path,
    accepted_intent: &Path,
    discovery: &OwnerDiscoveryReadback,
    authority: &CompletionRegistrationAuthority,
) -> Result<(ExactSourceAdmissionResult, String), String> {
    let source = binding.registration()?;
    let work_id = std::env::var("AGENT_BASH_OWNER_WORK_ID_V1")
        .map_err(|_| "fresh v30 consumed work ID absent".to_string())?;
    if discovery.owner.domain_id != source.domain_id {
        return Err("fresh v30 owner domain changed".into());
    }
    let capability = authority.process_environment_value().to_owned();
    let guardian = UnixStream::connect(&discovery.owner.endpoint).map_err(|e| e.to_string())?;
    let owner = OwnerWitness {
        root_id: discovery.root_id.clone(),
        domain_id: discovery.owner.domain_id.clone(),
        supervisor_id: discovery.owner.supervisor_authority_id.clone(),
        guardian: stamp(&discovery.owner.guardian_identity)?,
        driver: stamp(&discovery.owner.driver_identity)?,
        owner_generation: Some(discovery.owner.owner_generation.clone()),
        work_id: Some(work_id),
        owner_session_id: Some(source.owner_session_id.clone()),
        owner_invocation_uuid: Some(source.owner_invocation_uuid.clone()),
        registration_authority_sha256: Some(format!("{:x}", Sha256::digest(capability.as_bytes()))),
    };
    let witness = SourceWitnessProbe {
        owner,
        source_generation: discovery.source_generation.clone(),
        owner_generation: discovery.owner.owner_generation.clone(),
        registration_path: registration_path.to_path_buf(),
        registration_len: u64::try_from(binding.registration_bytes().len())
            .map_err(|e| e.to_string())?,
        registration_sha256: format!("{:x}", Sha256::digest(binding.registration_bytes())),
        owner_session_id: source.owner_session_id.clone(),
        owner_invocation_uuid: source.owner_invocation_uuid.clone(),
        capability,
        accepted_intent_path: Some(accepted_intent.to_path_buf()),
    };
    let request = ExactSourceDecisionRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        witness,
    };
    let broker = crate::completion_owner::v30_broker_socket();
    #[cfg(feature = "age319-private-broker-fixture")]
    let private_gate = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
        .map(std::path::PathBuf::from)
        .filter(|gate| gate.join("source-production-registration-cli").exists());
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_gate.is_some() {
        let mut direct = StateDb::open_existing(&StateDb::default_path()?)?;
        let bypass = direct
            .register_completion_continuation_with_authority(
                InvocationMutationAuthority::Standalone,
                authority,
                binding,
            )
            .unwrap_err();
        if !bypass.contains("fresh v30 exact Broker decision required") {
            return Err(format!(
                "fresh v30 public API bypass refusal changed: {bypass}"
            ));
        }
        drop(direct);
        private_refusal_probes(&broker, &request, &guardian, registration_fd, binding)?;
        protocol::issue_exact_source_decision_drop_reply_at(
            &broker,
            &request,
            guardian.as_raw_fd(),
            registration_fd.as_raw_fd(),
        )
        .map_err(|e| format!("fresh v30 private lost decision reply: {e}"))?;
    }
    let issue = || {
        protocol::issue_exact_source_decision_at(
            &broker,
            &request,
            guardian.as_raw_fd(),
            registration_fd.as_raw_fd(),
        )
    };
    // A lost reply cannot change the request. Any second refusal remains a
    // refusal; no path from here enters public/rootless registration.
    let decision = issue().or_else(|first| {
        issue().map_err(|second| {
            format!("fresh v30 Broker decision refused or uncertain: {first}; retry: {second}")
        })
    })?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_gate.is_some() && issue().map_err(|e| e.to_string())? != decision {
        return Err("fresh v30 exact Broker request retry changed".into());
    }
    let verification = ExactSourceDecisionVerification {
        request_id: request.request_id.clone(),
        decision_id: decision.decision_id.clone(),
        witness: request.witness.clone(),
        committed_retry: false,
    };
    let readback = protocol::verify_exact_source_decision_at(
        &broker,
        &verification,
        guardian.as_raw_fd(),
        registration_fd.as_raw_fd(),
    )
    .map_err(|e| format!("fresh v30 Broker decision readback: {e}"))?;
    let worker = stamp(&source.registering_caller)?;
    if readback != decision
        || readback.issuer_kind != ExactSourceIssuerKind::ConsumedHSealedHelper
        || readback.registration_worker.host_pid != worker.host_pid
        || readback.registration_worker.boot_id != worker.boot_id
        || readback.registration_worker.starttime_ticks != worker.starttime_ticks
        || readback.issuer.host_pid == worker.host_pid
    {
        return Err("fresh v30 Broker decision actor/readback conflict".into());
    }
    let state_verification = SourceDecisionVerification {
        request_id: request.request_id.clone(),
        decision_id: decision.decision_id,
        witness: serde_json::to_value(&request.witness).map_err(|e| e.to_string())?,
        committed_retry: false,
    };
    let submit = || -> Result<ExactSourceAdmissionResult, String> {
        let mut state = StateDb::open_existing(&StateDb::default_path()?)?;
        let reference = ExactSourceDecisionReference {
            verification: &state_verification,
            guardian_fd: guardian.as_raw_fd(),
            registration_fd: registration_fd.as_raw_fd(),
        };
        #[cfg(feature = "age319-private-broker-fixture")]
        {
            state.register_completion_continuation_with_broker_decision_at(
                &broker,
                InvocationMutationAuthority::Standalone,
                authority,
                binding,
                reference,
            )
        }
        #[cfg(not(feature = "age319-private-broker-fixture"))]
        {
            state.register_completion_continuation_with_broker_decision(
                InvocationMutationAuthority::Standalone,
                authority,
                binding,
                reference,
            )
        }
    };
    let first = submit().or_else(|first| {
        submit().map_err(|second| {
            format!("fresh v30 State admission refused or uncertain: {first}; retry: {second}")
        })
    })?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if let Some(gate) = private_gate {
        let retry = submit()?;
        if !first.inserted
            || retry.inserted
            || retry.decision_id != first.decision_id
            || first.projection_available
            || retry.projection_available
        {
            return Err("fresh v30 exact State retry changed admission".into());
        }
        std::fs::write(gate.join("source-production-probes.json"),
            serde_json::to_vec(&serde_json::json!({
            "broker_exact_retry": true, "state_exact_retry": true, "public_api_bypass_refused": true,
            "refusals": ["other-root", "stale-owner", "stale-guardian", "wrong-guardian", "copied-fd", "wrong-capability"]
            })).map_err(|e| e.to_string())?,
        ).map_err(|e| e.to_string())?;
    }
    Ok((first, request.request_id))
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_refusal_probes(
    broker: &Path,
    request: &ExactSourceDecisionRequest,
    guardian: &UnixStream,
    registration_fd: &File,
    binding: &AdmittedSourceBinding,
) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let issue = |request: &ExactSourceDecisionRequest, guardian_fd: i32, file_fd: i32| {
        protocol::issue_exact_source_decision_at(broker, request, guardian_fd, file_fd)
    };
    let mut other_root = request.clone();
    other_root.witness.owner.root_id = uuid::Uuid::new_v4().to_string();
    if issue(
        &other_root,
        guardian.as_raw_fd(),
        registration_fd.as_raw_fd(),
    )
    .is_ok()
    {
        return Err("fresh v30 other root issued a decision".into());
    }
    let mut stale_owner = request.clone();
    stale_owner.witness.owner_generation = uuid::Uuid::new_v4().to_string();
    if issue(
        &stale_owner,
        guardian.as_raw_fd(),
        registration_fd.as_raw_fd(),
    )
    .is_ok()
    {
        return Err("fresh v30 stale owner issued a decision".into());
    }
    let mut stale_guardian = request.clone();
    stale_guardian.witness.owner.guardian.starttime_ticks += 1;
    if issue(
        &stale_guardian,
        guardian.as_raw_fd(),
        registration_fd.as_raw_fd(),
    )
    .is_ok()
    {
        return Err("fresh v30 stale guardian issued a decision".into());
    }
    let (wrong_guardian, _peer) = UnixStream::pair().map_err(|e| e.to_string())?;
    if issue(
        request,
        wrong_guardian.as_raw_fd(),
        registration_fd.as_raw_fd(),
    )
    .is_ok()
    {
        return Err("fresh v30 disconnected guardian issued a decision".into());
    }
    let copied_path = request
        .witness
        .registration_path
        .with_file_name("source-registration-v30-copied-probe.json");
    std::fs::write(&copied_path, binding.registration_bytes()).map_err(|e| e.to_string())?;
    let copied = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&copied_path)
        .map_err(|e| e.to_string())?;
    if issue(request, guardian.as_raw_fd(), copied.as_raw_fd()).is_ok() {
        return Err("fresh v30 copied registration FD issued a decision".into());
    }
    let mut wrong_capability = request.clone();
    wrong_capability.witness.capability = "0".repeat(64);
    if issue(
        &wrong_capability,
        guardian.as_raw_fd(),
        registration_fd.as_raw_fd(),
    )
    .is_ok()
    {
        return Err("fresh v30 wrong capability issued a decision".into());
    }
    Ok(())
}
