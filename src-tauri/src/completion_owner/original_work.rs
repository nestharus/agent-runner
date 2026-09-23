//! Original-work-v1 authority hosted by the existing completion guardian.
//!
//! Durable intent lives in the exact agent-bash handle directory before this
//! module can accept a request. Acceptance is exclusive and effects-possible
//! acceptance is never replayed. The unpinned guardian owns legacy worker
//! custody; the broker-pinned guardian delegates its one accepted launch and
//! physical drain observation to the host broker.

use super::linux::identity;
use oulipoly_kernel_broker::protocol::{self, AcceptedWorkSpec, LaunchAcceptedWorkSpec};
use oulipoly_state::completion_continuation::SourceProcessIdentity;
use oulipoly_state::diagnostic_recorder::{
    DiagnosticPhase, PhaseObservation, SpanStart, process_recorder,
};
use oulipoly_state::mailbox::CompletionDomainOwner;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(test)]
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const PROTOCOL: &str = "original-work-v1";
pub(super) const ROOT_PROTOCOL: &str = "root-authority-v1";
pub(super) const SOURCE_CONTROL_PROTOCOL: &str = "source-control-v2";
const LEGACY_CONTROL_PROTOCOL: &str = "stream-control-v1";
pub(super) const EXECUTOR_ARG: &str = "__root-original-work-v1";
pub(super) const ACCEPTED_FILE: &str = "root-work-accepted-v1.json";
pub(super) const RESULT_FILE: &str = "root-work-result-v1.json";
const CANCEL_FILE: &str = "root-work-cancel-v1.json";
const BROKER_DRAIN_FILE: &str = "root-work-broker-drain-v1.json";
pub(super) const FD_COUNT: usize = 4;
const MAX_INTENT_BYTES: u64 = 1024 * 1024;
const RETRY_BASE: Duration = Duration::from_millis(50);
const RETRY_MAX: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct RootAuthorityGrant {
    pub protocol: String,
    #[serde(default = "legacy_control_protocol")]
    pub control_protocol: String,
    pub completion_protocol: String,
    pub domain_id: String,
    pub supervisor_authority_id: String,
    pub root_id: String,
    pub capability: String,
    pub root_identity: SourceProcessIdentity,
    pub guardian_identity: SourceProcessIdentity,
}

