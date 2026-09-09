//! Durable provider-launch ownership. No provider work or cross-store mutations occur here.
//! ## Declared roles
//! orchestration, validator, accessor, mapper

use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLaunchOwnerFence {
    pub logical_launch_id: Uuid,
    pub attempt_id: Uuid,
    pub owner_epoch: u64,
    pub invocation_row_id: i64,
    pub invocation_uuid: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub enum InvocationMutationAuthority<'a> {
    Standalone,
    ProviderLaunch(&'a ProviderLaunchOwnerFence),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLaunchCandidate {
    pub provider_index: usize,
    pub account_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLaunchStartMode {
    Create,
    Resume,
}

/// Allocate once and retain in the live caller across retries. Never persist the secret.
#[derive(Debug, Clone)]
pub struct ProviderLaunchAttemptAllocation {
    pub attempt_id: Uuid,
    pub invocation_uuid: Uuid,
    pub runtime_generation_uuid: Uuid,
    pub completion_authority: CompletionRegistrationAuthority,
}
impl ProviderLaunchAttemptAllocation {
    pub fn allocate() -> Result<Self, String> {
        Ok(Self {
            attempt_id: Uuid::new_v4(),
            invocation_uuid: Uuid::new_v4(),
            runtime_generation_uuid: Uuid::new_v4(),
            completion_authority: CompletionRegistrationAuthority::generate()?,
        })
    }
    fn identity(&self) -> serde_json::Value {
        serde_json::json!([
            self.attempt_id,
            self.invocation_uuid,
            self.runtime_generation_uuid,
            self.completion_authority.digest()
        ])
    }
}

#[derive(Debug, Clone)]
pub struct BeginProviderLaunchRequest {
    pub logical_launch_id: Uuid,
    pub request_identity_sha256: String,
    pub model_name: String,
    pub start_mode: ProviderLaunchStartMode,
    pub expected_provider_session_id: Option<String>,
    pub candidates: Vec<ProviderLaunchCandidate>,
    pub parent_invocation_id: Option<i64>,
    pub allocation: ProviderLaunchAttemptAllocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLaunchLease {
    pub owner: ProviderLaunchOwnerFence,
    pub candidate: ProviderLaunchCandidate,
    pub candidate_plan_sha256: String,
    pub runtime_generation_uuid: Uuid,
    pub return_channel_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLaunchEndpoint {
    pub endpoint_family: String,
    pub settings_id: String,
    pub provider_instance_id: String,
    pub endpoint_identity_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLaunchPromotion {
    ProviderSessionObserved,
    PromptAccepted,
    AssistantResponseObserved,
    CapturedChild,
    ReturnedArtifact,
    MailboxSubmissionAccepted,
}
impl ProviderLaunchPromotion {
    fn column(self) -> &'static str {
        match self {
            Self::ProviderSessionObserved => "provider_session_observed",
            Self::PromptAccepted => "prompt_accepted",
            Self::AssistantResponseObserved => "assistant_response_observed",
            Self::CapturedChild => "captured_child_count",
            Self::ReturnedArtifact => "returned_artifact_count",
            Self::MailboxSubmissionAccepted => "mailbox_submission_accepted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotatableLaunchFailureKind {
    HostTimeout,
    ProviderUnavailable,
    ProviderTimeout,
}
impl RotatableLaunchFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::HostTimeout => "host_timeout",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::ProviderTimeout => "provider_timeout",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLaunchFailureRecord {
    pub kind: RotatableLaunchFailureKind,
    pub request_id: Uuid,
    pub code: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLaunchActorSettlement {
    NeverSpawned {
        operation: String,
    },
    Reaped {
        operation: String,
        process_identity_sha256: String,
        process_tree_terminated: bool,
        leader_reaped: bool,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLaunchChannelSettlement {
    NotCreated,
    EmptyRemoved,
}

/// Immutable receipts supplied by the custody owner, not observations reconstructed from PIDs.
/// LT-02 owns producing these from process/runtime/channel evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLaunchCustodyProof {
    pub attempt_id: Uuid,
    pub runtime_generation_uuid: Uuid,
    pub spawn_invocation_uuid: Uuid,
    pub actors: Vec<ProviderLaunchActorSettlement>,
    pub runtime_terminal_code: String,
    pub runtime_never_bound: bool,
    pub runtime_process_identity_sha256: Option<String>,
    pub runtime_exited: bool,
    pub active_delivery_claim: bool,
    pub runtime_settlement_sha256: String,
    pub return_channel_id: String,
    pub channel: ProviderLaunchChannelSettlement,
    pub return_channel_settlement_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLaunchTerminalResult {
    pub success: bool,
    pub exit_code: i32,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderLaunchRecoveryDisposition {
    Failed,
    Cancelled,
    RecoveryBlocked,
}

const ZERO_PROMOTION: &str = "provider_session_observed + prompt_accepted + assistant_response_observed + captured_child_count + returned_artifact_count + mailbox_submission_accepted = 0";
fn conflict() -> String {
    "conflicting_provider_launch_transition".into()
}
fn sql_error(e: sqlite::Error) -> String {
    format!("provider launch persistence: {e}")
}
fn digest(value: &impl Serialize) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    hash.update(b"oulipoly-provider-launch-v1\0");
    hash.update(bytes);
    Ok(format!("{:x}", hash.finalize()))
}
fn valid_digest(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err("invalid provider launch digest".into())
    }
}
fn bounded(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err("invalid bounded provider launch identity/code".into())
    } else {
        Ok(())
    }
}
fn immediate(conn: &sqlite::Connection) -> Result<sqlite::Transaction<'_>, String> {
    sqlite::Transaction::new_unchecked(conn, sqlite::TransactionBehavior::Immediate)
        .map_err(sql_error)
}

/// Exact fence validation must run within the writer's transaction, never before acquiring it.
pub(super) fn validate_mutation_authority(
    conn: &sqlite::Connection,
    row_id: i64,
    authority: InvocationMutationAuthority<'_>,
) -> Result<(), String> {
    let linked: Option<(String, String, i64, String, String, i64, String)> = conn
        .query_row(
            "SELECT a.logical_launch_id,a.attempt_id,a.owner_epoch,a.invocation_uuid,
         l.current_attempt_id,l.owner_epoch,i.invocation_uuid FROM invocations i
         JOIN provider_launch_attempts a ON a.invocation_id=i.id
         JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id WHERE i.id=?1",
            [row_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(sql_error)?;
    match (linked, authority) {
        (None, InvocationMutationAuthority::Standalone) => Ok(()),
        (
            Some((launch, attempt, epoch, uuid, current, current_epoch, actual_uuid)),
            InvocationMutationAuthority::ProviderLaunch(f),
        ) if f.invocation_row_id == row_id
            && launch == f.logical_launch_id.to_string()
            && attempt == f.attempt_id.to_string()
            && attempt == current
            && epoch == current_epoch
            && u64::try_from(epoch).ok() == Some(f.owner_epoch)
            && uuid == actual_uuid
            && uuid == f.invocation_uuid.to_string() =>
        {
            Ok(())
        }
        _ => Err("stale_or_missing_provider_launch_owner_fence".into()),
    }
}

fn replay<T: for<'de> Deserialize<'de>>(
    conn: &sqlite::Connection,
    launch: Uuid,
    key: &str,
    hash: &str,
) -> Result<Option<T>, String> {
    let row: Option<(String,String)> = conn.query_row(
        "SELECT request_sha256,result_json FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key=?2",
        params![launch.to_string(),key], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(sql_error)?;
    match row {
        None => Ok(None),
        Some((old, result)) if old == hash => serde_json::from_str(&result)
            .map(Some)
            .map_err(|e| e.to_string()),
        Some(_) => Err(conflict()),
    }
}
fn remember(
    conn: &sqlite::Connection,
    launch: Uuid,
    key: &str,
    hash: &str,
    result: &impl Serialize,
) -> Result<(), String> {
    let json = serde_json::to_string(result).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO provider_launch_transition_replays VALUES (?1,?2,?3,?4)",
        params![launch.to_string(), key, hash, json],
    )
    .map_err(sql_error)?;
    Ok(())
}

impl StateDb {
    pub fn begin_launch(
        &self,
        request: &BeginProviderLaunchRequest,
    ) -> Result<ProviderLaunchLease, String> {
        valid_digest(&request.request_identity_sha256)?;
        bounded(&request.model_name)?;
        if request.candidates.is_empty()
            || request.logical_launch_id.is_nil()
            || (request.start_mode == ProviderLaunchStartMode::Resume
                && request.candidates.len() != 1)
        {
            return Err(conflict());
        }
        let mut accounts = std::collections::HashSet::new();
        let mut indexes = std::collections::HashSet::new();
        for candidate in &request.candidates {
            bounded(&candidate.account_name)?;
            if !accounts.insert(&candidate.account_name)
                || !indexes.insert(candidate.provider_index)
            {
                return Err(conflict());
            }
        }
        if request.candidates[1..]
            .windows(2)
            .any(|pair| pair[0].provider_index >= pair[1].provider_index)
        {
            return Err("provider launch siblings must retain model declaration order".into());
        }
        if request.start_mode == ProviderLaunchStartMode::Resume {
            bounded(
                request
                    .expected_provider_session_id
                    .as_deref()
                    .ok_or_else(conflict)?,
            )?;
        }
        let plan = serde_json::to_string(&request.candidates).map_err(|e| e.to_string())?;
        let plan_hash = digest(&request.candidates)?;
        let hash = digest(&serde_json::json!([
            request.logical_launch_id,
            request.request_identity_sha256,
            request.model_name,
            request.start_mode,
            request.expected_provider_session_id,
            request.candidates,
            request.parent_invocation_id,
            request.allocation.identity()
        ]))?;
        let tx = immediate(&self.conn)?;
        if let Some(lease) = replay(&tx, request.logical_launch_id, "begin", &hash)? {
            return Ok(lease);
        }
        let now = Self::current_rfc3339_timestamp();
        let mode = match request.start_mode {
            ProviderLaunchStartMode::Create => "create",
            ProviderLaunchStartMode::Resume => "resume",
        };
        tx.execute("INSERT INTO provider_logical_launches (logical_launch_id,request_identity_sha256,model_name,start_mode,
            expected_provider_session_id,candidate_plan_json,candidate_plan_sha256,status,current_attempt_id,owner_epoch,created_at,updated_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,'active',?8,1,?9,?9)",params![request.logical_launch_id.to_string(),request.request_identity_sha256,
            request.model_name,mode,request.expected_provider_session_id,plan,plan_hash,request.allocation.attempt_id.to_string(),now]).map_err(sql_error)?;
        let lease = Self::insert_launch_attempt(
            &tx,
            request.logical_launch_id,
            0,
            &request.model_name,
            &request.candidates[0],
            &plan_hash,
            request.parent_invocation_id,
            &request.allocation,
            &now,
        )?;
        remember(&tx, request.logical_launch_id, "begin", &hash, &lease)?;
        tx.commit().map_err(sql_error)?;
        Ok(lease)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_launch_attempt(
        conn: &sqlite::Connection,
        launch: Uuid,
        ordinal: i64,
        model: &str,
        candidate: &ProviderLaunchCandidate,
        plan_hash: &str,
        parent: Option<i64>,
        allocation: &ProviderLaunchAttemptAllocation,
        now: &str,
    ) -> Result<ProviderLaunchLease, String> {
        if allocation.attempt_id.is_nil()
            || allocation.invocation_uuid.is_nil()
            || allocation.runtime_generation_uuid.is_nil()
        {
            return Err(conflict());
        }
        let start = InvocationStart {
            invocation_uuid: allocation.invocation_uuid.to_string(),
            model_name: model.into(),
            provider_name: candidate.account_name.clone(),
            provider_index: candidate.provider_index,
            parent_invocation_id: parent,
        };
        let id = Self::insert_invocation_start_on(
            conn,
            &start,
            now,
            Some(&allocation.completion_authority.digest()),
        )
        .map_err(sql_error)?;
        let channel = format!("{launch}/{}", allocation.attempt_id);
        conn.execute("INSERT INTO provider_launch_attempts (attempt_id,logical_launch_id,attempt_ordinal,owner_epoch,invocation_id,invocation_uuid,
            provider_index,account_name,runtime_generation_uuid,return_channel_id,status,actor_custody_state,return_channel_state,created_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'leased','not_started','not_created',?11)",params![allocation.attempt_id.to_string(),launch.to_string(),ordinal,
            ordinal+1,id,allocation.invocation_uuid.to_string(),i64::try_from(candidate.provider_index).map_err(|_| conflict())?,candidate.account_name,
            allocation.runtime_generation_uuid.to_string(),channel,now]).map_err(sql_error)?;
        Ok(ProviderLaunchLease {
            owner: ProviderLaunchOwnerFence {
                logical_launch_id: launch,
                attempt_id: allocation.attempt_id,
                owner_epoch: (ordinal + 1) as u64,
                invocation_row_id: id,
                invocation_uuid: allocation.invocation_uuid,
            },
            candidate: candidate.clone(),
            candidate_plan_sha256: plan_hash.into(),
            runtime_generation_uuid: allocation.runtime_generation_uuid,
            return_channel_id: channel,
        })
    }

    pub fn activate_attempt(
        &self,
        lease: &ProviderLaunchLease,
        authority: &CompletionRegistrationAuthority,
    ) -> Result<(), String> {
        self.launch_transition(&lease.owner,"activate",&(lease,authority.digest()),|tx| {
            validate_retained_authority(tx,&lease.owner,authority)?;
            let changed = tx.execute("UPDATE provider_launch_attempts SET status='active',activated_at=?1 WHERE attempt_id=?2 AND status='leased'
                AND account_name=?3 AND provider_index=?4 AND runtime_generation_uuid=?5 AND return_channel_id=?6
                AND EXISTS(SELECT 1 FROM provider_logical_launches WHERE logical_launch_id=?7 AND candidate_plan_sha256=?8
                    AND status IN ('active','successor_leased') AND cancel_requested_at IS NULL)",params![Self::current_rfc3339_timestamp(),lease.owner.attempt_id.to_string(),
                    lease.candidate.account_name,lease.candidate.provider_index as i64,lease.runtime_generation_uuid.to_string(),lease.return_channel_id,
                    lease.owner.logical_launch_id.to_string(),lease.candidate_plan_sha256]).map_err(sql_error)?;
            require_one(changed)?;
            set_launch_status(tx,&lease.owner,"active",None)?;
            Ok(())
        })
    }

    /// Join accepted effect-writer promotions under the exact current fence.
    pub fn provider_launch_promotions(
        &self,
        owner: &ProviderLaunchOwnerFence,
    ) -> Result<Vec<ProviderLaunchPromotion>, String> {
        let tx = immediate(&self.conn)?;
        validate_mutation_authority(
            &tx,
            owner.invocation_row_id,
            InvocationMutationAuthority::ProviderLaunch(owner),
        )?;
        let values: [i64; 6] = tx.query_row("SELECT provider_session_observed,prompt_accepted,assistant_response_observed,captured_child_count,returned_artifact_count,mailbox_submission_accepted FROM provider_launch_attempts WHERE attempt_id=?1",
            [owner.attempt_id.to_string()], |r| Ok([r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?])).map_err(sql_error)?;
        Ok([
            ProviderLaunchPromotion::ProviderSessionObserved,
            ProviderLaunchPromotion::PromptAccepted,
            ProviderLaunchPromotion::AssistantResponseObserved,
            ProviderLaunchPromotion::CapturedChild,
            ProviderLaunchPromotion::ReturnedArtifact,
            ProviderLaunchPromotion::MailboxSubmissionAccepted,
        ]
        .into_iter()
        .zip(values)
        .filter_map(|(promotion, count)| (count > 0).then_some(promotion))
        .collect())
    }

    /// Readback only: consumes an already activated exact lease and retained
    /// authority. This never activates or allocates an attempt.
    pub fn validate_active_launch_attempt(
        &self,
        lease: &ProviderLaunchLease,
        authority: &CompletionRegistrationAuthority,
    ) -> Result<(), String> {
        let tx = immediate(&self.conn)?;
        validate_mutation_authority(
            &tx,
            lease.owner.invocation_row_id,
            InvocationMutationAuthority::ProviderLaunch(&lease.owner),
        )?;
        validate_retained_authority(&tx, &lease.owner, authority)?;
        let valid: bool = tx.query_row("SELECT a.status='active' AND l.status='active' AND l.cancel_requested_at IS NULL
            AND a.account_name=?2 AND a.provider_index=?3 AND a.runtime_generation_uuid=?4 AND a.return_channel_id=?5 AND l.candidate_plan_sha256=?6
            FROM provider_launch_attempts a JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id WHERE a.attempt_id=?1",
            params![lease.owner.attempt_id.to_string(),lease.candidate.account_name,lease.candidate.provider_index as i64,lease.runtime_generation_uuid.to_string(),lease.return_channel_id,lease.candidate_plan_sha256], |r| r.get(0)).map_err(sql_error)?;
        if !valid {
            return Err("inactive_or_changed_provider_launch_attempt".into());
        }
        Ok(())
    }

    pub fn bind_launch_endpoint(
        &self,
        owner: &ProviderLaunchOwnerFence,
        endpoint: &ProviderLaunchEndpoint,
    ) -> Result<(), String> {
        bounded(&endpoint.endpoint_family)?;
        bounded(&endpoint.settings_id)?;
        bounded(&endpoint.provider_instance_id)?;
        valid_digest(&endpoint.endpoint_identity_sha256)?;
        self.launch_transition(owner,"endpoint",endpoint,|tx| {
            require_one(tx.execute("UPDATE provider_launch_attempts SET endpoint_family=?1,settings_id=?2,provider_instance_id=?3,endpoint_identity_sha256=?4,
                actor_custody_state='active' WHERE attempt_id=?5 AND status='active' AND endpoint_family IS NULL",
                params![endpoint.endpoint_family,endpoint.settings_id,endpoint.provider_instance_id,endpoint.endpoint_identity_sha256,owner.attempt_id.to_string()]).map_err(sql_error)?)
        })
    }

    pub fn record_promotion(
        &self,
        owner: &ProviderLaunchOwnerFence,
        observation_id: Uuid,
        promotion: ProviderLaunchPromotion,
    ) -> Result<(), String> {
        self.launch_transition(owner,&format!("promotion/{observation_id}"),&promotion,|tx| {
            let column = promotion.column();
            let value = match promotion { ProviderLaunchPromotion::CapturedChild | ProviderLaunchPromotion::ReturnedArtifact => format!("{column}+1"), _ => "1".into() };
            require_one(tx.execute(&format!("UPDATE provider_launch_attempts SET {column}={value},effect_incapable_at=NULL,status='active'
                WHERE attempt_id=?1 AND status IN ('active','transfer_requested','effect_incapable')"),[owner.attempt_id.to_string()]).map_err(sql_error)?)?;
            tx.execute("UPDATE provider_logical_launches SET status='active',updated_at=?1 WHERE logical_launch_id=?2 AND status='transfer_requested'",
                params![Self::current_rfc3339_timestamp(),owner.logical_launch_id.to_string()]).map_err(sql_error)?;
            Ok(())
        })
    }

    pub fn request_transfer(
        &self,
        owner: &ProviderLaunchOwnerFence,
        failure: &ProviderLaunchFailureRecord,
    ) -> Result<(), String> {
        bounded(&failure.code)?;
        self.launch_transition(owner,"transfer",failure,|tx| {
            require_one(tx.execute(&format!("UPDATE provider_launch_attempts SET status='transfer_requested',rotatable_kind=?1,failure_request_id=?2,failure_code=?3
                WHERE attempt_id=?4 AND status='active' AND {ZERO_PROMOTION} AND EXISTS(SELECT 1 FROM invocations i WHERE i.id=provider_launch_attempts.invocation_id AND i.status='running') AND EXISTS(SELECT 1 FROM provider_logical_launches l
                 WHERE l.logical_launch_id=?5 AND l.status='active' AND l.start_mode='create' AND l.cancel_requested_at IS NULL
                 AND json_array_length(l.candidate_plan_json)>provider_launch_attempts.attempt_ordinal+1)"),params![failure.kind.as_str(),failure.request_id.to_string(),
                 failure.code,owner.attempt_id.to_string(),owner.logical_launch_id.to_string()]).map_err(sql_error)?)?;
            remember(tx,owner.logical_launch_id,&format!("{}/failure-record",owner.attempt_id),&digest(failure)?,failure)?;
            set_launch_status(tx,owner,"transfer_requested",None)
        })
    }

    pub fn certify_effect_incapable(
        &self,
        owner: &ProviderLaunchOwnerFence,
        proof: &ProviderLaunchCustodyProof,
    ) -> Result<(), String> {
        self.launch_transition(owner,"certify",proof,|tx| {
            validate_proof(tx,owner,proof)?;
            let actor_hash = digest(&proof.actors)?;
            let channel = match proof.channel { ProviderLaunchChannelSettlement::NotCreated => "not_created", ProviderLaunchChannelSettlement::EmptyRemoved => "empty_removed" };
            require_one(tx.execute(&format!("UPDATE provider_launch_attempts SET status='effect_incapable',actor_custody_state='effect_incapable',
                actor_settlement_sha256=?1,runtime_settlement_sha256=?2,return_channel_state=?3,return_channel_settlement_sha256=?4,effect_incapable_at=?5
                WHERE attempt_id=?6 AND status='transfer_requested' AND {ZERO_PROMOTION}
                AND EXISTS(SELECT 1 FROM provider_logical_launches WHERE logical_launch_id=?7 AND status='transfer_requested' AND cancel_requested_at IS NULL)"),
                params![actor_hash,proof.runtime_settlement_sha256,channel,proof.return_channel_settlement_sha256,Self::current_rfc3339_timestamp(),owner.attempt_id.to_string(),owner.logical_launch_id.to_string()]).map_err(sql_error)?)
        })
    }

    pub fn lease_successor(
        &self,
        predecessor: &ProviderLaunchOwnerFence,
        predecessor_authority: &CompletionRegistrationAuthority,
        plan_hash: &str,
        next: &ProviderLaunchCandidate,
        proof: &ProviderLaunchCustodyProof,
        allocation: &ProviderLaunchAttemptAllocation,
    ) -> Result<ProviderLaunchLease, String> {
        let hash = digest(&serde_json::json!([
            predecessor,
            plan_hash,
            next,
            proof,
            allocation.identity()
        ]))?;
        let key = format!("{}/successor", predecessor.attempt_id);
        let tx = immediate(&self.conn)?;
        validate_retained_authority(&tx, predecessor, predecessor_authority)?;
        // This is the sole stale-fence replay exception: immutable output only, never another lease.
        if let Some(lease) = replay(&tx, predecessor.logical_launch_id, &key, &hash)? {
            return Ok(lease);
        }
        validate_mutation_authority(
            &tx,
            predecessor.invocation_row_id,
            InvocationMutationAuthority::ProviderLaunch(predecessor),
        )?;
        validate_proof(&tx, predecessor, proof)?;
        let (model,plan,parent,ordinal): (String,String,Option<i64>,i64) = tx.query_row(
            &format!("SELECT l.model_name,l.candidate_plan_json,i.parent_invocation_id,a.attempt_ordinal FROM provider_logical_launches l
             JOIN provider_launch_attempts a ON a.attempt_id=l.current_attempt_id JOIN invocations i ON i.id=a.invocation_id
             WHERE l.logical_launch_id=?1 AND l.status='transfer_requested' AND l.start_mode='create' AND l.cancel_requested_at IS NULL
             AND l.candidate_plan_sha256=?2 AND a.status='effect_incapable' AND {ZERO_PROMOTION}
             AND a.actor_settlement_sha256=?3 AND a.runtime_settlement_sha256=?4 AND a.return_channel_settlement_sha256=?5"),
             params![predecessor.logical_launch_id.to_string(),plan_hash,digest(&proof.actors)?,proof.runtime_settlement_sha256,proof.return_channel_settlement_sha256],
             |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(sql_error)?;
        let candidates: Vec<ProviderLaunchCandidate> =
            serde_json::from_str(&plan).map_err(|e| e.to_string())?;
        if digest(&candidates)? != plan_hash || candidates.get((ordinal + 1) as usize) != Some(next)
        {
            return Err(conflict());
        }
        let failure_json: String = tx.query_row("SELECT result_json FROM provider_launch_transition_replays WHERE logical_launch_id=?1 AND operation_key=?2",
            params![predecessor.logical_launch_id.to_string(),format!("{}/failure-record",predecessor.attempt_id)],|r|r.get(0)).map_err(sql_error)?;
        let failure: ProviderLaunchFailureRecord =
            serde_json::from_str(&failure_json).map_err(|e| e.to_string())?;
        let kind = failure.kind.as_str();
        let code = &failure.code;
        let now = Self::current_rfc3339_timestamp();
        Self::write_invocation_final_row(
            &tx,
            predecessor.invocation_row_id,
            false,
            failure.exit_code,
            Some(kind),
            Some(code),
            &now,
        )?;
        Self::upsert_provider_finalize_aggregate(
            &tx,
            &model,
            Some(
                &tx.query_row(
                    "SELECT account_name FROM provider_launch_attempts WHERE attempt_id=?1",
                    [predecessor.attempt_id.to_string()],
                    |r| r.get::<_, String>(0),
                )
                .map_err(sql_error)?,
            ),
            false,
            Some(code),
            &now,
        )?;
        require_one(tx.execute("UPDATE provider_launch_attempts SET status='superseded',terminal_code=?1,finished_at=?2 WHERE attempt_id=?3 AND status='effect_incapable'",
            params![code,now,predecessor.attempt_id.to_string()]).map_err(sql_error)?)?;
        let lease = Self::insert_launch_attempt(
            &tx,
            predecessor.logical_launch_id,
            ordinal + 1,
            &model,
            next,
            plan_hash,
            parent,
            allocation,
            &now,
        )?;
        tx.execute("UPDATE provider_logical_launches SET status='successor_leased',current_attempt_id=?1,owner_epoch=owner_epoch+1,updated_at=?2 WHERE logical_launch_id=?3",
            params![allocation.attempt_id.to_string(),now,predecessor.logical_launch_id.to_string()]).map_err(sql_error)?;
        remember(&tx, predecessor.logical_launch_id, &key, &hash, &lease)?;
        tx.commit().map_err(sql_error)?;
        Ok(lease)
    }

    pub fn request_cancel(&self, launch: Uuid) -> Result<(), String> {
        let tx = immediate(&self.conn)?;
        if replay::<()>(&tx, launch, "cancel", &digest(&launch)?)?.is_some() {
            return Ok(());
        }
        require_one(tx.execute("UPDATE provider_logical_launches SET status='cancelling',cancel_requested_at=?1,updated_at=?1
            WHERE logical_launch_id=?2 AND status IN ('active','transfer_requested','successor_leased')",
            params![Self::current_rfc3339_timestamp(),launch.to_string()]).map_err(sql_error)?)?;
        remember(&tx, launch, "cancel", &digest(&launch)?, &())?;
        tx.commit().map_err(sql_error)
    }

    pub fn complete_launch(
        &self,
        owner: &ProviderLaunchOwnerFence,
        result: &ProviderLaunchTerminalResult,
    ) -> Result<(), String> {
        bounded(&result.code)?;
        self.launch_transition(owner,"complete",result,|tx| {
            let status = if result.success { "succeeded" } else { "failed" };
            let agrees: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM invocations i JOIN provider_launch_attempts a ON a.invocation_id=i.id
                JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id WHERE a.attempt_id=?1 AND a.status='active'
                AND l.status='active' AND l.cancel_requested_at IS NULL AND i.status=?2 AND i.success=?3 AND i.exit_code=?4 AND i.terminal_reason IS ?5)",
                params![owner.attempt_id.to_string(),status,result.success,result.exit_code,result.code],|r|r.get(0)).map_err(sql_error)?;
            if !agrees { return Err(conflict()); }
            terminalize(tx,owner,status,&result.code)
        })
    }

    pub fn settle_cancel(
        &self,
        owner: &ProviderLaunchOwnerFence,
        proof: &ProviderLaunchCustodyProof,
    ) -> Result<(), String> {
        self.launch_transition(owner,"settle_cancel",proof,|tx| {
            let cancelling: bool = tx.query_row("SELECT status='cancelling' FROM provider_logical_launches WHERE logical_launch_id=?1",
                [owner.logical_launch_id.to_string()],|r|r.get(0)).map_err(sql_error)?;
            if !cancelling { return Err(conflict()); }
            validate_proof(tx,owner,proof)?;
            finish_recovery_invocation(tx,owner,"cancelled")?;
            terminalize(tx,owner,"cancelled","cancelled")
        })
    }

    /// Same-owner non-executing settlement only. Uncertain joins must stay recovery-blocked.
    pub fn reconcile_incomplete(
        &self,
        owner: &ProviderLaunchOwnerFence,
        disposition: ProviderLaunchRecoveryDisposition,
        proof: Option<&ProviderLaunchCustodyProof>,
        join: &ProviderLaunchRecoveryJoin,
    ) -> Result<(), String> {
        valid_digest(&join.evidence_sha256)?;
        if join.state_db_path != self.db_path
            || join.sidecar_path != crate::mailbox::MailboxDb::path_for_state_db(&self.db_path)
        {
            return Err("provider_launch_recovery_store_identity_mismatch".into());
        }
        self.launch_transition(owner,&format!("reconcile/{disposition:?}"),&serde_json::json!([disposition,proof,join]),|tx| {
            let (status,cancel): (String,bool) = tx.query_row("SELECT status,cancel_requested_at IS NOT NULL FROM provider_logical_launches WHERE logical_launch_id=?1",
                [owner.logical_launch_id.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(sql_error)?;
            if matches!(status.as_str(),"succeeded"|"failed"|"cancelled") { return Err(conflict()); }
            let target = match disposition {
                ProviderLaunchRecoveryDisposition::RecoveryBlocked => "recovery_blocked",
                ProviderLaunchRecoveryDisposition::Failed if !cancel => "failed",
                ProviderLaunchRecoveryDisposition::Cancelled if cancel => "cancelled",
                _ => return Err(conflict()),
            };
            if target != "recovery_blocked" {
                validate_proof(tx,owner,proof.ok_or_else(conflict)?)?;
                finish_recovery_invocation(tx,owner,if cancel {"cancelled"} else {"recovered_before_transfer"})?;
            }
            terminalize(tx,owner,target,if target == "recovery_blocked" {"recovery_custody_uncertain"} else if cancel {"cancelled"} else {"recovered_before_transfer"})
        })
    }

    fn launch_transition<T: Serialize>(
        &self,
        owner: &ProviderLaunchOwnerFence,
        operation: &str,
        input: &T,
        apply: impl FnOnce(&sqlite::Transaction<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        let tx = immediate(&self.conn)?;
        validate_mutation_authority(
            &tx,
            owner.invocation_row_id,
            InvocationMutationAuthority::ProviderLaunch(owner),
        )?;
        if operation == "activate" || operation == "endpoint" {
            let executable: bool = tx.query_row("SELECT l.cancel_requested_at IS NULL AND l.status IN ('active','successor_leased')
                AND a.status IN ('leased','active') FROM provider_logical_launches l JOIN provider_launch_attempts a ON a.attempt_id=l.current_attempt_id
                WHERE l.logical_launch_id=?1",[owner.logical_launch_id.to_string()],|r|r.get(0)).map_err(sql_error)?;
            if !executable {
                return Err(conflict());
            }
        }
        let key = format!("{}/{operation}", owner.attempt_id);
        let hash = digest(&(owner, input))?;
        if replay::<()>(&tx, owner.logical_launch_id, &key, &hash)?.is_some() {
            return Ok(());
        }
        apply(&tx)?;
        remember(&tx, owner.logical_launch_id, &key, &hash, &())?;
        tx.commit().map_err(sql_error)
    }
}

fn require_one(changed: usize) -> Result<(), String> {
    if changed == 1 {
        Ok(())
    } else {
        Err(conflict())
    }
}
fn set_launch_status(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    status: &str,
    code: Option<&str>,
) -> Result<(), String> {
    require_one(conn.execute("UPDATE provider_logical_launches SET status=?1,terminal_code=?2,updated_at=?3 WHERE logical_launch_id=?4",
        params![status,code,StateDb::current_rfc3339_timestamp(),owner.logical_launch_id.to_string()]).map_err(sql_error)?)
}
fn terminalize(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    status: &str,
    code: &str,
) -> Result<(), String> {
    let timestamp = StateDb::current_rfc3339_timestamp();
    let now = if status == "recovery_blocked" {
        None
    } else {
        Some(timestamp)
    };
    conn.execute("UPDATE provider_launch_attempts SET status=?1,terminal_code=?2,finished_at=?3 WHERE attempt_id=?4",
        params![status,code,now,owner.attempt_id.to_string()]).map_err(sql_error)?;
    set_launch_status(conn, owner, status, Some(code))?;
    conn.execute(
        "UPDATE provider_logical_launches SET finished_at=?1 WHERE logical_launch_id=?2",
        params![now, owner.logical_launch_id.to_string()],
    )
    .map_err(sql_error)?;
    Ok(())
}
fn finish_recovery_invocation(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    code: &str,
) -> Result<(), String> {
    let row = StateDb::load_invocation_for_finalize(conn, owner.invocation_row_id)?;
    if row.status == "running" {
        let now = StateDb::current_rfc3339_timestamp();
        StateDb::write_invocation_final_row(
            conn,
            owner.invocation_row_id,
            false,
            1,
            Some(code),
            Some(code),
            &now,
        )?;
        StateDb::upsert_provider_finalize_aggregate(
            conn,
            &row.model_name,
            row.provider_name.as_deref(),
            false,
            Some(code),
            &now,
        )?;
    }
    Ok(())
}
fn validate_proof(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    proof: &ProviderLaunchCustodyProof,
) -> Result<(), String> {
    valid_digest(&proof.runtime_settlement_sha256)?;
    valid_digest(&proof.return_channel_settlement_sha256)?;
    bounded(&proof.runtime_terminal_code)?;
    if proof.attempt_id != owner.attempt_id
        || proof.spawn_invocation_uuid != owner.invocation_uuid
        || !proof.runtime_exited
        || proof.active_delivery_claim
        || proof.actors.is_empty()
    {
        return Err(conflict());
    }
    let mut operations = std::collections::HashSet::new();
    let mut launch_process_identity = None;
    for actor in &proof.actors {
        let operation = match actor {
            ProviderLaunchActorSettlement::NeverSpawned { operation } => operation,
            ProviderLaunchActorSettlement::Reaped {
                operation,
                process_identity_sha256,
                process_tree_terminated,
                leader_reaped,
            } => {
                valid_digest(process_identity_sha256)?;
                if !process_tree_terminated || !leader_reaped {
                    return Err(conflict());
                }
                if operation == "launch" {
                    launch_process_identity = Some(process_identity_sha256);
                }
                operation
            }
        };
        bounded(operation)?;
        if !operations.insert(operation.as_str()) {
            return Err(conflict());
        }
    }
    if operations != std::collections::HashSet::from(["describe", "policy", "launch"]) {
        return Err(conflict());
    }
    if proof.runtime_never_bound {
        if proof.runtime_process_identity_sha256.is_some()
            || launch_process_identity.is_some()
            || proof.runtime_terminal_code != "startup_failed"
        {
            return Err(conflict());
        }
    } else if proof.runtime_process_identity_sha256.as_ref() != launch_process_identity
        || launch_process_identity.is_none()
    {
        return Err(conflict());
    }
    let exact: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM provider_launch_attempts WHERE attempt_id=?1 AND runtime_generation_uuid=?2 AND return_channel_id=?3)",
        params![owner.attempt_id.to_string(),proof.runtime_generation_uuid.to_string(),proof.return_channel_id],|r|r.get(0)).map_err(sql_error)?;
    if !exact {
        return Err(conflict());
    }
    Ok(())
}

pub(super) fn validate_launch_schema(conn: &sqlite::Connection) -> Result<(), String> {
    // Compare the complete registered schema, including constraints, indexes and triggers.
    // Never repair a partial current schema.
    let expected = sqlite::Connection::open_in_memory().map_err(sql_error)?;
    expected.execute_batch("CREATE TABLE invocations(id INTEGER PRIMARY KEY,invocation_uuid TEXT,provider_name TEXT,provider_index INTEGER,status TEXT);").map_err(sql_error)?;
    expected
        .execute_batch(include_str!(
            "../../migrations/0023_provider_launch_lifecycle.sql"
        ))
        .map_err(sql_error)?;
    let mut statement = expected.prepare("SELECT type,name,sql FROM sqlite_master WHERE name LIKE 'provider_launch_%' OR name='provider_logical_launches' OR name='provider_logical_launch_immutable'").map_err(sql_error)?;
    let definitions = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(sql_error)?;
    for definition in definitions {
        let (kind, name, sql) = definition.map_err(sql_error)?;
        let actual: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type=?1 AND name=?2",
                params![kind, name],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if actual.as_deref() != Some(sql.as_str()) {
            return Err(format!("corrupt schema 23: missing or changed {name}"));
        }
    }
    Ok(())
}

pub(super) fn validate_invocation_mutation_authority(
    conn: &sqlite::Connection,
    row_id: i64,
    authority: InvocationMutationAuthority<'_>,
) -> Result<(), String> {
    validate_mutation_authority(conn, row_id, authority)?;
    if let InvocationMutationAuthority::ProviderLaunch(owner) = authority {
        let allowed: bool = conn.query_row("SELECT a.status='active' AND l.status IN ('active','cancelling') FROM provider_launch_attempts a
            JOIN provider_logical_launches l ON l.logical_launch_id=a.logical_launch_id WHERE a.attempt_id=?1",[owner.attempt_id.to_string()],|r|r.get(0)).map_err(sql_error)?;
        if !allowed {
            return Err("inactive_provider_launch_owner_fence".into());
        }
    }
    Ok(())
}

/// An accepted effect is promotion even if a caller omitted its observation callback.
/// Kept in the effect writer's transaction, so a rejected write cannot promote.
pub(super) fn promote_invocation_effect(
    conn: &sqlite::Connection,
    authority: InvocationMutationAuthority<'_>,
    promotion: ProviderLaunchPromotion,
    count: usize,
) -> Result<(), String> {
    let InvocationMutationAuthority::ProviderLaunch(owner) = authority else {
        return Ok(());
    };
    if count == 0 {
        return Ok(());
    }
    let count = i64::try_from(count).map_err(|_| conflict())?;
    let column = promotion.column();
    conn.execute(&format!("UPDATE provider_launch_attempts SET {column}=MAX({column},?1),effect_incapable_at=NULL WHERE attempt_id=?2"),params![count,owner.attempt_id.to_string()]).map_err(sql_error)?;
    Ok(())
}

/// Input to the request digest. Only the returned digest belongs in State DB.
#[derive(Debug, Serialize)]
pub struct ProviderLaunchRequestIdentity<'a> {
    pub model_name: &'a str,
    pub prompt_sha256: &'a str,
    pub prompt_mode: &'a str,
    pub extra_inputs: &'a std::collections::BTreeMap<String, serde_json::Value>,
    pub effective_cwd: &'a str,
    pub parent_invocation_uuid: Option<Uuid>,
    pub start_mode: ProviderLaunchStartMode,
    pub expected_provider_session_id: Option<&'a str>,
    pub mailbox_correlation: Option<&'a str>,
    pub candidates: &'a [ProviderLaunchCandidate],
}
impl ProviderLaunchRequestIdentity<'_> {
    pub fn sha256(&self) -> Result<String, String> {
        valid_digest(self.prompt_sha256)?;
        digest(&serde_json::json!([
            "request-identity-v1",
            self.model_name,
            self.prompt_sha256,
            self.prompt_mode,
            self.extra_inputs,
            self.effective_cwd,
            self.parent_invocation_uuid,
            self.start_mode,
            self.expected_provider_session_id,
            self.mailbox_correlation,
            digest(&self.candidates)?
        ]))
    }
}

/// Exact store namespaces and digest of the joined restart evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderLaunchRecoveryJoin {
    pub state_db_path: std::path::PathBuf,
    pub sidecar_path: std::path::PathBuf,
    pub evidence_sha256: String,
}

fn validate_retained_authority(
    conn: &sqlite::Connection,
    owner: &ProviderLaunchOwnerFence,
    authority: &CompletionRegistrationAuthority,
) -> Result<(), String> {
    let expected: String = conn.query_row("SELECT completion_registration_capability_digest FROM invocations WHERE id=?1 AND invocation_uuid=?2",
        params![owner.invocation_row_id,owner.invocation_uuid.to_string()],|row|row.get(0)).map_err(sql_error)?;
    let observed = authority.digest();
    if expected.len() != observed.len()
        || expected
            .bytes()
            .zip(observed.bytes())
            .fold(0u8, |different, (left, right)| different | (left ^ right))
            != 0
    {
        return Err("missing_retained_provider_launch_authority".into());
    }
    Ok(())
}
