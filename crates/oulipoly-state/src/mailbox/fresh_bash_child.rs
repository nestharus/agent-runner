/// A broker-minted child identity. Neither the request key nor the handle is
/// authority without the broker's pinned actor and released-root readback.
fn read_bash_json<T: serde::de::DeserializeOwned>(raw: &str, source: &str) -> Result<T, String> {
    use sha2::{Digest, Sha256};
    serde_json::from_str(raw).map_err(|error| {
        eprintln!(
            "oulipoly JSON artifact: stage=bash_child_readback source=v30/state.db:{source} bytes={} sha256={:x} cause={error}",
            raw.len(),
            Sha256::digest(raw.as_bytes())
        );
        "broker State JSON read failed".to_owned()
    })
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshBashChild {
    pub request_id: String,
    pub d_key: String,
    pub invocation_uuid: String,
    pub handle: String,
    pub root_handoff_id: String,
    pub root_id: String,
    pub parent_invocation_uuid: String,
    /// Broker-observed, consumed K for the work namespace containing Bash.
    /// These are immutable readback identities, never caller authority.
    pub parent_work_grant_id: String,
    pub parent_work_id: String,
    pub actor: FreshRecipientIdentity,
    pub registration_authority: String,
    #[serde(default = "default_bash_listener_policy", skip_serializing_if = "is_response_only_bash_listener")]
    pub listener_policy: String,
    pub session: FreshV30Session,
}

/// Broker-observed selected root K. These fields are evidence for a grant,
/// never selectors supplied by Bash.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootHSelectedK {
    pub grant_id: String,
    pub work_id: String,
    pub plan_sha256: String,
    pub account: String,
    pub model: String,
    pub provider_pid: i32,
    pub provider_starttime: u64,
    pub provider_boot_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootHDelegation {
    pub handoff_id: String,
    pub child_request_id: String,
    pub root_id: String,
    pub owner_generation: String,
    pub root_invocation_uuid: String,
    pub root_session_id: String,
    pub registration_authority_digest: String,
    pub root_work_authority_digest: String,
    pub root_endpoint: String,
    pub listener_policy: String,
    pub child_actor: FreshRecipientIdentity,
    pub selected_k: FreshRootHSelectedK,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshRootHConsumption {
    pub delegation: FreshRootHDelegation,
    pub registration_authority: String,
    pub root_work_authority: String,
}

fn default_bash_listener_policy() -> String { "response_only".into() }
fn is_response_only_bash_listener(value: &String) -> bool { value == "response_only" }

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FreshBashPrivateResult {
    pub request_id: String,
    pub grant_id: String,
    pub exit_code: i32,
    pub stdout_sha256: String,
    pub stdout_len: u64,
    pub stderr_sha256: String,
    pub stderr_len: u64,
}

impl FreshV30Lane {
    pub fn issue_root_h_delegation(
        &self, root: &FreshReleasedHandoff, root_actor: &FreshRecipientIdentity,
        child: &FreshBashChild, selected_k: FreshRootHSelectedK,
    ) -> Result<FreshRootHDelegation, String> {
        if !matches!(root.delegated_h_listener_policy.as_deref(), Some("response_only" | "notify"))
            || root.delegated_h_listener_policy.as_deref() != Some(child.listener_policy.as_str())
            || !matches!(root.root_work_intent, FreshRootWorkIntent::NormalCli(_))
            || child.root_handoff_id != root.handoff_id
            || child.parent_work_grant_id != selected_k.grant_id
            || child.parent_work_id != selected_k.work_id
            || selected_k.plan_sha256.len() != 64
            || selected_k.account.is_empty() || selected_k.model.is_empty()
            || selected_k.provider_pid <= 0 || selected_k.provider_starttime == 0
        {
            return Err("root H delegation source or selected K invalid".into());
        }
        let root_session = self.read_session(&root.d_key)?
            .ok_or("root H delegation original D absent")?;
        self.require_released_invocation(root, root_actor, &root_session)?;
        self.require_bash_child(child, root, root_actor, &child.actor)?;
        let authority = crate::CompletionRegistrationAuthority::from_process_environment_value(
            root.registration_authority.clone())?;
        let work_authority = root.delegated_root_work_authority.as_deref()
            .ok_or("root H original J work capability absent")?;
        let receipt = FreshRootHDelegation {
            handoff_id: root.handoff_id.clone(),
            child_request_id: child.request_id.clone(),
            root_id: root.old_release.prepared.root_id.clone(),
            owner_generation: root.old_release.prepared.owner_generation.clone(),
            root_invocation_uuid: root.invocation_uuid.clone(),
            root_session_id: root_session.session_id,
            registration_authority_digest: authority.digest(),
            root_work_authority_digest: format!("{:x}", Sha256::digest(work_authority.as_bytes())),
            root_endpoint: root.old_release.owner.endpoint.clone(),
            listener_policy: root.delegated_h_listener_policy.clone().unwrap(),
            child_actor: child.actor.clone(), selected_k,
        };
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state.execute_batch("PRAGMA synchronous=FULL").map_err(|e| e.to_string())?;
        state.execute(
            "INSERT INTO fresh_root_h_delegation(handoff_id,child_request_id,receipt_json,issued_at)
             VALUES(?1,?2,?3,?4)",
            params![receipt.handoff_id, receipt.child_request_id,
                serde_json::to_string(&receipt).map_err(|e| e.to_string())?, Utc::now().to_rfc3339()],
        ).map_err(|_| "root H delegation already issued or uncertain".to_string())?;
        drop(state);
        if self.read_root_h_delegation(&root.handoff_id)?.as_ref() != Some(&receipt) {
            return Err("root H delegation readback mismatch".into());
        }
        Ok(receipt)
    }

    pub fn read_root_h_delegation(&self, handoff_id: &str)
        -> Result<Option<FreshRootHDelegation>, String> {
        validate_request_id(handoff_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String,String)> = state.query_row(
            "SELECT child_request_id,receipt_json FROM fresh_root_h_delegation WHERE handoff_id=?1",
            [handoff_id], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional().map_err(|e| e.to_string())?;
        row.map(|(child_request_id,json)| {
            let receipt: FreshRootHDelegation = read_bash_json(&json,"fresh_root_h_delegation.receipt_json")?;
            if receipt.handoff_id != handoff_id || receipt.child_request_id != child_request_id {
                return Err("root H delegation identity changed".into());
            }
            Ok(receipt)
        }).transpose()
    }

    /// Read the spent authority without replaying its one-use consumption.
    /// Callers must separately prove the live actor and selected provider K.
    pub fn require_consumed_root_h_delegation(
        &self, root: &FreshReleasedHandoff, root_actor: &FreshRecipientIdentity,
        child: &FreshBashChild, selected_k: &FreshRootHSelectedK,
    ) -> Result<FreshRootHDelegation, String> {
        let receipt = self.read_root_h_delegation(&root.handoff_id)?
            .ok_or("root H delegation absent")?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let consumed: Option<String> = state.query_row(
            "SELECT child_request_id FROM fresh_root_h_consumption WHERE handoff_id=?1",
            [&root.handoff_id], |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?;
        let session = self.read_session(&root.d_key)?.ok_or("root H D absent")?;
        self.require_released_invocation(root, root_actor, &session)?;
        self.require_bash_child(child, root, root_actor, &child.actor)?;
        let authority = crate::CompletionRegistrationAuthority::from_process_environment_value(
            root.registration_authority.clone())?;
        let work_authority = root.delegated_root_work_authority.as_deref()
            .ok_or("root H original J work capability absent")?;
        if consumed.as_deref() != Some(child.request_id.as_str())
            || receipt.child_request_id != child.request_id
            || receipt.child_actor != child.actor
            || receipt.selected_k != *selected_k
            || receipt.root_id != root.old_release.prepared.root_id
            || receipt.owner_generation != root.old_release.prepared.owner_generation
            || receipt.root_invocation_uuid != root.invocation_uuid
            || receipt.root_session_id != session.session_id
            || receipt.registration_authority_digest != authority.digest()
            || receipt.root_work_authority_digest != format!("{:x}", Sha256::digest(work_authority.as_bytes()))
            || receipt.root_endpoint != root.old_release.owner.endpoint
            || root.delegated_h_listener_policy.as_deref() != Some(receipt.listener_policy.as_str())
            || receipt.listener_policy != child.listener_policy
        {
            return Err("consumed root H delegation binding changed".into());
        }
        Ok(receipt)
    }

    pub fn consume_root_h_delegation(
        &self, root: &FreshReleasedHandoff, root_actor: &FreshRecipientIdentity,
        child: &FreshBashChild, selected_k: &FreshRootHSelectedK,
    ) -> Result<FreshRootHConsumption, String> {
        let receipt = self.read_root_h_delegation(&root.handoff_id)?
            .ok_or("root H delegation absent")?;
        if receipt.child_request_id != child.request_id || receipt.child_actor != child.actor
            || receipt.selected_k != *selected_k || receipt.root_id != child.root_id
            || receipt.owner_generation != root.old_release.prepared.owner_generation
            || receipt.root_invocation_uuid != root.invocation_uuid
            || root.delegated_h_listener_policy.as_deref() != Some(receipt.listener_policy.as_str())
        {
            return Err("root H delegation actor, source or K changed".into());
        }
        let session = self.read_session(&root.d_key)?.ok_or("root H D absent")?;
        self.require_released_invocation(root, root_actor, &session)?;
        self.require_bash_child(child, root, root_actor, &child.actor)?;
        let authority = crate::CompletionRegistrationAuthority::from_process_environment_value(
            root.registration_authority.clone())?;
        let work_authority = root.delegated_root_work_authority.as_deref()
            .ok_or("root H original J work capability absent")?;
        if receipt.root_session_id != session.session_id
            || receipt.registration_authority_digest != authority.digest()
            || receipt.root_work_authority_digest != format!("{:x}", Sha256::digest(work_authority.as_bytes()))
            || receipt.root_endpoint != root.old_release.owner.endpoint {
            return Err("root H authority changed".into());
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state.execute_batch("PRAGMA synchronous=FULL").map_err(|e| e.to_string())?;
        state.execute(
            "INSERT INTO fresh_root_h_consumption(handoff_id,child_request_id,consumed_at)
             VALUES(?1,?2,?3)",
            params![receipt.handoff_id, child.request_id, Utc::now().to_rfc3339()],
        ).map_err(|_| "root H delegation already consumed or uncertain".to_string())?;
        Ok(FreshRootHConsumption { delegation: receipt,
            registration_authority: root.registration_authority.clone(),
            root_work_authority: work_authority.to_owned() })
    }

    /// Fixture-only caller. An accepted request may be executed once only
    /// after this committed grant is returned. A lost grant reply is unknown
    /// and must never be turned into a second launch.
    pub fn admit_private_bash_work(&self, child: &FreshBashChild) -> Result<String, String> {
        if self.require_complete_bash_child(&child.request_id)? != *child {
            return Err("Bash private work lacks complete child registration".into());
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state.execute_batch("PRAGMA synchronous=FULL").map_err(|e| e.to_string())?;
        let grant = Uuid::new_v4().to_string();
        state.execute(
            "INSERT INTO fresh_bash_private_work(request_id,grant_id,admitted_at)
             VALUES(?1,?2,?3)",
            params![child.request_id, grant, Utc::now().to_rfc3339()],
        ).map_err(|_| "Bash private work already admitted or uncertain".to_string())?;
        drop(state);
        let read = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let found: String = read.query_row(
            "SELECT grant_id FROM fresh_bash_private_work WHERE request_id=?1",
            [&child.request_id], |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if found != grant {
            return Err("Bash private work grant readback conflict".into());
        }
        Ok(grant)
    }

    pub fn record_private_bash_result(&self, result: &FreshBashPrivateResult) -> Result<(), String> {
        validate_request_id(&result.request_id)?;
        validate_request_id(&result.grant_id)?;
        self.require_complete_bash_child(&result.request_id)?;
        if result.stdout_len > i64::MAX as u64 || result.stderr_len > i64::MAX as u64 {
            return Err("Bash private result length overflow".into());
        }
        for digest in [&result.stdout_sha256, &result.stderr_sha256] {
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
                return Err("Bash private result digest invalid".into());
            }
        }
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state.execute_batch("PRAGMA synchronous=FULL").map_err(|e| e.to_string())?;
        let accepted: Option<String> = state.query_row(
            "SELECT grant_id FROM fresh_bash_private_work WHERE request_id=?1",
            [&result.request_id], |r| r.get(0),
        ).optional().map_err(|e| e.to_string())?;
        if accepted.as_deref() != Some(result.grant_id.as_str()) {
            return Err("Bash private result has no exact grant".into());
        }
        state.execute(
            "INSERT INTO fresh_bash_private_result
             (request_id,grant_id,exit_code,stdout_sha256,stdout_len,stderr_sha256,stderr_len,observed_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",
            params![result.request_id,result.grant_id,result.exit_code,result.stdout_sha256,
                result.stdout_len as i64,result.stderr_sha256,result.stderr_len as i64,
                Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        drop(state);
        if self.read_private_bash_result(&result.request_id)?.as_ref() != Some(result) {
            return Err("Bash private result collision or readback conflict".into());
        }
        Ok(())
    }

    pub fn read_private_bash_result(&self, request_id: &str) -> Result<Option<FreshBashPrivateResult>, String> {
        validate_request_id(request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let found = state.query_row(
            "SELECT request_id,grant_id,exit_code,stdout_sha256,stdout_len,stderr_sha256,stderr_len
             FROM fresh_bash_private_result WHERE request_id=?1",
            [request_id], |r| Ok(FreshBashPrivateResult {
                request_id:r.get(0)?,grant_id:r.get(1)?,exit_code:r.get(2)?,
                stdout_sha256:r.get(3)?,stdout_len:r.get::<_,i64>(4)? as u64,
                stderr_sha256:r.get(5)?,stderr_len:r.get::<_,i64>(6)? as u64,
            }),
        ).optional().map_err(|e| e.to_string())?;
        if found.as_ref().is_some_and(|row| row.stdout_len > i64::MAX as u64 || row.stderr_len > i64::MAX as u64) {
            return Err("Bash private result negative length".into());
        }
        Ok(found)
    }

    pub fn released_handoff_for_root(
        &self,
        root_id: &str,
    ) -> Result<(FreshReleasedHandoff, FreshRecipientIdentity), String> {
        validate_request_id(root_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: (String, String) = state.query_row(
            "SELECT receipt_json,actor_identity FROM fresh_released_handoff WHERE root_id=?1",
            [root_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|e| e.to_string())?
            .ok_or("released root absent for Bash child")?;
        let receipt: FreshReleasedHandoff = read_bash_json(&row.0, "fresh_released_handoff.receipt_json")?;
        let actor: FreshRecipientIdentity = read_bash_json(&row.1, "fresh_released_handoff.actor_identity")?;
        self.require_released_handoff(&receipt.d_key, &receipt, &actor)?;
        Ok((receipt, actor))
    }

    /// The broker first proves the connected process's image and process-tree
    /// scope. This method binds that pinned actor to one immutable child row,
    /// one separate D/session, one invocation with the exact root parent, and
    /// a separate registration secret. A partial commit is retried by key;
    /// no alternate child or work is authorized by a missing reply.
    pub fn admit_bash_child(
        &mut self,
        request_id: &str,
        root: &FreshReleasedHandoff,
        root_actor: &FreshRecipientIdentity,
        actor: &FreshRecipientIdentity,
        parent_work_grant_id: &str,
        parent_work_id: &str,
        listener_policy: FreshBashListenerPolicy,
    ) -> Result<FreshBashChild, String> {
        validate_request_id(request_id)?;
        validate_request_id(parent_work_grant_id)?;
        validate_request_id(parent_work_id)?;
        if actor.host_pid <= 0 || actor.starttime_ticks == 0 || actor.boot_id.is_empty()
            || actor == root_actor
        {
            return Err("invalid or reused Bash child actor".into());
        }
        let root_session = self.read_session(&root.d_key)?
            .ok_or("released root D absent before Bash child")?;
        self.require_released_invocation(root, root_actor, &root_session)?;
        let draft = FreshBashChild {
            request_id: request_id.into(),
            d_key: Uuid::new_v4().to_string(),
            invocation_uuid: Uuid::new_v4().to_string(),
            handle: format!("ab30_{}", Uuid::new_v4().simple()),
            root_handoff_id: root.handoff_id.clone(),
            root_id: root.old_release.prepared.root_id.clone(),
            parent_invocation_uuid: root.invocation_uuid.clone(),
            parent_work_grant_id: parent_work_grant_id.into(),
            parent_work_id: parent_work_id.into(),
            actor: actor.clone(),
            registration_authority: crate::CompletionRegistrationAuthority::generate()?
                .process_environment_value().into(),
            listener_policy: listener_policy.as_str().into(),
            // Stored separately by D. This field is filled only after exact
            // State and sidecar readback, never inserted from caller JSON.
            session: FreshV30Session {
                lane_id: String::new(), source_generation: String::new(),
                session_id: String::new(), request_id: String::new(), allocation_id: String::new(),
            },
        };
        let actor_json = serde_json::to_string(actor).map_err(|e| e.to_string())?;
        let draft_json = serde_json::to_string(&draft).map_err(|e| e.to_string())?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        state.execute_batch("PRAGMA synchronous=FULL").map_err(|e| e.to_string())?;
        state.execute(
            "INSERT INTO fresh_bash_child
             (request_id,d_key,invocation_uuid,handle,root_handoff_id,root_id,
              parent_invocation_uuid,actor_identity,receipt_json,admitted_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10) ON CONFLICT DO NOTHING",
            params![draft.request_id,draft.d_key,draft.invocation_uuid,draft.handle,
                draft.root_handoff_id,draft.root_id,draft.parent_invocation_uuid,
                actor_json,draft_json,Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        drop(state);
        let stored = self.read_bash_child(request_id)?
            .ok_or("Bash child request collision or uncertain insert")?;
        if stored.root_handoff_id != root.handoff_id
            || stored.root_id != root.old_release.prepared.root_id
            || stored.parent_invocation_uuid != root.invocation_uuid
            || stored.invocation_uuid == root.invocation_uuid
            || stored.d_key == root.d_key
            || stored.actor != *actor
            || stored.parent_work_grant_id != parent_work_grant_id
            || stored.parent_work_id != parent_work_id
            || stored.listener_policy != listener_policy.as_str()
        {
            return Err("Bash child actor or parent conflict".into());
        }
        let session = self.allocate_session(&stored.d_key)?;
        self.ensure_bash_child_invocation(&stored, root, &session)?;
        let mut result = stored;
        result.session = session;
        self.require_bash_child(&result, root, root_actor, actor)?;
        Ok(result)
    }

    /// Readback alone never admits a new child or resumes work. The broker
    /// rechecks this row against the same connected process on every request.
    pub fn read_bash_child(&self, request_id: &str) -> Result<Option<FreshBashChild>, String> {
        validate_request_id(request_id)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String,String,String,String,String,String,String,String)> = state.query_row(
            "SELECT d_key,invocation_uuid,handle,root_handoff_id,root_id,
                    parent_invocation_uuid,actor_identity,receipt_json
             FROM fresh_bash_child WHERE request_id=?1",
            [request_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,
                row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?)),
        ).optional().map_err(|e| e.to_string())?;
        let Some((d_key, invocation_uuid, handle, root_handoff_id, root_id,
            parent_invocation_uuid, actor_json, receipt_json)) = row else {
            return Ok(None);
        };
        let receipt: FreshBashChild = read_bash_json(&receipt_json, "fresh_bash_child.receipt_json")?;
        if receipt.request_id != request_id || receipt.d_key != d_key
            || receipt.invocation_uuid != invocation_uuid || receipt.handle != handle
            || receipt.root_handoff_id != root_handoff_id || receipt.root_id != root_id
            || receipt.parent_invocation_uuid != parent_invocation_uuid
            || serde_json::to_string(&receipt.actor).map_err(|e| e.to_string())? != actor_json
        {
            return Err("Bash child exact row/receipt conflict".into());
        }
        Ok(Some(receipt))
    }

    fn require_complete_bash_child(&self, request_id: &str) -> Result<FreshBashChild, String> {
        let mut child = self.read_bash_child(request_id)?
            .ok_or("Bash child registration absent")?;
        child.session = self.read_session(&child.d_key)?
            .ok_or("Bash child D incomplete")?;
        let (root, root_actor) = self.released_handoff_for_root(&child.root_id)?;
        self.require_bash_child(&child, &root, &root_actor, &child.actor)?;
        Ok(child)
    }

    fn ensure_bash_child_invocation(
        &self, child: &FreshBashChild, root: &FreshReleasedHandoff,
        session: &FreshV30Session,
    ) -> Result<(), String> {
        let authority = crate::CompletionRegistrationAuthority::from_process_environment_value(
            child.registration_authority.clone())?;
        let state = StateDb::open(&self.state_path)?;
        let parent = state.get_invocation_by_uuid(&root.invocation_uuid)?
            .ok_or("released root invocation absent")?;
        if state.get_invocation_by_uuid(&child.invocation_uuid)?.is_none() {
            state.start_invocation_with_prepared_completion_registration_authority(
                &crate::InvocationStart {
                    invocation_uuid: child.invocation_uuid.clone(),
                    model_name: "agent-bash-child".into(),
                    provider_name: "agent-bash".into(),
                    provider_index: 0,
                    parent_invocation_id: Some(parent.id),
                }, &authority,
            )?;
        }
        let row = state.get_invocation_by_uuid(&child.invocation_uuid)?
            .ok_or("Bash child invocation absent after start")?;
        if row.parent_invocation_id != Some(parent.id) || row.model_name != "agent-bash-child"
            || row.provider_name.as_deref() != Some("agent-bash") || row.provider_index != 0
            || row.session_id.as_deref().is_some_and(|id| id != session.session_id)
        {
            return Err("Bash child invocation parent or model conflict".into());
        }
        state.bind_invocation_provider_session_start(
            crate::InvocationMutationAuthority::Standalone, row.id,
            &crate::ProviderSessionBinding {
                provider_session_id: session.session_id.clone(),
                capture_method: "broker-bash-child-v30",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )?;
        state.mark_fresh_v30_exact_registration(
            &child.invocation_uuid, "broker-bash-child-v30", &authority,
        )?;
        self.require_bash_child_invocation(child, root, session, &authority)
    }

    pub fn require_bash_child(
        &self, child: &FreshBashChild, root: &FreshReleasedHandoff,
        root_actor: &FreshRecipientIdentity, actor: &FreshRecipientIdentity,
    ) -> Result<(), String> {
        let root_session = self.read_session(&root.d_key)?
            .ok_or("released root D absent")?;
        self.require_released_invocation(root, root_actor, &root_session)?;
        if child.actor != *actor || child.root_handoff_id != root.handoff_id
            || child.root_id != root.old_release.prepared.root_id
            || child.parent_invocation_uuid != root.invocation_uuid
            || validate_request_id(&child.parent_work_grant_id).is_err()
            || validate_request_id(&child.parent_work_id).is_err()
            || child.invocation_uuid == root.invocation_uuid
            || child.d_key == root.d_key
            || child.session.session_id == root_session.session_id
            || child.session.request_id != child.d_key
            || !child.handle.starts_with("ab30_")
            || !matches!(child.listener_policy.as_str(), "response_only" | "notify")
        {
            return Err("Bash child actor/root/session conflict".into());
        }
        let stored = self.read_bash_child(&child.request_id)?
            .ok_or("Bash child receipt absent")?;
        let mut expected = child.clone();
        expected.session = stored.session.clone();
        if stored != expected {
            return Err("Bash child receipt changed".into());
        }
        self.require_session(&child.session)?;
        let authority = crate::CompletionRegistrationAuthority::from_process_environment_value(
            child.registration_authority.clone())?;
        self.require_bash_child_invocation(child, root, &child.session, &authority)
    }

    fn require_bash_child_invocation(
        &self, child: &FreshBashChild, root: &FreshReleasedHandoff,
        session: &FreshV30Session, authority: &crate::CompletionRegistrationAuthority,
    ) -> Result<(), String> {
        self.require_session(session)?;
        let state = self.state_connection(OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let row: Option<(String, String, i64, Option<i64>, Option<String>, Option<String>)> = state.query_row(
            "SELECT c.model_name,c.provider_name,c.provider_index,c.parent_invocation_id,
                    c.provider_session_id,c.completion_registration_capability_digest
             FROM invocations c WHERE c.invocation_uuid=?1",
            [&child.invocation_uuid],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
        ).optional().map_err(|e| e.to_string())?;
        let parent_id: i64 = state.query_row(
            "SELECT id FROM invocations WHERE invocation_uuid=?1",
            [&root.invocation_uuid], |r| r.get(0),
        ).map_err(|e| e.to_string())?;
        if row != Some(("agent-bash-child".into(),"agent-bash".into(),0,
            Some(parent_id),Some(session.session_id.clone()),Some(authority.digest()))) {
            return Err("Bash child invocation/D/registration readback conflict".into());
        }
        Ok(())
    }
}

fn fresh_bash_child_schema_count(state: &Connection) -> Result<i64, String> {
    state.query_row(
        "SELECT count(*) FROM sqlite_master WHERE
         (type='table' AND name IN ('fresh_bash_child','fresh_bash_private_work','fresh_bash_private_result')) OR
         (type='trigger' AND name IN ('fresh_bash_child_no_update','fresh_bash_child_no_delete',
          'fresh_bash_private_work_no_update','fresh_bash_private_work_no_delete',
          'fresh_bash_private_result_no_update','fresh_bash_private_result_no_delete'))",
        [], |r| r.get(0),
    ).map_err(|e| e.to_string())
}

fn verify_fresh_bash_child_schema(state: &Connection) -> Result<(), String> {
    fn objects(state: &Connection) -> Result<Vec<(String, String, String)>, String> {
        let mut statement = state.prepare(
            "SELECT type,name,sql FROM sqlite_master WHERE
             (type='table' AND name IN ('fresh_bash_child','fresh_bash_private_work','fresh_bash_private_result')) OR
             (type='trigger' AND tbl_name IN ('fresh_bash_child','fresh_bash_private_work','fresh_bash_private_result'))
             ORDER BY type,name",
        ).map_err(|e| e.to_string())?;
        statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    let canonical = Connection::open_in_memory().map_err(|e| e.to_string())?;
    canonical.execute_batch(FRESH_BASH_CHILD_SCHEMA).map_err(|e| e.to_string())?;
    if objects(state)? != objects(&canonical)? {
        return Err("fresh Bash child schema differs from embedded SQL".into());
    }
    Ok(())
}