fn legacy_control_protocol() -> String {
    LEGACY_CONTROL_PROTOCOL.into()
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum RootJoinMode {
    Fresh,
    Inherit { grant: RootAuthorityGrant },
}

impl<'de> Deserialize<'de> for RootJoinMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut object = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let kind = object
            .remove("kind")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| serde::de::Error::custom("root join kind is required"))?;
        match kind.as_str() {
            "fresh" if object.is_empty() => Ok(Self::Fresh),
            "inherit" => {
                let grant = object
                    .remove("grant")
                    .ok_or_else(|| serde::de::Error::custom("inherited root grant is required"))?;
                if !object.is_empty() {
                    return Err(serde::de::Error::custom("unknown inherited root field"));
                }
                Ok(Self::Inherit {
                    grant: serde_json::from_value(grant).map_err(serde::de::Error::custom)?,
                })
            }
            "fresh" => Err(serde::de::Error::custom(
                "fresh root join has conflicting fields",
            )),
            _ => Err(serde::de::Error::custom("unknown root join kind")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RootJoinRequest {
    pub protocol: String,
    pub mode: RootJoinMode,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RootJoinResponse {
    pub owner: CompletionDomainOwner,
    pub root_authority: RootAuthorityGrant,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum WorkRegistration {
    Root,
    Nested {
        parent_work_id: String,
        parent_capability: String,
    },
}

impl<'de> Deserialize<'de> for WorkRegistration {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut object = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let kind = object
            .remove("kind")
            .and_then(|value| value.as_str().map(str::to_owned))
            .ok_or_else(|| serde::de::Error::custom("work registration kind is required"))?;
        match kind.as_str() {
            "root" if object.is_empty() => Ok(Self::Root),
            "nested" => {
                let parent_work_id = object
                    .remove("parent_work_id")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| serde::de::Error::custom("nested parent_work_id is required"))?;
                let parent_capability = object
                    .remove("parent_capability")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        serde::de::Error::custom("nested parent_capability is required")
                    })?;
                if !object.is_empty() {
                    return Err(serde::de::Error::custom(
                        "unknown nested work registration field",
                    ));
                }
                Ok(Self::Nested {
                    parent_work_id,
                    parent_capability,
                })
            }
            "root" => Err(serde::de::Error::custom(
                "root work registration has conflicting fields",
            )),
            _ => Err(serde::de::Error::custom("unknown work registration kind")),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkSubmission {
    pub protocol: String,
    pub root_authority: RootAuthorityGrant,
    pub work_id: String,
    pub request_sha256: String,
    pub registration: WorkRegistration,
    pub cancel_capability: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CancelSubmission {
    pub protocol: String,
    pub root_id: String,
    pub supervisor_authority_id: String,
    pub work_id: String,
    pub cancel_capability: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WorkResponse {
    pub(super) protocol: String,
    pub(super) work_id: String,
    pub(super) status: String,
    pub(super) root_id: String,
    pub(super) supervisor_authority_id: String,
    pub(super) worker_identity: Option<SourceProcessIdentity>,
    pub(super) detail: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IntentIdentity {
    protocol: String,
    work_id: String,
    root_id: String,
    handle: String,
    state_root: std::path::PathBuf,
    meta: IntentMetaIdentity,
    #[serde(default)]
    cancel_owner: Option<SourceProcessIdentity>,
}

#[derive(Debug, Deserialize)]
struct IntentMetaIdentity {
    cwd: std::path::PathBuf,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct AcceptanceReceipt {
    protocol: String,
    work_id: String,
    request_sha256: String,
    root_id: String,
    supervisor_authority_id: String,
    owner_generation: String,
    initiator: SourceProcessIdentity,
    registration: RegistrationReceipt,
    cancel_capability_sha256: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RegistrationReceipt {
    Root,
    Nested {
        parent_work_id: String,
        parent_capability_sha256: String,
    },
}

impl From<&WorkRegistration> for RegistrationReceipt {
    fn from(value: &WorkRegistration) -> Self {
        match value {
            WorkRegistration::Root => Self::Root,
            WorkRegistration::Nested {
                parent_work_id,
                parent_capability,
            } => Self::Nested {
                parent_work_id: parent_work_id.clone(),
                parent_capability_sha256: hex_digest(parent_capability.as_bytes()),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct CancellationReceipt {
    protocol: String,
    work_id: String,
    root_id: String,
    cause: String,
    requester: Option<SourceProcessIdentity>,
}

#[derive(Debug, Serialize)]
struct ResultReceipt<'a> {
    protocol: &'a str,
    work_id: &'a str,
    root_id: &'a str,
    result_nonce: &'a str,
    outcome: &'a str,
    causal_children: &'a BTreeSet<String>,
    physical_tree_drained: bool,
    worker_wait_status: Option<i32>,
    agent_state: Option<&'a str>,
    agent_rc: Option<i32>,
    agent_signal: Option<i32>,
    completion_reason: Option<&'a str>,
    detail: Option<&'a str>,
}

#[derive(Serialize)]
struct BrokerDrainReceipt<'a> {
    protocol: &'a str,
    root_id: &'a str,
    work_id: &'a str,
    grant_id: &'a str,
    work_incarnation: &'a str,
    worker_local_pid: i32,
    worker_wait_status: i32,
    physical_tree_drained: bool,
}

#[derive(Default, Deserialize)]
struct TerminalSnapshot {
    state: Option<String>,
    rc: Option<i32>,
    signal: Option<i32>,
    completion_reason: Option<String>,
}

pub(super) struct InboundWork {
    pub socket: UnixStream,
    pub peer: SourceProcessIdentity,
    pub submission: WorkSubmission,
    pub descriptors: [OwnedFd; FD_COUNT],
}

pub(super) struct InboundCancel {
    pub socket: UnixStream,
    pub peer: SourceProcessIdentity,
    pub submission: CancelSubmission,
}

struct RootScope {
    grant: RootAuthorityGrant,
    capability_hash: [u8; 32],
    contexts: Vec<SourceProcessIdentity>,
}

#[derive(Default)]
pub(super) struct RootAuthorities {
    scopes: BTreeMap<String, RootScope>,
}

impl RootAuthorities {
    pub fn fresh(
        &mut self,
        owner: &CompletionDomainOwner,
        context: SourceProcessIdentity,
    ) -> Result<RootAuthorityGrant, String> {
        self.fresh_with_protocol(
            owner,
            context,
            uuid::Uuid::new_v4().to_string(),
            LEGACY_CONTROL_PROTOCOL,
        )
    }

    pub fn fresh_with_root_id(
        &mut self,
        owner: &CompletionDomainOwner,
        context: SourceProcessIdentity,
        root_id: String,
    ) -> Result<RootAuthorityGrant, String> {
        self.fresh_with_protocol(owner, context, root_id, SOURCE_CONTROL_PROTOCOL)
    }

    fn fresh_with_protocol(
        &mut self,
        owner: &CompletionDomainOwner,
        context: SourceProcessIdentity,
        root_id: String,
        control_protocol: &str,
    ) -> Result<RootAuthorityGrant, String> {
        uuid::Uuid::parse_str(&root_id).map_err(|_| "invalid broker root ID")?;
        if self.scopes.contains_key(&root_id) {
            return Err("duplicate root authority ID".into());
        }
        if !self.roots_for_peer(&context).is_empty() {
            return Err("live context is already inside a root authority".into());
        }
        let capability = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let grant = RootAuthorityGrant {
            protocol: ROOT_PROTOCOL.into(),
            control_protocol: control_protocol.into(),
            completion_protocol: owner.protocol.clone(),
            domain_id: owner.domain_id.clone(),
            supervisor_authority_id: owner.supervisor_authority_id.clone(),
            root_id,
            capability: capability.clone(),
            root_identity: context.clone(),
            guardian_identity: owner.guardian_identity.clone(),
        };
        self.scopes.insert(
            grant.root_id.clone(),
            RootScope {
                capability_hash: digest(capability.as_bytes()),
                grant: grant.clone(),
                contexts: vec![context],
            },
        );
        Ok(grant)
    }

    pub fn inherit(
        &mut self,
        owner: &CompletionDomainOwner,
        context: SourceProcessIdentity,
        grant: &RootAuthorityGrant,
    ) -> Result<RootAuthorityGrant, String> {
        let scope = self.scope(owner, grant)?;
        if !scope
            .contexts
            .iter()
            .any(|ancestor| exact_descendant(&context, ancestor))
        {
            return Err("root authority inheritance is outside the exact process tree".into());
        }
        let answer = scope.grant.clone();
        if !scope.contexts.contains(&context) {
            scope.contexts.push(context);
        }
        Ok(answer)
    }

    pub fn validate_inherit(
        &mut self,
        owner: &CompletionDomainOwner,
        context: &SourceProcessIdentity,
        grant: &RootAuthorityGrant,
    ) -> Result<(), String> {
        let scope = self.scope(owner, grant)?;
        scope
            .contexts
            .iter()
            .any(|ancestor| exact_descendant(context, ancestor))
            .then_some(())
            .ok_or_else(|| "root authority inheritance is outside the exact process tree".into())
    }

    pub fn authorize_root(
        &mut self,
        owner: &CompletionDomainOwner,
        peer: &SourceProcessIdentity,
        grant: &RootAuthorityGrant,
    ) -> Result<(), String> {
        let scope = self.scope(owner, grant)?;
        if !scope
            .contexts
            .iter()
            .any(|ancestor| exact_descendant(peer, ancestor))
        {
            return Err("original root request is outside its live root context".into());
        }
        Ok(())
    }

    pub fn authorize_capability(
        &mut self,
        owner: &CompletionDomainOwner,
        grant: &RootAuthorityGrant,
    ) -> Result<(), String> {
        self.scope(owner, grant).map(|_| ())
    }

    pub fn roots_for_peer(&self, peer: &SourceProcessIdentity) -> BTreeSet<String> {
        self.scopes
            .iter()
            .filter(|(_, scope)| {
                scope
                    .contexts
                    .iter()
                    .any(|ancestor| exact_descendant(peer, ancestor))
            })
            .map(|(root_id, _)| root_id.clone())
            .collect()
    }

    pub fn retain_live(
        &mut self,
        contexts: &[SourceProcessIdentity],
        active_roots: &BTreeSet<String>,
    ) {
        self.scopes.retain(|root_id, scope| {
            scope.contexts.retain(|context| contexts.contains(context));
            !scope.contexts.is_empty() || active_roots.contains(root_id)
        });
    }

    pub fn inherit_from_active_tree(
        &mut self,
        owner: &CompletionDomainOwner,
        context: SourceProcessIdentity,
        grant: &RootAuthorityGrant,
    ) -> Result<RootAuthorityGrant, String> {
        let scope = self.scope(owner, grant)?;
        let answer = scope.grant.clone();
        if !scope.contexts.contains(&context) {
            scope.contexts.push(context);
        }
        Ok(answer)
    }

    fn scope(
        &mut self,
        owner: &CompletionDomainOwner,
        grant: &RootAuthorityGrant,
    ) -> Result<&mut RootScope, String> {
        if grant.protocol != ROOT_PROTOCOL
            || grant.completion_protocol != owner.protocol
            || grant.domain_id != owner.domain_id
            || grant.supervisor_authority_id != owner.supervisor_authority_id
            || grant.guardian_identity != owner.guardian_identity
        {
            return Err("root capability protocol or authority mismatch".into());
        }
        let scope = self
            .scopes
            .get_mut(&grant.root_id)
            .ok_or("unknown root capability")?;
        if scope.grant.control_protocol != grant.control_protocol
            || scope.capability_hash != digest(grant.capability.as_bytes())
            || scope.grant.root_identity != grant.root_identity
        {
            return Err("root capability mismatch".into());
        }
        Ok(scope)
    }
}

struct OriginalOperation {
    submission: WorkSubmission,
    intent: IntentIdentity,
    worker: Option<Child>,
    /// Present only for the one-use broker launch. This is retained even when
    /// K's response is lost: Q may resolve it, but K is never retried.
    kernel_grant: Option<String>,
    kernel_incarnation: Option<String>,
    kernel_worker_local_pid: Option<i32>,
    kernel_drain_recorded: bool,
    worker_identity: Option<SourceProcessIdentity>,
    worker_session: i64,
    control: UnixStream,
    acceptance_response: Option<UnixStream>,
    control_input: Vec<u8>,
    control_closed: bool,
    prepared_for_grant: bool,
    grant_sent: bool,
    state_dir: OwnedFd,
    result_nonce: String,
    child_capability_hash: [u8; 32],
    cancel_capability_hash: [u8; 32],
    children: BTreeSet<String>,
    admission_closed: bool,
    settlement_sent: bool,
    causal_terminal: bool,
    terminal_observed: bool,
    worker_wait_status: Option<i32>,
    cancellation_sent: bool,
    broker_cancel_sent: bool,
    broker_cancel_retry: RetrySchedule,
    session_signal_sent: bool,
    cancellation_cause: Option<String>,
    cancellation_requester: Option<SourceProcessIdentity>,
    cancellation_pending: Option<PendingCancellation>,
    cancellation_propagated: bool,
    cancellation_receipt_error: Option<String>,
    detail: Option<String>,
    session_drained: bool,
    drain_retry: RetrySchedule,
    result_written: bool,
    result_failure_recorded: bool,
    result_retry: RetrySchedule,
}

struct NeverForkedOperation {
    submission: WorkSubmission,
    intent: IntentIdentity,
    state_dir: OwnedFd,
    result_nonce: String,
    detail: String,
    handle_terminalized: bool,
    result_written: bool,
    result_failure_recorded: bool,
    result_retry: RetrySchedule,
}

#[derive(Clone)]
struct PendingCancellation {
    cause: String,
    requester: Option<SourceProcessIdentity>,
    retry: RetrySchedule,
}

#[derive(Clone)]
struct RetrySchedule {
    failures: u32,
    retry_at: Instant,
}

impl Default for RetrySchedule {
    fn default() -> Self {
        Self {
            failures: 0,
            retry_at: Instant::now(),
        }
    }
}

impl RetrySchedule {
    fn due(&self, now: Instant) -> bool {
        now >= self.retry_at
    }

    fn record_failure(&mut self, now: Instant) {
        let shift = self.failures.min(7);
        let multiplier = 1_u32 << shift;
        let delay = RETRY_BASE.saturating_mul(multiplier).min(RETRY_MAX);
        self.failures = self.failures.saturating_add(1);
        self.retry_at = now + delay;
    }
}

#[derive(Default)]
pub(super) struct OriginalWorkSupervisor {
    active: Vec<OriginalOperation>,
    never_forked: Vec<NeverForkedOperation>,
    adopted_child_live: bool,
    kernel_pinned: bool,
}

impl OriginalWorkSupervisor {
    pub fn set_kernel_pinned(&mut self, pinned: bool) {
        self.kernel_pinned = pinned;
    }
    pub fn set_adopted_child_gate(&mut self, live: bool) {
        self.adopted_child_live = live;
    }
    pub fn is_empty(&self) -> bool {
        self.active.is_empty() && self.never_forked.is_empty()
    }

    pub fn accepts_nested(
        &self,
        root_id: &str,
        parent_work_id: &str,
        parent_capability: &str,
        peer: &SourceProcessIdentity,
    ) -> bool {
        self.active.iter().any(|operation| {
            operation.submission.root_authority.root_id == root_id
                && operation.submission.work_id == parent_work_id
                && capability_matches(&operation.child_capability_hash, parent_capability)
                && !operation.admission_closed
                && operation.worker_identity.as_ref().is_some_and(|identity| {
                    operation.kernel_grant.is_some() || exact_descendant(peer, identity)
                })
        })
    }

    pub fn peer_in_root(&self, root_id: &str, peer: &SourceProcessIdentity) -> bool {
        self.active.iter().any(|operation| {
            operation.submission.root_authority.root_id == root_id
                && operation
                    .worker_identity
                    .as_ref()
                    .is_some_and(|identity| exact_descendant(peer, identity))
        })
    }

    pub fn parent_for_peer(&self, root_id: &str, peer: &SourceProcessIdentity) -> Option<&str> {
        self.active
            .iter()
            .find(|operation| {
                operation.submission.root_authority.root_id == root_id
                    && operation
                        .worker_identity
                        .as_ref()
                        .is_some_and(|identity| exact_descendant(peer, identity))
            })
            .map(|operation| operation.submission.work_id.as_str())
    }

    pub fn root_for_peer(&self, peer: &SourceProcessIdentity) -> Option<&str> {
        self.active.iter().find_map(|operation| {
            operation
                .worker_identity
                .as_ref()
                .is_some_and(|identity| exact_descendant(peer, identity))
                .then_some(operation.submission.root_authority.root_id.as_str())
        })
    }

    pub fn worker_pids(&self) -> Vec<i64> {
        self.active
            .iter()
            .filter(|operation| operation.worker.is_some())
            .map(|operation| operation.worker_session)
            .collect()
    }

    pub fn live_worker_pids(&self) -> Vec<i64> {
        self.active
            .iter()
            .filter(|operation| operation.worker.is_some())
            .filter(|operation| !operation.terminal_observed)
            .filter_map(|operation| operation.worker_identity.as_ref())
            .filter(|worker| identity(worker.pid).as_ref() == Ok(worker))
            .map(|worker| worker.pid)
            .collect()
    }

    pub fn active_root_ids(&self) -> BTreeSet<String> {
        self.active
            .iter()
            .map(|operation| operation.submission.root_authority.root_id.clone())
            .chain(
                self.never_forked
                    .iter()
                    .map(|operation| operation.submission.root_authority.root_id.clone()),
            )
            .collect()
    }

    pub fn submit(&mut self, owner: &CompletionDomainOwner, mut request: InboundWork) {
        let work_id = request.submission.work_id.clone();
        let root_id = request.submission.root_authority.root_id.clone();
        let start = SpanStart::new("root_original_work_submit", "process_tree")
            .with_lifecycle_phase("original_work_acceptance")
            .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
            .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
            .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
            .with_identifier(
                "authority_starttime_ticks",
                &owner.guardian_identity.starttime_ticks.to_string(),
            )
            .with_identifier("root_id", &root_id)
            .with_identifier("initiator_pid", &request.peer.pid.to_string())
            .with_identifier("initiator_boot_id", &request.peer.boot_id)
            .with_identifier(
                "initiator_starttime_ticks",
                &request.peer.starttime_ticks.to_string(),
            )
            .with_hashed_correlation("work_id", &work_id);
        process_recorder().with_requested_span(start, |span| {
            let mut effects_possible = false;
            let result = self.submit_inner(owner, &mut request, &mut effects_possible);
            let response = match result {
                Ok(response) => {
                    let _ = span.record(
                        DiagnosticPhase::Committed,
                        PhaseObservation::committed()
                            .with_cause("original_work_exclusively_accepted"),
                    );
                    response
                }
                Err(error) => {
                    let _ = span.record(
                        DiagnosticPhase::Failed,
                        (if effects_possible {
                            PhaseObservation::effects_possible()
                        } else {
                            PhaseObservation::not_started()
                        })
                        .with_cause("original_work_submit_failed")
                        .with_cause(&error),
                    );
                    Some(WorkResponse {
                        protocol: PROTOCOL.into(),
                        work_id,
                        status: submit_failure_status(effects_possible).into(),
                        root_id,
                        supervisor_authority_id: owner.supervisor_authority_id.clone(),
                        worker_identity: None,
                        detail: Some(error),
                    })
                }
            };
            if let Some(response) = response {
                send_response(&mut request.socket, &response);
            }
        });
    }

    fn submit_inner(
        &mut self,
        owner: &CompletionDomainOwner,
        request: &mut InboundWork,
        effects_possible: &mut bool,
    ) -> Result<Option<WorkResponse>, String> {
        if request.submission.protocol != PROTOCOL {
            return Err("unsupported original-work protocol".into());
        }
        if let Some(active) = self
            .active
            .iter()
            .find(|active| active.submission.work_id == request.submission.work_id)
        {
            if active.submission.root_authority.root_id != request.submission.root_authority.root_id
                || active.submission.request_sha256 != request.submission.request_sha256
                || active.submission.registration != request.submission.registration
            {
                return Err("duplicate work identity conflicts with the accepted request".into());
            }
            if !active.prepared_for_grant {
                return Ok(Some(WorkResponse {
                    protocol: PROTOCOL.into(),
                    work_id: request.submission.work_id.clone(),
                    status: "effects_possible_no_replay".into(),
                    root_id: request.submission.root_authority.root_id.clone(),
                    supervisor_authority_id: owner.supervisor_authority_id.clone(),
                    worker_identity: active.worker_identity.clone(),
                    detail: Some(
                        "work is exclusively accepted but execution preparation is not confirmed"
                            .into(),
                    ),
                }));
            }
            return Ok(Some(WorkResponse {
                protocol: PROTOCOL.into(),
                work_id: request.submission.work_id.clone(),
                status: "already_accepted_no_replay".into(),
                root_id: request.submission.root_authority.root_id.clone(),
                supervisor_authority_id: owner.supervisor_authority_id.clone(),
                worker_identity: active.worker_identity.clone(),
                detail: None,
            }));
        }
        let [executable, intent_fd, cwd, state_dir] =
            std::mem::replace(&mut request.descriptors, empty_descriptor_array()?);
        validate_descriptor(&executable, false, true)?;
        validate_descriptor(&cwd, true, false)?;
        validate_descriptor(&state_dir, true, false)?;
        validate_descriptor(&intent_fd, false, false)?;
        let intent_bytes = read_bounded(&intent_fd, MAX_INTENT_BYTES)?;
        if hex_digest(&intent_bytes) != request.submission.request_sha256 {
            return Err("original work intent digest mismatch".into());
        }
        let intent: IntentIdentity =
            serde_json::from_slice(&intent_bytes).map_err(|error| error.to_string())?;
        if intent.protocol != PROTOCOL
            || intent.work_id != request.submission.work_id
            || intent.root_id != request.submission.root_authority.root_id
            || intent.handle != request.submission.work_id
        {
            return Err("original work intent identity mismatch".into());
        }
        validate_bound_identity(
            &state_dir,
            &intent.state_root.join(&intent.handle),
            "state directory",
        )?;
        validate_bound_identity(&cwd, &intent.meta.cwd, "working directory")?;
        validate_intent_descriptor(&state_dir, &intent_fd)?;
        validate_peer_executable(&executable, &request.peer)?;

        // Prepare every fallible local launch resource before acceptance. The
        // process spawn itself is the sole effects-possible launch boundary.
        let (control, worker_control) = UnixStream::pair().map_err(|error| error.to_string())?;
        control
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let child_capability = new_capability();
        let capability_fd = create_capability_descriptor(&child_capability)?;
        let acceptance_response = request
            .socket
            .try_clone()
            .map_err(|error| error.to_string())?;

        let parent_index = match &request.submission.registration {
            WorkRegistration::Root => None,
            WorkRegistration::Nested { parent_work_id, .. } => {
                let index = self
                    .active
                    .iter()
                    .position(|active| {
                        active.submission.work_id == *parent_work_id
                            && active.submission.root_authority.root_id == intent.root_id
                    })
                    .ok_or("causal parent disappeared before child acceptance")?;
                if self.active[index].admission_closed {
                    return Err("causal parent admission is closed".into());
                }
                self.active[index].children.insert(intent.work_id.clone());
                Some(index)
            }
        };
        let acceptance = AcceptanceReceipt {
            protocol: PROTOCOL.into(),
            work_id: intent.work_id.clone(),
            request_sha256: request.submission.request_sha256.clone(),
            root_id: intent.root_id.clone(),
            supervisor_authority_id: owner.supervisor_authority_id.clone(),
            owner_generation: owner.owner_generation.clone(),
            initiator: request.peer.clone(),
            registration: RegistrationReceipt::from(&request.submission.registration),
            cancel_capability_sha256: hex_digest(request.submission.cancel_capability.as_bytes()),
        };
        match create_artifact(&state_dir, ACCEPTED_FILE, &acceptance) {
            Ok(()) => *effects_possible = true,
            Err(error) if error.starts_with("exists:") => {
                if let Some(index) = parent_index {
                    self.active[index].children.remove(&intent.work_id);
                }
                return Err(format!(
                    "original work was already accepted; effects are possible and replay is forbidden ({error})"
                ));
            }
            Err(error) => {
                if let Some(index) = parent_index {
                    self.active[index].children.remove(&intent.work_id);
                }
                return Err(error);
            }
        }
        if self.kernel_pinned {
            let prepared = prepare_kernel_grant(
                owner,
                &request.submission,
                &acceptance,
                &executable,
                &intent_fd,
                &cwd,
                &state_dir,
            );
            let grant = match prepared {
                Ok(grant) => grant,
                Err(error) => {
                    let detail = format!("kernel accepted grant preparation uncertain: {error}");
                    self.never_forked.push(NeverForkedOperation {
                        submission: request.submission.clone(),
                        intent,
                        state_dir,
                        result_nonce: new_capability(),
                        detail: detail.clone(),
                        handle_terminalized: false,
                        result_written: false,
                        result_failure_recorded: false,
                        result_retry: RetrySchedule::default(),
                    });
                    return Err(detail);
                }
            };
            // Persist the only Q handle before the irreversible K request.
            // Failure here means K was never sent. An H response lost before
            // this point cannot authorize a later launch by this guardian.
            if let Err(error) = create_artifact(
                &state_dir,
                "root-work-broker-grant-v1.json",
                &serde_json::json!({
                    "protocol": PROTOCOL,
                    "work_id": intent.work_id,
                    "request_sha256": request.submission.request_sha256,
                    "grant_id": grant,
                }),
            ) {
                let detail =
                    format!("kernel grant handle persistence failed before launch: {error}");
                self.never_forked.push(NeverForkedOperation {
                    submission: request.submission.clone(),
                    intent,
                    state_dir,
                    result_nonce: new_capability(),
                    detail: detail.clone(),
                    handle_terminalized: false,
                    result_written: false,
                    result_failure_recorded: false,
                    result_retry: RetrySchedule::default(),
                });
                return Err(detail);
            }
            let launch = launch_kernel_work(
                &grant,
                &executable,
                &intent_fd,
                &cwd,
                &state_dir,
                &worker_control,
                &capability_fd,
            );
            drop(worker_control);
            let (kernel_incarnation, worker_session, worker_identity, detail) = match launch {
                Ok((incarnation, worker_pid)) => {
                    let identity = identity(worker_pid).ok();
                    let detail = identity.is_none().then(|| {
                        "broker launched work but exact worker identity is unavailable".to_owned()
                    });
                    (Some(incarnation), worker_pid, identity, detail)
                }
                Err(error) => (
                    None,
                    0,
                    None,
                    Some(format!("kernel work launch uncertain: {error}")),
                ),
            };
            self.active.push(OriginalOperation {
                submission: request.submission.clone(),
                intent,
                worker: None,
                kernel_grant: Some(grant),
                kernel_incarnation,
                kernel_worker_local_pid: None,
                kernel_drain_recorded: false,
                worker_identity: worker_identity.clone(),
                worker_session,
                control,
                acceptance_response: worker_identity.is_some().then_some(acceptance_response),
                control_input: Vec::new(),
                control_closed: false,
                prepared_for_grant: false,
                grant_sent: false,
                state_dir,
                result_nonce: new_capability(),
                child_capability_hash: digest(child_capability.as_bytes()),
                cancel_capability_hash: digest(request.submission.cancel_capability.as_bytes()),
                children: BTreeSet::new(),
                admission_closed: false,
                settlement_sent: false,
                causal_terminal: false,
                terminal_observed: false,
                worker_wait_status: None,
                cancellation_sent: false,
                broker_cancel_sent: false,
                broker_cancel_retry: RetrySchedule::default(),
                session_signal_sent: false,
                cancellation_cause: None,
                cancellation_requester: None,
                cancellation_pending: None,
                cancellation_propagated: false,
                cancellation_receipt_error: None,
                detail: detail.clone(),
                session_drained: false,
                drain_retry: RetrySchedule::default(),
                result_written: false,
                result_failure_recorded: false,
                result_retry: RetrySchedule::default(),
            });
            if detail.is_some() {
                // K may have consumed its one-use grant even if its response
                // vanished. Never release G on an uncertain launch; request
                // work-specific cancellation and keep Q debt.
                self.stage_cancellation(
                    self.active.len() - 1,
                    "kernel_launch_uncertain",
                    None,
                    Instant::now(),
                );
            }
            return if let Some(detail) = detail {
                Err(detail)
            } else {
                Ok(None)
            };
        }
        let executable_fd = executable.as_raw_fd();
        let intent_raw = intent_fd.as_raw_fd();
        let cwd_fd = cwd.as_raw_fd();
        let state_dir_fd = state_dir.as_raw_fd();
        let control_fd = worker_control.as_raw_fd();
        let capability_raw = capability_fd.as_raw_fd();
        let mut command = Command::new(format!("/proc/self/fd/{executable_fd}"));
        command
            .arg(EXECUTOR_ARG)
            .arg(intent_raw.to_string())
            .arg(cwd_fd.to_string())
            .arg(state_dir_fd.to_string())
            .arg(control_fd.to_string())
            .arg(capability_raw.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let spawn = match super::custody::original_worker_lineage_name() {
            Ok(lineage_name) => {
                unsafe {
                    command.pre_exec(move || {
                        if libc::setsid() < 0 {
                            return Err(std::io::Error::last_os_error());
                        }
                        // Only the accepted child joins the paired ring. The
                        // guardian keeps its own ring for unrelated launches.
                        super::custody::establish_original_worker_lineage_pre_exec(&lineage_name)?;
                        // The executable stays CLOEXEC: the kernel resolves
                        // /proc/self/fd before closing it. Only the request boundary
                        // descriptors cross into the root-owned worker.
                        for fd in [intent_raw, cwd_fd, state_dir_fd, control_fd, capability_raw] {
                            let flags = libc::fcntl(fd, libc::F_GETFD);
                            if flags < 0
                                || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                            {
                                return Err(std::io::Error::last_os_error());
                            }
                        }
                        Ok(())
                    });
                }
                command.spawn()
            }
            Err(error) => Err(std::io::Error::other(error)),
        };
        let worker = match spawn {
            Ok(worker) => worker,
            Err(error) => {
                let detail = format!("root original worker spawn failed after acceptance: {error}");
                self.never_forked.push(NeverForkedOperation {
                    submission: request.submission.clone(),
                    intent,
                    state_dir,
                    result_nonce: new_capability(),
                    detail: detail.clone(),
                    handle_terminalized: false,
                    result_written: false,
                    result_failure_recorded: false,
                    result_retry: RetrySchedule::default(),
                });
                return Err(detail);
            }
        };
        drop(worker_control);
        drop(executable);
        drop(intent_fd);
        drop(cwd);
        let worker_session = i64::from(worker.id());
        let worker_identity = identity(worker_session).ok();
        let identity_error = worker_identity.is_none().then(|| {
            "root original worker identity unavailable after acceptance; cancellation and exact drain retained"
                .to_owned()
        });
        let operation = OriginalOperation {
            submission: request.submission.clone(),
            intent,
            worker: Some(worker),
            kernel_grant: None,
            kernel_incarnation: None,
            kernel_worker_local_pid: None,
            kernel_drain_recorded: false,
            worker_identity: worker_identity.clone(),
            worker_session,
            control,
            acceptance_response: identity_error.is_none().then_some(acceptance_response),
            control_input: Vec::new(),
            control_closed: false,
            prepared_for_grant: false,
            grant_sent: false,
            state_dir,
            result_nonce: new_capability(),
            child_capability_hash: digest(child_capability.as_bytes()),
            cancel_capability_hash: digest(request.submission.cancel_capability.as_bytes()),
            children: BTreeSet::new(),
            admission_closed: false,
            settlement_sent: false,
            causal_terminal: false,
            terminal_observed: false,
            worker_wait_status: None,
            cancellation_sent: false,
            broker_cancel_sent: false,
            broker_cancel_retry: RetrySchedule::default(),
            session_signal_sent: false,
            cancellation_cause: None,
            cancellation_requester: None,
            cancellation_pending: None,
            cancellation_propagated: false,
            cancellation_receipt_error: None,
            detail: identity_error.clone(),
            session_drained: false,
            drain_retry: RetrySchedule::default(),
            result_written: false,
            result_failure_recorded: false,
            result_retry: RetrySchedule::default(),
        };
        self.active.push(operation);
        if let Some(error) = identity_error {
            self.stage_cancellation(
                self.active.len() - 1,
                "worker_identity_unavailable",
                None,
                Instant::now(),
            );
            return Err(error);
        }
        Ok(None)
    }

    pub fn cancel(&mut self, owner: &CompletionDomainOwner, mut request: InboundCancel) {
        let result = if request.submission.protocol != PROTOCOL
            || request.submission.supervisor_authority_id != owner.supervisor_authority_id
        {
            Err("unsupported original-work cancellation authority".to_owned())
        } else if let Some(index) = self.active.iter().position(|active| {
            active.submission.work_id == request.submission.work_id
                && active.submission.root_authority.root_id == request.submission.root_id
                && capability_matches(
                    &active.cancel_capability_hash,
                    &request.submission.cancel_capability,
                )
        }) {
            let accepted = match (
                self.active[index].cancellation_cause.as_deref(),
                self.active[index].cancellation_requester.as_ref(),
                self.active[index].cancellation_pending.as_ref(),
            ) {
                (Some("explicit_request"), Some(principal), _) if principal == &request.peer => {
                    Ok(true)
                }
                (Some(cause), _, _) => Err(format!(
                    "original work already has accepted cancellation cause {cause}"
                )),
                (None, _, Some(pending))
                    if pending.cause != "explicit_request"
                        || pending.requester.as_ref() != Some(&request.peer) =>
                {
                    Err("a different cancellation is pending durable receipt".to_owned())
                }
                (None, _, Some(_)) => Ok(false),
                (None, _, None) => Ok(self.stage_cancellation(
                    index,
                    "explicit_request",
                    Some(request.peer.clone()),
                    Instant::now(),
                )),
            };
            match accepted {
                Err(error) => Err(error),
                Ok(accepted) => {
                    if accepted {
                        self.propagate_accepted_cancellations(Instant::now());
                    }
                    let start = SpanStart::new("root_original_work_cancel", "process_tree")
                        .with_lifecycle_phase("original_work_cancellation")
                        .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
                        .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
                        .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
                        .with_identifier("root_id", &request.submission.root_id)
                        .with_identifier("initiator_pid", &request.peer.pid.to_string())
                        .with_identifier("initiator_boot_id", &request.peer.boot_id)
                        .with_identifier(
                            "initiator_starttime_ticks",
                            &request.peer.starttime_ticks.to_string(),
                        )
                        .with_hashed_correlation("work_id", &request.submission.work_id);
                    process_recorder().with_requested_span(start, |span| {
                        let _ = span.record(
                            if accepted {
                                DiagnosticPhase::Committed
                            } else {
                                DiagnosticPhase::Contention
                            },
                            if accepted {
                                PhaseObservation::committed()
                                    .with_cause("original_work_cancellation_accepted")
                            } else {
                                PhaseObservation::effects_possible()
                                    .with_cause("original_work_cancellation_receipt_pending")
                            },
                        );
                    });
                    Ok(accepted)
                }
            }
        } else if self.never_forked.iter().any(|operation| {
            operation.submission.work_id == request.submission.work_id
                && operation.submission.root_authority.root_id == request.submission.root_id
                && digest(request.submission.cancel_capability.as_bytes())
                    == digest(operation.submission.cancel_capability.as_bytes())
        }) {
            Err("original work is already terminal without a worker".to_owned())
        } else {
            Err("original work is not active for this exact root".to_owned())
        };
        let (status, detail) = match result {
            Ok(true) => ("cancellation_accepted", None),
            Ok(false) => (
                "cancellation_pending_receipt",
                Some(
                    "cancellation has not taken effect; durable receipt persistence is pending"
                        .to_owned(),
                ),
            ),
            Err(error) => ("rejected", Some(error)),
        };
        let response = WorkResponse {
            protocol: PROTOCOL.into(),
            work_id: request.submission.work_id.clone(),
            status: status.into(),
            root_id: request.submission.root_id.clone(),
            supervisor_authority_id: owner.supervisor_authority_id.clone(),
            worker_identity: None,
            detail,
        };
        send_response(&mut request.socket, &response);
    }

    fn stage_cancellation(
        &mut self,
        index: usize,
        cause: &str,
        requester: Option<SourceProcessIdentity>,
        now: Instant,
    ) -> bool {
        Self::stage_operation_cancellation(&mut self.active[index], cause, requester, now)
    }

    fn stage_operation_cancellation(
        operation: &mut OriginalOperation,
        cause: &str,
        requester: Option<SourceProcessIdentity>,
        now: Instant,
    ) -> bool {
        if operation.cancellation_cause.is_some() {
            return true;
        }
        if operation.cancellation_pending.is_some() {
            return false;
        }
        let receipt = CancellationReceipt {
            protocol: PROTOCOL.into(),
            work_id: operation.submission.work_id.clone(),
            root_id: operation.submission.root_authority.root_id.clone(),
            cause: cause.into(),
            requester: requester.clone(),
        };
        match persist_cancellation(&operation.state_dir, &receipt) {
            Ok(()) => {
                operation.cancellation_cause = Some(cause.into());
                operation.cancellation_requester = requester;
                operation.cancellation_receipt_error = None;
                true
            }
            Err(error) => {
                let mut retry = RetrySchedule::default();
                retry.record_failure(now);
                operation.cancellation_receipt_error = Some(error);
                operation.cancellation_pending = Some(PendingCancellation {
                    cause: cause.into(),
                    requester,
                    retry,
                });
                false
            }
        }
    }

    fn retry_pending_cancellations(&mut self, now: Instant) {
        for operation in &mut self.active {
            let Some(pending) = operation.cancellation_pending.as_mut() else {
                continue;
            };
            if !pending.retry.due(now) {
                continue;
            }
            let receipt = CancellationReceipt {
                protocol: PROTOCOL.into(),
                work_id: operation.submission.work_id.clone(),
                root_id: operation.submission.root_authority.root_id.clone(),
                cause: pending.cause.clone(),
                requester: pending.requester.clone(),
            };
            match persist_cancellation(&operation.state_dir, &receipt) {
                Ok(()) => {
                    operation.cancellation_cause = Some(pending.cause.clone());
                    operation.cancellation_requester = pending.requester.clone();
                    operation.cancellation_pending = None;
                    operation.cancellation_receipt_error = None;
                }
                Err(error) => {
                    operation.cancellation_receipt_error = Some(error);
                    pending.retry.record_failure(now);
                }
            }
        }
    }

    fn propagate_accepted_cancellations(&mut self, now: Instant) {
        loop {
            let Some(index) = self.active.iter().position(|operation| {
                operation.cancellation_cause.is_some() && !operation.cancellation_propagated
            }) else {
                break;
            };
            self.active[index].cancellation_propagated = true;
            let children = self.active[index].children.clone();
            for child in children {
                if let Some(child_index) = self
                    .active
                    .iter()
                    .position(|active| active.submission.work_id == child)
                {
                    self.stage_cancellation(child_index, "causal_parent_cancelled", None, now);
                }
            }
        }
    }

    pub fn tick(&mut self, owner: &CompletionDomainOwner) {
        self.tick_at(owner, Instant::now());
    }

    fn tick_at(&mut self, owner: &CompletionDomainOwner, now: Instant) {
        for operation in &mut self.never_forked {
            if operation.result_written || !operation.result_retry.due(now) {
                continue;
            }
            if !operation.handle_terminalized {
                match terminalize_never_forked(&operation.state_dir, &operation.detail) {
                    Ok(()) => operation.handle_terminalized = true,
                    Err(error) => {
                        operation.result_retry.record_failure(now);
                        if !operation.result_failure_recorded {
                            operation.result_failure_recorded = true;
                            record_result_integration_failure(
                                owner,
                                &operation.submission,
                                "never_forked_handle_terminalization_pending",
                                &error,
                            );
                        }
                        continue;
                    }
                }
            }
            let result = write_result(
                &operation.state_dir,
                &operation.intent,
                &operation.result_nonce,
                &BTreeSet::new(),
                "never_forked",
                None,
                Some(&operation.detail),
            );
            operation.result_written = result.is_ok();
            if operation.result_written {
                let start = SpanStart::new("root_original_work_terminal", "process_tree")
                    .with_lifecycle_phase("original_work_terminal_integration")
                    .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
                    .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
                    .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
                    .with_identifier("root_id", &operation.submission.root_authority.root_id)
                    .with_identifier("worker_pid", "never_forked")
                    .with_hashed_correlation("work_id", &operation.submission.work_id);
                process_recorder().with_requested_span(start, |span| {
                    let _ = span.record(
                        DiagnosticPhase::Committed,
                        PhaseObservation::terminal().with_cause("never_forked_result_persisted"),
                    );
                });
            }
            if let Err(error) = result
                && !operation.result_failure_recorded
            {
                operation.result_failure_recorded = true;
                let start = SpanStart::new("root_original_work_result", "filesystem")
                    .with_lifecycle_phase("original_work_result_integration")
                    .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
                    .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
                    .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
                    .with_identifier("root_id", &operation.submission.root_authority.root_id)
                    .with_hashed_correlation("work_id", &operation.submission.work_id);
                process_recorder().with_requested_span(start, |span| {
                    let _ = span.record(
                        DiagnosticPhase::Failed,
                        PhaseObservation::effects_possible()
                            .with_cause("never_forked_result_persistence_failed")
                            .with_cause(&error),
                    );
                });
            }
            if !operation.result_written {
                operation.result_retry.record_failure(now);
            }
        }
        let owner_exit = self
            .active
            .iter()
            .enumerate()
            .filter(|(_, operation)| {
                operation.cancellation_cause.is_none()
                    && operation.cancellation_pending.is_none()
                    && operation
                        .intent
                        .cancel_owner
                        .as_ref()
                        .is_some_and(|owner| identity(owner.pid).as_ref() != Ok(owner))
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for index in owner_exit {
            self.stage_cancellation(index, "owner_exit", None, now);
        }
        let control_loss = self
            .active
            .iter()
            .enumerate()
            .filter(|(_, operation)| {
                (operation.control_closed || operation.terminal_observed)
                    && !operation.causal_terminal
                    && operation.cancellation_cause.is_none()
                    && operation.cancellation_pending.is_none()
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for index in control_loss {
            self.stage_cancellation(index, "worker_control_lost", None, now);
        }
        self.retry_pending_cancellations(now);
        self.propagate_accepted_cancellations(now);
        for operation in &mut self.active {
            if operation.cancellation_cause.is_some()
                && operation.kernel_grant.is_some()
                && !operation.broker_cancel_sent
                && !operation.session_drained
                && operation.broker_cancel_retry.due(now)
            {
                let grant = operation.kernel_grant.as_deref().unwrap();
                match protocol::cancel_accepted_work_at(&super::linux::owner_broker_socket(), grant)
                {
                    Ok(response)
                        if response.starts_with("cancel-signalled ")
                            && response.trim_end().split_whitespace().count() == 2 =>
                    {
                        let incarnation = response.trim_end().split_whitespace().nth(1).unwrap();
                        if operation
                            .kernel_incarnation
                            .as_deref()
                            .is_none_or(|id| id == incarnation)
                            && uuid::Uuid::parse_str(incarnation).is_ok()
                        {
                            operation.kernel_incarnation = Some(incarnation.to_owned());
                            operation.broker_cancel_sent = true;
                        } else {
                            operation.detail = Some("broker cancellation identity conflict".into());
                            operation.broker_cancel_retry.record_failure(now);
                        }
                    }
                    Ok(response) => {
                        operation.detail =
                            Some(format!("broker cancellation refused: {}", response.trim()));
                        operation.broker_cancel_retry.record_failure(now);
                    }
                    Err(error) => {
                        operation.detail = Some(format!("broker cancellation uncertain: {error}"));
                        operation.broker_cancel_retry.record_failure(now);
                    }
                }
            }
        }
        let settled_ids: BTreeMap<String, bool> = self
            .active
            .iter()
            .map(|operation| {
                (
                    operation.submission.work_id.clone(),
                    operation_tree_drained(operation, self.adopted_child_live)
                        && operation.causal_terminal
                        && operation.result_written,
                )
            })
            .chain(self.never_forked.iter().map(|operation| {
                (
                    operation.submission.work_id.clone(),
                    operation.result_written,
                )
            }))
            .collect();
        for operation in &mut self.active {
            observe_control(operation);
            reply_deferred_acceptance(operation, owner);
            if let Some(command) = next_execution_control(
                operation.prepared_for_grant,
                operation.grant_sent,
                operation.cancellation_sent,
                operation.cancellation_cause.as_deref(),
            ) && command != b'G'
                && !operation.control_closed
            {
                let command = &[command];
                match operation.control.write_all(command) {
                    Ok(()) => operation.cancellation_sent = true,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        operation.detail.get_or_insert_with(|| {
                            format!(
                                "cancellation control failed; signalled session directly: {error}"
                            )
                        });
                        if operation.kernel_grant.is_none()
                            && process_session_live(operation.worker_session)
                        {
                            signal_session(operation.worker_session, libc::SIGTERM);
                            operation.session_signal_sent = true;
                        } else {
                            operation.cancellation_sent = true;
                        }
                    }
                }
            }
            if grant_allowed(operation) {
                match operation.control.write_all(b"G") {
                    Ok(()) => {
                        operation.grant_sent = true;
                        let start = SpanStart::new("root_original_work_grant", "process_tree")
                            .with_lifecycle_phase("original_work_execution_grant")
                            .with_identifier(
                                "supervisor_authority_id",
                                &owner.supervisor_authority_id,
                            )
                            .with_identifier(
                                "authority_pid",
                                &owner.guardian_identity.pid.to_string(),
                            )
                            .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
                            .with_identifier(
                                "root_id",
                                &operation.submission.root_authority.root_id,
                            )
                            .with_identifier("worker_pid", &operation.worker_session.to_string())
                            .with_identifier(
                                "worker_boot_id",
                                &operation
                                    .worker_identity
                                    .as_ref()
                                    .map(|identity| identity.boot_id.as_str())
                                    .unwrap_or("unavailable"),
                            )
                            .with_identifier(
                                "worker_starttime_ticks",
                                &operation
                                    .worker_identity
                                    .as_ref()
                                    .map(|identity| identity.starttime_ticks.to_string())
                                    .unwrap_or_else(|| "unavailable".into()),
                            )
                            .with_hashed_correlation("work_id", &operation.submission.work_id);
                        process_recorder().with_requested_span(start, |span| {
                            let _ = span.record(
                                DiagnosticPhase::Committed,
                                PhaseObservation::committed()
                                    .with_cause("root_execution_grant_sent"),
                            );
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        operation.detail = Some(format!("execution grant failed: {error}"));
                        Self::stage_operation_cancellation(
                            operation,
                            "grant_transport_lost",
                            None,
                            now,
                        );
                    }
                }
            }
            if operation.kernel_grant.is_none()
                && operation.control_closed
                && operation.cancellation_cause.is_some()
                && !operation.session_signal_sent
                && process_session_live(operation.worker_session)
            {
                signal_session(operation.worker_session, libc::SIGTERM);
                operation.session_signal_sent = true;
            }
            let children_terminal = causal_children_terminal(&operation.children, &settled_ids);
            if operation.admission_closed
                && children_terminal
                && !operation.settlement_sent
                && !operation.control_closed
            {
                match operation.control.write_all(b"S") {
                    Ok(()) => operation.settlement_sent = true,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(error) => {
                        operation.detail = Some(format!("settlement channel failed: {error}"))
                    }
                }
            }
            if let Some(worker) = &mut operation.worker {
                match worker.try_wait() {
                    Ok(Some(status)) => {
                        operation.terminal_observed = true;
                        let status_text = status.to_string();
                        use std::os::unix::process::ExitStatusExt;
                        operation.worker_wait_status = Some(status.into_raw());
                        if !operation.causal_terminal {
                            operation.detail.get_or_insert_with(|| {
                                format!("worker exited before causal terminal: {status_text}")
                            });
                        }
                    }
                    Ok(None) => {}
                    Err(error) if error.raw_os_error() == Some(libc::ECHILD) => {
                        operation.terminal_observed = true;
                        operation.detail.get_or_insert_with(|| {
                            "exclusive worker wait was unavailable (ECHILD)".into()
                        });
                    }
                    Err(error) => operation.detail = Some(format!("worker wait failed: {error}")),
                }
            } else if operation.drain_retry.due(now) {
                match observe_kernel_drain(operation) {
                    Ok(Some((local_pid, status))) => {
                        operation.terminal_observed = true;
                        operation.session_drained = true;
                        operation.kernel_worker_local_pid = Some(local_pid);
                        operation.worker_wait_status = Some(status);
                    }
                    Ok(None) => operation.drain_retry.record_failure(now),
                    Err(error) => {
                        operation.detail =
                            Some(format!("broker work observation uncertain: {error}"));
                        operation.drain_retry.record_failure(now);
                    }
                }
            }
            if operation.worker.is_some()
                && operation.terminal_observed
                && !operation.session_drained
                && operation.drain_retry.due(now)
            {
                operation.session_drained = !process_session_live(operation.worker_session);
                if !operation.session_drained {
                    operation.drain_retry.record_failure(now);
                }
            }
            if operation.session_drained && !operation.causal_terminal {
                operation.admission_closed = true;
                operation
                    .detail
                    .get_or_insert_with(|| "worker drained before exact causal terminal".into());
                if children_terminal {
                    operation.causal_terminal = true;
                }
            }
        }
        for operation in &mut self.active {
            let drained = operation_tree_drained(operation, self.adopted_child_live);
            if drained
                && operation.causal_terminal
                && operation.cancellation_pending.is_none()
                && causal_children_terminal(&operation.children, &settled_ids)
                && !operation.result_written
                && operation.result_retry.due(now)
            {
                let drain_record =
                    if operation.kernel_grant.is_some() && !operation.kernel_drain_recorded {
                        write_kernel_drain_artifact(operation)
                    } else {
                        Ok(())
                    };
                if operation.kernel_grant.is_some() && drain_record.is_ok() {
                    operation.kernel_drain_recorded = true;
                }
                let result = drain_record.and_then(|()| {
                    write_result(
                        &operation.state_dir,
                        &operation.intent,
                        &operation.result_nonce,
                        &operation.children,
                        "terminal",
                        operation.worker_wait_status,
                        operation.detail.as_deref(),
                    )
                });
                operation.result_written = result.is_ok();
                if let Err(error) = result
                    && !operation.result_failure_recorded
                {
                    operation.result_failure_recorded = true;
                    let start = SpanStart::new("root_original_work_result", "filesystem")
                        .with_lifecycle_phase("original_work_result_integration")
                        .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
                        .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
                        .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
                        .with_identifier("root_id", &operation.submission.root_authority.root_id)
                        .with_hashed_correlation("work_id", &operation.submission.work_id);
                    process_recorder().with_requested_span(start, |span| {
                        let _ = span.record(
                            DiagnosticPhase::Failed,
                            PhaseObservation::effects_possible()
                                .with_cause("root_result_persistence_failed")
                                .with_cause(&error),
                        );
                    });
                }
                if !operation.result_written {
                    operation.result_retry.record_failure(now);
                }
            }
        }
        self.active.retain(|operation| {
            let drained = operation_tree_drained(operation, self.adopted_child_live);
            if drained
                && operation.causal_terminal
                && operation.cancellation_pending.is_none()
                && causal_children_terminal(&operation.children, &settled_ids)
                && operation.result_written
            {
                let start = SpanStart::new("root_original_work_terminal", "process_tree")
                    .with_lifecycle_phase("original_work_terminal_integration")
                    .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
                    .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
                    .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
                    .with_identifier("root_id", &operation.submission.root_authority.root_id)
                    .with_identifier("worker_pid", &operation.worker_session.to_string())
                    .with_identifier(
                        "worker_boot_id",
                        &operation
                            .worker_identity
                            .as_ref()
                            .map(|identity| identity.boot_id.as_str())
                            .unwrap_or("unavailable"),
                    )
                    .with_hashed_correlation("work_id", &operation.submission.work_id);
                process_recorder().with_requested_span(start, |span| {
                    let _ = span.record(
                        DiagnosticPhase::Committed,
                        PhaseObservation::terminal().with_cause("original_work_tree_drained"),
                    );
                });
                false
            } else {
                true
            }
        });
        self.never_forked
            .retain(|operation| !operation.result_written);
    }
}

fn submit_failure_status(effects_possible: bool) -> &'static str {
    if effects_possible {
        "effects_possible_no_replay"
    } else {
        "rejected_preaccept"
    }
}

fn next_execution_control(
    prepared: bool,
    grant_sent: bool,
    cancellation_sent: bool,
    cancellation_cause: Option<&str>,
) -> Option<u8> {
    if let Some(cause) = cancellation_cause {
        if cancellation_sent {
            return None;
        }
        return Some(match cause {
            "owner_exit" => b'O',
            "causal_parent_cancelled" => b'K',
            "explicit_request" => b'C',
            _ => b'F',
        });
    }
    (prepared && !grant_sent).then_some(b'G')
}

fn grant_allowed(operation: &OriginalOperation) -> bool {
    operation.cancellation_pending.is_none()
        && !operation.control_closed
        && next_execution_control(
            operation.prepared_for_grant,
            operation.grant_sent,
            operation.cancellation_sent,
            operation.cancellation_cause.as_deref(),
        ) == Some(b'G')
}

fn causal_children_terminal(
    children: &BTreeSet<String>,
    terminal: &BTreeMap<String, bool>,
) -> bool {
    children
        .iter()
        .all(|child| terminal.get(child).copied().unwrap_or(true))
}

fn operation_tree_drained(operation: &OriginalOperation, adopted_child_live: bool) -> bool {
    operation.terminal_observed
        && operation.session_drained
        && (operation.kernel_grant.is_some() || !adopted_child_live)
}

fn write_kernel_drain_artifact(operation: &OriginalOperation) -> Result<(), String> {
    let receipt = BrokerDrainReceipt {
        protocol: PROTOCOL,
        root_id: &operation.intent.root_id,
        work_id: &operation.intent.work_id,
        grant_id: operation.kernel_grant.as_deref().ok_or("Q grant missing")?,
        work_incarnation: operation
            .kernel_incarnation
            .as_deref()
            .ok_or("Q incarnation missing")?,
        worker_local_pid: operation
            .kernel_worker_local_pid
            .ok_or("Q worker PID missing")?,
        worker_wait_status: operation
            .worker_wait_status
            .ok_or("Q wait status missing")?,
        physical_tree_drained: true,
    };
    create_artifact(&operation.state_dir, BROKER_DRAIN_FILE, &receipt)
}

fn observe_kernel_drain(operation: &mut OriginalOperation) -> Result<Option<(i32, i32)>, String> {
    let grant = operation
        .kernel_grant
        .as_deref()
        .ok_or("broker grant missing")?;
    let response = protocol::observe_accepted_work_at(&super::linux::owner_broker_socket(), grant)
        .map_err(|error| error.to_string())?;
    let mut parts = response.split_whitespace();
    let status = parts
        .next()
        .ok_or("broker returned empty work observation")?;
    let incarnation = parts.next().ok_or("broker omitted work incarnation")?;
    let matches = operation
        .kernel_incarnation
        .as_ref()
        .is_none_or(|expected| expected == incarnation);
    if status == "work-uncertain" {
        return Err(response.trim().to_owned());
    }
    if !matches || uuid::Uuid::parse_str(incarnation).is_err() {
        return Err("broker returned conflicting work incarnation".into());
    }
    operation.kernel_incarnation = Some(incarnation.to_owned());
    match status {
        "work-live" | "work-drain-pending" if parts.next().is_none() => Ok(None),
        "work-drained" => {
            let worker_local_pid = parts.next().and_then(|value| value.parse::<i32>().ok());
            let wait_status = parts.next().and_then(|value| value.parse::<i32>().ok());
            if !worker_local_pid.is_some_and(|pid| pid > 1)
                || wait_status.is_none()
                || parts.next().is_some()
            {
                return Err("broker returned malformed drain certificate".into());
            }
            Ok(Some((worker_local_pid.unwrap(), wait_status.unwrap())))
        }
        _ => Err(format!(
            "broker returned invalid work observation: {}",
            response.trim()
        )),
    }
}

fn observe_control(operation: &mut OriginalOperation) {
    if operation.control_closed {
        return;
    }
    let mut bytes = [0; 64];
    loop {
        match operation.control.read(&mut bytes) {
            Ok(0) => {
                apply_control_frames(operation);
                operation.control_closed = true;
                if !operation.causal_terminal {
                    operation.admission_closed = true;
                    operation.detail.get_or_insert_with(|| {
                        "worker control closed before exact causal terminal".into()
                    });
                }
                break;
            }
            Ok(count) => {
                operation.control_input.extend_from_slice(&bytes[..count]);
                apply_control_frames(operation);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                apply_control_frames(operation);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                operation.detail = Some(format!("worker control failed: {error}"));
                break;
            }
        }
    }
    apply_control_frames(operation);
}

fn apply_control_frames(operation: &mut OriginalOperation) {
    for byte in operation.control_input.drain(..) {
        match byte {
            b'P' => operation.prepared_for_grant = true,
            b'R' => operation.admission_closed = true,
            b'T' => {
                operation.admission_closed = true;
                operation.causal_terminal = true;
            }
            _ => operation.detail = Some("worker emitted invalid control phase".into()),
        }
    }
}

fn reply_deferred_acceptance(operation: &mut OriginalOperation, owner: &CompletionDomainOwner) {
    let Some(mut socket) = operation.acceptance_response.take() else {
        return;
    };
    let (status, detail) = if operation.prepared_for_grant {
        ("accepted", None)
    } else if operation.admission_closed || operation.control_closed || operation.terminal_observed
    {
        (
            "effects_possible_no_replay",
            Some(
                operation
                    .detail
                    .clone()
                    .unwrap_or_else(|| "accepted worker ended before execution preparation".into()),
            ),
        )
    } else {
        operation.acceptance_response = Some(socket);
        return;
    };
    let response = WorkResponse {
        protocol: PROTOCOL.into(),
        work_id: operation.submission.work_id.clone(),
        status: status.into(),
        root_id: operation.submission.root_authority.root_id.clone(),
        supervisor_authority_id: owner.supervisor_authority_id.clone(),
        worker_identity: operation.worker_identity.clone(),
        detail,
    };
    if response.status == "accepted" && drop_acceptance_reply_for_fault_fixture() {
        return;
    }
    send_response(&mut socket, &response);
}

#[cfg(all(feature = "age360-fault-fixtures", target_os = "linux"))]
fn drop_acceptance_reply_for_fault_fixture() -> bool {
    let Some(root) = std::env::var_os("AGE360_FAULT_ROOT") else {
        return false;
    };
    let Some(parent_net) = std::env::var_os("AGE360_FAULT_PARENT_NET") else {
        return false;
    };
    if std::fs::read_link("/proc/self/ns/net")
        .ok()
        .is_none_or(|net| net.as_os_str() == parent_net)
    {
        return false;
    }
    let root = std::path::Path::new(&root);
    let request = root.join("root-work-acceptance-reply-loss.hold");
    if !request.exists() {
        return false;
    }
    let _ = std::fs::write(
        root.join("root-work-acceptance-reply-loss.reached"),
        std::process::id().to_string(),
    );
    let _ = std::fs::remove_file(request);
    true
}

#[cfg(not(all(feature = "age360-fault-fixtures", target_os = "linux")))]
fn drop_acceptance_reply_for_fault_fixture() -> bool {
    false
}

fn send_response(socket: &mut UnixStream, response: &WorkResponse) {
    let _ = serde_json::to_writer(&mut *socket, response);
    let _ = socket.write_all(b"\n");
}

fn create_capability_descriptor(capability: &str) -> Result<OwnedFd, String> {
    let mut descriptors = [-1; 2];
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let read = unsafe { OwnedFd::from_raw_fd(descriptors[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(descriptors[1]) };
    let mut file = File::from(write);
    file.write_all(capability.as_bytes())
        .map_err(|error| error.to_string())?;
    drop(file);
    Ok(read)
}

fn validate_descriptor(fd: &OwnedFd, directory: bool, executable: bool) -> Result<(), String> {
    let metadata = File::from(fd.try_clone().map_err(|error| error.to_string())?)
        .metadata()
        .map_err(|error| error.to_string())?;
    if metadata.is_dir() != directory || (!directory && !metadata.is_file()) {
        return Err("original work descriptor type mismatch".into());
    }
    if executable && metadata.mode() & 0o111 == 0 {
        return Err("original work executable descriptor is not executable".into());
    }
    Ok(())
}

fn validate_bound_identity(
    fd: &OwnedFd,
    expected: &std::path::Path,
    label: &str,
) -> Result<(), String> {
    let descriptor = File::from(fd.try_clone().map_err(|error| error.to_string())?)
        .metadata()
        .map_err(|error| error.to_string())?;
    let path = std::fs::metadata(expected)
        .map_err(|error| format!("original work {label} is unavailable: {error}"))?;
    if descriptor.dev() != path.dev()
        || descriptor.ino() != path.ino()
        || descriptor.file_type() != path.file_type()
    {
        return Err(format!(
            "original work {label} descriptor identity mismatch"
        ));
    }
    Ok(())
}

fn validate_intent_descriptor(state_dir: &OwnedFd, intent: &OwnedFd) -> Result<(), String> {
    let intent_metadata = File::from(intent.try_clone().map_err(|error| error.to_string())?)
        .metadata()
        .map_err(|error| error.to_string())?;
    let name = c"root-work-intent-v1.json";
    let mut pinned: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            state_dir.as_raw_fd(),
            name.as_ptr(),
            &mut pinned,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if intent_metadata.dev() != pinned.st_dev || intent_metadata.ino() != pinned.st_ino {
        return Err("original work intent descriptor identity mismatch".into());
    }
    Ok(())
}

fn validate_peer_executable(
    executable: &OwnedFd,
    peer: &SourceProcessIdentity,
) -> Result<(), String> {
    if identity(peer.pid).as_ref() != Ok(peer) {
        return Err("original work initiator identity changed before acceptance".into());
    }
    let requested = File::from(executable.try_clone().map_err(|error| error.to_string())?)
        .metadata()
        .map_err(|error| error.to_string())?;
    let peer_image = std::fs::metadata(format!("/proc/{}/exe", peer.pid))
        .map_err(|error| format!("original work initiator image unavailable: {error}"))?;
    if requested.dev() != peer_image.dev() || requested.ino() != peer_image.ino() {
        return Err("original work executable does not match the exact initiator image".into());
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "the exact accepted descriptors and owner are an indivisible grant request"
)]
fn prepare_kernel_grant(
    owner: &CompletionDomainOwner,
    submission: &WorkSubmission,
    acceptance: &AcceptanceReceipt,
    executable: &OwnedFd,
    intent: &OwnedFd,
    cwd: &OwnedFd,
    state_dir: &OwnedFd,
) -> Result<String, String> {
    let accepted_fd = unsafe {
        libc::openat(
            state_dir.as_raw_fd(),
            c"root-work-accepted-v1.json".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if accepted_fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let accepted = unsafe { OwnedFd::from_raw_fd(accepted_fd) };
    let mut bytes = serde_json::to_vec(acceptance).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let spec = AcceptedWorkSpec {
        root_id: submission.root_authority.root_id.clone(),
        work_id: submission.work_id.clone(),
        request_sha256: submission.request_sha256.clone(),
        accepted_sha256: hex_digest(&bytes),
        owner_generation: owner.owner_generation.clone(),
    };
    let response = protocol::prepare_accepted_work_at(
        &super::linux::owner_broker_socket(),
        &spec,
        [
            executable.as_raw_fd(),
            intent.as_raw_fd(),
            cwd.as_raw_fd(),
            state_dir.as_raw_fd(),
            accepted.as_raw_fd(),
        ],
    )
    .map_err(|error| error.to_string())?;
    let grant = response
        .trim_end()
        .strip_prefix("prepared-work ")
        .ok_or_else(|| format!("broker grant preparation refused: {}", response.trim()))?;
    uuid::Uuid::parse_str(grant).map_err(|_| "broker returned invalid grant ID".to_owned())?;
    Ok(grant.to_owned())
}

fn launch_kernel_work(
    grant_id: &str,
    executable: &OwnedFd,
    intent: &OwnedFd,
    cwd: &OwnedFd,
    state_dir: &OwnedFd,
    worker_control: &UnixStream,
    capability: &OwnedFd,
) -> Result<(String, i64), String> {
    let accepted_fd = unsafe {
        libc::openat(
            state_dir.as_raw_fd(),
            c"root-work-accepted-v1.json".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if accepted_fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let accepted = unsafe { OwnedFd::from_raw_fd(accepted_fd) };
    let response = protocol::launch_accepted_work_at(
        &super::linux::owner_broker_socket(),
        &LaunchAcceptedWorkSpec {
            grant_id: grant_id.to_owned(),
        },
        [
            executable.as_raw_fd(),
            intent.as_raw_fd(),
            cwd.as_raw_fd(),
            state_dir.as_raw_fd(),
            accepted.as_raw_fd(),
            worker_control.as_raw_fd(),
            capability.as_raw_fd(),
        ],
    )
    .map_err(|error| error.to_string())?;
    let mut parts = response.split_whitespace();
    if parts.next() != Some("launched-work") {
        return Err(format!("broker launch refused: {}", response.trim()));
    }
    let incarnation = parts.next().ok_or("broker omitted work incarnation")?;
    uuid::Uuid::parse_str(incarnation).map_err(|_| "broker returned invalid work incarnation")?;
    let init_pid = parts.next().and_then(|part| part.parse::<i64>().ok());
    let worker_pid = parts.next().and_then(|part| part.parse::<i64>().ok());
    if !init_pid.is_some_and(|pid| pid > 0)
        || !worker_pid.is_some_and(|pid| pid > 0)
        || parts.next().is_some()
    {
        return Err("broker returned invalid launched work identity".into());
    }
    Ok((incarnation.to_owned(), worker_pid.unwrap()))
}

fn read_bounded(fd: &OwnedFd, max: u64) -> Result<Vec<u8>, String> {
    let file = File::from(fd.try_clone().map_err(|error| error.to_string())?);
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size > max {
        return Err("original work intent exceeds bounded size".into());
    }
    let mut bytes = vec![0; size as usize];
    file.read_exact_at(&mut bytes, 0)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

fn create_artifact<T: Serialize>(directory: &OwnedFd, name: &str, value: &T) -> Result<(), String> {
    let name = std::ffi::CString::new(name).expect("static artifact name");
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Err(format!("exists:{name:?}"));
        }
        return Err(error.to_string());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    serde_json::to_writer(&mut file, value).map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    if unsafe { libc::fsync(directory.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn persist_cancellation(directory: &OwnedFd, receipt: &CancellationReceipt) -> Result<(), String> {
    match create_artifact(directory, CANCEL_FILE, receipt) {
        Ok(()) => Ok(()),
        Err(error) if error.starts_with("exists:") => {
            let name = c"root-work-cancel-v1.json";
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let file = unsafe { OwnedFd::from_raw_fd(fd) };
            let existing: CancellationReceipt =
                serde_json::from_slice(&read_bounded(&file, MAX_INTENT_BYTES)?)
                    .map_err(|error| error.to_string())?;
            if existing.protocol == receipt.protocol
                && existing.work_id == receipt.work_id
                && existing.root_id == receipt.root_id
                && existing.cause == receipt.cause
                && existing.requester == receipt.requester
            {
                Ok(())
            } else {
                Err("existing cancellation evidence conflicts with this request".into())
            }
        }
        Err(error) => Err(error),
    }
}

fn terminalize_never_forked(directory: &OwnedFd, detail: &str) -> Result<(), String> {
    let _lock = lock_handle_file(directory, "completion.lock")?;
    let meta_fd = open_handle_file(directory, "meta.json")?;
    let mut meta: serde_json::Value =
        serde_json::from_slice(&read_bounded(&meta_fd, MAX_INTENT_BYTES)?)
            .map_err(|error| error.to_string())?;
    let object = meta
        .as_object_mut()
        .ok_or_else(|| "agent-bash metadata is not an object".to_owned())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis() as u64;
    object.insert("state".into(), serde_json::Value::String("ERROR".into()));
    object.insert(
        "completion_reason".into(),
        serde_json::Value::String("root-worker-never-forked".into()),
    );
    object.insert("rc".into(), serde_json::Value::from(70));
    object.insert("signal".into(), serde_json::Value::Null);
    object.insert("completed_at_unix_ms".into(), serde_json::Value::from(now));
    object.insert("updated_at_unix_ms".into(), serde_json::Value::from(now));
    object.insert("error".into(), serde_json::Value::String(detail.into()));
    let mut meta_bytes = serde_json::to_vec_pretty(&meta).map_err(|error| error.to_string())?;
    meta_bytes.push(b'\n');
    atomic_replace_at(directory, "rc", b"70\n")?;
    atomic_replace_at(directory, "meta.json", &meta_bytes)
}

fn open_handle_file(directory: &OwnedFd, name: &str) -> Result<OwnedFd, String> {
    let name = std::ffi::CString::new(name).map_err(|error| error.to_string())?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn lock_handle_file(directory: &OwnedFd, name: &str) -> Result<File, String> {
    let name = std::ffi::CString::new(name).map_err(|error| error.to_string())?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.to_string());
        }
    }
}

fn atomic_replace_at(directory: &OwnedFd, name: &str, bytes: &[u8]) -> Result<(), String> {
    let temp = format!(".{name}.root-{}", uuid::Uuid::new_v4());
    let temp_name = std::ffi::CString::new(temp.as_str()).map_err(|error| error.to_string())?;
    let final_name = std::ffi::CString::new(name).map_err(|error| error.to_string())?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temp_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string());
    drop(file);
    if let Err(error) = result {
        unsafe { libc::unlinkat(directory.as_raw_fd(), temp_name.as_ptr(), 0) };
        return Err(error);
    }
    if unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            temp_name.as_ptr(),
            directory.as_raw_fd(),
            final_name.as_ptr(),
        )
    } != 0
    {
        let error = std::io::Error::last_os_error().to_string();
        unsafe { libc::unlinkat(directory.as_raw_fd(), temp_name.as_ptr(), 0) };
        return Err(error);
    }
    if unsafe { libc::fsync(directory.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn record_result_integration_failure(
    owner: &CompletionDomainOwner,
    submission: &WorkSubmission,
    cause: &'static str,
    error: &str,
) {
    let start = SpanStart::new("root_original_work_result", "filesystem")
        .with_lifecycle_phase("original_work_result_integration")
        .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
        .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
        .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
        .with_identifier("root_id", &submission.root_authority.root_id)
        .with_hashed_correlation("work_id", &submission.work_id);
    process_recorder().with_requested_span(start, |span| {
        let _ = span.record(
            DiagnosticPhase::Failed,
            PhaseObservation::effects_possible()
                .with_cause(cause)
                .with_cause(error),
        );
    });
}

fn write_result(
    directory: &OwnedFd,
    intent: &IntentIdentity,
    result_nonce: &str,
    children: &BTreeSet<String>,
    outcome: &str,
    worker_wait_status: Option<i32>,
    detail: Option<&str>,
) -> Result<(), String> {
    let snapshot = read_terminal_snapshot(directory).unwrap_or_default();
    let outcome = projected_result_outcome(outcome, &snapshot, detail);
    let receipt = ResultReceipt {
        protocol: PROTOCOL,
        work_id: &intent.work_id,
        root_id: &intent.root_id,
        result_nonce,
        outcome,
        causal_children: children,
        physical_tree_drained: true,
        worker_wait_status,
        agent_state: snapshot.state.as_deref(),
        agent_rc: snapshot.rc,
        agent_signal: snapshot.signal,
        completion_reason: snapshot.completion_reason.as_deref(),
        detail,
    };
    let expected = serde_json::to_value(&receipt).map_err(|error| error.to_string())?;
    match create_artifact(directory, RESULT_FILE, &receipt) {
        Ok(()) => Ok(()),
        Err(error) if error.starts_with("exists:") => {
            let name = c"root-work-result-v1.json";
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let file = unsafe { OwnedFd::from_raw_fd(fd) };
            let existing: serde_json::Value =
                serde_json::from_slice(&read_bounded(&file, MAX_INTENT_BYTES)?)
                    .map_err(|error| error.to_string())?;
            if existing == expected {
                Ok(())
            } else {
                Err("existing root result conflicts with the root-owned terminal result".into())
            }
        }
        Err(error) => Err(error),
    }
}

fn projected_result_outcome(
    requested: &str,
    snapshot: &TerminalSnapshot,
    detail: Option<&str>,
) -> &'static str {
    if requested == "never_forked" {
        return "never_forked";
    }
    match snapshot.completion_reason.as_deref() {
        Some("cancel-request") => "cancelled_explicit",
        Some("owner-exit") => "cancelled_owner_exit",
        Some("causal-parent-cancelled") => "cancelled_causal_parent",
        Some("root-authority-lost") => "root_lost",
        Some("supervisor-error" | "root-worker-never-forked") => "failed",
        _ if snapshot.state.as_deref() == Some("ERROR") => "failed",
        _ if detail.is_some() => "failed",
        _ if snapshot.state.as_deref() == Some("DONE") => "terminal",
        _ => "terminal",
    }
}

fn new_capability() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn read_terminal_snapshot(directory: &OwnedFd) -> Result<TerminalSnapshot, String> {
    let name = c"meta.json";
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let file = unsafe { OwnedFd::from_raw_fd(fd) };
    let bytes = read_bounded(&file, MAX_INTENT_BYTES)?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn capability_matches(expected_hash: &[u8; 32], supplied: &str) -> bool {
    expected_hash == &digest(supplied.as_bytes())
}

fn hex_digest(bytes: &[u8]) -> String {
    digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn exact_descendant(child: &SourceProcessIdentity, ancestor: &SourceProcessIdentity) -> bool {
    if identity(child.pid).as_ref() != Ok(child) || identity(ancestor.pid).as_ref() != Ok(ancestor)
    {
        return false;
    }
    if child == ancestor {
        return true;
    }
    let mut pid = child.pid;
    for _ in 0..4096 {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some(tail) = stat.rsplit_once(')').map(|(_, tail)| tail) else {
            return false;
        };
        let Some(parent) = tail
            .split_whitespace()
            .nth(1)
            .and_then(|value| value.parse::<i64>().ok())
        else {
            return false;
        };
        if parent == ancestor.pid {
            return identity(parent).as_ref() == Ok(ancestor);
        }
        if parent <= 1 || parent == pid {
            return false;
        }
        pid = parent;
    }
    false
}

fn process_session_live(session: i64) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return true;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<i64>().ok())
        else {
            return false;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        stat.rsplit_once(')')
            .and_then(|(_, tail)| tail.split_whitespace().nth(3))
            .and_then(|value| value.parse::<i64>().ok())
            == Some(session)
    })
}

fn signal_session(session: i64, signal: i32) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    for pid in entries.filter_map(Result::ok).filter_map(|entry| {
        let pid = entry.file_name().to_str()?.parse::<i64>().ok()?;
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let sid = stat
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .nth(3)?
            .parse::<i64>()
            .ok()?;
        (sid == session).then_some(pid)
    }) {
        if let Ok(pid) = i32::try_from(pid) {
            unsafe { libc::kill(pid, signal) };
        }
    }
}

fn empty_descriptor_array() -> Result<[OwnedFd; FD_COUNT], String> {
    let mut descriptors = Vec::with_capacity(FD_COUNT);
    for _ in 0..FD_COUNT {
        let fd = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        descriptors.push(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    descriptors
        .try_into()
        .map_err(|_| "failed to build descriptor placeholder".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::FromRawFd;

    fn test_operation(directory: OwnedFd, control: UnixStream, work_id: &str) -> OriginalOperation {
        let process = identity(i64::from(std::process::id())).unwrap();
        let domain = owner(process.clone());
        let mut authorities = RootAuthorities::default();
        let grant = authorities.fresh(&domain, process).unwrap();
        let worker = Command::new("true").spawn().unwrap();
        let worker_session = i64::from(worker.id());
        OriginalOperation {
            submission: WorkSubmission {
                protocol: PROTOCOL.into(),
                root_authority: grant.clone(),
                work_id: work_id.into(),
                request_sha256: "digest".into(),
                registration: WorkRegistration::Root,
                cancel_capability: "cancel-secret".into(),
            },
            intent: IntentIdentity {
                protocol: PROTOCOL.into(),
                work_id: work_id.into(),
                root_id: grant.root_id,
                handle: work_id.into(),
                state_root: std::path::PathBuf::from("/tmp"),
                meta: IntentMetaIdentity {
                    cwd: std::path::PathBuf::from("/tmp"),
                },
                cancel_owner: None,
            },
            worker: Some(worker),
            kernel_grant: None,
            kernel_incarnation: None,
            kernel_worker_local_pid: None,
            kernel_drain_recorded: false,
            worker_identity: None,
            worker_session,
            control,
            acceptance_response: None,
            control_input: Vec::new(),
            control_closed: false,
            prepared_for_grant: true,
            grant_sent: true,
            state_dir: directory,
            result_nonce: "result-secret".into(),
            child_capability_hash: digest(b"child-secret"),
            cancel_capability_hash: digest(b"cancel-secret"),
            children: BTreeSet::new(),
            admission_closed: false,
            settlement_sent: false,
            causal_terminal: false,
            terminal_observed: false,
            worker_wait_status: None,
            cancellation_sent: false,
            broker_cancel_sent: false,
            broker_cancel_retry: RetrySchedule::default(),
            session_signal_sent: false,
            cancellation_cause: None,
            cancellation_requester: None,
            cancellation_pending: None,
            cancellation_propagated: false,
            cancellation_receipt_error: None,
            detail: None,
            session_drained: false,
            drain_retry: RetrySchedule::default(),
            result_written: false,
            result_failure_recorded: false,
            result_retry: RetrySchedule::default(),
        }
    }

    fn owner(identity: SourceProcessIdentity) -> CompletionDomainOwner {
        CompletionDomainOwner {
            protocol: "completion-continuation-v2".into(),
            domain_id: "test-domain".into(),
            supervisor_authority_id: "test-supervisor".into(),
            owner_generation: "test-generation".into(),
            guardian_identity: identity.clone(),
            driver_identity: identity,
            endpoint: "/tmp/test-owner.sock".into(),
        }
    }

    #[test]
    fn broker_reserved_root_id_is_used_exactly_once_in_guardian_grant() {
        let context = identity(i64::from(std::process::id())).unwrap();
        let owner = owner(context.clone());
        let root_id = uuid::Uuid::new_v4().to_string();
        let mut authorities = RootAuthorities::default();
        let grant = authorities
            .fresh_with_root_id(&owner, context.clone(), root_id.clone())
            .unwrap();
        assert_eq!(grant.root_id, root_id);
        assert_eq!(grant.control_protocol, SOURCE_CONTROL_PROTOCOL);
        assert_eq!(grant.domain_id, owner.domain_id);
        assert_eq!(grant.guardian_identity, owner.guardian_identity);
        assert!(
            authorities
                .fresh_with_root_id(&owner, context, root_id)
                .is_err()
        );
    }

    fn open_directory(path: &std::path::Path) -> OwnedFd {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(fd >= 0);
        unsafe { OwnedFd::from_raw_fd(fd) }
    }

    #[test]
    fn kernel_result_requires_exact_q_fields_before_drain_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let (control, _worker_control) = UnixStream::pair().unwrap();
        let mut operation = test_operation(open_directory(temp.path()), control, "work-q");
        let grant = uuid::Uuid::new_v4().to_string();
        let incarnation = uuid::Uuid::new_v4().to_string();
        operation.kernel_grant = Some(grant.clone());
        assert!(write_kernel_drain_artifact(&operation).is_err());
        assert!(!temp.path().join(BROKER_DRAIN_FILE).exists());
        operation.kernel_incarnation = Some(incarnation.clone());
        operation.kernel_worker_local_pid = Some(2);
        operation.worker_wait_status = Some(0);
        write_kernel_drain_artifact(&operation).unwrap();
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(temp.path().join(BROKER_DRAIN_FILE)).unwrap())
                .unwrap();
        assert_eq!(receipt["grant_id"], grant);
        assert_eq!(receipt["work_incarnation"], incarnation);
        assert_eq!(receipt["work_id"], "work-q");
        assert_eq!(receipt["physical_tree_drained"], true);
        assert!(!temp.path().join(RESULT_FILE).exists());
    }

    #[test]
    fn validation_reads_do_not_consume_the_descriptor_transferred_to_the_worker() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(temp.path(), b"durable intent").unwrap();
        let descriptor: OwnedFd = File::open(temp.path()).unwrap().into();

        assert_eq!(read_bounded(&descriptor, 1024).unwrap(), b"durable intent");
        let mut transferred = File::from(descriptor);
        let mut bytes = Vec::new();
        transferred.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"durable intent");
    }

    #[test]
    fn registration_modes_are_explicit_and_mutually_exclusive() {
        let root: WorkRegistration = serde_json::from_str(r#"{"kind":"root"}"#).unwrap();
        let nested: WorkRegistration = serde_json::from_str(
            r#"{"kind":"nested","parent_work_id":"parent","parent_capability":"secret"}"#,
        )
        .unwrap();
        assert!(matches!(root, WorkRegistration::Root));
        assert!(matches!(nested, WorkRegistration::Nested { .. }));
        assert!(
            serde_json::from_str::<WorkRegistration>(
                r#"{"kind":"nested","parent_work_id":"parent"}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<WorkRegistration>(
                r#"{"kind":"root","parent_work_id":"ambiguous","parent_capability":"secret"}"#
            )
            .is_err()
        );
        let receipt = RegistrationReceipt::from(&nested);
        let receipt = serde_json::to_string(&receipt).unwrap();
        assert!(receipt.contains("parent_capability_sha256"));
        assert!(!receipt.contains("secret"));
    }

    #[test]
    fn protocol_has_no_deadline_or_recovery_scan_contract() {
        let source = include_str!("original_work.rs");
        assert!(!source.contains(&["read_", "timeout"].concat()));
        assert!(!source.contains(&["CONTROL_", "READ_TIMEOUT"].concat()));
        assert!(!source.contains(&["SEL", "ECT "].concat()));
    }

    #[test]
    fn fresh_root_is_unique_per_live_context_and_inheritance_reuses_it() {
        let context = identity(i64::from(std::process::id())).unwrap();
        let owner = owner(context.clone());
        let mut authorities = RootAuthorities::default();
        let first = authorities.fresh(&owner, context.clone()).unwrap();
        let inherited = authorities
            .inherit(&owner, context.clone(), &first)
            .unwrap();
        assert_eq!(inherited.root_id, first.root_id);
        assert_eq!(inherited.capability, first.capability);

        assert_eq!(
            authorities.fresh(&owner, context).unwrap_err(),
            "live context is already inside a root authority"
        );
    }

    #[test]
    fn capability_protocol_and_exact_peer_mismatches_are_rejected() {
        let context = identity(i64::from(std::process::id())).unwrap();
        let owner = owner(context.clone());
        let mut authorities = RootAuthorities::default();
        let grant = authorities.fresh(&owner, context.clone()).unwrap();

        let mut wrong_protocol = grant.clone();
        wrong_protocol.protocol = "original-work-v0".into();
        assert!(
            authorities
                .authorize_capability(&owner, &wrong_protocol)
                .is_err()
        );

        let mut wrong_control = grant.clone();
        wrong_control.control_protocol = SOURCE_CONTROL_PROTOCOL.into();
        assert!(
            authorities
                .authorize_capability(&owner, &wrong_control)
                .is_err()
        );

        let mut wrong_capability = grant.clone();
        wrong_capability.capability.push('x');
        assert!(
            authorities
                .authorize_capability(&owner, &wrong_capability)
                .is_err()
        );

        let mut stale_peer = context;
        stale_peer.starttime_ticks = stale_peer.starttime_ticks.saturating_add(1);
        assert!(
            authorities
                .authorize_root(&owner, &stale_peer, &grant)
                .is_err()
        );
        let parent_capability_hash = digest(b"parent-secret");
        assert!(capability_matches(&parent_capability_hash, "parent-secret"));
        assert!(!capability_matches(
            &parent_capability_hash,
            "sibling-secret"
        ));
    }

    #[test]
    fn exclusive_acceptance_and_cancellation_artifacts_are_idempotent_not_replayed() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(temp.path());
        let identity = identity(i64::from(std::process::id())).unwrap();
        let acceptance = AcceptanceReceipt {
            protocol: PROTOCOL.into(),
            work_id: "work-1".into(),
            request_sha256: "digest".into(),
            root_id: "root-1".into(),
            supervisor_authority_id: "authority-1".into(),
            owner_generation: "generation-1".into(),
            initiator: identity.clone(),
            registration: RegistrationReceipt::Root,
            cancel_capability_sha256: hex_digest(b"cancel-secret"),
        };
        create_artifact(&directory, ACCEPTED_FILE, &acceptance).unwrap();
        assert!(
            create_artifact(&directory, ACCEPTED_FILE, &acceptance)
                .unwrap_err()
                .starts_with("exists:")
        );

        let cancellation = CancellationReceipt {
            protocol: PROTOCOL.into(),
            work_id: "work-1".into(),
            root_id: "root-1".into(),
            cause: "explicit_request".into(),
            requester: Some(identity),
        };
        persist_cancellation(&directory, &cancellation).unwrap();
        persist_cancellation(&directory, &cancellation).unwrap();
        let mut conflict = cancellation;
        conflict.root_id = "root-2".into();
        assert!(persist_cancellation(&directory, &conflict).is_err());
    }

    #[test]
    fn parent_settlement_waits_for_every_admitted_child_outcome() {
        let children = BTreeSet::from(["child-a".to_owned(), "child-b".to_owned()]);
        let mut outcomes =
            BTreeMap::from([("child-a".to_owned(), true), ("child-b".to_owned(), false)]);
        assert!(!causal_children_terminal(&children, &outcomes));
        outcomes.insert("child-b".into(), true);
        assert!(causal_children_terminal(&children, &outcomes));
    }

    #[test]
    fn abnormal_parent_worker_exit_cannot_publish_before_child_result() {
        let parent_dir = tempfile::tempdir().unwrap();
        let child_dir = tempfile::tempdir().unwrap();
        let (parent_control, parent_peer) = UnixStream::pair().unwrap();
        let (child_control, _child_peer) = UnixStream::pair().unwrap();
        parent_control.set_nonblocking(true).unwrap();
        child_control.set_nonblocking(true).unwrap();
        let mut parent =
            test_operation(open_directory(parent_dir.path()), parent_control, "parent");
        let mut child = test_operation(open_directory(child_dir.path()), child_control, "child");
        child.worker.as_mut().unwrap().wait().unwrap();
        child.worker = Some(Command::new("sleep").arg("10").spawn().unwrap());
        child.worker_session = i64::from(child.worker.as_ref().unwrap().id());
        child.submission.root_authority = parent.submission.root_authority.clone();
        child.intent.root_id = parent.intent.root_id.clone();
        child.submission.registration = WorkRegistration::Nested {
            parent_work_id: "parent".into(),
            parent_capability: "child-secret".into(),
        };
        parent.children.insert("child".into());
        drop(parent_peer);
        let owner = owner(identity(i64::from(std::process::id())).unwrap());
        let mut supervisor = OriginalWorkSupervisor {
            active: vec![parent, child],
            never_forked: Vec::new(),
            adopted_child_live: false,
            kernel_pinned: false,
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !supervisor.active[0].terminal_observed {
            supervisor.tick(&owner);
            assert!(Instant::now() < deadline, "parent worker did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        let premature = parent_dir.path().join(RESULT_FILE).exists();
        supervisor.active[1]
            .worker
            .as_mut()
            .unwrap()
            .kill()
            .unwrap();
        assert!(
            !premature,
            "parent result preceded the accepted child outcome"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while !parent_dir.path().join(RESULT_FILE).exists() {
            supervisor.tick(&owner);
            assert!(
                Instant::now() < deadline,
                "parent result did not follow child result"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(child_dir.path().join(RESULT_FILE).exists());
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(parent_dir.path().join(RESULT_FILE)).unwrap())
                .unwrap();
        assert_eq!(result["causal_children"], serde_json::json!(["child"]));
    }

    #[test]
    fn preaccept_and_effects_possible_failures_are_not_conflated() {
        assert_eq!(submit_failure_status(false), "rejected_preaccept");
        assert_eq!(submit_failure_status(true), "effects_possible_no_replay");
    }

    #[test]
    fn cancellation_wins_before_grant_and_remains_routable_after_grant() {
        assert_eq!(
            next_execution_control(true, false, false, Some("explicit_request")),
            Some(b'C')
        );
        assert_eq!(
            next_execution_control(true, false, false, Some("owner_exit")),
            Some(b'O')
        );
        assert_eq!(next_execution_control(true, false, false, None), Some(b'G'));
        assert_eq!(
            next_execution_control(true, true, false, Some("explicit_request")),
            Some(b'C')
        );
        assert_eq!(
            next_execution_control(true, true, true, Some("explicit_request")),
            None
        );
    }

    #[test]
    fn complete_terminal_frame_is_applied_before_same_read_eof() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(temp.path());
        let (control, mut worker) = UnixStream::pair().unwrap();
        control.set_nonblocking(true).unwrap();
        worker.write_all(b"T").unwrap();
        drop(worker);
        let mut operation = test_operation(directory, control, "frame-eof");

        observe_control(&mut operation);

        assert!(operation.control_closed);
        assert!(operation.admission_closed);
        assert!(operation.causal_terminal);
        assert!(operation.detail.is_none());
        operation.worker.as_mut().unwrap().wait().unwrap();
    }

    #[test]
    fn cancellation_receipt_failure_stays_pending_without_effect_then_retries() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(temp.path());
        let (control, _worker_control) = UnixStream::pair().unwrap();
        control.set_nonblocking(true).unwrap();
        let operation = test_operation(directory, control, "cancel-pending");
        let conflict = CancellationReceipt {
            protocol: PROTOCOL.into(),
            work_id: "different-work".into(),
            root_id: "different-root".into(),
            cause: "explicit_request".into(),
            requester: None,
        };
        create_artifact(&operation.state_dir, CANCEL_FILE, &conflict).unwrap();
        let mut supervisor = OriginalWorkSupervisor {
            active: vec![operation],
            never_forked: Vec::new(),
            adopted_child_live: false,
            kernel_pinned: false,
        };
        let requester = identity(i64::from(std::process::id())).unwrap();
        let now = Instant::now();

        assert!(!supervisor.stage_cancellation(
            0,
            "explicit_request",
            Some(requester.clone()),
            now,
        ));
        assert!(supervisor.active[0].cancellation_cause.is_none());
        assert!(!supervisor.active[0].cancellation_sent);
        assert!(supervisor.active[0].cancellation_pending.is_some());
        let due = supervisor.active[0]
            .cancellation_pending
            .as_ref()
            .unwrap()
            .retry
            .retry_at;
        supervisor.retry_pending_cancellations(now);
        assert!(supervisor.active[0].cancellation_cause.is_none());
        std::fs::remove_file(temp.path().join(CANCEL_FILE)).unwrap();
        supervisor.retry_pending_cancellations(due);
        assert_eq!(
            supervisor.active[0].cancellation_cause.as_deref(),
            Some("explicit_request")
        );
        let persisted: CancellationReceipt =
            serde_json::from_slice(&std::fs::read(temp.path().join(CANCEL_FILE)).unwrap()).unwrap();
        assert_eq!(persisted.requester, Some(requester));
        supervisor.active[0]
            .worker
            .as_mut()
            .unwrap()
            .wait()
            .unwrap();
    }

    #[test]
    fn exceptional_grant_loss_waits_for_receipt_before_control_or_result() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(temp.path());
        let (control, mut worker_control) = UnixStream::pair().unwrap();
        control.set_nonblocking(true).unwrap();
        worker_control.set_nonblocking(true).unwrap();
        let mut operation = test_operation(directory, control, "grant-loss");
        operation.grant_sent = false;
        let conflict = CancellationReceipt {
            protocol: PROTOCOL.into(),
            work_id: "different-work".into(),
            root_id: "different-root".into(),
            cause: "explicit_request".into(),
            requester: None,
        };
        create_artifact(&operation.state_dir, CANCEL_FILE, &conflict).unwrap();
        let now = Instant::now();
        assert!(!OriginalWorkSupervisor::stage_operation_cancellation(
            &mut operation,
            "grant_transport_lost",
            None,
            now,
        ));
        assert!(!grant_allowed(&operation));
        let owner = owner(identity(i64::from(std::process::id())).unwrap());
        let mut supervisor = OriginalWorkSupervisor {
            active: vec![operation],
            never_forked: Vec::new(),
            adopted_child_live: false,
            kernel_pinned: false,
        };
        supervisor.tick_at(&owner, now);
        assert!(supervisor.active[0].cancellation_cause.is_none());
        assert!(!temp.path().join(RESULT_FILE).exists());
        assert_eq!(
            worker_control.read(&mut [0_u8; 1]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
        );
        std::fs::remove_file(temp.path().join(CANCEL_FILE)).unwrap();
        let due = supervisor.active[0]
            .cancellation_pending
            .as_ref()
            .unwrap()
            .retry
            .retry_at;
        supervisor.retry_pending_cancellations(due);
        assert_eq!(
            supervisor.active[0].cancellation_cause.as_deref(),
            Some("grant_transport_lost")
        );
        let persisted: CancellationReceipt =
            serde_json::from_slice(&std::fs::read(temp.path().join(CANCEL_FILE)).unwrap()).unwrap();
        assert_eq!(persisted.cause, "grant_transport_lost");
    }

    #[test]
    fn public_cancel_requires_exact_handle_capability_and_persists_actual_principal() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(temp.path());
        let (control, _worker_control) = UnixStream::pair().unwrap();
        control.set_nonblocking(true).unwrap();
        let operation = test_operation(directory, control, "public-cancel");
        let root_id = operation.submission.root_authority.root_id.clone();
        let principal = identity(i64::from(std::process::id())).unwrap();
        let domain = owner(principal.clone());
        let mut supervisor = OriginalWorkSupervisor {
            active: vec![operation],
            never_forked: Vec::new(),
            adopted_child_live: false,
            kernel_pinned: false,
        };

        let (bad_socket, mut bad_reply) = UnixStream::pair().unwrap();
        supervisor.cancel(
            &domain,
            InboundCancel {
                socket: bad_socket,
                peer: principal.clone(),
                submission: CancelSubmission {
                    protocol: PROTOCOL.into(),
                    root_id: root_id.clone(),
                    supervisor_authority_id: domain.supervisor_authority_id.clone(),
                    work_id: "public-cancel".into(),
                    cancel_capability: "wrong-secret".into(),
                },
            },
        );
        let rejected: WorkResponse = serde_json::from_reader(&mut bad_reply).unwrap();
        assert_eq!(rejected.status, "rejected");
        assert!(supervisor.active[0].cancellation_cause.is_none());

        let (socket, mut reply) = UnixStream::pair().unwrap();
        supervisor.cancel(
            &domain,
            InboundCancel {
                socket,
                peer: principal.clone(),
                submission: CancelSubmission {
                    protocol: PROTOCOL.into(),
                    root_id,
                    supervisor_authority_id: domain.supervisor_authority_id.clone(),
                    work_id: "public-cancel".into(),
                    cancel_capability: "cancel-secret".into(),
                },
            },
        );
        let accepted: WorkResponse = serde_json::from_reader(&mut reply).unwrap();
        assert_eq!(accepted.status, "cancellation_accepted");
        let receipt: CancellationReceipt =
            serde_json::from_slice(&std::fs::read(temp.path().join(CANCEL_FILE)).unwrap()).unwrap();
        assert_eq!(receipt.cause, "explicit_request");
        assert_eq!(receipt.requester, Some(principal));

        let mut different_principal = receipt.requester.clone().unwrap();
        different_principal.pid += 1;
        different_principal.starttime_ticks += 1;
        let (repeat_socket, mut repeat_reply) = UnixStream::pair().unwrap();
        supervisor.cancel(
            &domain,
            InboundCancel {
                socket: repeat_socket,
                peer: different_principal,
                submission: CancelSubmission {
                    protocol: PROTOCOL.into(),
                    root_id: receipt.root_id.clone(),
                    supervisor_authority_id: domain.supervisor_authority_id.clone(),
                    work_id: "public-cancel".into(),
                    cancel_capability: "cancel-secret".into(),
                },
            },
        );
        let rejected_repeat: WorkResponse = serde_json::from_reader(&mut repeat_reply).unwrap();
        assert_eq!(rejected_repeat.status, "rejected");
        let unchanged: CancellationReceipt =
            serde_json::from_slice(&std::fs::read(temp.path().join(CANCEL_FILE)).unwrap()).unwrap();
        assert_eq!(unchanged, receipt);
        supervisor.active[0]
            .worker
            .as_mut()
            .unwrap()
            .wait()
            .unwrap();
    }

    #[test]
    fn result_projection_distinguishes_terminal_cancellation_failure_and_root_loss() {
        let snapshot = |state: &str, reason: &str| TerminalSnapshot {
            state: Some(state.into()),
            completion_reason: Some(reason.into()),
            ..TerminalSnapshot::default()
        };
        assert_eq!(
            projected_result_outcome("terminal", &snapshot("DONE", "exit"), None),
            "terminal"
        );
        assert_eq!(
            projected_result_outcome("terminal", &snapshot("DONE", "cancel-request"), None),
            "cancelled_explicit"
        );
        assert_eq!(
            projected_result_outcome("terminal", &snapshot("DONE", "owner-exit"), None),
            "cancelled_owner_exit"
        );
        assert_eq!(
            projected_result_outcome(
                "terminal",
                &snapshot("DONE", "causal-parent-cancelled"),
                None
            ),
            "cancelled_causal_parent"
        );
        assert_eq!(
            projected_result_outcome("terminal", &snapshot("ERROR", "root-authority-lost"), None),
            "root_lost"
        );
        assert_eq!(
            projected_result_outcome("terminal", &snapshot("ERROR", "supervisor-error"), None),
            "failed"
        );
        assert_eq!(
            projected_result_outcome(
                "terminal",
                &snapshot("DONE", "exit"),
                Some("worker exited before causal terminal"),
            ),
            "failed"
        );
    }

    #[test]
    fn retry_schedule_is_per_obligation_exponential_and_bounded() {
        let start = Instant::now();
        let mut retry = RetrySchedule {
            failures: 0,
            retry_at: start,
        };
        let mut now = start;
        let mut previous = Duration::ZERO;
        for _ in 0..12 {
            retry.record_failure(now);
            let delay = retry.retry_at.duration_since(now);
            assert!(delay >= previous);
            assert!(delay <= RETRY_MAX);
            previous = delay;
            assert!(!retry.due(now));
            now = retry.retry_at;
            assert!(retry.due(now));
        }
        assert_eq!(previous, RETRY_MAX);
    }

    #[test]
    fn inactive_root_contexts_retire_after_no_context_or_operation_retains_them() {
        let context = identity(i64::from(std::process::id())).unwrap();
        let domain = owner(context.clone());
        let mut authorities = RootAuthorities::default();
        let grant = authorities.fresh(&domain, context).unwrap();
        assert!(authorities.scopes.contains_key(&grant.root_id));
        authorities.retain_live(&[], &BTreeSet::new());
        assert!(authorities.scopes.is_empty());
    }

    #[test]
    fn root_artifacts_are_handle_local_for_existing_retention_ownership() {
        for artifact in [ACCEPTED_FILE, CANCEL_FILE, RESULT_FILE] {
            assert_eq!(std::path::Path::new(artifact).components().count(), 1);
        }
    }

    #[test]
    fn terminal_result_is_idempotent_only_for_the_private_root_projection() {
        let temp = tempfile::tempdir().unwrap();
        let directory = open_directory(temp.path());
        let intent = IntentIdentity {
            protocol: PROTOCOL.into(),
            work_id: "work-1".into(),
            root_id: "root-1".into(),
            handle: "work-1".into(),
            state_root: temp.path().to_path_buf(),
            meta: IntentMetaIdentity {
                cwd: temp.path().to_path_buf(),
            },
            cancel_owner: None,
        };
        let children = BTreeSet::from(["child-1".to_owned()]);

        write_result(
            &directory,
            &intent,
            "private-nonce",
            &children,
            "terminal",
            Some(0),
            None,
        )
        .unwrap();
        write_result(
            &directory,
            &intent,
            "private-nonce",
            &children,
            "terminal",
            Some(0),
            None,
        )
        .unwrap();
        assert!(
            write_result(
                &directory,
                &intent,
                "forged-nonce",
                &children,
                "terminal",
                Some(0),
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn accepted_never_forked_work_keeps_root_alive_until_result_persists() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("meta.json"),
            br#"{"state":"RUNNING","rc":null,"signal":null,"completion_reason":null}"#,
        )
        .unwrap();
        let directory = open_directory(temp.path());
        let process = identity(i64::from(std::process::id())).unwrap();
        let owner = owner(process.clone());
        let mut authorities = RootAuthorities::default();
        let grant = authorities.fresh(&owner, process).unwrap();
        let intent = IntentIdentity {
            protocol: PROTOCOL.into(),
            work_id: "never-forked".into(),
            root_id: grant.root_id.clone(),
            handle: "never-forked".into(),
            state_root: temp.path().to_path_buf(),
            meta: IntentMetaIdentity {
                cwd: temp.path().to_path_buf(),
            },
            cancel_owner: None,
        };
        let mut supervisor = OriginalWorkSupervisor::default();
        supervisor.never_forked.push(NeverForkedOperation {
            submission: WorkSubmission {
                protocol: PROTOCOL.into(),
                root_authority: grant,
                work_id: "never-forked".into(),
                request_sha256: "digest".into(),
                registration: WorkRegistration::Root,
                cancel_capability: "cancel-secret".into(),
            },
            intent,
            state_dir: directory,
            result_nonce: "private-result".into(),
            detail: "spawn failed".into(),
            handle_terminalized: false,
            result_written: false,
            result_failure_recorded: false,
            result_retry: RetrySchedule::default(),
        });

        assert!(!supervisor.is_empty());
        supervisor.tick(&owner);
        assert!(supervisor.is_empty());
        assert!(temp.path().join(RESULT_FILE).exists());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("rc")).unwrap(),
            "70\n"
        );
        let meta: serde_json::Value =
            serde_json::from_slice(&std::fs::read(temp.path().join("meta.json")).unwrap()).unwrap();
        assert_eq!(meta["state"], "ERROR");
        assert_eq!(meta["completion_reason"], "root-worker-never-forked");
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(temp.path().join(RESULT_FILE)).unwrap()).unwrap();
        assert_eq!(result["outcome"], "never_forked");
        assert_eq!(result["agent_state"], "ERROR");
        assert_eq!(result["agent_rc"], 70);
    }
}
